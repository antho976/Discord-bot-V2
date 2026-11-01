//! New Twitch clips of the followed channels, posted to a chosen channel. Helix `/clips` is read
//! every 15 minutes per broadcaster, only for servers that turned clips on; each clip is stored
//! once by its id, so a clip is never posted twice. When a broadcaster hasn't been checked for a
//! while (clips were off, or the channel left the Twitch tab), watching starts over from now, so
//! turning clips back on doesn't post the clips made in between. When the whole bot was down
//! instead, the next check picks up where the last good read left off.

use std::sync::Arc;

use bot_core::anyhow::Result;
use bot_core::rusqlite::params;
use bot_core::{Ctx, jiff, now_ms, text, tracing};
use serde::Deserialize;

use crate::twitch::{self, List};
use crate::{Settings, due, last_due, state_get, state_set};

const EVERY_MS: i64 = 15 * 60_000;
/// Twitch can take a while to list a new clip, so each check looks back an hour past the last
/// one. Clips seen already are skipped by id.
const OVERLAP_MS: i64 = 60 * 60_000;
/// Twitch caps a clip search at a week from its start.
const MAX_SPAN_MS: i64 = 6 * 86_400_000;
/// A broadcaster not checked for this long wasn't being watched, so watching starts over. A gap
/// this long between alerts ticks means the bot itself was down.
const STALE_MS: i64 = 2 * EVERY_MS;
/// The state row stamped by every alerts tick, so a check can tell a bot outage from clips
/// being off.
const TICK_KEY: &str = "clips:tick";
/// At most this many clips go out per check (the newest), so a catch-up can't flood a channel.
const MAX_POSTS: usize = 5;
/// Helix returns clips by views, 100 a page; this many pages are read per check.
const MAX_PAGES: usize = 3;
/// Stored clips are dropped this long after they were first seen. Far past the six-day search,
/// so a dropped clip can't come back and post again, and long enough for the 30-day count.
const KEEP_MS: i64 = 31 * 86_400_000;

#[derive(Debug, Clone, Deserialize)]
struct HClip {
    id: String,
    url: String,
    #[serde(default)]
    broadcaster_id: String,
    #[serde(default)]
    creator_name: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    view_count: i64,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    thumbnail_url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    pub id: String,
    pub title: String,
    pub creator: String,
    pub url: String,
    pub thumbnail: String,
    pub views: i64,
    pub created_at: i64,
}

/// Where a check's search starts: an hour before the last good check, never before the bot
/// started watching (so old clips aren't posted), and within Twitch's week.
pub fn window_start(since: i64, last_ok: Option<i64>, now: i64) -> i64 {
    last_ok.map_or(since, |l| l - OVERLAP_MS).max(since).max(now - MAX_SPAN_MS)
}

/// What a check of one broadcaster does.
#[derive(Debug, PartialEq)]
pub enum Plan {
    /// Mark now as where watching starts, and post nothing.
    Start,
    /// Search for clips made from `from`, and post the new ones made since watching started.
    Search { since: i64, from: i64 },
}

/// `since` is when watching started, `last_run` the previous check of this broadcaster (whether
/// or not Twitch answered), `last_ok` the last time Twitch answered and `last_tick` the alerts
/// tick before this one.
///
/// A broadcaster the running bot stopped checking (clips were off, or the channel left the
/// Twitch tab) starts over, so clips made in that gap are never posted. When the ticks stopped
/// too, the whole bot was down: only the time up to its last tick counts, so a broadcaster it was
/// still checking then searches on from the last good read. `MAX_POSTS` and Twitch's six days
/// bound that catch-up.
pub fn plan(since: Option<i64>, last_run: Option<i64>, last_ok: Option<i64>, last_tick: Option<i64>, now: i64) -> Plan {
    let running_until = last_tick.filter(|t| now - t > STALE_MS).unwrap_or(now);
    match (since, last_run) {
        (Some(since), Some(run)) if running_until - run <= STALE_MS => Plan::Search { since, from: window_start(since, last_ok, now) },
        _ => Plan::Start,
    }
}

/// Stamp this alerts tick and return the one before it. Called first thing on every tick, so
/// the stamp only goes stale while the bot is down.
pub fn stamp_tick(ctx: &Ctx) -> Option<i64> {
    let last = state_get(ctx, TICK_KEY).and_then(|v| v.parse().ok());
    state_set(ctx, TICK_KEY, &now_ms().to_string());
    last
}

/// The clips to post from what a check found (oldest first): the newest few, oldest first.
fn to_post(fresh: &[Clip]) -> &[Clip] {
    &fresh[fresh.len().saturating_sub(MAX_POSTS)..]
}

