//! Twitch: the app token, Helix calls, EventSub subscriptions and webhook, the polling fallback,
//! and the live post with its edits.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use bot_core::anyhow::{Context as _, Result, anyhow};
use bot_core::axum::body::Bytes;
use bot_core::axum::extract::State;
use bot_core::axum::http::{HeaderMap, StatusCode, header};
use bot_core::axum::response::{IntoResponse, Response};
use bot_core::reqwest::Method;
use bot_core::rusqlite::{OptionalExtension, Row, params, params_from_iter};
use bot_core::serde_json::{self, Value, json};
use bot_core::twilight_model::channel::message::component::Component;
use bot_core::twilight_model::channel::message::{AllowedMentions, Embed};
use bot_core::twilight_model::id::Id;
use bot_core::twilight_util::builder::embed::{EmbedAuthorBuilder, EmbedBuilder, EmbedFieldBuilder, EmbedFooterBuilder, ImageSource};
use bot_core::{Ctx, discord::link_row, jiff, now_ms, text, tracing};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::{COLOR, DEFAULT_LIVE_MESSAGE, KEY, Settings, alert_guilds, due, logins, state_get, state_set};

const HELIX: &str = "https://api.twitch.tv/helix";
/// EventSub subscription types and versions kept for every broadcaster.
const TYPES: &[(&str, &str)] = &[("stream.online", "1"), ("stream.offline", "1"), ("channel.update", "2")];
const FIVE_MIN: i64 = 5 * 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// No client id or secret.
    Off,
    /// EventSub webhooks.
    Push,
    /// Helix `/streams` every 2 minutes.
    Poll,
}

pub fn mode(ctx: &Ctx) -> Mode {
    let c = &ctx.cfg;
    if c.twitch_client_id.is_none() || c.twitch_client_secret.is_none() {
        return Mode::Off;
    }
    let secret_ok = c.twitch_eventsub_secret.as_deref().is_some_and(|s| (10..=100).contains(&s.len()));
    if secret_ok && c.public_url.starts_with("https://") { Mode::Push } else { Mode::Poll }
}

// ---- the app token and Helix ----

static TOKEN: Mutex<Option<(String, i64)>> = Mutex::new(None);

async fn token(ctx: &Ctx) -> Result<String> {
    if let Some((t, expires)) = TOKEN.lock().unwrap().clone()
        && expires - now_ms() > 10 * 60_000
    {
        return Ok(t);
    }
    let (Some(id), Some(secret)) = (&ctx.cfg.twitch_client_id, &ctx.cfg.twitch_client_secret) else {
        return Err(anyhow!("Twitch credentials are not set"));
    };
    #[derive(Deserialize)]
    struct Token {
        access_token: String,
        expires_in: i64,
    }
    let t: Token = ctx
        .web
        .post("https://id.twitch.tv/oauth2/token")
        .form(&[("client_id", id.as_str()), ("client_secret", secret.as_str()), ("grant_type", "client_credentials")])
        .send()
        .await?
        .error_for_status()
        .context("Twitch refused the app token request")?
        .json()
        .await?;
    *TOKEN.lock().unwrap() = Some((t.access_token.clone(), now_ms() + t.expires_in * 1000));
    Ok(t.access_token)
}

/// A Helix call with the app token. A 401 drops the token and tries once more.
async fn request(ctx: &Ctx, method: Method, path: &str, query: &[(&str, String)], body: Option<&Value>) -> Result<bot_core::reqwest::Response> {
    let client_id = ctx.cfg.twitch_client_id.clone().unwrap_or_default();
    let mut tries = 0;
    loop {
        let token = token(ctx).await?;
        let mut req = ctx.web.request(method.clone(), format!("{HELIX}/{path}")).query(query).bearer_auth(&token).header("Client-Id", &client_id);
        if let Some(b) = body {
            req = req.json(b);
        }
        let res = req.send().await?;
        if res.status() == StatusCode::UNAUTHORIZED && tries == 0 {
            *TOKEN.lock().unwrap() = None;
            tries += 1;
            continue;
        }
        return Ok(res);
    }
}

/// GET a Helix endpoint. `None` on 404.
pub(crate) async fn get<T: DeserializeOwned>(ctx: &Ctx, path: &str, query: &[(&str, String)]) -> Result<Option<T>> {
    let res = request(ctx, Method::GET, path, query, None).await?;
    if res.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let res = res.error_for_status().with_context(|| format!("Helix {path}"))?;
    Ok(Some(res.json().await?))
}

#[derive(Deserialize)]
pub(crate) struct List<T> {
    data: Vec<T>,
    #[serde(default)]
    pagination: Option<Pagination>,
}

#[derive(Deserialize, Default)]
struct Pagination {
    cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HStream {
    pub id: String,
    pub user_id: String,
    #[serde(default)]
    pub game_name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub viewer_count: i64,
    #[serde(default)]
    pub started_at: String,
    /// The live preview, with `{width}` and `{height}` to fill in.
    #[serde(default)]
    pub thumbnail_url: String,
}

/// Live streams among these broadcaster ids.
async fn streams(ctx: &Ctx, ids: &[String]) -> Result<Vec<HStream>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut q: Vec<(&str, String)> = ids.iter().take(100).map(|id| ("user_id", id.clone())).collect();
    q.push(("first", "100".into()));
    Ok(get::<List<HStream>>(ctx, "streams", &q).await?.map(|l| l.data).unwrap_or_default())
}

