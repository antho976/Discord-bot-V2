//! YouTube uploads from each channel's public RSS feed, checked every 5 minutes. No API key.

use std::collections::HashMap;
use std::sync::Arc;

use bot_core::anyhow::Result;
use bot_core::discord::{ComponentCall, Reply, button_row, snowflake_ms};
use bot_core::rusqlite::{params, params_from_iter};
use bot_core::twilight_model::channel::message::component::{ButtonStyle, Component};
use bot_core::twilight_model::channel::message::AllowedMentions;
use bot_core::twilight_model::id::Id;
use bot_core::{Ctx, jiff, now_ms, text, tracing};

use crate::{DEFAULT_VIDEO_MESSAGE, KEY, Settings, due, state_get, state_set};

const FEED: &str = "https://www.youtube.com/feeds/videos.xml";
/// A video that shows up in the feed but is older than this isn't announced (a deleted video
/// can bring an old one back into the feed's last 15).
const MAX_AGE_MS: i64 = 2 * 86_400_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub author: String,
    pub published_ms: i64,
}

impl Video {
    pub fn url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.id)
    }
}

#[derive(Debug, Default)]
pub struct Feed {
    pub title: String,
    pub videos: Vec<Video>,
}

/// The text inside the first `<name ...>...</name>` in `s`.
fn tag<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}");
    let mut from = 0;
    loop {
        let at = s[from..].find(&open)? + from;
        let after = &s[at + open.len()..];
        if !after.starts_with(|c: char| c == '>' || c == '/' || c.is_whitespace()) {
            from = at + open.len();
            continue;
        }
        let gt = after.find('>')?;
        if after[..gt].ends_with('/') {
            return Some("");
        }
        let body = &after[gt + 1..];
        let end = body.find(&format!("</{name}>"))?;
        return Some(&body[..end]);
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s.trim();
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(semi) = tail.find(';').filter(|n| *n <= 10) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..semi];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Parse a channel's Atom feed.
pub fn parse_feed(xml: &str) -> Feed {
    let mut parts = xml.split("<entry>");
    let head = parts.next().unwrap_or_default();
    let title = tag(head, "title").map(unescape).unwrap_or_default();
    let videos = parts
        .filter_map(|e| {
            let id = tag(e, "yt:videoId")?.trim().to_owned();
            if id.is_empty() {
                return None;
            }
            Some(Video {
                id,
                title: tag(e, "title").map(unescape).unwrap_or_default(),
                author: tag(e, "author").and_then(|a| tag(a, "name")).map(unescape).unwrap_or_default(),
                published_ms: tag(e, "published")
                    .and_then(|p| p.trim().parse::<jiff::Timestamp>().ok())
                    .map(|t| t.as_millisecond())
                    .unwrap_or(0),
            })
        })
        .collect();
    Feed { title, videos }
}

