//! Channel points: the broadcaster's Twitch sign-in, the reward picker, syncing redemptions
//! into the queue and marking them fulfilled.
//!
//! The stream alerts only hold an app token. Reading and fulfilling redemptions takes the
//! broadcaster's own token with `channel:manage:redemptions`, and Twitch only shows the
//! redemptions of rewards this Twitch app created. The tokens sit in `reviews_twitch` like the
//! bot's other stored credentials; they never reach a page or the log.

use bot_core::anyhow::{Context, Result, anyhow, bail};
use bot_core::reqwest::{Method, StatusCode};
use bot_core::rusqlite::{OptionalExtension, params};
use bot_core::serde_json::{Value, json};
use bot_core::{Ctx, now_ms, text};
use serde::Deserialize;

use crate::store::{self, Redemption, Request, Tally};

const HELIX: &str = "https://api.twitch.tv/helix";
const TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";
pub const SCOPE: &str = "channel:manage:redemptions";
/// How often a connected server's redemptions are fetched on their own.
pub const SYNC_EVERY_MS: i64 = 5 * 60_000;
/// Pages of 50 unfulfilled redemptions read per sync, newest first.
const SYNC_PAGES: usize = 5;

/// The broadcaster a server connected. Tokens stay in here.
pub struct Conn {
    pub login: String,
    pub broadcaster_id: String,
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_at: i64,
    pub scopes: String,
    pub synced_at: Option<i64>,
    /// The last thing that went wrong, in words for the page.
    pub error: Option<String>,
}

impl Conn {
    /// Signed in, with a token the bot can still use or refresh.
    pub fn connected(&self) -> bool {
        self.refresh_token.is_some()
    }

    pub fn can_manage(&self) -> bool {
        self.scopes.split(' ').any(|s| s == SCOPE)
    }
}

pub fn conn(ctx: &Ctx, guild: u64) -> Result<Option<Conn>> {
    ctx.db.with(|c| {
        c.query_row(
            "SELECT login, broadcaster_id, access_token, refresh_token, expires_at, scopes, synced_at, error FROM reviews_twitch WHERE guild_id = ?1",
            [guild as i64],
            |r| {
                Ok(Conn {
                    login: r.get(0)?,
                    broadcaster_id: r.get(1)?,
                    access_token: r.get(2)?,
                    refresh_token: r.get(3)?,
                    expires_at: r.get(4)?,
                    scopes: r.get(5)?,
                    synced_at: r.get(6)?,
                    error: r.get(7)?,
                })
            },
        )
        .optional()
    })
}

/// The Twitch app's id and secret, from the same settings the stream alerts use.
fn app(ctx: &Ctx) -> Option<(&str, &str)> {
    Some((ctx.cfg.twitch_client_id.as_deref()?, ctx.cfg.twitch_client_secret.as_deref()?))
}

pub fn configured(ctx: &Ctx) -> bool {
    app(ctx).is_some()
}

/// Fixed, so it can be added once to the Twitch app's OAuth redirect URLs.
pub fn redirect_uri(ctx: &Ctx) -> String {
    format!("{}/reviews/twitch/callback", ctx.cfg.public_url)
}

pub fn authorize_url(ctx: &Ctx, state: &str) -> Option<String> {
    let (id, _) = app(ctx)?;
    let params = [("response_type", "code"), ("client_id", id), ("redirect_uri", &redirect_uri(ctx)), ("scope", SCOPE), ("state", state), ("force_verify", "true")];
    bot_core::reqwest::Url::parse_with_params("https://id.twitch.tv/oauth2/authorize", &params).ok().map(String::from)
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
    #[serde(default)]
    scope: Vec<String>,
}

/// Twitch's own words from an error body, or the status.
async fn twitch_error(res: bot_core::reqwest::Response) -> String {
    helix_error(res).await.msg
}

/// A Helix call Twitch refused, with its status so callers can tell "gone" from "try again".
#[derive(Debug)]
struct HelixError {
    status: StatusCode,
    msg: String,
}

impl std::fmt::Display for HelixError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for HelixError {}

