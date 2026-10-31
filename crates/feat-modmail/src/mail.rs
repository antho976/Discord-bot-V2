//! Conversations: carrying DMs to a staff thread and replies back, opening and closing, and the
//! queries behind the dashboard.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use bot_core::anyhow::{self, Result};
use bot_core::discord::{CommandCall, ComponentCall, Reply, avatar_url, button_row, is_not_found, why_refused_any};
use bot_core::rusqlite::{Connection, OptionalExtension, params};
use bot_core::twilight_http::request::channel::reaction::RequestReactionType;
use bot_core::twilight_model::application::interaction::Interaction;
use bot_core::twilight_model::channel::message::component::{ActionRow, ButtonStyle, Component, SelectMenu, SelectMenuOption, SelectMenuType};
use bot_core::twilight_model::channel::message::{AllowedMentions, Embed, MessageType};
use bot_core::twilight_model::channel::thread::AutoArchiveDuration;
use bot_core::twilight_model::channel::{ChannelType, Message};
use bot_core::twilight_model::guild::Permissions;
use bot_core::twilight_model::id::Id;
use bot_core::twilight_util::builder::embed::{EmbedAuthorBuilder, EmbedBuilder, EmbedFooterBuilder, ImageSource};
use bot_core::{Ctx, now_ms, text};

use crate::{KEY, Modmail, Settings};

const COLOR: u32 = 0xf0806a;
const DAY_MS: i64 = 86_400_000;
/// How long messages wait for a member to pick a server.
const HOLD: Duration = Duration::from_secs(600);
/// The most messages held for one member while they pick.
const MAX_HELD: usize = 10;

pub const OPEN: &str = "open";
pub const CLOSED: &str = "closed";

#[derive(Debug, Clone, PartialEq)]
pub struct Conv {
    pub id: i64,
    pub guild_id: u64,
    pub user_id: u64,
    /// Their name when the conversation opened, for when the member cache doesn't have them.
    pub user_name: String,
    /// 0 while the thread is being made.
    pub thread_id: u64,
    pub status: String,
    pub opened_at: i64,
    pub last_at: i64,
    pub last_author: u64,
    /// The start of the last message, either way.
    pub preview: String,
    pub messages: i64,
    pub closed_at: Option<i64>,
    /// None when the thread was closed in Discord.
    pub closed_by: Option<u64>,
}

impl Conv {
    pub fn is_open(&self) -> bool {
        self.status == OPEN
    }

    /// The member wrote last, so staff owe the reply.
    pub fn waiting_on_staff(&self) -> bool {
        self.last_author == self.user_id
    }

    pub fn name(&self, ctx: &Ctx, guild: u64) -> String {
        ctx.known_name(guild, self.user_id)
            .or_else(|| (!self.user_name.is_empty()).then(|| self.user_name.clone()))
            .unwrap_or_else(|| "Unknown member".into())
    }
}

const COLUMNS: &str = "id, guild_id, user_id, user_name, thread_id, status, opened_at, last_at, last_author_id, preview, messages, closed_at, closed_by";

fn from_row(r: &bot_core::rusqlite::Row<'_>) -> bot_core::rusqlite::Result<Conv> {
    Ok(Conv {
        id: r.get(0)?,
        guild_id: r.get::<_, i64>(1)? as u64,
        user_id: r.get::<_, i64>(2)? as u64,
        user_name: r.get(3)?,
        thread_id: r.get::<_, i64>(4)? as u64,
        status: r.get(5)?,
        opened_at: r.get(6)?,
        last_at: r.get(7)?,
        last_author: r.get::<_, i64>(8)? as u64,
        preview: r.get(9)?,
        messages: r.get(10)?,
        closed_at: r.get(11)?,
        closed_by: r.get::<_, Option<i64>>(12)?.map(|u| u as u64),
    })
}

// ---- queries ----

/// Start a conversation with no thread yet; set it with [`set_thread`] or drop it with
/// [`discard`]. A leftover without a thread (the bot stopped halfway) is cleared first. None
/// when the member already has one open here.
pub fn insert(conn: &Connection, guild: u64, user: u64, name: &str, now: i64) -> bot_core::rusqlite::Result<Option<i64>> {
    conn.execute(
        "DELETE FROM modmail WHERE guild_id = ?1 AND user_id = ?2 AND status = 'open' AND thread_id = 0",
        params![guild as i64, user as i64],
    )?;
    let n = conn.execute(
        "INSERT OR IGNORE INTO modmail (guild_id, user_id, user_name, opened_at, last_at, last_author_id) VALUES (?1, ?2, ?3, ?4, ?4, ?2)",
        params![guild as i64, user as i64, name, now],
    )?;
    Ok((n > 0).then(|| conn.last_insert_rowid()))
}

pub fn set_thread(conn: &Connection, id: i64, thread: u64) -> bot_core::rusqlite::Result<()> {
    conn.execute("UPDATE modmail SET thread_id = ?2 WHERE id = ?1", params![id, thread as i64])?;
    Ok(())
}

pub fn discard(conn: &Connection, id: i64) -> bot_core::rusqlite::Result<()> {
    conn.execute("DELETE FROM modmail WHERE id = ?1 AND thread_id = 0", [id])?;
    Ok(())
}

