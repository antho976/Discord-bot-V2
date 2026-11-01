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
use bot_core::rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use bot_core::serde_json::{self, Value, json};
use bot_core::twilight_model::channel::message::component::Component;
use bot_core::twilight_model::channel::message::{AllowedMentions, Embed};
use bot_core::twilight_model::id::Id;
use bot_core::twilight_model::util::Timestamp;
use bot_core::twilight_util::builder::embed::{EmbedAuthorBuilder, EmbedBuilder, EmbedFieldBuilder, EmbedFooterBuilder, ImageSource};
use bot_core::{Ctx, discord::link_row, jiff, now_ms, text, tracing};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::{COLOR, DEFAULT_LIVE_MESSAGE, KEY, Settings, alert_guilds, due, logins, state_del, state_get, state_set};

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
    pub(crate) data: Vec<T>,
    #[serde(default)]
    pagination: Option<Pagination>,
}

impl<T> List<T> {
    /// Where the next page starts, when there is one.
    pub(crate) fn cursor(&self) -> Option<String> {
        self.pagination.as_ref().and_then(|p| p.cursor.clone()).filter(|c| !c.is_empty())
    }
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
    // Each step goes ahead when an earlier one fails, or one Twitch error would skip the
    // subscription repair and follower totals for a whole day.
    if let Err(e) = resolve(ctx, &logins).await {
        tracing::warn!(error = %e, "couldn't refresh Twitch channels");
    }
    if mode(ctx) == Mode::Push
        && let Err(e) = ensure_subscriptions(ctx).await
    {
        tracing::warn!(error = %e, "couldn't check Twitch subscriptions");
    }
    followers(ctx, &guilds).await;
    Ok(())
}

// ---- EventSub ----

pub fn callback_url(ctx: &Ctx) -> String {
    format!("{}/webhooks/twitch", ctx.cfg.public_url)
}

/// One `ensure_subscriptions` at a time. A save and the daily check can overlap, and two runs
/// would both create the same missing subscriptions.
static ENSURING: bot_core::tokio::sync::Mutex<()> = bot_core::tokio::sync::Mutex::const_new(());

/// Make the app's webhook subscriptions match the broadcasters servers follow: create missing
/// ones, drop failed ones and ones nobody needs. Runs one at a time; a second call waits and
/// then finds what the first created.
pub async fn ensure_subscriptions(ctx: &Ctx) -> Result<()> {
    let _one_at_a_time = ENSURING.lock().await;
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
    let (before, _) = sub_states(ctx);
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
        match request(ctx, Method::DELETE, "eventsub/subscriptions", &[("id", s.id.clone())], None).await {
            Ok(r) if r.status().is_success() || r.status() == StatusCode::NOT_FOUND => {}
            Ok(r) => tracing::warn!(id = %s.id, status = %r.status(), "Twitch refused to remove a subscription"),
            Err(e) => tracing::warn!(id = %s.id, error = %e, "couldn't remove a Twitch subscription"),
        }
    }
    // What each wanted subscription ended up as, for the Twitch tab.
    let mut states: HashMap<String, HashMap<String, String>> = HashMap::new();
    for s in subs.iter().filter(|s| s.transport.callback.as_deref() == Some(callback.as_str())) {
        let bid = s.condition["broadcaster_user_id"].as_str().unwrap_or_default();
        if have.contains(&(s.kind.clone(), bid.to_owned())) {
            states.entry(bid.to_owned()).or_default().insert(s.kind.clone(), s.status.clone());
        }
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
            // One failed create doesn't stop the rest, and what was learned is still saved.
            let res = match request(ctx, Method::POST, "eventsub/subscriptions", &[], Some(&body)).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(kind, broadcaster = %bid, error = %e, "couldn't reach Twitch to subscribe");
                    continue;
                }
            };
            let state = if res.status().is_success() {
                tracing::info!(kind, broadcaster = %bid, "subscribed to Twitch events");
                let v: Value = res.json().await.unwrap_or_default();
                v["data"][0]["status"].as_str().unwrap_or("webhook_callback_verification_pending").to_owned()
            } else if res.status() == StatusCode::CONFLICT {
                // It already exists (made after the list above was read).
                let last = before.get(bid).and_then(|m| m.get(*kind)).map(String::as_str);
                existing_state(last).to_owned()
            } else {
                let status = res.status();
                let text = res.text().await.unwrap_or_default();
                tracing::warn!(kind, broadcaster = %bid, %status, body = %text, "Twitch refused a subscription");
                format!("refused ({})", status.as_u16())
            };
            states.entry(bid.clone()).or_default().insert((*kind).to_owned(), state);
        }
    }
    save_sub_states(ctx, &states);
    Ok(())
}

/// The state to show for a subscription Twitch says already exists (409) but the listing
/// didn't have: still working if it was working at the last check, else waiting for Twitch's
/// check of the address, which is where every new subscription starts.
fn existing_state(last: Option<&str>) -> &'static str {
    if last == Some("enabled") { "enabled" } else { "webhook_callback_verification_pending" }
}

// ---- what the Twitch tab shows about EventSub ----

/// Broadcaster id -> subscription type -> Twitch's status, as of the last check.
pub type SubStates = HashMap<String, HashMap<String, String>>;

fn save_sub_states(ctx: &Ctx, states: &SubStates) {
    state_set(ctx, "subs", &serde_json::to_string(states).unwrap_or_default());
    state_set(ctx, "subs_at", &now_ms().to_string());
    for (bid, kinds) in states {
        note_online_state(ctx, bid, kinds.get("stream.online").map(String::as_str));
    }
}

