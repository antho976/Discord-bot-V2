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
use bot_core::twilight_util::builder::command::{CommandBuilder, IntegerBuilder, StringBuilder, SubCommandBuilder, UserBuilder};
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
    /// Text and voice channels that earn no XP, and the threads in them.
    pub no_xp_channels: Vec<u64>,
    /// Categories whose channels and threads earn no XP.
    pub no_xp_categories: Vec<u64>,
    pub no_xp_roles: Vec<u64>,
    /// Post the week's top three when the week ends.
    pub weekly_post: bool,
    pub weekly_channel: Option<u64>,
    /// The level members can prestige from with `/prestige`; 0 turns prestige off.
    pub prestige_level: u32,
    /// Extra XP per prestige, in percent, on XP members earn.
    pub prestige_bonus: i64,
    /// Extra XP, in percent, for each day in a row a member earned XP after the first. 0 is off.
    pub streak_bonus: i64,
    /// The most the streak bonus grows to, in percent.
    pub streak_max: i64,
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
            no_xp_categories: Vec::new(),
            no_xp_roles: Vec::new(),
            weekly_post: false,
            weekly_channel: None,
            prestige_level: 0,
            prestige_bonus: 5,
            streak_bonus: 0,
            streak_max: 20,
        }
    }
}

pub const DEFAULT_MESSAGE: &str = "{user} reached level {level}.";

impl Settings {
    pub fn progress(&self, xp: i64) -> curve::Progress {
        curve::progress(xp, self.curve_base, self.curve_factor, &self.custom_curve)
    }

    pub fn prestige_on(&self) -> bool {
        self.prestige_level > 0
    }

    /// The extra XP, in percent, a member with `prestige` gets on what they earn.
    pub fn prestige_pct(&self, prestige: i64) -> i64 {
        if self.prestige_on() { prestige.max(0).saturating_mul(self.prestige_bonus.max(0)) } else { 0 }
    }

    /// The extra XP, in percent, a streak of `streak` days earns: nothing on the first day, then
    /// `streak_bonus` for each day after it, up to `streak_max`.
    /// Streaks show only while they earn something, like prestige, so a server that never set a
    /// bonus doesn't see them on /rank and the boards.
    pub fn streaks_on(&self) -> bool {
        self.streak_bonus > 0 && self.streak_max > 0
    }

    pub fn streak_pct(&self, streak: i64) -> i64 {
        (streak - 1).max(0).saturating_mul(self.streak_bonus.max(0)).min(self.streak_max.max(0))
    }

    /// Whether `channel` earns no XP: it's listed, it's a thread in a listed channel
    /// (`thread_parent`), or it sits in a listed category.
    fn no_xp_in(&self, channel: u64, thread_parent: Option<u64>, category: Option<u64>) -> bool {
        self.no_xp_channels.contains(&channel)
            || thread_parent.is_some_and(|p| self.no_xp_channels.contains(&p))
            || category.is_some_and(|c| self.no_xp_categories.contains(&c))
    }

    /// How the leaderboard sorts: prestige first when it's on.
    fn order(&self) -> &'static str {
        if self.prestige_on() { "prestige DESC, xp DESC, user_id" } else { "xp DESC, user_id" }
    }
}

/// Whether `channel` earns no XP. Looks up the channel's thread parent and category in the cache
/// only when the server has listed channels or categories.
fn no_xp_here(ctx: &Ctx, s: &Settings, channel: u64) -> bool {
    if s.no_xp_channels.is_empty() && s.no_xp_categories.is_empty() {
        return false;
    }
    if s.no_xp_channels.contains(&channel) {
        return true;
    }
    let parent_of = |id: u64| ctx.cache.channel(Id::new(id)).map(|c| (c.kind.is_thread(), c.parent_id.map(|p| p.get())));
    let Some((thread, parent)) = parent_of(channel) else { return false };
    let (thread_parent, category) = if thread { (parent, parent.and_then(parent_of).and_then(|p| p.1)) } else { (None, parent) };
    s.no_xp_in(channel, thread_parent, category)
}

/// Days since 1970 in UTC: streaks count these.
fn day_of(ms: i64) -> i64 {
    ms.div_euclid(86_400_000)
}

/// A streak after earning XP on `today`, given the streak and the day it last grew: one more the
/// day after, the same later that day, and back to 1 after a day without XP.
fn next_streak(streak: i64, day: i64, today: i64) -> i64 {
    if day == today {
        streak.max(1)
    } else if day == today - 1 {
        streak + 1
    } else {
        1
    }
}

/// A streak as it stands on `today`. It's still alive the day after it last grew, since the member
/// can carry it on until midnight UTC.
fn live_streak(streak: i64, day: i64, today: i64) -> i64 {
    if day >= today - 1 { streak } else { 0 }
}

/// `amount` with a bonus in percent, rounded to the nearest XP.
fn boosted(amount: i64, pct: i64) -> i64 {
    if pct <= 0 || amount <= 0 {
        return amount;
    }
    amount.saturating_mul(100 + pct).saturating_add(50) / 100
}

