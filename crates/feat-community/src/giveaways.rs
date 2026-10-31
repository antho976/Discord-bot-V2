//! Giveaways: the `/giveaway` commands, the Enter button, ending in `tick`, and the data the
//! dashboard reads.

use std::collections::HashSet;

use bot_core::anyhow::{Result, anyhow};
use bot_core::discord::{CommandCall, ComponentCall, Reply};
use bot_core::rand::{Rng, seq::SliceRandom};
use bot_core::rusqlite::{Connection, Row, params};
use bot_core::twilight_model::application::command::{Command, CommandType};
use bot_core::twilight_model::channel::ChannelType;
use bot_core::twilight_model::channel::message::component::{ButtonStyle, Component};
use bot_core::twilight_model::channel::message::{AllowedMentions, Embed};
use bot_core::twilight_model::guild::Permissions;
use bot_core::twilight_model::id::Id;
use bot_core::twilight_util::builder::command::{
    ChannelBuilder, CommandBuilder, IntegerBuilder, RoleBuilder, StringBuilder, SubCommandBuilder,
};
use bot_core::twilight_util::builder::embed::{EmbedBuilder, EmbedFieldBuilder, EmbedFooterBuilder};
use bot_core::{Ctx, now_ms, text, tracing};

use crate::{CORAL, KEY, NEUTRAL, button, id_i64, row};

const MIN_SECS: i64 = 60;
const MAX_SECS: i64 = 90 * 86_400;
pub const MAX_WINNERS: i64 = 25;
/// Enough for flags, skin tones and joined emoji like 👩‍💻.
const MAX_EMOJI_CHARS: usize = 10;

#[derive(Debug, Clone)]
pub struct Giveaway {
    pub id: i64,
    pub guild_id: u64,
    pub channel_id: u64,
    pub message_id: Option<u64>,
    pub prize: String,
    pub winners: i64,
    pub ends_at: i64,
    pub required_role: Option<u64>,
    pub created_by: u64,
    pub ended: bool,
    pub winner_ids: Vec<u64>,
    pub created_at: i64,
    /// When it was ended. 0 for rows ended before this was kept, and for running ones.
    pub ended_at: i64,
    /// Earlier winners that a reroll replaced. They aren't picked again.
    pub passed_ids: Vec<u64>,
    /// The creator's pick for the prize tile and the post's title. Empty for the gift icon.
    pub emoji: String,
}

impl Giveaway {
    /// When it finished: the time it was ended, or its scheduled end for rows from before that
    /// was kept.
    pub fn finished_at(&self) -> i64 {
        if self.ended_at > 0 { self.ended_at } else { self.ends_at }
    }
}

const COLS: &str =
    "id, guild_id, channel_id, message_id, prize, winners, ends_at, required_role, created_by, ended, winner_ids, created_at, ended_at, passed_ids, emoji";

fn ids(s: &str) -> Vec<u64> {
    s.split(',').filter_map(|s| s.parse().ok()).collect()
}

fn join_ids(ids: &[u64]) -> String {
    ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",")
}

fn from_row(r: &Row) -> bot_core::rusqlite::Result<Giveaway> {
    Ok(Giveaway {
        id: r.get(0)?,
        guild_id: r.get::<_, i64>(1)? as u64,
        channel_id: r.get::<_, i64>(2)? as u64,
        message_id: r.get::<_, Option<i64>>(3)?.map(|m| m as u64),
        prize: r.get(4)?,
        winners: r.get(5)?,
        ends_at: r.get(6)?,
        required_role: r.get::<_, Option<i64>>(7)?.map(|m| m as u64),
        created_by: r.get::<_, i64>(8)? as u64,
        ended: r.get::<_, i64>(9)? != 0,
        winner_ids: ids(&r.get::<_, String>(10)?),
        created_at: r.get(11)?,
        ended_at: r.get(12)?,
        passed_ids: ids(&r.get::<_, String>(13)?),
        emoji: r.get(14)?,
    })
}

pub fn get(c: &Connection, id: i64) -> bot_core::rusqlite::Result<Option<Giveaway>> {
    use bot_core::rusqlite::OptionalExtension;
    c.query_row(&format!("SELECT {COLS} FROM giveaways WHERE id = ?1"), [id], from_row).optional()
}

pub fn entries(c: &Connection, id: i64) -> bot_core::rusqlite::Result<Vec<u64>> {
    let mut s = c.prepare_cached("SELECT user_id FROM giveaway_entries WHERE giveaway_id = ?1")?;
    s.query_map([id], |r| Ok(r.get::<_, i64>(0)? as u64))?.collect()
}

