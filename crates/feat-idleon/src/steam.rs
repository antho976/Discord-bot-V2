//! Game updates: once an hour, IdleOn's announcements on Steam (the official news API, no key),
//! and a DM to the bot's owner for each new one, with the patch notes and what to run so the
//! account check follows the update.
//!
//! The first check only notes the newest post, so turning this on never sends old ones.

use bot_core::serde_json::Value;
use bot_core::anyhow::Result;
use bot_core::{Ctx, now_ms};

/// IdleOn's app on Steam.
pub const APP: u32 = 1476970;
const EVERY_MS: i64 = 60 * 60_000;
/// Longest patch-note excerpt in the DM (Discord allows 2,000 characters a message).
const EXCERPT: usize = 1_400;

/// One announcement.
#[derive(Debug, Clone, PartialEq)]
pub struct Post {
    pub gid: String,
    pub title: String,
    pub url: String,
    /// Seconds since 1970.
    pub date: i64,
    pub text: String,
}

/// Announcements in the news API's answer, newest first. Anything that isn't one is skipped.
pub fn parse(json: &str) -> Vec<Post> {
    let Ok(v) = bot_core::serde_json::from_str::<Value>(json) else { return Vec::new() };
    let Some(items) = v.pointer("/appnews/newsitems").and_then(Value::as_array) else { return Vec::new() };
    let mut out: Vec<Post> = items
        .iter()
        .filter_map(|n| {
            let s = |k: &str| n.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let gid = match n.get("gid") {
                Some(Value::String(g)) => g.clone(),
                Some(Value::Number(g)) => g.to_string(),
                _ => return None,
            };
            Some(Post { gid, title: s("title"), url: s("url"), date: n.get("date").and_then(Value::as_i64).unwrap_or(0), text: plain(&s("contents")) })
        })
        .collect();
    out.sort_by_key(|p| std::cmp::Reverse(p.date));
    out
}

/// Steam's BBCode and HTML down to plain lines: tags dropped, list items as "- ", blank runs
/// folded.
pub fn plain(s: &str) -> String {
    let s = s.replace("[*]", "\n- ").replace("<li>", "\n- ").replace("<br>", "\n").replace("<br/>", "\n").replace("<p>", "\n");
    let mut out = String::with_capacity(s.len());
    let mut depth: Option<char> = None;
    for ch in s.chars() {
        match (depth, ch) {
            (None, '[') => depth = Some(']'),
            (None, '<') => depth = Some('>'),
            (Some(end), c) if c == end => depth = None,
            (Some(_), _) => {}
            (None, c) => out.push(c),
        }
    }
    let out = out.replace("&amp;", "&").replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">");
    let mut lines: Vec<&str> = Vec::new();
    for l in out.lines().map(str::trim_end) {
        if l.trim().is_empty() && lines.last().is_none_or(|p| p.trim().is_empty()) {
            continue;
        }
        lines.push(l);
    }
    lines.join("\n").trim().to_string()
}

/// The DM for one post.
pub fn message(p: &Post) -> String {
    let mut text: String = p.text.chars().take(EXCERPT).collect();
    if text.len() < p.text.len() {
        text.push_str("\n…");
    }
    format!(
        "**IdleOn update: {}**\n<{}>\n\n{}\n\n\
        To bring the account check up to date: run `tools/idleon-extract/extract.py` (it reads your Steam install), \
        then the crowd example on your saves. New items show in `crowd.txt` as \"new since the last run\" until saves have them.",
        p.title, p.url, text
    )
}

async fn fetch(ctx: &Ctx) -> Result<Vec<Post>> {
    let url = format!("https://api.steampowered.com/ISteamNews/GetNewsForApp/v2/?appid={APP}&count=5&maxlength=0&feeds=steam_community_announcements&format=json");
    let body = ctx.web.get(url).timeout(std::time::Duration::from_secs(20)).send().await?.error_for_status()?.text().await?;
    Ok(parse(&body))
}

