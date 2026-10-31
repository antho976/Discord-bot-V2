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
    pub status: String,
    pub thread_id: u64,
    pub opened_at: i64,
    pub closed_at: Option<i64>,
    pub closed_by: Option<u64>,
    pub reason: Option<String>,
}

impl Ticket {
    pub fn is_open(&self) -> bool {
        self.status == OPEN
    }
}

const COLUMNS: &str = "number, opener_id, subject, status, thread_id, opened_at, closed_at, closed_by, close_reason";

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
        "INSERT INTO tickets (guild_id, number, opener_id, subject, details, status, thread_id, opened_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'open', 0, ?6)",
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

#[derive(Clone, Copy, PartialEq)]
pub enum Filter {
    All,
    Open,
    Closed,
}

pub fn list(conn: &Connection, guild: u64, filter: Filter, limit: i64) -> bot_core::rusqlite::Result<Vec<Ticket>> {
    let clause = match filter {
        Filter::All => "",
        Filter::Open => "AND status = 'open'",
        Filter::Closed => "AND status != 'open'",
    };
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM tickets WHERE guild_id = ?1 {clause} ORDER BY number DESC LIMIT ?2"))?;
    stmt.query_map(params![guild as i64, limit], from_row)?.collect()
}

/// Open tickets opened before `before`, oldest first: the ones nobody has come back to.
pub fn open_before(conn: &Connection, guild: u64, before: i64, limit: i64) -> bot_core::rusqlite::Result<Vec<Ticket>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM tickets WHERE guild_id = ?1 AND status = 'open' AND opened_at < ?2 ORDER BY opened_at LIMIT ?3"))?;
    stmt.query_map(params![guild as i64, before, limit], from_row)?.collect()
}

pub struct Stats {
    pub open: i64,
    pub this_week: i64,
    /// Median seconds from opening to closing, over tickets closed in the last 30 days.
    pub median_close_secs: Option<i64>,
}

pub fn stats(conn: &Connection, guild: u64, now: i64) -> bot_core::rusqlite::Result<Stats> {
    let g = guild as i64;
    let open = conn.query_row("SELECT COUNT(*) FROM tickets WHERE guild_id = ?1 AND status = 'open'", [g], |r| r.get(0))?;
    let this_week = conn.query_row("SELECT COUNT(*) FROM tickets WHERE guild_id = ?1 AND opened_at > ?2", params![g, now - 7 * DAY_MS], |r| r.get(0))?;
    let mut stmt = conn.prepare(
        "SELECT (closed_at - opened_at) / 1000 FROM tickets
         WHERE guild_id = ?1 AND status = 'closed' AND closed_at > ?2 ORDER BY 1",
    )?;
    let secs: Vec<i64> = stmt.query_map(params![g, now - 30 * DAY_MS], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(Stats { open, this_week, median_close_secs: median(&secs) })
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

fn user_of(i: &Interaction) -> (u64, String, String) {
    let user = i.author();
    let id = user.map(|u| u.id.get()).unwrap_or_default();
    let name = i
        .member
        .as_ref()
        .and_then(|m| m.nick.clone())
        .or_else(|| user.and_then(|u| u.global_name.clone()))
        .or_else(|| user.map(|u| u.name.clone()))
        .unwrap_or_else(|| "Member".into());
    let avatar = bot_core::discord::avatar_url(id, user.and_then(|u| u.avatar.map(|a| a.to_string())).as_deref());
    (id, name, avatar)
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
                let msg = format!("You already have an open ticket: <#{thread}>. Close it before opening another.");
                return ctx.reply(&call.interaction, Reply::text(msg).ephemeral()).await;
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
    ctx.defer(&call.interaction, true).await?;
    let (user, name, avatar) = user_of(&call.interaction);
    let subject = text::clip(call.value("subject").unwrap_or_default().trim(), 200);
    let details = text::clip(call.value("details").unwrap_or_default().trim(), 2000);
    let subject = if subject.is_empty() { "No subject".to_owned() } else { subject };

    let reserved = {
        let mut conn = ctx.db.lock();
        reserve(&mut conn, guild, user, &subject, &details, s.max_open, now_ms())?
    };
    let number = match reserved {
        Reserved::Number(n) => n,
        Reserved::Full(thread) => {
            let msg = format!("You already have an open ticket: <#{thread}>. Close it before opening another.");
            return ctx.edit_original(&call.interaction, Reply::text(msg)).await;
        }
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
    let by = i.author_id().map(|u| u.get()).unwrap_or_default();
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
    for (i, (ago, status, took, by, reason, subject)) in rows.iter().enumerate() {
        let number = i as i64 + 1;
        let opener = bot_core::preview::MEMBERS[people[i % people.len()]].0;
        let closer = bot_core::preview::MEMBERS[*by].0;
        let opened = now - ago;
        let closed = (*status != OPEN).then_some(opened + took);
        ctx.db.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO tickets (guild_id, number, opener_id, subject, details, status, thread_id, opened_at, closed_at, closed_by, close_reason)
                 VALUES (?1, ?2, ?3, ?4, '', ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    guild as i64,
                    number,
                    opener as i64,
                    subject,
                    status,
                    (guild + 5000 + number as u64) as i64,
                    opened,
                    closed,
                    (*status == CLOSED).then_some(closer as i64),
                    (!reason.is_empty()).then_some(*reason)
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