pub fn entry_count(c: &Connection, id: i64) -> bot_core::rusqlite::Result<i64> {
    c.query_row("SELECT COUNT(*) FROM giveaway_entries WHERE giveaway_id = ?1", [id], |r| r.get(0))
}

/// Up to `limit` of the newest entrants, for the avatars on a card.
pub fn newest_entrants(c: &Connection, id: i64, limit: i64) -> bot_core::rusqlite::Result<Vec<u64>> {
    let mut s = c.prepare_cached("SELECT user_id FROM giveaway_entries WHERE giveaway_id = ?1 ORDER BY rowid DESC LIMIT ?2")?;
    s.query_map(params![id, limit], |r| Ok(r.get::<_, i64>(0)? as u64))?.collect()
}

/// Running and ended giveaway counts for a server.
pub fn counts(c: &Connection, guild: u64) -> bot_core::rusqlite::Result<(i64, i64)> {
    c.query_row(
        "SELECT COALESCE(SUM(ended = 0), 0), COALESCE(SUM(ended = 1), 0) FROM giveaways WHERE guild_id = ?1",
        [id_i64(guild)],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
}

/// Giveaways started since `since` (epoch ms): how many, the entries they got, and how many
/// different people entered.
pub fn since_totals(c: &Connection, guild: u64, since: i64) -> bot_core::rusqlite::Result<(i64, i64, i64)> {
    c.query_row(
        "SELECT COUNT(DISTINCT g.id), COUNT(e.user_id), COUNT(DISTINCT e.user_id)
         FROM giveaways g LEFT JOIN giveaway_entries e ON e.giveaway_id = g.id
         WHERE g.guild_id = ?1 AND g.created_at >= ?2",
        params![id_i64(guild), since],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
}

/// Giveaways of a server with their entry counts: running ones soonest first, ended ones most
/// recently ended first.
pub fn list(c: &Connection, guild: u64, ended: bool, limit: i64) -> bot_core::rusqlite::Result<Vec<(Giveaway, i64)>> {
    let order = if ended { "CASE WHEN ended_at > 0 THEN ended_at ELSE ends_at END DESC" } else { "ends_at ASC" };
    let mut s = c.prepare(&format!(
        "SELECT {COLS}, (SELECT COUNT(*) FROM giveaway_entries e WHERE e.giveaway_id = giveaways.id)
         FROM giveaways WHERE guild_id = ?1 AND ended = ?2 ORDER BY {order} LIMIT ?3"
    ))?;
    s.query_map(params![id_i64(guild), ended as i64, limit], |r| Ok((from_row(r)?, r.get(15)?)))?.collect()
}

/// Add the member to the giveaway, or take them out if they're already in. True when they're in
/// afterwards.
pub fn toggle_entry(c: &Connection, id: i64, user: u64) -> bot_core::rusqlite::Result<bool> {
    let removed = c.execute("DELETE FROM giveaway_entries WHERE giveaway_id = ?1 AND user_id = ?2", params![id, id_i64(user)])?;
    if removed > 0 {
        return Ok(false);
    }
    c.execute("INSERT INTO giveaway_entries (giveaway_id, user_id, entered_at) VALUES (?1, ?2, ?3)", params![id, id_i64(user), now_ms()])?;
    Ok(true)
}

/// Entries made since `since`.
pub fn entered_since(c: &Connection, id: i64, since: i64) -> bot_core::rusqlite::Result<i64> {
    c.query_row("SELECT COUNT(*) FROM giveaway_entries WHERE giveaway_id = ?1 AND entered_at >= ?2", params![id, since], |r| r.get(0))
}

/// Pick up to `count` different winners from the entries, skipping anyone in `exclude`.
pub fn pick_winners<R: Rng + ?Sized>(entries: &[u64], count: usize, exclude: &[u64], rng: &mut R) -> Vec<u64> {
    let mut seen = HashSet::new();
    let mut pool: Vec<u64> = entries.iter().copied().filter(|u| !exclude.contains(u) && seen.insert(*u)).collect();
    pool.shuffle(rng);
    pool.truncate(count);
    pool
}

/// Read "30m", "2h", "1d 12h" and refuse anything under a minute or over 90 days.
pub fn parse_span(input: &str) -> Result<i64, &'static str> {
    let secs = text::parse_duration(input).ok_or("Couldn't read that duration. Try 30m, 2h or 1d 12h.")?;
    if secs < MIN_SECS {
        return Err("A giveaway needs to run for at least a minute.");
    }
    if secs > MAX_SECS {
        return Err("A giveaway can run for 90 days at most.");
    }
    Ok(secs)
}

/// Check an emoji for the prize. Empty is fine and means the gift icon. Custom Discord emoji
/// aren't taken: the dashboard can't draw them.
pub fn parse_emoji(input: &str) -> Result<String, &'static str> {
    let e = input.trim();
    if e.is_empty() {
        return Ok(String::new());
    }
    if e.chars().count() > MAX_EMOJI_CHARS || e.chars().any(|c| c.is_ascii() || c.is_whitespace()) {
        return Err("Use one emoji, like 🎁, or leave it empty.");
    }
    Ok(e.to_owned())
}

