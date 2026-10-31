//! The starboard: keep a post per popular message and its star count up to date.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use bot_core::rusqlite::{Connection, OptionalExtension, params};
use bot_core::twilight_http::error::ErrorType;
use bot_core::twilight_http::request::channel::reaction::RequestReactionType;
use bot_core::twilight_model::channel::Message;
use bot_core::twilight_model::channel::message::{AllowedMentions, EmojiReactionType};
use bot_core::twilight_model::id::marker::{ChannelMarker, EmojiMarker, GuildMarker, MessageMarker};
use bot_core::twilight_model::id::Id;
use bot_core::twilight_util::builder::embed::{EmbedAuthorBuilder, EmbedBuilder, EmbedFieldBuilder, ImageSource};
use bot_core::{Ctx, now_ms, text};

use crate::{KEY, Settings, Tickets};

const COLOR: u32 = 0xf0806a;

/// The configured emoji, parsed.
#[derive(Debug, Clone, PartialEq)]
pub enum Emoji {
    Unicode(String),
    Custom(Id<EmojiMarker>, String),
}

/// A unicode emoji as is, or a custom one in the `<:name:id>` form Discord copies. Plain words
/// like `:star:` are not an emoji.
pub fn parse_emoji(raw: &str) -> Option<Emoji> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(inner) = raw.strip_prefix('<').and_then(|r| r.strip_suffix('>')) {
        let parts: Vec<&str> = inner.split(':').collect();
        return match parts[..] {
            [flag, name, id] if flag.is_empty() || flag == "a" => {
                let id: u64 = id.parse().ok().filter(|i| *i != 0)?;
                Some(Emoji::Custom(Id::new(id), name.to_owned()))
            }
            _ => None,
        };
    }
    (!raw.is_ascii() && raw.chars().count() <= 16).then(|| Emoji::Unicode(raw.to_owned()))
}

impl Emoji {
    fn matches(&self, other: &EmojiReactionType) -> bool {
        let plain = |s: &str| s.replace('\u{fe0f}', "");
        match (self, other) {
            (Emoji::Unicode(a), EmojiReactionType::Unicode { name }) => plain(a) == plain(name),
            (Emoji::Custom(a, _), EmojiReactionType::Custom { id, .. }) => a == id,
            _ => false,
        }
    }

    fn request(&self) -> RequestReactionType<'_> {
        match self {
            Emoji::Unicode(name) => RequestReactionType::Unicode { name },
            Emoji::Custom(id, name) => RequestReactionType::Custom { id: *id, name: Some(name) },
        }
    }
}

// ---- stored posts ----

#[derive(Debug, Clone, PartialEq)]
pub struct Post {
    pub message_id: u64,
    pub channel_id: u64,
    pub author_id: u64,
    pub post_channel_id: u64,
    pub post_id: u64,
    pub stars: i64,
    pub preview: String,
    pub created_at: i64,
    pub removed: bool,
    /// The message's picture. None until it has been looked up; empty when there is none.
    pub image: Option<String>,
}

const COLUMNS: &str = "message_id, channel_id, author_id, post_channel_id, post_id, stars, preview, created_at, removed, image";

fn from_row(r: &bot_core::rusqlite::Row<'_>) -> bot_core::rusqlite::Result<Post> {
    Ok(Post {
        message_id: r.get::<_, i64>(0)? as u64,
        channel_id: r.get::<_, i64>(1)? as u64,
        author_id: r.get::<_, i64>(2)? as u64,
        post_channel_id: r.get::<_, i64>(3)? as u64,
        post_id: r.get::<_, i64>(4)? as u64,
        stars: r.get(5)?,
        preview: r.get(6)?,
        created_at: r.get(7)?,
        removed: r.get::<_, i64>(8)? != 0,
        image: r.get(9)?,
    })
}