#[derive(Debug, Clone)]
pub struct Member {
    pub user_id: u64,
    pub xp: i64,
    pub level: u32,
    pub messages: i64,
    pub voice_minutes: i64,
    pub prestige: i64,
    /// Days in a row with XP, counting today or yesterday; 0 once it has lapsed.
    pub streak: i64,
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
}, Migration {
    name: "003_levels_week_winners",
    sql: "
        CREATE TABLE level_week_winners (
            guild_id INTEGER NOT NULL, week TEXT NOT NULL, place INTEGER NOT NULL,
            user_id INTEGER NOT NULL, xp INTEGER NOT NULL, PRIMARY KEY (guild_id, place));
    ",
}, Migration {
    name: "004_levels_streaks",
    sql: "
        ALTER TABLE levels ADD COLUMN streak INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE levels ADD COLUMN streak_day INTEGER NOT NULL DEFAULT 0;
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
            nav: vec![NavItem { label: "Levels", path: "levels", icon: Icon::Trophy, about: "XP from messages, voice and reactions, and the roles it earns." }],
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
            CommandBuilder::new("leaderboard", "The server's top members by XP", CommandType::ChatInput)
                .option(StringBuilder::new("board", "All time, or this week's XP").choices([("All time", "all"), ("This week", "week")]))
                .build(),
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

    /// `/prestige` only where prestige is on, so members elsewhere don't see a command that
    /// would only say it's off.
    fn guild_commands(&self, ctx: &Ctx, guild: u64) -> Vec<Command> {
        if ctx.settings::<Settings>(guild, KEY).is_ok_and(|s| s.prestige_on()) {
            vec![CommandBuilder::new("prestige", "Start over at level 0 for a prestige rank and more XP", CommandType::ChatInput).build()]
        } else {
            Vec::new()
        }
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
                if no_xp_here(ctx, &s, m.channel_id.get()) {
                    return Ok(());
                }
                if let Some(member) = &m.member
                    && member.roles.iter().any(|r| s.no_xp_roles.contains(&r.get())) {
                        return Ok(());
                    }
                if !cooled_down(&self.cooldowns, (guild, m.author.id.get()), s.cooldown_secs) {
                    return Ok(());
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
                if s.reaction_xp <= 0 || no_xp_here(ctx, &s, r.channel_id.get()) {
                    return Ok(());
                }
                if r.member.as_ref().is_some_and(|m| m.roles.iter().any(|role| s.no_xp_roles.contains(&role.get()))) {
                    return Ok(());
                }
                if !cooled_down(&self.reaction_cooldowns, (guild, r.user_id.get()), s.cooldown_secs) {
                    return Ok(());
                }
                grant(ctx, guild, r.user_id.get(), s.reaction_xp, Earned::Reaction, None).await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn tick(&self, ctx: &Ctx) -> Result<()> {
        for guild in ctx.enabled_guilds(KEY) {
            let winners = roll_week(ctx, guild)?;
            if !winners.is_empty() {
                post_winners(ctx, guild, &winners).await;
            }
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
                .map(|v| {
                    let bot = ctx.cache.user(v.user_id()).is_some_and(|u| u.bot);
                    (v.user_id().get(), v.channel_id(), v.self_mute() || v.self_deaf() || v.mute() || v.deaf(), bot)
                })
                .collect();
            for (user, channel, quiet, bot) in &states {
                if *bot || Some(*channel) == afk || (s.voice_require_active && *quiet) || no_xp_here(ctx, &s, channel.get()) {
                    continue;
                }
                let no_xp_role = ctx
                    .cache
                    .member(Id::new(guild), Id::new(*user))
                    .is_some_and(|m| m.roles().iter().any(|r| s.no_xp_roles.contains(&r.get())));
                if no_xp_role {
                    continue;
                }
                // Alone in a channel earns nothing, and a music bot is no company.
                let humans = states.iter().filter(|(_, c, _, b)| c == channel && !b).count();
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
                let m = member(ctx, guild, user)?.unwrap_or(Member { user_id: user, xp: 0, level: 0, messages: 0, voice_minutes: 0, prestige: 0, streak: 0 });
                let p = s.progress(m.xp);
                let place = rank_of(ctx, guild, user)?;
                let name = ctx.display_name(guild, user);
                let mut embed = EmbedBuilder::new()
                    .title(format!("{name}, level {}", p.level))
                    .color(0xf0806a)
                    .description(format!("`{}`  {} / {} XP", curve::bar(p.into, p.needed, 18), text::num(p.into), text::num(p.needed)))
                    .field(EmbedFieldBuilder::new("Rank", place.map(|r| format!("#{r}")).unwrap_or_else(|| "Unranked".into())).inline())
                    .field(EmbedFieldBuilder::new("Total XP", text::num(m.xp)).inline());
                if s.prestige_on() {
                    let bonus = s.prestige_pct(m.prestige);
                    let value = if bonus > 0 { format!("{}, +{bonus}% XP", m.prestige) } else { m.prestige.to_string() };
                    embed = embed.field(EmbedFieldBuilder::new("Prestige", value).inline());
                }
                if s.streaks_on() {
                    let streak = match (m.streak, s.streak_pct(m.streak)) {
                        (0, _) => "None".to_owned(),
                        (1, _) => "1 day".to_owned(),
                        (n, 0) => format!("{n} days"),
                        (n, pct) => format!("{n} days, +{pct}% XP"),
                    };
                    embed = embed.field(EmbedFieldBuilder::new("Streak", streak).inline());
                }
                let embed = embed
                    .field(EmbedFieldBuilder::new("Messages", text::num(m.messages)).inline())
                    .field(EmbedFieldBuilder::new("Voice", text::duration(m.voice_minutes * 60)).inline())
                    .build();
                ctx.reply(&call.interaction, Reply::embed(embed)).await?;
            }
            "leaderboard" => {
                let board = if call.opts().str("board") == Some("week") { Board::Week } else { Board::All };
                let (embed, rows) = leaderboard(ctx, guild, board, 0)?;
                ctx.reply(&call.interaction, Reply::embed(embed).components(rows)).await?;
            }
            "prestige" => {
                let reply = match prestige_offer(&s, member(ctx, guild, call.user_id())?.as_ref()) {
                    Ok(text) => {
                        let id = format!("{KEY}:prestige:{}", call.user_id());
                        Reply::text(text).components(vec![button_row(&[(&id, "Prestige", ButtonStyle::Danger)])])
                    }
                    Err(why) => Reply::text(why),
                };
                ctx.reply(&call.interaction, reply.ephemeral()).await?;
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
                        grant(ctx, guild, user, -n, Earned::Manual, None).await?;
                        format!("Took {} XP from {name}.", text::num(n))
                    }
                    "set-level" => {
                        let level = opts.int("level").unwrap_or(0).max(0) as u32;
                        let xp = curve::xp_to_reach(level, s.curve_base, s.curve_factor, &s.custom_curve);
                        if xp > curve::MAX_XP {
                            format!("Level {level} takes more XP than a member can hold with this curve. Pick a lower level.")
                        } else {
                            set_xp(ctx, guild, user, xp).await?;
                            format!("{name} is now level {level}.")
                        }
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
        if let [which @ ("lb" | "lbw"), page] = call.args().as_slice() {
            let page: i64 = page.parse().unwrap_or(0);
            let board = if *which == "lbw" { Board::Week } else { Board::All };
            let (embed, rows) = leaderboard(ctx, guild, board, page.max(0))?;
            ctx.update(&call.interaction, Reply::embed(embed).components(rows)).await?;
        }
        if let ["prestige", user] = call.args().as_slice() {
            // The offer is only shown to the member it's for, but check anyway.
            if user.parse::<u64>().ok() != Some(call.user_id()) {
                return Ok(());
            }
            let s = ctx.settings::<Settings>(guild, KEY)?;
            let text = match prestige(ctx, guild, call.user_id(), s.prestige_level)? {
                Some(n) => {
                    sync_rewards(ctx, guild, call.user_id(), 0).await;
                    let bonus = if s.prestige_bonus > 0 { format!(" You earn {}% more XP from here.", s.prestige_pct(n)) } else { String::new() };
                    format!("You're prestige {n} now, back at level 0.{bonus}")
                }
                None => prestige_offer(&s, member(ctx, guild, call.user_id())?.as_ref()).err().unwrap_or_else(|| "Nothing changed. Try /prestige again.".into()),
            };
            ctx.update(&call.interaction, Reply::text(text)).await?;
        }
        Ok(())
    }
}

/// What prestiging would do for this member, or why they can't yet.
fn prestige_offer(s: &Settings, m: Option<&Member>) -> std::result::Result<String, String> {
    if !s.prestige_on() {
        return Err("Prestige is off on this server.".into());
    }
    let (level, now) = m.map(|m| (m.level, m.prestige)).unwrap_or((0, 0));
    if level < s.prestige_level {
        return Err(format!("You can prestige from level {}. You're level {level}.", s.prestige_level));
    }
    let bonus = if s.prestige_bonus > 0 { format!(", and you earn {}% more XP", s.prestige_pct(now + 1)) } else { String::new() };
    Ok(format!(
        "Prestige {} puts your XP and level back to 0 and takes away your level reward roles until you earn them again. Your prestige shows on /rank and the leaderboard{bonus}. This can't be undone.",
        now + 1
    ))
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
            &format!("SELECT {COLS} FROM levels WHERE guild_id = ?1 AND user_id = ?2"),
            params![guild as i64, user as i64],
            member_row,
        )
        .optional()
    })
}

/// The columns `member_row` reads.
const COLS: &str = "user_id, xp, level, messages, voice_minutes, prestige, streak, streak_day";

/// Who is on the leaderboard: anyone with XP, and anyone who prestiged and is back at 0.
const ON_BOARD: &str = "(xp > 0 OR prestige > 0)";

fn rank_of(ctx: &Ctx, guild: u64, user: u64) -> Result<Option<i64>> {
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let ahead = if s.prestige_on() { "o.prestige > l.prestige OR (o.prestige = l.prestige AND o.xp > l.xp)" } else { "o.xp > l.xp" };
    ctx.db.with(|c| {
        c.query_row(
            &format!(
                "SELECT (SELECT COUNT(*) FROM levels o WHERE o.guild_id = l.guild_id AND ({ahead})) + 1
                 FROM levels l WHERE l.guild_id = ?1 AND l.user_id = ?2 AND (l.xp > 0 OR l.prestige > 0)"
            ),
            params![guild as i64, user as i64],
            |r| r.get(0),
        )
        .optional()
    })
}

fn member_row(r: &bot_core::rusqlite::Row<'_>) -> bot_core::rusqlite::Result<Member> {
    Ok(Member {
        user_id: r.get::<_, i64>(0)? as u64,
        xp: r.get(1)?,
        level: r.get::<_, i64>(2)? as u32,
        messages: r.get(3)?,
        voice_minutes: r.get(4)?,
        prestige: r.get(5)?,
        streak: live_streak(r.get(6)?, r.get(7)?, day_of(bot_core::now_ms())),
    })
}

pub fn top(ctx: &Ctx, guild: u64, offset: i64, limit: i64) -> Result<Vec<Member>> {
    let s = ctx.settings::<Settings>(guild, KEY)?;
    ctx.db.with(|c| {
        let mut stmt = c.prepare_cached(&format!("SELECT {COLS} FROM levels WHERE guild_id = ?1 AND {ON_BOARD} ORDER BY {} LIMIT ?2 OFFSET ?3", s.order()))?;
        stmt.query_map(params![guild as i64, limit, offset], member_row)?.collect()
    })
}

/// The members among `ids` with XP, best first, each with their place on the leaderboard: up to
/// `limit` of them, and how many matched in all.
pub fn find(ctx: &Ctx, guild: u64, ids: &std::collections::HashSet<u64>, limit: usize) -> Result<(Vec<(i64, Member)>, i64)> {
    let s = ctx.settings::<Settings>(guild, KEY)?;
    ctx.db.with(|c| {
        let mut stmt = c.prepare_cached(&format!("SELECT {COLS} FROM levels WHERE guild_id = ?1 AND {ON_BOARD} ORDER BY {}", s.order()))?;
        let (mut out, mut matched) = (Vec::new(), 0);
        for (i, m) in stmt.query_map([guild as i64], member_row)?.enumerate() {
            let m = m?;
            if ids.contains(&m.user_id) {
                matched += 1;
                if out.len() < limit {
                    out.push((i as i64 + 1, m));
                }
            }
        }
        Ok((out, matched))
    })
}

pub fn count(ctx: &Ctx, guild: u64) -> Result<i64> {
    ctx.db.with(|c| c.query_row(&format!("SELECT COUNT(*) FROM levels WHERE guild_id = ?1 AND {ON_BOARD}"), [guild as i64], |r| r.get(0)))
}

/// Which leaderboard: total XP, or XP gained since Monday.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Board {
    All,
    Week,
}