pub fn embed(g: &Giveaway, entries: i64) -> Embed {
    let title = if g.emoji.is_empty() { g.prize.clone() } else { format!("{} {}", g.emoji, g.prize) };
    let mut b = EmbedBuilder::new().title(text::clip(&title, 250)).footer(EmbedFooterBuilder::new(format!("Giveaway #{}", g.id)));
    if g.ended {
        let winners = if g.winner_ids.is_empty() {
            "Nobody entered".to_owned()
        } else {
            g.winner_ids.iter().map(|u| format!("<@{u}>")).collect::<Vec<_>>().join(" ")
        };
        b = b
            .color(NEUTRAL)
            .description(format!("Ended <t:{}:R>", g.finished_at() / 1000))
            .field(EmbedFieldBuilder::new(if g.winner_ids.len() == 1 { "Winner" } else { "Winners" }, winners))
            .field(EmbedFieldBuilder::new("Entries", text::num(entries)).inline());
    } else {
        let mut desc = String::from("Press Enter to join. Press it again to leave.");
        if let Some(role) = g.required_role {
            desc.push_str(&format!("\nOnly for <@&{role}>."));
        }
        b = b
            .color(CORAL)
            .description(desc)
            .field(EmbedFieldBuilder::new("Ends", format!("<t:{}:R>", g.ends_at / 1000)).inline())
            .field(EmbedFieldBuilder::new("Winners", g.winners.to_string()).inline())
            .field(EmbedFieldBuilder::new("Entries", text::num(entries)).inline());
    }
    b.build()
}

fn components(id: i64) -> Vec<Component> {
    vec![row(vec![button(&format!("{KEY}:gw:{id}"), "Enter", ButtonStyle::Primary, false)])]
}

// ---- commands ----

pub fn command() -> Command {
    CommandBuilder::new("giveaway", "Run giveaways", CommandType::ChatInput)
        .default_member_permissions(Permissions::MANAGE_GUILD)
        .option(
            SubCommandBuilder::new("start", "Start a giveaway")
                .option(StringBuilder::new("prize", "What's being given away").required(true).max_length(200))
                .option(StringBuilder::new("duration", "How long it runs, like 30m, 2h or 1d 12h").required(true))
                .option(IntegerBuilder::new("winners", "How many winners").required(true).min_value(1).max_value(MAX_WINNERS))
                .option(ChannelBuilder::new("channel", "Where to post it, default this channel").channel_types([ChannelType::GuildText, ChannelType::GuildAnnouncement]))
                .option(RoleBuilder::new("required_role", "Only members with this role can enter"))
                .option(StringBuilder::new("emoji", "An emoji for the prize, like 🎁").max_length(MAX_EMOJI_CHARS as u16)),
        )
        .option(
            SubCommandBuilder::new("end", "End a giveaway now and pick winners")
                .option(IntegerBuilder::new("id", "The giveaway number").required(true).min_value(1)),
        )
        .option(
            SubCommandBuilder::new("reroll", "Pick new winners for an ended giveaway")
                .option(IntegerBuilder::new("id", "The giveaway number").required(true).min_value(1)),
        )
        .option(SubCommandBuilder::new("list", "Running and recent giveaways"))
        .build()
}