pub fn get(conn: &Connection, guild: u64, message: u64) -> bot_core::rusqlite::Result<Option<Post>> {
    conn.query_row(&format!("SELECT {COLUMNS} FROM starboard_posts WHERE guild_id = ?1 AND message_id = ?2"), params![guild as i64, message as i64], from_row).optional()
}

pub fn insert(conn: &Connection, guild: u64, p: &Post) -> bot_core::rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO starboard_posts (guild_id, message_id, channel_id, author_id, post_channel_id, post_id, stars, preview, created_at, removed, image)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            guild as i64,
            p.message_id as i64,
            p.channel_id as i64,
            p.author_id as i64,
            p.post_channel_id as i64,
            p.post_id as i64,
            p.stars,
            p.preview,
            p.created_at,
            p.removed as i64,
            p.image
        ],
    )?;
    Ok(())
}

fn set_image(conn: &Connection, guild: u64, message: u64, image: &str) -> bot_core::rusqlite::Result<()> {
    conn.execute("UPDATE starboard_posts SET image = ?3 WHERE guild_id = ?1 AND message_id = ?2", params![guild as i64, message as i64, image])?;
    Ok(())
}

fn set_stars(conn: &Connection, guild: u64, message: u64, stars: i64) -> bot_core::rusqlite::Result<()> {
    conn.execute("UPDATE starboard_posts SET stars = ?3 WHERE guild_id = ?1 AND message_id = ?2", params![guild as i64, message as i64, stars])?;
    Ok(())
}

fn forget(conn: &Connection, guild: u64, message: u64) -> bot_core::rusqlite::Result<()> {
    conn.execute("DELETE FROM starboard_posts WHERE guild_id = ?1 AND message_id = ?2", params![guild as i64, message as i64])?;
    Ok(())
}

/// A moderator deleted the starboard post: remember it, so the message isn't posted again.
fn mark_removed(conn: &Connection, guild: u64, post: u64) -> bot_core::rusqlite::Result<usize> {
    conn.execute("UPDATE starboard_posts SET removed = 1 WHERE guild_id = ?1 AND post_id = ?2 AND removed = 0", params![guild as i64, post as i64])
}

pub fn top(conn: &Connection, guild: u64, limit: i64) -> bot_core::rusqlite::Result<Vec<Post>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM starboard_posts WHERE guild_id = ?1 AND removed = 0 ORDER BY stars DESC, created_at DESC LIMIT ?2"
    ))?;
    stmt.query_map(params![guild as i64, limit], from_row)?.collect()
}

/// The newest posts first.
pub fn recent(conn: &Connection, guild: u64, limit: i64) -> bot_core::rusqlite::Result<Vec<Post>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM starboard_posts WHERE guild_id = ?1 AND removed = 0 ORDER BY created_at DESC LIMIT ?2"
    ))?;
    stmt.query_map(params![guild as i64, limit], from_row)?.collect()
}

/// Members with the most posts on the starboard: `(author, posts)`.
pub fn top_authors(conn: &Connection, guild: u64, limit: i64) -> bot_core::rusqlite::Result<Vec<(u64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT author_id, COUNT(*) FROM starboard_posts WHERE guild_id = ?1 AND removed = 0
         GROUP BY author_id ORDER BY 2 DESC, 1 LIMIT ?2",
    )?;
    stmt.query_map(params![guild as i64, limit], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?)))?.collect()
}

pub struct Stats {
    pub posts: i64,
    pub this_week: i64,
}

pub fn stats(conn: &Connection, guild: u64, now: i64) -> bot_core::rusqlite::Result<Stats> {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(created_at > ?2), 0) FROM starboard_posts WHERE guild_id = ?1 AND removed = 0",
        params![guild as i64, now - 7 * 86_400_000],
        |r| Ok(Stats { posts: r.get(0)?, this_week: r.get(1)? }),
    )
}

// ---- events ----

