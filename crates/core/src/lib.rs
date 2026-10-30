//! The bot's shared runtime: config, database, the Discord connection, the feature system and
//! the dashboard framework. Each feature lives in its own crate and implements [`Feature`].

pub mod config;
pub mod db;
pub mod discord;
pub mod preview;
pub mod scheduler;
pub mod text;
pub mod web;

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use anyhow::Result;
use async_trait::async_trait;
use rusqlite::params;
use serde::{Serialize, de::DeserializeOwned};
use twilight_cache_inmemory::{DefaultInMemoryCache, ResourceType};
use twilight_gateway::Event;
use twilight_model::application::command::Command;
use twilight_model::id::{Id, marker::ApplicationMarker};

pub use anyhow;
pub use async_trait::async_trait as feature_async;
pub use axum;
pub use maud;
pub use rusqlite;
pub use serde_json;
pub use tracing;
pub use twilight_http;
pub use twilight_mention;
pub use twilight_model;
pub use twilight_util;
pub use jiff;
pub use reqwest;
pub use rand;
pub use tokio;

pub use config::Config;
pub use db::{Db, Migration};
pub use discord::{CommandCall, ComponentCall, ModalCall, Opts, Reply};
pub use web::ui::Icon;

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// Today's date in UTC as `YYYY-MM-DD`, the key daily stats are stored under.
pub fn today() -> String {
    jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC).date().to_string()
}

/// Where a feature's pages sit in the dashboard sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Streams,
    Community,
    Members,
    Safety,
    Pets,
    Tools,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Streams => "Streams",
            Group::Community => "Community",
            Group::Members => "Members",
            Group::Safety => "Safety",
            Group::Pets => "Idleon pets",
            Group::Tools => "Tools",
        }
    }
}

/// A sidebar link. `path` is relative to `/g/{guild}/`, so `"levels"` becomes `/g/123/levels`.
#[derive(Debug, Clone)]
pub struct NavItem {
    pub label: &'static str,
    pub path: &'static str,
    pub icon: Icon,
}

#[derive(Debug, Clone)]
pub struct FeatureMeta {
    /// Stable id: used in the database, in permission grants and as the custom-id prefix of the
    /// feature's buttons and modals (`"{key}:..."`).
    pub key: &'static str,
    pub name: &'static str,
    /// One line, shown on the Features page.
    pub description: &'static str,
    pub group: Group,
    pub nav: Vec<NavItem>,
}

/// One piece of the bot. Every hook has a no-op default, so a feature implements only what it
/// uses. Hooks only run for servers where the feature is switched on.
#[async_trait]
pub trait Feature: Send + Sync + 'static {
    fn meta(&self) -> FeatureMeta;

    fn migrations(&self) -> &'static [Migration] {
        &[]
    }

    /// Admin-only features can't be granted per server: they act on things shared by every
    /// server (app connections), so only dashboard admins see or change them.
    fn admin_only(&self) -> bool {
        false
    }

    /// Slash commands, registered per server where the feature is on.
    fn commands(&self) -> Vec<Command> {
        Vec::new()
    }

    /// Dashboard pages and endpoints. Pages go under `/g/{guild}/{key}`, public pages under
    /// `/p/...`, machine endpoints under `/api/...`.
    fn routes(&self) -> axum::Router<Ctx> {
        axum::Router::new()
    }

    /// Runs once after the database is ready and before the gateway connects.
    async fn start(&self, _ctx: &Ctx) -> Result<()> {
        Ok(())
    }

    /// Every gateway event for a server where the feature is on, plus events with no server.
    async fn on_event(&self, _ctx: &Ctx, _event: &Event) -> Result<()> {
        Ok(())
    }

    async fn on_command(&self, _ctx: &Ctx, _call: &CommandCall) -> Result<()> {
        Ok(())
    }

    async fn on_autocomplete(&self, _ctx: &Ctx, _call: &CommandCall) -> Result<()> {
        Ok(())
    }

    /// Buttons and selects whose custom id starts with `"{key}:"`.
    async fn on_component(&self, _ctx: &Ctx, _call: &ComponentCall) -> Result<()> {
        Ok(())
    }

    /// Modals whose custom id starts with `"{key}:"`.
    async fn on_modal(&self, _ctx: &Ctx, _call: &ModalCall) -> Result<()> {
        Ok(())
    }

    /// Called once a minute. The feature keeps its own slower cadence if it needs one.
    async fn tick(&self, _ctx: &Ctx) -> Result<()> {
        Ok(())
    }

    /// `bot preview` only: fill the feature's tables with believable sample rows for `guild`, so
    /// its pages can be looked at without Discord. Member ids from [`preview::MEMBERS`].
    fn seed_preview(&self, _ctx: &Ctx, _guild: u64) -> Result<()> {
        Ok(())
    }
}