/// The first line of a clip post, as Discord shows it.
pub fn headline(c: &Clip) -> String {
    line(c, |s| s.to_owned())
}

/// What a clip post says. The title and creator are set by viewers, so their markdown is
/// escaped. Discord turns the link into a player.
pub fn post_text(c: &Clip) -> String {
    format!("{}\n{}", line(c, text::escape_markdown), c.url)
}

fn line(c: &Clip, show: impl Fn(&str) -> String) -> String {
    let title = if c.title.trim().is_empty() { "Untitled clip".to_owned() } else { show(c.title.trim()) };
    if c.creator.is_empty() { format!("New clip: {title}") } else { format!("New clip by {}: {title}", show(&c.creator)) }
}

/// Servers posting clips, with their settings.
fn clip_guilds(guilds: &[(u64, Arc<Settings>)]) -> Vec<&(u64, Arc<Settings>)> {
    guilds.iter().filter(|(_, s)| s.clips && s.clips_channel.is_some() && !s.twitch_logins.is_empty()).collect()
}

fn rfc3339(ms: i64) -> String {
    jiff::Timestamp::from_second(ms / 1000).map(|t| t.to_string()).unwrap_or_default()
}

/// Called every tick: look for new clips of each broadcaster a clip-posting server follows,
/// every 15 minutes, and post them. `last_tick` is what `stamp_tick` returned this tick.
pub async fn check(ctx: &Ctx, guilds: &[(u64, Arc<Settings>)], last_tick: Option<i64>) {
    let posting = clip_guilds(guilds);
    if posting.is_empty() {
        return;
    }
    let all = twitch::broadcasters(ctx);
    for b in all.iter().filter(|b| posting.iter().any(|(_, s)| s.twitch_logins.contains(&b.login))) {
        let key = format!("clips:{}", b.user_id);
        let last_run = last_due(ctx, &key);
        if !due(ctx, &key, EVERY_MS - 30_000) {
            continue;
        }
        let now = now_ms();
        let since_key = format!("clips_since:{}", b.user_id);
        let since = state_get(ctx, &since_key).and_then(|v| v.parse::<i64>().ok());
        let last_ok = state_get(ctx, &format!("clips_ok:{}", b.user_id)).and_then(|v| v.parse().ok());
        let (since, from) = match plan(since, last_run, last_ok, last_tick, now) {
            Plan::Start => {
                state_set(ctx, &since_key, &now.to_string());
                continue;
            }
            Plan::Search { since, from } => (since, from),
        };
        let found = match fetch(ctx, &b.user_id, from, now).await {
            Ok(found) => found,
            Err(e) => {
                tracing::warn!(login = %b.login, error = %e, "couldn't read Twitch clips");
                continue;
            }
        };
        state_set(ctx, &format!("clips_ok:{}", b.user_id), &now.to_string());
        let fresh = match store(ctx, &b.user_id, &found, since) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(login = %b.login, error = %e, "couldn't store Twitch clips");
                continue;
            }
        };
        let posts = to_post(&fresh);
        if posts.len() < fresh.len() {
            tracing::info!(login = %b.login, found = fresh.len(), "posting only the newest Twitch clips");
        }
        for c in posts {
            let text = post_text(c);
            for (guild, s) in posting.iter().filter(|(_, s)| s.twitch_logins.contains(&b.login)) {
                let Some(channel) = s.clips_channel else { continue };
                if let Err(e) = ctx.say(channel, &text).await {
                    tracing::warn!(guild, channel, error = %e, "couldn't post a Twitch clip");
                }
            }
        }
    }
}

/// Clips made from `from` until now, up to `MAX_PAGES` pages of them.
async fn fetch(ctx: &Ctx, bid: &str, from: i64, now: i64) -> Result<Vec<HClip>> {
    let mut found = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut q = vec![
            ("broadcaster_id", bid.to_owned()),
            ("started_at", rfc3339(from)),
            ("ended_at", rfc3339(now + 60_000)),
            ("first", "100".into()),
        ];
        q.extend(after.take().map(|a| ("after", a)));
        let Some(page) = twitch::get::<List<HClip>>(ctx, "clips", &q).await? else { break };
        after = page.cursor();
        found.extend(page.data);
        if after.is_none() {
            break;
        }
    }
    Ok(found)
}

