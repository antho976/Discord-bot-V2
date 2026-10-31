//! Idleon account reviews: a queue of review requests from a forum channel's posts and from
//! Twitch channel points, worked through on the dashboard. A queue only; reviews happen in
//! the threads.

pub mod store;
pub mod twitch;
mod web;

use bot_core::anyhow::Result;
use bot_core::twilight_model::channel::Channel;
use bot_core::twilight_model::gateway::event::Event;
use bot_core::twilight_model::id::Id;
use bot_core::twilight_model::id::marker::TagMarker;
use bot_core::{Ctx, Feature, FeatureMeta, Group, Icon, Migration, NavItem, Need, Overview, feature_async, now_ms, text};
use serde::{Deserialize, Serialize};

use store::{Added, Request};

pub const KEY: &str = "reviews";
/// Forum tags that mean a post was dealt with; a scan skips those posts.
const DONE_TAGS: &[&str] = &["done", "closed", "completed", "finished", "resolved"];
pub const DEFAULT_PING: &str = "{mention}, it's your turn for an account review. Answer here when you're ready.";
/// A scan takes archived posts created this recently. Forums archive idle posts, so older
/// archived ones are mostly reviews that were finished without a tag.
const SCAN_ARCHIVED_MS: i64 = 30 * 86_400_000;
pub const DEFAULT_DONE: &str = "Your account review is done. Thanks for waiting.";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The forum (or text channel) whose new posts are requests.
    pub channel: Option<u64>,
    /// Posted in the thread by Ping. `{mention}` and `{name}`.
    pub ping_message: String,
    /// Filled in when completing a request; it can be changed or cleared each time.
    pub done_message: String,
    /// The channel points reward whose redemptions are requests.
    pub reward_id: Option<String>,
    pub reward_title: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self { channel: None, ping_message: DEFAULT_PING.into(), done_message: DEFAULT_DONE.into(), reward_id: None, reward_title: String::new() }
    }
}

pub struct Reviews;

#[feature_async]
impl Feature for Reviews {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Account reviews",
            description: "A queue of Idleon account review requests from a forum channel and Twitch channel points.",
            group: Group::Pets,
            nav: vec![NavItem { label: "Account reviews", path: "reviews", icon: Icon::Review, about: "Review requests from your forum and channel points, in the order you take them." }],
        }
    }

    fn migrations(&self) -> &'static [Migration] {
        store::MIGRATIONS
    }

    fn routes(&self) -> bot_core::axum::Router<Ctx> {
        web::routes()
    }

    async fn on_event(&self, ctx: &Ctx, event: &Event) -> Result<()> {
        let Event::ThreadCreate(t) = event else { return Ok(()) };
        let Some(guild) = t.guild_id.map(|g| g.get()) else { return Ok(()) };
        if t.newly_created != Some(true) {
            return Ok(());
        }
        let s = ctx.settings::<Settings>(guild, KEY)?;
        if s.channel.is_none() || t.parent_id.map(|p| p.get()) != s.channel {
            return Ok(());
        }
        let Some(req) = request_from_thread(ctx, &t.0, "thread") else { return Ok(()) };
        if let Added::New(_) = store::add(ctx, guild, &req)? {
            let who = req.user_id.map(|u| ctx.display_name(guild, u)).unwrap_or_else(|| "Someone".into());
            ctx.notify(Some(guild), KEY, format!("{who} asked for an account review: {}", req.name), Some(format!("/g/{guild}/reviews")));
        }
        Ok(())
    }

    /// Fetches channel points redemptions every 5 minutes for servers that connected Twitch,
    /// in the background so a slow Twitch doesn't hold up the other features' minute.
    async fn tick(&self, ctx: &Ctx) -> Result<()> {
        let now = now_ms();
        for guild in ctx.enabled_guilds(KEY) {
            let s = ctx.settings::<Settings>(guild, KEY)?;
            let Some(reward) = s.reward_id.clone() else { continue };
            if !twitch::due(ctx, guild, now) {
                continue;
            }
            let ctx = ctx.clone();
            bot_core::tokio::spawn(async move {
                if let Err(e) = twitch::sync(&ctx, guild, &reward).await {
                    bot_core::tracing::warn!(guild, error = %e, "couldn't sync channel points redemptions");
                    twitch::note_error(&ctx, guild, Some(&e.to_string()));
                }
            });
        }
        Ok(())
    }

    fn overview(&self, ctx: &Ctx, page: &bot_core::web::Page) -> Overview {
        let guild = page.guild_id;
        let mut o = Overview::default();
        let open = store::open(ctx, guild).unwrap_or_default();
        let Some(oldest) = open.iter().map(|r| r.requested_at).min() else { return o };
        let people: Vec<u64> = open.iter().filter_map(|r| r.user_id).take(2).collect();
        let names: Vec<String> = open.iter().take(2).map(|r| r.name.clone()).collect();
        let more = open.len().saturating_sub(names.len());
        let n = open.len();
        o.needs.push(Need {
            since: oldest,
            people: people.iter().map(|u| ctx.member_avatar(guild, *u)).collect(),
            icon: Icon::Review,
            title: format!("{n} account {} waiting", if n == 1 { "review" } else { "reviews" }),
            detail: format!("{}{} · oldest {}", names.join(", "), if more > 0 { format!(" and {more} more") } else { String::new() }, text::ago(oldest)),
            href: page.path("reviews"),
            action: "Review",
            alert: false,
        });
        o
    }

    fn seed_preview(&self, ctx: &Ctx, guild: u64) -> Result<()> {
        web::seed(ctx, guild)
    }
}

