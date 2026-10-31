//! Tickets: the panel, opening and closing, the idle sweep, and the queries behind the dashboard.

use bot_core::discord::{CommandCall, ComponentCall, ModalCall, Reply, button_row};
use bot_core::rusqlite::{Connection, OptionalExtension, params};
use bot_core::twilight_http::error::ErrorType;
use bot_core::twilight_model::application::interaction::Interaction;
use bot_core::twilight_model::channel::ChannelType;
use bot_core::twilight_model::channel::message::AllowedMentions;
use bot_core::twilight_model::channel::message::component::ButtonStyle;
use bot_core::twilight_model::channel::thread::AutoArchiveDuration;
use bot_core::twilight_model::guild::Permissions;
use bot_core::twilight_model::id::Id;
use bot_core::twilight_util::builder::embed::{EmbedAuthorBuilder, EmbedBuilder, EmbedFooterBuilder, ImageSource};
use bot_core::{Ctx, now_ms, text};

use crate::{KEY, Settings, Tickets};

const COLOR: u32 = 0xf0806a;
const DAY_MS: i64 = 86_400_000;

pub const OPEN: &str = "open";
pub const CLOSED: &str = "closed";
pub const IDLE: &str = "idle";

#[derive(Debug, Clone, PartialEq)]
pub struct Ticket {
    pub number: i64,
    pub opener_id: u64,
    pub subject: String,
    /// What the member wrote when opening it.
    pub details: String,
    pub status: String,
    pub thread_id: u64,
    pub opened_at: i64,
    pub closed_at: Option<i64>,
    pub closed_by: Option<u64>,
    pub reason: Option<String>,
    /// Who wrote last in the thread, bots aside. None on tickets opened before this was kept.
    pub last_author: Option<u64>,
    pub last_message_at: Option<i64>,
    /// When someone other than the opener first wrote in the thread.
    pub first_reply_at: Option<i64>,
}

impl Ticket {
    pub fn is_open(&self) -> bool {
        self.status == OPEN
    }

    /// Whose turn it is, from what the bot stored. None when it doesn't know.
    pub fn waiting_on(&self) -> Option<Turn> {
        let last = self.last_author?;
        Some(match (last == self.opener_id, self.first_reply_at) {
            (true, None) => Turn::NoReplyYet,
            (true, Some(_)) => Turn::Staff,
            (false, _) => Turn::Member,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Turn {
    /// The member wrote and nobody else has said anything yet.
    NoReplyYet,
    /// The member wrote last, after staff had answered before.
    Staff,
    /// Staff wrote last.
    Member,
}

const COLUMNS: &str =
    "number, opener_id, subject, status, thread_id, opened_at, closed_at, closed_by, close_reason, details, last_author_id, last_message_at, first_reply_at";

fn from_row(r: &bot_core::rusqlite::Row<'_>) -> bot_core::rusqlite::Result<Ticket> {
    Ok(Ticket {
        number: r.get(0)?,
        opener_id: r.get::<_, i64>(1)? as u64,
        subject: r.get(2)?,
        status: r.get(3)?,
        thread_id: r.get::<_, i64>(4)? as u64,
        opened_at: r.get(5)?,
        closed_at: r.get(6)?,
        closed_by: r.get::<_, Option<i64>>(7)?.map(|u| u as u64),
        reason: r.get(8)?,
        details: r.get(9)?,
        last_author: r.get::<_, Option<i64>>(10)?.map(|u| u as u64),
        last_message_at: r.get(11)?,
        first_reply_at: r.get(12)?,
    })
}

// ---- queries ----

pub enum Reserved {
    Number(i64),
    /// The member is at their limit; this is their newest open ticket's thread.
    Full(u64),
}

/// Take the next ticket number for a member, unless they already have `max` open. The row has no
/// thread yet; set it with [`set_thread`] or give the number back with [`release`].
pub fn reserve(conn: &mut Connection, guild: u64, user: u64, subject: &str, details: &str, max: i64, now: i64) -> bot_core::rusqlite::Result<Reserved> {
    let tx = conn.transaction()?;
    if let Some(thread) = full_thread(&tx, guild, user, max)? {
        return Ok(Reserved::Full(thread));
    }
    let number: i64 = tx.query_row("SELECT COALESCE(MAX(number), 0) + 1 FROM tickets WHERE guild_id = ?1", [guild as i64], |r| r.get(0))?;
    tx.execute(
        "INSERT INTO tickets (guild_id, number, opener_id, subject, details, status, thread_id, opened_at, last_author_id, last_message_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'open', 0, ?6, ?3, ?6)",
        params![guild as i64, number, user as i64, subject, details, now],
    )?;
    tx.commit()?;
    Ok(Reserved::Number(number))
}

/// The newest open thread of a member who already has `max` tickets open.
pub fn full_thread(conn: &Connection, guild: u64, user: u64, max: i64) -> bot_core::rusqlite::Result<Option<u64>> {
    let open: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tickets WHERE guild_id = ?1 AND opener_id = ?2 AND status = 'open'",
        params![guild as i64, user as i64],
        |r| r.get(0),
    )?;
    if open < max.max(1) {
        return Ok(None);
    }
    conn.query_row(
        "SELECT thread_id FROM tickets WHERE guild_id = ?1 AND opener_id = ?2 AND status = 'open' ORDER BY number DESC LIMIT 1",
        params![guild as i64, user as i64],
        |r| r.get::<_, i64>(0).map(|t| Some(t as u64)),
    )
}

pub fn set_thread(conn: &Connection, guild: u64, number: i64, thread: u64) -> bot_core::rusqlite::Result<()> {
    conn.execute("UPDATE tickets SET thread_id = ?3 WHERE guild_id = ?1 AND number = ?2", params![guild as i64, number, thread as i64])?;
    Ok(())
}

pub fn release(conn: &Connection, guild: u64, number: i64) -> bot_core::rusqlite::Result<()> {
    conn.execute("DELETE FROM tickets WHERE guild_id = ?1 AND number = ?2 AND thread_id = 0", params![guild as i64, number])?;
    Ok(())
}

pub fn by_number(conn: &Connection, guild: u64, number: i64) -> bot_core::rusqlite::Result<Option<Ticket>> {
    conn.query_row(&format!("SELECT {COLUMNS} FROM tickets WHERE guild_id = ?1 AND number = ?2"), params![guild as i64, number], from_row).optional()
}

pub fn by_thread(conn: &Connection, guild: u64, thread: u64) -> bot_core::rusqlite::Result<Option<Ticket>> {
    conn.query_row(&format!("SELECT {COLUMNS} FROM tickets WHERE guild_id = ?1 AND thread_id = ?2"), params![guild as i64, thread as i64], from_row).optional()
}

/// Close a ticket that is still open. False when it already wasn't.
pub fn mark_closed(conn: &Connection, guild: u64, number: i64, by: Option<u64>, reason: Option<&str>, status: &str, now: i64) -> bot_core::rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE tickets SET status = ?3, closed_at = ?4, closed_by = ?5, close_reason = ?6
         WHERE guild_id = ?1 AND number = ?2 AND status = 'open'",
        params![guild as i64, number, status, now, by.map(|u| u as i64), reason],
    )?;
    Ok(n > 0)
}

/// Staff can reopen a ticket for this long after it closed.
pub const REOPEN_DAYS: i64 = 7;

impl Ticket {
    /// Whether staff can still reopen it: closed in the last [`REOPEN_DAYS`], with a thread to go back to.
    pub fn can_reopen(&self, now: i64) -> bool {
        !self.is_open() && self.thread_id != 0 && self.closed_at.is_some_and(|at| now - at <= REOPEN_DAYS * DAY_MS)
    }
}

/// Set a closed ticket open again. False when it was already open. The waiting clock starts at
/// `now`, so the days it spent closed don't count as waiting on staff.
pub fn mark_reopened(conn: &Connection, guild: u64, number: i64, now: i64) -> bot_core::rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE tickets SET status = 'open', closed_at = NULL, closed_by = NULL, close_reason = NULL, last_message_at = ?3
         WHERE guild_id = ?1 AND number = ?2 AND status != 'open'",
        params![guild as i64, number, now],
    )?;
    Ok(n > 0)
}

