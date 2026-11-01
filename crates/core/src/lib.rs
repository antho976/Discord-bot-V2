//! The bot's shared runtime: config, database, the Discord connection, the feature system and
//! the dashboard framework. Each feature lives in its own crate and implements [`Feature`].

pub mod config;
pub mod db;
pub mod discord;
pub mod preview;
pub mod scheduler;
pub mod text;
pub mod web;

use std::collections::{HashMap, HashSet};
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
            Group::Pets => "Idleon",
            Group::Tools => "Tools",
        }
    }

    /// The group's colour on the dashboard, as a `data-hue` value: its icon tiles, page
    /// headers and empty states take it.
    pub fn hue(self) -> &'static str {
        match self {
            Group::Streams => "streams",
            Group::Community => "community",
            Group::Members => "members",
            Group::Safety => "safety",
            Group::Pets => "pets",
            Group::Tools => "tools",
        }
    }
}

/// A sidebar link. `path` is relative to `/g/{guild}/`, so `"levels"` becomes `/g/123/levels`.
#[derive(Debug, Clone)]
pub struct NavItem {
    pub label: &'static str,
    pub path: &'static str,
    pub icon: Icon,
    /// One line on what the page does, shown under its title.
    pub about: &'static str,
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

/// What a feature adds to a server's overview page. Every part is optional.
#[derive(Default)]
pub struct Overview {
    /// Things waiting on someone, under "Needs you".
    pub needs: Vec<Need>,
    /// Recent happenings for "Around the server". Newest first.
    pub feed: Vec<Happening>,
    /// Blocks for the right column as `(order, markup)`; a lower order sits higher. Each block
    /// brings its own `h2`.
    pub panel: Vec<(u8, maud::Markup)>,
}

/// One thing waiting on someone: "2 pet requests waiting", with a button to deal with it.
pub struct Need {
    /// When it started waiting, for the order (oldest first).
    pub since: i64,
    /// Avatar URLs of the people involved; the icon shows when empty.
    pub people: Vec<String>,
    pub icon: Icon,
    pub title: String,
    pub detail: String,
    pub href: String,
    /// The button: "Review", "Open thread".
    pub action: &'static str,
    /// Draws the icon in the alert colour (a caught scam).
    pub alert: bool,
}

/// One line of "Around the server": a member did something.
pub struct Happening {
    pub at: i64,
    pub avatar: String,
    /// Shown in bold before `text`.
    pub who: String,
    pub text: String,
}

/// One piece of the bot. Every hook has a no-op default, so a feature implements only what it
/// uses. Hooks only run for servers where the feature is switched on.
#[async_trait]
pub trait Feature: Send + Sync + 'static {
    fn meta(&self) -> FeatureMeta;