pub fn by_id(conn: &Connection, guild: u64, id: i64) -> bot_core::rusqlite::Result<Option<Conv>> {
    conn.query_row(&format!("SELECT {COLUMNS} FROM modmail WHERE guild_id = ?1 AND id = ?2"), params![guild as i64, id], from_row).optional()
}

/// The open conversation in a staff thread.
pub fn by_thread(conn: &Connection, guild: u64, thread: u64) -> bot_core::rusqlite::Result<Option<Conv>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM modmail WHERE guild_id = ?1 AND thread_id = ?2 AND status = 'open'"),
        params![guild as i64, thread as i64],
        from_row,
    )
    .optional()
}

pub fn open_in(conn: &Connection, guild: u64, user: u64) -> bot_core::rusqlite::Result<Option<Conv>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM modmail WHERE guild_id = ?1 AND user_id = ?2 AND status = 'open'"),
        params![guild as i64, user as i64],
        from_row,
    )
    .optional()
}

/// The staff threads of every open conversation.
pub fn open_threads(conn: &Connection) -> bot_core::rusqlite::Result<HashSet<u64>> {
    let mut stmt = conn.prepare("SELECT thread_id FROM modmail WHERE status = 'open' AND thread_id != 0")?;
    stmt.query_map([], |r| r.get::<_, i64>(0).map(|t| t as u64))?.collect()
}

/// A member's open conversations in every server.
pub fn open_for_user(conn: &Connection, user: u64) -> bot_core::rusqlite::Result<Vec<Conv>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM modmail WHERE user_id = ?1 AND status = 'open' AND thread_id != 0"))?;
    stmt.query_map([user as i64], from_row)?.collect()
}

/// A message went through: who wrote it, when, and how it starts.
pub fn record(conn: &Connection, id: i64, author: u64, preview: &str, at: i64) -> bot_core::rusqlite::Result<()> {
    conn.execute(
        "UPDATE modmail SET last_at = ?3, last_author_id = ?2, preview = ?4, messages = messages + 1 WHERE id = ?1",
        params![id, author as i64, at, preview],
    )?;
    Ok(())
}

/// Close a conversation that is still open. False when it already wasn't.
pub fn mark_closed(conn: &Connection, id: i64, by: Option<u64>, now: i64) -> bot_core::rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE modmail SET status = 'closed', closed_at = ?2, closed_by = ?3 WHERE id = ?1 AND status = 'open'",
        params![id, now, by.map(|u| u as i64)],
    )?;
    Ok(n > 0)
}

/// Open conversations, the ones waiting on staff first and longest waiting at the top, then
/// the rest by their latest message.
pub fn list_open(conn: &Connection, guild: u64, limit: i64) -> bot_core::rusqlite::Result<Vec<Conv>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM modmail WHERE guild_id = ?1 AND status = 'open' AND thread_id != 0
         ORDER BY last_author_id = user_id DESC, CASE WHEN last_author_id = user_id THEN last_at END, last_at DESC LIMIT ?2"
    ))?;
    stmt.query_map(params![guild as i64, limit], from_row)?.collect()
}

/// Closed conversations, the most recent close first.
pub fn list_closed(conn: &Connection, guild: u64, limit: i64) -> bot_core::rusqlite::Result<Vec<Conv>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM modmail WHERE guild_id = ?1 AND status != 'open' ORDER BY closed_at DESC LIMIT ?2"))?;
    stmt.query_map(params![guild as i64, limit], from_row)?.collect()
}

/// Open conversations where the member has waited on staff since before `before`, longest first.
pub fn waiting_on_staff(conn: &Connection, guild: u64, before: i64, limit: i64) -> bot_core::rusqlite::Result<Vec<Conv>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM modmail WHERE guild_id = ?1 AND status = 'open' AND thread_id != 0 AND last_author_id = user_id AND last_at < ?2
         ORDER BY last_at LIMIT ?3"
    ))?;
    stmt.query_map(params![guild as i64, before, limit], from_row)?.collect()
}

pub struct Stats {
    pub open: i64,
    pub waiting: i64,
    pub opened_week: i64,
    pub closed_week: i64,
}

pub fn stats(conn: &Connection, guild: u64, now: i64) -> bot_core::rusqlite::Result<Stats> {
    let (g, week) = (guild as i64, now - 7 * DAY_MS);
    let (open, waiting) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(last_author_id = user_id), 0) FROM modmail WHERE guild_id = ?1 AND status = 'open' AND thread_id != 0",
        [g],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let opened_week = conn.query_row("SELECT COUNT(*) FROM modmail WHERE guild_id = ?1 AND opened_at > ?2", params![g, week], |r| r.get(0))?;
    let closed_week = conn.query_row("SELECT COUNT(*) FROM modmail WHERE guild_id = ?1 AND closed_at > ?2", params![g, week], |r| r.get(0))?;
    Ok(Stats { open, waiting, opened_week, closed_week })
}

// ---- message text ----

/// Discord's limit on an embed description, in characters.
const MAX_BODY: usize = 4096;
/// Kept free for the line saying some attachments didn't fit.
const MORE_ROOM: usize = 48;