// ---- broadcasters ----

#[derive(Debug, Clone)]
pub struct Broadcaster {
    pub login: String,
    pub user_id: String,
    pub name: String,
    pub avatar: String,
}

impl Broadcaster {
    pub fn url(&self) -> String {
        format!("https://www.twitch.tv/{}", self.login)
    }
}

/// Every broadcaster the bot has looked up.
pub fn broadcasters(ctx: &Ctx) -> Vec<Broadcaster> {
    ctx.db
        .with(|c| {
            let mut s = c.prepare_cached("SELECT login, user_id, name, avatar FROM alerts_broadcasters ORDER BY login")?;
            s.query_map([], |r| Ok(Broadcaster { login: r.get(0)?, user_id: r.get(1)?, name: r.get(2)?, avatar: r.get(3)? }))?.collect()
        })
        .unwrap_or_default()
}

/// The broadcasters a server follows, in its order. Logins not looked up yet are left out.
pub fn for_settings(ctx: &Ctx, s: &Settings) -> Vec<Broadcaster> {
    let all = broadcasters(ctx);
    s.twitch_logins.iter().filter_map(|l| all.iter().find(|b| &b.login == l).cloned()).collect()
}

/// Look up logins on Twitch and store their ids. Returns the logins Twitch doesn't know.
pub async fn resolve(ctx: &Ctx, logins: &[String]) -> Result<Vec<String>> {
    if logins.is_empty() {
        return Ok(Vec::new());
    }
    #[derive(Deserialize)]
    struct HUser {
        id: String,
        login: String,
        display_name: String,
        #[serde(default)]
        profile_image_url: String,
    }
    let q: Vec<(&str, String)> = logins.iter().take(100).map(|l| ("login", l.clone())).collect();
    let users = get::<List<HUser>>(ctx, "users", &q).await?.map(|l| l.data).unwrap_or_default();
    let now = now_ms();
    ctx.db.with(|c| {
        for u in &users {
            c.execute(
                "INSERT INTO alerts_broadcasters (login, user_id, name, avatar, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (login) DO UPDATE SET user_id = excluded.user_id, name = excluded.name, avatar = excluded.avatar, updated_at = excluded.updated_at",
                params![u.login, u.id, u.display_name, u.profile_image_url, now],
            )?;
        }
        Ok(())
    })?;
    Ok(logins.iter().filter(|l| !users.iter().any(|u| &u.login == *l)).cloned().collect())
}

/// Once a day (and at start): refresh broadcaster ids, make sure EventSub subscriptions exist,
/// and store follower totals.
pub async fn daily(ctx: &Ctx) -> Result<()> {
    let guilds = alert_guilds(ctx);
    let logins = logins(&guilds);
    if logins.is_empty() {
        return Ok(());
    }
    resolve(ctx, &logins).await?;
    if mode(ctx) == Mode::Push {
        ensure_subscriptions(ctx).await?;
    }
    followers(ctx, &guilds).await;
    Ok(())
}

// ---- EventSub ----

pub fn callback_url(ctx: &Ctx) -> String {
    format!("{}/webhooks/twitch", ctx.cfg.public_url)
}

/// Make the app's webhook subscriptions match the broadcasters servers follow: create missing
/// ones, drop failed ones and ones nobody needs.
pub async fn ensure_subscriptions(ctx: &Ctx) -> Result<()> {
    #[derive(Deserialize)]
    struct Sub {
        id: String,
        status: String,
        #[serde(rename = "type")]
        kind: String,
        condition: Value,
        transport: Transport,
    }
    #[derive(Deserialize)]
    struct Transport {
        callback: Option<String>,
    }
    let Some(secret) = ctx.cfg.twitch_eventsub_secret.clone() else { return Ok(()) };
    let callback = callback_url(ctx);
    let wanted: Vec<String> = {
        let logins = logins(&alert_guilds(ctx));
        broadcasters(ctx).into_iter().filter(|b| logins.contains(&b.login)).map(|b| b.user_id).collect()
    };
    let mut subs: Vec<Sub> = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let q: Vec<(&str, String)> = after.iter().map(|a| ("after", a.clone())).collect();
        let Some(page) = get::<List<Sub>>(ctx, "eventsub/subscriptions", &q).await? else { break };
        subs.extend(page.data);
        after = page.pagination.and_then(|p| p.cursor).filter(|c| !c.is_empty());
        if after.is_none() || subs.len() > 2000 {
            break;
        }
    }
    let mut have: HashSet<(String, String)> = HashSet::new();
    for s in subs.iter().filter(|s| s.transport.callback.as_deref() == Some(callback.as_str())) {
        let bid = s.condition["broadcaster_user_id"].as_str().unwrap_or_default().to_owned();
        let alive = s.status == "enabled" || s.status == "webhook_callback_verification_pending";
        if alive && wanted.contains(&bid) && have.insert((s.kind.clone(), bid.clone())) {
            continue;
        }
        tracing::info!(id = %s.id, kind = %s.kind, status = %s.status, "removing a Twitch subscription");
        let _ = request(ctx, Method::DELETE, "eventsub/subscriptions", &[("id", s.id.clone())], None).await;
    }
    for bid in &wanted {
        for (kind, version) in TYPES {
            if have.contains(&((*kind).to_owned(), bid.clone())) {
                continue;
            }
            let body = json!({
                "type": kind, "version": version,
                "condition": { "broadcaster_user_id": bid },
                "transport": { "method": "webhook", "callback": callback, "secret": secret },
            });
            let res = request(ctx, Method::POST, "eventsub/subscriptions", &[], Some(&body)).await?;
            if res.status().is_success() {
                tracing::info!(kind, broadcaster = %bid, "subscribed to Twitch events");
            } else {
                let status = res.status();
                let text = res.text().await.unwrap_or_default();
                tracing::warn!(kind, broadcaster = %bid, %status, body = %text, "Twitch refused a subscription");
            }
        }
    }
    Ok(())
}

