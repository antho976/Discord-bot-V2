//! Suggestions: `/suggest`, the vote and review buttons, and the status changes staff make from
//! Discord or the dashboard.

use bot_core::anyhow::{Result, anyhow};
use bot_core::discord::{CommandCall, ComponentCall, ModalCall, Reply, avatar_url};
use bot_core::rusqlite::{Connection, OptionalExtension, Row, params};
use bot_core::twilight_model::application::command::{Command, CommandType};
use bot_core::twilight_model::channel::message::component::{ButtonStyle, Component};
use bot_core::twilight_model::channel::message::Embed;
use bot_core::twilight_model::id::Id;
use bot_core::twilight_util::builder::command::{CommandBuilder, StringBuilder};
use bot_core::twilight_util::builder::embed::{EmbedAuthorBuilder, EmbedBuilder, EmbedFieldBuilder, EmbedFooterBuilder, ImageSource};
use bot_core::{Ctx, now_ms, text, tracing};

use crate::{CORAL, KEY, NEUTRAL, Settings, button, id_i64, is_staff, message_url, row};

pub const MAX_TEXT: usize = 1500;
pub const MAX_REASON: usize = 800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Open,
    Considering,
    Approved,
    Denied,
    Implemented,
}

impl Status {
    pub const ALL: [Status; 5] = [Status::Open, Status::Considering, Status::Approved, Status::Denied, Status::Implemented];

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::Considering => "considering",
            Status::Approved => "approved",
            Status::Denied => "denied",
            Status::Implemented => "implemented",
        }
    }

    pub fn parse(s: &str) -> Option<Status> {
        Status::ALL.into_iter().find(|st| st.as_str() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Open => "Open",
            Status::Considering => "Considering",
            Status::Approved => "Approved",
            Status::Denied => "Denied",
            Status::Implemented => "Implemented",
        }
    }

    pub(crate) fn color(self) -> u32 {
        match self {
            Status::Open => NEUTRAL,
            Status::Considering => 0xe3a948,
            Status::Approved => 0x4fc38a,
            Status::Denied => CORAL,
            Status::Implemented => 0x3f9d77,
        }
    }

    /// Decided suggestions stop taking votes.
    pub fn is_decided(self) -> bool {
        matches!(self, Status::Approved | Status::Denied | Status::Implemented)
    }
}

#[derive(Debug, Clone)]
pub struct Suggestion {
    pub id: i64,
    pub guild_id: u64,
    pub channel_id: u64,
    pub message_id: Option<u64>,
    pub author_id: u64,
    pub content: String,
    pub anonymous: bool,
    pub status: Status,
    pub reason: String,
    pub staff_id: Option<u64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    pub up: i64,
    pub down: i64,
}

const COLS: &str = "id, guild_id, channel_id, message_id, author_id, content, anonymous, status, reason, staff_id, created_at";

fn from_row(r: &Row) -> bot_core::rusqlite::Result<Suggestion> {
    let status: String = r.get(7)?;
    Ok(Suggestion {
        id: r.get(0)?,
        guild_id: r.get::<_, i64>(1)? as u64,
        channel_id: r.get::<_, i64>(2)? as u64,
        message_id: r.get::<_, Option<i64>>(3)?.map(|m| m as u64),
        author_id: r.get::<_, i64>(4)? as u64,
        content: r.get(5)?,
        anonymous: r.get::<_, i64>(6)? != 0,
        status: Status::parse(&status).unwrap_or(Status::Open),
        reason: r.get(8)?,
        staff_id: r.get::<_, Option<i64>>(9)?.map(|m| m as u64),
        created_at: r.get(10)?,
    })
}

pub fn get(c: &Connection, id: i64) -> bot_core::rusqlite::Result<Option<Suggestion>> {
    c.query_row(&format!("SELECT {COLS} FROM suggestions WHERE id = ?1"), [id], from_row).optional()
}