/// A message as it reaches the other side: its text, then its attachments as links, within
/// [`MAX_BODY`]. Links that don't fit are counted on a last line instead.
pub fn body(content: &str, files: &[(String, String)]) -> String {
    let mut out = text::clip(content.trim(), 3500);
    let mut len = out.chars().count();
    let mut shown = 0;
    for (name, url) in files {
        let name: String = name.chars().filter(|c| !matches!(c, '[' | ']')).collect();
        let link = format!("[{}]({url})", text::clip(&name, 80));
        let add = link.chars().count() + usize::from(len > 0);
        if len + add > MAX_BODY - MORE_ROOM {
            break;
        }
        if len > 0 {
            out.push('\n');
        }
        out.push_str(&link);
        len += add;
        shown += 1;
    }
    match files.len() - shown {
        0 => {}
        1 => out.push_str("\n1 more attachment didn't fit here."),
        n => out.push_str(&format!("\n{n} more attachments didn't fit here.")),
    }
    if out.is_empty() {
        out = "Sent something the bot can't show here, like a sticker.".into();
    }
    out
}

/// How a message starts, on one line, for the dashboard.
pub fn preview(content: &str, files: usize) -> String {
    let line = content.split_whitespace().collect::<Vec<_>>().join(" ");
    match (line.is_empty(), files) {
        (false, _) => text::clip(&line, 120),
        (true, 0) => "A sticker or embed".into(),
        (true, 1) => "An attachment".into(),
        (true, n) => format!("{n} attachments"),
    }
}

/// Like `text::duration`, but in days once it passes two of them.
pub fn span(secs: i64) -> String {
    if secs >= 2 * 86_400 {
        let (d, h) = (secs / 86_400, (secs % 86_400) / 3600);
        if h == 0 { format!("{d} days") } else { format!("{d} days {h} h") }
    } else {
        text::duration(secs)
    }
}

/// A DM, kept until it's carried over.
#[derive(Debug, Clone)]
pub(crate) struct Incoming {
    channel: u64,
    message: u64,
    content: String,
    files: Vec<(String, String)>,
    at: i64,
}

impl Incoming {
    fn of(m: &Message) -> Self {
        Self {
            channel: m.channel_id.get(),
            message: m.id.get(),
            content: m.content.clone(),
            files: m.attachments.iter().map(|a| (a.filename.clone(), a.url.clone())).collect(),
            at: m.timestamp.as_micros() / 1000,
        }
    }
}

/// DMs from a member who shares several servers with modmail on, until they pick one.
pub(crate) struct Held {
    since: Instant,
    name: String,
    avatar: String,
    messages: Vec<Incoming>,
}

// ---- where a DM goes ----

/// Modmail is on in this server and has somewhere to put conversations.
fn takes_mail(ctx: &Ctx, guild: u64) -> bool {
    ctx.enabled(guild, KEY) && ctx.cache.guild(Id::new(guild)).is_some() && ctx.settings::<Settings>(guild, KEY).is_ok_and(|s| s.channel.is_some())
}

/// Servers the member is in that take modmail.
fn shared_servers(ctx: &Ctx, user: u64) -> Vec<u64> {
    ctx.enabled_guilds(KEY).into_iter().filter(|g| takes_mail(ctx, *g) && ctx.cache.member(Id::new(*g), Id::new(user)).is_some()).collect()
}

/// Where a member's DM goes: their one open conversation, else the one server they share. None
/// means they have to pick from the list.
fn route(open: &[u64], shared: &[u64]) -> Route {
    match (open, shared) {
        ([one], _) => Route::To(*one),
        ([], []) => Route::Nowhere,
        ([], [one]) => Route::To(*one),
        ([], many) => Route::Pick(many.to_vec()),
        (many, _) => Route::Pick(many.to_vec()),
    }
}

#[derive(Debug, PartialEq)]
enum Route {
    To(u64),
    Pick(Vec<u64>),
    Nowhere,
}

pub async fn on_dm(f: &Modmail, ctx: &Ctx, m: &Message) -> Result<()> {
    // A pin or another system message in the DM isn't something to pass on.
    if !matches!(m.kind, MessageType::Regular | MessageType::Reply) {
        return Ok(());
    }
    let user = m.author.id.get();
    let name = m.author.global_name.clone().unwrap_or_else(|| m.author.name.clone());
    let avatar = avatar_url(user, m.author.avatar.map(|a| a.to_string()).as_deref());
    let incoming = Incoming::of(m);
    let turn = f.turn(user);
    let _turn = turn.lock().await;
    let open: Vec<u64> = ctx.db.with(|c| open_for_user(c, user))?.into_iter().map(|c| c.guild_id).filter(|g| takes_mail(ctx, *g)).collect();
    let shared = if open.is_empty() { shared_servers(ctx, user) } else { Vec::new() };
    match route(&open, &shared) {
        Route::To(guild) => deliver(f, ctx, guild, user, &name, &avatar, &[incoming]).await,
        Route::Pick(guilds) => match f.last_pick(user).filter(|g| guilds.contains(g)) {
            Some(guild) => deliver(f, ctx, guild, user, &name, &avatar, &[incoming]).await,
            None => hold(f, ctx, user, name, avatar, incoming, &guilds).await,
        },
        // Nobody to pass it to: the bot also gets DMs that aren't meant for staff.
        Route::Nowhere => Ok(()),
    }
}

