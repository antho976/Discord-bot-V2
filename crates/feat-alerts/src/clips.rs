//! New Twitch clips of the followed channels, posted to a chosen channel. Helix `/clips` is read
//! every 15 minutes per broadcaster, only for servers that turned clips on; each clip is stored
//! once by its id, so a clip is never posted twice.

use std::sync::Arc;

use bot_core::anyhow::Result;
use bot_core::rusqlite::params;
use bot_core::{Ctx, jiff, now_ms, text, tracing};
use serde::Deserialize;

use crate::twitch::{self, List};
use crate::{Settings, due, state_get, state_set};

const EVERY_MS: i64 = 15 * 60_000;
/// Twitch can take a while to list a new clip, so each check looks back an hour past the last
/// one. Clips seen already are skipped by id.
const OVERLAP_MS: i64 = 60 * 60_000;
/// Twitch caps a clip search at a week from its start.
const MAX_SPAN_MS: i64 = 6 * 86_400_000;

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

/// What a clip post says. Discord turns the link into a player.
pub fn post_text(c: &Clip) -> String {
    let title = if c.title.trim().is_empty() { "Untitled clip" } else { c.title.trim() };
    if c.creator.is_empty() { format!("New clip: {title}\n{}", c.url) } else { format!("New clip by {}: {title}\n{}", c.creator, c.url) }
}

/// Servers posting clips, with their settings.
fn clip_guilds(guilds: &[(u64, Arc<Settings>)]) -> Vec<&(u64, Arc<Settings>)> {
    guilds.iter().filter(|(_, s)| s.clips && s.clips_channel.is_some() && !s.twitch_logins.is_empty()).collect()
}

fn rfc3339(ms: i64) -> String {
    jiff::Timestamp::from_second(ms / 1000).map(|t| t.to_string()).unwrap_or_default()
}

/// Called every tick: look for new clips of each broadcaster a clip-posting server follows,
/// every 15 minutes, and post them.
pub async fn check(ctx: &Ctx, guilds: &[(u64, Arc<Settings>)]) {
    let posting = clip_guilds(guilds);
    if posting.is_empty() {
        return;
    }
    let all = twitch::broadcasters(ctx);
    for b in all.iter().filter(|b| posting.iter().any(|(_, s)| s.twitch_logins.contains(&b.login))) {
        if !due(ctx, &format!("clips:{}", b.user_id), EVERY_MS - 30_000) {
            continue;
        }
        let now = now_ms();
        // The first check only marks when watching started.
        let Some(since) = state_get(ctx, &format!("clips_since:{}", b.user_id)).and_then(|v| v.parse::<i64>().ok()) else {
            state_set(ctx, &format!("clips_since:{}", b.user_id), &now.to_string());
            continue;
        };
        let last_ok = state_get(ctx, &format!("clips_ok:{}", b.user_id)).and_then(|v| v.parse().ok());
        let q = [
            ("broadcaster_id", b.user_id.clone()),
            ("started_at", rfc3339(window_start(since, last_ok, now))),
            ("ended_at", rfc3339(now + 60_000)),
            ("first", "100".into()),
        ];
        let found = match twitch::get::<List<HClip>>(ctx, "clips", &q).await {
            Ok(l) => l.map(|l| l.data).unwrap_or_default(),
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
        for c in &fresh {
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

/// Insert clips not seen before and return the ones made since watching started, oldest first.
/// Keeps the newest 50 per broadcaster.
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
    tx.execute(
        "DELETE FROM alerts_clips WHERE broadcaster_id = ?1 AND id NOT IN
         (SELECT id FROM alerts_clips WHERE broadcaster_id = ?1 ORDER BY created_at DESC LIMIT 50)",
        [bid],
    )?;
    tx.commit()?;
    fresh.sort_by_key(|c| c.created_at);
    Ok(fresh)
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
    fn posts_read_plainly() {
        let c = Clip { id: "a".into(), title: " Gold stake ".into(), creator: "kiwi".into(), url: "https://clips.twitch.tv/a".into(), thumbnail: String::new(), views: 0, created_at: 0 };
        assert_eq!(post_text(&c), "New clip by kiwi: Gold stake\nhttps://clips.twitch.tv/a");
        let c = Clip { title: String::new(), creator: String::new(), ..c };
        assert_eq!(post_text(&c), "New clip: Untitled clip\nhttps://clips.twitch.tv/a");
    }

    #[test]
    fn times_go_out_whole_seconds() {
        assert_eq!(rfc3339(1_793_448_000_123), "2026-10-31T12:00:00Z");
    }
}