pub fn tally(c: &Connection, id: i64) -> bot_core::rusqlite::Result<Tally> {
    c.query_row(
        "SELECT COALESCE(SUM(vote = 1), 0), COALESCE(SUM(vote = -1), 0) FROM suggestion_votes WHERE suggestion_id = ?1",
        [id],
        |r| Ok(Tally { up: r.get(0)?, down: r.get(1)? }),
    )
}

/// Newest first, with vote counts.
pub fn list(c: &Connection, guild: u64, limit: i64) -> bot_core::rusqlite::Result<Vec<(Suggestion, Tally)>> {
    let mut s = c.prepare(&format!(
        "SELECT {COLS},
           (SELECT COALESCE(SUM(vote = 1), 0) FROM suggestion_votes v WHERE v.suggestion_id = suggestions.id),
           (SELECT COALESCE(SUM(vote = -1), 0) FROM suggestion_votes v WHERE v.suggestion_id = suggestions.id)
         FROM suggestions WHERE guild_id = ?1 ORDER BY created_at DESC, id DESC LIMIT ?2"
    ))?;
    s.query_map(params![id_i64(guild), limit], |r| Ok((from_row(r)?, Tally { up: r.get(11)?, down: r.get(12)? })))?.collect()
}

/// Suggestions waiting for staff.
pub fn open_count(c: &Connection, guild: u64) -> bot_core::rusqlite::Result<i64> {
    c.query_row("SELECT COUNT(*) FROM suggestions WHERE guild_id = ?1 AND status = 'open'", [id_i64(guild)], |r| r.get(0))
}

/// Since `since` (epoch ms): suggestions made, approved and marked implemented.
pub fn month_stats(c: &Connection, guild: u64, since: i64) -> bot_core::rusqlite::Result<(i64, i64, i64)> {
    c.query_row(
        "SELECT COALESCE(SUM(created_at >= ?2), 0),
                COALESCE(SUM(status = 'approved' AND status_at >= ?2), 0),
                COALESCE(SUM(status = 'implemented' AND status_at >= ?2), 0)
         FROM suggestions WHERE guild_id = ?1",
        params![id_i64(guild), since],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
}

/// Members who have cast the most votes in a server since `since` (epoch ms), most first, with
/// their vote counts. A vote counts from when it was last cast or switched; votes from before
/// they were dated have no time and are left out.
pub fn top_voters(c: &Connection, guild: u64, since: i64, limit: i64) -> bot_core::rusqlite::Result<Vec<(u64, i64)>> {
    let mut s = c.prepare_cached(
        "SELECT v.user_id, COUNT(*) AS n FROM suggestion_votes v JOIN suggestions s ON s.id = v.suggestion_id
         WHERE s.guild_id = ?1 AND v.at >= ?2 AND v.at > 0 GROUP BY v.user_id ORDER BY n DESC, v.user_id LIMIT ?3",
    )?;
    s.query_map(params![id_i64(guild), since, limit], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?)))?.collect()
}

/// What a vote press does: the same button again takes the vote back, the other one switches.
pub fn next_vote(existing: Option<i64>, pressed: i64) -> Option<i64> {
    if existing == Some(pressed) { None } else { Some(pressed) }
}

pub fn cast_vote(c: &Connection, id: i64, user: u64, pressed: i64) -> bot_core::rusqlite::Result<Tally> {
    let existing: Option<i64> =
        c.query_row("SELECT vote FROM suggestion_votes WHERE suggestion_id = ?1 AND user_id = ?2", params![id, id_i64(user)], |r| r.get(0)).optional()?;
    match next_vote(existing, pressed) {
        Some(v) => c.execute(
            "INSERT OR REPLACE INTO suggestion_votes (suggestion_id, user_id, vote, at) VALUES (?1, ?2, ?3, ?4)",
            params![id, id_i64(user), v, now_ms()],
        )?,
        None => c.execute("DELETE FROM suggestion_votes WHERE suggestion_id = ?1 AND user_id = ?2", params![id, id_i64(user)])?,
    };
    tally(c, id)
}