/// Keep the message and, the first time, ask which server it's for.
async fn hold(f: &Modmail, ctx: &Ctx, user: u64, name: String, avatar: String, msg: Incoming, guilds: &[u64]) -> Result<()> {
    let ask = {
        let mut held = f.held.lock().unwrap();
        held.retain(|_, h| h.since.elapsed() < HOLD);
        match held.get_mut(&user) {
            Some(h) => {
                if h.messages.len() < MAX_HELD {
                    h.messages.push(msg);
                }
                false
            }
            None => {
                held.insert(user, Held { since: Instant::now(), name, avatar, messages: vec![msg] });
                true
            }
        }
    };
    if !ask {
        return Ok(());
    }
    let options = guilds
        .iter()
        .take(25)
        .map(|g| SelectMenuOption { default: false, description: None, emoji: None, label: text::clip(&ctx.guild_name(*g), 100), value: g.to_string() })
        .collect();
    let menu = Component::ActionRow(ActionRow {
        id: None,
        components: vec![Component::SelectMenu(SelectMenu {
            id: None,
            channel_types: None,
            custom_id: format!("{KEY}:pick"),
            default_values: None,
            disabled: false,
            kind: SelectMenuType::Text,
            max_values: Some(1),
            min_values: Some(1),
            options: Some(options),
            placeholder: Some("Pick a server".into()),
            required: None,
        })],
    });
    let asked = ctx.dm(user, "Which server's staff is this for? Pick one and your messages go to them.", &[], &[menu]).await?;
    if asked.is_none() {
        f.held.lock().unwrap().remove(&user);
    }
    Ok(())
}

/// Carry DMs into the member's conversation in `guild`, opening one if needed. A thread that
/// was deleted without the bot noticing closes its conversation, and a new one opens.
async fn deliver(f: &Modmail, ctx: &Ctx, guild: u64, user: u64, name: &str, avatar: &str, messages: &[Incoming]) -> Result<()> {
    let s = ctx.settings::<Settings>(guild, KEY)?;
    for attempt in 0..2 {
        let conv = match ctx.db.with(|c| open_in(c, guild, user))?.filter(|c| c.thread_id != 0) {
            Some(c) => c,
            None => match open(f, ctx, guild, &s, user, name).await? {
                Some(c) => c,
                None => return Ok(()),
            },
        };
        match relay_in(ctx, &conv, name, avatar, messages).await {
            Ok(()) => return Ok(()),
            Err(e) if attempt == 0 && e.downcast_ref::<bot_core::twilight_http::Error>().is_some_and(is_not_found) => {
                ctx.db.with(|c| mark_closed(c, conv.id, None, now_ms()))?;
                f.drop_thread(conv.thread_id);
            }
            Err(e) => {
                bot_core::tracing::warn!(guild, thread = conv.thread_id, error = %e, "couldn't carry a DM to staff");
                ctx.notify_once(guild, KEY, "relay", format!("A DM couldn't reach the staff thread. {}", why_refused_any(&e)), Some(format!("/g/{guild}/modmail")), Duration::from_secs(86_400));
                let text = format!("Couldn't pass that on to the staff of {}. Try again in a while.", ctx.guild_name(guild));
                ctx.dm(user, &text, &[], &[]).await?;
                return Ok(());
            }
        }
    }
    Ok(())
}

async fn relay_in(ctx: &Ctx, conv: &Conv, name: &str, avatar: &str, messages: &[Incoming]) -> Result<()> {
    let none = AllowedMentions::default();
    for m in messages {
        let embed = embed(name, avatar, &body(&m.content, &m.files), None);
        ctx.http.create_message(Id::new(conv.thread_id)).embeds(&[embed]).allowed_mentions(Some(&none)).await?;
        let _ = ctx.http.create_reaction(Id::new(m.channel), Id::new(m.message), &RequestReactionType::Unicode { name: "✅" }).await;
        ctx.db.with(|c| record(c, conv.id, conv.user_id, &preview(&m.content, m.files.len()), m.at))?;
    }
    Ok(())
}

fn embed(author: &str, avatar: &str, description: &str, footer: Option<&str>) -> Embed {
    let mut a = EmbedAuthorBuilder::new(text::clip(author, 200));
    if let Ok(icon) = ImageSource::url(avatar) {
        a = a.icon_url(icon);
    }
    let mut e = EmbedBuilder::new().color(COLOR).author(a).description(description);
    if let Some(f) = footer {
        e = e.footer(EmbedFooterBuilder::new(text::clip(f, 200)));
    }
    e.build()
}