#[derive(Clone, Copy, PartialEq)]
pub enum Filter {
    Open,
    Closed,
}

pub fn list(conn: &Connection, guild: u64, filter: Filter, limit: i64) -> bot_core::rusqlite::Result<Vec<Ticket>> {
    let clause = match filter {
        Filter::Open => "AND status = 'open'",
        Filter::Closed => "AND status != 'open'",
    };
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM tickets WHERE guild_id = ?1 {clause} ORDER BY number DESC LIMIT ?2"))?;
    stmt.query_map(params![guild as i64, limit], from_row)?.collect()
}

/// Tickets that have closed, the most recent close first.
pub fn recently_closed(conn: &Connection, guild: u64, limit: i64) -> bot_core::rusqlite::Result<Vec<Ticket>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM tickets WHERE guild_id = ?1 AND status != 'open' ORDER BY closed_at DESC LIMIT ?2"))?;
    stmt.query_map(params![guild as i64, limit], from_row)?.collect()
}

/// Who closed the most tickets since `since`: `(member, closed)`, busiest first. A member closing
/// their own ticket isn't answering anyone, so those don't count.
pub fn closers(conn: &Connection, guild: u64, since: i64, limit: i64) -> bot_core::rusqlite::Result<Vec<(u64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT closed_by, COUNT(*) FROM tickets WHERE guild_id = ?1 AND closed_by IS NOT NULL AND closed_by != opener_id AND closed_at >= ?2
         GROUP BY closed_by ORDER BY 2 DESC, 1 LIMIT ?3",
    )?;
    stmt.query_map(params![guild as i64, since, limit], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?)))?.collect()
}

/// Open tickets where the member has been waiting on staff since before `before`, longest wait
/// first. A ticket from before the bot kept the last author counts from when it opened.
pub fn waiting_on_staff(conn: &Connection, guild: u64, before: i64, limit: i64) -> bot_core::rusqlite::Result<Vec<Ticket>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM tickets WHERE guild_id = ?1 AND status = 'open'
           AND (last_author_id IS NULL OR last_author_id = opener_id) AND COALESCE(last_message_at, opened_at) < ?2
         ORDER BY COALESCE(last_message_at, opened_at) LIMIT ?3"
    ))?;
    stmt.query_map(params![guild as i64, before, limit], from_row)?.collect()
}

/// A message in a ticket thread: note who wrote it, and when staff first answered.
pub fn record_message(conn: &Connection, guild: u64, thread: u64, author: u64, at: i64) -> bot_core::rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE tickets SET last_author_id = ?3, last_message_at = ?4,
           first_reply_at = COALESCE(first_reply_at, CASE WHEN ?3 != opener_id THEN ?4 END)
         WHERE guild_id = ?1 AND thread_id = ?2 AND status = 'open'",
        params![guild as i64, thread as i64, author as i64, at],
    )?;
    Ok(n > 0)
}