pub fn embed(ctx: &Ctx, s: &Suggestion) -> Embed {
    let mut b = EmbedBuilder::new()
        .color(s.status.color())
        .description(&s.content)
        .footer(EmbedFooterBuilder::new(format!("Suggestion #{}", s.id)));
    if !s.anonymous {
        let name = ctx.display_name(s.guild_id, s.author_id);
        let avatar = ctx.cache.user(Id::new(s.author_id)).and_then(|u| u.avatar.map(|a| a.to_string()));
        let mut author = EmbedAuthorBuilder::new(name);
        if let Ok(icon) = ImageSource::url(avatar_url(s.author_id, avatar.as_deref())) {
            author = author.icon_url(icon);
        }
        b = b.author(author);
    }
    if s.status != Status::Open {
        let by = s.staff_id.map(|u| format!(" by {}", ctx.display_name(s.guild_id, u))).unwrap_or_default();
        let reason = if s.reason.is_empty() { "No reason given.".to_owned() } else { s.reason.clone() };
        b = b.field(EmbedFieldBuilder::new(format!("{}{by}", s.status.label()), reason));
    }
    b.build()
}

pub fn components(s: &Suggestion, t: Tally) -> Vec<Component> {
    let closed = s.status.is_decided();
    vec![row(vec![
        button(&format!("{KEY}:sg:up:{}", s.id), &format!("Upvote · {}", t.up), ButtonStyle::Secondary, closed),
        button(&format!("{KEY}:sg:down:{}", s.id), &format!("Downvote · {}", t.down), ButtonStyle::Secondary, closed),
        button(&format!("{KEY}:sg:review:{}", s.id), "Review", ButtonStyle::Secondary, false),
    ])]
}

pub fn command() -> Command {
    CommandBuilder::new("suggest", "Suggest something for the server", CommandType::ChatInput)
        .option(StringBuilder::new("text", "Your suggestion").required(true).max_length(MAX_TEXT as u16))
        .build()
}

pub async fn on_command(ctx: &Ctx, call: &CommandCall) -> Result<()> {
    let Some(guild) = call.guild_id() else { return Ok(()) };
    let settings = ctx.settings::<Settings>(guild, KEY)?;
    let Some(channel) = settings.suggestions_channel else {
        return ctx
            .reply(&call.interaction, Reply::text("Suggestions aren't set up here yet. Ask staff to pick a channel in the dashboard.").ephemeral())
            .await;
    };
    let content = call.opts().str("text").unwrap_or_default().trim();
    if content.is_empty() {
        return ctx.reply(&call.interaction, Reply::text("Write your suggestion after /suggest.").ephemeral()).await;
    }
    ctx.defer(&call.interaction, true).await?;
    let msg = match post(ctx, &settings, guild, channel, call.user_id(), content).await {
        Ok((id, link)) => format!("Suggestion #{id} is posted: {link}"),
        Err(e) => {
            tracing::warn!(guild, error = %e, "couldn't post a suggestion");
            "Couldn't post it. Staff need to check that the bot can send messages in the suggestions channel.".to_owned()
        }
    };
    ctx.edit_original(&call.interaction, Reply::text(msg)).await
}