/// A YouTube channel id: "UC" and 22 more URL-safe characters.
pub fn is_channel_id(s: &str) -> bool {
    s.len() == 24 && s.starts_with("UC") && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The first channel id that follows one of the markers in a page or URL.
pub fn find_channel_id(s: &str) -> Option<String> {
    for marker in ["\"externalId\":\"", "/channel/", "channel_id="] {
        let mut rest = s;
        while let Some(i) = rest.find(marker) {
            let cand = rest[i + marker.len()..].get(..24).unwrap_or_default();
            if is_channel_id(cand) {
                return Some(cand.to_owned());
            }
            rest = &rest[i + marker.len()..];
        }
    }
    None
}

/// Where to look up what someone typed, when it isn't a channel id: their handle's page, or
/// the youtube.com link they pasted (`/c/Name` and `/user/Name` links name no handle).
pub fn lookup_url(input: &str) -> Option<String> {
    let input = input.trim();
    let handle = match input.rfind('@') {
        // "@Antho", "youtube.com/@Antho/videos?x=1"
        Some(at) => input[at + 1..].split(['/', '?', '#']).next().unwrap_or_default(),
        None if input.contains("youtube.com/") => {
            let rest = input.split_once("youtube.com/")?.1.split(['?', '#']).next().unwrap_or_default();
            return (!rest.is_empty()).then(|| format!("https://www.youtube.com/{}", rest.trim_end_matches('/')));
        }
        // A bare name is taken as a handle.
        None if !input.contains('/') => input,
        None => return None,
    };
    let ok = !handle.is_empty() && handle.chars().all(|c| c.is_alphanumeric() || "._-".contains(c));
    ok.then(|| format!("https://www.youtube.com/@{handle}"))
}

/// What someone typed into the settings: an id, a channel URL, or a handle (`@name`, which is
/// looked up on youtube.com).
pub async fn resolve_input(ctx: &Ctx, input: &str) -> Option<String> {
    let input = input.trim().trim_end_matches('/');
    if is_channel_id(input) {
        return Some(input.to_owned());
    }
    if let Some(id) = find_channel_id(input) {
        return Some(id);
    }
    let page = ctx.web.get(lookup_url(input)?).send().await.ok()?.error_for_status().ok()?.text().await.ok()?;
    find_channel_id(&page)
}

pub fn fill_video(template: &str, channel: &str, title: &str, url: &str) -> String {
    let template = if template.trim().is_empty() { DEFAULT_VIDEO_MESSAGE } else { template };
    text::fill(template, &[("channel", channel), ("title", title), ("url", url)])
}

async fn fetch(ctx: &Ctx, channel_id: &str) -> Result<Feed> {
    let xml = ctx.web.get(FEED).query(&[("channel_id", channel_id)]).send().await?.error_for_status()?.text().await?;
    Ok(parse_feed(&xml))
}

/// Every 5 minutes per channel: store new videos and post them. The first read of a channel
/// only records what's there.
pub async fn check(ctx: &Ctx, guilds: &[(u64, Arc<Settings>)]) {
    let mut feeds: Vec<String> = guilds.iter().flat_map(|(_, s)| s.youtube_ids.iter().cloned()).collect();
    feeds.sort();
    feeds.dedup();
    for feed in feeds {
        if !due(ctx, &format!("yt:{feed}"), 5 * 60_000 - 30_000) {
            continue;
        }
        let f = match fetch(ctx, &feed).await {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(feed = %feed, error = %e, "couldn't read a YouTube feed");
                continue;
            }
        };
        let fresh = match store(ctx, &feed, &f) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(feed = %feed, error = %e, "couldn't store YouTube videos");
                continue;
            }
        };
        let init_key = format!("yt_init:{feed}");
        if state_get(ctx, &init_key).is_none() {
            state_set(ctx, &init_key, "1");
            continue;
        }
        let now = now_ms();
        for v in fresh.iter().filter(|v| now - v.published_ms <= MAX_AGE_MS) {
            let channel_name = if v.author.is_empty() { f.title.as_str() } else { v.author.as_str() };
            for (guild, s) in guilds {
                let Some(channel) = s.youtube_channel.filter(|_| s.youtube_ids.contains(&feed)) else { continue };
                let text = fill_video(&s.youtube_message, channel_name, &v.title, &v.url());
                let content = match s.youtube_role {
                    Some(r) => format!("<@&{r}> {text}"),
                    None => text,
                };
                let mentions = AllowedMentions { roles: s.youtube_role.map(Id::new).into_iter().collect(), ..Default::default() };
                let rows: Vec<Component> = rewards_on(ctx, *guild, s).then(|| reward_row(s, &v.id, false)).into_iter().collect();
                if let Err(e) = ctx.http.create_message(Id::new(channel)).content(&content).components(&rows).allowed_mentions(Some(&mentions)).await {
                    tracing::warn!(guild, channel, error = %e, "couldn't post a new video");
                }
            }
        }
    }
}

/// The dashboard's "Send a test post": the post for a stored video, with nobody pinged. The
/// reward button shows but can't be pressed, so a test pays nobody.
pub async fn test_post(ctx: &Ctx, guild: u64, channel: u64, s: &Settings, v: &Video) -> Result<()> {
    let text = fill_video(&s.youtube_message, &v.author, &v.title, &v.url());
    let content = format!("{text}\n-# Test post. Nobody was pinged.");
    let none = AllowedMentions::default();
    let rows: Vec<Component> = rewards_on(ctx, guild, s).then(|| reward_row(s, &v.id, true)).into_iter().collect();
    ctx.http.create_message(Id::new(channel)).content(&content).components(&rows).allowed_mentions(Some(&none)).await?;
    Ok(())
}

// ---- upload rewards ----