/// Insert clips not seen before and return the ones made since watching started, oldest first.
/// Drops clips first seen more than `KEEP_MS` ago.
fn store(ctx: &Ctx, bid: &str, found: &[HClip], since: i64) -> Result<Vec<Clip>> {
    let now = now_ms();
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    let mut fresh = Vec::new();
    for h in found.iter().filter(|h| h.broadcaster_id.is_empty() || h.broadcaster_id == bid) {
        let created = h.created_at.parse::<jiff::Timestamp>().map(|t| t.as_millisecond()).unwrap_or(now);
        let c = Clip {
            id: h.id.clone(),
            title: text::clip(&h.title, 200),
            creator: h.creator_name.clone(),
            url: h.url.clone(),
            thumbnail: h.thumbnail_url.clone(),
            views: h.view_count,
            created_at: created,
        };
        let n = tx.execute(
            "INSERT OR IGNORE INTO alerts_clips (id, broadcaster_id, title, creator, url, thumbnail, views, created_at, seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![c.id, bid, c.title, c.creator, c.url, c.thumbnail, c.views, c.created_at, now],
        )?;
        if n == 0 {
            tx.execute("UPDATE alerts_clips SET views = ?2 WHERE id = ?1", params![c.id, c.views])?;
        } else if c.created_at >= since {
            fresh.push(c);
        }
    }
    tx.execute("DELETE FROM alerts_clips WHERE broadcaster_id = ?1 AND seen_at < ?2", params![bid, now - KEEP_MS])?;
    tx.commit()?;
    fresh.sort_by_key(|c| c.created_at);
    Ok(fresh)
}

/// The dashboard's "Send a test post" for clips: the newest stored clip, posted now.
pub async fn test_post(ctx: &Ctx, s: &Settings) -> Result<()> {
    let channel = s.clips_channel.ok_or_else(|| bot_core::anyhow::anyhow!("no channel"))?;
    let ids: Vec<String> = twitch::for_settings(ctx, s).into_iter().map(|b| b.user_id).collect();
    let Some(c) = recent(ctx, &ids, 1).into_iter().next() else { bot_core::anyhow::bail!("no clips") };
    ctx.say(channel, &format!("{}\n-# Test post.", post_text(&c))).await?;
    Ok(())
}

/// The newest stored clips of these broadcasters.
pub fn recent(ctx: &Ctx, ids: &[String], limit: i64) -> Vec<Clip> {
    if ids.is_empty() {
        return Vec::new();
    }
    let marks = vec!["?"; ids.len()].join(", ");
    let sql = format!(
        "SELECT id, title, creator, url, thumbnail, views, created_at FROM alerts_clips WHERE broadcaster_id IN ({marks}) ORDER BY created_at DESC LIMIT {limit}"
    );
    ctx.db
        .with(|c| {
            let mut s = c.prepare(&sql)?;
            s.query_map(bot_core::rusqlite::params_from_iter(ids.iter()), |r| {
                Ok(Clip { id: r.get(0)?, title: r.get(1)?, creator: r.get(2)?, url: r.get(3)?, thumbnail: r.get(4)?, views: r.get(5)?, created_at: r.get(6)? })
            })?
            .collect()
        })
        .unwrap_or_default()
}

/// How many stored clips of these broadcasters were made since `since`.
pub fn count_since(ctx: &Ctx, ids: &[String], since: i64) -> i64 {
    if ids.is_empty() {
        return 0;
    }
    let marks = vec!["?"; ids.len()].join(", ");
    let sql = format!("SELECT COUNT(*) FROM alerts_clips WHERE broadcaster_id IN ({marks}) AND created_at >= {since}");
    ctx.db.with(|c| c.query_row(&sql, bot_core::rusqlite::params_from_iter(ids.iter()), |r| r.get(0))).unwrap_or(0)
}

