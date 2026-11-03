//! The RPG: patrons, expeditions, hunts, the forge, the temple and the Barrow King. See
//! `docs/RPG.md` for the decisions behind it.
//!
//! Each player has one public hub message. Every click edits it with one reply, and the
//! click's route and the player's state version ride in the custom id, so nothing is held in
//! memory between clicks and a restart loses nothing.

pub mod banner;
pub mod data;
pub mod emoji;
pub mod engine;
pub mod game;
pub mod labyrinth;
pub mod quests;
pub mod store;
pub mod ui;
mod web;

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Instant;

use bot_core::anyhow::Result;
use bot_core::discord::{CommandCall, ComponentCall, Reply};
use bot_core::twilight_model::application::command::{Command, CommandType};
use bot_core::twilight_model::application::interaction::Interaction;
use bot_core::twilight_model::channel::message::{AllowedMentions, MessageFlags};
use bot_core::twilight_model::guild::Permissions;
use bot_core::twilight_model::http::interaction::{InteractionResponse, InteractionResponseData, InteractionResponseType};
use bot_core::twilight_model::id::Id;
use bot_core::twilight_util::builder::command::CommandBuilder;
use bot_core::{Ctx, Feature, FeatureMeta, Group, Icon, Migration, NavItem, feature_async, now_ms, tracing};
use serde::{Deserialize, Serialize};

use crate::engine::expedition::Rates;
use crate::game::Route;
use crate::ui::Screen;

pub const KEY: &str = "rpg";

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
    /// Post in the channel when someone beats the realm boss for the first time.
    pub announce: bool,
    /// Test speed: expeditions, healing and Vigor run this many times faster.
    pub speed: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self { channel: None, test_only: true, tester_role: None, xp_rate: 100, loot_rate: 100, gold_rate: 100, announce: true, speed: 1 }
    }
}

impl Settings {
    pub fn rates(&self) -> Rates {
        Rates { xp: self.xp_rate.clamp(10, 1000), loot: self.loot_rate.clamp(10, 1000), gold: self.gold_rate.clamp(10, 1000) }
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

/// When the RPG last posted in each server, so its public lines never pile up.
static LAST_POST: Mutex<Vec<(u64, i64)>> = Mutex::new(Vec::new());

/// At most one public line per server every ten minutes.
pub const POST_GAP_MS: i64 = 10 * 60_000;

fn may_post(guild: u64, now: i64) -> bool {
    let mut l = LAST_POST.lock().unwrap();
    match l.iter_mut().find(|(g, _)| *g == guild) {
        Some((_, at)) if now - *at < POST_GAP_MS => false,
        Some((_, at)) => {
            *at = now;
            true
        }
        None => {
            l.push((guild, now));
            true
        }
    }
}

/// The last 500 clicks' timings.
pub fn clicks() -> Vec<Click> {
    CLICKS.lock().unwrap().iter().copied().collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn public_lines_are_spaced_out() {
        assert!(super::may_post(7, 1_000_000));
        assert!(!super::may_post(7, 1_000_000 + 60_000));
        assert!(!super::may_post(7, 1_000_000 + super::POST_GAP_MS - 1));
        assert!(super::may_post(7, 1_000_000 + super::POST_GAP_MS));
        assert!(super::may_post(8, 1_000_000 + 60_000));
    }
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
    let u = i.member.as_ref().and_then(|m| m.user.as_ref()).or(i.user.as_ref());
    u.map(|u| u.global_name.clone().unwrap_or_else(|| u.name.clone())).unwrap_or_else(|| "Adventurer".into())
}

/// Whether this member may play while the closed test is on.
fn may_play(s: &Settings, i: &Interaction) -> bool {
    if !s.test_only {
        return true;
    }
    let m = i.member.as_ref();
    if m.and_then(|m| m.permissions).is_some_and(|p| p.intersects(Permissions::MANAGE_GUILD | Permissions::ADMINISTRATOR)) {
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
    ctx.interaction().create_response(i.id, &i.token, &InteractionResponse { kind, data: Some(data) }).await?;
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
            nav: vec![NavItem { label: "RPG", path: "rpg", icon: Icon::Star, about: "The RPG's test, its players, its economy and how fast its buttons answer." }],
        }
    }

    fn migrations(&self) -> &'static [Migration] {
        store::MIGRATIONS
    }