/// Open a conversation: a thread (or forum post) in the staff channel, then the greeting by DM.
async fn open(f: &Modmail, ctx: &Ctx, guild: u64, s: &Settings, user: u64, name: &str) -> Result<Option<Conv>> {
    let Some(channel) = s.channel else { return Ok(None) };
    let Some(id) = ctx.db.with(|c| insert(c, guild, user, name, now_ms()))? else {
        return ctx.db.with(|c| open_in(c, guild, user));
    };
    let thread = match make_thread(ctx, channel, s, id, user, name).await {
        Ok(t) => t,
        Err(e) => {
            bot_core::tracing::warn!(guild, channel, error = %e, "couldn't open a modmail thread");
            ctx.db.with(|c| discard(c, id))?;
            ctx.notify_once(guild, KEY, "open", format!("A modmail conversation couldn't open. {}", why_refused_any(&e)), Some(format!("/g/{guild}/modmail?view=settings")), Duration::from_secs(86_400));
            let text = format!("Couldn't reach the staff of {} right now. Try again in a while.", ctx.guild_name(guild));
            ctx.dm(user, &text, &[], &[]).await?;
            return Ok(None);
        }
    };
    ctx.db.with(|c| set_thread(c, id, thread))?;
    f.add_thread(thread);
    let server = ctx.guild_name(guild);
    let greeting = if s.greeting.trim().is_empty() { crate::DEFAULT_GREETING } else { s.greeting.trim() };
    ctx.dm(user, &text::fill(greeting, &[("server", &server)]), &[], &[]).await?;
    ctx.count(guild, "modmail_opened", 1);
    ctx.notify(Some(guild), KEY, format!("New modmail from {name}"), Some(format!("/g/{guild}/modmail")));
    ctx.db.with(|c| by_id(c, guild, id))
}

async fn make_thread(ctx: &Ctx, channel: u64, s: &Settings, id: i64, user: u64, name: &str) -> Result<u64> {
    let intro = EmbedBuilder::new()
        .color(COLOR)
        .title(text::clip(name, 200))
        .description("Reply in this thread to answer by DM. Start a message with // to keep it among staff.")
        .build();
    let close = button_row(&[(&format!("{KEY}:close:{id}"), "Close", ButtonStyle::Secondary)]);
    let (content, mentions) = match s.staff_role {
        Some(role) => (format!("<@&{role}>, new modmail from <@{user}>."), AllowedMentions { roles: vec![Id::new(role)], ..Default::default() }),
        None => (format!("New modmail from <@{user}>."), AllowedMentions::default()),
    };
    let title = text::clip(name, 90);
    let forum = ctx.cache.channel(Id::new(channel)).is_some_and(|c| c.kind == ChannelType::GuildForum);
    if forum {
        let post = ctx
            .http
            .create_forum_thread(Id::new(channel), &title)
            .auto_archive_duration(AutoArchiveDuration::Week)
            .message()
            .content(&content)
            .embeds(&[intro])
            .components(&[close])
            .allowed_mentions(Some(&mentions))
            .await?
            .model()
            .await?;
        return Ok(post.channel.id.get());
    }
    let thread = ctx
        .http
        .create_thread(Id::new(channel), &title, ChannelType::PublicThread)
        .auto_archive_duration(AutoArchiveDuration::Week)
        .await?
        .model()
        .await?;
    ctx.http.create_message(thread.id).content(&content).embeds(&[intro]).components(&[close]).allowed_mentions(Some(&mentions)).await?;
    Ok(thread.id.get())
}

// ---- staff replies ----

/// Every message in a server where modmail is on comes through here, so the channel is checked
/// against the open conversations' threads, held in memory, before anything else. That set, not
/// the current staff channel, decides: conversations opened before the channel changed keep
/// working in their old threads.
pub async fn on_staff_message(f: &Modmail, ctx: &Ctx, guild: u64, m: &Message) -> Result<()> {
    if !matches!(m.kind, MessageType::Regular | MessageType::Reply) || m.content.trim_start().starts_with("//") || !f.is_staff_thread(ctx, m.channel_id.get())? {
        return Ok(());
    }
    let Some(conv) = ctx.db.with(|c| by_thread(c, guild, m.channel_id.get()))? else {
        f.drop_thread(m.channel_id.get());
        return Ok(());
    };
    let staff = m.author.id.get();
    let staff_name = m
        .member
        .as_ref()
        .and_then(|p| p.nick.clone())
        .or_else(|| m.author.global_name.clone())
        .unwrap_or_else(|| m.author.name.clone());
    let files: Vec<(String, String)> = m.attachments.iter().map(|a| (a.filename.clone(), a.url.clone())).collect();
    let avatar = ctx.member_avatar(guild, staff);
    let reply = embed(&staff_name, &avatar, &body(&m.content, &files), Some(&ctx.guild_name(guild)));
    match dm_embed(ctx, conv.user_id, reply).await {
        Ok(()) => {
            let _ = ctx.http.create_reaction(m.channel_id, m.id, &RequestReactionType::Unicode { name: "✅" }).await;
            ctx.db.with(|c| record(c, conv.id, staff, &preview(&m.content, files.len()), m.timestamp.as_micros() / 1000))?;
        }
        Err(why) => {
            ctx.say(m.channel_id.get(), &format!("Couldn't DM {}. {why}", conv.name(ctx, guild))).await?;
        }
    }
    Ok(())
}