/// When clips of a broadcaster were last read from Twitch.
pub fn last_check(ctx: &Ctx, bid: &str) -> Option<i64> {
    state_get(ctx, &format!("clips_ok:{bid}")).and_then(|v| v.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_overlap_but_never_reach_back_past_the_start() {
        let min = 60_000;
        let now = 100_000 * min;
        let since = now - 300 * min;
        // An hour before the last check.
        assert_eq!(window_start(since, Some(now - 15 * min), now), now - 75 * min);
        // Not before watching started.
        assert_eq!(window_start(now - 30 * min, Some(now - 15 * min), now), now - 30 * min);
        // No good check yet: from the start.
        assert_eq!(window_start(since, None, now), since);
        // After a long outage, only Twitch's last six days.
        assert_eq!(window_start(now - 30 * 1440 * min, Some(now - 20 * 1440 * min), now), now - MAX_SPAN_MS);
    }

    #[test]
    fn a_broadcaster_nobody_watched_lately_starts_over() {
        let min = 60_000;
        let now = 100_000 * min;
        let since = now - 300 * min;
        // The bot ticked a minute ago: it has been running all along.
        let tick = Some(now - min);
        // Never checked, or never started: mark the start.
        assert_eq!(plan(None, None, None, tick, now), Plan::Start);
        assert_eq!(plan(None, Some(now - 15 * min), None, tick, now), Plan::Start);
        assert_eq!(plan(Some(since), None, Some(now - 15 * min), tick, now), Plan::Start);
        // Checked on the last round: search from an hour before the last good read.
        assert_eq!(plan(Some(since), Some(now - 15 * min), Some(now - 15 * min), tick, now), Plan::Search { since, from: now - 75 * min });
        // Twitch failing for a while doesn't start over: the check still ran, so the search
        // reaches back to the last good read.
        assert_eq!(plan(Some(since), Some(now - 15 * min), Some(now - 120 * min), tick, now), Plan::Search { since, from: now - 180 * min });
        // Clips were off for a day (or the channel left the Twitch tab): start over, so nothing
        // from the gap is posted.
        assert_eq!(plan(Some(now - 10 * 1440 * min), Some(now - 1440 * min), Some(now - 1440 * min), tick, now), Plan::Start);
        assert_eq!(plan(Some(since), Some(now - 31 * min), Some(now - 31 * min), tick, now), Plan::Start);
        // No tick stamp yet (the first tick after this shipped): the same as running all along.
        assert_eq!(plan(Some(since), Some(now - 31 * min), Some(now - 31 * min), None, now), Plan::Start);
    }

    #[test]
    fn after_a_bot_outage_checks_pick_up_where_they_left_off() {
        let min = 60_000;
        let now = 100_000 * min;
        let since = now - 3000 * min;
        // Down for two hours: the last tick, check and good read were just before it stopped.
        let down = now - 120 * min;
        assert_eq!(plan(Some(since), Some(down - 5 * min), Some(down - 5 * min), Some(down), now), Plan::Search { since, from: down - 65 * min });
        // Down for two weeks: still searched, within Twitch's six days.
        let down = now - 14 * 1440 * min;
        let long_since = now - 30 * 1440 * min;
        assert_eq!(plan(Some(long_since), Some(down - 5 * min), Some(down - 5 * min), Some(down), now), Plan::Search { since: long_since, from: now - MAX_SPAN_MS });
        // Clips were already off for a day when the bot went down: start over.
        let down = now - 60 * min;
        assert_eq!(plan(Some(since), Some(down - 1440 * min), Some(down - 1440 * min), Some(down), now), Plan::Start);
        // Never started: mark the start.
        assert_eq!(plan(None, None, None, Some(down), now), Plan::Start);
    }

    #[test]
    fn a_check_posts_only_the_newest_five() {
        let clip = |n: i64| Clip { id: n.to_string(), title: String::new(), creator: String::new(), url: String::new(), thumbnail: String::new(), views: 0, created_at: n };
        let fresh: Vec<Clip> = (1..=8).map(clip).collect();
        let ids: Vec<i64> = to_post(&fresh).iter().map(|c| c.created_at).collect();
        assert_eq!(ids, vec![4, 5, 6, 7, 8]);
        assert_eq!(to_post(&fresh[..2]).len(), 2);
        assert!(to_post(&[]).is_empty());
    }

    #[test]
    fn posts_read_plainly() {
        let c = Clip { id: "a".into(), title: " Gold stake ".into(), creator: "kiwi".into(), url: "https://clips.twitch.tv/a".into(), thumbnail: String::new(), views: 0, created_at: 0 };
        assert_eq!(post_text(&c), "New clip by kiwi: Gold stake\nhttps://clips.twitch.tv/a");
        let c = Clip { title: String::new(), creator: String::new(), ..c };
        assert_eq!(post_text(&c), "New clip: Untitled clip\nhttps://clips.twitch.tv/a");
    }

    #[test]
    fn viewer_set_text_cant_hide_a_link() {
        let c = Clip {
            id: "a".into(),
            title: "[Free nitro](https://evil.example)".into(),
            creator: "kiwi_bird".into(),
            url: "https://clips.twitch.tv/a".into(),
            thumbnail: String::new(),
            views: 0,
            created_at: 0,
        };
        assert_eq!(post_text(&c), "New clip by kiwi\\_bird: \\[Free nitro\\]\\(https://evil.example\\)\nhttps://clips.twitch.tv/a");
        // The dashboard shows it as Discord will: as typed.
        assert_eq!(headline(&c), "New clip by kiwi_bird: [Free nitro](https://evil.example)");
    }

    #[test]
    fn times_go_out_whole_seconds() {
        assert_eq!(rfc3339(1_793_448_000_123), "2026-10-31T12:00:00Z");
    }
}