/// Store a suggestion and post it. Returns its number and a link to the message.
async fn post(ctx: &Ctx, settings: &Settings, guild: u64, channel: u64, author: u64, content: &str) -> Result<(i64, String)> {
    let content = text::clip(content, MAX_TEXT);
    let id = ctx.db.with(|c| {
        c.execute(
            "INSERT INTO suggestions (guild_id, channel_id, author_id, content, anonymous, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id_i64(guild), id_i64(channel), id_i64(author), content, settings.anonymous as i64, now_ms()],
        )?;
        Ok(c.last_insert_rowid())
    })?;
    let s = ctx.db.with(|c| get(c, id))?.ok_or_else(|| anyhow!("suggestion vanished"))?;
    let embeds = [embed(ctx, &s)];
    let sent = ctx.http.create_message(Id::new(channel)).embeds(&embeds).components(&components(&s, Tally::default())).await;
    let message = match sent {
        Ok(r) => match r.model().await {
            Ok(m) => m,
            // Posted, but Discord's reply couldn't be read: the buttons still work (they carry the
            // number), so don't tell the member it failed and invite a second post.
            Err(e) => {
                tracing::warn!(suggestion = id, error = %e, "posted a suggestion but couldn't read the message back");
                return Ok((id, format!("<#{channel}>")));
            }
        },
        Err(e) => {
            ctx.db.with(|c| c.execute("DELETE FROM suggestions WHERE id = ?1", [id]))?;
            return Err(e.into());
        }
    };
    ctx.db.with(|c| c.execute("UPDATE suggestions SET message_id = ?2 WHERE id = ?1", params![id, id_i64(message.id.get())]))?;
    if settings.threads {
        let name = format!("Suggestion #{id}");
        match ctx.http.create_thread_from_message(Id::new(channel), message.id, &name).await {
            Ok(_) => {}
            Err(e) => tracing::warn!(guild, error = %e, "couldn't open a thread for a suggestion"),
        }
    }
    Ok((id, message_url(guild, channel, message.id.get())))
}

pub async fn on_component(ctx: &Ctx, call: &ComponentCall, args: &[&str]) -> Result<()> {
    let Some(guild) = call.guild_id() else { return Ok(()) };
    let Some(id) = args.get(1).and_then(|s| s.parse::<i64>().ok()) else { return Ok(()) };
    let Some(s) = ctx.db.with(|c| get(c, id))?.filter(|s| s.guild_id == guild) else {
        return ctx.reply(&call.interaction, Reply::text("This suggestion doesn't exist any more.").ephemeral()).await;
    };
    match args[0] {
        "up" | "down" => {
            if s.status.is_decided() {
                return ctx.reply(&call.interaction, Reply::text("Voting is closed on this suggestion.").ephemeral()).await;
            }
            let pressed = if args[0] == "up" { 1 } else { -1 };
            let t = ctx.db.with(|c| cast_vote(c, id, call.user_id(), pressed))?;
            ctx.update(&call.interaction, Reply::embed(embed(ctx, &s)).components(components(&s, t))).await
        }
        "review" => {
            let settings = ctx.settings::<Settings>(guild, KEY)?;
            if !is_staff(&call.interaction, &settings) {
                return ctx.reply(&call.interaction, Reply::text("Only staff can review suggestions.").ephemeral()).await;
            }
            // The current decision's button edits its reply instead; "Reopen" on an open one does nothing.
            let pick = |st: Status, label: &str, style: ButtonStyle| {
                let current = s.status == st;
                let label = if current && st != Status::Open { "Edit reply" } else { label };
                button(&format!("{KEY}:sg:set:{id}:{}", st.as_str()), label, style, current && st == Status::Open)
            };
            let rows = vec![row(vec![
                pick(Status::Considering, "Considering", ButtonStyle::Secondary),
                pick(Status::Approved, "Approve", ButtonStyle::Success),
                pick(Status::Denied, "Deny", ButtonStyle::Danger),
                pick(Status::Implemented, "Implemented", ButtonStyle::Secondary),
                pick(Status::Open, "Reopen", ButtonStyle::Secondary),
            ])];
            ctx.reply(&call.interaction, Reply::text(format!("Suggestion #{id} is {}. Set its status:", s.status.label().to_lowercase())).components(rows).ephemeral()).await
        }
        "set" => {
            let settings = ctx.settings::<Settings>(guild, KEY)?;
            if !is_staff(&call.interaction, &settings) {
                return ctx.reply(&call.interaction, Reply::text("Only staff can review suggestions.").ephemeral()).await;
            }
            let Some(status) = args.get(2).and_then(|s| Status::parse(s)) else { return Ok(()) };
            let editing = s.status == status;
            let title = if editing { format!("Edit the reply on #{id}") } else { format!("{} suggestion #{id}", if status == Status::Open { "Reopen" } else { status.label() }) };
            let current = editing.then_some(s.reason.as_str()).filter(|r| !r.is_empty());
            ctx.modal(&call.interaction, &format!("{KEY}:sg:status:{id}:{}", status.as_str()), &title, &[("reason", "Reason (optional)", true, false, current)]).await
        }
        _ => Ok(()),
    }
}