/// Keep when a broadcaster's go-live subscription was first seen working: set the first time
/// it's enabled, cleared when it stops. Streams from before then can't have been missed.
fn note_online_state(ctx: &Ctx, bid: &str, status: Option<&str>) {
    let key = format!("online_since:{bid}");
    if status == Some("enabled") {
        if state_get(ctx, &key).is_none() {
            state_set(ctx, &key, &now_ms().to_string());
        }
    } else {
        state_del(ctx, &key);
    }
}

/// Since when the go-live subscription has been working, if it is.
pub fn online_since(ctx: &Ctx, bid: &str) -> Option<i64> {
    state_get(ctx, &format!("online_since:{bid}")).and_then(|v| v.parse().ok())
}

/// The subscription states from the last check, and when that was.
pub fn sub_states(ctx: &Ctx) -> (SubStates, Option<i64>) {
    let states = state_get(ctx, "subs").and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default();
    (states, state_get(ctx, "subs_at").and_then(|v| v.parse().ok()))
}

/// When the last event for a broadcaster arrived, and its type.
pub fn last_event(ctx: &Ctx, bid: &str) -> Option<(i64, String)> {
    let v = state_get(ctx, &format!("event:{bid}"))?;
    let (at, kind) = v.split_once(' ')?;
    Some((at.parse().ok()?, kind.to_owned()))
}

/// Broadcaster id -> (when read, newest VOD start), so page loads within a few minutes share one
/// Helix call.
type VodStarts = HashMap<String, (i64, Option<i64>)>;
static VOD_STARTS: LazyLock<Mutex<VodStarts>> = LazyLock::new(Default::default);

/// When Twitch's newest VOD of a broadcaster started, if it has one. One Helix call every five
/// minutes at most, given up after 3 seconds so a slow Twitch doesn't hold the page.
pub async fn latest_vod_start(ctx: &Ctx, bid: &str) -> Option<i64> {
    let now = now_ms();
    if let Some((at, start)) = VOD_STARTS.lock().unwrap().get(bid)
        && now - at < FIVE_MIN
    {
        return *start;
    }
    let q = [("user_id", bid.to_owned()), ("type", "archive".into()), ("first", "1".into())];
    let call = get::<List<HVideo>>(ctx, "videos", &q);
    // A timeout or error isn't cached, so the next load tries again.
    let list = bot_core::tokio::time::timeout(std::time::Duration::from_secs(3), call).await.ok()?.ok()?;
    let start = list.and_then(|l| l.data.into_iter().next()).and_then(|v| v.created_at.parse::<jiff::Timestamp>().ok()).map(|t| t.as_millisecond());
    let mut cache = VOD_STARTS.lock().unwrap();
    cache.retain(|_, (at, _)| now - *at < FIVE_MIN);
    cache.insert(bid.to_owned(), (now, start));
    start
}

/// Push mode looks broken for a stream when Twitch shows it started more than 15 minutes ago,
/// after the go-live subscription was working, no event has arrived since it started, and the
/// bot has no record of it. A stream from before the subscription worked wasn't missed.
pub fn missed_stream(vod_start: Option<i64>, online_since: Option<i64>, last_event: Option<i64>, recorded: bool, now: i64) -> bool {
    let (Some(start), Some(since)) = (vod_start, online_since) else { return false };
    start >= since && now - start > 15 * 60_000 && last_event.is_none_or(|e| e < start) && !recorded
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
            if let Some(bid) = sub["condition"]["broadcaster_user_id"].as_str() {
                let (mut states, _) = sub_states(&ctx);
                states.entry(bid.to_owned()).or_default().insert(t.to_owned(), status.to_owned());
                state_set(&ctx, "subs", &serde_json::to_string(&states).unwrap_or_default());
                if t == "stream.online" {
                    note_online_state(&ctx, bid, Some(status));
                }
            }
            ctx.notify(None, KEY, format!("Twitch stopped sending {t} events ({status}). The bot subscribes again within a day."), None);
            StatusCode::NO_CONTENT.into_response()
        }
        "notification" => {
            let bid = v["event"]["broadcaster_user_id"].as_str().unwrap_or_default();
            let kind = v["subscription"]["type"].as_str().unwrap_or_default();
            state_set(&ctx, &format!("event:{bid}"), &format!("{now} {kind}"));
            let sent = ts.parse::<jiff::Timestamp>().map(|t| t.as_millisecond()).unwrap_or(now);
            bot_core::tokio::spawn(async move {
                if let Err(e) = handle_event(&ctx, &v, sent).await {
                    tracing::warn!(error = %e, "couldn't handle a Twitch event");
                }
            });
            StatusCode::NO_CONTENT.into_response()
        }
        _ => StatusCode::NO_CONTENT.into_response(),
    }
}

