//! XP and levels: XP from messages, voice and reactions, level-up messages, role rewards,
//! `/rank`, `/leaderboard`, `/xp`, the dashboard pages, and the import from antho-s-bot.

pub mod curve;
pub mod import;
mod web;

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::Instant;

use bot_core::anyhow::Result;
use bot_core::discord::{CommandCall, ComponentCall, Reply, button_row};
use bot_core::rusqlite::{OptionalExtension, params};
use bot_core::twilight_model::application::command::{Command, CommandType};
use bot_core::twilight_model::channel::message::component::ButtonStyle;
use bot_core::twilight_model::channel::message::AllowedMentions;
use bot_core::twilight_model::gateway::event::Event;
use bot_core::twilight_model::guild::Permissions;
use bot_core::twilight_model::id::Id;
use bot_core::twilight_util::builder::command::{CommandBuilder, IntegerBuilder, SubCommandBuilder, UserBuilder};
use bot_core::twilight_util::builder::embed::{EmbedBuilder, EmbedFieldBuilder};
use bot_core::{Ctx, Feature, FeatureMeta, Group, Icon, Migration, NavItem, feature_async, text};
use rand::RngExt as _;
use serde::{Deserialize, Serialize};

pub const KEY: &str = "levels";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub xp_min: i64,
    pub xp_max: i64,
    pub cooldown_secs: i64,
    pub voice_xp: i64,
    pub voice_require_active: bool,
    pub reaction_xp: i64,
    pub curve_base: i64,
    pub curve_factor: f64,
    /// Level -> XP that level costs, overriding the curve. Carried over from antho-s-bot.
    pub custom_curve: BTreeMap<u32, i64>,
    pub announce: bool,
    /// None posts in the channel where they levelled up.
    pub announce_channel: Option<u64>,
    pub announce_ping: bool,
    pub announce_min_level: u32,
    pub message: String,
    /// Keep every reward role earned, instead of only the highest.
    pub stack_rewards: bool,
    pub no_xp_channels: Vec<u64>,
    pub no_xp_roles: Vec<u64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            xp_min: 15,
            xp_max: 25,
            cooldown_secs: 60,
            voice_xp: 5,
            voice_require_active: true,
            reaction_xp: 0,
            curve_base: 100,
            curve_factor: 1.2,
            custom_curve: BTreeMap::new(),
            announce: true,
            announce_channel: None,
            announce_ping: true,
            announce_min_level: 0,
            message: DEFAULT_MESSAGE.into(),
            stack_rewards: true,
            no_xp_channels: Vec::new(),
            no_xp_roles: Vec::new(),
        }
    }
}

pub const DEFAULT_MESSAGE: &str = "{user} reached level {level}.";

impl Settings {
    pub fn progress(&self, xp: i64) -> curve::Progress {
        curve::progress(xp, self.curve_base, self.curve_factor, &self.custom_curve)
    }
}

#[derive(Debug, Clone)]
pub struct Member {
    pub user_id: u64,
    pub xp: i64,
    pub level: u32,
    pub messages: i64,
    pub voice_minutes: i64,
}

#[derive(Default)]
pub struct Levels {
    cooldowns: Mutex<HashMap<(u64, u64), Instant>>,
    reaction_cooldowns: Mutex<HashMap<(u64, u64), Instant>>,
}

const MIGRATIONS: &[Migration] = &[Migration {
    name: "001_levels",
    sql: "
        CREATE TABLE levels (
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, xp INTEGER NOT NULL DEFAULT 0,
            level INTEGER NOT NULL DEFAULT 0, messages INTEGER NOT NULL DEFAULT 0,
            voice_minutes INTEGER NOT NULL DEFAULT 0, prestige INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (guild_id, user_id));
        CREATE INDEX levels_by_xp ON levels(guild_id, xp DESC);
        CREATE TABLE level_rewards (
            guild_id INTEGER NOT NULL, level INTEGER NOT NULL, role_id INTEGER NOT NULL,
            PRIMARY KEY (guild_id, level));
    ",
}, Migration {
    name: "002_levels_week",
    sql: "
        CREATE TABLE level_week (
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, xp INTEGER NOT NULL,
            PRIMARY KEY (guild_id, user_id));
        CREATE TABLE level_week_state (guild_id INTEGER PRIMARY KEY, week TEXT NOT NULL);
        CREATE TABLE level_ups (
            id INTEGER PRIMARY KEY, guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL,
            level INTEGER NOT NULL, role_id INTEGER, at INTEGER NOT NULL);
        CREATE INDEX level_ups_by_time ON level_ups(guild_id, at);
    ",
}];