/// Every message in a server goes through here, so only private threads reach the database.
pub fn on_message(ctx: &Ctx, guild: u64, channel: u64, author: u64, bot: bool, at: i64) -> bot_core::anyhow::Result<()> {
    if bot {
        return Ok(());
    }
    let private_thread = ctx.cache.channel(Id::new(channel)).is_some_and(|c| c.kind == ChannelType::PrivateThread);
    if private_thread {
        ctx.db.with(|c| record_message(c, guild, channel, author, at))?;
    }
    Ok(())
}

pub struct Stats {
    pub open: i64,
    /// Open tickets where the member wrote last, counted like [`waiting_on_staff`]: a ticket
    /// from before the bot kept the last author counts as waiting.
    pub waiting: i64,
    pub this_week: i64,
    /// Median seconds from opening to closing, over tickets closed in the last 30 days.
    pub median_close_secs: Option<i64>,
}

pub fn stats(conn: &Connection, guild: u64, now: i64) -> bot_core::rusqlite::Result<Stats> {
    let g = guild as i64;
    let (open, waiting) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(last_author_id IS NULL OR last_author_id = opener_id), 0) FROM tickets WHERE guild_id = ?1 AND status = 'open'",
        [g],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let this_week = conn.query_row("SELECT COUNT(*) FROM tickets WHERE guild_id = ?1 AND opened_at > ?2", params![g, now - 7 * DAY_MS], |r| r.get(0))?;
    let mut stmt = conn.prepare(
        "SELECT (closed_at - opened_at) / 1000 FROM tickets
         WHERE guild_id = ?1 AND status = 'closed' AND closed_at > ?2 ORDER BY 1",
    )?;
    let secs: Vec<i64> = stmt.query_map(params![g, now - 30 * DAY_MS], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(Stats { open, waiting, this_week, median_close_secs: median(&secs) })
}

/// The middle of a sorted list; the mean of the two middle values when there is an even count.
pub fn median(sorted: &[i64]) -> Option<i64> {
    let n = sorted.len();
    match n {
        0 => None,
        _ if n % 2 == 1 => Some(sorted[n / 2]),
        _ => Some((sorted[n / 2 - 1] + sorted[n / 2]) / 2),
    }
}

/// A thread was deleted or archived outside the bot: stop counting its ticket as open.
pub fn on_thread_gone(ctx: &Ctx, guild: u64, thread: u64, why: &str) -> bot_core::anyhow::Result<()> {
    if !ctx.enabled(guild, KEY) {
        return Ok(());
    }
    ctx.db.with(|c| thread_gone(c, thread, why, now_ms()).map(|_| ()))
}

fn thread_gone(conn: &Connection, thread: u64, why: &str, now: i64) -> bot_core::rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE tickets SET status = 'closed', closed_at = ?2, close_reason = ?3 WHERE thread_id = ?1 AND status = 'open'",
        params![thread as i64, now, why],
    )?;
    Ok(n > 0)
}

pub fn snowflake_ms(id: u64) -> i64 {
    ((id >> 22) + 1_420_070_400_000) as i64
}

fn is_not_found(e: &bot_core::twilight_http::Error) -> bool {
    matches!(e.kind(), ErrorType::Response { status, .. } if status.get() == 404)
}

// ---- who is allowed ----

/// Who is behind an interaction: id, name and avatar. None in the odd case Discord leaves the
/// author out, which would otherwise become user 0.
fn user_of(i: &Interaction) -> Option<(u64, String, String)> {
    let user = i.author();
    let id = user.map(|u| u.id.get())?;
    let name = i
        .member
        .as_ref()
        .and_then(|m| m.nick.clone())
        .or_else(|| user.and_then(|u| u.global_name.clone()))
        .or_else(|| user.map(|u| u.name.clone()))
        .unwrap_or_else(|| "Member".into());
    let avatar = bot_core::discord::avatar_url(id, user.and_then(|u| u.avatar.map(|a| a.to_string())).as_deref());
    Some((id, name, avatar))
}

/// What to tell a member who is at their open-ticket limit. Thread 0 is a ticket still being
/// opened: they pressed the button twice.
fn already_open(thread: u64) -> String {
    if thread == 0 {
        "Your ticket is still being opened. It shows up in a moment.".into()
    } else {
        format!("You already have an open ticket: <#{thread}>. Close it before opening another.")
    }
}

fn has_perm(i: &Interaction, perms: Permissions) -> bool {
    i.member.as_ref().and_then(|m| m.permissions).is_some_and(|p| p.contains(Permissions::ADMINISTRATOR) || p.intersects(perms))
}

fn can_close(i: &Interaction, s: &Settings, t: &Ticket) -> bool {
    if i.author_id().is_some_and(|u| u.get() == t.opener_id) {
        return true;
    }
    let staff = s.staff_role.is_some_and(|role| i.member.as_ref().is_some_and(|m| m.roles.iter().any(|r| r.get() == role)));
    staff || has_perm(i, Permissions::MANAGE_GUILD | Permissions::MANAGE_THREADS)
}

fn channel_label(ctx: &Ctx, guild: u64, channel: u64) -> String {
    ctx.text_channels(guild).into_iter().find(|(id, _)| *id == channel).map(|(_, n)| format!("#{n}")).unwrap_or_else(|| "the ticket channel".into())
}

// ---- the panel ----