pub async fn on_command(ctx: &Ctx, call: &CommandCall) -> Result<()> {
    let Some(guild) = call.guild_id() else { return Ok(()) };
    let Some((sub, opts)) = call.opts().sub() else { return Ok(()) };
    let say = |msg: String| ctx.reply(&call.interaction, Reply::text(msg).ephemeral());
    if !call.member_has(Permissions::MANAGE_GUILD) {
        return say("Giveaways need the Manage Server permission.".into()).await;
    }
    match sub {
        "start" => {
            let prize = opts.str("prize").unwrap_or_default().trim();
            let winners = opts.int("winners").unwrap_or(1).clamp(1, MAX_WINNERS);
            let secs = match parse_span(opts.str("duration").unwrap_or_default()) {
                Ok(s) => s,
                Err(e) => return say(e.into()).await,
            };
            if prize.is_empty() {
                return say("Say what's being given away.".into()).await;
            }
            let emoji = match parse_emoji(opts.str("emoji").unwrap_or_default()) {
                Ok(e) => e,
                Err(e) => return say(e.into()).await,
            };
            let Some(channel) = opts.channel("channel").map(|c| c.get()).or(call.channel_id()) else { return Ok(()) };
            if ctx.channel_in(guild, Some(channel)).is_none() {
                return say("That channel isn't in this server.".into()).await;
            }
            let role = opts.role("required_role").map(|r| r.get());
            ctx.defer(&call.interaction, true).await?;
            let msg = match create(ctx, New { guild, channel, prize, emoji: &emoji, winners, ends_at: now_ms() + secs * 1000, role, by: call.user_id() }).await {
                Ok(id) => format!("Giveaway #{id} is running in <#{channel}>."),
                Err(e) => {
                    tracing::warn!(guild, error = %e, "couldn't start a giveaway");
                    "Couldn't post there. Check that the bot can send messages and embed links in that channel.".into()
                }
            };
            ctx.edit_original(&call.interaction, Reply::text(msg)).await?;
        }
        "end" => {
            let id = opts.int("id").unwrap_or(0);
            if !owned_by(ctx, id, guild)? {
                return say(format!("There's no giveaway #{id} here.")).await;
            }
            ctx.defer(&call.interaction, true).await?;
            let msg = match end(ctx, id).await? {
                EndOutcome::AlreadyEnded => format!("Giveaway #{id} has already ended. Use /giveaway reroll for new winners."),
                EndOutcome::Ended(w) if w.is_empty() => format!("Giveaway #{id} ended. Nobody had entered."),
                EndOutcome::Ended(w) => format!("Giveaway #{id} ended with {} winner{}.", w.len(), if w.len() == 1 { "" } else { "s" }),
                EndOutcome::Missing => format!("There's no giveaway #{id} here."),
            };
            ctx.edit_original(&call.interaction, Reply::text(msg)).await?;
        }
        "reroll" => {
            let id = opts.int("id").unwrap_or(0);
            if !owned_by(ctx, id, guild)? {
                return say(format!("There's no giveaway #{id} here.")).await;
            }
            ctx.defer(&call.interaction, true).await?;
            let msg = match reroll(ctx, id).await? {
                RerollOutcome::Rolled(w) => format!("Picked {}.", w.iter().map(|u| format!("<@{u}>")).collect::<Vec<_>>().join(" ")),
                RerollOutcome::StillRunning => format!("Giveaway #{id} is still running. End it first."),
                RerollOutcome::NoOneElse => "Nobody else entered, so there's no one to pick.".into(),
                RerollOutcome::Missing => format!("There's no giveaway #{id} here."),
            };
            ctx.edit_original(&call.interaction, Reply::text(msg)).await?;
        }
        "list" => {
            let (running, ended) = ctx.db.with(|c| Ok((list(c, guild, false, 10)?, list(c, guild, true, 5)?)))?;
            let line = |(g, n): &(Giveaway, i64)| {
                let state = if g.ended { format!("ended <t:{}:R>", g.finished_at() / 1000) } else { format!("ends <t:{}:R>", g.ends_at / 1000) };
                format!("`#{}` {}, {state}, {} entries", g.id, text::clip(&g.prize, 60), text::num(*n))
            };
            let mut lines: Vec<String> = Vec::new();
            if !running.is_empty() {
                lines.push("**Running**".into());
                lines.extend(running.iter().map(line));
            }
            if !ended.is_empty() {
                lines.push("**Ended**".into());
                lines.extend(ended.iter().map(line));
            }
            if lines.is_empty() {
                return say("No giveaways yet. Start one with /giveaway start.".into()).await;
            }
            ctx.reply(&call.interaction, Reply::embed(EmbedBuilder::new().color(CORAL).description(lines.join("\n")).build()).ephemeral()).await?;
        }
        _ => {}
    }
    Ok(())
}

