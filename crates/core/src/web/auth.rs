//! Discord sign-in, sessions and per-page permissions.
//!
//! Nobody gets in unless they're on the dashboard's user list. The owner (`OWNER_ID`) is always
//! an admin. Admins see everything; everyone else gets View or Edit per page, per server.

use axum::Router;
use axum::extract::{FromRequestParts, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use maud::{DOCTYPE, html};
use rand::RngExt as _;
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{WebError, WebResult, ui};
use crate::Ctx;

const SESSION_COOKIE: &str = "sid";
const STATE_COOKIE: &str = "oauth_state";
const SESSION_DAYS: i64 = 30;

/// How much someone can do on a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    None = 0,
    View = 1,
    Edit = 2,
}

impl Level {
    pub fn from_i64(n: i64) -> Self {
        match n {
            2.. => Level::Edit,
            1 => Level::View,
            _ => Level::None,
        }
    }
}

/// A signed-in dashboard user.
#[derive(Debug, Clone)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub avatar: Option<String>,
    pub admin: bool,
}

impl User {
    pub fn avatar_url(&self) -> String {
        crate::discord::avatar_url(self.id, self.avatar.as_deref())
    }
}

/// A page being served: who is looking, which server, and what they may do there.
#[derive(Debug, Clone)]
pub struct Page {
    pub user: User,
    pub guild_id: u64,
    /// The permission key (a feature key, or a core page key).
    pub key: &'static str,
    pub level: Level,
}

impl Page {
    pub fn can_edit(&self) -> bool {
        self.level >= Level::Edit
    }

    pub fn require_edit(&self) -> WebResult<()> {
        if self.can_edit() { Ok(()) } else { Err(WebError::Forbidden) }
    }

    /// `/g/{guild}/{suffix}`
    pub fn path(&self, suffix: &str) -> String {
        if suffix.is_empty() {
            format!("/g/{}", self.guild_id)
        } else {
            format!("/g/{}/{}", self.guild_id, suffix)
        }
    }
}

impl Ctx {
    /// What `user` can do on page `key` in a server.
    pub fn level(&self, user: &User, guild_id: u64, key: &str) -> Level {
        if user.admin {
            return Level::Edit;
        }
        self.db
            .with(|c| {
                c.query_row(
                    "SELECT level FROM dash_perms WHERE user_id = ?1 AND guild_id = ?2 AND page = ?3",
                    params![user.id as i64, guild_id as i64, key],
                    |r| r.get::<_, i64>(0),
                )
                .optional()
            })
            .ok()
            .flatten()
            .map(Level::from_i64)
            .unwrap_or(Level::None)
    }

    /// Open a page for a user, or refuse when they lack `need` on it.
    pub fn page(&self, user: &User, guild_id: u64, key: &'static str, need: Level) -> WebResult<Page> {
        if !self.guild_ids().contains(&guild_id) {
            return Err(WebError::NotFound);
        }
        let level = self.level(user, guild_id, key);
        if level < need {
            return Err(WebError::Forbidden);
        }
        Ok(Page { user: user.clone(), guild_id, key, level })
    }

    /// Servers this user can open anything in.
    pub fn guilds_for(&self, user: &User) -> Vec<u64> {
        let all = self.guild_ids();
        if user.admin {
            return all;
        }
        let granted: Vec<u64> = self
            .db
            .with(|c| {
                let mut s = c.prepare("SELECT DISTINCT guild_id FROM dash_perms WHERE user_id = ?1 AND level > 0")?;
                let rows = s.query_map([user.id as i64], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .unwrap_or_default()
            .into_iter()
            .map(|g| g as u64)
            .collect();
        all.into_iter().filter(|g| granted.contains(g)).collect()
    }
}

impl FromRequestParts<Ctx> for User {
    type Rejection = WebError;

    async fn from_request_parts(parts: &mut Parts, ctx: &Ctx) -> Result<Self, Self::Rejection> {
        let token = cookie(&parts.headers, SESSION_COOKIE).ok_or(WebError::Login)?;
        let hash = hash_token(&token);
        let user = ctx.db.with(|c| {
            c.query_row(
                "SELECT u.id, u.name, u.avatar, u.admin FROM dash_sessions s JOIN dash_users u ON u.id = s.user_id
                 WHERE s.token_hash = ?1 AND s.expires_at > ?2",
                params![hash, crate::now_ms()],
                |r| {
                    Ok(User {
                        id: r.get::<_, i64>(0)? as u64,
                        name: r.get(1)?,
                        avatar: r.get(2)?,
                        admin: r.get(3)?,
                    })
                },
            )
            .optional()
        })?;
        let mut user = user.ok_or(WebError::Login)?;
        if user.id == ctx.cfg.owner_id {
            user.admin = true;
        }
        Ok(user)
    }
}

/// Like [`User`] but doesn't redirect when nobody is signed in. For public pages.
pub struct MaybeUser(pub Option<User>);

impl FromRequestParts<Ctx> for MaybeUser {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, ctx: &Ctx) -> Result<Self, Self::Rejection> {
        Ok(MaybeUser(User::from_request_parts(parts, ctx).await.ok()))
    }
}

pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_owned())
}

fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn random_token() -> String {
    let bytes: [u8; 32] = rand::rng().random();
    hex::encode(bytes)
}

fn set_cookie(ctx: &Ctx, name: &str, value: &str, max_age_secs: i64) -> String {
    let secure = if ctx.cfg.secure_cookies() { "; Secure" } else { "" };
    format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_secs}{secure}")
}