/// Upload posts carry the claim button: rewards on, some XP to give, and XP and levels on.
pub fn rewards_on(ctx: &Ctx, guild: u64, s: &Settings) -> bool {
    s.reward && s.reward_xp > 0 && ctx.enabled(guild, feat_levels::KEY)
}

/// "Claim 50 XP", with the video in its custom id.
pub fn reward_row(s: &Settings, video: &str, disabled: bool) -> Component {
    let id = format!("{KEY}:claim:{video}");
    let mut row = button_row(&[(&id, &format!("Claim {} XP", text::num(s.reward_xp)), ButtonStyle::Success)]);
    if let Component::ActionRow(r) = &mut row {
        for c in &mut r.components {
            if let Component::Button(b) = c {
                b.disabled = disabled;
            }
        }
    }
    row
}

/// The button pays out for `hours` after the post went up.
pub fn claim_open(posted_ms: i64, now: i64, hours: i64) -> bool {
    posted_ms > 0 && now >= posted_ms && now - posted_ms < hours * 3_600_000
}

/// The claim button: once per member per video, within the window, through XP and levels.
pub async fn claim(ctx: &Ctx, call: &ComponentCall) -> Result<()> {
    let (Some(guild), Some(video)) = (call.guild_id(), call.args().get(1).map(|v| (*v).to_owned())) else { return Ok(()) };
    let say = |t: String| Reply::text(t).ephemeral();
    let s = ctx.settings::<Settings>(guild, KEY)?;
    if !rewards_on(ctx, guild, &s) {
        return ctx.reply(&call.interaction, say("Upload rewards are off on this server.".into())).await;
    }
    let posted = call.interaction.message.as_ref().map(|m| snowflake_ms(m.id.get())).unwrap_or(0);
    let now = now_ms();
    if !claim_open(posted, now, s.reward_hours) {
        let closed = posted + s.reward_hours * 3_600_000;
        return ctx.reply(&call.interaction, say(format!("This reward closed {}.", text::ago(closed)))).await;
    }
    let user = call.user_id();
    // A level-up can mean role changes and a post, so answer later. Deferring first means a
    // failed defer leaves no claim behind that was never paid.
    ctx.defer(&call.interaction, true).await?;
    // The row goes in before the XP, so a double click can't pay twice.
    let added = ctx.db.with(|c| {
        c.execute(
            "INSERT OR IGNORE INTO alerts_video_claims (guild_id, video_id, user_id, xp, at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![guild as i64, video, user as i64, s.reward_xp, now],
        )
    });
    let added = match added {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(guild, user, error = %e, "couldn't record an upload reward claim");
            return ctx.edit_original(&call.interaction, say("The XP didn't go through. Try again in a minute.".into())).await;
        }
    };
    if added == 0 {
        return ctx.edit_original(&call.interaction, say("You already claimed this one.".into())).await;
    }
    // Level-ups from a claim go to the levels announce channel when one is set, never here.
    let answer = match feat_levels::give_xp(ctx, guild, user, s.reward_xp, None).await {
        Ok(true) => return ctx.edit_original(&call.interaction, say(format!("You got {} XP.", text::num(s.reward_xp)))).await,
        Ok(false) => "Upload rewards are off on this server.",
        Err(e) => {
            tracing::warn!(guild, user, error = %e, "couldn't give upload reward XP");
            "The XP didn't go through. Try again in a minute."
        }
    };
    // Nothing was paid, so the claim is taken back.
    let undo = ctx.db.with(|c| {
        c.execute("DELETE FROM alerts_video_claims WHERE guild_id = ?1 AND video_id = ?2 AND user_id = ?3", params![guild as i64, video, user as i64])
    });
    if let Err(e) = undo {
        tracing::warn!(guild, user, error = %e, "couldn't undo a failed claim");
    }
    ctx.edit_original(&call.interaction, say(answer.into())).await
}