fn owned_by(ctx: &Ctx, id: i64, guild: u64) -> Result<bool> {
    Ok(ctx.db.with(|c| get(c, id))?.is_some_and(|g| g.guild_id == guild))
}

/// Press of the Enter button: join, or leave when already in.
pub async fn on_enter(ctx: &Ctx, call: &ComponentCall, id: i64) -> Result<()> {
    let reply = |msg: &str| ctx.reply(&call.interaction, Reply::text(msg).ephemeral());
    let Some(g) = ctx.db.with(|c| get(c, id))? else {
        return reply("This giveaway doesn't exist any more.").await;
    };
    if call.guild_id() != Some(g.guild_id) {
        return Ok(());
    }
    if g.ended || g.ends_at <= now_ms() {
        return reply("This giveaway has ended.").await;
    }
    if let Some(role) = g.required_role {
        let has = call.interaction.member.as_ref().is_some_and(|m| m.roles.iter().any(|r| r.get() == role));
        if !has {
            return ctx.reply(&call.interaction, Reply::text(format!("You need <@&{role}> to enter this one.")).ephemeral()).await;
        }
    }
    let user = call.user_id();
    let (entered, count) = ctx.db.with(|c| Ok((toggle_entry(c, id, user)?, entry_count(c, id)?)))?;
    ctx.update(&call.interaction, Reply::embed(embed(&g, count)).components(components(id))).await?;
    let note = if entered { "You're in." } else { "You left the giveaway." };
    ctx.followup(&call.interaction, Reply::text(note).ephemeral()).await
}

// ---- running one ----

/// What it takes to start a giveaway.
pub struct New<'a> {
    pub guild: u64,
    pub channel: u64,
    pub prize: &'a str,
    /// Checked with `parse_emoji`.
    pub emoji: &'a str,
    pub winners: i64,
    pub ends_at: i64,
    pub role: Option<u64>,
    pub by: u64,
}

/// Post a giveaway and store it. Returns its number.
pub async fn create(ctx: &Ctx, new: New<'_>) -> Result<i64> {
    let New { guild, channel, prize, emoji, winners, ends_at, role, by } = new;
    let prize = text::clip(prize.trim(), 200);
    let id = ctx.db.with(|c| {
        c.execute(
            "INSERT INTO giveaways (guild_id, channel_id, prize, emoji, winners, ends_at, required_role, created_by, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![id_i64(guild), id_i64(channel), prize, emoji, winners, ends_at, role.map(id_i64), id_i64(by), now_ms()],
        )?;
        Ok(c.last_insert_rowid())
    })?;
    let g = ctx.db.with(|c| get(c, id))?.ok_or_else(|| anyhow!("giveaway vanished"))?;
    let embeds = [embed(&g, 0)];
    let sent = ctx.http.create_message(Id::new(channel)).embeds(&embeds).components(&components(id)).await;
    match sent {
        Ok(r) => {
            let message = r.model().await?;
            ctx.db.with(|c| c.execute("UPDATE giveaways SET message_id = ?2 WHERE id = ?1", params![id, id_i64(message.id.get())]))?;
            Ok(id)
        }
        Err(e) => {
            ctx.db.with(|c| c.execute("DELETE FROM giveaways WHERE id = ?1", [id]))?;
            Err(e.into())
        }
    }
}

/// What an editor can change on a running giveaway. Checked by the caller.
pub struct Changes<'a> {
    pub prize: &'a str,
    pub emoji: &'a str,
    pub ends_at: i64,
    pub winners: i64,
}

pub enum EditOutcome {
    Missing,
    AlreadyEnded,
    Edited,
}

/// Change a running giveaway and rewrite its message. One that has ended is left alone: the
/// update only applies while `ended` is 0, so it can't race the end in `tick`.
pub async fn edit(ctx: &Ctx, id: i64, ch: Changes<'_>) -> Result<EditOutcome> {
    let prize = text::clip(ch.prize.trim(), 200);
    let changed = ctx.db.with(|c| {
        c.execute(
            "UPDATE giveaways SET prize = ?2, emoji = ?3, ends_at = ?4, winners = ?5 WHERE id = ?1 AND ended = 0",
            params![id, prize, ch.emoji, ch.ends_at, ch.winners.clamp(1, MAX_WINNERS)],
        )
    })?;
    let Some((g, n)) = ctx.db.with(|c| Ok(match get(c, id)? { Some(g) => Some((g, entry_count(c, id)?)), None => None }))? else {
        return Ok(EditOutcome::Missing);
    };
    if changed == 0 {
        return Ok(EditOutcome::AlreadyEnded);
    }
    refresh(ctx, &g, n).await;
    Ok(EditOutcome::Edited)
}

