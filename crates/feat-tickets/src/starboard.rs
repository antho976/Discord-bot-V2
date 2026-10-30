//! The starboard: keep a post per popular message and its star count up to date.

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
}

const COLUMNS: &str = "message_id, channel_id, author_id, post_channel_id, post_id, stars, preview, created_at, removed";

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
    })
}

pub fn get(conn: &Connection, guild: u64, message: u64) -> bot_core::rusqlite::Result<Option<Post>> {
    conn.query_row(&format!("SELECT {COLUMNS} FROM starboard_posts WHERE guild_id = ?1 AND message_id = ?2"), params![guild as i64, message as i64], from_row).optional()
}

pub fn insert(conn: &Connection, guild: u64, p: &Post) -> bot_core::rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO starboard_posts (guild_id, message_id, channel_id, author_id, post_channel_id, post_id, stars, preview, created_at, removed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
            p.removed as i64
        ],
    )?;
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

pub struct Stats {
    pub posts: i64,
    pub this_week: i64,
    pub most_stars: i64,
}

pub fn stats(conn: &Connection, guild: u64, now: i64) -> bot_core::rusqlite::Result<Stats> {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(created_at > ?2), 0), COALESCE(MAX(stars), 0) FROM starboard_posts WHERE guild_id = ?1 AND removed = 0",
        params![guild as i64, now - 7 * 86_400_000],
        |r| Ok(Stats { posts: r.get(0)?, this_week: r.get(1)?, most_stars: r.get(2)? }),
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
    emoji: &EmojiReactionType,
) -> bot_core::anyhow::Result<()> {
    let Some(guild) = guild.map(|g| g.get()) else { return Ok(()) };
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let Some(star_channel) = s.star_channel else { return Ok(()) };
    if channel.get() == star_channel || s.star_ignored.contains(&channel.get()) {
        return Ok(());
    }
    let Some(wanted) = parse_emoji(&s.star_emoji) else { return Ok(()) };
    if !wanted.matches(emoji) {
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
            let post = ctx.http.create_message(Id::new(star_channel)).content(&line).embeds(&[embed]).allowed_mentions(Some(&none)).await?.model().await?;
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
    let image = m
        .attachments
        .iter()
        .find(|a| is_image(a))
        .map(|a| a.url.clone())
        .or_else(|| m.embeds.iter().find_map(|e| e.image.as_ref().map(|i| i.url.clone()).or_else(|| e.thumbnail.as_ref().map(|t| t.url.clone()))));
    if let Some(Ok(source)) = image.map(ImageSource::url) {
        embed = embed.image(source);
    }
    let files: Vec<String> = m.attachments.iter().filter(|a| !is_image(a)).take(5).map(|a| format!("[{}]({})", text::clip(&a.filename, 60), a.url)).collect();
    if !files.is_empty() {
        embed = embed.field(EmbedFieldBuilder::new("Attachments", files.join("\n")));
    }
    embed = embed.field(EmbedFieldBuilder::new("Source", format!("[Jump to message](https://discord.com/channels/{guild}/{}/{})", m.channel_id, m.id)));
    embed.build()
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
        Post { message_id: message, channel_id: 1, author_id: 2, post_channel_id: 3, post_id: message + 1000, stars, preview: "hi".into(), created_at: created, removed: false }
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
        assert_eq!((s.posts, s.most_stars), (2, 6));

        forget(&c, 1, 12).unwrap();
        assert!(get(&c, 1, 12).unwrap().is_none());
    }
}
