//! Twitch live alerts, stream schedule reminders and YouTube uploads.
//!
//! Twitch comes in through EventSub webhooks when the bot has a public https address and an
//! EventSub secret, and falls back to polling Helix every 2 minutes otherwise. Viewer counts are
//! only polled while a stream is live. Each stream is one row in `alerts_streams`.

pub mod hmac;
mod schedule;
mod twitch;
mod web;
mod youtube;

use std::sync::Arc;

use bot_core::anyhow::Result;
use bot_core::discord::CommandCall;
use bot_core::rusqlite::{OptionalExtension, params};
use bot_core::twilight_model::application::command::{Command, CommandType};
use bot_core::twilight_util::builder::command::CommandBuilder;
use bot_core::{Ctx, Feature, FeatureMeta, Group, Icon, Migration, NavItem, feature_async, now_ms};
use serde::{Deserialize, Serialize};

pub const KEY: &str = "alerts";
/// Embed colour for live posts and reminders.
pub const COLOR: u32 = 0xf0806a;

pub const DEFAULT_LIVE_MESSAGE: &str = "{streamer} is live.";
pub const DEFAULT_VIDEO_MESSAGE: &str = "New video from {channel}: {title}\n{url}";
pub const DEFAULT_TIMEZONE: &str = "America/Toronto";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Twitch logins this server follows. Usually one.
    pub twitch_logins: Vec<String>,
    pub live_channel: Option<u64>,
    pub live_role: Option<u64>,
    pub live_message: String,

    pub reminders: bool,
    pub reminder_channel: Option<u64>,
    pub reminder_role: Option<u64>,
    /// Minutes before a segment starts, largest first.
    pub reminder_minutes: Vec<i64>,
    pub weekly_post: bool,
    pub weekly_channel: Option<u64>,
    /// Hour on Monday, in `timezone`.
    pub weekly_hour: i64,
    pub timezone: String,

    /// YouTube channel ids (UC...).
    pub youtube_ids: Vec<String>,
    pub youtube_channel: Option<u64>,
    pub youtube_role: Option<u64>,
    pub youtube_message: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            twitch_logins: Vec::new(),
            live_channel: None,
            live_role: None,
            live_message: DEFAULT_LIVE_MESSAGE.into(),
            reminders: true,
            reminder_channel: None,
            reminder_role: None,
            reminder_minutes: vec![60, 10],
            weekly_post: false,
            weekly_channel: None,
            weekly_hour: 10,
            timezone: DEFAULT_TIMEZONE.into(),
            youtube_ids: Vec::new(),
            youtube_channel: None,
            youtube_role: None,
            youtube_message: DEFAULT_VIDEO_MESSAGE.into(),
        }
    }
}

impl Settings {
    pub fn tz(&self) -> bot_core::jiff::tz::TimeZone {
        bot_core::jiff::tz::TimeZone::get(&self.timezone).unwrap_or(bot_core::jiff::tz::TimeZone::UTC)
    }
}