/// Post the "Open a ticket" panel in the ticket channel and take down the previous one.
pub async fn send_panel(ctx: &Ctx, guild: u64) -> Result<u64, String> {
    let s = ctx.settings::<Settings>(guild, KEY).map_err(|e| e.to_string())?;
    let Some(channel) = s.ticket_channel else { return Err("Pick a ticket channel and save before sending the panel.".into()) };
    let title = if s.panel_title.trim().is_empty() { crate::DEFAULT_PANEL_TITLE } else { s.panel_title.as_str() };
    let body = if s.panel_text.trim().is_empty() { crate::DEFAULT_PANEL_TEXT } else { s.panel_text.as_str() };
    let embed = EmbedBuilder::new().title(title).description(body).color(COLOR).build();
    let button = button_row(&[(&format!("{KEY}:open"), "Open a ticket", ButtonStyle::Primary)]);
    let none = AllowedMentions::default();
    let sent = ctx.http.create_message(Id::new(channel)).embeds(&[embed]).components(&[button]).allowed_mentions(Some(&none)).await;
    let message = match sent {
        Ok(r) => r.model().await.map_err(|e| e.to_string())?,
        Err(e) => {
            bot_core::tracing::warn!(guild, channel, error = %e, "couldn't post the ticket panel");
            return Err(format!("Couldn't post in {}. The bot needs to see it and send embeds there.", channel_label(ctx, guild, channel)));
        }
    };
    if let Some((old_channel, old_message)) = s.panel {
        let _ = ctx.http.delete_message(Id::new(old_channel), Id::new(old_message)).await;
    }
    let mut next = (*s).clone();
    next.panel = Some((channel, message.id.get()));
    ctx.save_settings(guild, KEY, &next).map_err(|e| e.to_string())?;
    Ok(channel)
}

// ---- commands, buttons, modals ----

pub async fn on_command(ctx: &Ctx, call: &CommandCall) -> bot_core::anyhow::Result<()> {
    let Some(guild) = call.guild_id() else { return Ok(()) };
    let Some((sub, opts)) = call.opts().sub() else { return Ok(()) };
    match sub {
        "panel" => {
            if !has_perm(&call.interaction, Permissions::MANAGE_GUILD) {
                return ctx.reply(&call.interaction, Reply::text("You need the Manage Server permission to post the panel.").ephemeral()).await;
            }
            ctx.defer(&call.interaction, true).await?;
            let msg = match send_panel(ctx, guild).await {
                Ok(channel) => format!("Panel posted in {}.", channel_label(ctx, guild, channel)),
                Err(e) => e,
            };
            ctx.edit_original(&call.interaction, Reply::text(msg)).await?;
        }
        "close" => {
            let s = ctx.settings::<Settings>(guild, KEY)?;
            let ticket = match call.channel_id() {
                Some(thread) => ctx.db.with(|c| by_thread(c, guild, thread))?,
                None => None,
            };
            let Some(ticket) = ticket else {
                return ctx.reply(&call.interaction, Reply::text("This isn't a ticket thread.").ephemeral()).await;
            };
            if !can_close(&call.interaction, &s, &ticket) {
                return ctx.reply(&call.interaction, Reply::text("Only the person who opened this ticket or staff can close it.").ephemeral()).await;
            }
            close_from_interaction(ctx, &call.interaction, guild, &ticket, opts.str("reason")).await?;
        }
        _ => {}
    }
    Ok(())
}

pub async fn on_component(ctx: &Ctx, call: &ComponentCall) -> bot_core::anyhow::Result<()> {
    let Some(guild) = call.guild_id() else { return Ok(()) };
    let s = ctx.settings::<Settings>(guild, KEY)?;
    match call.args().as_slice() {
        ["open"] => {
            if s.ticket_channel.is_none() {
                return ctx.reply(&call.interaction, Reply::text("Tickets aren't set up on this server yet.").ephemeral()).await;
            }
            if let Some(thread) = ctx.db.with(|c| full_thread(c, guild, call.user_id(), s.max_open))? {
                return ctx.reply(&call.interaction, Reply::text(already_open(thread)).ephemeral()).await;
            }
            ctx.modal(
                &call.interaction,
                &format!("{KEY}:new"),
                "Open a ticket",
                &[("subject", "Subject", false, true, None), ("details", "Details", true, true, None)],
            )
            .await?;
        }
        ["close", number] => {
            let Ok(number) = number.parse::<i64>() else { return Ok(()) };
            let ticket = ctx.db.with(|c| by_number(c, guild, number))?;
            let Some(ticket) = ticket.filter(Ticket::is_open) else {
                return ctx.reply(&call.interaction, Reply::text("This ticket is already closed.").ephemeral()).await;
            };
            if !can_close(&call.interaction, &s, &ticket) {
                return ctx.reply(&call.interaction, Reply::text("Only the person who opened this ticket or staff can close it.").ephemeral()).await;
            }
            ctx.modal(
                &call.interaction,
                &format!("{KEY}:closing:{number}"),
                "Close this ticket",
                &[("reason", "Reason (optional)", true, false, None)],
            )
            .await?;
        }
        _ => {}
    }
    Ok(())
}

pub async fn on_modal(ctx: &Ctx, call: &ModalCall) -> bot_core::anyhow::Result<()> {
    let Some(guild) = call.guild_id() else { return Ok(()) };
    let s = ctx.settings::<Settings>(guild, KEY)?;
    match call.args().as_slice() {
        ["new"] => open_ticket(ctx, call, guild, &s).await,
        ["closing", number] => {
            let Ok(number) = number.parse::<i64>() else { return Ok(()) };
            let ticket = ctx.db.with(|c| by_number(c, guild, number))?;
            let Some(ticket) = ticket.filter(Ticket::is_open) else {
                return ctx.reply(&call.interaction, Reply::text("This ticket is already closed.").ephemeral()).await;
            };
            if !can_close(&call.interaction, &s, &ticket) {
                return ctx.reply(&call.interaction, Reply::text("Only the person who opened this ticket or staff can close it.").ephemeral()).await;
            }
            let reason = call.value("reason").map(|r| r.trim().to_owned()).filter(|r| !r.is_empty());
            close_from_interaction(ctx, &call.interaction, guild, &ticket, reason.as_deref()).await
        }
        _ => Ok(()),
    }
}