/// DM an embed to a member. The error says why it didn't go, for staff: closed DMs and a refused
/// message read differently. (`ctx.dm` reports both as closed DMs.)
async fn dm_embed(ctx: &Ctx, user: u64, embed: Embed) -> std::result::Result<(), String> {
    let channel = match ctx.http.create_private_channel(Id::new(user)).await {
        Ok(r) => r.model().await.map_err(|_| "Discord didn't open a DM with them.".to_owned())?,
        Err(e) => return Err(why_no_dm(&e)),
    };
    let none = AllowedMentions::default();
    ctx.http.create_message(channel.id).embeds(&[embed]).allowed_mentions(Some(&none)).await.map(|_| ()).map_err(|e| why_no_dm(&e))
}

fn why_no_dm(e: &bot_core::twilight_http::Error) -> String {
    match e.kind() {
        bot_core::twilight_http::error::ErrorType::Response { status, .. } if status.get() == 403 => {
            "Their DMs are closed, or they left the server.".into()
        }
        _ => text::clip(&format!("Discord refused the message: {e}"), 300),
    }
}

// ---- closing ----

fn is_staff(i: &Interaction, s: &Settings) -> bool {
    let Some(member) = i.member.as_ref() else { return false };
    let role = s.staff_role.is_some_and(|r| member.roles.iter().any(|x| x.get() == r));
    let perms = member.permissions.is_some_and(|p| {
        p.contains(Permissions::ADMINISTRATOR) || p.intersects(Permissions::MANAGE_THREADS | Permissions::MANAGE_MESSAGES | Permissions::MANAGE_GUILD)
    });
    role || perms
}

/// Tell the member the conversation is closed, with the server's closing text.
async fn send_closing(ctx: &Ctx, guild: u64, user: u64) {
    let s = ctx.settings::<Settings>(guild, KEY).unwrap_or_default();
    let closing = if s.closing.trim().is_empty() { crate::DEFAULT_CLOSING } else { s.closing.trim() };
    let text = text::fill(closing, &[("server", &ctx.guild_name(guild))]);
    if let Err(e) = ctx.dm(user, &text, &[], &[]).await {
        bot_core::tracing::warn!(guild, user, error = %e, "couldn't send the modmail closing text");
    }
}

async fn archive(ctx: &Ctx, thread: u64) {
    if let Err(e) = ctx.http.update_thread(Id::new(thread)).archived(true).locked(true).await {
        bot_core::tracing::warn!(thread, error = %e, "couldn't archive a closed modmail thread");
    }
}

/// Close from the button or the command, in the thread itself.
async fn close_here(f: &Modmail, ctx: &Ctx, i: &Interaction, guild: u64, conv: &Conv) -> Result<()> {
    let Some(by) = i.author_id().map(|u| u.get()) else { return Ok(()) };
    if !ctx.db.with(|c| mark_closed(c, conv.id, Some(by), now_ms()))? {
        return ctx.reply(i, Reply::text("This conversation is already closed.").ephemeral()).await;
    }
    f.drop_thread(conv.thread_id);
    ctx.reply(i, Reply::text(format!("Closed by <@{by}>. {} gets the closing text by DM.", conv.name(ctx, guild)))).await?;
    send_closing(ctx, guild, conv.user_id).await;
    archive(ctx, conv.thread_id).await;
    Ok(())
}

/// The dashboard's Close button. False when the conversation wasn't open.
pub async fn close_from_dashboard(ctx: &Ctx, guild: u64, id: i64, by: u64) -> Result<bool> {
    let Some(conv) = ctx.db.with(|c| by_id(c, guild, id))? else { return Ok(false) };
    if !ctx.db.with(|c| mark_closed(c, id, Some(by), now_ms()))? {
        return Ok(false);
    }
    send_closing(ctx, guild, conv.user_id).await;
    if conv.thread_id != 0 {
        let _ = ctx.say(conv.thread_id, &format!("Closed from the dashboard by <@{by}>.")).await;
        archive(ctx, conv.thread_id).await;
    }
    Ok(true)
}

/// A staff thread was deleted, or archived and locked, outside the bot. Threads in an earlier
/// staff channel count too.
pub async fn on_thread_gone(f: &Modmail, ctx: &Ctx, guild: u64, thread: u64) -> Result<()> {
    if !f.is_staff_thread(ctx, thread)? {
        return Ok(());
    }
    f.drop_thread(thread);
    let Some(conv) = ctx.db.with(|c| by_thread(c, guild, thread))? else { return Ok(()) };
    if ctx.db.with(|c| mark_closed(c, conv.id, None, now_ms()))? {
        send_closing(ctx, guild, conv.user_id).await;
    }
    Ok(())
}

// ---- commands and components ----

pub async fn on_command(f: &Modmail, ctx: &Ctx, call: &CommandCall) -> Result<()> {
    let Some(guild) = call.guild_id() else { return Ok(()) };
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let conv = match call.channel_id() {
        Some(thread) => ctx.db.with(|c| by_thread(c, guild, thread))?,
        None => None,
    };
    let Some(conv) = conv else {
        return ctx.reply(&call.interaction, Reply::text("This isn't an open modmail thread.").ephemeral()).await;
    };
    if !is_staff(&call.interaction, &s) {
        return ctx.reply(&call.interaction, Reply::text("Only staff can close modmail.").ephemeral()).await;
    }
    close_here(f, ctx, &call.interaction, guild, &conv).await
}

