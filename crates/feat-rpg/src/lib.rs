//! The RPG: patrons, expeditions, hunts, the forge, the temple and the Barrow King. See
//! `docs/RPG.md` for the decisions behind it.
//!
//! Each player has one public hub message. Every click edits it with one reply, and the
//! click's route and the player's state version ride in the custom id, so nothing is held in
//! memory between clicks and a restart loses nothing.

pub mod banner;
pub mod data;
pub mod dungeons;
pub mod emoji;
pub mod engine;
pub mod game;
pub mod guild;
pub mod labyrinth;
pub mod map;
pub mod market;
pub mod quests;
pub mod shared;
pub mod store;
pub mod ui;
mod web;

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Instant;

use bot_core::anyhow::Result;
use bot_core::discord::{CommandCall, ComponentCall, ModalCall, Reply};
use bot_core::twilight_model::application::command::{Command, CommandType};
use bot_core::twilight_model::application::interaction::Interaction;
use bot_core::twilight_model::channel::message::{AllowedMentions, MessageFlags};
use bot_core::twilight_model::guild::Permissions;
use bot_core::twilight_model::http::interaction::{
    InteractionResponse, InteractionResponseData, InteractionResponseType,
};
use bot_core::twilight_model::id::Id;
use bot_core::twilight_util::builder::command::CommandBuilder;
use bot_core::{
    Ctx, Feature, FeatureMeta, Group, Icon, Migration, NavItem, feature_async, now_ms, tracing,
};
use serde::{Deserialize, Serialize};

use crate::engine::expedition::Rates;
use crate::game::Route;
use crate::ui::Screen;

pub const KEY: &str = "rpg";

/// The longest a kill line can be kept before it's deleted: a week.
pub const MAX_ANNOUNCE_MINUTES: i64 = 7 * 24 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Where hubs live. Empty: wherever `/rpg` is used.
    pub channel: Option<u64>,
    /// Closed test: only members with the tester role (or Manage Server) can play.
    pub test_only: bool,
    pub tester_role: Option<u64>,
    /// Reward rates in percent, for tuning without a deploy.
    pub xp_rate: i64,
    pub loot_rate: i64,
    pub gold_rate: i64,
    /// Post in the channel when someone beats an elite or the realm boss for the first time.
    pub announce: bool,
    /// Those posts are deleted after this many minutes (0: kept).
    pub announce_minutes: i64,
    /// Test speed: expeditions, healing and Vigor run this many times faster.
    pub speed: i64,
    /// Which parts of the game are on, so testing can cut what doesn't work.
    pub modules: store::Modules,
    /// A Titan rises every this many hours (0: only when started from the dashboard).
    pub titan_every_hours: i64,
    /// How long a Titan stays.
    pub titan_hours: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            channel: None,
            test_only: true,
            tester_role: None,
            xp_rate: 100,
            loot_rate: 100,
            gold_rate: 100,
            announce: true,
            announce_minutes: 30,
            speed: 1,
            modules: store::Modules::default(),
            titan_every_hours: 72,
            titan_hours: 48,
        }
    }
}

impl Settings {
    pub fn rates(&self) -> Rates {
        Rates {
            xp: self.xp_rate.clamp(10, 1000),
            loot: self.loot_rate.clamp(10, 1000),
            gold: self.gold_rate.clamp(10, 1000),
        }
    }
}

/// One click's timing: bot work (load, rules, save) and Discord's answer to our reply.
#[derive(Debug, Clone, Copy)]
pub struct Click {
    pub at: i64,
    pub work_us: i64,
    pub reply_ms: i64,
}

static CLICKS: Mutex<VecDeque<Click>> = Mutex::new(VecDeque::new());

/// The last 500 clicks' timings.
pub fn clicks() -> Vec<Click> {
    CLICKS.lock().unwrap().iter().copied().collect()
}

#[derive(Default)]
pub struct Rpg {
    emoji_tried: AtomicI64,
}

impl Rpg {
    fn record(&self, c: Click) {
        let mut q = CLICKS.lock().unwrap();
        if q.len() >= 500 {
            q.pop_front();
        }
        q.push_back(c);
    }
}

fn display_name(i: &Interaction) -> String {
    if let Some(n) = i.member.as_ref().and_then(|m| m.nick.clone()) {
        return n;
    }
    let u = i
        .member
        .as_ref()
        .and_then(|m| m.user.as_ref())
        .or(i.user.as_ref());
    u.map(|u| u.global_name.clone().unwrap_or_else(|| u.name.clone()))
        .unwrap_or_else(|| "Adventurer".into())
}

/// Whether this member may play while the closed test is on.
fn may_play(s: &Settings, i: &Interaction) -> bool {
    if !s.test_only {
        return true;
    }
    let m = i.member.as_ref();
    if m.and_then(|m| m.permissions)
        .is_some_and(|p| p.intersects(Permissions::MANAGE_GUILD | Permissions::ADMINISTRATOR))
    {
        return true;
    }
    match s.tester_role {
        Some(r) => m.is_some_and(|m| m.roles.iter().any(|x| x.get() == r)),
        None => false,
    }
}