pub(super) fn routes(preview: bool) -> Router<Ctx> {
    let r = Router::new();
    let r = if preview { r.route("/dev/login", get(preview_login)) } else { r };
    r
        .route("/login", get(login_page))
        .route("/auth/discord", get(start))
        .route("/auth/callback", get(callback))
        .route("/logout", post(logout))
}

#[derive(Deserialize)]
struct PreviewQuery {
    to: Option<String>,
}

/// Preview only: sign in as the owner without Discord.
async fn preview_login(State(ctx): State<Ctx>, Query(q): Query<PreviewQuery>) -> WebResult {
    let session = random_token();
    let now = crate::now_ms();
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO dash_sessions (token_hash, user_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
            params![hash_token(&session), ctx.cfg.owner_id as i64, now, now + 86_400_000],
        )
    })?;
    let to = q.to.filter(|t| t.starts_with('/')).unwrap_or_else(|| "/".into());
    Ok(([(header::SET_COOKIE, set_cookie(&ctx, SESSION_COOKIE, &session, 86_400))], Redirect::to(&to)).into_response())
}

async fn login_page() -> Response {
    let body = html! {
        (DOCTYPE)
        html lang="en" {
            head { (ui::head("Sign in")) }
            body.bare {
                main.notice {
                    img.mark src="/static/icon.svg" alt="" width="36" height="36";
                    h1 { "Bot dashboard" }
                    p { "Sign in with the Discord account the owner added." }
                    a.btn.primary href="/auth/discord" { "Sign in with Discord" }
                }
            }
        }
    };
    body.into_response()
}

async fn start(State(ctx): State<Ctx>) -> Response {
    let state = random_token();
    let url = format!(
        "https://discord.com/oauth2/authorize?response_type=code&client_id={}&scope=identify&prompt=none&state={}&redirect_uri={}",
        ctx.cfg.discord_client_id,
        state,
        encode(&redirect_uri(&ctx)),
    );
    ([(header::SET_COOKIE, set_cookie(&ctx, STATE_COOKIE, &state, 600))], Redirect::to(&url)).into_response()
}

fn redirect_uri(ctx: &Ctx) -> String {
    format!("{}/auth/callback", ctx.cfg.public_url)
}

fn encode(s: &str) -> String {
    form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct DiscordMe {
    id: String,
    username: String,
    global_name: Option<String>,
    avatar: Option<String>,
}

async fn callback(State(ctx): State<Ctx>, headers: HeaderMap, Query(q): Query<CallbackQuery>) -> WebResult {
    let expected = cookie(&headers, STATE_COOKIE);
    let (Some(code), Some(state)) = (q.code, q.state) else { return Ok(Redirect::to("/login").into_response()) };
    if expected.as_deref() != Some(state.as_str()) {
        return Err(WebError::Bad("The sign-in link expired. Try again.".into()));
    }
    let token: TokenResponse = ctx
        .web
        .post("https://discord.com/api/v10/oauth2/token")
        .basic_auth(&ctx.cfg.discord_client_id, Some(&ctx.cfg.discord_client_secret))
        .form(&[("grant_type", "authorization_code"), ("code", code.as_str()), ("redirect_uri", redirect_uri(&ctx).as_str())])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let me: DiscordMe = ctx
        .web
        .get("https://discord.com/api/v10/users/@me")
        .bearer_auth(&token.access_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let id: u64 = me.id.parse().map_err(|_| WebError::Bad("Discord sent an odd account id.".into()))?;
    let name = me.global_name.unwrap_or(me.username);
    let is_owner = id == ctx.cfg.owner_id;

    let known = ctx.db.with(|c| {
        if is_owner {
            c.execute(
                "INSERT INTO dash_users (id, name, avatar, admin, created_at) VALUES (?1, ?2, ?3, 1, ?4)
                 ON CONFLICT (id) DO UPDATE SET admin = 1",
                params![id as i64, name, me.avatar, crate::now_ms()],
            )?;
        }
        let n = c.execute(
            "UPDATE dash_users SET name = ?2, avatar = ?3 WHERE id = ?1",
            params![id as i64, name, me.avatar],
        )?;
        Ok(n > 0)
    })?;

    if !known {
        let body = html! {
            (DOCTYPE)
            html lang="en" {
                head { (ui::head("No access yet")) }
                body.bare {
                    main.notice {
                        h1 { "You're not on the list yet" }
                        p { "Signed in as " strong { (name) } ". Send the owner your Discord ID so they can add you:" }
                        p.mono.copyable { (id) }
                        a.btn href="/login" { "Back" }
                    }
                }
            }
        };
        return Ok(body.into_response());
    }

    let session = random_token();
    let now = crate::now_ms();
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO dash_sessions (token_hash, user_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
            params![hash_token(&session), id as i64, now, now + SESSION_DAYS * 86_400_000],
        )
    })?;
    let mut res = Redirect::to("/").into_response();
    let h = res.headers_mut();
    h.append(header::SET_COOKIE, set_cookie(&ctx, SESSION_COOKIE, &session, SESSION_DAYS * 86_400).parse().unwrap());
    h.append(header::SET_COOKIE, set_cookie(&ctx, STATE_COOKIE, "", 0).parse().unwrap());
    Ok(res)
}

async fn logout(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    if let Some(token) = cookie(&headers, SESSION_COOKIE) {
        let _ = ctx.db.with(|c| c.execute("DELETE FROM dash_sessions WHERE token_hash = ?1", [hash_token(&token)]));
    }
    ([(header::SET_COOKIE, set_cookie(&ctx, SESSION_COOKIE, "", 0))], Redirect::to("/login")).into_response()
}