/// When a message older than this arrives, it's refused (Twitch's replay advice).
const MAX_AGE_MS: i64 = 10 * 60_000;
static SEEN: LazyLock<Mutex<HashMap<String, i64>>> = LazyLock::new(Default::default);

/// The message timestamp is within 10 minutes of now.
pub fn fresh(timestamp: &str, now: i64) -> bool {
    timestamp.parse::<jiff::Timestamp>().is_ok_and(|t| (now - t.as_millisecond()).abs() <= MAX_AGE_MS)
}

/// True the first time a message id is seen in the last 10 minutes.
fn first_sighting(id: &str, now: i64) -> bool {
    let mut seen = SEEN.lock().unwrap();
    seen.retain(|_, at| now - *at <= MAX_AGE_MS);
    if seen.contains_key(id) {
        return false;
    }
    seen.insert(id.to_owned(), now);
    true
}

/// `POST /webhooks/twitch`: EventSub notifications, the subscription challenge and revocations.
pub async fn webhook(State(ctx): State<Ctx>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(secret) = ctx.cfg.twitch_eventsub_secret.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let h = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
    let (id, ts, sig, kind) = (
        h("twitch-eventsub-message-id"),
        h("twitch-eventsub-message-timestamp"),
        h("twitch-eventsub-message-signature"),
        h("twitch-eventsub-message-type"),
    );
    if !crate::hmac::twitch_signature_ok(&secret, &id, &ts, &body, &sig) {
        tracing::warn!("refused a Twitch webhook with a bad signature");
        return StatusCode::FORBIDDEN.into_response();
    }
    let now = now_ms();
    if !fresh(&ts, now) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !first_sighting(&id, now) {
        return StatusCode::NO_CONTENT.into_response();
    }
    let v: Value = serde_json::from_slice(&body).unwrap_or_default();
    match kind.as_str() {
        "webhook_callback_verification" => {
            let challenge = v["challenge"].as_str().unwrap_or_default().to_owned();
            ([(header::CONTENT_TYPE, "text/plain")], challenge).into_response()
        }
        "revocation" => {
            let sub = &v["subscription"];
            let (t, status) = (sub["type"].as_str().unwrap_or("?"), sub["status"].as_str().unwrap_or("?"));
            tracing::warn!(kind = t, status, "Twitch revoked a subscription");
            ctx.notify(None, KEY, format!("Twitch stopped sending {t} events ({status}). The bot subscribes again within a day."), None);
            StatusCode::NO_CONTENT.into_response()
        }
        "notification" => {
            bot_core::tokio::spawn(async move {
                if let Err(e) = handle_event(&ctx, &v).await {
                    tracing::warn!(error = %e, "couldn't handle a Twitch event");
                }
            });
            StatusCode::NO_CONTENT.into_response()
        }
        _ => StatusCode::NO_CONTENT.into_response(),
    }
}

async fn handle_event(ctx: &Ctx, v: &Value) -> Result<()> {
    let e = &v["event"];
    let s = |k: &str| e[k].as_str().unwrap_or_default().to_owned();
    let bid = s("broadcaster_user_id");
    match v["subscription"]["type"].as_str().unwrap_or_default() {
        "stream.online" if e["type"].as_str() == Some("live") => {
            let b = broadcasters(ctx).into_iter().find(|b| b.user_id == bid).unwrap_or(Broadcaster {
                login: s("broadcaster_user_login"),
                user_id: bid.clone(),
                name: s("broadcaster_user_name"),
                avatar: String::new(),
            });
            let started = s("started_at").parse::<jiff::Timestamp>().map(|t| t.as_millisecond()).unwrap_or_else(|_| now_ms());
            on_online(ctx, &b, &s("id"), started, None).await
        }
        "stream.offline" => on_offline(ctx, &bid, now_ms()).await,
        "channel.update" => on_update(ctx, &bid, &s("title"), &s("category_name")).await,
        _ => Ok(()),
    }
}

// ---- streams ----