    fn commands(&self) -> Vec<Command> {
        vec![CommandBuilder::new("rpg", "Open your adventure", CommandType::ChatInput).build()]
    }

    fn routes(&self) -> bot_core::axum::Router<Ctx> {
        banner::routes().merge(web::routes())
    }

    async fn tick(&self, ctx: &Ctx) -> Result<()> {
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
        let Some(guild) = call.guild_id() else { return Ok(()) };
        let s = ctx.settings::<Settings>(guild, KEY)?;
        store::set_speed(guild, s.speed);
        if !may_play(&s, i) {
            return ctx.reply(i, Reply::text("The RPG is in a closed test for now. It opens to everyone soon.").ephemeral()).await;
        }
        if let (Some(ch), Some(here)) = (s.channel, call.channel_id())
            && ch != here
        {
            return ctx.reply(i, Reply::text(format!("Play in <#{ch}>: use `/rpg` there.")).ephemeral()).await;
        }
        let user = call.user_id();
        let name = display_name(i);
        let started = Instant::now();
        let screen = match store::run(&ctx.db, guild, user, Some(&name), |g| game::overview(ctx, g, s.rates()))? {
            Some(sc) => sc,
            None => game::patron_choice(user, &name),
        };
        let work = started.elapsed().as_micros() as i64;
        let previous = store::get(&ctx.db, guild, user)?.and_then(|p| p.hub);
        let t = Instant::now();
        send(ctx, i, InteractionResponseType::ChannelMessageWithSource, &screen).await?;
        self.record(Click { at: now_ms(), work_us: work, reply_ms: t.elapsed().as_millis() as i64 });
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
        let Some(guild) = call.guild_id() else { return Ok(()) };
        let Some(route) = Route::parse(&call.data.custom_id) else { return Ok(()) };
        let user = call.user_id();
        if route.owner != user {
            return ctx.reply(i, Reply::text(format!("This is <@{}>'s adventure. Use `/rpg` to start yours.", route.owner)).ephemeral()).await;
        }
        let s = ctx.settings::<Settings>(guild, KEY)?;
        store::set_speed(guild, s.speed);
        if !may_play(&s, i) {
            return ctx.reply(i, Reply::text("The RPG is in a closed test for now.").ephemeral()).await;
        }
        let name = display_name(i);
        let started = Instant::now();
        // A new player swearing to a patron: make the character first.
        if route.screen == "patron" && route.action == "swear" && store::get(&ctx.db, guild, user)?.is_none()
            && let Some(god) = data::God::from_id(&route.arg) {
                store::create(&ctx.db, guild, user, &name, god)?;
            }
        let rates = s.rates();
        let out = store::run(&ctx.db, guild, user, Some(&name), |g| game::handle(ctx, g, &route, &call.data.values, rates))?;
        let work = started.elapsed().as_micros() as i64;
        let Some(out) = out else {
            let screen = game::patron_choice(user, &name);
            return send(ctx, i, InteractionResponseType::UpdateMessage, &screen).await;
        };
        let t = Instant::now();
        send(ctx, i, InteractionResponseType::UpdateMessage, &out.screen).await?;
        self.record(Click { at: now_ms(), work_us: work, reply_ms: t.elapsed().as_millis() as i64 });
        if let Some(text) = out.announce
            && s.announce
            && may_post(guild, now_ms())
            && let Some(ch) = i.channel.as_ref().map(|c| c.id.get())
        {
            let _ = ctx.say(ch, &text).await;
        }
        Ok(())
    }

    fn seed_preview(&self, ctx: &Ctx, guild: u64) -> Result<()> {
        web::seed(ctx, guild)
    }
}