async fn helix_error(res: bot_core::reqwest::Response) -> HelixError {
    let status = res.status();
    let body: Value = res.json().await.unwrap_or(Value::Null);
    let msg = body["message"].as_str().unwrap_or_default();
    let msg = if msg.is_empty() { format!("Twitch answered {status}") } else { format!("Twitch said: {}", text::clip(msg, 200)) };
    HelixError { status, msg }
}

/// Finish the sign-in: trade the code for tokens, look up who signed in, and store it.
pub async fn connect(ctx: &Ctx, guild: u64, code: &str) -> Result<String> {
    let (id, secret) = app(ctx).context("Twitch isn't set up on this bot")?;
    let res = ctx
        .web
        .post(TOKEN_URL)
        .form(&[("client_id", id), ("client_secret", secret), ("code", code), ("grant_type", "authorization_code"), ("redirect_uri", &redirect_uri(ctx))])
        .send()
        .await?;
    if !res.status().is_success() {
        bail!(twitch_error(res).await);
    }
    let t: Tokens = res.json().await?;
    let me: Value = ctx
        .web
        .get(format!("{HELIX}/users"))
        .bearer_auth(&t.access_token)
        .header("Client-Id", id)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let user = &me["data"][0];
    let (bid, login) = (user["id"].as_str().unwrap_or_default().to_owned(), user["display_name"].as_str().or(user["login"].as_str()).unwrap_or_default().to_owned());
    if bid.is_empty() {
        bail!("Twitch didn't say who signed in");
    }
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO reviews_twitch (guild_id, broadcaster_id, login, access_token, refresh_token, expires_at, scopes) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (guild_id) DO UPDATE SET broadcaster_id = excluded.broadcaster_id, login = excluded.login, access_token = excluded.access_token,
             refresh_token = excluded.refresh_token, expires_at = excluded.expires_at, scopes = excluded.scopes, synced_at = NULL, error = NULL",
            params![guild as i64, bid, login, t.access_token, t.refresh_token, now_ms() + t.expires_in * 1000, t.scope.join(" ")],
        )
    })?;
    Ok(login)
}

/// Forget the sign-in, and ask Twitch to revoke it too. Twitch revokes by access token, so an
/// expired one is refreshed first; revoking it ends the sign-in, refresh token included.
pub async fn disconnect(ctx: &Ctx, guild: u64) -> Result<()> {
    if let (Some(c), Some((id, _))) = (conn(ctx, guild)?, app(ctx))
        && c.connected()
    {
        let access = match token(ctx, guild, false).await {
            Ok((t, _)) => Some(t),
            Err(_) => c.access_token,
        };
        if let Some(t) = access {
            let _ = ctx.web.post("https://id.twitch.tv/oauth2/revoke").form(&[("client_id", id), ("token", t.as_str())]).send().await;
        }
    }
    ctx.db.with(|c| c.execute("DELETE FROM reviews_twitch WHERE guild_id = ?1", [guild as i64]))?;
    Ok(())
}

pub fn note_error(ctx: &Ctx, guild: u64, error: Option<&str>) {
    let _ = ctx.db.with(|c| c.execute("UPDATE reviews_twitch SET error = ?2 WHERE guild_id = ?1", params![guild as i64, error]));
}