const MIGRATIONS: &[Migration] = &[Migration {
    name: "001_alerts",
    sql: "
        CREATE TABLE alerts_streams (
            id INTEGER PRIMARY KEY, broadcaster_id TEXT NOT NULL, stream_id TEXT NOT NULL DEFAULT '',
            started_at INTEGER NOT NULL, ended_at INTEGER, duration_secs INTEGER NOT NULL DEFAULT 0,
            title TEXT NOT NULL DEFAULT '', game TEXT NOT NULL DEFAULT '', thumbnail TEXT NOT NULL DEFAULT '',
            peak INTEGER NOT NULL DEFAULT 0, average INTEGER NOT NULL DEFAULT 0,
            samples INTEGER NOT NULL DEFAULT 0, last_sample INTEGER NOT NULL DEFAULT 0, vod_url TEXT);
        CREATE INDEX alerts_streams_by_start ON alerts_streams(broadcaster_id, started_at DESC);
        CREATE TABLE alerts_live_posts (
            stream INTEGER NOT NULL, guild_id INTEGER NOT NULL, channel_id INTEGER NOT NULL,
            message_id INTEGER NOT NULL, PRIMARY KEY (stream, guild_id));
        CREATE TABLE alerts_broadcasters (
            login TEXT PRIMARY KEY, user_id TEXT NOT NULL, name TEXT NOT NULL,
            avatar TEXT NOT NULL DEFAULT '', updated_at INTEGER NOT NULL);
        CREATE TABLE alerts_segments (
            id TEXT PRIMARY KEY, broadcaster_id TEXT NOT NULL, starts_at INTEGER NOT NULL, ends_at INTEGER,
            title TEXT NOT NULL DEFAULT '', game TEXT NOT NULL DEFAULT '', canceled INTEGER NOT NULL DEFAULT 0);
        CREATE INDEX alerts_segments_by_start ON alerts_segments(starts_at);
        CREATE TABLE alerts_reminders_sent (
            guild_id INTEGER NOT NULL, segment TEXT NOT NULL, minutes INTEGER NOT NULL, at INTEGER NOT NULL,
            PRIMARY KEY (guild_id, segment, minutes));
        CREATE TABLE alerts_videos (
            feed TEXT NOT NULL, video_id TEXT NOT NULL, title TEXT NOT NULL, author TEXT NOT NULL,
            published_at INTEGER NOT NULL, seen_at INTEGER NOT NULL, PRIMARY KEY (feed, video_id));
        CREATE TABLE alerts_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    ",
}, Migration {
    // A running sum so the average doesn't drift from rounding, and every category a stream had.
    name: "002_alerts_stream_stats",
    sql: "
        ALTER TABLE alerts_streams ADD COLUMN viewer_sum INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE alerts_streams ADD COLUMN games TEXT NOT NULL DEFAULT '';
        UPDATE alerts_streams SET viewer_sum = average * samples, games = game;
    ",
}];

#[derive(Default)]
pub struct Alerts;

#[feature_async]
impl Feature for Alerts {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Stream alerts",
            description: "Twitch live alerts, stream schedule reminders and YouTube uploads.",
            group: Group::Streams,
            nav: vec![NavItem { label: "Stream alerts", path: KEY, icon: Icon::Live, about: "Twitch and YouTube posts, and the stream schedule." }],
        }
    }

    fn migrations(&self) -> &'static [Migration] {
        MIGRATIONS
    }

    fn commands(&self) -> Vec<Command> {
        vec![CommandBuilder::new("schedule", "This week's stream schedule", CommandType::ChatInput).build()]
    }

    fn routes(&self) -> bot_core::axum::Router<Ctx> {
        web::routes()
    }

    async fn start(&self, ctx: &Ctx) -> Result<()> {
        if twitch::mode(ctx) == twitch::Mode::Off {
            return Ok(());
        }
        // Twitch checks a new subscription by calling the webhook, so wait for the server.
        state_set(ctx, "at:daily", &now_ms().to_string());
        let ctx = ctx.clone();
        bot_core::tokio::spawn(async move {
            bot_core::tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            if let Err(e) = twitch::daily(&ctx).await {
                bot_core::tracing::warn!(error = %e, "couldn't set up Twitch at start");
            }
        });
        Ok(())
    }

    async fn tick(&self, ctx: &Ctx) -> Result<()> {
        let guilds = alert_guilds(ctx);
        if guilds.is_empty() {
            return Ok(());
        }
        let mode = twitch::mode(ctx);
        if mode != twitch::Mode::Off && !logins(&guilds).is_empty() {
            if due(ctx, "daily", 86_400_000)
                && let Err(e) = twitch::daily(ctx).await
            {
                bot_core::tracing::warn!(error = %e, "daily Twitch check failed");
            }
            if let Err(e) = twitch::poll(ctx, mode).await {
                bot_core::tracing::warn!(error = %e, "Twitch poll failed");
            }
            if due(ctx, "schedule", 30 * 60_000)
                && let Err(e) = schedule::refresh(ctx).await
            {
                bot_core::tracing::warn!(error = %e, "schedule refresh failed");
            }
            if due(ctx, "thumbs", 15 * 60_000)
                && let Err(e) = twitch::fill_thumbnails(ctx).await
            {
                bot_core::tracing::warn!(error = %e, "couldn't fill VOD pictures");
            }
        }
        schedule::send_reminders(ctx, &guilds).await;
        schedule::weekly_posts(ctx, &guilds).await;
        youtube::check(ctx, &guilds).await;
        Ok(())
    }

    async fn on_command(&self, ctx: &Ctx, call: &CommandCall) -> Result<()> {
        if call.data.name == "schedule" {
            schedule::command(ctx, call).await?;
        }
        Ok(())
    }

    fn seed_preview(&self, ctx: &Ctx, guild: u64) -> Result<()> {
        preview_rows(ctx, guild)
    }

    fn overview(&self, ctx: &Ctx, page: &bot_core::web::Page) -> bot_core::Overview {
        web::overview(ctx, page)
    }
}