pub async fn on_component(f: &Modmail, ctx: &Ctx, call: &ComponentCall) -> Result<()> {
    match call.args().as_slice() {
        ["pick"] => pick(f, ctx, call).await,
        ["close", id] => {
            let Some(guild) = call.guild_id() else { return Ok(()) };
            let Ok(id) = id.parse::<i64>() else { return Ok(()) };
            let s = ctx.settings::<Settings>(guild, KEY)?;
            let Some(conv) = ctx.db.with(|c| by_id(c, guild, id))?.filter(Conv::is_open) else {
                return ctx.reply(&call.interaction, Reply::text("This conversation is already closed.").ephemeral()).await;
            };
            if !is_staff(&call.interaction, &s) {
                return ctx.reply(&call.interaction, Reply::text("Only staff can close modmail.").ephemeral()).await;
            }
            close_here(f, ctx, &call.interaction, guild, &conv).await
        }
        _ => Ok(()),
    }
}

/// A member picked a server for the messages they're holding.
async fn pick(f: &Modmail, ctx: &Ctx, call: &ComponentCall) -> Result<()> {
    let user = call.user_id();
    let Some(guild) = call.data.values.first().and_then(|v| v.parse::<u64>().ok()) else { return Ok(()) };
    let allowed = takes_mail(ctx, guild)
        && (ctx.cache.member(Id::new(guild), Id::new(user)).is_some() || ctx.db.with(|c| open_in(c, guild, user))?.is_some());
    if !allowed {
        return ctx.update(&call.interaction, Reply::text("That server doesn't take modmail now. Send your message again to pick another.")).await;
    }
    let held = f.held.lock().unwrap().remove(&user).filter(|h| h.since.elapsed() < HOLD);
    let Some(held) = held else {
        return ctx.update(&call.interaction, Reply::text("That choice timed out. Send your message again.")).await;
    };
    f.remember_pick(user, guild);
    let text = format!("Sending to the staff of {}. What you send in the next 10 minutes goes there too.", ctx.guild_name(guild));
    ctx.update(&call.interaction, Reply::text(text)).await?;
    let turn = f.turn(user);
    let _turn = turn.lock().await;
    deliver(f, ctx, guild, user, &held.name, &held.avatar, &held.messages).await
}

// ---- preview ----