/// XP gained this week: XP now less XP when the week started. Members who first earned XP this
/// week have no starting row and count from zero.
const WEEK_GAIN: &str = "SELECT l.user_id, l.xp, l.level, l.messages, l.voice_minutes, l.prestige, l.streak, l.streak_day, l.xp - COALESCE(w.xp, 0) AS gain FROM levels l
     LEFT JOIN level_week w ON w.guild_id = l.guild_id AND w.user_id = l.user_id
     WHERE l.guild_id = ?1 AND gain > 0";

/// This week's top members by XP gained, each with what they gained.
pub fn week_top(ctx: &Ctx, guild: u64, offset: i64, limit: i64) -> Result<Vec<(Member, i64)>> {
    ctx.db.with(|c| {
        let mut stmt = c.prepare_cached(&format!("{WEEK_GAIN} ORDER BY gain DESC, l.user_id LIMIT ?2 OFFSET ?3"))?;
        stmt.query_map(params![guild as i64, limit, offset], |r| Ok((member_row(r)?, r.get(8)?)))?.collect()
    })
}

/// How many members gained XP this week, and how much they gained together.
pub fn week_totals(ctx: &Ctx, guild: u64) -> Result<(i64, i64)> {
    ctx.db.with(|c| c.query_row(&format!("SELECT COUNT(*), COALESCE(SUM(gain), 0) FROM ({WEEK_GAIN})"), [guild as i64], |r| Ok((r.get(0)?, r.get(1)?))))
}

/// A finished week's top three: the week ("2026-W44") and `(user, XP gained)` best first.
pub type Winners = (String, Vec<(u64, i64)>);

/// Last week's top three, kept from when the week rolled over.
pub fn last_week(ctx: &Ctx, guild: u64) -> Result<Option<Winners>> {
    let rows: Vec<(String, u64, i64)> = ctx.db.with(|c| {
        let mut stmt = c.prepare_cached("SELECT week, user_id, xp FROM level_week_winners WHERE guild_id = ?1 ORDER BY place")?;
        stmt.query_map([guild as i64], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64, r.get(2)?)))?.collect()
    })?;
    let Some(week) = rows.first().map(|r| r.0.clone()) else { return Ok(None) };
    Ok(Some((week, rows.into_iter().map(|(_, u, x)| (u, x)).collect())))
}