async fn handle_event(ctx: &Ctx, v: &Value, sent: i64) -> Result<()> {
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
        "stream.offline" => on_offline(ctx, &bid, now_ms(), sent).await,
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
    /// Every viewer count read, added up. `average` is this over `samples`, rounded down.
    pub viewer_sum: i64,
    /// Last time viewers were read (or the stream was checked).
    pub last_sample: i64,
    pub vod_url: Option<String>,
    /// A picture of the stream with `{width}` and `{height}` to fill in: the live preview while
    /// live, the VOD's once Twitch has made one, or empty.
    pub thumbnail: String,
    /// Every category the stream was in, in order, one per line. `game` is the current one.
    pub games: String,
    /// The last viewer count read.
    pub viewers: i64,
    /// The follower total when the stream started and when it ended, when Twitch said.
    pub followers_start: Option<i64>,
    pub followers_end: Option<i64>,
    /// Clips made during the stream, counted when it ended and again a while after.
    pub clips: i64,
    /// When the stream last came back after a drop.
    pub resumed_at: Option<i64>,
}

impl StreamRow {
    /// The thumbnail at 640x360, or `None` when there isn't one.
    pub fn thumb_640(&self) -> Option<String> {
        (!self.thumbnail.is_empty()).then(|| sized(&self.thumbnail, 640, 360))
    }

    /// Followers gained over the stream, when both totals were read.
    pub fn followers_gained(&self) -> Option<i64> {
        Some(self.followers_end? - self.followers_start?)
    }

    /// Hours watched: the average viewer count over the stream's length. `None` when no viewers
    /// were read or it rounds to nothing.
    pub fn hours_watched(&self) -> Option<i64> {
        (self.samples > 0).then(|| (self.average * self.duration_secs + 1800) / 3600).filter(|h| *h > 0)
    }

    /// The categories as one line: "Hades II, Just Chatting". Falls back to `game` for rows
    /// from before categories were kept.
    pub fn games_line(&self) -> String {
        let list: Vec<&str> = self.games.lines().filter(|g| !g.is_empty()).collect();
        if list.is_empty() { self.game.clone() } else { list.join(", ") }
    }
}

/// `games` with `game` added at the end, unless it's empty or already there.
pub fn add_game(games: &str, game: &str) -> String {
    if game.is_empty() || games.lines().any(|g| g == game) {
        return games.to_owned();
    }
    if games.is_empty() { game.to_owned() } else { format!("{games}\n{game}") }
}

/// Fill a Twitch image template. Live previews use `{width}`, VODs `%{width}`.
pub fn sized(template: &str, w: u32, h: u32) -> String {
    template.replace("%{width}", "{width}").replace("%{height}", "{height}").replace("{width}", &w.to_string()).replace("{height}", &h.to_string())
}

const COLS: &str = "id, broadcaster_id, stream_id, started_at, ended_at, duration_secs, title, game, peak, average, samples, last_sample, vod_url, thumbnail, games, viewer_sum, viewers, followers_start, followers_end, clips, resumed_at";

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
        games: r.get(14)?,
        viewer_sum: r.get(15)?,
        viewers: r.get(16)?,
        followers_start: r.get(17)?,
        followers_end: r.get(18)?,
        clips: r.get(19)?,
        resumed_at: r.get(20)?,
    })
}

/// Streams still marked live.
pub fn open_streams(ctx: &Ctx) -> Result<Vec<StreamRow>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(&format!("SELECT {COLS} FROM alerts_streams WHERE ended_at IS NULL"))?;
        s.query_map([], stream_row)?.collect()
    })
}

/// A broadcaster's latest stream: the open one if there is one, else the newest.
fn latest_stream(c: &Connection, bid: &str) -> bot_core::rusqlite::Result<Option<StreamRow>> {
    c.query_row(&format!("SELECT {COLS} FROM alerts_streams WHERE broadcaster_id = ?1 ORDER BY ended_at IS NULL DESC, id DESC LIMIT 1"), [bid], stream_row)
        .optional()
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
            "UPDATE alerts_streams SET peak = MAX(peak, ?2), viewers = ?2, viewer_sum = viewer_sum + ?2,
             average = (viewer_sum + ?2) / (samples + 1), samples = samples + 1, last_sample = ?3 WHERE id = ?1",
            params![id, viewers, now_ms()],
        )
    })?;
    Ok(())
}

/// A stream that drops and comes back within this long is the same stream: its posts go back to
/// live instead of a second post (and ping).
const RECONNECT_MS: i64 = 10 * 60_000;

/// What going live does with the broadcaster's latest stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GoLive {
    /// It's already recorded as live.
    Known,
    /// The same stream back after a drop: keep its row and posts, and edit them back to live.
    Resume,
    /// A new stream, posted everywhere. An open one before it is closed first.
    New,
}

/// `latest` is the broadcaster's open stream, else their newest. A stream counts as the same
/// one when Twitch gives the same id, or when it starts within [`RECONNECT_MS`] of the last one
/// ending (or, when the offline event never came, of the last sign of it).
fn go_live(latest: Option<&StreamRow>, stream_id: &str, started_at: i64) -> GoLive {
    let Some(prev) = latest else { return GoLive::New };
    let same_id = !stream_id.is_empty() && prev.stream_id == stream_id;
    let (last_seen, open) = match prev.ended_at {
        Some(ended) => (ended, false),
        None => (prev.last_sample.max(prev.started_at), true),
    };
    if open && (same_id || stream_id.is_empty()) {
        GoLive::Known
    } else if same_id || started_at - last_seen <= RECONNECT_MS {
        GoLive::Resume
    } else {
        GoLive::New
    }
}