/// A token good for the next minute, refreshed when it isn't. A refresh Twitch refuses means
/// the broadcaster took the bot's access away: the tokens are dropped and the page says so.
async fn token(ctx: &Ctx, guild: u64, force: bool) -> Result<(String, String)> {
    let c = conn(ctx, guild)?.context("Twitch isn't connected")?;
    let Some(refresh) = c.refresh_token.clone() else { bail!("Twitch isn't connected") };
    if !force
        && let Some(t) = c.access_token.clone()
        && c.expires_at > now_ms() + 60_000
    {
        return Ok((t, c.broadcaster_id));
    }
    let (id, secret) = app(ctx).context("Twitch isn't set up on this bot")?;
    let res = ctx.web.post(TOKEN_URL).form(&[("client_id", id), ("client_secret", secret), ("grant_type", "refresh_token"), ("refresh_token", refresh.as_str())]).send().await?;
    if matches!(res.status(), StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED) {
        ctx.db.with(|c| {
            c.execute(
                "UPDATE reviews_twitch SET access_token = NULL, refresh_token = NULL, error = 'Twitch no longer accepts the bot''s sign-in. Connect again.' WHERE guild_id = ?1",
                [guild as i64],
            )
        })?;
        ctx.notify_once(
            guild,
            crate::KEY,
            "twitch",
            "Twitch signed the bot out. Connect again on Account reviews > Settings; channel points and follower counts stop until then.",
            Some(format!("/g/{guild}/reviews/settings")),
            std::time::Duration::from_secs(86_400),
        );
        bail!("Twitch no longer accepts the bot's sign-in. Connect again.");
    }
    if !res.status().is_success() {
        bail!(twitch_error(res).await);
    }
    let t: Tokens = res.json().await?;
    ctx.db.with(|c| {
        c.execute(
            "UPDATE reviews_twitch SET access_token = ?2, refresh_token = ?3, expires_at = ?4, scopes = ?5 WHERE guild_id = ?1",
            params![guild as i64, t.access_token, t.refresh_token, now_ms() + t.expires_in * 1000, t.scope.join(" ")],
        )
    })?;
    Ok((t.access_token, c.broadcaster_id))
}