#[derive(Debug, Clone, Default)]
pub struct StreamRow {
    pub id: i64,
    pub broadcaster_id: String,
    pub stream_id: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub duration_secs: i64,
    pub title: String,
    pub game: String,
    pub peak: i64,
    pub average: i64,
    pub samples: i64,
    /// Last time viewers were read (or the stream was checked).
    pub last_sample: i64,
    pub vod_url: Option<String>,
    /// A picture of the stream with `{width}` and `{height}` to fill in: the live preview while
    /// live, the VOD's once Twitch has made one, or empty.
    pub thumbnail: String,
}

impl StreamRow {
    /// The thumbnail at 640x360, or `None` when there isn't one.
    pub fn thumb_640(&self) -> Option<String> {
        (!self.thumbnail.is_empty()).then(|| sized(&self.thumbnail, 640, 360))
    }
}

/// Fill a Twitch image template. Live previews use `{width}`, VODs `%{width}`.
pub fn sized(template: &str, w: u32, h: u32) -> String {
    template.replace("%{width}", "{width}").replace("%{height}", "{height}").replace("{width}", &w.to_string()).replace("{height}", &h.to_string())
}

const COLS: &str = "id, broadcaster_id, stream_id, started_at, ended_at, duration_secs, title, game, peak, average, samples, last_sample, vod_url, thumbnail";

fn stream_row(r: &Row) -> bot_core::rusqlite::Result<StreamRow> {
    Ok(StreamRow {
        id: r.get(0)?,
        broadcaster_id: r.get(1)?,
        stream_id: r.get(2)?,
        started_at: r.get(3)?,
        ended_at: r.get(4)?,
        duration_secs: r.get(5)?,
        title: r.get(6)?,
        game: r.get(7)?,
        peak: r.get(8)?,
        average: r.get(9)?,
        samples: r.get(10)?,
        last_sample: r.get(11)?,
        vod_url: r.get(12)?,
        thumbnail: r.get(13)?,
    })
}

/// Streams still marked live.
pub fn open_streams(ctx: &Ctx) -> Result<Vec<StreamRow>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(&format!("SELECT {COLS} FROM alerts_streams WHERE ended_at IS NULL"))?;
        s.query_map([], stream_row)?.collect()
    })
}

fn stream(ctx: &Ctx, id: i64) -> Result<Option<StreamRow>> {
    ctx.db.with(|c| c.query_row(&format!("SELECT {COLS} FROM alerts_streams WHERE id = ?1"), [id], stream_row).optional())
}

/// Streams of these broadcasters that started at or after `since`, newest first.
pub fn streams_since(ctx: &Ctx, ids: &[String], since: i64, limit: i64) -> Result<Vec<StreamRow>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let marks = vec!["?"; ids.len()].join(", ");
    let sql = format!("SELECT {COLS} FROM alerts_streams WHERE broadcaster_id IN ({marks}) AND started_at >= {since} ORDER BY started_at DESC LIMIT {limit}");
    ctx.db.with(|c| {
        let mut s = c.prepare(&sql)?;
        s.query_map(params_from_iter(ids.iter()), stream_row)?.collect()
    })
}

fn sample(ctx: &Ctx, id: i64, viewers: i64) -> Result<()> {
    ctx.db.with(|c| {
        c.execute(
            "UPDATE alerts_streams SET peak = MAX(peak, ?2), average = (average * samples + ?2) / (samples + 1),
             samples = samples + 1, last_sample = ?3 WHERE id = ?1",
            params![id, viewers, now_ms()],
        )
    })?;
    Ok(())
}

/// A broadcaster went live: store the stream and post in every server that follows them.
pub async fn on_online(ctx: &Ctx, b: &Broadcaster, stream_id: &str, started_at: i64, info: Option<HStream>) -> Result<()> {
    if let Some(open) = open_streams(ctx)?.into_iter().find(|r| r.broadcaster_id == b.user_id) {
        if open.stream_id == stream_id || stream_id.is_empty() {
            return Ok(());
        }
        // The offline event for the last stream never came.
        let end = open.last_sample.max(open.started_at);
        finish(ctx, open, end).await?;
    }
    let info = match info {
        Some(i) => Some(i),
        None => streams(ctx, std::slice::from_ref(&b.user_id)).await.ok().and_then(|v| v.into_iter().next()),
    };
    let (title, game) = match &info {
        Some(i) => (i.title.clone(), i.game_name.clone()),
        None => channel_info(ctx, &b.user_id).await.unwrap_or_default(),
    };
    let thumbnail = info.as_ref().map(|i| i.thumbnail_url.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| live_thumbnail(&b.login));
    let now = now_ms();
    let id = ctx.db.with(|c| {
        c.execute(
            "INSERT INTO alerts_streams (broadcaster_id, stream_id, started_at, title, game, last_sample, thumbnail) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![b.user_id, stream_id, started_at, title, game, now, thumbnail],
        )?;
        Ok(c.last_insert_rowid())
    })?;
    if let Some(i) = &info {
        sample(ctx, id, i.viewer_count)?;
    }
    let Some(row) = stream(ctx, id)? else { return Ok(()) };
    tracing::info!(login = %b.login, title = %row.title, "stream went live");
    for (guild, s) in alert_guilds(ctx) {
        let Some(channel) = s.live_channel.filter(|_| s.twitch_logins.contains(&b.login)) else { continue };
        let post = live_post(&s, b, &row);
        match send(ctx, channel, &post).await {
            Ok(message) => {
                ctx.db.with(|c| {
                    c.execute(
                        "INSERT OR REPLACE INTO alerts_live_posts (stream, guild_id, channel_id, message_id) VALUES (?1, ?2, ?3, ?4)",
                        params![id, guild as i64, channel as i64, message as i64],
                    )
                })?;
            }
            Err(e) => tracing::warn!(guild, channel, error = %e, "couldn't post a live alert"),
        }
    }
    Ok(())
}