    fn migrations(&self) -> &'static [Migration] {
        &[]
    }

    /// Features kept for the owner and the people they give Admin+: they act on things shared
    /// by every server (app connections), so no per-server grant or plain admin reaches them.
    fn admin_plus_only(&self) -> bool {
        false
    }

    /// Slash commands, registered per server where the feature is on.
    fn commands(&self) -> Vec<Command> {
        Vec::new()
    }

    /// Slash commands that differ per server (ones people define on the dashboard, or one a
    /// setting switches on), registered next to [`Feature::commands`]. Calls to them reach
    /// `on_command` like the others. Called on every registration and for commands no
    /// feature lists in `commands`, so read cached settings only. After a change, call
    /// [`discord::register_commands`] for that server.
    fn guild_commands(&self, _ctx: &Ctx, _guild: u64) -> Vec<Command> {
        Vec::new()
    }

    /// Names [`Feature::guild_commands`] may use in some server, even where it doesn't now
    /// (a setting that's off). Custom commands can't take them.
    fn reserved_command_names(&self) -> &'static [&'static str] {
        &[]
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

    /// What this feature shows on the server overview. Only called for people who can view the
    /// feature; `page` is that person's page for this feature in this server.
    fn overview(&self, _ctx: &Ctx, _page: &web::Page) -> Overview {
        Overview::default()
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
    /// Bumped on every settings save, so a read that loaded from the database before a save
    /// doesn't put its older value back into the cache after it.
    settings_saves: std::sync::atomic::AtomicU64,
    counters: Mutex<HashMap<(u64, &'static str), i64>>,
    /// Who sent a message today, per server, for the "active today" count: `(day, sets)`.
    active: Mutex<(String, HashMap<u64, HashSet<u64>>)>,
    pub(crate) snapshots: Mutex<discord::Snapshots>,
    /// Live member counts: set from GuildCreate, then moved by joins and leaves (the cache's own
    /// count never changes after connect).
    pub(crate) member_counts: Mutex<HashMap<u64, u64>>,
    /// When [`Ctx::notify_once`] last posted, per `(server, feature, key)`.
    notified: Mutex<HashMap<(u64, String, String), std::time::Instant>>,
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
            settings_saves: std::sync::atomic::AtomicU64::new(0),
            counters: Mutex::new(HashMap::new()),
            active: Mutex::new((today(), HashMap::new())),
            snapshots: Mutex::new(discord::Snapshots::default()),
            member_counts: Mutex::new(HashMap::new()),
            notified: Mutex::new(HashMap::new()),
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

    /// The bot's application id. Set once the gateway task has fetched it; until then a
    /// placeholder, so an early call fails at Discord rather than panicking here (release
    /// builds abort on panic). Check [`Ctx::has_app_id`] first where it matters.
    pub fn app_id(&self) -> Id<ApplicationMarker> {
        self.app_id.get().copied().unwrap_or(Id::new(1))
    }

    pub fn has_app_id(&self) -> bool {
        self.app_id.get().is_some()
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
        let saves = self.settings_saves.load(std::sync::atomic::Ordering::Acquire);
        let value = Arc::new(self.db.settings::<T>(guild_id, feature)?);
        let mut cache = self.settings_cache.write().unwrap();
        if self.settings_saves.load(std::sync::atomic::Ordering::Acquire) == saves {
            cache.insert(key, value.clone());
        }
        Ok(value)
    }

    pub fn save_settings<T>(&self, guild_id: u64, feature: &str, value: &T) -> Result<()>
    where
        T: Serialize,
    {
        self.db.save_settings(guild_id, feature, value)?;
        let mut cache = self.settings_cache.write().unwrap();
        self.settings_saves.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        cache.remove(&(guild_id, feature.to_owned()));
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

    /// [`Ctx::notify`], at most once per `every` for each server, feature and `key`, so something
    /// that keeps failing (a deleted channel, a missing permission) is noticed without a notice
    /// for every attempt. `key` tells a feature's kinds of failure apart ("post", "roles").
    /// Remembered in memory: after a restart the next failure posts again. Returns whether it
    /// posted.
    pub fn notify_once(&self, guild_id: u64, feature: &str, key: &str, text: impl Into<String>, link: Option<String>, every: std::time::Duration) -> bool {
        {
            let mut last = self.notified.lock().unwrap();
            let k = (guild_id, feature.to_owned(), key.to_owned());
            if last.get(&k).is_some_and(|t| t.elapsed() < every) {
                return false;
            }
            last.insert(k, std::time::Instant::now());
        }
        self.notify(Some(guild_id), feature, text, link);
        true
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

    /// Count `user` as active in `guild` today.
    pub(crate) fn mark_active(&self, guild_id: u64, user_id: u64) {
        self.active.lock().unwrap().1.entry(guild_id).or_default().insert(user_id);
    }

    pub(crate) fn flush_counters(&self) -> Result<()> {
        let drained: Vec<_> = self.counters.lock().unwrap().drain().collect();
        let day = today();
        // Active members: written under the day they were counted, then reset at midnight UTC.
        // MAX keeps the day's count after a restart empties the sets.
        let active: Vec<(String, u64, i64)> = {
            let mut a = self.active.lock().unwrap();
            let list = a.1.iter().map(|(g, set)| (a.0.clone(), *g, set.len() as i64)).collect();
            if a.0 != day {
                a.0 = day.clone();
                a.1.clear();
            }
            list
        };
        if drained.is_empty() && active.is_empty() {
            return Ok(());
        }
        let mut conn = self.db.lock();
        let tx = conn.transaction()?;
        for ((guild, key), n) in drained {
            tx.execute(
                "INSERT INTO daily_stats (guild_id, day, key, value) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (guild_id, key, day) DO UPDATE SET value = value + excluded.value",
                params![guild as i64, day, key, n],
            )?;
        }
        for (d, guild, n) in active {
            tx.execute(
                "INSERT INTO daily_stats (guild_id, day, key, value) VALUES (?1, ?2, 'active', ?3)
                 ON CONFLICT (guild_id, key, day) DO UPDATE SET value = MAX(value, excluded.value)",
                params![guild as i64, d, n],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Config, Ctx, Db};

    #[tokio::test]
    async fn notify_once_posts_once_per_window_and_key() {
        let ctx = Ctx::new(Config::preview(0), Db::memory().unwrap()).unwrap();
        let day = Duration::from_secs(86_400);
        let count = || ctx.db.with(|c| c.query_row("SELECT COUNT(*) FROM notifications", [], |r| r.get::<_, i64>(0))).unwrap();
        assert!(ctx.notify_once(1, "levels", "post", "Level-ups aren't being posted.", None, day));
        assert!(!ctx.notify_once(1, "levels", "post", "Level-ups aren't being posted.", None, day));
        assert_eq!(count(), 1);
        // Another key, feature or server has its own window.
        assert!(ctx.notify_once(1, "levels", "roles", "x", None, day));
        assert!(ctx.notify_once(1, "welcome", "post", "x", None, day));
        assert!(ctx.notify_once(2, "levels", "post", "x", None, day));
        assert_eq!(count(), 4);
        // A window of zero never holds anything back.
        assert!(ctx.notify_once(1, "levels", "post", "again", None, Duration::ZERO));
        assert_eq!(count(), 5);
    }
}