pub async fn on_modal(ctx: &Ctx, call: &ModalCall, args: &[&str]) -> Result<()> {
    let ["status", id, status] = args else { return Ok(()) };
    let (Some(guild), Ok(id), Some(status)) = (call.guild_id(), id.parse::<i64>(), Status::parse(status)) else { return Ok(()) };
    let settings = ctx.settings::<Settings>(guild, KEY)?;
    if !is_staff(&call.interaction, &settings) {
        return ctx.reply(&call.interaction, Reply::text("Only staff can review suggestions.").ephemeral()).await;
    }
    let Some(before) = ctx.db.with(|c| get(c, id))?.filter(|s| s.guild_id == guild) else {
        return ctx.reply(&call.interaction, Reply::text("This suggestion doesn't exist any more.").ephemeral()).await;
    };
    let reason = call.value("reason").unwrap_or_default();
    ctx.defer(&call.interaction, true).await?;
    set_status(ctx, id, status, &reason, call.user_id()).await?;
    let msg = if before.status == status && status != Status::Open {
        format!("Updated the reply on suggestion #{id}.")
    } else {
        format!("Suggestion #{id} is now {}.", status.label().to_lowercase())
    };
    ctx.edit_original(&call.interaction, Reply::text(msg)).await
}

/// Record a status, rewrite the suggestion's message and tell the author. Used by the buttons and
/// the dashboard. Saving the status it already has edits the reply: the message changes, but the
/// decision keeps its date and the author isn't told again.
pub async fn set_status(ctx: &Ctx, id: i64, status: Status, reason: &str, staff: u64) -> Result<Option<Suggestion>> {
    let reason = if status == Status::Open { String::new() } else { text::clip(reason.trim(), MAX_REASON) };
    let Some(before) = ctx.db.with(|c| get(c, id))? else { return Ok(None) };
    let decided = status != before.status;
    ctx.db.with(|c| record_status(c, id, status, &reason, staff, now_ms()))?;
    let Some(s) = ctx.db.with(|c| get(c, id))? else { return Ok(None) };
    if let Some(message) = s.message_id {
        let t = ctx.db.with(|c| tally(c, id))?;
        let embeds = [embed(ctx, &s)];
        let comps = components(&s, t);
        let res = ctx.http.update_message(Id::new(s.channel_id), Id::new(message)).embeds(Some(&embeds)).components(Some(&comps)).await;
        if let Err(e) = res {
            tracing::warn!(suggestion = id, error = %e, "couldn't edit the suggestion message");
        }
    }
    if decided
        && status != Status::Open
        && let Err(e) = ctx.dm(s.author_id, "", &[decision_embed(ctx, &s)], &[]).await
    {
        tracing::warn!(suggestion = id, error = %e, "couldn't DM a suggestion author");
    }
    Ok(Some(s))
}

/// What the author is sent when staff decide on their suggestion.
fn decision_embed(ctx: &Ctx, s: &Suggestion) -> Embed {
    let mut dm = EmbedBuilder::new()
        .color(s.status.color())
        .title(format!("Your suggestion was {}", s.status.label().to_lowercase()))
        .description(text::clip(&s.content, 300))
        .footer(EmbedFooterBuilder::new(ctx.guild_name(s.guild_id)));
    if !s.reason.is_empty() {
        dm = dm.field(EmbedFieldBuilder::new("Reason", &s.reason));
    }
    dm.build()
}