async fn channel_info(ctx: &Ctx, bid: &str) -> Result<(String, String)> {
    #[derive(Deserialize)]
    struct HChannel {
        title: String,
        game_name: String,
    }
    let list = get::<List<HChannel>>(ctx, "channels", &[("broadcaster_id", bid.to_owned())]).await?;
    Ok(list.and_then(|l| l.data.into_iter().next()).map(|c| (c.title, c.game_name)).unwrap_or_default())
}

pub async fn on_offline(ctx: &Ctx, bid: &str, ended: i64) -> Result<()> {
    let Some(row) = open_streams(ctx)?.into_iter().find(|r| r.broadcaster_id == bid) else { return Ok(()) };
    finish(ctx, row, ended).await
}

/// The live preview Twitch serves for a login while they're live.
fn live_thumbnail(login: &str) -> String {
    format!("https://static-cdn.jtvnw.net/previews-ttv/live_user_{login}-{{width}}x{{height}}.jpg")
}

/// Close a stream: duration, VOD link and picture, and the live posts edited to say it ended.
/// The live preview stops working when the stream ends, so it's dropped; the VOD's picture
/// takes its place now or, while Twitch is still making it, from `fill_thumbnails`.
async fn finish(ctx: &Ctx, row: StreamRow, ended: i64) -> Result<()> {
    let duration = ((ended - row.started_at) / 1000).max(0);
    let vod = vod(ctx, &row).await.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "couldn't look up the VOD");
        None
    });
    let (vod_url, thumbnail) = vod.map(|v| (Some(v.url), v.thumbnail_url)).unwrap_or_default();
    ctx.db.with(|c| {
        c.execute(
            "UPDATE alerts_streams SET ended_at = ?2, duration_secs = ?3, vod_url = ?4, thumbnail = ?5 WHERE id = ?1",
            params![row.id, ended, duration, vod_url, thumbnail],
        )
    })?;
    MISSING.lock().unwrap().remove(&row.broadcaster_id);
    tracing::info!(broadcaster = %row.broadcaster_id, duration, "stream ended");
    edit_posts(ctx, row.id).await
}

#[derive(Deserialize)]
struct HVideo {
    #[serde(default)]
    id: String,
    url: String,
    #[serde(default)]
    stream_id: Option<String>,
    #[serde(default)]
    created_at: String,
    /// Empty while Twitch is still processing the VOD.
    #[serde(default)]
    thumbnail_url: String,
}

async fn vod(ctx: &Ctx, row: &StreamRow) -> Result<Option<HVideo>> {
    let q = [("user_id", row.broadcaster_id.clone()), ("type", "archive".into()), ("first", "5".into())];
    let videos = get::<List<HVideo>>(ctx, "videos", &q).await?.map(|l| l.data).unwrap_or_default();
    let created = |v: &HVideo| v.created_at.parse::<jiff::Timestamp>().map(|t| t.as_millisecond()).unwrap_or(0);
    Ok(videos.into_iter().find(|v| {
        if row.stream_id.is_empty() {
            created(v) >= row.started_at - 15 * 60_000
        } else {
            v.stream_id.as_deref() == Some(row.stream_id.as_str())
        }
    }))
}