/// Servers whose connection signed in as `broadcaster_id` and can still be refreshed.
fn guilds_signed_in_as(ctx: &Ctx, broadcaster_id: &str) -> Vec<u64> {
    ctx.db
        .with(|c| {
            let mut s = c.prepare_cached("SELECT guild_id FROM reviews_twitch WHERE broadcaster_id = ?1 AND refresh_token IS NOT NULL ORDER BY guild_id")?;
            s.query_map([broadcaster_id], |r| r.get::<_, i64>(0))?.collect::<bot_core::rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default()
        .into_iter()
        .map(|g| g as u64)
        .collect()
}

/// Whether any server connected Twitch here as `broadcaster_id`, so [`user_token_for`] has a
/// sign-in to use.
pub fn signed_in_as(ctx: &Ctx, broadcaster_id: &str) -> bool {
    !broadcaster_id.is_empty() && !guilds_signed_in_as(ctx, broadcaster_id).is_empty()
}

/// The broadcaster's own access token, from any server whose connection signed in as
/// `broadcaster_id`, refreshed when it's about to expire. For other features that read what
/// Twitch only shows a user token, like the follower total. A refresh Twitch refuses drops
/// that sign-in as it does for the queue, and the settings page asks to connect again. The
/// token is never logged.
pub async fn user_token_for(ctx: &Ctx, broadcaster_id: &str) -> Option<String> {
    if ctx.cfg.preview || broadcaster_id.is_empty() {
        return None;
    }
    for guild in guilds_signed_in_as(ctx, broadcaster_id) {
        match token(ctx, guild, false).await {
            Ok((t, _)) => return Some(t),
            Err(e) => bot_core::tracing::warn!(guild, error = %e, "couldn't use the broadcaster's Twitch sign-in"),
        }
    }
    None
}

/// A Helix call as the broadcaster. `broadcaster_id` is added to the query. A 401 refreshes
/// the token and tries once more.
async fn helix(ctx: &Ctx, guild: u64, method: Method, path: &str, query: &[(&str, &str)], body: Option<Value>) -> Result<Value> {
    let client_id = app(ctx).map(|a| a.0).context("Twitch isn't set up on this bot")?;
    for attempt in 0..2 {
        let (token, bid) = token(ctx, guild, attempt > 0).await?;
        let mut req = ctx.web.request(method.clone(), format!("{HELIX}/{path}")).bearer_auth(&token).header("Client-Id", client_id).query(&[("broadcaster_id", bid.as_str())]).query(query);
        if let Some(b) = &body {
            req = req.json(b);
        }
        let res = req.send().await?;
        if res.status() == StatusCode::UNAUTHORIZED && attempt == 0 {
            continue;
        }
        if !res.status().is_success() {
            return Err(helix_error(res).await.into());
        }
        return Ok(if res.status() == StatusCode::NO_CONTENT { Value::Null } else { res.json().await.unwrap_or(Value::Null) });
    }
    Err(anyhow!("Twitch refused the bot's sign-in"))
}

#[derive(Debug, Clone)]
pub struct Reward {
    pub id: String,
    pub title: String,
    pub cost: i64,
    pub enabled: bool,
}

fn reward(v: &Value) -> Reward {
    Reward {
        id: v["id"].as_str().unwrap_or_default().to_owned(),
        title: v["title"].as_str().unwrap_or_default().to_owned(),
        cost: v["cost"].as_i64().unwrap_or_default(),
        enabled: v["is_enabled"].as_bool().unwrap_or(true),
    }
}

/// The channel's rewards this Twitch app made: the only ones whose redemptions it can read.
pub async fn rewards(ctx: &Ctx, guild: u64) -> Result<Vec<Reward>> {
    if ctx.cfg.preview {
        return Ok(vec![Reward { id: "preview-reward".into(), title: "Account review".into(), cost: 25_000, enabled: true }]);
    }
    let v = helix(ctx, guild, Method::GET, "channel_points/custom_rewards", &[("only_manageable_rewards", "true")], None).await?;
    Ok(v["data"].as_array().map(|a| a.iter().map(reward).collect()).unwrap_or_default())
}

pub async fn create_reward(ctx: &Ctx, guild: u64, title: &str, cost: i64) -> Result<Reward> {
    let body = json!({
        "title": title,
        "cost": cost,
        "prompt": "The Idleon account to review, and anything we should look at",
        "is_user_input_required": true,
        "is_enabled": true,
    });
    let v = helix(ctx, guild, Method::POST, "channel_points/custom_rewards", &[], Some(body)).await?;
    let r = reward(&v["data"][0]);
    if r.id.is_empty() {
        bail!("Twitch didn't send the new reward back");
    }
    Ok(r)
}

/// Turn unfulfilled redemptions of `reward_id` into requests, and retry the fulfils and
/// cancels Twitch didn't take before. Reads newest first, so a new redemption always arrives
/// however many old ones wait, and adds them oldest first. A viewer reviewed within
/// `wait_ms` goes on the waitlist.
pub async fn sync(ctx: &Ctx, guild: u64, reward_id: &str, wait_ms: i64) -> Result<Tally> {
    let _ = ctx.db.with(|c| c.execute("UPDATE reviews_twitch SET synced_at = ?2 WHERE guild_id = ?1", params![guild as i64, now_ms()]));
    let mut redemptions = Vec::new();
    let mut cursor = String::new();
    for _ in 0..SYNC_PAGES {
        let mut query = vec![("reward_id", reward_id), ("status", "UNFULFILLED"), ("sort", "NEWEST"), ("first", "50")];
        if !cursor.is_empty() {
            query.push(("after", cursor.as_str()));
        }
        let mut v = helix(ctx, guild, Method::GET, "channel_points/custom_rewards/redemptions", &query, None).await?;
        if let Value::Array(page) = v["data"].take() {
            redemptions.extend(page);
        }
        cursor = v["pagination"]["cursor"].as_str().unwrap_or_default().to_owned();
        if cursor.is_empty() {
            break;
        }
    }
    let mut tally = Tally::default();
    for r in redemptions.iter().rev() {
        let Some(req) = request_from(r, reward_id) else { continue };
        tally.count(store::add(ctx, guild, &req, wait_ms)?);
    }
    let retry = store::pending_twitch(ctx, guild)?;
    if settle(ctx, guild, &retry).await == 0 {
        note_error(ctx, guild, None);
    }
    Ok(tally)
}

/// A queue request from one redemption: the account typed in, or the viewer's name.
fn request_from(r: &Value, reward_id: &str) -> Option<Request> {
    let id = r["id"].as_str().filter(|s| !s.is_empty())?;
    let viewer = r["user_name"].as_str().or(r["user_login"].as_str()).unwrap_or_default();
    let login = r["user_login"].as_str().unwrap_or(viewer);
    let at = r["redeemed_at"].as_str().and_then(|t| t.parse::<bot_core::jiff::Timestamp>().ok()).map(|t| t.as_millisecond()).unwrap_or_else(now_ms);
    let (who, extra) = store::parse_input(r["user_input"].as_str().unwrap_or_default());
    let name = if who.is_empty() { viewer.to_owned() } else { text::clip(&who, 50) };
    if name.is_empty() {
        return None;
    }
    let mut notes = Vec::new();
    if !who.is_empty() && !who.eq_ignore_ascii_case(viewer) && !who.eq_ignore_ascii_case(login) {
        notes.push(format!("Redeemed by {viewer} on Twitch"));
    }
    if !extra.is_empty() {
        notes.push(extra);
    }
    Some(Request {
        name,
        twitch_user: Some(login.to_owned()),
        reward_id: Some(reward_id.to_owned()),
        redemption_id: Some(id.to_owned()),
        redeemed_at: Some(at),
        requested_at: at,
        notes: text::clip(&notes.join("\n"), 500),
        source: "twitch",
        ..Request::default()
    })
}

/// Set redemptions on Twitch: FULFILLED keeps the points spent, CANCELED gives them back.
/// Either way they leave Twitch's queue. One Twitch no longer has as unfulfilled is settled
/// too. One that fails stays pending for the next sync, and the page shows why. Returns how
/// many failed.
pub async fn settle(ctx: &Ctx, guild: u64, list: &[Redemption]) -> usize {
    let mut failed = 0;
    for r in list {
        let res = if ctx.cfg.preview {
            Ok(Value::Null)
        } else {
            helix(ctx, guild, Method::PATCH, "channel_points/custom_rewards/redemptions", &[("id", &r.id), ("reward_id", &r.reward_id)], Some(json!({ "status": r.status }))).await
        };
        match res {
            Err(e) if !e.downcast_ref::<HelixError>().is_some_and(|h| h.status == StatusCode::NOT_FOUND) => {
                failed += 1;
                bot_core::tracing::warn!(guild, error = %e, "couldn't set a redemption's status");
                let what = if r.status == "CANCELED" { "cancel" } else { "fulfil" };
                note_error(ctx, guild, Some(&format!("Couldn't {what} a channel points redemption; the bot tries again at the next sync. {e}")));
            }
            _ => {
                let _ = store::twitch_settled(ctx, guild, &r.id);
            }
        }
    }
    failed
}

/// Servers due for their automatic sync: connected, with a reward, and not synced for 5 min.
pub fn due(ctx: &Ctx, guild: u64, now: i64) -> bool {
    conn(ctx, guild).ok().flatten().is_some_and(|c| c.connected() && c.can_manage() && c.synced_at.is_none_or(|t| now - t >= SYNC_EVERY_MS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redemptions_become_requests() {
        let r = json!({"id": "rd1", "user_login": "pavel_tv", "user_name": "Pavel_TV", "user_input": "review for Mira - check alchemy", "redeemed_at": "2026-10-30T20:00:00Z"});
        let q = request_from(&r, "rw").unwrap();
        assert_eq!(q.name, "Mira");
        assert_eq!(q.twitch_user.as_deref(), Some("pavel_tv"));
        assert_eq!(q.notes, "Redeemed by Pavel_TV on Twitch\ncheck alchemy");
        assert_eq!(q.redeemed_at, Some(1_793_390_400_000));
        // Nothing typed: the viewer's own name, and no note.
        let q = request_from(&json!({"id": "rd2", "user_login": "noor", "user_name": "Noor", "user_input": ""}), "rw").unwrap();
        assert_eq!((q.name.as_str(), q.notes.as_str()), ("Noor", ""));
        assert!(request_from(&json!({"user_login": "x"}), "rw").is_none());
    }

    #[test]
    fn scopes_are_checked() {
        let c = Conn { login: "a".into(), broadcaster_id: "1".into(), access_token: None, refresh_token: Some("r".into()), expires_at: 0, scopes: "user:read:email channel:manage:redemptions".into(), synced_at: None, error: None };
        assert!(c.connected() && c.can_manage());
        let c = Conn { scopes: "channel:read:redemptions".into(), ..c };
        assert!(!c.can_manage());
    }
}
