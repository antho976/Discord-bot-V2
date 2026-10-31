//! AFK status and member timezones: `/afk`, `/timezone`, `/time` and the dashboard page.

mod web;
pub mod zones;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use bot_core::anyhow::Result;
use bot_core::discord::{CommandCall, Reply};
use bot_core::jiff::Timestamp;
use bot_core::rusqlite::{OptionalExtension, params};
use bot_core::twilight_model::application::command::{Command, CommandType};
use bot_core::twilight_model::channel::message::AllowedMentions;
use bot_core::twilight_model::gateway::event::Event;
use bot_core::twilight_model::id::Id;
use bot_core::twilight_util::builder::command::{CommandBuilder, StringBuilder, SubCommandBuilder, UserBuilder};
use bot_core::{Ctx, Feature, FeatureMeta, Group, Icon, Migration, NavItem, feature_async, text};
use serde::{Deserialize, Serialize};

pub const KEY: &str = "utility";

/// What goes in front of a nickname while someone is away.
pub const NICK_PREFIX: &str = "[AFK] ";
/// The bot answers a mention of the same AFK person in the same channel once per this long.
const NOTICE_COOLDOWN_MS: i64 = 10 * 60 * 1000;
/// How long "Welcome back" stays in the channel.
const WELCOME_LIFETIME: Duration = Duration::from_secs(8);
/// The most AFK people one message gets an answer for.
const MAX_NOTICES: usize = 4;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub afk_enabled: bool,
    pub nick_prefix: bool,
    pub welcome_back: bool,
    pub timezones_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { afk_enabled: true, nick_prefix: false, welcome_back: true, timezones_enabled: true }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Afk {
    pub user_id: u64,
    pub reason: String,
    pub since_ms: i64,
    /// The nickname to put back, when the prefix was added. `None` means they had no nickname.
    pub prev_nick: Option<String>,
    pub prefixed: bool,
}

#[derive(Default)]
pub struct Utility {
    notices: Mutex<Notices>,
}

const MIGRATIONS: &[Migration] = &[Migration {
    name: "001_utility",
    sql: "
        CREATE TABLE utility_afk (
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, reason TEXT NOT NULL DEFAULT '',
            since_ms INTEGER NOT NULL, prev_nick TEXT, prefixed INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (guild_id, user_id));
        CREATE TABLE utility_timezones (
            user_id INTEGER NOT NULL PRIMARY KEY, zone TEXT NOT NULL, updated_at INTEGER NOT NULL DEFAULT 0);
    ",
}];

/// Who was already told about whom, so a busy channel doesn't get the same notice twice.
#[derive(Default)]
struct Notices(HashMap<(u64, u64), i64>);

impl Notices {
    /// Of these AFK people, the ones nobody has been told about in `channel` recently. Marks
    /// them as told.
    fn due(&mut self, channel: u64, afk_users: &[u64], now_ms: i64) -> Vec<u64> {
        if self.0.len() > 2000 {
            self.0.retain(|_, t| now_ms - *t < NOTICE_COOLDOWN_MS);
        }
        let mut out = Vec::new();
        for user in afk_users {
            match self.0.get(&(channel, *user)) {
                Some(t) if now_ms - *t < NOTICE_COOLDOWN_MS => {}
                _ => {
                    self.0.insert((channel, *user), now_ms);
                    out.push(*user);
                }
            }
        }
        out
    }
}

/// The people in a message's mentions worth looking up: not the author, no repeats, and no more
/// than a message could sensibly answer for.
fn mention_candidates(author: u64, mentioned: impl IntoIterator<Item = u64>) -> Vec<u64> {
    let mut out: Vec<u64> = Vec::new();
    for id in mentioned {
        if id != author && !out.contains(&id) {
            out.push(id);
        }
    }
    out.truncate(MAX_NOTICES * 3);
    out
}

/// One reason on one line, short enough to read in a notice.
fn clean_reason(raw: &str) -> String {
    let one_line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    text::clip(&one_line, 100)
}

/// "Mira is AFK: lunch (since <t:...:R>)".
fn notice_line(name: &str, a: &Afk) -> String {
    let since = format!("since <t:{}:R>", a.since_ms / 1000);
    if a.reason.is_empty() { format!("{name} is AFK ({since})") } else { format!("{name} is AFK: {} ({since})", a.reason) }
}