async fn open_ticket(ctx: &Ctx, call: &ModalCall, guild: u64, s: &Settings) -> bot_core::anyhow::Result<()> {
    let Some(channel) = s.ticket_channel else {
        return ctx.reply(&call.interaction, Reply::text("Tickets aren't set up on this server yet.").ephemeral()).await;
    };
    let Some((user, name, avatar)) = user_of(&call.interaction) else { return Ok(()) };
    ctx.defer(&call.interaction, true).await?;
    let subject = text::clip(call.value("subject").unwrap_or_default().trim(), 200);
    let details = text::clip(call.value("details").unwrap_or_default().trim(), 2000);
    let subject = if subject.is_empty() { "No subject".to_owned() } else { subject };

    let reserved = {
        let mut conn = ctx.db.lock();
        reserve(&mut conn, guild, user, &subject, &details, s.max_open, now_ms())?
    };
    let number = match reserved {
        Reserved::Number(n) => n,
        Reserved::Full(thread) => return ctx.edit_original(&call.interaction, Reply::text(already_open(thread))).await,
    };

    let thread_name = format!("ticket-{number:04} {}", text::clip(&name, 60));
    let created = ctx
        .http
        .create_thread(Id::new(channel), &thread_name, ChannelType::PrivateThread)
        .invitable(false)
        .auto_archive_duration(AutoArchiveDuration::Week)
        .await;
    let thread = match created {
        Ok(r) => r.model().await?,
        Err(e) => {
            bot_core::tracing::warn!(guild, channel, error = %e, "couldn't create a ticket thread");
            ctx.db.with(|c| release(c, guild, number))?;
            let msg = "Couldn't open the ticket. The bot needs permission to create private threads in the ticket channel. Tell the staff.";
            return ctx.edit_original(&call.interaction, Reply::text(msg)).await;
        }
    };
    ctx.db.with(|c| set_thread(c, guild, number, thread.id.get()))?;
    if let Err(e) = ctx.http.add_thread_member(thread.id, Id::new(user)).await {
        bot_core::tracing::warn!(guild, user, error = %e, "couldn't add the member to their ticket");
    }

    let mut embed = EmbedBuilder::new().title(&subject).color(COLOR).footer(EmbedFooterBuilder::new(format!("Ticket {number:04}")));
    if !details.is_empty() {
        embed = embed.description(&details);
    }
    let mut author = EmbedAuthorBuilder::new(&name);
    if let Ok(icon) = ImageSource::url(avatar) {
        author = author.icon_url(icon);
    }
    let embed = embed.author(author).build();
    let close = button_row(&[(&format!("{KEY}:close:{number}"), "Close", ButtonStyle::Secondary)]);
    let (content, mentions) = match s.staff_role {
        Some(role) => (
            format!("<@&{role}>, <@{user}> opened this ticket."),
            AllowedMentions { roles: vec![Id::new(role)], users: vec![Id::new(user)], ..Default::default() },
        ),
        None => (format!("<@{user}>, your ticket is open."), AllowedMentions { users: vec![Id::new(user)], ..Default::default() }),
    };
    if let Err(e) = ctx.http.create_message(thread.id).content(&content).embeds(&[embed]).components(&[close]).allowed_mentions(Some(&mentions)).await {
        bot_core::tracing::warn!(guild, thread = thread.id.get(), error = %e, "couldn't post in a new ticket");
    }

    ctx.count(guild, "tickets_opened", 1);
    ctx.notify(Some(guild), KEY, format!("New ticket {number:04} from {name}: {}", text::clip(&subject, 80)), Some(format!("/g/{guild}/tickets")));
    ctx.edit_original(&call.interaction, Reply::text(format!("Your ticket is open: <#{}>", thread.id.get()))).await
}

fn closing_text(by: u64, reason: Option<&str>) -> String {
    match reason {
        Some(r) => format!("Closed by <@{by}>. Reason: {r}"),
        None => format!("Closed by <@{by}>."),
    }
}

/// Close from a button, modal or command: say so in the thread, then archive and lock it.
async fn close_from_interaction(ctx: &Ctx, i: &Interaction, guild: u64, t: &Ticket, reason: Option<&str>) -> bot_core::anyhow::Result<()> {
    let Some(by) = i.author_id().map(|u| u.get()) else { return Ok(()) };
    let reason = reason.map(|r| text::clip(r, 300));
    if !ctx.db.with(|c| mark_closed(c, guild, t.number, Some(by), reason.as_deref(), CLOSED, now_ms()))? {
        return ctx.reply(i, Reply::text("This ticket is already closed.").ephemeral()).await;
    }
    ctx.reply(i, Reply::text(closing_text(by, reason.as_deref()))).await?;
    archive(ctx, t.thread_id).await;
    Ok(())
}

/// The dashboard's Close button. False when the ticket wasn't open.
pub async fn close_from_dashboard(ctx: &Ctx, guild: u64, number: i64, by: u64) -> bot_core::anyhow::Result<bool> {
    let Some(t) = ctx.db.with(|c| by_number(c, guild, number))? else { return Ok(false) };
    if !ctx.db.with(|c| mark_closed(c, guild, number, Some(by), None, CLOSED, now_ms()))? {
        return Ok(false);
    }
    if t.thread_id != 0 {
        let _ = ctx.say(t.thread_id, &closing_text(by, None)).await;
        archive(ctx, t.thread_id).await;
    }
    Ok(true)
}