fn get(ctx: &Ctx, key: &str) -> Option<String> {
    ctx.db.with(|c| c.query_row("SELECT value FROM idleon_state WHERE key = ?1", [key], |r| r.get::<_, String>(0))).ok()
}

fn set(ctx: &Ctx, key: &str, value: &str) {
    let res = ctx.db.with(|c| c.execute("INSERT INTO idleon_state (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value", [key, value]));
    if let Err(e) = res {
        bot_core::tracing::warn!(key, error = %e, "couldn't store idleon state");
    }
}

/// The posts newer than the last one seen (by date, then not the same gid), oldest first.
pub fn new_since(posts: &[Post], last_date: i64, last_gid: &str) -> Vec<Post> {
    let mut v: Vec<Post> = posts.iter().filter(|p| p.date > last_date || (p.date == last_date && p.gid != last_gid && !last_gid.is_empty())).cloned().collect();
    v.retain(|p| p.gid != last_gid);
    v.reverse();
    v
}

/// Called every minute; asks Steam once an hour.
pub async fn tick(ctx: &Ctx) {
    if ctx.cfg.preview {
        return;
    }
    let now = now_ms();
    let last = get(ctx, "steam:at").and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
    if now - last < EVERY_MS {
        return;
    }
    set(ctx, "steam:at", &now.to_string());
    let posts = match fetch(ctx).await {
        Ok(p) => p,
        Err(e) => {
            bot_core::tracing::warn!(error = %e, "couldn't read IdleOn's Steam news");
            return;
        }
    };
    let Some(newest) = posts.first() else { return };
    let seen = get(ctx, "steam:last");
    let (last_date, last_gid) = seen.as_deref().and_then(|s| s.split_once(' ')).map(|(d, g)| (d.parse().unwrap_or(0), g.to_string())).unwrap_or((0, String::new()));
    if seen.is_some() {
        for p in new_since(&posts, last_date, &last_gid) {
            match ctx.dm(ctx.cfg.owner_id, &message(&p), &[], &[]).await {
                Ok(Some(_)) => {}
                Ok(None) => bot_core::tracing::warn!("couldn't DM the owner about an IdleOn update (DMs closed?)"),
                Err(e) => bot_core::tracing::warn!(error = %e, "couldn't DM the owner about an IdleOn update"),
            }
        }
    }
    set(ctx, "steam:last", &format!("{} {}", newest.date, newest.gid));
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSWER: &str = r#"{"appnews":{"appid":1476970,"newsitems":[
        {"gid":"501","title":"v2.48 Spelunking part 2","url":"https://store.steampowered.com/news/app/1476970/view/501","date":1730900000,
         "contents":"[h2]New stuff[/h2][list][*]12 new stamps[*]Bubble cap &amp; more[/list]<br>See you!"},
        {"gid":"500","title":"v2.47","url":"u","date":1730000000,"contents":"Fixes"},
        {"title":"no gid"}]}}"#;

    #[test]
    fn reads_posts_newest_first() {
        let p = parse(ANSWER);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].gid, "501");
        assert_eq!(p[0].text, "New stuff\n- 12 new stamps\n- Bubble cap & more\nSee you!");
        assert!(parse("not json").is_empty());
        assert!(parse(r#"{"appnews":{}}"#).is_empty());
    }

    #[test]
    fn only_posts_after_the_last_seen_go_out_oldest_first() {
        let p = parse(ANSWER);
        assert!(new_since(&p, 1730900000, "501").is_empty());
        let n = new_since(&p, 1729000000, "499");
        assert_eq!(n.iter().map(|p| p.gid.as_str()).collect::<Vec<_>>(), ["500", "501"]);
        assert_eq!(new_since(&p, 1730000000, "500").len(), 1);
    }

    #[test]
    fn the_message_fits_in_one_discord_message() {
        let mut p = parse(ANSWER).remove(0);
        p.text = "x".repeat(10_000);
        let m = message(&p);
        assert!(m.chars().count() < 2_000, "{}", m.chars().count());
    }
}