pub fn on_message_delete(ctx: &Ctx, guild: Option<Id<GuildMarker>>, channel: Id<ChannelMarker>, message: Id<MessageMarker>) -> bot_core::anyhow::Result<()> {
    let Some(guild) = guild.map(|g| g.get()) else { return Ok(()) };
    let s = ctx.settings::<Settings>(guild, KEY)?;
    if s.star_channel == Some(channel.get()) {
        ctx.db.with(|c| mark_removed(c, guild, message.get()).map(|_| ()))?;
    }
    Ok(())
}

pub async fn on_reaction(
    feature: &Tickets,
    ctx: &Ctx,
    guild: Option<Id<GuildMarker>>,
    channel: Id<ChannelMarker>,
    message: Id<MessageMarker>,
    // None when a moderator cleared every reaction from the message.
    emoji: Option<&EmojiReactionType>,
) -> bot_core::anyhow::Result<()> {
    let Some(guild) = guild.map(|g| g.get()) else { return Ok(()) };
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let Some(star_channel) = s.star_channel else { return Ok(()) };
    if channel.get() == star_channel || s.star_ignored.contains(&channel.get()) {
        return Ok(());
    }
    let Some(wanted) = parse_emoji(&s.star_emoji) else { return Ok(()) };
    if emoji.is_some_and(|e| !wanted.matches(e)) {
        return Ok(());
    }
    let _one_at_a_time = feature.star_lock.lock().await;
    if let Err(e) = refresh(ctx, guild, channel.get(), message.get(), &s, &wanted, star_channel).await {
        bot_core::tracing::warn!(guild, message = message.get(), error = %e, "couldn't update the starboard");
    }
    Ok(())
}