/// Servers with the feature on, with their settings. Read from the database rather than the
/// gateway cache so it also works at boot.
pub(crate) fn alert_guilds(ctx: &Ctx) -> Vec<(u64, Arc<Settings>)> {
    let ids: Vec<i64> = ctx
        .db
        .with(|c| {
            let mut s = c.prepare_cached("SELECT guild_id FROM guild_features WHERE feature = ?1 AND enabled = 1")?;
            s.query_map([KEY], |r| r.get(0))?.collect()
        })
        .unwrap_or_default();
    ids.into_iter().filter_map(|g| ctx.settings::<Settings>(g as u64, KEY).ok().map(|s| (g as u64, s))).collect()
}

/// Every Twitch login any server follows.
pub(crate) fn logins(guilds: &[(u64, Arc<Settings>)]) -> Vec<String> {
    let mut out: Vec<String> = guilds.iter().flat_map(|(_, s)| s.twitch_logins.iter().cloned()).collect();
    out.sort();
    out.dedup();
    out
}

pub(crate) fn state_get(ctx: &Ctx, key: &str) -> Option<String> {
    ctx.db
        .with(|c| c.query_row("SELECT value FROM alerts_state WHERE key = ?1", [key], |r| r.get(0)).optional())
        .ok()
        .flatten()
}