/// A test for the dashboard, in `to`'s DMs: the newest suggestion's post as members see it
/// (with the saved "hide the author" setting, and without its vote buttons, which would count),
/// then the message its author gets when staff approve it. With no suggestion yet, a sample
/// from `to`. False when Discord won't let the bot DM them.
pub async fn send_test(ctx: &Ctx, guild: u64, to: u64, settings: &Settings) -> Result<bool> {
    let newest = ctx.db.with(|c| list(c, guild, 1))?.into_iter().next().map(|(s, _)| s);
    let mut s = newest.unwrap_or_else(|| Suggestion {
        id: 0,
        guild_id: guild,
        channel_id: settings.suggestions_channel.unwrap_or_default(),
        message_id: None,
        author_id: to,
        content: "Test suggestion: this is how one posts.".into(),
        anonymous: false,
        status: Status::Open,
        reason: String::new(),
        staff_id: None,
        created_at: now_ms(),
    });
    s.anonymous = settings.anonymous;
    let post = embed(ctx, &s);
    if !s.status.is_decided() {
        s.status = Status::Approved;
    }
    let decided = decision_embed(ctx, &s);
    let note = "-# Test from the dashboard: a suggestion post as members see it, without its vote buttons, then the message its author gets when staff decide.";
    Ok(ctx.dm(to, note, &[post, decided], &[]).await?.is_some())
}

/// Store a status and reply. A new status is dated `now`; the same status keeps its date.
fn record_status(c: &Connection, id: i64, status: Status, reason: &str, staff: u64, now: i64) -> bot_core::rusqlite::Result<usize> {
    let open = status == Status::Open;
    c.execute(
        "UPDATE suggestions SET reason = ?3, staff_id = ?4,
           status_at = CASE WHEN ?5 THEN 0 WHEN status = ?2 THEN status_at ELSE ?6 END, status = ?2
         WHERE id = ?1",
        params![id, status.as_str(), reason, (!open).then(|| id_i64(staff)), open, now],
    )
}