pub enum EndOutcome {
    Missing,
    AlreadyEnded,
    Ended(Vec<u64>),
}

pub enum RerollOutcome {
    Missing,
    StillRunning,
    NoOneElse,
    Rolled(Vec<u64>),
}

/// End a giveaway: pick winners, edit its message and announce them.
pub async fn end(ctx: &Ctx, id: i64) -> Result<EndOutcome> {
    let Some(g) = ctx.db.with(|c| get(c, id))? else { return Ok(EndOutcome::Missing) };
    // Only one caller wins the race to end it.
    let now = now_ms();
    let claimed = ctx.db.with(|c| c.execute("UPDATE giveaways SET ended = 1, ended_at = ?2 WHERE id = ?1 AND ended = 0", params![id, now]))?;
    if claimed == 0 {
        return Ok(EndOutcome::AlreadyEnded);
    }
    let all = ctx.db.with(|c| entries(c, id))?;
    let winners = pick_winners(&all, g.winners.max(1) as usize, &[], &mut bot_core::rand::rng());
    store_winners(ctx, id, &winners)?;
    let g = Giveaway { ended: true, ended_at: now, winner_ids: winners.clone(), ..g };
    refresh(ctx, &g, all.len() as i64).await;
    announce(ctx, &g, &winners, false).await;
    Ok(EndOutcome::Ended(winners))
}

/// Pick new winners from everyone who entered, leaving out everyone picked before.
pub async fn reroll(ctx: &Ctx, id: i64) -> Result<RerollOutcome> {
    let Some(g) = ctx.db.with(|c| get(c, id))? else { return Ok(RerollOutcome::Missing) };
    if !g.ended {
        return Ok(RerollOutcome::StillRunning);
    }
    let all = ctx.db.with(|c| entries(c, id))?;
    let picked_before: Vec<u64> = g.passed_ids.iter().chain(&g.winner_ids).copied().collect();
    let winners = pick_winners(&all, g.winners.max(1) as usize, &picked_before, &mut bot_core::rand::rng());
    if winners.is_empty() {
        return Ok(RerollOutcome::NoOneElse);
    }
    ctx.db.with(|c| {
        c.execute("UPDATE giveaways SET winner_ids = ?2, passed_ids = ?3 WHERE id = ?1", params![id, join_ids(&winners), join_ids(&picked_before)])
    })?;
    let g = Giveaway { winner_ids: winners.clone(), passed_ids: picked_before, ..g };
    refresh(ctx, &g, all.len() as i64).await;
    announce(ctx, &g, &winners, true).await;
    Ok(RerollOutcome::Rolled(winners))
}

fn store_winners(ctx: &Ctx, id: i64, winners: &[u64]) -> Result<()> {
    ctx.db.with(|c| c.execute("UPDATE giveaways SET winner_ids = ?2 WHERE id = ?1", params![id, join_ids(winners)]))?;
    Ok(())
}

/// Rewrite the giveaway's message. A deleted message is not an error worth stopping for.
async fn refresh(ctx: &Ctx, g: &Giveaway, entries: i64) {
    let Some(message) = g.message_id else { return };
    let embeds = [embed(g, entries)];
    let comps = if g.ended { Vec::new() } else { components(g.id) };
    let res = ctx.http.update_message(Id::new(g.channel_id), Id::new(message)).embeds(Some(&embeds)).components(Some(&comps)).await;
    if let Err(e) = res {
        tracing::warn!(giveaway = g.id, error = %e, "couldn't edit the giveaway message");
    }
}

async fn announce(ctx: &Ctx, g: &Giveaway, winners: &[u64], reroll: bool) {
    let content = if winners.is_empty() {
        format!("Nobody entered the {} giveaway.", g.prize)
    } else {
        let who = winners.iter().map(|u| format!("<@{u}>")).collect::<Vec<_>>().join(" ");
        if reroll { format!("New winner for **{}**: {who}", g.prize) } else { format!("Congratulations {who}, you won **{}**.", g.prize) }
    };
    let mentions = AllowedMentions { users: winners.iter().map(|u| Id::new(*u)).collect(), ..Default::default() };
    let mut req = ctx.http.create_message(Id::new(g.channel_id)).content(&content).allowed_mentions(Some(&mentions));
    if let Some(m) = g.message_id {
        req = req.reply(Id::new(m)).fail_if_not_exists(false);
    }
    if let Err(e) = req.await {
        tracing::warn!(giveaway = g.id, error = %e, "couldn't announce giveaway winners");
    }
}