pub(crate) fn state_set(ctx: &Ctx, key: &str, value: &str) {
    let res = ctx.db.with(|c| {
        c.execute(
            "INSERT INTO alerts_state (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
    });
    if let Err(e) = res {
        bot_core::tracing::warn!(key, error = %e, "couldn't store alerts state");
    }
}

pub(crate) fn state_del(ctx: &Ctx, key: &str) {
    if let Err(e) = ctx.db.with(|c| c.execute("DELETE FROM alerts_state WHERE key = ?1", [key])) {
        bot_core::tracing::warn!(key, error = %e, "couldn't clear alerts state");
    }
}

/// True (and the clock reset) when `every_ms` has passed since this key last fired.
pub(crate) fn due(ctx: &Ctx, key: &str, every_ms: i64) -> bool {
    let last: i64 = state_get(ctx, &format!("at:{key}")).and_then(|v| v.parse().ok()).unwrap_or(0);
    let now = now_ms();
    if now - last < every_ms {
        return false;
    }
    state_set(ctx, &format!("at:{key}"), &now.to_string());
    true
}

fn preview_rows(ctx: &Ctx, guild: u64) -> Result<()> {
    let channels = ctx.text_channels(guild);
    let channel = |name: &str| channels.iter().find(|(_, n)| n == name).map(|(id, _)| *id);
    let role = |name: &str| ctx.roles(guild).into_iter().find(|(_, n, _)| n == name).map(|(id, _, _)| id);
    let s = Settings {
        twitch_logins: vec!["antho".into()],
        live_channel: channel("stream-alerts"),
        live_role: role("Live pings"),
        reminder_channel: channel("stream-alerts"),
        reminder_role: role("Live pings"),
        weekly_post: true,
        weekly_channel: channel("announcements"),
        youtube_ids: vec!["UCx7Antho0000000000000a".into()],
        youtube_channel: channel("announcements"),
        ..Settings::default()
    };
    ctx.save_settings(guild, KEY, &s)?;
    let now = now_ms();
    let day = 86_400_000i64;
    let hour = 3_600_000i64;
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT OR REPLACE INTO alerts_broadcasters (login, user_id, name, avatar, updated_at) VALUES ('antho', '41234567', 'Antho', '', ?1)",
        [now],
    )?;
    let games = ["Legends of Idleon", "Hades II", "Balatro", "Legends of Idleon", "Just Chatting", "Hades II"];
    let titles = [
        "World 6 push, then pets",
        "Night run, heat 16",
        "Gold stake attempts",
        "Sailing and gaming upgrades",
        "Q&A and server stuff",
        "Back to the Underworld",
    ];
    for i in 0..44i64 {
        if i % 7 == 4 || i % 11 == 9 {
            continue;
        }
        let started = now - (i * 2 + 1) * day - (i % 3) * hour - 4 * hour;
        let secs = 3 * 3600 + (i * 1871) % 7200;
        let peak = 30 + (i * 37 + i * i * 7) % 61 + (44 - i) / 3;
        let game = games[i as usize % games.len()];
        // Every third stream ends in Just Chatting, to show a stream with two categories.
        let all_games = if i % 3 == 0 && game != "Just Chatting" { format!("{game}\nJust Chatting") } else { game.to_owned() };
        let samples = secs / 300;
        tx.execute(
            "INSERT INTO alerts_streams (broadcaster_id, stream_id, started_at, ended_at, duration_secs, title, game, peak, average, samples, vod_url, viewer_sum, games)
             VALUES ('41234567', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                format!("4{i:010}"),
                started,
                started + secs * 1000,
                secs,
                titles[i as usize % titles.len()],
                game,
                peak,
                peak * 2 / 3,
                samples,
                (i < 6).then(|| format!("https://www.twitch.tv/videos/2{i:09}")),
                peak * 2 / 3 * samples,
                all_games,
            ],
        )?;
    }
    // A follower total for each of the last 30 days, as the daily check stores them.
    for d in 0..30i64 {
        let day = bot_core::jiff::Timestamp::from_millisecond(now - d * day)?.to_zoned(bot_core::jiff::tz::TimeZone::UTC).date().to_string();
        let total = 1_184 + (30 - d) * 2 + (30 - d) * (30 - d) / 25;
        tx.execute(
            "INSERT OR REPLACE INTO daily_stats (guild_id, day, key, value) VALUES (?1, ?2, 'followers', ?3)",
            params![guild as i64, day, total],
        )?;
    }
    // Segments around now, in the server's time zone, so the week image has a past and a future.
    let tz = s.tz();
    let today = bot_core::jiff::Timestamp::now().to_zoned(tz.clone()).date();
    let monday = today.checked_sub(bot_core::jiff::Span::new().days(today.weekday().to_monday_zero_offset() as i64))?;
    let plan = [(0, 19, "Idleon: world 6 push", "Legends of Idleon", false), (2, 20, "Hades II, heat 16", "Hades II", false), (3, 19, "Community game night", "Just Chatting", true), (4, 20, "Balatro gold stakes", "Balatro", false), (6, 15, "Long Sunday Idleon", "Legends of Idleon", false), (7, 19, "Idleon: world 6 push", "Legends of Idleon", false)];
    for (n, (d, h, title, game, canceled)) in plan.iter().enumerate() {
        let date = monday.checked_add(bot_core::jiff::Span::new().days(*d))?;
        let start = date.at(*h, 0, 0, 0).to_zoned(tz.clone())?.timestamp().as_millisecond();
        tx.execute(
            "INSERT OR REPLACE INTO alerts_segments (id, broadcaster_id, starts_at, ends_at, title, game, canceled) VALUES (?1, '41234567', ?2, ?3, ?4, ?5, ?6)",
            params![format!("preview-{n}"), start, start + 3 * hour, title, game, *canceled as i64],
        )?;
    }
    let videos = [
        ("dQw4w9WgXcQ", "I tried every pet in Idleon so you don't have to", 2),
        ("kJQP7kiw5Fk", "World 6 in 40 minutes", 9),
        ("9bZkp7q19f0", "Balatro: the gold stake run", 17),
    ];
    for (id, title, back) in videos {
        tx.execute(
            "INSERT OR REPLACE INTO alerts_videos (feed, video_id, title, author, published_at, seen_at) VALUES ('UCx7Antho0000000000000a', ?1, ?2, 'Antho', ?3, ?3)",
            params![id, title, now - back * day],
        )?;
    }
    tx.execute("INSERT OR REPLACE INTO alerts_state (key, value) VALUES ('yt_init:UCx7Antho0000000000000a', '1')", [])?;
    tx.commit()?;
    Ok(())
}