/// Put a stream back on air after a drop, under the stream id Twitch gave it this time.
/// Duration and VOD are worked out again when it ends.
fn resume_row(c: &Connection, prev: &StreamRow, stream_id: &str, title: &str, game: &str, thumbnail: &str, now: i64) -> bot_core::rusqlite::Result<()> {
    let stream_id = if stream_id.is_empty() { prev.stream_id.as_str() } else { stream_id };
    let title = if title.is_empty() { prev.title.as_str() } else { title };
    let game = if game.is_empty() { prev.game.as_str() } else { game };
    c.execute(
        "UPDATE alerts_streams SET ended_at = NULL, duration_secs = 0, vod_url = NULL, followers_end = NULL, clips = 0, stream_id = ?2,
         title = ?3, game = ?4, games = ?5, thumbnail = ?6, last_sample = ?7, resumed_at = ?7 WHERE id = ?1",
        params![prev.id, stream_id, title, game, add_game(&prev.games, game), thumbnail, now],
    )?;
    Ok(())
}

/// A broadcaster went live: store the stream and post in every server that follows them. When
/// it's the last stream back after a drop, its posts are edited back to live instead.
pub async fn on_online(ctx: &Ctx, b: &Broadcaster, stream_id: &str, started_at: i64, info: Option<HStream>) -> Result<()> {
    let latest = ctx.db.with(|c| latest_stream(c, &b.user_id))?;
    let decision = go_live(latest.as_ref(), stream_id, started_at);
    match (&latest, decision) {
        (_, GoLive::Known) => return Ok(()),
        // The offline event for the last stream never came.
        (Some(open), GoLive::New) if open.ended_at.is_none() => {
            let end = open.last_sample.max(open.started_at);
            finish(ctx, open.clone(), end).await?;
        }
        _ => {}
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
    if let (GoLive::Resume, Some(prev)) = (decision, &latest) {
        ctx.db.with(|c| resume_row(c, prev, stream_id, &title, &game, &thumbnail, now))?;
        MISSING.lock().unwrap().remove(&b.user_id);
        if let Some(i) = &info {
            sample(ctx, prev.id, i.viewer_count)?;
        }
        tracing::info!(login = %b.login, "stream came back after a drop");
        return edit_posts(ctx, prev.id, false).await;
    }
    let id = ctx.db.with(|c| {
        c.execute(
            "INSERT INTO alerts_streams (broadcaster_id, stream_id, started_at, title, game, games, last_sample, thumbnail) VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7)",
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
    // After the posts, so they don't wait on it. The ended post shows the gain.
    note_followers_start(ctx, b, &row).await
}

/// A stream's gain is only honest from a total read near its start: a stream first seen late
/// (the bot was down, or polling) gets none rather than part of one.
const FOLLOWERS_START_WINDOW_MS: i64 = 15 * 60_000;

/// Read and store the follower total a stream starts from, while it's still near the start.
async fn note_followers_start(ctx: &Ctx, b: &Broadcaster, row: &StreamRow) -> Result<()> {
    if row.followers_start.is_some() || now_ms() - row.started_at > FOLLOWERS_START_WINDOW_MS {
        return Ok(());
    }
    if let Some(total) = follower_total(ctx, b).await {
        ctx.db.with(|c| c.execute("UPDATE alerts_streams SET followers_start = ?2 WHERE id = ?1 AND followers_start IS NULL", params![row.id, total]))?;
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

/// `sent` is when Twitch sent the event. An offline sent before the stream last came back is
/// about the drop, arriving late or retried, and mustn't end the stream that's on now.
pub async fn on_offline(ctx: &Ctx, bid: &str, ended: i64, sent: i64) -> Result<()> {
    let Some(row) = open_streams(ctx)?.into_iter().find(|r| r.broadcaster_id == bid) else { return Ok(()) };
    if row.resumed_at.is_some_and(|at| sent < at) {
        tracing::info!(broadcaster = bid, "ignored an offline sent before the stream came back");
        return Ok(());
    }
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
    // Only worth asking when the start was read, since the post shows the difference.
    let clips = clips_during(ctx, &StreamRow { ended_at: Some(ended), ..row.clone() }).unwrap_or(0);
    let followers_end = match row.followers_start {
        Some(_) => match broadcasters(ctx).into_iter().find(|b| b.user_id == row.broadcaster_id) {
            Some(b) => follower_total(ctx, &b).await,
            None => None,
        },
        None => None,
    };
    // Only while it's still the stream that was read: a reconnect may have resumed the row
    // under a new stream id during the VOD lookup, and an end from elsewhere may have won.
    let n = ctx.db.with(|c| {
        c.execute(
            "UPDATE alerts_streams SET ended_at = ?2, duration_secs = ?3, vod_url = ?4, thumbnail = ?5, followers_end = ?7, clips = ?8
             WHERE id = ?1 AND stream_id = ?6 AND ended_at IS NULL",
            params![row.id, ended, duration, vod_url, thumbnail, row.stream_id, followers_end, clips],
        )
    })?;
    MISSING.lock().unwrap().remove(&row.broadcaster_id);
    if n == 0 {
        return Ok(());
    }
    tracing::info!(broadcaster = %row.broadcaster_id, duration, "stream ended");
    edit_posts(ctx, row.id, false).await
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

/// Count again the clips of streams that ended in the last three hours, and edit their posts
/// when the count changed: Twitch lists a clip from a stream's last minutes after it ends.
pub async fn recount_clips(ctx: &Ctx) -> Result<()> {
    let since = now_ms() - 3 * 3_600_000;
    let rows: Vec<StreamRow> = ctx.db.with(|c| {
        let mut s = c.prepare_cached(&format!("SELECT {COLS} FROM alerts_streams WHERE ended_at >= ?1"))?;
        s.query_map([since], stream_row)?.collect()
    })?;
    for row in rows {
        let n = clips_during(ctx, &row)?;
        if n != row.clips {
            ctx.db.with(|c| c.execute("UPDATE alerts_streams SET clips = ?2 WHERE id = ?1", params![row.id, n]))?;
            edit_posts(ctx, row.id, false).await?;
        }
    }
    Ok(())
}

/// "https://www.twitch.tv/videos/123" -> "123".
fn video_id(url: &str) -> Option<&str> {
    url.trim_end_matches('/').rsplit_once("/videos/").map(|(_, id)| id).filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
}

/// Title or game changed while live: edit the live posts.
pub async fn on_update(ctx: &Ctx, bid: &str, title: &str, game: &str) -> Result<()> {
    let Some(row) = open_streams(ctx)?.into_iter().find(|r| r.broadcaster_id == bid) else { return Ok(()) };
    if note_info(ctx, &row, title, game)? {
        edit_posts(ctx, row.id, false).await?;
    }
    Ok(())
}

/// Store an open stream's title and game. True when either changed.
fn note_info(ctx: &Ctx, row: &StreamRow, title: &str, game: &str) -> Result<bool> {
    if row.title == title && row.game == game {
        return Ok(false);
    }
    let games = add_game(&row.games, game);
    ctx.db.with(|c| c.execute("UPDATE alerts_streams SET title = ?2, game = ?3, games = ?4 WHERE id = ?1", params![row.id, title, game, games]))?;
    Ok(true)
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
                let read = now - row.last_sample >= FIVE_MIN - 30_000;
                if read {
                    sample(ctx, row.id, st.viewer_count)?;
                }
                // In push mode title and game come from channel.update; /streams can lag behind
                // it and would put the old ones back for a while.
                let info = mode == Mode::Poll && note_info(ctx, row, &st.title, &st.game_name)?;
                let viewers = read && (st.viewer_count != row.viewers || row.samples == 0);
                // The posts show the viewer count, so a new one is an edit too.
                if info || viewers {
                    edit_posts(ctx, row.id, !info).await?;
                }
                // The read at go-live may have failed.
                if row.followers_start.is_none()
                    && let Some(b) = all.iter().find(|b| b.user_id == row.broadcaster_id)
                {
                    note_followers_start(ctx, b, row).await?;
                }
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

/// How long after Twitch refuses the app token for follower totals the bot asks it again.
const APP_FOLLOWERS_RETRY_MS: i64 = 7 * 86_400_000;

/// A token a follower total can be read with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    /// The broadcaster's own sign-in, from Account reviews.
    User,
    App,
}

/// The tokens to try for one broadcaster's follower total, in order. Any user token may read
/// the total, so a signed-in broadcaster is read every day. The app token comes after it, and
/// once Twitch refuses the app token it's only tried again a week later.
fn follower_tokens(signed_in: bool, app_refused_at: Option<i64>, now: i64) -> Vec<Via> {
    let mut out = Vec::with_capacity(2);
    if signed_in {
        out.push(Via::User);
    }
    if app_refused_at.is_none_or(|at| now - at >= APP_FOLLOWERS_RETRY_MS) {
        out.push(Via::App);
    }
    out
}

/// One broadcaster's follower total. It's read with their own sign-in when a server connected
/// Twitch on Account reviews, else with the app token, which Twitch usually refuses here; then
/// the app token waits a week before it's tried again.
async fn follower_total(ctx: &Ctx, b: &Broadcaster) -> Option<i64> {
    let refused: Option<i64> = state_get(ctx, "followers_unsupported").and_then(|v| v.parse().ok());
    let client_id = ctx.cfg.twitch_client_id.clone().unwrap_or_default();
    let query = [("broadcaster_id", b.user_id.clone()), ("first", "1".to_owned())];
    let user = feat_reviews::twitch::user_token_for(ctx, &b.user_id).await;
    for via in follower_tokens(user.is_some(), refused, now_ms()) {
        let res = match (via, &user) {
            (Via::User, Some(t)) => ctx.web.get(format!("{HELIX}/channels/followers")).query(&query).bearer_auth(t).header("Client-Id", &client_id).send().await.map_err(Into::into),
            _ => request(ctx, Method::GET, "channels/followers", &query, None).await,
        };
        match res {
            Ok(r) if r.status() == StatusCode::UNAUTHORIZED || r.status() == StatusCode::FORBIDDEN => {
                if via == Via::App {
                    tracing::info!("Twitch needs a user token for follower totals; asking again in a week");
                    state_set(ctx, "followers_unsupported", &now_ms().to_string());
                } else {
                    tracing::warn!(broadcaster = %b.login, status = %r.status(), "Twitch refused the broadcaster's sign-in for the follower total");
                }
            }
            Ok(r) if r.status().is_success() => {
                if let Ok(v) = r.json::<Value>().await
                    && let Some(total) = v["total"].as_i64()
                {
                    if via == Via::App {
                        state_del(ctx, "followers_unsupported");
                    }
                    return Some(total);
                }
            }
            Ok(r) => tracing::warn!(status = %r.status(), "couldn't read the follower total"),
            Err(e) => tracing::warn!(error = %e, "couldn't read the follower total"),
        }
    }
    None
}

/// Store each server's follower total, from each broadcaster it follows.
async fn followers(ctx: &Ctx, guilds: &[(u64, std::sync::Arc<Settings>)]) {
    let mut totals: HashMap<String, i64> = HashMap::new();
    for b in broadcasters(ctx) {
        if !guilds.iter().any(|(_, s)| s.twitch_logins.contains(&b.login)) {
            continue;
        }
        if let Some(total) = follower_total(ctx, &b).await {
            totals.insert(b.login.clone(), total);
        }
    }
    for (guild, s) in guilds {
        // All or nothing: a channel missing one day would read as followers lost, then gained.
        let found: Vec<i64> = s.twitch_logins.iter().filter_map(|l| totals.get(l).copied()).collect();
        if !found.is_empty() && found.len() == s.twitch_logins.len()
            && let Err(e) = ctx.record_stat(*guild, "followers", found.iter().sum())
        {
            tracing::warn!(guild, error = %e, "couldn't store the follower total");
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
    let counted = s.live_viewers && st.samples > 0;
    if counted {
        e = e.field(EmbedFieldBuilder::new("Viewers", viewers_line(st)).inline());
    }
    let now = now_ms();
    let thumb = format!("https://static-cdn.jtvnw.net/previews-ttv/live_user_{}-1280x720.jpg?t={}", b.login, now / 1000);
    if let Ok(src) = ImageSource::url(thumb) {
        e = e.image(src);
    }
    // The post is edited every few minutes; Discord shows each reader when it last was.
    if counted && let Ok(at) = Timestamp::from_secs(now / 1000) {
        e = e.footer(EmbedFooterBuilder::new("Twitch · updated")).timestamp(at);
    } else {
        e = e.footer(EmbedFooterBuilder::new("Twitch"));
    }
    Post {
        content,
        embed: e.build(),
        row: link_row("Watch on Twitch", &url),
        mentions: AllowedMentions { roles: s.live_role.map(Id::new).into_iter().collect(), ..Default::default() },
    }
}

/// "1,204", or "1,204 (peak 1,530)" once the count has dropped from its peak.
pub fn viewers_line(st: &StreamRow) -> String {
    if st.peak > st.viewers { format!("{} (peak {})", text::num(st.viewers), text::num(st.peak)) } else { text::num(st.viewers) }
}

/// "+12", "-3" or "0".
fn signed(n: i64) -> String {
    if n > 0 { format!("+{}", text::num(n)) } else { text::num(n) }
}

/// The post after the stream: how long, what was played, how many watched, followers gained,
/// clips made, and the VOD.
pub fn ended_post(b: &Broadcaster, st: &StreamRow) -> Post {
    let length = text::duration(st.duration_secs);
    let games = st.games_line();
    let content = if games.is_empty() {
        format!("{} streamed for {length}.", b.name)
    } else {
        format!("{} streamed {} for {length}.", b.name, text::clip(&games, 400))
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
    if let Some(h) = st.hours_watched() {
        e = e.field(EmbedFieldBuilder::new("Hours watched", text::num(h)).inline());
    }
    if let Some(n) = st.followers_gained() {
        e = e.field(EmbedFieldBuilder::new("Followers", signed(n)).inline());
    }
    if st.clips > 0 {
        e = e.field(EmbedFieldBuilder::new("Clips", text::num(st.clips)).inline());
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

/// Clips of the stream's channel made while it was on. Only counted when some server posts
/// clips, since only then are they stored.
pub fn clips_during(ctx: &Ctx, st: &StreamRow) -> Result<i64> {
    let end = st.ended_at.unwrap_or_else(now_ms);
    ctx.db.with(|c| {
        c.query_row(
            "SELECT COUNT(*) FROM alerts_clips WHERE broadcaster_id = ?1 AND created_at >= ?2 AND created_at <= ?3",
            params![st.broadcaster_id, st.started_at, end],
            |r| r.get(0),
        )
    })
}

/// Bring every live post of a stream up to date (live or ended). `viewers_only` when only the
/// viewer count changed, so posts in servers that hide it are left alone.
async fn edit_posts(ctx: &Ctx, stream_id: i64, viewers_only: bool) -> Result<()> {
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
                Ok(s) if viewers_only && !s.live_viewers => continue,
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

/// The dashboard's "Send a test alert": a sample live post, with nobody pinged. It reads nothing
/// from Twitch (the broadcaster comes from storage), so a failure is Discord refusing the post.
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
    fn follower_totals_try_the_sign_in_first() {
        let now = 100 * 86_400_000;
        let day = 86_400_000;
        // Never refused: the sign-in first, the app token as the fallback.
        assert_eq!(follower_tokens(true, None, now), [Via::User, Via::App]);
        assert_eq!(follower_tokens(false, None, now), [Via::App]);
        // Refused yesterday: a signed-in broadcaster is still read today, with no app token.
        assert_eq!(follower_tokens(true, Some(now - day), now), [Via::User]);
        assert!(follower_tokens(false, Some(now - day), now).is_empty());
        // A week after the refusal the app token is asked again.
        assert!(follower_tokens(false, Some(now - 7 * day + 1), now).is_empty());
        assert_eq!(follower_tokens(false, Some(now - 7 * day), now), [Via::App]);
        assert_eq!(follower_tokens(true, Some(now - 8 * day), now), [Via::User, Via::App]);
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
        // 31 viewers for 2 h 14 min.
        assert!(fields.contains(&("Hours watched", "69")));
        assert!(!fields.iter().any(|(n, _)| *n == "Followers" || *n == "Clips"));
        assert!(p.mentions.roles.is_empty());
    }

    #[test]
    fn ended_post_names_every_category_and_the_gains() {
        let row = StreamRow {
            game: "Just Chatting".into(),
            games: "Hades II\nJust Chatting".into(),
            duration_secs: 3600,
            followers_start: Some(100),
            followers_end: Some(112),
            clips: 3,
            ended_at: Some(1),
            ..Default::default()
        };
        let p = ended_post(&antho(), &row);
        assert_eq!(p.content, "Antho streamed Hades II, Just Chatting for 1 h.");
        let fields: Vec<_> = p.embed.fields.iter().map(|f| (f.name.as_str(), f.value.as_str())).collect();
        assert!(fields.contains(&("Followers", "+12")));
        assert!(fields.contains(&("Clips", "3")));
        // No viewers read, so no viewer numbers at all.
        assert!(!fields.iter().any(|(n, _)| n.contains("iewers") || *n == "Hours watched"));
    }

    #[test]
    fn live_post_shows_viewers_unless_hidden() {
        let row = StreamRow { started_at: 1_700_000_000_000, viewers: 40, peak: 52, samples: 3, ..Default::default() };
        let p = live_post(&Settings::default(), &antho(), &row);
        let viewers = p.embed.fields.iter().find(|f| f.name == "Viewers").map(|f| f.value.as_str());
        assert_eq!(viewers, Some("40 (peak 52)"));
        assert!(p.embed.timestamp.is_some());
        let hidden = live_post(&Settings { live_viewers: false, ..Settings::default() }, &antho(), &row);
        assert!(!hidden.embed.fields.iter().any(|f| f.name == "Viewers"));
        assert!(hidden.embed.timestamp.is_none());
        // Nothing read yet: no count, rather than a zero.
        let fresh = live_post(&Settings::default(), &antho(), &StreamRow { samples: 0, ..row });
        assert!(!fresh.embed.fields.iter().any(|f| f.name == "Viewers"));
    }

    #[test]
    fn thumbnails_fill_both_templates() {
        assert_eq!(sized(&live_thumbnail("antho"), 640, 360), "https://static-cdn.jtvnw.net/previews-ttv/live_user_antho-640x360.jpg");
        assert_eq!(sized("https://x/thumb0-%{width}x%{height}.jpg", 320, 180), "https://x/thumb0-320x180.jpg");
        let row = StreamRow::default();
        assert_eq!(row.thumb_640(), None);
    }

    #[test]
    fn categories_add_once() {
        assert_eq!(add_game("", "Hades II"), "Hades II");
        assert_eq!(add_game("Hades II", "Just Chatting"), "Hades II\nJust Chatting");
        assert_eq!(add_game("Hades II\nJust Chatting", "Hades II"), "Hades II\nJust Chatting");
        assert_eq!(add_game("Hades II", ""), "Hades II");
        let row = StreamRow { game: "Balatro".into(), ..Default::default() };
        assert_eq!(row.games_line(), "Balatro");
        let row = StreamRow { game: "Balatro".into(), games: "Hades II\nBalatro".into(), ..Default::default() };
        assert_eq!(row.games_line(), "Hades II, Balatro");
    }

    #[test]
    fn vod_ids_come_from_links() {
        assert_eq!(video_id("https://www.twitch.tv/videos/2283"), Some("2283"));
        assert_eq!(video_id("https://www.twitch.tv/antho"), None);
        assert_eq!(video_id("https://www.twitch.tv/videos/"), None);
    }

    #[test]
    fn missed_streams_need_all_three_signs() {
        let min = 60_000;
        let now = 1_000 * min;
        let start = now - 60 * min;
        let since = Some(start - 60 * min);
        // Twitch has a stream from an hour ago, nothing arrived, nothing recorded.
        assert!(missed_stream(Some(start), since, None, false, now));
        assert!(missed_stream(Some(start), since, Some(start - min), false, now));
        // An event after it started, a recorded stream, or a fresh one: fine.
        assert!(!missed_stream(Some(start), since, Some(start + min), false, now));
        assert!(!missed_stream(Some(start), since, None, true, now));
        assert!(!missed_stream(Some(now - 5 * min), since, None, false, now));
        assert!(!missed_stream(None, since, None, false, now));
        // Push mode was set up after that stream, or isn't working at all: nothing was missed.
        assert!(!missed_stream(Some(start), Some(start + min), None, false, now));
        assert!(!missed_stream(Some(start), None, None, false, now));
    }

    #[test]
    fn timestamps_expire() {
        let now = "2026-10-30T18:00:00Z".parse::<jiff::Timestamp>().unwrap().as_millisecond();
        assert!(fresh("2026-10-30T17:55:00.123456789Z", now));
        assert!(!fresh("2026-10-30T17:49:00Z", now));
        assert!(!fresh("not a time", now));
    }

    #[test]
    fn a_stream_back_within_ten_minutes_is_the_same_stream() {
        let min = 60_000;
        let ended = |at: i64, id: &str| StreamRow { stream_id: id.into(), started_at: at - 120 * min, ended_at: Some(at), ..Default::default() };
        let open = |seen: i64, id: &str| StreamRow { stream_id: id.into(), started_at: seen - 120 * min, last_sample: seen, ..Default::default() };
        let t = 1_000 * min;
        // Nothing before: a new stream.
        assert_eq!(go_live(None, "b", t), GoLive::New);
        // Ended 3 or 10 minutes before the new one started: a drop, so the posts go back to live.
        assert_eq!(go_live(Some(&ended(t - 3 * min, "a")), "b", t), GoLive::Resume);
        assert_eq!(go_live(Some(&ended(t - 10 * min, "a")), "b", t), GoLive::Resume);
        assert_eq!(go_live(Some(&ended(t - 3 * min, "a")), "", t), GoLive::Resume);
        // Ended longer ago: a new stream, posted and pinged.
        assert_eq!(go_live(Some(&ended(t - 11 * min, "a")), "b", t), GoLive::New);
        // Polling ended it after two misses but Twitch still has the same stream: resume.
        assert_eq!(go_live(Some(&ended(t - 60 * min, "a")), "a", t), GoLive::Resume);
        // Still open with the same id (or none given): already known.
        assert_eq!(go_live(Some(&open(t - min, "a")), "a", t), GoLive::Known);
        assert_eq!(go_live(Some(&open(t - min, "a")), "", t), GoLive::Known);
        // Still open with a new id: the offline event got lost in a quick reconnect, or it's an
        // old stream that never got closed.
        assert_eq!(go_live(Some(&open(t - 4 * min, "a")), "b", t), GoLive::Resume);
        assert_eq!(go_live(Some(&open(t - 90 * min, "a")), "b", t), GoLive::New);
    }

    fn ctx() -> Ctx {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.install_features(vec![std::sync::Arc::new(crate::Alerts)]).unwrap();
        ctx
    }

    fn rows(ctx: &Ctx) -> Vec<StreamRow> {
        ctx.db
            .with(|c| {
                let mut s = c.prepare(&format!("SELECT {COLS} FROM alerts_streams ORDER BY id"))?;
                s.query_map([], stream_row)?.collect()
            })
            .unwrap()
    }

    #[test]
    fn a_reconnect_reopens_the_stream_instead_of_adding_one() {
        let rt = bot_core::tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let _in = rt.enter();
        let ctx = ctx();
        let b = antho();
        let min = 60_000;
        let t = now_ms();
        ctx.db
            .with(|c| {
                c.execute(
                    "INSERT INTO alerts_streams (broadcaster_id, stream_id, started_at, ended_at, duration_secs, title, game, games, vod_url, peak, samples)
                     VALUES ('1', 'a', ?1, ?2, 7200, 'Pets', 'Idleon', 'Idleon', 'https://www.twitch.tv/videos/9', 40, 24)",
                    params![t - 122 * min, t - 2 * min],
                )
            })
            .unwrap();
        rt.block_on(async {
            // Twitch isn't reachable in tests, so title and game come back empty and are kept.
            on_online(&ctx, &b, "b", t - min, None).await.unwrap();
        });
        let r = rows(&ctx);
        assert_eq!(r.len(), 1);
        let r = &r[0];
        assert_eq!((r.stream_id.as_str(), r.ended_at, r.vod_url.as_deref(), r.title.as_str(), r.peak), ("b", None, None, "Pets", 40));
        assert_eq!(r.started_at, t - 122 * min);
        assert!(r.resumed_at.is_some_and(|at| at >= t));
        // The offline for the drop, arriving late, leaves the resumed stream on.
        rt.block_on(async { on_offline(&ctx, "1", t, t - 2 * min).await.unwrap() });
        assert_eq!(rows(&ctx)[0].ended_at, None);
        // The end that comes later closes it.
        rt.block_on(async { on_offline(&ctx, "1", t, now_ms()).await.unwrap() });
        let r = &rows(&ctx)[0];
        assert_eq!((r.ended_at, r.duration_secs), (Some(t), 122 * 60));
        // An hour later it's a new stream.
        rt.block_on(async { on_online(&ctx, &b, "c", t + 60 * min, None).await.unwrap() });
        let r = rows(&ctx);
        assert_eq!(r.len(), 2);
        assert_eq!((r[1].stream_id.as_str(), r[1].ended_at), ("c", None));
    }

    #[test]
    fn an_end_for_a_stream_that_moved_on_is_ignored() {
        let rt = bot_core::tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let _in = rt.enter();
        let ctx = ctx();
        let t = now_ms();
        ctx.db.with(|c| c.execute("INSERT INTO alerts_streams (broadcaster_id, stream_id, started_at, last_sample) VALUES ('1', 'b', ?1, ?1)", [t])).unwrap();
        // The row was read as stream "a", then a reconnect moved it to "b".
        let stale = StreamRow { stream_id: "a".into(), ..rows(&ctx)[0].clone() };
        rt.block_on(async { finish(&ctx, stale, t).await.unwrap() });
        assert_eq!(rows(&ctx)[0].ended_at, None);
    }

    #[test]
    fn a_subscription_that_already_exists_keeps_its_state() {
        assert_eq!(existing_state(Some("enabled")), "enabled");
        assert_eq!(existing_state(Some("refused (409)")), "webhook_callback_verification_pending");
        assert_eq!(existing_state(Some("webhook_callback_verification_failed")), "webhook_callback_verification_pending");
        assert_eq!(existing_state(None), "webhook_callback_verification_pending");
    }

    #[test]
    fn replays_are_caught() {
        assert!(first_sighting("msg-a", 1_000_000));
        assert!(!first_sighting("msg-a", 1_000_500));
        assert!(first_sighting("msg-a", 1_000_000 + MAX_AGE_MS + 1));
    }
}