fn leaderboard(ctx: &Ctx, guild: u64, board: Board, page: i64) -> Result<(bot_core::twilight_model::channel::message::Embed, Vec<bot_core::twilight_model::channel::message::component::Component>)> {
    let per = 10;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let total = match board {
        Board::All => count(ctx, guild)?,
        Board::Week => week_totals(ctx, guild)?.0,
    };
    let pages = ((total + per - 1) / per).max(1);
    let page = page.min(pages - 1);
    let rows: Vec<(Member, Option<i64>)> = match board {
        Board::All => top(ctx, guild, page * per, per)?.into_iter().map(|m| (m, None)).collect(),
        Board::Week => week_top(ctx, guild, page * per, per)?.into_iter().map(|(m, g)| (m, Some(g))).collect(),
    };
    let mut lines = Vec::new();
    for (i, (m, gain)) in rows.iter().enumerate() {
        let place = page * per + i as i64 + 1;
        let figure = match gain {
            Some(g) => format!("+{} XP", text::num(*g)),
            None => format!("{} XP", text::num(m.xp)),
        };
        let prestige = if s.prestige_on() && m.prestige > 0 { format!("prestige {}  ·  ", m.prestige) } else { String::new() };
        let streak = if s.streaks_on() && m.streak > 1 { format!("{}-day streak  ·  ", m.streak) } else { String::new() };
        lines.push(format!("`{place:>2}`  {}  ·  {prestige}level {}  ·  {streak}{figure}", ctx.display_name(guild, m.user_id), m.level));
    }
    let (title, nobody, prefix) = match board {
        Board::All => (format!("{} leaderboard", ctx.guild_name(guild)), "Nobody has XP yet.", "lb"),
        Board::Week => (format!("{} this week", ctx.guild_name(guild)), "Nobody has earned XP since Monday.", "lbw"),
    };
    let embed = EmbedBuilder::new()
        .title(title)
        .color(0xf0806a)
        .description(if lines.is_empty() { nobody.into() } else { lines.join("\n") })
        .footer(bot_core::twilight_util::builder::embed::EmbedFooterBuilder::new(format!("Page {} of {pages}", page + 1)))
        .build();
    let mut buttons = Vec::new();
    if pages > 1 {
        let prev = format!("{KEY}:{prefix}:{}", (page - 1).max(0));
        let next = format!("{KEY}:{prefix}:{}", (page + 1).min(pages - 1));
        buttons.push(button_row(&[(&prev, "Previous", ButtonStyle::Secondary), (&next, "Next", ButtonStyle::Secondary)]));
    }
    Ok((embed, buttons))
}

/// Whether `key` is past its cooldown; if so, the cooldown starts again now. Entries that have
/// run out are dropped once the map grows, so it stays small.
fn cooled_down(map: &Mutex<HashMap<(u64, u64), Instant>>, key: (u64, u64), cooldown_secs: i64) -> bool {
    let limit = cooldown_secs.max(0) as u64;
    let mut cd = map.lock().unwrap();
    if cd.get(&key).is_some_and(|t| t.elapsed().as_secs() < limit) {
        return false;
    }
    cd.insert(key, Instant::now());
    if cd.len() > 5000 {
        cd.retain(|_, t| t.elapsed().as_secs() < limit);
    }
    true
}