pub struct CtxInner {
    pub cfg: Config,
    pub http: twilight_http::Client,
    pub cache: DefaultInMemoryCache,
    pub db: Db,
    /// Outgoing HTTP for Twitch, YouTube and Discord OAuth.
    pub web: reqwest::Client,
    app_id: OnceLock<Id<ApplicationMarker>>,
    bot_user_id: OnceLock<u64>,
    features: OnceLock<Vec<Arc<dyn Feature>>>,
    toggles: RwLock<HashMap<(u64, String), bool>>,
    settings_cache: RwLock<HashMap<(u64, String), Arc<dyn std::any::Any + Send + Sync>>>,
    counters: Mutex<HashMap<(u64, &'static str), i64>>,
    pub(crate) snapshots: Mutex<discord::Snapshots>,
    /// Live member counts: set from GuildCreate, then moved by joins and leaves (the cache's own
    /// count never changes after connect).
    pub(crate) member_counts: Mutex<HashMap<u64, u64>>,
}

/// Cheap to clone; everything a handler needs.
#[derive(Clone)]
pub struct Ctx(Arc<CtxInner>);

impl Deref for Ctx {
    type Target = CtxInner;
    fn deref(&self) -> &CtxInner {
        &self.0
    }
}

impl Ctx {
    pub fn new(cfg: Config, db: Db) -> Result<Self> {
        let http = twilight_http::Client::new(cfg.discord_token.clone());
        let cache = DefaultInMemoryCache::builder()
            .resource_types(
                ResourceType::GUILD
                    | ResourceType::CHANNEL
                    | ResourceType::ROLE
                    | ResourceType::MEMBER
                    | ResourceType::USER
                    | ResourceType::MESSAGE
                    | ResourceType::VOICE_STATE
                    | ResourceType::USER_CURRENT,
            )
            .message_cache_size(25)
            .build();
        let web = reqwest::Client::builder()
            .user_agent("antho-bot/2 (+discord)")
            .timeout(std::time::Duration::from_secs(20))
            .build()?;
        let toggles = db.with(|c| {
            let mut stmt = c.prepare("SELECT guild_id, feature, enabled FROM guild_features")?;
            let rows = stmt
                .query_map([], |r| Ok(((r.get::<_, i64>(0)? as u64, r.get(1)?), r.get(2)?)))?
                .collect::<rusqlite::Result<HashMap<_, _>>>()?;
            Ok(rows)
        })?;
        Ok(Self(Arc::new(CtxInner {
            cfg,
            http,
            cache,
            db,
            web,
            app_id: OnceLock::new(),
            bot_user_id: OnceLock::new(),
            features: OnceLock::new(),
            toggles: RwLock::new(toggles),
            settings_cache: RwLock::new(HashMap::new()),
            counters: Mutex::new(HashMap::new()),
            snapshots: Mutex::new(discord::Snapshots::default()),
            member_counts: Mutex::new(HashMap::new()),
        })))
    }

    pub fn install_features(&self, features: Vec<Arc<dyn Feature>>) -> Result<()> {
        for f in &features {
            self.db.migrate(f.meta().key, f.migrations())?;
        }
        let _ = self.features.set(features);
        Ok(())
    }

    pub fn features(&self) -> &[Arc<dyn Feature>] {
        self.features.get().map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn feature(&self, key: &str) -> Option<&Arc<dyn Feature>> {
        self.features().iter().find(|f| f.meta().key == key)
    }

    pub fn app_id(&self) -> Id<ApplicationMarker> {
        *self.app_id.get().expect("application id is set at boot")
    }

    pub(crate) fn set_app_id(&self, id: Id<ApplicationMarker>) {
        let _ = self.app_id.set(id);
    }

    /// The bot's own user id, once connected.
    pub fn bot_id(&self) -> Option<u64> {
        self.bot_user_id.get().copied()
    }

    pub(crate) fn set_bot_id(&self, id: u64) {
        let _ = self.bot_user_id.set(id);
    }

    pub fn interaction(&self) -> twilight_http::client::InteractionClient<'_> {
        self.http.interaction(self.app_id())
    }

    // ---- per-server switches ----

    pub fn enabled(&self, guild_id: u64, feature: &str) -> bool {
        self.toggles
            .read()
            .unwrap()
            .get(&(guild_id, feature.to_owned()))
            .copied()
            .unwrap_or(false)
    }