pub fn seed(ctx: &Ctx, guild: u64) -> anyhow::Result<()> {
    use bot_core::preview::MEMBERS;
    let now = now_ms();
    let (m, h) = (60_000, 3_600_000);
    let staff = [MEMBERS[1].0, MEMBERS[0].0];
    // (member, opened ago, last message ago, staff wrote last, messages, closed ago and by, preview)
    type Row<'a> = (usize, i64, i64, bool, i64, Option<(i64, Option<usize>)>, &'a str);
    let rows: &[Row] = &[
        (9, 30 * h, 26 * h, false, 2, None, "I was muted yesterday and nobody told me why. Can someone look at it?"),
        (4, 5 * h, 3 * h, false, 4, None, "Someone in #trading sent me a link that looks like a fake Steam login. Screenshots attached."),
        (5, 2 * DAY_MS, 40 * m, true, 7, None, "We've removed their posts. Tell us if they DM you again."),
        (6, 25 * m, 20 * m, false, 1, None, "Is it fine to post my commission prices in #general, or is there a better place?"),
        (7, 6 * h, 2 * h + 10 * m, true, 5, Some((2 * h, Some(0))), "Sorted, your role is back. Thanks for the patience."),
        (8, 2 * DAY_MS, DAY_MS + 3 * h, true, 3, Some((DAY_MS + 2 * h, Some(1))), "The giveaway draw is redone on Friday. You're still in."),
        (3, 4 * DAY_MS, 3 * DAY_MS + 5 * h, false, 2, Some((3 * DAY_MS, None)), "ok thanks"),
        (2, 6 * DAY_MS, 5 * DAY_MS + 2 * h, true, 6, Some((5 * DAY_MS, Some(0))), "Appeal accepted. Welcome back, read #rules once more."),
    ];
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM modmail WHERE guild_id = ?1", [guild as i64])?;
    for (i, (who, opened, last, staff_last, count, closed, text)) in rows.iter().enumerate() {
        let (user, name) = MEMBERS[*who];
        let author = if *staff_last { staff[i % 2] } else { user };
        tx.execute(
            "INSERT INTO modmail (guild_id, user_id, user_name, thread_id, status, opened_at, last_at, last_author_id, preview, messages, closed_at, closed_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                guild as i64,
                user as i64,
                name,
                (guild + 7000 + i as u64) as i64,
                if closed.is_some() { CLOSED } else { OPEN },
                now - opened,
                now - last,
                author as i64,
                text,
                count,
                closed.map(|(ago, _)| now - ago),
                closed.and_then(|(_, by)| by).map(|b| staff[b] as i64)
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::Db;

    fn db() -> Db {
        let db = Db::memory().unwrap();
        db.migrate(KEY, crate::MIGRATIONS).unwrap();
        db
    }

    #[test]
    fn one_open_conversation_per_member_and_server() {
        let db = db();
        let c = db.lock();
        let a = insert(&c, 1, 10, "Mira", 1000).unwrap().unwrap();
        set_thread(&c, a, 500).unwrap();
        assert_eq!(insert(&c, 1, 10, "Mira", 2000).unwrap(), None);
        // Another server, or another member, opens their own.
        assert!(insert(&c, 2, 10, "Mira", 2000).unwrap().is_some());
        assert!(insert(&c, 1, 11, "Oskar", 2000).unwrap().is_some());
        // Once closed, the member can open again.
        assert!(mark_closed(&c, a, Some(99), 3000).unwrap());
        assert!(!mark_closed(&c, a, Some(99), 4000).unwrap());
        assert!(insert(&c, 1, 10, "Mira", 5000).unwrap().is_some());
    }

    #[test]
    fn a_half_opened_conversation_does_not_block_the_next() {
        let db = db();
        let c = db.lock();
        // The bot stopped between the row and the thread.
        insert(&c, 1, 10, "Mira", 1000).unwrap().unwrap();
        let again = insert(&c, 1, 10, "Mira", 2000).unwrap().unwrap();
        set_thread(&c, again, 500).unwrap();
        assert_eq!(open_for_user(&c, 10).unwrap().len(), 1);
        // A failed thread gives the slot back; one with its thread stays.
        discard(&c, again).unwrap();
        assert!(by_id(&c, 1, again).unwrap().is_some());
    }

    #[test]
    fn staff_threads_are_the_open_ones() {
        let db = db();
        let c = db.lock();
        let a = insert(&c, 1, 10, "Mira", 1000).unwrap().unwrap();
        set_thread(&c, a, 500).unwrap();
        let b = insert(&c, 2, 11, "Oskar", 1000).unwrap().unwrap();
        set_thread(&c, b, 501).unwrap();
        // Still making its thread.
        insert(&c, 1, 12, "Ada", 1000).unwrap().unwrap();
        mark_closed(&c, b, None, 2000).unwrap();
        assert_eq!(open_threads(&c).unwrap(), HashSet::from([500]));
    }

    #[test]
    fn waiting_follows_who_wrote_last() {
        let db = db();
        let c = db.lock();
        let a = insert(&c, 1, 10, "Mira", 1000).unwrap().unwrap();
        set_thread(&c, a, 500).unwrap();
        let b = insert(&c, 1, 11, "Oskar", 1000).unwrap().unwrap();
        set_thread(&c, b, 501).unwrap();
        record(&c, a, 10, "hello", 2000).unwrap();
        record(&c, b, 99, "answered", 2000).unwrap();
        let waiting: Vec<i64> = waiting_on_staff(&c, 1, 5000, 10).unwrap().iter().map(|c| c.id).collect();
        assert_eq!(waiting, vec![a]);
        assert!(waiting_on_staff(&c, 1, 1500, 10).unwrap().is_empty());
        let s = stats(&c, 1, 5000).unwrap();
        assert_eq!((s.open, s.waiting, s.opened_week, s.closed_week), (2, 1, 2, 0));
        // Waiting ones come first on the dashboard.
        assert_eq!(list_open(&c, 1, 10).unwrap()[0].id, a);
        let conv = by_thread(&c, 1, 501).unwrap().unwrap();
        assert_eq!((conv.messages, conv.preview.as_str(), conv.last_author), (1, "answered", 99));
    }

    #[test]
    fn dms_go_to_the_one_place_or_ask() {
        assert_eq!(route(&[1], &[]), Route::To(1));
        assert_eq!(route(&[], &[2]), Route::To(2));
        assert_eq!(route(&[], &[]), Route::Nowhere);
        assert_eq!(route(&[], &[2, 3]), Route::Pick(vec![2, 3]));
        assert_eq!(route(&[1, 4], &[]), Route::Pick(vec![1, 4]));
    }

    #[test]
    fn bodies_carry_attachments_as_links() {
        let files = vec![("shot [1].png".to_owned(), "https://cdn/x.png".to_owned())];
        assert_eq!(body("look", &files), "look\n[shot 1.png](https://cdn/x.png)");
        assert_eq!(body("", &files), "[shot 1.png](https://cdn/x.png)");
        assert!(body("  ", &[]).starts_with("Sent something"));
        // Long text and long links stay within an embed, and say what was left out.
        let long: Vec<(String, String)> = (0..10).map(|i| (format!("{i}.png"), format!("https://cdn/{}", "a".repeat(300)))).collect();
        let full = body(&"x".repeat(5000), &long);
        assert!(full.chars().count() <= MAX_BODY);
        assert!(full.ends_with("9 more attachments didn't fit here."), "{full}");
        assert_eq!(body("", &long[..1]).lines().count(), 1);
        assert_eq!(preview("two\nlines", 0), "two lines");
        assert_eq!(preview("", 1), "An attachment");
        assert_eq!(preview("", 3), "3 attachments");
    }

    #[test]
    fn spans_switch_to_days() {
        assert_eq!(span(3 * 3600 + 120), "3 h 2 min");
        assert_eq!(span(2 * 86_400 + 3 * 3600), "2 days 3 h");
    }
}