/// Add XP (or take it, when `amount` is negative) and handle a level change: announcement and
/// reward roles.
async fn grant(ctx: &Ctx, guild: u64, user: u64, amount: i64, how: Earned, channel: Option<u64>) -> Result<()> {
    let s = ctx.settings::<Settings>(guild, KEY)?;
    // Read and write under one lock: a message, a reaction and the voice tick for the same member
    // can land together, and each must add to what the other wrote.
    let now = bot_core::now_ms();
    let (before, level) = ctx.db.with(|c| {
        let (xp, level, prestige, streak, day): (i64, u32, i64, i64, i64) = c
            .query_row(
                "SELECT xp, level, prestige, streak, streak_day FROM levels WHERE guild_id = ?1 AND user_id = ?2",
                params![guild as i64, user as i64],
                |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u32, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?
            .unwrap_or((0, 0, 0, 0, 0));
        let before = (xp, level);
        // Earning keeps the streak going, and prestige and the streak boost what members earn;
        // staff giving or taking XP touches neither.
        let (amount, streak, day) = if how == Earned::Manual {
            (amount, streak, day)
        } else {
            let today = day_of(now);
            let streak = next_streak(streak, day, today);
            (boosted(amount, s.prestige_pct(prestige).saturating_add(s.streak_pct(streak))), streak, today)
        };
        let xp = before.0.saturating_add(amount).clamp(0, curve::MAX_XP);
        let level = s.progress(xp).level;
        c.execute(
            "INSERT INTO levels (guild_id, user_id, xp, level, messages, voice_minutes, updated_at, streak, streak_day)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT (guild_id, user_id) DO UPDATE SET xp = excluded.xp, level = excluded.level,
               messages = messages + ?5, voice_minutes = voice_minutes + ?6, updated_at = excluded.updated_at,
               streak = excluded.streak, streak_day = excluded.streak_day",
            params![
                guild as i64,
                user as i64,
                xp,
                level as i64,
                (how == Earned::Message) as i64,
                (how == Earned::Voice) as i64,
                now,
                streak,
                day
            ],
        )?;
        Ok((before, level))
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
    week_key_of(bot_core::jiff::Timestamp::now().to_zoned(bot_core::jiff::tz::TimeZone::UTC).date())
}

/// The ISO week a day falls in. Weeks start on Monday, and the days around New Year can belong
/// to the other year's first or last week.
fn week_key_of(d: bot_core::jiff::civil::Date) -> String {
    let w = d.iso_week_date();
    format!("{}-W{:02}", w.year(), w.week())
}

/// At the start of each week, keep the top three of the week that ended, remember everyone's XP
/// so "this week" can be worked out, and drop level-ups older than a month. Returns the top three
/// when a week just ended, so they can be posted.
fn roll_week(ctx: &Ctx, guild: u64) -> Result<Vec<(u64, i64)>> {
    roll_week_to(ctx, guild, &week_key())
}

/// The top three kept for the stored week are measured from that week's start to now, so after
/// the bot was down for a whole week they also hold the missed week's XP.
fn roll_week_to(ctx: &Ctx, guild: u64, week: &str) -> Result<Vec<(u64, i64)>> {
    let mut conn = ctx.db.lock();
    let stored: Option<String> =
        conn.query_row("SELECT week FROM level_week_state WHERE guild_id = ?1", [guild as i64], |r| r.get(0)).optional()?;
    if stored.as_deref() == Some(week) {
        return Ok(Vec::new());
    }
    let tx = conn.transaction()?;
    // The first roll has no start of week to measure from, so it names no winners.
    let mut winners = Vec::new();
    if let Some(ended) = &stored {
        {
            let mut st = tx.prepare(&format!("{WEEK_GAIN} ORDER BY gain DESC, l.user_id LIMIT 3"))?;
            for row in st.query_map([guild as i64], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(8)?)))? {
                winners.push(row?);
            }
        }
        tx.execute("DELETE FROM level_week_winners WHERE guild_id = ?1", [guild as i64])?;
        for (place, (user, gain)) in winners.iter().enumerate() {
            tx.execute(
                "INSERT INTO level_week_winners (guild_id, week, place, user_id, xp) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![guild as i64, ended, place as i64 + 1, *user as i64, gain],
            )?;
        }
    }
    tx.execute("DELETE FROM level_week WHERE guild_id = ?1", [guild as i64])?;
    tx.execute("INSERT INTO level_week (guild_id, user_id, xp) SELECT guild_id, user_id, xp FROM levels WHERE guild_id = ?1", [guild as i64])?;
    tx.execute(
        "INSERT INTO level_week_state (guild_id, week) VALUES (?1, ?2) ON CONFLICT (guild_id) DO UPDATE SET week = excluded.week",
        params![guild as i64, week],
    )?;
    tx.execute("DELETE FROM level_ups WHERE guild_id = ?1 AND at < ?2", params![guild as i64, bot_core::now_ms() - 30 * 86_400_000])?;
    tx.commit()?;
    Ok(winners)
}

/// The week's top three as the weekly post lists them.
pub fn winners_text(winners: &[(u64, i64)]) -> String {
    winners
        .iter()
        .enumerate()
        .map(|(i, (user, gain))| format!("**{}.** <@{user}>  ·  +{} XP", i + 1, text::num(*gain)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Post the top three of the week that ended, when the server asked for it. Mentions in an embed
/// show the name without pinging anyone.
async fn post_winners(ctx: &Ctx, guild: u64, winners: &[(u64, i64)]) {
    let Ok(s) = ctx.settings::<Settings>(guild, KEY) else { return };
    let Some(channel) = s.weekly_channel.filter(|_| s.weekly_post) else { return };
    let embed = EmbedBuilder::new().title("Most XP last week").color(0xf0806a).description(winners_text(winners)).build();
    if let Err(e) = ctx.http.create_message(Id::new(channel)).embeds(&[embed]).allowed_mentions(Some(&AllowedMentions::default())).await {
        bot_core::tracing::warn!(guild, channel, error = %e, "couldn't post the weekly top three");
        notify_failure(ctx, guild, format!("The weekly top three wasn't posted. {}", why_refused(&e)), format!("/g/{guild}/{KEY}/settings"));
    }
}

/// Set a member's XP outright and fix their level and roles.
pub async fn set_xp(ctx: &Ctx, guild: u64, user: u64, xp: i64) -> Result<()> {
    let xp = xp.clamp(0, curve::MAX_XP);
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let level = s.progress(xp).level;
    {
        let mut conn = ctx.db.lock();
        let tx = conn.transaction()?;
        let old: i64 = tx
            .query_row("SELECT xp FROM levels WHERE guild_id = ?1 AND user_id = ?2", params![guild as i64, user as i64], |r| r.get(0))
            .optional()?
            .unwrap_or(0);
        tx.execute(
            "INSERT INTO levels (guild_id, user_id, xp, level, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (guild_id, user_id) DO UPDATE SET xp = excluded.xp, level = excluded.level, updated_at = excluded.updated_at",
            params![guild as i64, user as i64, xp, level as i64, bot_core::now_ms()],
        )?;
        shift_week(&tx, guild, user, xp - old)?;
        tx.commit()?;
    }
    sync_rewards(ctx, guild, user, level).await;
    Ok(())
}

/// Move a member's XP at the start of the week by `delta` alongside a staff change or an import,
/// so XP that wasn't earned this week doesn't count as earned this week. A member with no row
/// counts from zero, so the new row is the change itself.
pub(crate) fn shift_week(c: &bot_core::rusqlite::Connection, guild: u64, user: u64, delta: i64) -> bot_core::rusqlite::Result<()> {
    if delta != 0 {
        c.execute(
            "INSERT INTO level_week (guild_id, user_id, xp) VALUES (?1, ?2, ?3) ON CONFLICT (guild_id, user_id) DO UPDATE SET xp = xp + ?3",
            params![guild as i64, user as i64, delta],
        )?;
    }
    Ok(())
}

/// Start a member over at level 0 with one more prestige, if they're still at `from_level` or
/// higher, and return their new prestige. Their XP at the start of the week drops by the XP they
/// give up, so what they gained this week stays the same.
fn prestige(ctx: &Ctx, guild: u64, user: u64, from_level: u32) -> Result<Option<i64>> {
    if from_level == 0 {
        return Ok(None);
    }
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    let row: Option<(i64, i64, i64)> = tx
        .query_row("SELECT xp, level, prestige FROM levels WHERE guild_id = ?1 AND user_id = ?2", params![guild as i64, user as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    let Some((xp, _, prestige)) = row.filter(|r| r.1 >= from_level as i64) else { return Ok(None) };
    tx.execute(
        "UPDATE levels SET xp = 0, level = 0, prestige = prestige + 1, updated_at = ?3 WHERE guild_id = ?1 AND user_id = ?2",
        params![guild as i64, user as i64, bot_core::now_ms()],
    )?;
    tx.execute(
        "INSERT INTO level_week (guild_id, user_id, xp) VALUES (?1, ?2, -?3) ON CONFLICT (guild_id, user_id) DO UPDATE SET xp = xp - ?3",
        params![guild as i64, user as i64, xp],
    )?;
    tx.commit()?;
    Ok(Some(prestige + 1))
}

/// Set a member's prestige outright, from the dashboard.
pub fn set_prestige(ctx: &Ctx, guild: u64, user: u64, prestige: i64) -> Result<()> {
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO levels (guild_id, user_id, prestige, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (guild_id, user_id) DO UPDATE SET prestige = excluded.prestige, updated_at = excluded.updated_at",
            params![guild as i64, user as i64, prestige.max(0), bot_core::now_ms()],
        )
    })?;
    Ok(())
}

pub fn rewards(ctx: &Ctx, guild: u64) -> Result<Vec<(u32, u64)>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached("SELECT level, role_id FROM level_rewards WHERE guild_id = ?1 ORDER BY level")?;
        
        s.query_map([guild as i64], |r| Ok((r.get::<_, i64>(0)? as u32, r.get::<_, i64>(1)? as u64)))?.collect()
    })
}

/// After a reward is added or changed, bring members at or above `from_level` in line now
/// instead of at their next level change: they get the new role, lose one they no longer
/// qualify for, and lose `dropped` (the role the reward gave before) unless another reward still
/// gives it. Only members the cache holds are touched, since only their roles are known; the rest
/// catch up when their level next changes. Runs in the background, paced by Discord's limits.
pub fn resync_rewards(ctx: &Ctx, guild: u64, from_level: u32, dropped: Option<u64>) {
    let ctx = ctx.clone();
    bot_core::tokio::spawn(async move {
        let members: Vec<(u64, u32)> = ctx
            .db
            .with(|c| {
                let mut s = c.prepare("SELECT user_id, level FROM levels WHERE guild_id = ?1 AND level >= ?2")?;
                s.query_map(params![guild as i64, from_level as i64], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u32)))?.collect()
            })
            .unwrap_or_default();
        let still_given = rewards(&ctx, guild).unwrap_or_default();
        let dropped = dropped.filter(|r| !still_given.iter().any(|(_, x)| x == r));
        for (user, level) in members {
            let Some(has) = ctx.cache.member(Id::new(guild), Id::new(user)).map(|m| m.roles().iter().map(|r| r.get()).collect::<Vec<_>>()) else {
                continue;
            };
            if let Some(role) = dropped
                && has.contains(&role)
                && let Err(e) = ctx.http.remove_guild_member_role(Id::new(guild), Id::new(user), Id::new(role)).await
            {
                bot_core::tracing::warn!(guild, user, role, error = %e, "couldn't take back a changed reward role");
            }
            sync_rewards(&ctx, guild, user, level).await;
        }
    });
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
        // Only the chosen channel is worth a notice; a member's own channel may simply be closed to the bot.
        if s.announce_channel.is_some() {
            notify_failure(ctx, guild, format!("Level-ups aren't being posted. {}", why_refused(&e)), format!("/g/{guild}/{KEY}/settings"));
        }
    }
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