#[feature_async]
impl Feature for Levels {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "XP and levels",
            description: "XP from messages, voice and reactions, level-up messages and role rewards.",
            group: Group::Members,
            nav: vec![NavItem { label: "Levels", path: "levels", icon: Icon::Trophy }],
        }
    }

    fn migrations(&self) -> &'static [Migration] {
        MIGRATIONS
    }

    fn commands(&self) -> Vec<Command> {
        vec![
            CommandBuilder::new("rank", "Your level and XP, or someone else's", CommandType::ChatInput)
                .option(UserBuilder::new("member", "Whose rank to show"))
                .build(),
            CommandBuilder::new("leaderboard", "The server's top members by XP", CommandType::ChatInput).build(),
            CommandBuilder::new("xp", "Change a member's XP", CommandType::ChatInput)
                .default_member_permissions(Permissions::MANAGE_GUILD)
                .option(
                    SubCommandBuilder::new("give", "Add XP to a member")
                        .option(UserBuilder::new("member", "Who gets the XP").required(true))
                        .option(IntegerBuilder::new("amount", "How much XP").required(true).min_value(1).max_value(1_000_000)),
                )
                .option(
                    SubCommandBuilder::new("take", "Remove XP from a member")
                        .option(UserBuilder::new("member", "Who loses the XP").required(true))
                        .option(IntegerBuilder::new("amount", "How much XP").required(true).min_value(1).max_value(1_000_000)),
                )
                .option(
                    SubCommandBuilder::new("set-level", "Put a member at the start of a level")
                        .option(UserBuilder::new("member", "Whose level to set").required(true))
                        .option(IntegerBuilder::new("level", "The level").required(true).min_value(0).max_value(500)),
                )
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
                if s.no_xp_channels.contains(&m.channel_id.get()) {
                    return Ok(());
                }
                if let Some(member) = &m.member
                    && member.roles.iter().any(|r| s.no_xp_roles.contains(&r.get())) {
                        return Ok(());
                    }
                let key = (guild, m.author.id.get());
                {
                    let mut cd = self.cooldowns.lock().unwrap();
                    if let Some(last) = cd.get(&key)
                        && last.elapsed().as_secs() < s.cooldown_secs.max(0) as u64 {
                            return Ok(());
                        }
                    cd.insert(key, Instant::now());
                    if cd.len() > 5000 {
                        let limit = s.cooldown_secs.max(0) as u64;
                        cd.retain(|_, t| t.elapsed().as_secs() < limit);
                    }
                }
                let amount = if s.xp_max > s.xp_min { rand::rng().random_range(s.xp_min..=s.xp_max) } else { s.xp_min };
                grant(ctx, guild, m.author.id.get(), amount, Earned::Message, Some(m.channel_id.get())).await?;
            }
            Event::ReactionAdd(r) => {
                let Some(guild) = r.guild_id.map(|g| g.get()) else { return Ok(()) };
                if r.member.as_ref().is_some_and(|m| m.user.bot) {
                    return Ok(());
                }
                let s = ctx.settings::<Settings>(guild, KEY)?;
                if s.reaction_xp <= 0 || s.no_xp_channels.contains(&r.channel_id.get()) {
                    return Ok(());
                }
                let key = (guild, r.user_id.get());
                {
                    let mut cd = self.reaction_cooldowns.lock().unwrap();
                    if cd.get(&key).is_some_and(|t| t.elapsed().as_secs() < s.cooldown_secs.max(0) as u64) {
                        return Ok(());
                    }
                    cd.insert(key, Instant::now());
                }
                grant(ctx, guild, r.user_id.get(), s.reaction_xp, Earned::Reaction, None).await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn tick(&self, ctx: &Ctx) -> Result<()> {
        for guild in ctx.enabled_guilds(KEY) {
            roll_week(ctx, guild)?;
        }
        // One minute of voice XP for everyone talking in a channel.
        for guild in ctx.enabled_guilds(KEY) {
            let s = ctx.settings::<Settings>(guild, KEY)?;
            if s.voice_xp <= 0 {
                continue;
            }
            let afk = ctx.cache.guild(Id::new(guild)).and_then(|g| g.afk_channel_id());
            let states: Vec<_> = ctx
                .cache
                .iter()
                .voice_states()
                .filter(|v| v.guild_id().get() == guild)
                .map(|v| (v.user_id().get(), v.channel_id(), v.self_mute() || v.self_deaf() || v.mute() || v.deaf()))
                .collect();
            for (user, channel, quiet) in &states {
                if Some(*channel) == afk || (s.voice_require_active && *quiet) {
                    continue;
                }
                if ctx.cache.user(Id::new(*user)).is_some_and(|u| u.bot) {
                    continue;
                }
                // Alone in a channel earns nothing.
                let humans = states.iter().filter(|(_, c, _)| c == channel).count();
                if humans < 2 {
                    continue;
                }
                grant(ctx, guild, *user, s.voice_xp, Earned::Voice, None).await?;
            }
        }
        Ok(())
    }

    async fn on_command(&self, ctx: &Ctx, call: &CommandCall) -> Result<()> {
        let Some(guild) = call.guild_id() else { return Ok(()) };
        let s = ctx.settings::<Settings>(guild, KEY)?;
        match call.data.name.as_str() {
            "rank" => {
                let user = call.opts().user("member").map(|u| u.get()).unwrap_or(call.user_id());
                let m = member(ctx, guild, user)?.unwrap_or(Member { user_id: user, xp: 0, level: 0, messages: 0, voice_minutes: 0 });
                let p = s.progress(m.xp);
                let place = rank_of(ctx, guild, user)?;
                let name = ctx.display_name(guild, user);
                let embed = EmbedBuilder::new()
                    .title(format!("{name}, level {}", p.level))
                    .color(0xf0806a)
                    .description(format!("`{}`  {} / {} XP", curve::bar(p.into, p.needed, 18), text::num(p.into), text::num(p.needed)))
                    .field(EmbedFieldBuilder::new("Rank", place.map(|r| format!("#{r}")).unwrap_or_else(|| "Unranked".into())).inline())
                    .field(EmbedFieldBuilder::new("Total XP", text::num(m.xp)).inline())
                    .field(EmbedFieldBuilder::new("Messages", text::num(m.messages)).inline())
                    .field(EmbedFieldBuilder::new("Voice", text::duration(m.voice_minutes * 60)).inline())
                    .build();
                ctx.reply(&call.interaction, Reply::embed(embed)).await?;
            }
            "leaderboard" => {
                let (embed, rows) = leaderboard(ctx, guild, 0)?;
                ctx.reply(&call.interaction, Reply::embed(embed).components(rows)).await?;
            }
            "xp" => {
                // Discord hides /xp from members without Manage Server, but server admins can
                // loosen that per role, so check here too.
                if !call.member_has(Permissions::MANAGE_GUILD) {
                    ctx.reply(&call.interaction, Reply::text("You need Manage Server to change XP.").ephemeral()).await?;
                    return Ok(());
                }
                let Some((sub, opts)) = call.opts().sub() else { return Ok(()) };
                let Some(user) = opts.user("member").map(|u| u.get()) else { return Ok(()) };
                let name = ctx.display_name(guild, user);
                let msg = match sub {
                    "give" => {
                        let n = opts.int("amount").unwrap_or(0);
                        grant(ctx, guild, user, n, Earned::Manual, None).await?;
                        format!("Gave {} XP to {name}.", text::num(n))
                    }
                    "take" => {
                        let n = opts.int("amount").unwrap_or(0);
                        let cur = member(ctx, guild, user)?.map(|m| m.xp).unwrap_or(0);
                        set_xp(ctx, guild, user, (cur - n).max(0)).await?;
                        format!("Took {} XP from {name}.", text::num(n))
                    }
                    "set-level" => {
                        let level = opts.int("level").unwrap_or(0).max(0) as u32;
                        set_xp(ctx, guild, user, curve::xp_to_reach(level, s.curve_base, s.curve_factor, &s.custom_curve)).await?;
                        format!("{name} is now level {level}.")
                    }
                    _ => return Ok(()),
                };
                ctx.reply(&call.interaction, Reply::text(msg).ephemeral()).await?;
            }
            _ => {}
        }
        Ok(())
    }

    fn seed_preview(&self, ctx: &Ctx, guild: u64) -> Result<()> {
        preview_rows(ctx, guild)
    }

    fn overview(&self, ctx: &Ctx, page: &bot_core::web::Page) -> bot_core::Overview {
        web::overview(ctx, page)
    }

    async fn on_component(&self, ctx: &Ctx, call: &ComponentCall) -> Result<()> {
        let Some(guild) = call.guild_id() else { return Ok(()) };
        if let ["lb", page] = call.args().as_slice() {
            let page: i64 = page.parse().unwrap_or(0);
            let (embed, rows) = leaderboard(ctx, guild, page.max(0))?;
            ctx.update(&call.interaction, Reply::embed(embed).components(rows)).await?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Earned {
    Message,
    Voice,
    Reaction,
    Manual,
}

pub fn member(ctx: &Ctx, guild: u64, user: u64) -> Result<Option<Member>> {
    ctx.db.with(|c| {
        c.query_row(
            "SELECT user_id, xp, level, messages, voice_minutes FROM levels WHERE guild_id = ?1 AND user_id = ?2",
            params![guild as i64, user as i64],
            |r| {
                Ok(Member {
                    user_id: r.get::<_, i64>(0)? as u64,
                    xp: r.get(1)?,
                    level: r.get::<_, i64>(2)? as u32,
                    messages: r.get(3)?,
                    voice_minutes: r.get(4)?,
                })
            },
        )
        .optional()
    })
}

fn rank_of(ctx: &Ctx, guild: u64, user: u64) -> Result<Option<i64>> {
    ctx.db.with(|c| {
        c.query_row(
            "SELECT (SELECT COUNT(*) FROM levels o WHERE o.guild_id = l.guild_id AND o.xp > l.xp) + 1
             FROM levels l WHERE l.guild_id = ?1 AND l.user_id = ?2 AND l.xp > 0",
            params![guild as i64, user as i64],
            |r| r.get(0),
        )
        .optional()
    })
}

pub fn top(ctx: &Ctx, guild: u64, offset: i64, limit: i64, search: Option<&[u64]>) -> Result<Vec<Member>> {
    ctx.db.with(|c| {
        let mut out = Vec::new();
        let mut stmt = c.prepare_cached(
            "SELECT user_id, xp, level, messages, voice_minutes FROM levels WHERE guild_id = ?1 AND xp > 0
             ORDER BY xp DESC LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![guild as i64, if search.is_some() { 100_000 } else { limit }, if search.is_some() { 0 } else { offset }], |r| {
            Ok(Member {
                user_id: r.get::<_, i64>(0)? as u64,
                xp: r.get(1)?,
                level: r.get::<_, i64>(2)? as u32,
                messages: r.get(3)?,
                voice_minutes: r.get(4)?,
            })
        })?;
        for m in rows {
            let m = m?;
            if search.is_none_or(|ids| ids.contains(&m.user_id)) {
                out.push(m);
            }
        }
        Ok(out)
    })
}

pub fn count(ctx: &Ctx, guild: u64) -> Result<i64> {
    ctx.db.with(|c| c.query_row("SELECT COUNT(*) FROM levels WHERE guild_id = ?1 AND xp > 0", [guild as i64], |r| r.get(0)))
}

fn leaderboard(ctx: &Ctx, guild: u64, page: i64) -> Result<(bot_core::twilight_model::channel::message::Embed, Vec<bot_core::twilight_model::channel::message::component::Component>)> {
    let per = 10;
    let total = count(ctx, guild)?;
    let pages = ((total + per - 1) / per).max(1);
    let page = page.min(pages - 1);
    let rows = top(ctx, guild, page * per, per, None)?;
    let mut lines = Vec::new();
    for (i, m) in rows.iter().enumerate() {
        let place = page * per + i as i64 + 1;
        lines.push(format!("`{place:>2}`  {}  ·  level {}  ·  {} XP", ctx.display_name(guild, m.user_id), m.level, text::num(m.xp)));
    }
    let embed = EmbedBuilder::new()
        .title(format!("{} leaderboard", ctx.guild_name(guild)))
        .color(0xf0806a)
        .description(if lines.is_empty() { "Nobody has XP yet.".into() } else { lines.join("\n") })
        .footer(bot_core::twilight_util::builder::embed::EmbedFooterBuilder::new(format!("Page {} of {pages}", page + 1)))
        .build();
    let mut buttons = Vec::new();
    if pages > 1 {
        let prev = format!("{KEY}:lb:{}", (page - 1).max(0));
        let next = format!("{KEY}:lb:{}", (page + 1).min(pages - 1));
        buttons.push(button_row(&[(&prev, "Previous", ButtonStyle::Secondary), (&next, "Next", ButtonStyle::Secondary)]));
    }
    Ok((embed, buttons))
}

/// Add XP and handle a level-up: announcement and reward roles.
async fn grant(ctx: &Ctx, guild: u64, user: u64, amount: i64, how: Earned, channel: Option<u64>) -> Result<()> {
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let before = member(ctx, guild, user)?.map(|m| (m.xp, m.level)).unwrap_or((0, 0));
    let xp = (before.0 + amount).max(0);
    let level = s.progress(xp).level;
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO levels (guild_id, user_id, xp, level, messages, voice_minutes, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (guild_id, user_id) DO UPDATE SET xp = excluded.xp, level = excluded.level,
               messages = messages + ?5, voice_minutes = voice_minutes + ?6, updated_at = excluded.updated_at",
            params![
                guild as i64,
                user as i64,
                xp,
                level as i64,
                (how == Earned::Message) as i64,
                (how == Earned::Voice) as i64,
                bot_core::now_ms()
            ],
        )
    })?;
    if level > before.1 {
        let role = rewards(ctx, guild).unwrap_or_default().into_iter().find(|(l, _)| *l == level).map(|(_, r)| r as i64);
        ctx.db.with(|c| {
            c.execute(
                "INSERT INTO level_ups (guild_id, user_id, level, role_id, at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![guild as i64, user as i64, level as i64, role, bot_core::now_ms()],
            )
        })?;
    }
    if level != before.1 {
        sync_rewards(ctx, guild, user, level).await;
        if level > before.1 && s.announce && level >= s.announce_min_level {
            announce(ctx, &s, guild, user, level, channel).await;
        }
    }
    Ok(())
}

/// This ISO week, like "2026-W44", in UTC.
fn week_key() -> String {
    let d = bot_core::jiff::Timestamp::now().to_zoned(bot_core::jiff::tz::TimeZone::UTC).date();
    let w = d.iso_week_date();
    format!("{}-W{:02}", w.year(), w.week())
}

/// At the start of each week, remember everyone's XP so "this week" can be worked out, and drop
/// level-ups older than a month.
fn roll_week(ctx: &Ctx, guild: u64) -> Result<()> {
    let week = week_key();
    let mut conn = ctx.db.lock();
    let stored: Option<String> =
        conn.query_row("SELECT week FROM level_week_state WHERE guild_id = ?1", [guild as i64], |r| r.get(0)).optional()?;
    if stored.as_deref() == Some(week.as_str()) {
        return Ok(());
    }
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM level_week WHERE guild_id = ?1", [guild as i64])?;
    tx.execute("INSERT INTO level_week (guild_id, user_id, xp) SELECT guild_id, user_id, xp FROM levels WHERE guild_id = ?1", [guild as i64])?;
    tx.execute(
        "INSERT INTO level_week_state (guild_id, week) VALUES (?1, ?2) ON CONFLICT (guild_id) DO UPDATE SET week = excluded.week",
        params![guild as i64, week],
    )?;
    tx.execute("DELETE FROM level_ups WHERE guild_id = ?1 AND at < ?2", params![guild as i64, bot_core::now_ms() - 30 * 86_400_000])?;
    tx.commit()?;
    Ok(())
}

/// Set a member's XP outright and fix their level and roles.
pub async fn set_xp(ctx: &Ctx, guild: u64, user: u64, xp: i64) -> Result<()> {
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let level = s.progress(xp).level;
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO levels (guild_id, user_id, xp, level, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (guild_id, user_id) DO UPDATE SET xp = excluded.xp, level = excluded.level, updated_at = excluded.updated_at",
            params![guild as i64, user as i64, xp, level as i64, bot_core::now_ms()],
        )
    })?;
    sync_rewards(ctx, guild, user, level).await;
    Ok(())
}