async fn send(ctx: &Ctx, i: &Interaction, kind: InteractionResponseType, s: &Screen) -> Result<()> {
    let data = InteractionResponseData {
        components: Some(s.components()),
        flags: Some(MessageFlags::IS_COMPONENTS_V2),
        allowed_mentions: Some(AllowedMentions::default()),
        ..Default::default()
    };
    ctx.interaction()
        .create_response(
            i.id,
            &i.token,
            &InteractionResponse {
                kind,
                data: Some(data),
            },
        )
        .await?;
    Ok(())
}

#[feature_async]
impl Feature for Rpg {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "RPG",
            description: "A Greek and Norse myth RPG: patrons, expeditions, turn-based fights, the forge and a weekly server offering.",
            group: Group::Community,
            nav: vec![NavItem {
                label: "RPG",
                path: "rpg",
                icon: Icon::Star,
                about: "The RPG's test, its players, its economy and how fast its buttons answer.",
            }],
        }
    }

    fn migrations(&self) -> &'static [Migration] {
        store::MIGRATIONS
    }

    fn commands(&self) -> Vec<Command> {
        vec![CommandBuilder::new("rpg", "Open your adventure", CommandType::ChatInput).build()]
    }

    fn routes(&self) -> bot_core::axum::Router<Ctx> {
        banner::routes().merge(map::routes()).merge(web::routes())
    }

    async fn tick(&self, ctx: &Ctx) -> Result<()> {
        // Kill lines that have had their moment, a small batch a minute so the tick stays quick.
        for (ch, msg) in store::take_due_posts(&ctx.db, now_ms(), 20).unwrap_or_default() {
            if let (Some(ch), Some(msg)) = (Id::new_checked(ch), Id::new_checked(msg)) {
                let _ = ctx.http.delete_message(ch, msg).await;
            }
        }
        // Titans: rise on schedule, and say so when one gets away.
        for guild in ctx.enabled_guilds(KEY) {
            let s = ctx.settings::<Settings>(guild, KEY)?;
            store::set_modules(guild, s.modules);
            if !s.modules.titans {
                continue;
            }
            let lines: Vec<(Option<i64>, String)> = ctx.db.with(|c| {
                let mut out = Vec::new();
                for (id, text) in shared::unannounced_endings(c, guild).unwrap_or_default() {
                    out.push((Some(id), text));
                }
                if shared::titan_due(c, guild, s.titan_every_hours).unwrap_or(false)
                    && let Ok(Some(text)) =
                        shared::rise_titan(c, guild, s.titan_hours.clamp(1, 168))
                {
                    out.push((None, text));
                }
                Ok(out)
            })?;
            for (id, text) in lines {
                if let Some(id) = id {
                    let _ = ctx.db.with(|c| Ok(shared::mark_announced(c, id).is_ok()));
                }
                if let Some(ch) = s.channel {
                    let _ = ctx.say(ch, &text).await;
                }
            }
        }
        if emoji::ready() || ctx.cfg.preview || ctx.enabled_guilds(KEY).is_empty() {
            return Ok(());
        }
        // Try now, then every ten minutes until the set is in.
        let now = now_ms();
        if now - self.emoji_tried.load(Ordering::Relaxed) < 600_000 {
            return Ok(());
        }
        self.emoji_tried.store(now, Ordering::Relaxed);
        match emoji::sync(ctx).await {
            Ok(n) => tracing::info!(uploaded = n, "rpg emoji set ready"),
            Err(e) => tracing::warn!(error = %e, "rpg emoji upload failed; plain emojis meanwhile"),
        }
        Ok(())
    }

    async fn on_command(&self, ctx: &Ctx, call: &CommandCall) -> Result<()> {
        let i = &call.interaction;
        let Some(guild) = call.guild_id() else {
            return Ok(());
        };
        let s = ctx.settings::<Settings>(guild, KEY)?;
        store::set_speed(guild, s.speed);
        store::set_modules(guild, s.modules);
        if !may_play(&s, i) {
            return ctx
                .reply(
                    i,
                    Reply::text("The RPG is in a closed test for now. It opens to everyone soon.")
                        .ephemeral(),
                )
                .await;
        }
        if let (Some(ch), Some(here)) = (s.channel, call.channel_id())
            && ch != here
        {
            return ctx
                .reply(
                    i,
                    Reply::text(format!("Play in <#{ch}>: use `/rpg` there.")).ephemeral(),
                )
                .await;
        }
        let user = call.user_id();
        let name = display_name(i);
        let started = Instant::now();
        let screen = match store::run(&ctx.db, guild, user, Some(&name), |g| {
            game::overview(ctx, g, s.rates())
        })? {
            Some(sc) => sc,
            None => game::patron_choice(user, &name),
        };
        let work = started.elapsed().as_micros() as i64;
        let previous = store::get(&ctx.db, guild, user)?.and_then(|p| p.hub);
        let t = Instant::now();
        send(
            ctx,
            i,
            InteractionResponseType::ChannelMessageWithSource,
            &screen,
        )
        .await?;
        self.record(Click {
            at: now_ms(),
            work_us: work,
            reply_ms: t.elapsed().as_millis() as i64,
        });
        // Keep one hub per player: remember this one, remove the last.
        if let Ok(resp) = ctx.interaction().response(&i.token).await
            && let Ok(msg) = resp.model().await
        {
            let _ = store::set_hub(&ctx.db, guild, user, msg.channel_id.get(), msg.id.get());
            if let Some((ch, m)) = previous
                && m != msg.id.get()
                && let (Some(ch), Some(m)) = (Id::new_checked(ch), Id::new_checked(m))
            {
                let _ = ctx.http.delete_message(ch, m).await;
            }
        }
        Ok(())
    }

    async fn on_component(&self, ctx: &Ctx, call: &ComponentCall) -> Result<()> {
        let i = &call.interaction;
        let Some(guild) = call.guild_id() else {
            return Ok(());
        };
        let Some(route) = Route::parse(&call.data.custom_id) else {
            return Ok(());
        };
        let user = call.user_id();
        if route.owner != user {
            return ctx
                .reply(
                    i,
                    Reply::text(format!(
                        "This is <@{}>'s adventure. Use `/rpg` to start yours.",
                        route.owner
                    ))
                    .ephemeral(),
                )
                .await;
        }
        let s = ctx.settings::<Settings>(guild, KEY)?;
        store::set_speed(guild, s.speed);
        store::set_modules(guild, s.modules);
        if !may_play(&s, i) {
            return ctx
                .reply(
                    i,
                    Reply::text("The RPG is in a closed test for now.").ephemeral(),
                )
                .await;
        }
        let name = display_name(i);
        // Founding a guild asks for its name in a form first.
        if route.screen == "guild" && route.action == "new" {
            let id = format!("rpg:{user}:guild:create::{}", route.version.max(0));
            return ctx
                .modal(
                    i,
                    &id,
                    "Found a guild",
                    &[
                        ("name", "Guild name", false, true, None),
                        ("tag", "Tag: 2 to 4 letters, like SKG", false, true, None),
                    ],
                )
                .await;
        }
        // Searching for a guild asks for the name in a form too.
        if route.screen == "guild" && route.action == "search" {
            let id = format!("rpg:{user}:guild:find::{}", route.version.max(0));
            return ctx
                .modal(
                    i,
                    &id,
                    "Find a guild",
                    &[("name", "Guild name or tag", false, true, None)],
                )
                .await;
        }
        let started = Instant::now();
        // A new player swearing to a patron: make the character first.
        if route.screen == "patron"
            && route.action == "swear"
            && store::get(&ctx.db, guild, user)?.is_none()
            && let Some(god) = data::God::from_id(&route.arg)
        {
            store::create(&ctx.db, guild, user, &name, god)?;
        }
        let rates = s.rates();
        let out = store::run(&ctx.db, guild, user, Some(&name), |g| {
            game::handle(ctx, g, &route, &call.data.values, rates)
        })?;
        let work = started.elapsed().as_micros() as i64;
        let Some(out) = out else {
            let screen = game::patron_choice(user, &name);
            return send(ctx, i, InteractionResponseType::UpdateMessage, &screen).await;
        };
        let t = Instant::now();
        send(ctx, i, InteractionResponseType::UpdateMessage, &out.screen).await?;
        self.record(Click {
            at: now_ms(),
            work_us: work,
            reply_ms: t.elapsed().as_millis() as i64,
        });
        if let Some(text) = out.announce
            && s.announce
            && let Some(ch) = i.channel.as_ref().map(|c| c.id.get())
            && let Ok(msg) = ctx.say(ch, &text).await
            && s.announce_minutes > 0
        {
            let at = now_ms() + s.announce_minutes.min(MAX_ANNOUNCE_MINUTES) * 60_000;
            let _ = store::delete_later(&ctx.db, ch, msg, at);
        }
        Ok(())
    }

    async fn on_modal(&self, ctx: &Ctx, call: &ModalCall) -> Result<()> {
        let i = &call.interaction;
        let Some(guild) = i.guild_id.map(|g| g.get()) else {
            return Ok(());
        };
        let Some(mut route) = Route::parse(&call.data.custom_id) else {
            return Ok(());
        };
        let user = i.author_id().map(|u| u.get()).unwrap_or_default();
        if route.owner != user {
            return Ok(());
        }
        let s = ctx.settings::<Settings>(guild, KEY)?;
        store::set_modules(guild, s.modules);
        // A form's answer isn't a stale button: run it at the current version.
        route.version = -1;
        let values = vec![
            call.value("name").unwrap_or_default(),
            call.value("tag").unwrap_or_default(),
        ];
        let name = display_name(i);
        let out = store::run(&ctx.db, guild, user, Some(&name), |g| {
            game::handle(ctx, g, &route, &values, s.rates())
        })?;
        if let Some(out) = out {
            send(ctx, i, InteractionResponseType::UpdateMessage, &out.screen).await?;
        }
        Ok(())
    }

    fn seed_preview(&self, ctx: &Ctx, guild: u64) -> Result<()> {
        web::seed(ctx, guild)
    }
}