/// End every giveaway whose time is up.
pub async fn tick(ctx: &Ctx) {
    for guild in ctx.enabled_guilds(KEY) {
        let due: Vec<i64> = ctx
            .db
            .with(|c| {
                let mut s = c.prepare_cached("SELECT id FROM giveaways WHERE guild_id = ?1 AND ended = 0 AND ends_at <= ?2")?;
                s.query_map(params![id_i64(guild), now_ms()], |r| r.get(0))?.collect()
            })
            .unwrap_or_default();
        for id in due {
            if let Err(e) = end(ctx, id).await {
                tracing::warn!(guild, giveaway = id, error = %e, "couldn't end a giveaway");
            }
        }
    }
}

pub fn seed(ctx: &Ctx, guild: u64) -> Result<()> {
    use bot_core::preview::MEMBERS;
    let who = |i: usize| MEMBERS[i % MEMBERS.len()].0;
    let hour = 3_600_000i64;
    let now = now_ms();
    // (prize, emoji, winners, ends_in_ms, role, entrants, winners picked)
    type Row = (&'static str, &'static str, i64, i64, Option<u64>, usize, &'static [usize]);
    let rows: [Row; 5] = [
        ("Discord Nitro, 3 months", "💎", 1, 2 * 24 * hour + 5 * hour, None, 8, &[]),
        ("Signed poster from the next stream", "🖼️", 2, 9 * hour, Some(guild + 300), 5, &[]),
        ("Steam key: Hades II", "🎮", 1, 40 * 60_000, None, 3, &[]),
        ("Idleon account boost", "🚀", 1, -5 * hour, None, 9, &[2]),
        ("Merch pack", "👕", 3, -10 * 24 * hour, Some(guild + 100), 7, &[0, 4, 5]),
    ];
    for (prize, emoji, winners, ends_in, role, entrants, picked) in rows {
        let ended = ends_in < 0;
        let ids: Vec<String> = picked.iter().map(|i| who(*i).to_string()).collect();
        let message = 1_300_000_000_000_000_000u64 + now as u64 % 1000;
        ctx.db.with(|c| {
            c.execute(
                "INSERT INTO giveaways (guild_id, channel_id, message_id, prize, winners, ends_at, required_role, created_by, created_at, ended, winner_ids, ended_at, emoji)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![id_i64(guild), id_i64(guild + 30), id_i64(message), prize, winners, now + ends_in, role.map(id_i64), id_i64(who(0)), now - 5 * 24 * hour, ended as i64, ids.join(","), if ended { now + ends_in } else { 0 }, emoji],
            )?;
            let id = c.last_insert_rowid();
            for n in 0..entrants {
                c.execute(
                    "INSERT INTO giveaway_entries (giveaway_id, user_id, entered_at) VALUES (?1, ?2, ?3)",
                    params![id, id_i64(who(n)), now - (n as i64 * 23 % 180) * 60_000],
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
    use bot_core::rand::{SeedableRng, rngs::StdRng};

    #[test]
    fn winners_are_unique_and_capped() {
        let entries = [1, 2, 3, 3, 4, 5, 5, 5];
        for seed in 0..20 {
            let w = pick_winners(&entries, 3, &[], &mut StdRng::seed_from_u64(seed));
            assert_eq!(w.len(), 3);
            let set: HashSet<_> = w.iter().collect();
            assert_eq!(set.len(), 3);
            assert!(w.iter().all(|u| entries.contains(u)));
        }
    }

    #[test]
    fn fewer_entries_than_winners() {
        let w = pick_winners(&[7, 7, 8], 5, &[], &mut StdRng::seed_from_u64(1));
        assert_eq!(w.len(), 2);
        assert!(pick_winners(&[], 2, &[], &mut StdRng::seed_from_u64(1)).is_empty());
    }

    #[test]
    fn reroll_skips_previous_winners() {
        for seed in 0..20 {
            let w = pick_winners(&[1, 2, 3, 4], 2, &[1, 2], &mut StdRng::seed_from_u64(seed));
            assert_eq!(w.len(), 2);
            assert!(w.iter().all(|u| *u == 3 || *u == 4));
        }
        assert!(pick_winners(&[1, 2], 1, &[1, 2], &mut StdRng::seed_from_u64(1)).is_empty());
    }

    #[test]
    fn durations_have_limits() {
        assert_eq!(parse_span("2h"), Ok(7200));
        assert_eq!(parse_span("1d 12h"), Ok(129_600));
        assert_eq!(parse_span("90"), Ok(5400));
        assert!(parse_span("30s").is_err());
        assert!(parse_span("100d").is_err());
        assert!(parse_span("soon").is_err());
    }

    #[test]
    fn emoji_is_one_emoji_or_nothing() {
        assert_eq!(parse_emoji("  ").unwrap(), "");
        assert_eq!(parse_emoji(" 🎁 ").unwrap(), "🎁");
        assert_eq!(parse_emoji("👩‍💻").unwrap(), "👩‍💻");
        assert_eq!(parse_emoji("🇨🇦").unwrap(), "🇨🇦");
        assert!(parse_emoji("gift").is_err());
        assert!(parse_emoji("<:pog:123>").is_err());
        assert!(parse_emoji("🎁 🎁").is_err());
        assert!(parse_emoji(&"🎁".repeat(11)).is_err());
    }

    #[test]
    fn entering_twice_leaves() {
        let db = bot_core::Db::memory().unwrap();
        db.migrate(KEY, crate::MIGRATIONS).unwrap();
        db.with(|c| {
            assert!(toggle_entry(c, 1, 42)?);
            assert!(toggle_entry(c, 1, 43)?);
            assert_eq!(entry_count(c, 1)?, 2);
            assert!(!toggle_entry(c, 1, 42)?);
            assert_eq!(entries(c, 1)?, vec![43]);
            Ok(())
        })
        .unwrap();
    }

    fn ctx() -> Ctx {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.install_features(vec![std::sync::Arc::new(crate::Community)]).unwrap();
        ctx
    }

    #[test]
    fn editing_changes_running_giveaways_only() {
        let rt = bot_core::tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let _in = rt.enter();
        let ctx = ctx();
        ctx.db
            .with(|c| {
                c.execute("INSERT INTO giveaways (id, guild_id, channel_id, prize, ends_at) VALUES (1, 1, 2, 'old', 10), (2, 1, 2, 'done', 10)", [])?;
                c.execute("UPDATE giveaways SET ended = 1 WHERE id = 2", [])
            })
            .unwrap();
        let ch = |prize| Changes { prize, emoji: "🎮", ends_at: 99, winners: 40 };
        rt.block_on(async {
            assert!(matches!(edit(&ctx, 1, ch("  New prize ")).await.unwrap(), EditOutcome::Edited));
            assert!(matches!(edit(&ctx, 2, ch("x")).await.unwrap(), EditOutcome::AlreadyEnded));
            assert!(matches!(edit(&ctx, 3, ch("x")).await.unwrap(), EditOutcome::Missing));
        });
        let g = ctx.db.with(|c| get(c, 1)).unwrap().unwrap();
        assert_eq!((g.prize.as_str(), g.emoji.as_str(), g.ends_at, g.winners), ("New prize", "🎮", 99, MAX_WINNERS));
        assert_eq!(ctx.db.with(|c| get(c, 2)).unwrap().unwrap().prize, "done");
    }

    #[test]
    fn dashboard_totals_read_empty_and_filled_tables() {
        let db = bot_core::Db::memory().unwrap();
        db.migrate(KEY, crate::MIGRATIONS).unwrap();
        db.with(|c| {
            assert_eq!(counts(c, 1)?, (0, 0));
            assert_eq!(since_totals(c, 1, 0)?, (0, 0, 0));
            assert!(newest_entrants(c, 1, 5)?.is_empty());
            c.execute("INSERT INTO giveaways (guild_id, channel_id, prize, ends_at, created_at) VALUES (1, 2, 'x', 10, 5)", [])?;
            c.execute("INSERT INTO giveaway_entries (giveaway_id, user_id) VALUES (1, 7), (1, 8)", [])?;
            assert_eq!(counts(c, 1)?, (1, 0));
            assert_eq!(since_totals(c, 1, 0)?, (1, 2, 2));
            assert_eq!(newest_entrants(c, 1, 1)?, vec![8]);
            Ok(())
        })
        .unwrap();
    }
}