pub fn rewards(ctx: &Ctx, guild: u64) -> Result<Vec<(u32, u64)>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached("SELECT level, role_id FROM level_rewards WHERE guild_id = ?1 ORDER BY level")?;
        
        s.query_map([guild as i64], |r| Ok((r.get::<_, i64>(0)? as u32, r.get::<_, i64>(1)? as u64)))?.collect()
    })
}

async fn sync_rewards(ctx: &Ctx, guild: u64, user: u64, level: u32) {
    let Ok(list) = rewards(ctx, guild) else { return };
    if list.is_empty() {
        return;
    }
    let stack = ctx.settings::<Settings>(guild, KEY).map(|s| s.stack_rewards).unwrap_or(true);
    let earned: Vec<u64> = list.iter().filter(|(l, _)| *l <= level).map(|(_, r)| *r).collect();
    let keep: Vec<u64> = if stack { earned.clone() } else { earned.last().copied().into_iter().collect() };
    let has: Vec<u64> = ctx
        .cache
        .member(Id::new(guild), Id::new(user))
        .map(|m| m.roles().iter().map(|r| r.get()).collect())
        .unwrap_or_default();
    for (_, role) in &list {
        let want = keep.contains(role);
        let have = has.contains(role);
        let res = if want && !have {
            ctx.http.add_guild_member_role(Id::new(guild), Id::new(user), Id::new(*role)).await.map(|_| ())
        } else if !want && have {
            ctx.http.remove_guild_member_role(Id::new(guild), Id::new(user), Id::new(*role)).await.map(|_| ())
        } else {
            Ok(())
        };
        if let Err(e) = res {
            bot_core::tracing::warn!(guild, user, role, error = %e, "couldn't update a level reward role");
        }
    }
}

