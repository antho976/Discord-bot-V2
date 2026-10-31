//! YouTube uploads from each channel's public RSS feed, checked every 5 minutes. No API key.

use std::sync::Arc;

use bot_core::anyhow::Result;
use bot_core::rusqlite::params;
use bot_core::twilight_model::channel::message::AllowedMentions;
use bot_core::twilight_model::id::Id;
use bot_core::{Ctx, jiff, now_ms, text, tracing};

use crate::{DEFAULT_VIDEO_MESSAGE, Settings, due, state_get, state_set};

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
    let handle = input.rsplit('/').next().unwrap_or(input).trim_start_matches('@');
    if handle.is_empty() || !handle.chars().all(|c| c.is_alphanumeric() || "._-".contains(c)) {
        return None;
    }
    let page = ctx.web.get(format!("https://www.youtube.com/@{handle}")).send().await.ok()?.error_for_status().ok()?.text().await.ok()?;
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
                if let Err(e) = ctx.http.create_message(Id::new(channel)).content(&content).allowed_mentions(Some(&mentions)).await {
                    tracing::warn!(guild, channel, error = %e, "couldn't post a new video");
                }
            }
        }
    }
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
    fn video_template_fills() {
        assert_eq!(fill_video("{channel}: {title} {url}", "Antho", "Pets", "https://y/1"), "Antho: Pets https://y/1");
        assert_eq!(fill_video("", "Antho", "Pets", "u"), "New video from Antho: Pets\nu");
    }
}