    pub fn set_enabled(&self, guild_id: u64, feature: &str, on: bool) -> Result<()> {
        self.db.with(|c| {
            c.execute(
                "INSERT INTO guild_features (guild_id, feature, enabled) VALUES (?1, ?2, ?3)
                 ON CONFLICT (guild_id, feature) DO UPDATE SET enabled = excluded.enabled",
                params![guild_id as i64, feature, on],
            )
        })?;
        self.toggles.write().unwrap().insert((guild_id, feature.to_owned()), on);
        Ok(())
    }

    /// Servers the bot is in where `feature` is on.
    pub fn enabled_guilds(&self, feature: &str) -> Vec<u64> {
        self.guild_ids().into_iter().filter(|g| self.enabled(*g, feature)).collect()
    }

    /// Servers the bot is in, from the gateway cache.
    pub fn guild_ids(&self) -> Vec<u64> {
        let mut ids: Vec<u64> = self.cache.iter().guilds().map(|g| g.id().get()).collect();
        ids.sort_unstable();
        ids
    }

    // ---- settings ----

    /// A feature's settings for a server. Cached in memory until the next save.
    pub fn settings<T>(&self, guild_id: u64, feature: &str) -> Result<Arc<T>>
    where
        T: DeserializeOwned + Default + Send + Sync + 'static,
    {
        let key = (guild_id, feature.to_owned());
        if let Some(hit) = self.settings_cache.read().unwrap().get(&key)
            && let Ok(v) = hit.clone().downcast::<T>() {
                return Ok(v);
            }
        let value = Arc::new(self.db.settings::<T>(guild_id, feature)?);
        self.settings_cache.write().unwrap().insert(key, value.clone());
        Ok(value)
    }

    pub fn save_settings<T>(&self, guild_id: u64, feature: &str, value: &T) -> Result<()>
    where
        T: Serialize,
    {
        self.db.save_settings(guild_id, feature, value)?;
        self.settings_cache.write().unwrap().remove(&(guild_id, feature.to_owned()));
        Ok(())
    }

    // ---- notifications, dashboard log, counters ----

    /// Put a notice behind the dashboard's bell. `link` is a dashboard path.
    pub fn notify(&self, guild_id: Option<u64>, feature: &str, text: impl Into<String>, link: Option<String>) {
        let text = text.into();
        let res = self.db.with(|c| {
            c.execute(
                "INSERT INTO notifications (at, guild_id, feature, text, link) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![now_ms(), guild_id.map(|g| g as i64), feature, text, link],
            )
        });
        if let Err(e) = res {
            tracing::warn!(error = %e, "couldn't store notification");
        }
    }

    /// Record a dashboard change in the activity log.
    pub fn audit(&self, user: &web::auth::User, guild_id: Option<u64>, feature: &str, action: impl Into<String>) {
        let action = action.into();
        let res = self.db.with(|c| {
            c.execute(
                "INSERT INTO dash_audit (at, user_id, user_name, guild_id, feature, action) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![now_ms(), user.id as i64, user.name, guild_id.map(|g| g as i64), feature, action],
            )
        });
        if let Err(e) = res {
            tracing::warn!(error = %e, "couldn't store dashboard activity");
        }
    }

    /// Add to today's counter for `key` in a server. Held in memory, written once a minute.
    pub fn count(&self, guild_id: u64, key: &'static str, n: i64) {
        *self.counters.lock().unwrap().entry((guild_id, key)).or_default() += n;
    }

    /// Set today's value for `key` outright, e.g. a follower total.
    pub fn record_stat(&self, guild_id: u64, key: &str, value: i64) -> Result<()> {
        self.db.with(|c| {
            c.execute(
                "INSERT INTO daily_stats (guild_id, day, key, value) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (guild_id, key, day) DO UPDATE SET value = excluded.value",
                params![guild_id as i64, today(), key, value],
            )
        })?;
        Ok(())
    }

    pub(crate) fn flush_counters(&self) -> Result<()> {
        let drained: Vec<_> = self.counters.lock().unwrap().drain().collect();
        if drained.is_empty() {
            return Ok(());
        }
        let day = today();
        let mut conn = self.db.lock();
        let tx = conn.transaction()?;
        for ((guild, key), n) in drained {
            tx.execute(
                "INSERT INTO daily_stats (guild_id, day, key, value) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (guild_id, key, day) DO UPDATE SET value = value + excluded.value",
                params![guild as i64, day, key, n],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