async fn announce(ctx: &Ctx, s: &Settings, guild: u64, user: u64, level: u32, here: Option<u64>) {
    let Some(channel) = s.announce_channel.or(here) else { return };
    let who = if s.announce_ping { format!("<@{user}>") } else { ctx.display_name(guild, user) };
    let template = if s.message.trim().is_empty() { DEFAULT_MESSAGE } else { s.message.as_str() };
    let content = text::fill(template, &[("user", &who), ("level", &level.to_string())]);
    let mentions = AllowedMentions { users: if s.announce_ping { vec![Id::new(user)] } else { vec![] }, ..Default::default() };
    if let Err(e) = ctx.http.create_message(Id::new(channel)).content(&content).allowed_mentions(Some(&mentions)).await {
        bot_core::tracing::warn!(guild, channel, error = %e, "couldn't post a level-up");
    }
}

pub(crate) fn preview_rows(ctx: &Ctx, guild: u64) -> Result<()> {
    let s = Settings::default();
    for (i, (user, _)) in bot_core::preview::MEMBERS.iter().enumerate() {
        let xp = 48_000 / (i as i64 + 1) + (i as i64 * 731) % 900;
        ctx.db.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO levels (guild_id, user_id, xp, level, messages, voice_minutes, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
                params![guild as i64, *user as i64, xp, s.progress(xp).level as i64, xp / 21, xp / 40],
            )
        })?;
    }
    // XP at the start of the week and a few level-ups, for the overview.
    let now = bot_core::now_ms();
    ctx.db.with(|c| {
        c.execute(
            "INSERT OR REPLACE INTO level_week (guild_id, user_id, xp)
             SELECT guild_id, user_id, xp - (abs(random()) % 4000) - 400 FROM levels WHERE guild_id = ?1",
            [guild as i64],
        )?;
        c.execute("INSERT OR REPLACE INTO level_week_state (guild_id, week) VALUES (?1, ?2)", params![guild as i64, week_key()])
    })?;
    let regulars = ctx.roles(guild).into_iter().find(|r| r.1 == "Regulars").map(|r| r.0 as i64);
    for (i, level, role, back) in [(0usize, 25, regulars, 720_000i64), (3, 12, None, 9_000_000), (5, 8, None, 30_000_000)] {
        ctx.db.with(|c| {
            c.execute(
                "INSERT INTO level_ups (guild_id, user_id, level, role_id, at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![guild as i64, bot_core::preview::MEMBERS[i].0 as i64, level, role, now - back],
            )
        })?;
    }
    let roles = ctx.roles(guild);
    if let (Some(a), Some(b)) = (roles.get(2), roles.get(1)) {
        ctx.db.with(|c| {
            c.execute("INSERT OR REPLACE INTO level_rewards (guild_id, level, role_id) VALUES (?1, 10, ?2)", params![guild as i64, a.0 as i64])?;
            c.execute("INSERT OR REPLACE INTO level_rewards (guild_id, level, role_id) VALUES (?1, 25, ?2)", params![guild as i64, b.0 as i64])
        })?;
    }
    Ok(())
}