pub(crate) fn preview_rows(ctx: &Ctx, guild: u64) -> Result<()> {
    // The sample server has no voice channels; add a few so the no-XP picker has something.
    let music = guild + 2004;
    for (n, name, kind, parent) in [(1u64, "Voice", 4u8, None), (2, "Lounge", 2, Some(guild + 2001)), (3, "Gaming", 2, Some(guild + 2001)), (4, "Music", 2, Some(guild + 2001))] {
        let channel = bot_core::serde_json::json!({
            "id": (guild + 2000 + n).to_string(), "type": kind, "name": name, "position": 20 + n, "guild_id": guild.to_string(),
            "parent_id": parent.map(|p: u64| p.to_string()), "permission_overwrites": []
        });
        let channel = bot_core::serde_json::from_value(channel)?;
        ctx.cache.update(&Event::ChannelCreate(Box::new(bot_core::twilight_model::gateway::payload::incoming::ChannelCreate(channel))));
    }
    let s = Settings { prestige_level: 30, streak_bonus: 2, no_xp_channels: vec![music], ..Default::default() };
    ctx.save_settings(guild, KEY, &s)?;
    for (i, (user, _)) in bot_core::preview::MEMBERS.iter().enumerate() {
        let xp = 48_000 / (i as i64 + 1) + (i as i64 * 731) % 900;
        let prestige = match i { 1 => 2, 3 => 1, _ => 0 };
        // Some members on a streak today or yesterday, one whose streak lapsed.
        let (streak, back) = [(12, 0), (3, 0), (0, 0), (27, 1), (5, 0), (2, 1), (1, 0), (8, 4), (4, 0), (0, 0)][i];
        let day = day_of(bot_core::now_ms()) - back;
        ctx.db.with(|c| {
            c.execute(
                "INSERT OR REPLACE INTO levels (guild_id, user_id, xp, level, messages, voice_minutes, prestige, streak, streak_day, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0)",
                params![guild as i64, *user as i64, xp, s.progress(xp).level as i64, xp / 21, xp / 40, prestige, streak, day],
            )
        })?;
    }
    // XP at the start of the week, last week's top three, and a few level-ups, for the overview.
    let now = bot_core::now_ms();
    let last = week_key_of(bot_core::jiff::Timestamp::now().to_zoned(bot_core::jiff::tz::TimeZone::UTC).date().saturating_sub(bot_core::jiff::Span::new().days(7)));
    ctx.db.with(|c| {
        c.execute(
            "INSERT OR REPLACE INTO level_week (guild_id, user_id, xp)
             SELECT guild_id, user_id, xp - (abs(random()) % 4000) - 400 FROM levels WHERE guild_id = ?1",
            [guild as i64],
        )?;
        c.execute("INSERT OR REPLACE INTO level_week_state (guild_id, week) VALUES (?1, ?2)", params![guild as i64, week_key()])?;
        for (place, (i, gain)) in [(2usize, 5_120i64), (0, 4_380), (6, 3_910)].into_iter().enumerate() {
            c.execute(
                "INSERT OR REPLACE INTO level_week_winners (guild_id, week, place, user_id, xp) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![guild as i64, last, place as i64 + 1, bot_core::preview::MEMBERS[i].0 as i64, gain],
            )?;
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooldowns_hold_and_stay_small() {
        let map = Mutex::new(HashMap::new());
        assert!(cooled_down(&map, (1, 10), 60));
        assert!(!cooled_down(&map, (1, 10), 60));
        // Another member, or another server, has their own cooldown.
        assert!(cooled_down(&map, (1, 11), 60));
        assert!(cooled_down(&map, (2, 10), 60));
        // No cooldown: every message counts.
        assert!(cooled_down(&map, (3, 10), 0));
        assert!(cooled_down(&map, (3, 10), 0));
        // Past the size limit, finished cooldowns are dropped.
        for user in 0..5001 {
            cooled_down(&map, (4, user), 0);
        }
        assert!(map.lock().unwrap().len() < 10);
    }

    #[test]
    fn weeks_follow_iso_numbering() {
        use bot_core::jiff::civil::date;
        assert_eq!(week_key_of(date(2026, 10, 31)), "2026-W44");
        // Monday starts the week.
        assert_eq!(week_key_of(date(2026, 11, 1)), "2026-W44");
        assert_eq!(week_key_of(date(2026, 11, 2)), "2026-W45");
        // New Year: 2026 starts on a Thursday, so it has a week 53 running into 2027, and the last
        // days of 2024 already belong to 2025.
        assert_eq!(week_key_of(date(2026, 1, 1)), "2026-W01");
        assert_eq!(week_key_of(date(2027, 1, 1)), "2026-W53");
        assert_eq!(week_key_of(date(2024, 12, 30)), "2025-W01");
    }

    fn ctx() -> Ctx {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.install_features(vec![std::sync::Arc::new(Levels::default())]).unwrap();
        ctx
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn prestige_is_a_command_only_where_it_is_on() {
        let ctx = ctx();
        let names = |ctx: &Ctx| Levels::default().guild_commands(ctx, 1).into_iter().map(|c| c.name).collect::<Vec<_>>();
        assert!(names(&ctx).is_empty());
        assert!(!Levels::default().commands().iter().any(|c| c.name == "prestige"));
        ctx.save_settings(1, KEY, &Settings { prestige_level: 50, ..Settings::default() }).unwrap();
        assert_eq!(names(&ctx), vec!["prestige"]);
        assert!(Levels::default().guild_commands(&ctx, 2).is_empty());
    }

    fn set(ctx: &Ctx, user: u64, xp: i64) {
        ctx.db
            .with(|c| c.execute("INSERT OR REPLACE INTO levels (guild_id, user_id, xp) VALUES (1, ?1, ?2)", params![user as i64, xp]))
            .unwrap();
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn the_week_rolls_over_and_keeps_its_top_three() {
        let ctx = ctx();
        for (user, xp) in [(10, 500), (11, 900), (12, 100), (13, 50)] {
            set(&ctx, user, xp);
        }
        // The first roll only marks the start: nothing to measure the week before against.
        assert!(roll_week_to(&ctx, 1, "2026-W44").unwrap().is_empty());
        assert_eq!(crate::week_totals(&ctx, 1).unwrap(), (0, 0));
        // During the week: gains count from the start, and a newcomer counts from zero.
        for (user, xp) in [(10, 800), (11, 950), (12, 600), (13, 60), (14, 70)] {
            set(&ctx, user, xp);
        }
        let top: Vec<(u64, i64)> = crate::week_top(&ctx, 1, 0, 10).unwrap().into_iter().map(|(m, g)| (m.user_id, g)).collect();
        assert_eq!(top, vec![(12, 500), (10, 300), (14, 70), (11, 50), (13, 10)]);
        assert_eq!(crate::week_totals(&ctx, 1).unwrap(), (5, 930));
        // Rolling twice in the same week does nothing.
        assert!(roll_week_to(&ctx, 1, "2026-W44").unwrap().is_empty());
        // The next week: the top three are kept under the week that ended, and the board is empty.
        assert_eq!(roll_week_to(&ctx, 1, "2026-W45").unwrap(), vec![(12, 500), (10, 300), (14, 70)]);
        assert_eq!(crate::last_week(&ctx, 1).unwrap(), Some(("2026-W44".into(), vec![(12, 500), (10, 300), (14, 70)])));
        assert_eq!(crate::week_totals(&ctx, 1).unwrap(), (0, 0));
        // A quiet week names nobody, and clears the last top three.
        assert!(roll_week_to(&ctx, 1, "2026-W46").unwrap().is_empty());
        assert_eq!(crate::last_week(&ctx, 1).unwrap(), None);
    }

    #[test]
    fn bonuses_round_and_only_add() {
        assert_eq!(boosted(20, 0), 20);
        assert_eq!(boosted(15, 5), 16);
        assert_eq!(boosted(20, 50), 30);
        // Taking XP away is never boosted.
        assert_eq!(boosted(-20, 50), -20);
        assert_eq!(boosted(i64::MAX, 50), i64::MAX / 100);
        let mut s = Settings::default();
        assert_eq!(s.prestige_pct(3), 0, "prestige is off by default");
        s.prestige_level = 30;
        assert_eq!(s.prestige_pct(3), 15);
    }

    #[test]
    fn prestige_offer_says_why_not() {
        let mut s = Settings::default();
        let m = |level, prestige| Member { user_id: 1, xp: 0, level, messages: 0, voice_minutes: 0, prestige, streak: 0 };
        assert_eq!(prestige_offer(&s, Some(&m(40, 0))).unwrap_err(), "Prestige is off on this server.");
        s.prestige_level = 30;
        assert_eq!(prestige_offer(&s, Some(&m(12, 0))).unwrap_err(), "You can prestige from level 30. You're level 12.");
        assert_eq!(prestige_offer(&s, None).unwrap_err(), "You can prestige from level 30. You're level 0.");
        let offer = prestige_offer(&s, Some(&m(30, 1))).unwrap();
        assert!(offer.starts_with("Prestige 2 puts"), "{offer}");
        assert!(offer.contains("10% more XP"), "{offer}");
    }

    fn set_level(ctx: &Ctx, user: u64, xp: i64, level: i64, prestige: i64) {
        ctx.db
            .with(|c| {
                c.execute(
                    "INSERT OR REPLACE INTO levels (guild_id, user_id, xp, level, prestige) VALUES (1, ?1, ?2, ?3, ?4)",
                    params![user as i64, xp, level, prestige],
                )
            })
            .unwrap();
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn prestige_starts_over_and_keeps_the_week() {
        let ctx = ctx();
        set_level(&ctx, 10, 4_000, 18, 0);
        roll_week_to(&ctx, 1, "2026-W44").unwrap();
        set_level(&ctx, 10, 5_000, 20, 0);
        // 11 first earned XP this week, so has no start-of-week row.
        set_level(&ctx, 11, 3_000, 15, 0);
        set_level(&ctx, 12, 900, 6, 0);
        // Off, or below the level: nothing happens.
        assert_eq!(prestige(&ctx, 1, 10, 0).unwrap(), None);
        assert_eq!(prestige(&ctx, 1, 12, 10).unwrap(), None);
        assert_eq!(prestige(&ctx, 1, 10, 10).unwrap(), Some(1));
        assert_eq!(prestige(&ctx, 1, 11, 10).unwrap(), Some(1));
        // A second press finds them at level 0.
        assert_eq!(prestige(&ctx, 1, 10, 10).unwrap(), None);
        let m = member(&ctx, 1, 10).unwrap().unwrap();
        assert_eq!((m.xp, m.level, m.prestige), (0, 0, 1));
        // This week's gains are what they were before.
        let week: Vec<(u64, i64)> = crate::week_top(&ctx, 1, 0, 10).unwrap().into_iter().map(|(m, g)| (m.user_id, g)).collect();
        assert_eq!(week, vec![(11, 3_000), (10, 1_000), (12, 900)]);
        // With prestige on, prestige ranks first; off, XP alone. Both stay on the board at 0 XP.
        let ids = |ctx: &Ctx| top(ctx, 1, 0, 10).unwrap().into_iter().map(|m| m.user_id).collect::<Vec<_>>();
        assert_eq!(ids(&ctx), vec![12, 10, 11]);
        assert_eq!(rank_of(&ctx, 1, 12).unwrap(), Some(1));
        ctx.save_settings(1, KEY, &Settings { prestige_level: 10, ..Default::default() }).unwrap();
        assert_eq!(ids(&ctx), vec![10, 11, 12]);
        assert_eq!(rank_of(&ctx, 1, 12).unwrap(), Some(3));
        assert_eq!(count(&ctx, 1).unwrap(), 3);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn staff_changes_and_imports_stay_off_the_weekly_board() {
        let ctx = ctx();
        set_level(&ctx, 10, 4_000, 18, 0);
        roll_week_to(&ctx, 1, "2026-W44").unwrap();
        set_level(&ctx, 10, 4_500, 18, 0);
        // Staff set 10 to 9,000 and 11 (no XP before) to 2,000, as Set XP and an import do.
        let shift = |user, delta| ctx.db.with(|c| shift_week(c, 1, user, delta)).unwrap();
        set_level(&ctx, 10, 9_000, 30, 0);
        shift(10, 4_500);
        set_level(&ctx, 11, 2_000, 12, 0);
        shift(11, 2_000);
        let week: Vec<(u64, i64)> = crate::week_top(&ctx, 1, 0, 10).unwrap().into_iter().map(|(m, g)| (m.user_id, g)).collect();
        assert_eq!(week.iter().find(|w| w.0 == 10), Some(&(10, 500)));
        assert!(week.iter().all(|w| w.0 != 11 || w.1 == 0));
    }

    #[test]
    fn no_xp_covers_threads_and_categories() {
        let s = Settings { no_xp_channels: vec![10], no_xp_categories: vec![90], ..Default::default() };
        assert!(s.no_xp_in(10, None, None));
        assert!(!s.no_xp_in(11, None, Some(80)));
        // A thread in a listed channel, and a channel or thread under a listed category.
        assert!(s.no_xp_in(500, Some(10), Some(80)));
        assert!(s.no_xp_in(11, None, Some(90)));
        assert!(s.no_xp_in(501, Some(11), Some(90)));
        assert!(!s.no_xp_in(502, Some(11), Some(80)));
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn no_xp_finds_a_thread_parent_and_category_in_the_cache() {
        let ctx = ctx();
        for (id, kind, parent) in [(90u64, 4u8, None), (91, 4, None), (10, 0, Some(90)), (11, 0, Some(91)), (500, 11, Some(10)), (501, 11, Some(11))] {
            let channel = bot_core::serde_json::json!({
                "id": id.to_string(), "type": kind, "name": "x", "guild_id": "1", "parent_id": parent.map(|p: u64| p.to_string()),
                "permission_overwrites": [], "thread_metadata": {"archived": false, "auto_archive_duration": 60, "archive_timestamp": "2026-01-01T00:00:00.000000+00:00", "locked": false}
            });
            ctx.cache.update(&Event::ChannelCreate(Box::new(bot_core::twilight_model::gateway::payload::incoming::ChannelCreate(bot_core::serde_json::from_value(channel).unwrap()))));
        }
        let s = Settings { no_xp_channels: vec![10], ..Default::default() };
        assert!(no_xp_here(&ctx, &s, 500));
        assert!(!no_xp_here(&ctx, &s, 501));
        let s = Settings { no_xp_categories: vec![91], ..Default::default() };
        assert!(no_xp_here(&ctx, &s, 11));
        assert!(no_xp_here(&ctx, &s, 501));
        assert!(!no_xp_here(&ctx, &s, 500));
        // A channel the cache doesn't know earns XP.
        assert!(!no_xp_here(&ctx, &s, 999));
    }

    #[test]
    fn streaks_count_days_in_a_row() {
        // First XP ever, then again the same day, the next day, and after a gap.
        assert_eq!(next_streak(0, 0, 20_000), 1);
        assert_eq!(next_streak(1, 20_000, 20_000), 1);
        assert_eq!(next_streak(1, 20_000, 20_001), 2);
        assert_eq!(next_streak(6, 20_001, 20_002), 7);
        assert_eq!(next_streak(6, 20_001, 20_003), 1);
        // It still shows the day after, and is gone the day after that.
        assert_eq!(live_streak(6, 20_001, 20_001), 6);
        assert_eq!(live_streak(6, 20_001, 20_002), 6);
        assert_eq!(live_streak(6, 20_001, 20_003), 0);
        // Days split at midnight UTC.
        assert_eq!(day_of(86_400_000 - 1), 0);
        assert_eq!(day_of(86_400_000), 1);
    }

    #[test]
    fn the_streak_bonus_starts_on_day_two_and_stops_at_the_cap() {
        let s = Settings { streak_bonus: 3, streak_max: 20, ..Default::default() };
        assert_eq!([0, 1, 2, 5, 7, 8, 100].map(|d| s.streak_pct(d)), [0, 0, 3, 12, 18, 20, 20]);
        assert_eq!(Settings::default().streak_pct(30), 0, "the bonus is off by default");
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn earning_keeps_the_streak_and_staff_changes_dont() {
        let ctx = ctx();
        ctx.save_settings(1, KEY, &Settings { streak_bonus: 10, streak_max: 50, prestige_level: 30, prestige_bonus: 10, ..Default::default() }).unwrap();
        let today = day_of(bot_core::now_ms());
        let streak = |ctx: &Ctx| ctx.db.with(|c| c.query_row("SELECT streak, streak_day, xp FROM levels WHERE user_id = 10", [], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))).unwrap();
        grant(&ctx, 1, 10, 20, Earned::Voice, None).await.unwrap();
        assert_eq!(streak(&ctx), (1, today, 20));
        // Yesterday was day 4 of a streak, with one prestige: today is day 5, worth 40% + 10%.
        ctx.db.with(|c| c.execute("UPDATE levels SET streak = 4, streak_day = ?1, prestige = 1 WHERE user_id = 10", [today - 1])).unwrap();
        grant(&ctx, 1, 10, 20, Earned::Voice, None).await.unwrap();
        assert_eq!(streak(&ctx), (5, today, 50));
        // Staff changes neither move the streak nor get a bonus.
        grant(&ctx, 1, 10, 100, Earned::Manual, None).await.unwrap();
        grant(&ctx, 1, 10, -30, Earned::Manual, None).await.unwrap();
        assert_eq!(streak(&ctx), (5, today, 120));
        assert_eq!(member(&ctx, 1, 10).unwrap().unwrap().streak, 5);
    }
}