/// Streams that ended in the last two days with a VOD but no picture yet: ask Twitch for the
/// VODs' pictures, all in one call.
pub async fn fill_thumbnails(ctx: &Ctx) -> Result<()> {
    let since = now_ms() - 2 * 86_400_000;
    let rows: Vec<(i64, String)> = ctx.db.with(|c| {
        let mut s = c.prepare_cached(
            "SELECT id, vod_url FROM alerts_streams WHERE ended_at >= ?1 AND thumbnail = '' AND vod_url IS NOT NULL LIMIT 100",
        )?;
        s.query_map([since], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
    })?;
    let ids: Vec<(i64, String)> = rows.into_iter().filter_map(|(id, url)| video_id(&url).map(|v| (id, v.to_owned()))).collect();
    if ids.is_empty() {
        return Ok(());
    }
    let q: Vec<(&str, String)> = ids.iter().map(|(_, v)| ("id", v.clone())).collect();
    let videos = get::<List<HVideo>>(ctx, "videos", &q).await?.map(|l| l.data).unwrap_or_default();
    ctx.db.with(|c| {
        for v in videos.iter().filter(|v| !v.thumbnail_url.is_empty()) {
            if let Some((row, _)) = ids.iter().find(|(_, id)| *id == v.id) {
                c.execute("UPDATE alerts_streams SET thumbnail = ?2 WHERE id = ?1", params![row, v.thumbnail_url])?;
            }
        }
        Ok(())
    })?;
    Ok(())
}

/// "https://www.twitch.tv/videos/123" -> "123".
fn video_id(url: &str) -> Option<&str> {
    url.trim_end_matches('/').rsplit_once("/videos/").map(|(_, id)| id).filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
}

/// Title or game changed while live: edit the live posts.
pub async fn on_update(ctx: &Ctx, bid: &str, title: &str, game: &str) -> Result<()> {
    let Some(row) = open_streams(ctx)?.into_iter().find(|r| r.broadcaster_id == bid) else { return Ok(()) };
    if row.title == title && row.game == game {
        return Ok(());
    }
    ctx.db.with(|c| c.execute("UPDATE alerts_streams SET title = ?2, game = ?3 WHERE id = ?1", params![row.id, title, game]))?;
    edit_posts(ctx, row.id).await
}

/// Broadcaster id -> how many checks in a row didn't find their stream.
static MISSING: LazyLock<Mutex<HashMap<String, u8>>> = LazyLock::new(Default::default);

/// Called every tick. Polling mode reads `/streams` every 2 minutes for everyone; push mode only
/// reads it every 5 minutes for broadcasters who are live, for viewer counts.
pub async fn poll(ctx: &Ctx, mode: Mode) -> Result<()> {
    let open = open_streams(ctx)?;
    let now = now_ms();
    let ids: Vec<String> = match mode {
        Mode::Off => return Ok(()),
        Mode::Poll => {
            if !due(ctx, "poll", 115_000) {
                return Ok(());
            }
            let logins = logins(&alert_guilds(ctx));
            broadcasters(ctx).into_iter().filter(|b| logins.contains(&b.login)).map(|b| b.user_id).collect()
        }
        Mode::Push => {
            if open.is_empty() || open.iter().all(|r| now - r.last_sample < FIVE_MIN - 30_000) {
                return Ok(());
            }
            open.iter().map(|r| r.broadcaster_id.clone()).collect()
        }
    };
    if ids.is_empty() {
        return Ok(());
    }
    let live = streams(ctx, &ids).await?;
    let all = broadcasters(ctx);
    for st in &live {
        match open.iter().find(|r| r.broadcaster_id == st.user_id) {
            None => {
                let Some(b) = all.iter().find(|b| b.user_id == st.user_id) else { continue };
                let started = st.started_at.parse::<jiff::Timestamp>().map(|t| t.as_millisecond()).unwrap_or(now);
                on_online(ctx, b, &st.id, started, Some(st.clone())).await?;
            }
            Some(row) => {
                MISSING.lock().unwrap().remove(&row.broadcaster_id);
                if now - row.last_sample >= FIVE_MIN - 30_000 {
                    sample(ctx, row.id, st.viewer_count)?;
                }
                on_update(ctx, &row.broadcaster_id, &st.title, &st.game_name).await?;
            }
        }
    }
    for row in open.iter().filter(|r| !live.iter().any(|s| s.user_id == r.broadcaster_id)) {
        // Helix can lag right after going live, and blink now and then: wait for two misses.
        if now - row.started_at < 10 * 60_000 {
            continue;
        }
        let misses = {
            let mut m = MISSING.lock().unwrap();
            let n = m.entry(row.broadcaster_id.clone()).or_default();
            *n += 1;
            *n
        };
        if misses >= 2 {
            let end = if mode == Mode::Poll { now - 2 * 60_000 } else { row.last_sample.max(row.started_at) };
            finish(ctx, row.clone(), end).await?;
        } else if mode == Mode::Push {
            ctx.db.with(|c| c.execute("UPDATE alerts_streams SET last_sample = ?2 WHERE id = ?1", params![row.id, now]))?;
        }
    }
    Ok(())
}

/// Store each server's follower total, if an app token may read it. Twitch documents the
/// endpoint as needing a user token; when it refuses, the bot stops asking.
async fn followers(ctx: &Ctx, guilds: &[(u64, std::sync::Arc<Settings>)]) {
    if state_get(ctx, "followers_unsupported").is_some() {
        return;
    }
    let mut totals: HashMap<String, i64> = HashMap::new();
    for b in broadcasters(ctx) {
        if !guilds.iter().any(|(_, s)| s.twitch_logins.contains(&b.login)) {
            continue;
        }
        let res = request(ctx, Method::GET, "channels/followers", &[("broadcaster_id", b.user_id.clone()), ("first", "1".into())], None).await;
        match res {
            Ok(r) if r.status() == StatusCode::UNAUTHORIZED || r.status() == StatusCode::FORBIDDEN => {
                tracing::info!("Twitch needs a user token for follower totals; skipping them");
                state_set(ctx, "followers_unsupported", "1");
                return;
            }
            Ok(r) if r.status().is_success() => {
                if let Ok(v) = r.json::<Value>().await
                    && let Some(total) = v["total"].as_i64()
                {
                    totals.insert(b.login.clone(), total);
                }
            }
            Ok(r) => tracing::warn!(status = %r.status(), "couldn't read the follower total"),
            Err(e) => tracing::warn!(error = %e, "couldn't read the follower total"),
        }
    }
    for (guild, s) in guilds {
        let found: Vec<i64> = s.twitch_logins.iter().filter_map(|l| totals.get(l).copied()).collect();
        if !found.is_empty() {
            let _ = ctx.record_stat(*guild, "followers", found.iter().sum());
        }
    }
}

// ---- the live post ----

pub struct Post {
    pub content: String,
    pub embed: Embed,
    pub row: Component,
    pub mentions: AllowedMentions,
}

/// The template with its placeholders filled.
pub fn fill_live(template: &str, b: &Broadcaster, title: &str, game: &str) -> String {
    let template = if template.trim().is_empty() { DEFAULT_LIVE_MESSAGE } else { template };
    text::fill(template, &[("streamer", &b.name), ("title", title), ("game", game), ("url", &b.url())])
}

/// The post while live. The thumbnail gets a fresh query string so Discord fetches it again.
pub fn live_post(s: &Settings, b: &Broadcaster, st: &StreamRow) -> Post {
    let url = b.url();
    let text = fill_live(&s.live_message, b, &st.title, &st.game);
    let content = match s.live_role {
        Some(r) => format!("<@&{r}> {text}"),
        None => text,
    };
    let mut author = EmbedAuthorBuilder::new(&b.name).url(&url);
    if !b.avatar.is_empty()
        && let Ok(src) = ImageSource::url(&b.avatar)
    {
        author = author.icon_url(src);
    }
    let title = if st.title.is_empty() { format!("{} is live", b.name) } else { text::clip(&st.title, 256) };
    let mut e = EmbedBuilder::new().color(COLOR).author(author).title(title).url(&url);
    if !st.game.is_empty() {
        e = e.field(EmbedFieldBuilder::new("Playing", text::clip(&st.game, 1024)).inline());
    }
    e = e.field(EmbedFieldBuilder::new("Started", format!("<t:{}:R>", st.started_at / 1000)).inline());
    let thumb = format!("https://static-cdn.jtvnw.net/previews-ttv/live_user_{}-1280x720.jpg?t={}", b.login, now_ms() / 1000);
    if let Ok(src) = ImageSource::url(thumb) {
        e = e.image(src);
    }
    e = e.footer(EmbedFooterBuilder::new("Twitch"));
    Post {
        content,
        embed: e.build(),
        row: link_row("Watch on Twitch", &url),
        mentions: AllowedMentions { roles: s.live_role.map(Id::new).into_iter().collect(), ..Default::default() },
    }
}

/// The post after the stream: how long, how many watched, and the VOD.
pub fn ended_post(b: &Broadcaster, st: &StreamRow) -> Post {
    let length = text::duration(st.duration_secs);
    let content = if st.game.is_empty() {
        format!("{} streamed for {length}.", b.name)
    } else {
        format!("{} streamed {} for {length}.", b.name, st.game)
    };
    let link = st.vod_url.clone().unwrap_or_else(|| b.url());
    let title = if st.title.is_empty() { "Stream ended".to_owned() } else { text::clip(&st.title, 256) };
    let mut e = EmbedBuilder::new()
        .color(0x3a3a3a)
        .author(EmbedAuthorBuilder::new(&b.name).url(b.url()))
        .title(title)
        .url(&link)
        .description(format!("Ended <t:{}:R>.", st.ended_at.unwrap_or_else(now_ms) / 1000))
        .field(EmbedFieldBuilder::new("Length", length).inline());
    if st.samples > 0 {
        e = e
            .field(EmbedFieldBuilder::new("Peak viewers", text::num(st.peak)).inline())
            .field(EmbedFieldBuilder::new("Average", text::num(st.average)).inline());
    }
    Post {
        content,
        embed: e.footer(EmbedFooterBuilder::new("Twitch")).build(),
        row: if st.vod_url.is_some() { link_row("Watch the VOD", &link) } else { link_row("Twitch channel", &link) },
        mentions: AllowedMentions::default(),
    }
}

async fn send(ctx: &Ctx, channel: u64, p: &Post) -> Result<u64> {
    let msg = ctx
        .http
        .create_message(Id::new(channel))
        .content(&p.content)
        .embeds(std::slice::from_ref(&p.embed))
        .components(std::slice::from_ref(&p.row))
        .allowed_mentions(Some(&p.mentions))
        .await?
        .model()
        .await?;
    Ok(msg.id.get())
}

/// Bring every live post of a stream up to date (live or ended).
async fn edit_posts(ctx: &Ctx, stream_id: i64) -> Result<()> {
    let Some(row) = stream(ctx, stream_id)? else { return Ok(()) };
    let posts: Vec<(i64, i64, i64)> = ctx.db.with(|c| {
        let mut s = c.prepare_cached("SELECT guild_id, channel_id, message_id FROM alerts_live_posts WHERE stream = ?1")?;
        s.query_map([stream_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect()
    })?;
    let Some(b) = broadcasters(ctx).into_iter().find(|b| b.user_id == row.broadcaster_id) else { return Ok(()) };
    let none = AllowedMentions::default();
    for (guild, channel, message) in posts {
        let p = if row.ended_at.is_some() {
            ended_post(&b, &row)
        } else {
            match ctx.settings::<Settings>(guild as u64, KEY) {
                Ok(s) => live_post(&s, &b, &row),
                Err(e) => {
                    tracing::warn!(guild, error = %e, "couldn't read alert settings to edit a live post");
                    continue;
                }
            }
        };
        let res = ctx
            .http
            .update_message(Id::new(channel as u64), Id::new(message as u64))
            .content(Some(&p.content))
            .embeds(Some(std::slice::from_ref(&p.embed)))
            .components(Some(std::slice::from_ref(&p.row)))
            .allowed_mentions(Some(&none))
            .await;
        if let Err(e) = res {
            tracing::warn!(guild, channel, error = %e, "couldn't edit a live post");
        }
    }
    Ok(())
}

/// The dashboard's "Send a test alert": a sample live post, with nobody pinged.
pub async fn test_alert(ctx: &Ctx, s: &Settings) -> Result<()> {
    let channel = s.live_channel.ok_or_else(|| anyhow!("no channel"))?;
    let b = for_settings(ctx, s).into_iter().next().unwrap_or_else(|| {
        let login = s.twitch_logins.first().cloned().unwrap_or_else(|| "twitch".into());
        Broadcaster { name: login.clone(), login, user_id: String::new(), avatar: String::new() }
    });
    let row = StreamRow {
        title: "Test alert: this is how a live post looks".into(),
        game: "Just Chatting".into(),
        started_at: now_ms(),
        ..Default::default()
    };
    let mut p = live_post(s, &b, &row);
    p.content = format!("{}\n-# Test alert. Nobody was pinged.", fill_live(&s.live_message, &b, &row.title, &row.game));
    p.mentions = AllowedMentions::default();
    send(ctx, channel, &p).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn antho() -> Broadcaster {
        Broadcaster { login: "antho".into(), user_id: "1".into(), name: "Antho".into(), avatar: String::new() }
    }

    #[test]
    fn template_fills() {
        let b = antho();
        assert_eq!(fill_live("{streamer} is live with {game}: {title} {url}", &b, "Pets", "Idleon"), "Antho is live with Idleon: Pets https://www.twitch.tv/antho");
        assert_eq!(fill_live("  ", &b, "t", "g"), "Antho is live.");
    }

    #[test]
    fn live_post_pings_only_its_role() {
        let s = Settings { live_role: Some(42), ..Settings::default() };
        let row = StreamRow { title: "Pets".into(), game: "Idleon".into(), started_at: 1_700_000_000_000, ..Default::default() };
        let p = live_post(&s, &antho(), &row);
        assert_eq!(p.content, "<@&42> Antho is live.");
        assert_eq!(p.mentions.roles, vec![Id::new(42)]);
        assert!(p.mentions.users.is_empty() && p.mentions.parse.is_empty());
        let img = p.embed.image.unwrap().url;
        assert!(img.contains("live_user_antho-1280x720.jpg?t="), "{img}");
    }

    #[test]
    fn ended_post_has_numbers_and_vod() {
        let row = StreamRow { title: "Pets".into(), duration_secs: 8040, peak: 48, average: 31, samples: 20, ended_at: Some(1), vod_url: Some("https://www.twitch.tv/videos/9".into()), ..Default::default() };
        let p = ended_post(&antho(), &row);
        assert_eq!(p.content, "Antho streamed for 2 h 14 min.");
        let fields: Vec<_> = p.embed.fields.iter().map(|f| (f.name.as_str(), f.value.as_str())).collect();
        assert!(fields.contains(&("Peak viewers", "48")));
        assert!(p.mentions.roles.is_empty());
    }

    #[test]
    fn thumbnails_fill_both_templates() {
        assert_eq!(sized(&live_thumbnail("antho"), 640, 360), "https://static-cdn.jtvnw.net/previews-ttv/live_user_antho-640x360.jpg");
        assert_eq!(sized("https://x/thumb0-%{width}x%{height}.jpg", 320, 180), "https://x/thumb0-320x180.jpg");
        let row = StreamRow::default();
        assert_eq!(row.thumb_640(), None);
    }

    #[test]
    fn vod_ids_come_from_links() {
        assert_eq!(video_id("https://www.twitch.tv/videos/2283"), Some("2283"));
        assert_eq!(video_id("https://www.twitch.tv/antho"), None);
        assert_eq!(video_id("https://www.twitch.tv/videos/"), None);
    }

    #[test]
    fn timestamps_expire() {
        let now = "2026-10-30T18:00:00Z".parse::<jiff::Timestamp>().unwrap().as_millisecond();
        assert!(fresh("2026-10-30T17:55:00.123456789Z", now));
        assert!(!fresh("2026-10-30T17:49:00Z", now));
        assert!(!fresh("not a time", now));
    }

    #[test]
    fn replays_are_caught() {
        assert!(first_sighting("msg-a", 1_000_000));
        assert!(!first_sighting("msg-a", 1_000_500));
        assert!(first_sighting("msg-a", 1_000_000 + MAX_AGE_MS + 1));
    }
}