/// Who claimed each of these videos, earliest first.
pub fn claims(ctx: &Ctx, guild: u64, videos: &[String]) -> HashMap<String, Vec<u64>> {
    let mut out: HashMap<String, Vec<u64>> = HashMap::new();
    if videos.is_empty() {
        return out;
    }
    let marks = vec!["?"; videos.len()].join(", ");
    let sql = format!("SELECT video_id, user_id FROM alerts_video_claims WHERE guild_id = {guild} AND video_id IN ({marks}) ORDER BY at");
    let rows: Vec<(String, i64)> = ctx
        .db
        .with(|c| {
            let mut st = c.prepare(&sql)?;
            st.query_map(params_from_iter(videos.iter()), |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
        })
        .unwrap_or_default();
    for (video, user) in rows {
        out.entry(video).or_default().push(user as u64);
    }
    out
}

/// Claims and XP given since `since`.
pub fn claim_totals(ctx: &Ctx, guild: u64, since: i64) -> (i64, i64) {
    ctx.db
        .with(|c| {
            c.query_row(
                "SELECT COUNT(*), COALESCE(SUM(xp), 0) FROM alerts_video_claims WHERE guild_id = ?1 AND at >= ?2",
                params![guild as i64, since],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
        })
        .unwrap_or((0, 0))
}

/// Insert videos not seen before and return them. Keeps the newest 50 per channel.
fn store(ctx: &Ctx, feed: &str, f: &Feed) -> Result<Vec<Video>> {
    let now = now_ms();
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    let mut fresh = Vec::new();
    for v in &f.videos {
        let n = tx.execute(
            "INSERT OR IGNORE INTO alerts_videos (feed, video_id, title, author, published_at, seen_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![feed, v.id, v.title, v.author, v.published_ms, now],
        )?;
        if n > 0 {
            fresh.push(v.clone());
        }
    }
    tx.execute(
        "DELETE FROM alerts_videos WHERE feed = ?1 AND video_id NOT IN
         (SELECT video_id FROM alerts_videos WHERE feed = ?1 ORDER BY published_at DESC LIMIT 50)",
        [feed],
    )?;
    tx.commit()?;
    Ok(fresh)
}

/// A video's picture at 320x180, from YouTube's image server.
pub fn thumb(id: &str) -> String {
    format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg")
}

/// Each channel with its newest stored video, in the order given. `None` until the first check.
pub fn latest_per_feed(ctx: &Ctx, feeds: &[String]) -> Vec<(String, Option<Video>)> {
    feeds.iter().map(|feed| (feed.clone(), recent(ctx, std::slice::from_ref(feed), 1).into_iter().next())).collect()
}

/// The newest videos across these channels.
pub fn recent(ctx: &Ctx, feeds: &[String], limit: usize) -> Vec<Video> {
    let mut out: Vec<Video> = feeds
        .iter()
        .flat_map(|feed| {
            ctx.db
                .with(|c| {
                    let mut s = c.prepare_cached("SELECT video_id, title, author, published_at FROM alerts_videos WHERE feed = ?1 ORDER BY published_at DESC LIMIT ?2")?;
                    s
                        .query_map(params![feed, limit as i64], |r| Ok(Video { id: r.get(0)?, title: r.get(1)?, author: r.get(2)?, published_ms: r.get(3)? }))?
                        .collect::<bot_core::rusqlite::Result<Vec<_>>>()
                })
                .unwrap_or_default()
        })
        .collect();
    out.sort_by_key(|v| std::cmp::Reverse(v.published_ms));
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns:yt="http://www.youtube.com/xml/schemas/2015" xmlns:media="http://search.yahoo.com/mrss/" xmlns="http://www.w3.org/2005/Atom">
 <link rel="self" href="http://www.youtube.com/feeds/videos.xml?channel_id=UCabcdefghijklmnopqrstuv"/>
 <id>yt:channel:abcdefghijklmnopqrstuv</id>
 <yt:channelId>abcdefghijklmnopqrstuv</yt:channelId>
 <title>Antho &amp; Friends</title>
 <author><name>Antho &amp; Friends</name><uri>https://www.youtube.com/channel/UCabcdefghijklmnopqrstuv</uri></author>
 <entry>
  <id>yt:video:dQw4w9WgXcQ</id>
  <yt:videoId>dQw4w9WgXcQ</yt:videoId>
  <yt:channelId>UCabcdefghijklmnopqrstuv</yt:channelId>
  <title>Pets &quot;tier list&quot; &#8211; part 2</title>
  <link rel="alternate" href="https://www.youtube.com/watch?v=dQw4w9WgXcQ"/>
  <author><name>Antho</name></author>
  <published>2026-10-29T17:00:06+00:00</published>
  <media:group><media:title>Pets "tier list" – part 2</media:title></media:group>
 </entry>
 <entry>
  <id>yt:video:kJQP7kiw5Fk</id>
  <yt:videoId>kJQP7kiw5Fk</yt:videoId>
  <title>World 6 &lt;fast&gt;</title>
  <author><name>Antho</name></author>
  <published>2026-10-20T12:00:00+00:00</published>
 </entry>
</feed>"#;

    #[test]
    fn feed_parses() {
        let f = parse_feed(SAMPLE);
        assert_eq!(f.title, "Antho & Friends");
        assert_eq!(f.videos.len(), 2);
        let v = &f.videos[0];
        assert_eq!(v.id, "dQw4w9WgXcQ");
        assert_eq!(v.title, "Pets \"tier list\" – part 2");
        assert_eq!(v.author, "Antho");
        assert_eq!(v.published_ms, "2026-10-29T17:00:06Z".parse::<jiff::Timestamp>().unwrap().as_millisecond());
        assert_eq!(f.videos[1].title, "World 6 <fast>");
        assert_eq!(v.url(), "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
    }

    #[test]
    fn junk_feed_is_empty() {
        assert!(parse_feed("<html>not a feed</html>").videos.is_empty());
        assert!(parse_feed("<entry><title>no id</title></entry>").videos.is_empty());
    }

    #[test]
    fn entities_unescape() {
        assert_eq!(unescape("a &amp; b &#x41;&#66; &bogus; & c"), "a & b AB &bogus; & c");
    }

    #[test]
    fn channel_ids() {
        assert!(is_channel_id("UCabcdefghijklmnopqrstuv"));
        assert!(!is_channel_id("UCshort"));
        assert_eq!(find_channel_id("https://www.youtube.com/channel/UCabcdefghijklmnopqrstuv/videos").as_deref(), Some("UCabcdefghijklmnopqrstuv"));
        assert_eq!(find_channel_id(r#"..."externalId":"UC_x5XG1OV2P6uZZ5FSM9Ttw","#).as_deref(), Some("UC_x5XG1OV2P6uZZ5FSM9Ttw"));
        assert_eq!(find_channel_id("nothing here"), None);
    }

    #[test]
    fn lookups_find_the_handle() {
        let u = |s: &str| lookup_url(s);
        assert_eq!(u("@Antho").as_deref(), Some("https://www.youtube.com/@Antho"));
        assert_eq!(u("Antho").as_deref(), Some("https://www.youtube.com/@Antho"));
        assert_eq!(u("https://www.youtube.com/@Antho/videos?view=0").as_deref(), Some("https://www.youtube.com/@Antho"));
        assert_eq!(u("https://youtube.com/c/AnthoPlays/").as_deref(), Some("https://www.youtube.com/c/AnthoPlays"));
        assert_eq!(u("https://www.youtube.com/user/antho").as_deref(), Some("https://www.youtube.com/user/antho"));
        assert_eq!(u("https://example.com/antho"), None);
        assert_eq!(u("@"), None);
        assert_eq!(u("@bad name"), None);
    }

    #[test]
    fn claims_stay_open_for_the_window() {
        let h = 3_600_000;
        let posted = 1_000 * h;
        assert!(claim_open(posted, posted + h, 24));
        assert!(claim_open(posted, posted + 24 * h - 1, 24));
        assert!(!claim_open(posted, posted + 24 * h, 24));
        // No message to time it by, or a clock behind the post: closed.
        assert!(!claim_open(0, posted, 24));
        assert!(!claim_open(posted, posted - 1, 24));
    }

    #[test]
    fn reward_button_names_the_xp() {
        let s = Settings { reward_xp: 1500, ..Settings::default() };
        let Component::ActionRow(row) = reward_row(&s, "dQw4w9WgXcQ", true) else { panic!("not a row") };
        let Component::Button(b) = &row.components[0] else { panic!("not a button") };
        assert_eq!(b.label.as_deref(), Some("Claim 1,500 XP"));
        assert_eq!(b.custom_id.as_deref(), Some("alerts:claim:dQw4w9WgXcQ"));
        assert!(b.disabled);
    }

    #[test]
    fn video_template_fills() {
        assert_eq!(fill_video("{channel}: {title} {url}", "Antho", "Pets", "https://y/1"), "Antho: Pets https://y/1");
        assert_eq!(fill_video("", "Antho", "Pets", "u"), "New video from Antho: Pets\nu");
    }
}