/// The dashboard's Reopen button: unarchive the thread, then count the ticket as open again.
pub async fn reopen_from_dashboard(ctx: &Ctx, guild: u64, number: i64, by: u64) -> Result<(), String> {
    let t = ctx.db.with(|c| by_number(c, guild, number)).map_err(|e| e.to_string())?.ok_or("That ticket doesn't exist.")?;
    if t.is_open() {
        return Ok(());
    }
    if !t.can_reopen(now_ms()) {
        return Err(format!("Only tickets closed in the last {REOPEN_DAYS} days can be reopened."));
    }
    // Discord first: if the thread can't come back, the ticket stays closed.
    if let Err(e) = ctx.http.update_thread(Id::new(t.thread_id)).archived(false).locked(false).await {
        bot_core::tracing::warn!(guild, thread = t.thread_id, error = %e, "couldn't reopen a ticket thread");
        return Err(if is_not_found(&e) {
            "The ticket's thread was deleted, so it can't be reopened.".into()
        } else {
            "Couldn't unarchive the thread. The bot needs the Manage Threads permission in the ticket channel.".into()
        });
    }
    ctx.db.with(|c| mark_reopened(c, guild, number, now_ms())).map_err(|e| e.to_string())?;
    let _ = ctx.say(t.thread_id, &format!("Reopened by <@{by}>.")).await;
    Ok(())
}

async fn archive(ctx: &Ctx, thread: u64) {
    if let Err(e) = ctx.http.update_thread(Id::new(thread)).archived(true).locked(true).await {
        bot_core::tracing::warn!(thread, error = %e, "couldn't archive a closed ticket");
    }
}

// ---- idle tickets ----

pub async fn sweep_idle(feature: &Tickets, ctx: &Ctx) -> bot_core::anyhow::Result<()> {
    for guild in ctx.enabled_guilds(KEY) {
        let s = ctx.settings::<Settings>(guild, KEY)?;
        if s.auto_close_days <= 0 || !feature.sweep_due(guild) {
            continue;
        }
        let open: Vec<Ticket> = ctx.db.with(|c| list(c, guild, Filter::Open, 500))?.into_iter().filter(|t| t.thread_id != 0).collect();
        let limit = s.auto_close_days * DAY_MS;
        for t in open {
            let channel = match ctx.http.channel(Id::new(t.thread_id)).await {
                Ok(r) => match r.model().await {
                    Ok(c) => c,
                    Err(_) => continue,
                },
                Err(e) if is_not_found(&e) => {
                    on_thread_gone(ctx, guild, t.thread_id, "The thread was deleted")?;
                    continue;
                }
                Err(_) => continue,
            };
            let last = channel.last_message_id.map(|id| snowflake_ms(id.get())).unwrap_or(t.opened_at).max(t.opened_at);
            if now_ms() - last < limit {
                continue;
            }
            let why = format!("No messages for {} days", s.auto_close_days);
            if ctx.db.with(|c| mark_closed(c, guild, t.number, None, Some(&why), IDLE, now_ms()))? {
                let _ = ctx.say(t.thread_id, &format!("Closed: no messages for {} days.", s.auto_close_days)).await;
                archive(ctx, t.thread_id).await;
            }
        }
    }
    Ok(())
}

// ---- preview ----