/// Fetch the message and its star count, then post, update or take down its starboard post.
async fn refresh(ctx: &Ctx, guild: u64, channel: u64, message_id: u64, s: &Settings, emoji: &Emoji, star_channel: u64) -> bot_core::anyhow::Result<()> {
    let existing = ctx.db.with(|c| get(c, guild, message_id))?;
    if existing.as_ref().is_some_and(|p| p.removed) {
        return Ok(());
    }
    let message = match ctx.http.message(Id::new(channel), Id::new(message_id)).await {
        Ok(r) => r.model().await?,
        Err(e) if is_not_found(&e) => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let threshold = s.star_threshold.max(1);
    let raw = message.reactions.iter().find(|r| emoji.matches(&r.emoji)).map(|r| r.count as i64).unwrap_or(0);
    // The author's own star only needs finding when it could change what happens.
    let author_starred = !s.star_self && raw > 0 && (raw >= threshold || existing.is_some()) && author_reacted(ctx, &message, emoji).await;
    let stars = raw - author_starred as i64;
    let line = format!("{} {} in <#{}>", s.star_emoji.trim(), stars, channel);
    let none = AllowedMentions::default();

    match existing {
        Some(post) if stars >= threshold || !s.star_remove_below => {
            if post.stars == stars {
                return Ok(());
            }
            let edited = ctx.http.update_message(Id::new(post.post_channel_id), Id::new(post.post_id)).content(Some(&line)).allowed_mentions(Some(&none)).await;
            match edited {
                Ok(_) => ctx.db.with(|c| set_stars(c, guild, message_id, stars))?,
                // Someone deleted the post by hand: leave it deleted.
                Err(e) if is_not_found(&e) => ctx.db.with(|c| mark_removed(c, guild, post.post_id).map(|_| ()))?,
                Err(e) => return Err(e.into()),
            }
        }
        Some(post) => {
            ctx.db.with(|c| forget(c, guild, message_id))?;
            if let Err(e) = ctx.http.delete_message(Id::new(post.post_channel_id), Id::new(post.post_id)).await
                && !is_not_found(&e)
            {
                return Err(e.into());
            }
        }
        None if stars >= threshold => {
            let embed = build_embed(guild, &message);
            let sent = ctx.http.create_message(Id::new(star_channel)).content(&line).embeds(&[embed]).allowed_mentions(Some(&none)).await;
            let post = match sent {
                Ok(r) => r.model().await?,
                Err(e) => {
                    let text = format!("Starred messages aren't reaching the starboard. {}", why_refused(&e));
                    notify_failure(ctx, guild, text, format!("/g/{guild}/{KEY}/starboard?view=settings"));
                    return Err(e.into());
                }
            };
            ctx.db.with(|c| {
                insert(
                    c,
                    guild,
                    &Post {
                        message_id,
                        channel_id: channel,
                        author_id: message.author.id.get(),
                        post_channel_id: star_channel,
                        post_id: post.id.get(),
                        stars,
                        preview: preview_of(&message),
                        created_at: now_ms(),
                        removed: false,
                        image: Some(image_of(&message).unwrap_or_default()),
                    },
                )
            })?;
        }
        None => {}
    }
    Ok(())
}

fn is_not_found(e: &bot_core::twilight_http::Error) -> bool {
    matches!(e.kind(), ErrorType::Response { status, .. } if status.get() == 404)
}

/// Why Discord refused a post, in words a server owner can act on.
fn why_refused(e: &bot_core::twilight_http::Error) -> String {
    match e.kind() {
        bot_core::twilight_http::error::ErrorType::Response { status, .. } if status.get() == 403 => {
            "The bot can't post in that channel. Check it can see the channel and send messages there.".into()
        }
        bot_core::twilight_http::error::ErrorType::Response { status, .. } if status.get() == 404 => "The channel was deleted. Pick another one.".into(),
        _ => format!("Discord refused the message: {e}"),
    }
}

/// Put a failed post behind the dashboard bell, at most once a day per server, so a broken channel
/// gets noticed without a notice for every post.
fn notify_failure(ctx: &Ctx, guild: u64, text: String, link: String) {
    static LAST: std::sync::LazyLock<Mutex<HashMap<u64, Instant>>> = std::sync::LazyLock::new(Default::default);
    {
        let mut last = LAST.lock().unwrap();
        if last.get(&guild).is_some_and(|t| t.elapsed().as_secs() < 86_400) {
            return;
        }
        last.insert(guild, Instant::now());
    }
    ctx.notify(Some(guild), KEY, text, Some(link));
}

/// A 4xx answer other than a rate limit: asking again gets the same answer.
fn is_final(e: &bot_core::twilight_http::Error) -> bool {
    matches!(e.kind(), ErrorType::Response { status, .. } if (400..500).contains(&status.get()) && status.get() != 429)
}

/// Whether the message's author reacted with the emoji. Looks through up to 500 reactors.
async fn author_reacted(ctx: &Ctx, message: &Message, emoji: &Emoji) -> bool {
    let request = emoji.request();
    let mut after = None;
    for _ in 0..5 {
        let mut req = ctx.http.reactions(message.channel_id, message.id, &request).limit(100);
        if let Some(id) = after {
            req = req.after(id);
        }
        let users = match req.await {
            Ok(r) => match r.models().await {
                Ok(u) => u,
                Err(_) => return false,
            },
            Err(_) => return false,
        };
        if users.iter().any(|u| u.id == message.author.id) {
            return true;
        }
        match users.last() {
            Some(last) if users.len() == 100 => after = Some(last.id),
            _ => return false,
        }
    }
    false
}

// ---- the post ----

fn is_image(a: &bot_core::twilight_model::channel::Attachment) -> bool {
    a.content_type.as_deref().is_some_and(|t| t.starts_with("image/"))
        || [".png", ".jpg", ".jpeg", ".gif", ".webp"].iter().any(|ext| a.filename.to_lowercase().ends_with(ext))
}

/// The first image attachment, or else an embed's image or thumbnail.
fn image_of(m: &Message) -> Option<String> {
    m.attachments
        .iter()
        .find(|a| is_image(a))
        .map(|a| a.url.clone())
        .or_else(|| m.embeds.iter().find_map(|e| e.image.as_ref().map(|i| i.url.clone()).or_else(|| e.thumbnail.as_ref().map(|t| t.url.clone()))))
}

/// A short line for the dashboard table.
fn preview_of(m: &Message) -> String {
    let content = m.content.split_whitespace().collect::<Vec<_>>().join(" ");
    if !content.is_empty() {
        return text::clip(&content, 140);
    }
    if m.attachments.is_empty() { String::new() } else { "Attachment".into() }
}

fn build_embed(guild: u64, m: &Message) -> bot_core::twilight_model::channel::message::Embed {
    let name = m
        .member
        .as_ref()
        .and_then(|mem| mem.nick.clone())
        .or_else(|| m.author.global_name.clone())
        .unwrap_or_else(|| m.author.name.clone());
    let avatar = bot_core::discord::avatar_url(m.author.id.get(), m.author.avatar.map(|a| a.to_string()).as_deref());
    let mut author = EmbedAuthorBuilder::new(name);
    if let Ok(icon) = ImageSource::url(avatar) {
        author = author.icon_url(icon);
    }
    let mut embed = EmbedBuilder::new().color(COLOR).author(author).timestamp(m.timestamp);
    if !m.content.trim().is_empty() {
        embed = embed.description(text::clip(m.content.trim(), 3500));
    }
    if let Some(Ok(source)) = image_of(m).map(ImageSource::url) {
        embed = embed.image(source);
    }
    let files: Vec<String> = m.attachments.iter().filter(|a| !is_image(a)).take(5).map(|a| format!("[{}]({})", text::clip(&a.filename, 60), a.url)).collect();
    if !files.is_empty() {
        embed = embed.field(EmbedFieldBuilder::new("Attachments", files.join("\n")));
    }
    embed = embed.field(EmbedFieldBuilder::new("Source", format!("[Jump to message](https://discord.com/channels/{guild}/{}/{})", m.channel_id, m.id)));
    embed.build()
}

// ---- pictures on the dashboard ----

/// Discord signs attachment links and they stop working after about a day. These need a fresh
/// signature before they can be shown; any other link is shown as stored.
fn is_discord_attachment(url: &str) -> bool {
    ["https://cdn.discordapp.com/attachments/", "https://media.discordapp.net/attachments/"].iter().any(|p| url.starts_with(p))
}

/// The link without its signature, which is what stays the same between refreshes.
fn unsigned(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

/// Freshly signed links last about a day, so one refresh is good for an hour of page loads.
const FRESH_MS: i64 = 3_600_000;
static FRESH: LazyLock<Mutex<HashMap<String, (String, i64)>>> = LazyLock::new(Default::default);

/// Look up the pictures of posts from before the bot kept them, a few per page load, so each
/// message is fetched once.
pub async fn fill_missing_images(ctx: &Ctx, guild: u64, posts: &mut [Post]) {
    for p in posts.iter_mut().filter(|p| p.image.is_none()).take(6) {
        let image = match ctx.http.message(Id::new(p.channel_id), Id::new(p.message_id)).await {
            Ok(r) => match r.model().await {
                Ok(m) => image_of(&m).unwrap_or_default(),
                Err(_) => continue,
            },
            // Gone, or the bot can't read the channel: that won't change by asking again.
            Err(e) if is_final(&e) => String::new(),
            Err(_) => continue,
        };
        if ctx.db.with(|c| set_image(c, guild, p.message_id, &image)).is_ok() {
            p.image = Some(image);
        }
    }
}

/// A link to show for each stored picture, keyed by the stored link. Discord attachments get a
/// fresh signature, all of them in one request; one that can't be refreshed is left out rather
/// than shown broken.
pub async fn shown_images(ctx: &Ctx, posts: &[Post]) -> HashMap<String, String> {
    let now = now_ms();
    let mut out = HashMap::new();
    let mut stale = Vec::new();
    {
        let mut fresh = FRESH.lock().unwrap();
        fresh.retain(|_, (_, at)| now - *at < FRESH_MS);
        for url in posts.iter().filter_map(|p| p.image.as_deref()).filter(|u| !u.is_empty()) {
            if !is_discord_attachment(url) {
                out.insert(url.to_owned(), url.to_owned());
            } else if let Some((signed, _)) = fresh.get(unsigned(url)) {
                out.insert(url.to_owned(), signed.clone());
            } else {
                stale.push(url.to_owned());
            }
        }
    }
    if stale.is_empty() {
        return out;
    }
    let refreshed = refresh_urls(ctx, &stale).await;
    let mut fresh = FRESH.lock().unwrap();
    for url in stale {
        if let Some(signed) = refreshed.get(unsigned(&url)) {
            fresh.insert(unsigned(&url).to_owned(), (signed.clone(), now));
            out.insert(url, signed.clone());
        }
    }
    out
}

/// Ask Discord to sign attachment links again: unsigned link -> signed link.
async fn refresh_urls(ctx: &Ctx, urls: &[String]) -> HashMap<String, String> {
    #[derive(serde::Deserialize)]
    struct Refreshed {
        original: String,
        refreshed: String,
    }
    #[derive(serde::Deserialize)]
    struct Answer {
        refreshed_urls: Vec<Refreshed>,
    }
    let mut out = HashMap::new();
    let Some(token) = ctx.http.token() else { return out };
    // Discord takes up to 50 links per request.
    for chunk in urls.chunks(50) {
        let sent = ctx
            .web
            .post("https://discord.com/api/v10/attachments/refresh-urls")
            .header("Authorization", token)
            .json(&bot_core::serde_json::json!({ "attachment_urls": chunk }))
            .send()
            .await;
        let answer = match sent {
            Ok(r) if r.status().is_success() => r.json::<Answer>().await,
            Ok(r) => {
                bot_core::tracing::warn!(status = %r.status(), "couldn't refresh starboard image links");
                break;
            }
            Err(e) => {
                bot_core::tracing::warn!(error = %e, "couldn't refresh starboard image links");
                break;
            }
        };
        if let Ok(a) = answer {
            out.extend(a.refreshed_urls.into_iter().map(|r| (unsigned(&r.original).to_owned(), r.refreshed)));
        }
    }
    out
}

// ---- preview ----

pub fn seed(ctx: &Ctx, guild: u64) -> bot_core::anyhow::Result<()> {
    let now = now_ms();
    let h = 3_600_000;
    let general = guild + 40;
    let pets = guild + 60;
    let star = guild + 90;
    // (channel, author index, stars, hours ago, preview)
    let rows: &[(u64, usize, i64, i64, &str)] = &[
        (general, 2, 14, 30, "Finally hit level 100 on the Idleon alt after eight months. The last stretch was just stubbornness."),
        (pets, 4, 9, 52, "Rolled a shiny on the third egg of the day. Not saying it was luck but I sacrificed three hours to get here."),
        (general, 0, 7, 8, "Stream tonight is going to be a long one. Bring snacks."),
        (general, 6, 5, 80, "Attachment"),
        (pets, 1, 4, 5, "Does anyone else feel like the new pet tiers are a bit too generous? Asking for the economy."),
        (general, 7, 3, 120, "Welcome to everyone who joined this week. Read the rules, then say hi."),
    ];
    for (i, (channel, author, stars, ago, preview)) in rows.iter().enumerate() {
        let n = i as u64 + 1;
        // The attachment-only post shows a picture: the welcome card, which the preview draws itself.
        let image = if *preview == "Attachment" { format!("/g/{guild}/welcome/card.png") } else { String::new() };
        ctx.db.with(|c| {
            insert(
                c,
                guild,
                &Post {
                    message_id: 9_000_000_000_000_000_000 + n,
                    channel_id: *channel,
                    author_id: bot_core::preview::MEMBERS[*author].0,
                    post_channel_id: star,
                    post_id: 9_100_000_000_000_000_000 + n,
                    stars: *stars,
                    preview: (*preview).to_owned(),
                    created_at: now - ago * h,
                    removed: false,
                    image: Some(image),
                },
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::Db;

    #[test]
    fn emoji_forms() {
        assert_eq!(parse_emoji("⭐"), Some(Emoji::Unicode("⭐".into())));
        assert_eq!(parse_emoji(" <:pog:123> "), Some(Emoji::Custom(Id::new(123), "pog".into())));
        assert_eq!(parse_emoji("<a:dance:456>"), Some(Emoji::Custom(Id::new(456), "dance".into())));
        assert_eq!(parse_emoji(":star:"), None);
        assert_eq!(parse_emoji("star"), None);
        assert_eq!(parse_emoji("<:broken>"), None);
        assert_eq!(parse_emoji(""), None);
    }

    #[test]
    fn attachment_links_are_told_apart() {
        let signed = "https://cdn.discordapp.com/attachments/1/2/cat.png?ex=65&is=64&hm=ab";
        assert!(is_discord_attachment(signed));
        assert!(is_discord_attachment("https://media.discordapp.net/attachments/1/2/cat.png"));
        assert!(!is_discord_attachment("https://media.tenor.com/x.gif"));
        assert!(!is_discord_attachment("https://cdn.discordapp.com/avatars/1/a.png"));
        assert_eq!(unsigned(signed), "https://cdn.discordapp.com/attachments/1/2/cat.png");
    }

    #[test]
    fn emoji_matching() {
        let star = Emoji::Unicode("⭐".into());
        assert!(star.matches(&EmojiReactionType::Unicode { name: "⭐".into() }));
        assert!(!star.matches(&EmojiReactionType::Unicode { name: "❤".into() }));
        // Hearts come with and without the variation selector.
        let heart = Emoji::Unicode("❤️".into());
        assert!(heart.matches(&EmojiReactionType::Unicode { name: "❤".into() }));
        let custom = Emoji::Custom(Id::new(5), "pog".into());
        assert!(custom.matches(&EmojiReactionType::Custom { animated: false, id: Id::new(5), name: None }));
        assert!(!custom.matches(&EmojiReactionType::Custom { animated: false, id: Id::new(6), name: None }));
        assert!(!custom.matches(&EmojiReactionType::Unicode { name: "⭐".into() }));
    }

    fn post(message: u64, stars: i64, created: i64) -> Post {
        Post { message_id: message, channel_id: 1, author_id: 2, post_channel_id: 3, post_id: message + 1000, stars, preview: "hi".into(), created_at: created, removed: false, image: None }
    }

    #[test]
    fn posts_rank_by_stars_and_removed_ones_drop_out() {
        let db = Db::memory().unwrap();
        db.migrate(KEY, crate::MIGRATIONS).unwrap();
        let c = db.lock();
        insert(&c, 1, &post(10, 3, 100)).unwrap();
        insert(&c, 1, &post(11, 8, 200)).unwrap();
        insert(&c, 1, &post(12, 5, 300)).unwrap();
        insert(&c, 2, &post(13, 99, 300)).unwrap();
        let order: Vec<u64> = top(&c, 1, 10).unwrap().iter().map(|p| p.message_id).collect();
        assert_eq!(order, vec![11, 12, 10]);

        set_stars(&c, 1, 10, 6).unwrap();
        assert_eq!(get(&c, 1, 10).unwrap().unwrap().stars, 6);
        assert_eq!(mark_removed(&c, 1, 1011).unwrap(), 1);
        assert_eq!(mark_removed(&c, 1, 1011).unwrap(), 0);
        assert!(get(&c, 1, 11).unwrap().unwrap().removed);
        assert_eq!(top(&c, 1, 10).unwrap().len(), 2);
        let s = stats(&c, 1, 1000).unwrap();
        assert_eq!(s.posts, 2);

        forget(&c, 1, 12).unwrap();
        assert!(get(&c, 1, 12).unwrap().is_none());
    }
}