/// The nickname while away, kept inside Discord's 32 characters.
fn afk_nick(display: &str) -> String {
    let room = 32 - NICK_PREFIX.chars().count();
    format!("{NICK_PREFIX}{}", display.chars().take(room).collect::<String>())
}

#[feature_async]
impl Feature for Utility {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "AFK and timezones",
            description: "AFK status with mention notices, and member timezones with /time.",
            group: Group::Tools,
            nav: vec![NavItem { label: "AFK and timezones", path: KEY, icon: Icon::Afk, about: "/afk tells people you're away, and /time shows anyone's local time." }],
        }
    }

    fn migrations(&self) -> &'static [Migration] {
        MIGRATIONS
    }

    fn commands(&self) -> Vec<Command> {
        vec![
            CommandBuilder::new("afk", "Mark yourself away. People who mention you are told", CommandType::ChatInput)
                .option(StringBuilder::new("reason", "Why you're away").max_length(100))
                .build(),
            CommandBuilder::new("timezone", "Set the timezone /time shows for you", CommandType::ChatInput)
                .option(
                    SubCommandBuilder::new("set", "Pick your timezone").option(
                        StringBuilder::new("zone", "A city or region, like Europe/Warsaw").required(true).autocomplete(true),
                    ),
                )
                .option(SubCommandBuilder::new("clear", "Forget your timezone"))
                .build(),
            CommandBuilder::new("time", "What time it is for you or someone else", CommandType::ChatInput)
                .option(UserBuilder::new("member", "Whose time to show"))
                .build(),
        ]
    }

    fn routes(&self) -> bot_core::axum::Router<Ctx> {
        web::routes()
    }

    async fn on_event(&self, ctx: &Ctx, event: &Event) -> Result<()> {
        match event {
            Event::MessageCreate(m) => {
                let Some(guild) = m.guild_id.map(|g| g.get()) else { return Ok(()) };
                if m.author.bot {
                    return Ok(());
                }
                let s = ctx.settings::<Settings>(guild, KEY)?;
                if !s.afk_enabled {
                    return Ok(());
                }
                let (author, channel) = (m.author.id.get(), m.channel_id.get());
                if let Some(back) = afk_clear(ctx, guild, author)? {
                    restore_nick(ctx, guild, author, &back).await;
                    if s.welcome_back {
                        let away = ((bot_core::now_ms() - back.since_ms) / 1000).max(0);
                        let name = ctx.display_name(guild, author);
                        let line = format!("Welcome back, {name}. You were away for {}.", text::duration(away));
                        if let Ok(id) = ctx.say(channel, &line).await {
                            let ctx = ctx.clone();
                            bot_core::tokio::spawn(async move {
                                bot_core::tokio::time::sleep(WELCOME_LIFETIME).await;
                                let _ = ctx.http.delete_message(Id::new(channel), Id::new(id)).await;
                            });
                        }
                    }
                }
                if m.mentions.is_empty() {
                    return Ok(());
                }
                let candidates = mention_candidates(author, m.mentions.iter().map(|u| u.id.get()));
                let away = afk_among(ctx, guild, &candidates)?;
                if away.is_empty() {
                    return Ok(());
                }
                let ids: Vec<u64> = away.iter().map(|a| a.user_id).collect();
                let due = self.notices.lock().unwrap().due(channel, &ids, bot_core::now_ms());
                let lines: Vec<String> = away
                    .iter()
                    .filter(|a| due.contains(&a.user_id))
                    .take(MAX_NOTICES)
                    .map(|a| notice_line(&ctx.display_name(guild, a.user_id), a))
                    .collect();
                if lines.is_empty() {
                    return Ok(());
                }
                let none = AllowedMentions::default();
                let sent = ctx
                    .http
                    .create_message(m.channel_id)
                    .reply(m.id)
                    .fail_if_not_exists(false)
                    .content(&lines.join("\n"))
                    .allowed_mentions(Some(&none))
                    .await;
                if let Err(e) = sent {
                    bot_core::tracing::debug!(guild, channel, error = %e, "couldn't post an AFK notice");
                }
            }
            Event::MemberRemove(r) => {
                afk_clear(ctx, r.guild_id.get(), r.user.id.get())?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn on_command(&self, ctx: &Ctx, call: &CommandCall) -> Result<()> {
        let Some(guild) = call.guild_id() else { return Ok(()) };
        let s = ctx.settings::<Settings>(guild, KEY)?;
        let user = call.user_id();
        match call.data.name.as_str() {
            "afk" => {
                if !s.afk_enabled {
                    return ctx.reply(&call.interaction, Reply::text("AFK is turned off on this server.").ephemeral()).await;
                }
                let reason = clean_reason(call.opts().str("reason").unwrap_or_default());
                let current_nick = call.interaction.member.as_ref().and_then(|m| m.nick.clone());
                let before = afk_get(ctx, guild, user)?;
                // Setting it again changes the reason and keeps the nickname we'd restore.
                let (prefixed, prev_nick) = match &before {
                    Some(b) if b.prefixed => (true, b.prev_nick.clone()),
                    _ => (s.nick_prefix && can_edit_nick(ctx, guild, user), current_nick.clone()),
                };
                let since_ms = before.as_ref().map(|b| b.since_ms).unwrap_or_else(bot_core::now_ms);
                afk_set(ctx, guild, &Afk { user_id: user, reason: reason.clone(), since_ms, prev_nick, prefixed })?;
                let msg = if reason.is_empty() {
                    "You're AFK. Your next message ends it.".to_owned()
                } else {
                    format!("You're AFK: {reason}. Your next message ends it.")
                };
                ctx.reply(&call.interaction, Reply::text(msg).ephemeral()).await?;
                if prefixed && before.as_ref().is_none_or(|b| !b.prefixed) {
                    let base = current_nick.unwrap_or_else(|| ctx.display_name(guild, user));
                    set_nick(ctx, guild, user, Some(&afk_nick(&base))).await;
                }
            }
            "timezone" => {
                if !s.timezones_enabled {
                    return ctx.reply(&call.interaction, Reply::text("Timezones are turned off on this server.").ephemeral()).await;
                }
                let Some((sub, opts)) = call.opts().sub() else { return Ok(()) };
                let msg = match sub {
                    "set" => match zones::normalize(opts.str("zone").unwrap_or_default()) {
                        Some(zone) => {
                            tz_set(ctx, user, &zone)?;
                            let there = zones::local(&zone, Timestamp::now()).map(|l| format!(" It's {} there ({}).", l.time, l.offset));
                            format!("Your timezone is {zone}.{}", there.unwrap_or_default())
                        }
                        None => "That isn't a timezone I know. Start typing a city and pick one from the list.".to_owned(),
                    },
                    "clear" => {
                        tz_clear(ctx, user)?;
                        "Your timezone is cleared.".to_owned()
                    }
                    _ => return Ok(()),
                };
                ctx.reply(&call.interaction, Reply::text(msg).ephemeral()).await?;
            }
            "time" => {
                if !s.timezones_enabled {
                    return ctx.reply(&call.interaction, Reply::text("Timezones are turned off on this server.").ephemeral()).await;
                }
                let target = call.opts().user("member").map(|u| u.get()).unwrap_or(user);
                let name = ctx.display_name(guild, target);
                let local = tz_get(ctx, target)?.and_then(|z| zones::local(&z, Timestamp::now()).map(|l| (z, l)));
                let reply = match local {
                    Some((zone, l)) => Reply::text(format!("**{name}**: {} ({}, {zone})", l.time, l.offset)),
                    None if target == user => {
                        Reply::text("You haven't set a timezone. Use /timezone set and pick your city.").ephemeral()
                    }
                    None => Reply::text(format!("{name} hasn't set a timezone.")).ephemeral(),
                };
                ctx.reply(&call.interaction, reply).await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn on_autocomplete(&self, ctx: &Ctx, call: &CommandCall) -> Result<()> {
        let Some(("zone", typed)) = call.opts().focused() else { return Ok(()) };
        let now = Timestamp::now();
        let choices = zones::search(typed, 25)
            .into_iter()
            .map(|name| {
                let label = match zones::local(name, now) {
                    Some(l) => format!("{name} ({})", l.offset),
                    None => name.to_owned(),
                };
                (label, name.to_owned())
            })
            .collect();
        ctx.autocomplete(&call.interaction, choices).await
    }

    fn seed_preview(&self, ctx: &Ctx, guild: u64) -> Result<()> {
        let members = bot_core::preview::MEMBERS;
        let now = bot_core::now_ms();
        let away = [
            (0, "Lunch, back soon", 25 * 60_000),
            (3, "", 3 * 3_600_000 + 12 * 60_000),
            (6, "Exam week, slow replies", 26 * 3_600_000),
        ];
        for (i, reason, ago) in away {
            afk_set(ctx, guild, &Afk { user_id: members[i].0, reason: reason.into(), since_ms: now - ago, prev_nick: None, prefixed: false })?;
        }
        let zones = [
            "Europe/Warsaw", "America/New_York", "Asia/Tokyo", "Europe/London", "Australia/Sydney",
            "America/Los_Angeles", "Asia/Kolkata", "Europe/Berlin",
        ];
        for (i, zone) in zones.iter().enumerate() {
            tz_set(ctx, members[i].0, zone)?;
        }
        Ok(())
    }
}

// ---- AFK storage ----

fn afk_from_row(r: &bot_core::rusqlite::Row<'_>) -> bot_core::rusqlite::Result<Afk> {
    Ok(Afk {
        user_id: r.get::<_, i64>(0)? as u64,
        reason: r.get(1)?,
        since_ms: r.get(2)?,
        prev_nick: r.get(3)?,
        prefixed: r.get::<_, i64>(4)? != 0,
    })
}

const AFK_COLUMNS: &str = "user_id, reason, since_ms, prev_nick, prefixed";

pub fn afk_get(ctx: &Ctx, guild: u64, user: u64) -> Result<Option<Afk>> {
    ctx.db.with(|c| {
        c.query_row(
            &format!("SELECT {AFK_COLUMNS} FROM utility_afk WHERE guild_id = ?1 AND user_id = ?2"),
            params![guild as i64, user as i64],
            afk_from_row,
        )
        .optional()
    })
}

pub fn afk_set(ctx: &Ctx, guild: u64, a: &Afk) -> Result<()> {
    ctx.db.with(|c| {
        c.execute(
            "INSERT OR REPLACE INTO utility_afk (guild_id, user_id, reason, since_ms, prev_nick, prefixed) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![guild as i64, a.user_id as i64, a.reason, a.since_ms, a.prev_nick, a.prefixed as i64],
        )
    })?;
    Ok(())
}

/// Remove someone's AFK state and return it, if they had any.
pub fn afk_clear(ctx: &Ctx, guild: u64, user: u64) -> Result<Option<Afk>> {
    ctx.db.with(|c| {
        c.query_row(
            &format!("DELETE FROM utility_afk WHERE guild_id = ?1 AND user_id = ?2 RETURNING {AFK_COLUMNS}"),
            params![guild as i64, user as i64],
            afk_from_row,
        )
        .optional()
    })
}

/// Which of these users are AFK, longest away first.
fn afk_among(ctx: &Ctx, guild: u64, users: &[u64]) -> Result<Vec<Afk>> {
    let mut out = Vec::new();
    for user in users {
        if let Some(a) = afk_get(ctx, guild, *user)? {
            out.push(a);
        }
    }
    out.sort_by_key(|a| a.since_ms);
    Ok(out)
}

pub fn afk_list(ctx: &Ctx, guild: u64) -> Result<Vec<Afk>> {
    ctx.db.with(|c| {
        let mut st = c.prepare_cached(&format!("SELECT {AFK_COLUMNS} FROM utility_afk WHERE guild_id = ?1 ORDER BY since_ms"))?;
        st.query_map([guild as i64], afk_from_row)?.collect::<Result<Vec<_>, _>>()
    })
}

// ---- nicknames ----

/// False for the server owner and for members whose top role sits at or above the bot's, since
/// Discord refuses those edits.
fn can_edit_nick(ctx: &Ctx, guild: u64, user: u64) -> bool {
    let (g, u) = (Id::new(guild), Id::new(user));
    if ctx.cache.guild(g).is_some_and(|g| g.owner_id().get() == user) {
        return false;
    }
    let top = |member: Id<bot_core::twilight_model::id::marker::UserMarker>| -> Option<i64> {
        let m = ctx.cache.member(g, member)?;
        Some(m.roles().iter().filter_map(|r| ctx.cache.role(*r).map(|r| r.position)).max().unwrap_or(0))
    };
    match (ctx.bot_id().and_then(|b| top(Id::new(b))), top(u)) {
        (Some(bot), Some(them)) => bot > them,
        _ => true,
    }
}

async fn set_nick(ctx: &Ctx, guild: u64, user: u64, nick: Option<&str>) {
    if let Err(e) = ctx.http.update_guild_member(Id::new(guild), Id::new(user)).nick(nick).await {
        bot_core::tracing::debug!(guild, user, error = %e, "couldn't change a nickname");
    }
}

/// Put the old nickname back, unless they've changed it themselves since.
pub async fn restore_nick(ctx: &Ctx, guild: u64, user: u64, was: &Afk) {
    if !was.prefixed {
        return;
    }
    let now = ctx.cache.member(Id::new(guild), Id::new(user)).and_then(|m| m.nick().map(str::to_owned));
    if now.is_some_and(|n| n.starts_with(NICK_PREFIX)) {
        set_nick(ctx, guild, user, was.prev_nick.as_deref()).await;
    }
}

// ---- timezone storage ----

pub fn tz_get(ctx: &Ctx, user: u64) -> Result<Option<String>> {
    ctx.db.with(|c| c.query_row("SELECT zone FROM utility_timezones WHERE user_id = ?1", [user as i64], |r| r.get(0)).optional())
}

pub fn tz_set(ctx: &Ctx, user: u64, zone: &str) -> Result<()> {
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO utility_timezones (user_id, zone, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (user_id) DO UPDATE SET zone = excluded.zone, updated_at = excluded.updated_at",
            params![user as i64, zone, bot_core::now_ms()],
        )
    })?;
    Ok(())
}

pub fn tz_clear(ctx: &Ctx, user: u64) -> Result<()> {
    ctx.db.with(|c| c.execute("DELETE FROM utility_timezones WHERE user_id = ?1", [user as i64]))?;
    Ok(())
}

pub fn tz_all(ctx: &Ctx) -> Result<Vec<(u64, String)>> {
    ctx.db.with(|c| {
        let mut st = c.prepare_cached("SELECT user_id, zone FROM utility_timezones")?;
        st.query_map([], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    #[test]
    fn notices_once_per_channel_per_ten_minutes() {
        let mut n = Notices::default();
        assert_eq!(n.due(1, &[10, 11], 0), vec![10, 11]);
        // Same channel again: nothing new to say.
        assert!(n.due(1, &[10, 11], 5 * MIN).is_empty());
        // Another channel is its own conversation.
        assert_eq!(n.due(2, &[10], 5 * MIN), vec![10]);
        // A new AFK person in the first channel is still announced.
        assert_eq!(n.due(1, &[10, 12], 9 * MIN), vec![12]);
        // After ten minutes it's allowed again.
        assert_eq!(n.due(1, &[10], 10 * MIN), vec![10]);
        // A repeat inside one call counts once.
        assert_eq!(n.due(3, &[20, 20], 0), vec![20]);
    }

    #[test]
    fn stale_notices_are_pruned() {
        let mut n = Notices::default();
        for u in 0..2100u64 {
            n.due(1, &[u], 0);
        }
        n.due(1, &[9999], 11 * MIN);
        assert!(n.0.len() < 10);
    }

    #[test]
    fn candidates_skip_the_author_and_repeats() {
        assert_eq!(mention_candidates(1, [1, 2, 2, 3]), vec![2, 3]);
        assert!(mention_candidates(1, [1, 1]).is_empty());
        assert_eq!(mention_candidates(1, 2..100).len(), MAX_NOTICES * 3);
    }

    #[test]
    fn notice_reads_plainly() {
        let a = Afk { user_id: 1, reason: "lunch".into(), since_ms: 1_700_000_000_000, prev_nick: None, prefixed: false };
        assert_eq!(notice_line("Mira", &a), "Mira is AFK: lunch (since <t:1700000000:R>)");
        let bare = Afk { reason: String::new(), ..a };
        assert_eq!(notice_line("Mira", &bare), "Mira is AFK (since <t:1700000000:R>)");
    }

    #[test]
    fn reasons_are_one_short_line() {
        assert_eq!(clean_reason("  at the\n dentist  "), "at the dentist");
        assert_eq!(clean_reason(&"x".repeat(300)).chars().count(), 100);
    }

    #[test]
    fn nickname_prefix_fits_discord() {
        assert_eq!(afk_nick("Mira"), "[AFK] Mira");
        let long = afk_nick(&"n".repeat(32));
        assert_eq!(long.chars().count(), 32);
        assert!(long.starts_with(NICK_PREFIX));
    }
}