pub fn seed(ctx: &Ctx, guild: u64) -> bot_core::anyhow::Result<()> {
    let now = now_ms();
    let (m, h) = (60_000, 3_600_000);
    // (opened ago, status, closed after, closed by member index, reason, subject)
    // What each ticket's opener wrote, in the same order as the rows.
    let details = [
        "I got the Idleon role this morning but #pets still doesn't show up.",
        "I hit level 20 and the Regulars role didn't come through.",
        "Someone keeps posting the same link in #general and #trading, screenshots attached.",
        "My colour role went grey after the server renamed the role.",
        "I was banned for two days last month. I'd like to talk about coming back.",
        "Where do I find the link button for Twitch?",
        "I won the Frog pet last Friday and haven't been sent it yet.",
        "Which pets count toward the collection?",
        "The bot didn't post alerts during the stream last night.",
    ];
    let rows: &[(i64, &str, i64, usize, &str, &str)] = &[
        (20 * m, OPEN, 0, 0, "", "Can't see the pets channel"),
        (3 * h, OPEN, 0, 0, "", "Role didn't update after my level-up"),
        (2 * DAY_MS + 5 * h, OPEN, 0, 0, "", "Report: spam in #general"),
        (DAY_MS + 3 * h, CLOSED, 40 * m, 1, "Fixed, roles resynced", "Wrong colour role after rename"),
        (3 * DAY_MS, CLOSED, 6 * h, 1, "", "Ban appeal"),
        (4 * DAY_MS, CLOSED, 55 * m, 3, "Answered in the thread", "How do I link my Twitch account?"),
        (6 * DAY_MS, CLOSED, 2 * h + 10 * m, 1, "", "Giveaway prize not received"),
        (9 * DAY_MS, IDLE, 7 * DAY_MS, 0, "No messages for 7 days", "Question about the idleon pets"),
        (12 * DAY_MS, CLOSED, 3 * h, 3, "Duplicate of an older ticket", "Bot offline during the stream"),
    ];
    let people = [2usize, 4, 5, 6, 7, 8, 9, 3];
    // For the open ones: (minutes since the last message, whether staff wrote it, whether staff has answered at all).
    let turns: [(i64, bool, bool); 3] = [(18, false, false), (50, true, true), (26 * 60, false, true)];
    for (i, (ago, status, took, by, reason, subject)) in rows.iter().enumerate() {
        let number = i as i64 + 1;
        let opener = bot_core::preview::MEMBERS[people[i % people.len()]].0;
        let closer = bot_core::preview::MEMBERS[*by].0;
        let opened = now - ago;
        let closed = (*status != OPEN).then_some(opened + took);
        let (last_author, last_at, first_reply) = match turns.get(i).filter(|_| *status == OPEN) {
            Some((mins, staff_last, answered)) => {
                let staff = bot_core::preview::MEMBERS[1].0;
                let at = now - mins * m;
                (Some(if *staff_last { staff } else { opener }), Some(at), answered.then_some(opened + (now - opened - mins * m) / 2))
            }
            None => (None, None, None),
        };
        ctx.db.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO tickets (guild_id, number, opener_id, subject, details, status, thread_id, opened_at, closed_at, closed_by, close_reason,
                   last_author_id, last_message_at, first_reply_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    guild as i64,
                    number,
                    opener as i64,
                    subject,
                    details[i.min(details.len() - 1)],
                    status,
                    (guild + 5000 + number as u64) as i64,
                    opened,
                    closed,
                    (*status == CLOSED).then_some(closer as i64),
                    (!reason.is_empty()).then_some(*reason),
                    last_author.map(|u| u as i64),
                    last_at,
                    first_reply
                ],
            )
        })?;
    }
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
    fn median_of_lists() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[5]), Some(5));
        assert_eq!(median(&[1, 9, 100]), Some(9));
        assert_eq!(median(&[2, 4, 10, 20]), Some(7));
    }

    #[test]
    fn snowflake_time() {
        // 2016-04-30 11:18:25.796 UTC, from Discord's docs.
        assert_eq!(snowflake_ms(175_928_847_299_117_063), 1_462_015_105_796);
    }

    #[test]
    fn numbers_count_up_per_server_and_cap_open_tickets() {
        let db = db();
        let mut c = db.lock();
        let a = reserve(&mut c, 1, 10, "a", "", 1, 1000).unwrap();
        assert!(matches!(a, Reserved::Number(1)));
        set_thread(&c, 1, 1, 555).unwrap();
        // Same member, limit 1: told where their ticket is.
        assert!(matches!(reserve(&mut c, 1, 10, "b", "", 1, 2000).unwrap(), Reserved::Full(555)));
        // A higher limit lets them open another; another server starts again at 1.
        assert!(matches!(reserve(&mut c, 1, 10, "b", "", 2, 2000).unwrap(), Reserved::Number(2)));
        assert!(matches!(reserve(&mut c, 2, 10, "c", "", 1, 2000).unwrap(), Reserved::Number(1)));
    }

    #[test]
    fn closing_frees_the_member_and_only_works_once() {
        let db = db();
        let mut c = db.lock();
        reserve(&mut c, 1, 10, "a", "", 1, 1000).unwrap();
        set_thread(&c, 1, 1, 555).unwrap();
        assert!(mark_closed(&c, 1, 1, Some(99), Some("done"), CLOSED, 5000).unwrap());
        assert!(!mark_closed(&c, 1, 1, Some(99), None, CLOSED, 6000).unwrap());
        let t = by_thread(&c, 1, 555).unwrap().unwrap();
        assert_eq!((t.status.as_str(), t.closed_by, t.closed_at), (CLOSED, Some(99), Some(5000)));
        assert!(matches!(reserve(&mut c, 1, 10, "again", "", 1, 7000).unwrap(), Reserved::Number(2)));
    }

    #[test]
    fn numbers_are_never_reused_once_a_ticket_has_a_thread() {
        let db = db();
        let mut c = db.lock();
        for (user, thread) in [(10u64, 501u64), (11, 502), (12, 503)] {
            let Reserved::Number(n) = reserve(&mut c, 1, user, "a", "", 1, 1000).unwrap() else { panic!("full") };
            set_thread(&c, 1, n, thread).unwrap();
        }
        // Closing, or releasing a ticket that already has its thread, frees nothing.
        mark_closed(&c, 1, 3, None, None, IDLE, 2000).unwrap();
        release(&c, 1, 3).unwrap();
        assert!(by_number(&c, 1, 3).unwrap().is_some());
        assert!(matches!(reserve(&mut c, 1, 13, "b", "", 1, 3000).unwrap(), Reserved::Number(4)));
        assert!(by_number(&c, 1, 99).unwrap().is_none());
    }

    #[test]
    fn pressing_twice_finds_the_ticket_still_being_opened() {
        let db = db();
        let mut c = db.lock();
        assert!(matches!(reserve(&mut c, 1, 10, "a", "", 1, 1000).unwrap(), Reserved::Number(1)));
        // No thread yet: the second press is told to wait, not sent to <#0>.
        assert!(matches!(reserve(&mut c, 1, 10, "a", "", 1, 1000).unwrap(), Reserved::Full(0)));
        assert_eq!(already_open(0), "Your ticket is still being opened. It shows up in a moment.");
        assert!(already_open(555).contains("<#555>"));
    }

    #[test]
    fn a_failed_thread_gives_the_number_back() {
        let db = db();
        let mut c = db.lock();
        reserve(&mut c, 1, 10, "a", "", 1, 1000).unwrap();
        release(&c, 1, 1).unwrap();
        assert!(matches!(reserve(&mut c, 1, 10, "a", "", 1, 1000).unwrap(), Reserved::Number(1)));
    }

    #[test]
    fn stats_use_recent_closed_tickets_only() {
        let db = db();
        let mut c = db.lock();
        let now = 100 * DAY_MS;
        for (i, took) in [60_000i64, 3_600_000, 7_200_000].iter().enumerate() {
            reserve(&mut c, 1, 10 + i as u64, "x", "", 1, now - DAY_MS).unwrap();
            let n = i as i64 + 1;
            set_thread(&c, 1, n, 100 + n as u64).unwrap();
            mark_closed(&c, 1, n, None, None, CLOSED, now - DAY_MS + took).unwrap();
        }
        reserve(&mut c, 1, 50, "open", "", 1, now - 10 * DAY_MS).unwrap();
        let s = stats(&c, 1, now).unwrap();
        assert_eq!((s.open, s.this_week), (1, 3));
        assert_eq!(s.median_close_secs, Some(3600));
    }

    #[test]
    fn replies_set_whose_turn_it_is() {
        let db = db();
        let mut c = db.lock();
        reserve(&mut c, 1, 10, "a", "", 1, 1000).unwrap();
        set_thread(&c, 1, 1, 555).unwrap();
        let turn = |c: &Connection| by_number(c, 1, 1).unwrap().unwrap().waiting_on();
        // Opening counts as the member's message.
        assert_eq!(turn(&c), Some(Turn::NoReplyYet));
        record_message(&c, 1, 555, 10, 2000).unwrap();
        assert_eq!(turn(&c), Some(Turn::NoReplyYet));
        record_message(&c, 1, 555, 99, 3000).unwrap();
        assert_eq!(turn(&c), Some(Turn::Member));
        record_message(&c, 1, 555, 10, 4000).unwrap();
        assert_eq!(turn(&c), Some(Turn::Staff));
        let t = by_number(&c, 1, 1).unwrap().unwrap();
        assert_eq!((t.first_reply_at, t.last_message_at), (Some(3000), Some(4000)));
        assert_eq!(stats(&c, 1, 5000).unwrap().waiting, 1);
        // A ticket from before the last author was kept counts as waiting, as on the overview.
        c.execute("UPDATE tickets SET last_author_id = NULL WHERE number = 1", []).unwrap();
        assert_eq!(stats(&c, 1, 5000).unwrap().waiting, 1);
        // Another thread, or a closed ticket, isn't touched.
        assert!(!record_message(&c, 1, 556, 10, 5000).unwrap());
        mark_closed(&c, 1, 1, Some(99), None, CLOSED, 6000).unwrap();
        assert!(!record_message(&c, 1, 555, 10, 7000).unwrap());
    }

    #[test]
    fn waiting_on_staff_skips_tickets_staff_answered_last() {
        let db = db();
        let mut c = db.lock();
        for (user, thread) in [(10u64, 555u64), (11, 556), (12, 557)] {
            reserve(&mut c, 1, user, "a", "", 1, 1000).unwrap();
            let n = number_opened_by(&c, user);
            set_thread(&c, 1, n, thread).unwrap();
        }
        record_message(&c, 1, 556, 99, 2000).unwrap(); // staff answered 11
        record_message(&c, 1, 557, 12, 9000).unwrap(); // 12 wrote recently
        let numbers: Vec<i64> = waiting_on_staff(&c, 1, 5000, 10).unwrap().iter().map(|t| t.number).collect();
        assert_eq!(numbers, vec![1]);
    }

    fn number_opened_by(c: &Connection, user: u64) -> i64 {
        c.query_row("SELECT number FROM tickets WHERE opener_id = ?1", [user as i64], |r| r.get(0)).unwrap()
    }

    #[test]
    fn closing_your_own_ticket_is_not_answering() {
        let db = db();
        let mut c = db.lock();
        reserve(&mut c, 1, 10, "a", "", 1, 1000).unwrap();
        reserve(&mut c, 1, 11, "b", "", 1, 1000).unwrap();
        mark_closed(&c, 1, 1, Some(10), None, CLOSED, 2000).unwrap();
        mark_closed(&c, 1, 2, Some(99), None, CLOSED, 2000).unwrap();
        assert_eq!(closers(&c, 1, 0, 5).unwrap(), vec![(99, 1)]);
    }

    #[test]
    fn reopening_is_for_recent_closes_with_a_thread() {
        let db = db();
        let mut c = db.lock();
        reserve(&mut c, 1, 10, "a", "", 1, 1000).unwrap();
        set_thread(&c, 1, 1, 555).unwrap();
        mark_closed(&c, 1, 1, Some(99), Some("done"), CLOSED, 2000).unwrap();
        let t = by_number(&c, 1, 1).unwrap().unwrap();
        assert!(t.can_reopen(2000 + REOPEN_DAYS * DAY_MS));
        assert!(!t.can_reopen(2001 + REOPEN_DAYS * DAY_MS));
        assert!(mark_reopened(&c, 1, 1, 9 * DAY_MS).unwrap());
        assert!(!mark_reopened(&c, 1, 1, 9 * DAY_MS).unwrap());
        let t = by_number(&c, 1, 1).unwrap().unwrap();
        assert_eq!((t.status.as_str(), t.closed_at, t.closed_by, t.reason.as_deref()), (OPEN, None, None, None));
        // The days it was closed aren't counted as waiting.
        assert_eq!(t.last_message_at, Some(9 * DAY_MS));
        assert!(waiting_on_staff(&c, 1, 9 * DAY_MS, 10).unwrap().is_empty());
        assert!(!t.can_reopen(3000));
        // The member is at their limit again.
        assert!(matches!(reserve(&mut c, 1, 10, "b", "", 1, 4000).unwrap(), Reserved::Full(555)));
    }

    #[test]
    fn a_vanished_thread_closes_its_ticket() {
        let db = db();
        let mut c = db.lock();
        reserve(&mut c, 1, 10, "a", "", 1, 1000).unwrap();
        set_thread(&c, 1, 1, 555).unwrap();
        assert!(thread_gone(&c, 555, "The thread was deleted", 2000).unwrap());
        assert!(!thread_gone(&c, 555, "again", 3000).unwrap());
        let t = by_number(&c, 1, 1).unwrap().unwrap();
        assert_eq!((t.status.as_str(), t.closed_by, t.closed_at), (CLOSED, None, Some(2000)));
    }
}