/// A request from a new post (thread): its title is the name, its starter the member.
fn request_from_thread(ctx: &Ctx, t: &Channel, source: &'static str) -> Option<Request> {
    let owner = t.owner_id.map(|o| o.get());
    if owner.is_some() && owner == ctx.bot_id() {
        return None;
    }
    let name = text::clip(t.name.as_deref().unwrap_or_default().trim(), 100);
    if name.is_empty() {
        return None;
    }
    let at = created_ms(t);
    Some(Request { name, user_id: owner, thread_id: Some(t.id.get()), requested_at: at, source, ..Request::default() })
}

/// When a post was made.
fn created_ms(t: &Channel) -> i64 {
    t.thread_metadata.as_ref().and_then(|m| m.create_timestamp).map(|ts| ts.as_micros() / 1000).unwrap_or_else(|| bot_core::discord::snowflake_ms(t.id.get()))
}

/// Look through the channel's open posts, and up to 50 archived ones made in the last 30 days,
/// for requests missed while the bot was away, skipping posts tagged as done. Returns (added,
/// already here or merged).
pub async fn scan(ctx: &Ctx, guild: u64, channel: u64) -> Result<(usize, usize)> {
    let done: Vec<Id<TagMarker>> = ctx
        .cache
        .channel(Id::new(channel))
        .and_then(|c| c.available_tags.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|t| DONE_TAGS.contains(&t.name.trim().to_lowercase().as_str()))
        .map(|t| t.id)
        .collect();
    let mut threads: Vec<Channel> = ctx
        .http
        .active_threads(Id::new(guild))
        .await?
        .model()
        .await?
        .threads
        .into_iter()
        .filter(|t| t.parent_id.map(|p| p.get()) == Some(channel))
        .collect();
    match ctx.http.public_archived_threads(Id::new(channel)).limit(50).await {
        Ok(r) => {
            let since = now_ms() - SCAN_ARCHIVED_MS;
            threads.extend(r.model().await?.threads.into_iter().filter(|t| created_ms(t) >= since));
        }
        Err(e) => bot_core::tracing::warn!(guild, channel, error = %e, "couldn't list archived posts"),
    }
    let (mut added, mut skipped) = (0, 0);
    for t in &threads {
        if t.applied_tags.as_ref().is_some_and(|tags| tags.iter().any(|id| done.contains(id))) {
            continue;
        }
        let Some(req) = request_from_thread(ctx, t, "scan") else { continue };
        match store::add(ctx, guild, &req)? {
            Added::New(_) => added += 1,
            Added::Merged(_) | Added::Known => skipped += 1,
        }
    }
    Ok((added, skipped))
}
