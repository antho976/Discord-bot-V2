//! The starboard: keep a post per popular message and its star count up to date.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use bot_core::rusqlite::{Connection, OptionalExtension, params};
use bot_core::twilight_http::error::ErrorType;
use bot_core::twilight_http::request::channel::reaction::RequestReactionType;
use bot_core::twilight_model::channel::permission_overwrite::{PermissionOverwrite, PermissionOverwriteType};
use bot_core::twilight_model::channel::{ChannelType, Message};
use bot_core::twilight_model::guild::Permissions;
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

fn set_content(conn: &Connection, guild: u64, message: u64, preview: &str, image: &str) -> bot_core::rusqlite::Result<()> {
    conn.execute(
        "UPDATE starboard_posts SET preview = ?3, image = ?4 WHERE guild_id = ?1 AND message_id = ?2",
        params![guild as i64, message as i64, preview, image],
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

/// Messages were deleted. In the starboard channel, a post a moderator took down is remembered,
/// so its message isn't posted again. Anywhere else, a starred message that was deleted takes
/// its post down with it.
pub async fn on_message_delete(
    feature: &Tickets,
    ctx: &Ctx,
    guild: Option<Id<GuildMarker>>,
    channel: Id<ChannelMarker>,
    messages: &[Id<MessageMarker>],
) -> bot_core::anyhow::Result<()> {
    let Some(guild) = guild.map(|g| g.get()) else { return Ok(()) };
    let s = ctx.settings::<Settings>(guild, KEY)?;
    if s.star_channel == Some(channel.get()) {
        for m in messages {
            // The bot's own deletes (a post taken down below the threshold) aren't a moderator's.
            if !ctx.own_message_deleted(m.get()) {
                ctx.db.with(|c| mark_removed(c, guild, m.get()).map(|_| ()))?;
            }
        }
        return Ok(());
    }
    // Read under the lock: a post being made right now is stored before this looks for it.
    let lock = feature.star_lock(guild);
    let _one_at_a_time = lock.lock().await;
    let posts = ctx.db.with(|c| messages.iter().filter_map(|m| get(c, guild, m.get()).transpose()).collect::<bot_core::rusqlite::Result<Vec<Post>>>())?;
    for post in posts {
        if !post.removed
            && let Err(e) = take_down(ctx, &post).await
        {
            bot_core::tracing::warn!(guild, message = post.message_id, error = %e, "couldn't take down the starboard post of a deleted message");
            continue;
        }
        ctx.db.with(|c| forget(c, guild, post.message_id))?;
    }
    Ok(())
}

/// A message was edited: if it's on the starboard, rewrite its post to match.
pub async fn on_message_update(feature: &Tickets, ctx: &Ctx, guild: u64, message: &Message) -> bot_core::anyhow::Result<()> {
    // Embeds filling in and pins send updates too; only an edit by its author changes the post.
    if message.edited_timestamp.is_none() {
        return Ok(());
    }
    let lock = feature.star_lock(guild);
    let _one_at_a_time = lock.lock().await;
    let Some(post) = ctx.db.with(|c| get(c, guild, message.id.get()))?.filter(|p| !p.removed) else { return Ok(()) };
    let embed = build_embed(guild, message);
    match ctx.http.update_message(Id::new(post.post_channel_id), Id::new(post.post_id)).embeds(Some(&[embed])).await {
        Ok(_) => ctx.db.with(|c| set_content(c, guild, post.message_id, &preview_of(message), &image_of(message).unwrap_or_default()))?,
        // Someone deleted the post by hand: leave it deleted.
        Err(e) if is_not_found(&e) => ctx.db.with(|c| mark_removed(c, guild, post.post_id).map(|_| ()))?,
        Err(e) => bot_core::tracing::warn!(guild, message = post.message_id, error = %e, "couldn't update the starboard post of an edited message"),
    }
    Ok(())
}

/// Delete a post from the starboard. One that is already gone counts as taken down.
async fn take_down(ctx: &Ctx, post: &Post) -> Result<(), bot_core::twilight_http::Error> {
    // The audit log skips the bot's own deletes when it ignores bots.
    ctx.deleting_own_message(post.post_id);
    match ctx.http.delete_message(Id::new(post.post_channel_id), Id::new(post.post_id)).await {
        Err(e) if !is_not_found(&e) => Err(e),
        _ => Ok(()),
    }
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
    let Some(star) = Id::new_checked(star_channel) else { return Ok(()) };
    let star_nsfw = ctx.cache.channel(star).is_some_and(|c| c.nsfw.unwrap_or(false));
    let Some(place) = place_of(ctx, guild, channel, star) else { return Ok(()) };
    if !may_repost(&place, star_channel, star_nsfw, &s.star_ignored) {
        // Hidden from part of the starboard's audience, and nothing else in the way: say so on
        // the dashboard once the message has the stars, so a quiet starboard isn't a mystery.
        if place.hidden && emoji.is_some() && may_repost(&Place { hidden: false, ..place.clone() }, star_channel, star_nsfw, &s.star_ignored) {
            note_hidden(ctx, guild, &place, channel, message, &wanted, s.star_threshold.max(1)).await;
        }
        return Ok(());
    }
    let lock = feature.star_lock(guild);
    let _one_at_a_time = lock.lock().await;
    match refresh(ctx, guild, channel.get(), message.get(), &s, &wanted, star_channel).await {
        Ok(Some(at)) => recheck_at(ctx, lock.clone(), guild, channel.get(), message.get(), at),
        Ok(None) => {}
        Err(e) => bot_core::tracing::warn!(guild, message = message.get(), error = %e, "couldn't update the starboard"),
    }
    Ok(())
}

/// Look at a post again once it's no longer young (see [`KEEP_MS`]), so one kept up under the
/// threshold comes down even if nobody reacts again. One look per post at a time, each at most
/// [`KEEP_MS`] away; the look only updates or takes down a post that's still there.
fn recheck_at(ctx: &Ctx, lock: std::sync::Arc<bot_core::tokio::sync::Mutex<()>>, guild: u64, channel: u64, message: u64, at: i64) {
    static PENDING: LazyLock<Mutex<std::collections::HashSet<(u64, u64)>>> = LazyLock::new(Default::default);
    if !PENDING.lock().unwrap().insert((guild, message)) {
        return;
    }
    let ctx = ctx.clone();
    bot_core::tokio::spawn(async move {
        let wait = (at - now_ms()).clamp(0, KEEP_MS) as u64 + 1_000;
        bot_core::tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
        PENDING.lock().unwrap().remove(&(guild, message));
        let _one_at_a_time = lock.lock().await;
        let Ok(s) = ctx.settings::<Settings>(guild, KEY) else { return };
        let (Some(star_channel), Some(wanted)) = (s.star_channel, parse_emoji(&s.star_emoji)) else { return };
        if !ctx.db.with(|c| get(c, guild, message)).ok().flatten().is_some_and(|p| !p.removed) {
            return;
        }
        if let Err(e) = refresh(&ctx, guild, channel, message, &s, &wanted, star_channel).await {
            bot_core::tracing::warn!(guild, message, error = %e, "couldn't update the starboard");
        }
    });
}

/// A star in a channel the starboard won't repost from. At most one look a channel each 10
/// minutes, and one notice a channel a week, once a message there reaches the threshold.
async fn note_hidden(ctx: &Ctx, guild: u64, place: &Place, channel: Id<ChannelMarker>, message: Id<MessageMarker>, emoji: &Emoji, threshold: i64) {
    static LOOKED: LazyLock<Mutex<HashMap<u64, Instant>>> = LazyLock::new(Default::default);
    {
        let mut looked = LOOKED.lock().unwrap();
        looked.retain(|_, t| t.elapsed().as_secs() < 600);
        if looked.contains_key(&place.channel) {
            return;
        }
        looked.insert(place.channel, Instant::now());
    }
    let Ok(r) = ctx.http.message(channel, message).await else { return };
    let Ok(m) = r.model().await else { return };
    let raw = m.reactions.iter().find(|r| emoji.matches(&r.emoji)).map_or(0, |r| r.count as i64);
    if raw < threshold {
        return;
    }
    let name = ctx.cache.channel(Id::new(place.channel)).and_then(|c| c.name.clone()).unwrap_or_else(|| "a channel".into());
    ctx.notify_once(
        guild,
        KEY,
        &format!("star-hidden-{}", place.channel),
        format!("Starred messages in #{name} aren't reposted: some people who can see the starboard can't see #{name}."),
        Some(format!("/g/{guild}/{KEY}/starboard?view=settings")),
        std::time::Duration::from_secs(7 * 86_400),
    );
}

/// Where a starred message is, as far as the starboard cares.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    /// The channel, or for a thread the channel it's in.
    pub channel: u64,
    /// A private thread, like a ticket.
    pub private_thread: bool,
    /// Some role that can see the starboard can't see the channel (a thread's parent).
    pub hidden: bool,
    /// The channel (a thread's parent) is age-restricted.
    pub nsfw: bool,
}

/// Whether a message may be reposted: nothing from a private thread or a channel hidden from
/// some of the people who can see the starboard, nothing age-restricted unless the starboard is
/// too, and nothing from the starboard or an ignored channel, threads included.
pub fn may_repost(p: &Place, star_channel: u64, star_nsfw: bool, ignored: &[u64]) -> bool {
    p.channel != star_channel && !ignored.contains(&p.channel) && !p.private_thread && !p.hidden && (!p.nsfw || star_nsfw)
}

/// Where a message is, from the cache. The cache holds every channel and every thread the bot
/// can see (active ones arrive with the server, new ones as they're made), so None, a channel
/// it doesn't know (or a starboard it doesn't know), means no repost rather than a guess.
fn place_of(ctx: &Ctx, guild: u64, channel: Id<ChannelMarker>, star: Id<ChannelMarker>) -> Option<Place> {
    let roles = server_roles(ctx, guild);
    // One cache entry at a time: each is let go before the next is read.
    let star_overwrites = ctx.cache.channel(star).map(|c| c.permission_overwrites.clone().unwrap_or_default())?;
    let (kind, parent_id) = ctx.cache.channel(channel).map(|c| (c.kind, c.parent_id))?;
    let thread = matches!(kind, ChannelType::PublicThread | ChannelType::PrivateThread | ChannelType::AnnouncementThread);
    let private_thread = kind == ChannelType::PrivateThread;
    let parent = ctx.cache.channel(if thread { parent_id? } else { channel })?;
    Some(Place {
        channel: parent.id.get(),
        private_thread,
        hidden: !audience_covered(&roles, guild, &star_overwrites, parent.permission_overwrites.as_deref().unwrap_or_default()),
        nsfw: parent.nsfw.unwrap_or(false),
    })
}

/// Every role of a server with its permissions, @everyone (the server's id) included.
fn server_roles(ctx: &Ctx, guild: u64) -> Vec<(u64, Permissions)> {
    let Some(g) = Id::new_checked(guild) else { return Vec::new() };
    let ids: Vec<_> = ctx.cache.guild_roles(g).map(|set| set.iter().copied().collect()).unwrap_or_default();
    let mut roles: Vec<(u64, Permissions)> = ids.into_iter().filter_map(|id| ctx.cache.role(id).map(|r| (id.get(), r.permissions))).collect();
    if !roles.iter().any(|r| r.0 == guild) {
        roles.push((guild, Permissions::empty()));
    }
    roles
}

/// Whether everyone who can see the starboard can see the channel too, judged role by role: a
/// role's view is @everyone's permissions with its own, then the channel's overwrite for
/// @everyone, then its own. Administrator sees everything. Overwrites for single members are
/// left out.
pub fn audience_covered(roles: &[(u64, Permissions)], guild: u64, star: &[PermissionOverwrite], source: &[PermissionOverwrite]) -> bool {
    roles.iter().all(|(role, _)| !role_sees(roles, guild, *role, star) || role_sees(roles, guild, *role, source))
}

fn role_sees(roles: &[(u64, Permissions)], guild: u64, role: u64, overwrites: &[PermissionOverwrite]) -> bool {
    let perms = |id: u64| roles.iter().find(|r| r.0 == id).map_or(Permissions::empty(), |r| r.1);
    let base = perms(guild) | perms(role);
    if base.contains(Permissions::ADMINISTRATOR) {
        return true;
    }
    let mut view = base.contains(Permissions::VIEW_CHANNEL);
    for id in [guild, role] {
        if let Some(o) = overwrites.iter().find(|o| o.kind == PermissionOverwriteType::Role && o.id.get() == id) {
            if o.deny.contains(Permissions::VIEW_CHANNEL) {
                view = false;
            }
            if o.allow.contains(Permissions::VIEW_CHANNEL) {
                view = true;
            }
        }
    }
    view
}

/// Fetch the message and its star count, then post, update or take down its starboard post.
/// Returns when to look again: a post kept up under the threshold only because it's young.
async fn refresh(ctx: &Ctx, guild: u64, channel: u64, message_id: u64, s: &Settings, emoji: &Emoji, star_channel: u64) -> bot_core::anyhow::Result<Option<i64>> {
    let (Some(channel_id), Some(message_key)) = (Id::new_checked(channel), Id::new_checked(message_id)) else { return Ok(None) };
    let existing = ctx.db.with(|c| get(c, guild, message_id))?;
    if existing.as_ref().is_some_and(|p| p.removed) {
        return Ok(None);
    }
    let message = match ctx.http.message(channel_id, message_key).await {
        Ok(r) => r.model().await?,
        Err(e) if is_not_found(&e) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let threshold = s.star_threshold.max(1);
    let reaction = message.reactions.iter().find(|r| emoji.matches(&r.emoji));
    // Normal reactions and super reactions together.
    let raw = reaction.map_or(0, |r| r.count as i64);
    let burst = reaction.map_or(0, |r| r.count_details.burst as i64);
    // Who starred it only needs reading when it could change what happens, and then only one
    // page: past a page of reactors a bot or the author's own star changes nothing.
    let matters = raw > 0 && (raw >= threshold || existing.is_some()) && raw <= REACTORS_PAGE as i64;
    let stars = if matters { counted_stars(ctx, &message, emoji, s.star_self, burst > 0).await.unwrap_or(raw) } else { raw };
    let line = format!("{} {} in <#{}>", s.star_emoji.trim(), stars, channel);
    let none = AllowedMentions::default();

    match existing {
        Some(post) if stays_up(stars, threshold, s.star_remove_below, now_ms() - post.created_at) => {
            let recheck = (stars < threshold && s.star_remove_below).then_some(post.created_at + KEEP_MS);
            if post.stars == stars {
                return Ok(recheck);
            }
            let edited = ctx.http.update_message(Id::new(post.post_channel_id), Id::new(post.post_id)).content(Some(&line)).allowed_mentions(Some(&none)).await;
            match edited {
                Ok(_) => ctx.db.with(|c| set_stars(c, guild, message_id, stars))?,
                // Someone deleted the post by hand: leave it deleted.
                Err(e) if is_not_found(&e) => {
                    ctx.db.with(|c| mark_removed(c, guild, post.post_id).map(|_| ()))?;
                    return Ok(None);
                }
                Err(e) => return Err(e.into()),
            }
            return Ok(recheck);
        }
        Some(post) => {
            // Forgotten only once it's gone, so a failed delete can't leave a post the next
            // crossing would duplicate.
            take_down(ctx, &post).await?;
            ctx.db.with(|c| forget(c, guild, message_id))?;
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
    Ok(None)
}

/// Whether a post stays up at this count: at or over the threshold, when posts aren't taken down
/// under it, or while it's young (`age` under [`KEEP_MS`]) so one member taking a star back and
/// giving it again doesn't delete and repost it each time. Not at no stars at all, which is
/// reactions cleared, not a dip.
fn stays_up(stars: i64, threshold: i64, remove_below: bool, age: i64) -> bool {
    stars >= threshold || !remove_below || (stars > 0 && age < KEEP_MS)
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

/// Reactors Discord returns in one request.
const REACTORS_PAGE: u16 = 100;

/// A post younger than this stays up when its count dips under the threshold.
const KEEP_MS: i64 = 10 * 60_000;

/// The stars that count, from one page of reactors: people, not bots, and the author only when
/// `with_author`. Discord lists normal and super reactions apart; with `burst` the super ones are
/// read too, and someone who gave both counts once. None when a list can't be read.
async fn counted_stars(ctx: &Ctx, message: &Message, emoji: &Emoji, with_author: bool, burst: bool) -> Option<i64> {
    use bot_core::twilight_model::channel::message::ReactionType;
    let request = emoji.request();
    let mut users = ctx.http.reactions(message.channel_id, message.id, &request).limit(REACTORS_PAGE).await.ok()?.models().await.ok()?;
    if burst {
        let supers = ctx.http.reactions(message.channel_id, message.id, &request).kind(ReactionType::Burst).limit(REACTORS_PAGE).await.ok()?.models().await.ok()?;
        users.extend(supers);
    }
    Some(count_stars(&users, message.author.id, with_author))
}

fn count_stars(users: &[bot_core::twilight_model::user::User], author: Id<bot_core::twilight_model::id::marker::UserMarker>, with_author: bool) -> i64 {
    let mut seen = std::collections::HashSet::new();
    users.iter().filter(|u| !u.bot && (with_author || u.id != author) && seen.insert(u.id)).count() as i64
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

/// The dashboard's "Send a test post": a sample repost in the starboard channel, credited to
/// whoever pressed it, marked as a test. Nothing is starred or stored.
pub async fn test_post(ctx: &Ctx, s: &Settings, who: &str, avatar: &str) -> bot_core::anyhow::Result<()> {
    let channel = s.star_channel.ok_or_else(|| bot_core::anyhow::anyhow!("no starboard channel"))?;
    let mut author = EmbedAuthorBuilder::new(who);
    if let Ok(icon) = ImageSource::url(avatar) {
        author = author.icon_url(icon);
    }
    let embed = EmbedBuilder::new()
        .color(COLOR)
        .author(author)
        .description("A message that gets enough stars is reposted here like this, with a link back to it.")
        .build();
    let line = format!("{} {}\n-# Test post. Nothing was starred.", s.star_emoji.trim(), s.star_threshold.max(1));
    let none = AllowedMentions::default();
    ctx.http.create_message(Id::new(channel)).content(&line).embeds(&[embed]).allowed_mentions(Some(&none)).await?;
    Ok(())
}

/// Discord refuses an embed field longer than this.
const FIELD_MAX: usize = 1024;

/// The Attachments field: a link per file, five at most, while they fit in a field, then
/// "and N more". Signed attachment links run 200 characters and more, so a few fill it.
fn attachments_field(files: &[(&str, &str)]) -> Option<String> {
    // Discord counts UTF-16 units; counting them never undercounts.
    let len = |s: &str| s.encode_utf16().count();
    let more_line = |n: usize| format!("and {n} more");
    let mut lines: Vec<String> = Vec::new();
    let mut used = 0;
    for (i, (name, url)) in files.iter().enumerate().take(5) {
        let line = format!("[{}]({})", text::clip(name, 60), url);
        let after = files.len() - i - 1;
        // This line, and the "and N more" line in case nothing more fits after it.
        let need = used + usize::from(!lines.is_empty()) + len(&line) + if after > 0 { 1 + len(&more_line(after)) } else { 0 };
        if need > FIELD_MAX {
            break;
        }
        used += usize::from(!lines.is_empty()) + len(&line);
        lines.push(line);
    }
    let left = files.len() - lines.len();
    match (lines.is_empty(), left) {
        (true, 0) => None,
        (true, n) => Some(if n == 1 { "1 file".to_owned() } else { format!("{n} files") }),
        (false, 0) => Some(lines.join("\n")),
        (false, n) => Some(format!("{}\n{}", lines.join("\n"), more_line(n))),
    }
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
    let files: Vec<(&str, &str)> = m.attachments.iter().filter(|a| !is_image(a)).map(|a| (a.filename.as_str(), a.url.as_str())).collect();
    if let Some(value) = attachments_field(&files) {
        embed = embed.field(EmbedFieldBuilder::new("Attachments", value));
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
    // (channel, author index, stars, hours ago, preview, sample picture)
    let rows: &[(u64, usize, i64, i64, &str, &str)] = &[
        (general, 2, 14, 30, "Finally hit level 100 on the Idleon alt after eight months. The last stretch was just stubbornness.", ""),
        (general, 5, 11, 20, "Took the long way home and found this. Worth the extra hour.", "banner.jpg"),
        (pets, 4, 9, 52, "Rolled a shiny on the third egg of the day. Not saying it was luck but I sacrificed three hours to get here.", ""),
        (general, 0, 7, 8, "Stream tonight is going to be a long one. Bring snacks.", "stream.jpg"),
        (general, 6, 5, 80, "Attachment", "lake.jpg"),
        (pets, 4, 4, 5, "Does anyone else feel like the new pet tiers are a bit too generous? Asking for the economy.", ""),
        (general, 0, 3, 120, "Welcome to everyone who joined this week. Read the rules, then say hi.", ""),
    ];
    for (i, (channel, author, stars, ago, preview, picture)) in rows.iter().enumerate() {
        let n = i as u64 + 1;
        let image = if picture.is_empty() { String::new() } else { bot_core::preview::picture(picture) };
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

    #[test]
    fn only_public_places_reach_the_starboard() {
        let open = Place { channel: 10, private_thread: false, hidden: false, nsfw: false };
        assert!(may_repost(&open, 99, false, &[]));
        // The starboard itself, an ignored channel (or a thread in one, which carries its parent).
        assert!(!may_repost(&Place { channel: 99, ..open.clone() }, 99, false, &[]));
        assert!(!may_repost(&open, 99, false, &[10]));
        // A ticket, a staff channel, an age-restricted channel on an ordinary starboard.
        assert!(!may_repost(&Place { private_thread: true, ..open.clone() }, 99, false, &[]));
        assert!(!may_repost(&Place { hidden: true, ..open.clone() }, 99, false, &[]));
        assert!(!may_repost(&Place { nsfw: true, ..open.clone() }, 99, false, &[]));
        assert!(may_repost(&Place { nsfw: true, ..open }, 99, true, &[]));
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn places_come_from_the_cache_with_threads_under_their_parent() {
        use bot_core::twilight_model::gateway::event::Event;
        use bot_core::twilight_model::gateway::payload::incoming::ChannelCreate;
        let ctx = Ctx::new(bot_core::Config::preview(0), Db::memory().unwrap()).unwrap();
        let everyone = bot_core::serde_json::json!({"guild_id": "1", "role": {"id": "1", "name": "@everyone", "color": 0, "colors": {"primary_color": 0}, "hoist": false, "managed": false, "mentionable": false, "permissions": (1u64 << 10).to_string(), "position": 0, "flags": 0}});
        ctx.cache.update(&Event::RoleCreate(bot_core::serde_json::from_value(everyone).unwrap()));
        // 10 public, 11 hidden from @everyone, 12 age-restricted; 500 a public thread in 11,
        // 501 a private thread in 10, 502 a public thread in 10; 99 the starboard.
        let deny_view = bot_core::serde_json::json!([{"id": "1", "type": 0, "allow": "0", "deny": (1u64 << 10).to_string()}]);
        let none = bot_core::serde_json::json!([]);
        for (id, kind, parent, overwrites, nsfw) in
            [(99u64, 0u8, None, &none, false), (10, 0, None, &none, false), (11, 0, None, &deny_view, false), (12, 0, None, &none, true), (500, 11, Some(11u64), &none, false), (501, 12, Some(10), &none, false), (502, 11, Some(10), &none, false)]
        {
            let channel = bot_core::serde_json::json!({
                "id": id.to_string(), "type": kind, "name": "x", "guild_id": "1", "parent_id": parent.map(|p| p.to_string()), "nsfw": nsfw,
                "permission_overwrites": overwrites, "thread_metadata": {"archived": false, "auto_archive_duration": 60, "archive_timestamp": "2026-01-01T00:00:00.000000+00:00", "locked": false}
            });
            ctx.cache.update(&Event::ChannelCreate(Box::new(ChannelCreate(bot_core::serde_json::from_value(channel).unwrap()))));
        }
        let place = |id: u64| place_of(&ctx, 1, Id::new(id), Id::new(99));
        assert_eq!(place(10), Some(Place { channel: 10, private_thread: false, hidden: false, nsfw: false }));
        assert!(place(11).unwrap().hidden && place(12).unwrap().nsfw);
        assert_eq!(place(500), Some(Place { channel: 11, private_thread: false, hidden: true, nsfw: false }));
        assert_eq!(place(501), Some(Place { channel: 10, private_thread: true, hidden: false, nsfw: false }));
        // Ignoring 10 covers its threads.
        assert!(may_repost(&place(10).unwrap(), 99, false, &[]));
        assert!(!may_repost(&place(502).unwrap(), 99, false, &[10]));
        // Unknown to the cache, or a starboard it doesn't know: no repost.
        assert_eq!(place(999), None);
        assert_eq!(place_of(&ctx, 1, Id::new(10), Id::new(998)), None);
    }

    fn overwrite(role: u64, allow: Permissions, deny: Permissions) -> PermissionOverwrite {
        PermissionOverwrite { allow, deny, id: Id::new(role), kind: PermissionOverwriteType::Role }
    }

    #[test]
    fn the_starboards_audience_decides_what_is_hidden() {
        let (view, none) = (Permissions::VIEW_CHANNEL, Permissions::empty());
        // @everyone (1) sees nothing; Member (5) sees the server; Staff (6) too; Admin (7).
        let gated = [(1, none), (5, view), (6, view), (7, Permissions::ADMINISTRATOR)];
        let staff_only = [overwrite(1, none, view), overwrite(5, none, view), overwrite(6, view, none)];
        // Role-gated server, ordinary channels and a starboard members see: reposted.
        assert!(audience_covered(&gated, 1, &[], &[]));
        // A staff channel on a starboard members see: hidden.
        assert!(!audience_covered(&gated, 1, &[], &staff_only));
        // A staff starboard can take the staff channel.
        assert!(audience_covered(&gated, 1, &staff_only, &staff_only));
        // A channel only verified members see (@everyone denied, Member allowed) on a starboard
        // the same people see.
        let members = [overwrite(1, none, view), overwrite(5, view, none)];
        assert!(audience_covered(&gated, 1, &members, &members));
        assert!(audience_covered(&gated, 1, &members, &[]));
        // An open server: @everyone sees the starboard, so a channel hidden from it is hidden.
        let open = [(1, view), (5, none)];
        assert!(audience_covered(&open, 1, &[], &[]));
        assert!(!audience_covered(&open, 1, &[], &[overwrite(1, none, view), overwrite(5, view, none)]));
        // Administrators see everything, so they never make a channel hidden.
        assert!(role_sees(&gated, 1, 7, &staff_only));
    }

    #[test]
    fn bots_and_the_author_dont_count() {
        let user = |id: u64, bot: bool| -> bot_core::twilight_model::user::User {
            bot_core::serde_json::from_value(bot_core::serde_json::json!({"id": id.to_string(), "username": "u", "discriminator": "0", "avatar": null, "bot": bot})).unwrap()
        };
        let users = [user(1, false), user(2, false), user(3, true), user(4, false)];
        assert_eq!(count_stars(&users, Id::new(4), false), 2);
        assert_eq!(count_stars(&users, Id::new(4), true), 3);
        assert_eq!(count_stars(&[], Id::new(4), true), 0);
        // Normal and super reactors read one after the other: 2 normal and 1 super make 3, and
        // someone who gave both counts once.
        let both = [user(1, false), user(2, false), user(5, false), user(2, false)];
        assert_eq!(count_stars(&both, Id::new(4), false), 3);
    }

    #[test]
    fn young_posts_ride_out_a_dip_but_not_a_clear() {
        let young = KEEP_MS - 1;
        assert!(stays_up(3, 3, true, KEEP_MS * 10));
        assert!(stays_up(2, 3, true, young), "a dip while young");
        assert!(!stays_up(2, 3, true, KEEP_MS), "a dip once it's settled");
        assert!(!stays_up(0, 3, true, young), "reactions cleared");
        assert!(stays_up(0, 3, false, KEEP_MS), "posts aren't taken down under the threshold");
    }

    #[test]
    fn attachment_links_fit_in_one_field() {
        let url = format!("https://cdn.discordapp.com/attachments/1/2/report.pdf?ex=1&is=2&hm={}", "a".repeat(180));
        let files: Vec<(&str, &str)> = (0..7).map(|_| ("report.pdf", url.as_str())).collect();
        let field = attachments_field(&files).unwrap();
        assert!(field.encode_utf16().count() <= FIELD_MAX, "{}", field.len());
        let links = field.lines().filter(|l| l.starts_with('[')).count();
        assert!((1..5).contains(&links), "{links}");
        assert_eq!(field.lines().last().unwrap(), format!("and {} more", 7 - links));
        // Short links: up to five, then the rest counted.
        let short: Vec<(&str, &str)> = (0..7).map(|_| ("a.txt", "https://x/a.txt")).collect();
        let field = attachments_field(&short).unwrap();
        assert_eq!(field.lines().count(), 6);
        assert!(field.ends_with("and 2 more"));
        assert_eq!(attachments_field(&short[..2]).unwrap(), "[a.txt](https://x/a.txt)\n[a.txt](https://x/a.txt)");
        assert_eq!(attachments_field(&[]), None);
        // One link too long for a field on its own.
        let huge = "x".repeat(2000);
        assert_eq!(attachments_field(&[("a", huge.as_str())]).unwrap(), "1 file");
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