pub fn seed(ctx: &Ctx, guild: u64) -> Result<()> {
    use bot_core::preview::MEMBERS;
    let who = |i: usize| MEMBERS[i % MEMBERS.len()].0;
    let hour = 3_600_000i64;
    let now = now_ms();
    // (author, text, status, reason, ups, downs)
    let rows: [(usize, &str, Status, &str, usize, usize); 7] = [
        (1, "A voice channel just for body doubling and quiet co-working.", Status::Open, "", 7, 1),
        (3, "Let us post clips in #general without them being auto-deleted.", Status::Open, "", 3, 4),
        (4, "A weekly Idleon pet trading thread, pinned.", Status::Considering, "We'll try it for a month and see if anyone uses it.", 9, 0),
        (2, "Add a role colour for people who have been here a year.", Status::Approved, "Good idea. Setting up the role this week.", 10, 0),
        (5, "Remove the slowmode in #general.", Status::Denied, "It keeps the channel readable during streams.", 2, 8),
        (6, "A /8ball command.", Status::Implemented, "Shipped with the fun commands.", 6, 1),
        (7, "Dark mode for the stream overlay page.", Status::Open, "", 5, 0),
    ];
    for (i, (author, body, status, reason, ups, downs)) in rows.into_iter().enumerate() {
        ctx.db.with(|c| {
            c.execute(
                "INSERT INTO suggestions (guild_id, channel_id, message_id, author_id, content, status, reason, staff_id, created_at, status_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    id_i64(guild),
                    id_i64(guild + 70),
                    1_300_000_000_000_000_200i64 + i as i64,
                    id_i64(who(author)),
                    body,
                    status.as_str(),
                    reason,
                    (status != Status::Open).then(|| id_i64(who(0))),
                    now - (i as i64 + 1) * 9 * hour,
                    if status == Status::Open { 0 } else { now - (i as i64 + 1) * 4 * hour }
                ],
            )?;
            let id = c.last_insert_rowid();
            for n in 0..ups + downs {
                c.execute(
                    "INSERT OR IGNORE INTO suggestion_votes (suggestion_id, user_id, vote, at) VALUES (?1, ?2, ?3, ?4)",
                    params![id, id_i64(who(n)), if n < ups { 1 } else { -1 }, now - ((i + n) as i64 % 9 + 1) * 5 * hour],
                )?;
            }
            Ok(())
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressing_twice_takes_the_vote_back() {
        assert_eq!(next_vote(None, 1), Some(1));
        assert_eq!(next_vote(Some(1), 1), None);
        assert_eq!(next_vote(Some(1), -1), Some(-1));
        assert_eq!(next_vote(Some(-1), 1), Some(1));
    }

    #[test]
    fn one_vote_per_member_and_switching() {
        let db = bot_core::Db::memory().unwrap();
        db.migrate(KEY, crate::MIGRATIONS).unwrap();
        db.with(|c| {
            assert_eq!(cast_vote(c, 1, 10, 1)?, Tally { up: 1, down: 0 });
            assert_eq!(cast_vote(c, 1, 11, 1)?, Tally { up: 2, down: 0 });
            // Switching moves the vote instead of adding one.
            assert_eq!(cast_vote(c, 1, 10, -1)?, Tally { up: 1, down: 1 });
            // Same button again withdraws it.
            assert_eq!(cast_vote(c, 1, 10, -1)?, Tally { up: 1, down: 0 });
            // Other suggestions are separate.
            assert_eq!(tally(c, 2)?, Tally::default());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn editing_a_reply_keeps_the_decision_date() {
        let db = bot_core::Db::memory().unwrap();
        db.migrate(KEY, crate::MIGRATIONS).unwrap();
        db.with(|c| {
            c.execute("INSERT INTO suggestions (guild_id, channel_id, author_id, content) VALUES (1, 2, 3, 'a')", [])?;
            let at = |c: &Connection| c.query_row("SELECT status, reason, status_at FROM suggestions WHERE id = 1", [], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)));
            record_status(c, 1, Status::Approved, "Yes", 9, 100)?;
            assert_eq!(at(c)?, ("approved".into(), "Yes".into(), 100));
            record_status(c, 1, Status::Approved, "Yes, this week", 9, 200)?;
            assert_eq!(at(c)?, ("approved".into(), "Yes, this week".into(), 100));
            record_status(c, 1, Status::Implemented, "Done", 9, 300)?;
            assert_eq!(at(c)?, ("implemented".into(), "Done".into(), 300));
            record_status(c, 1, Status::Open, "", 9, 400)?;
            assert_eq!(at(c)?, ("open".into(), "".into(), 0));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn statuses_round_trip() {
        for st in Status::ALL {
            assert_eq!(Status::parse(st.as_str()), Some(st));
        }
        assert_eq!(Status::parse("nope"), None);
        assert!(Status::Approved.is_decided() && !Status::Considering.is_decided() && !Status::Open.is_decided());
    }

    #[test]
    fn board_totals_read_empty_and_filled_tables() {
        let db = bot_core::Db::memory().unwrap();
        db.migrate(KEY, crate::MIGRATIONS).unwrap();
        db.with(|c| {
            assert_eq!(open_count(c, 1)?, 0);
            assert_eq!(month_stats(c, 1, 0)?, (0, 0, 0));
            assert!(top_voters(c, 1, 0, 3)?.is_empty());
            c.execute("INSERT INTO suggestions (guild_id, channel_id, author_id, content, created_at) VALUES (1, 2, 3, 'a', 10)", [])?;
            c.execute(
                "INSERT INTO suggestions (guild_id, channel_id, author_id, content, status, created_at, status_at) VALUES (1, 2, 3, 'b', 'approved', 10, 20)",
                [],
            )?;
            c.execute("INSERT INTO suggestion_votes (suggestion_id, user_id, vote, at) VALUES (1, 9, 1, 30), (2, 9, -1, 40), (2, 8, 1, 10)", [])?;
            // A vote from before votes were dated.
            c.execute("INSERT INTO suggestion_votes (suggestion_id, user_id, vote) VALUES (1, 7, 1)", [])?;
            assert_eq!(open_count(c, 1)?, 1);
            assert_eq!(month_stats(c, 1, 5)?, (2, 1, 0));
            assert_eq!(top_voters(c, 1, 0, 3)?, vec![(9, 2), (8, 1)]);
            assert_eq!(top_voters(c, 1, 35, 3)?, vec![(9, 1)]);
            Ok(())
        })
        .unwrap();
    }
}
