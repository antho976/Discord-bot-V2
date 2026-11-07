//! What the desktop app syncs with: the servers that have the account check, the tiers as this
//! bot has them now (so the owner's edits to `crowd.txt` and `expected.txt` reach every app on
//! the next deploy, no new app needed), and a Discord sign-in handed to the app for reports.
//!
//! Signing the app in: the app asks for a link code (`POST /idleon/app/link`, answered with the
//! code and a secret only the app holds), opens `/p/{guild}/idleon/link?code=...` in the browser,
//! where the person signs in with Discord as on the page and confirms the code they see in the
//! app. The app then collects a member session (`GET /idleon/app/link/{code}?secret=...`), once,
//! and sends reports to the page's own report address with it as the `msid` cookie, so every
//! rule a report from the page meets (membership, timeouts, the closed test, five an hour)
//! applies unchanged.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use bot_core::Ctx;
use bot_core::axum::Json;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::http::{HeaderMap, StatusCode, header};
use bot_core::axum::response::{IntoResponse, Response};
use bot_core::maud::html;
use bot_core::rand::RngExt as _;
use bot_core::serde_json::json;
use bot_core::web::auth::MaybeMember;
use bot_core::web::{FormData, WebError, WebResult, ui};
use serde::Deserialize;

use crate::{KEY, Settings, address, open};

/// How long a link code waits to be confirmed in the browser and collected by the app.
const LINK_TTL: Duration = Duration::from_secs(600);
/// Codes waiting at once, and from one address: anyone can ask for one.
const LINKS_MAX: usize = 1000;
const LINKS_EACH: usize = 5;
/// Letters a person can read back without mixing them up (no 0/O, 1/I/L).
const CODE_CHARS: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

struct Pending {
    secret: String,
    from: String,
    at: Instant,
    /// Set once confirmed: the member session and whose it is.
    done: Option<(String, String)>,
}

static LINKS: LazyLock<Mutex<HashMap<String, Pending>>> = LazyLock::new(Default::default);

fn links() -> std::sync::MutexGuard<'static, HashMap<String, Pending>> {
    let mut l = LINKS.lock().unwrap_or_else(PoisonError::into_inner);
    l.retain(|_, p| p.at.elapsed() < LINK_TTL);
    l
}

/// The servers with the account check on, for the app to pick from, and what it should fetch.
pub(crate) async fn info(State(ctx): State<Ctx>) -> Response {
    let guilds: Vec<_> = ctx
        .enabled_guilds(KEY)
        .into_iter()
        .map(|g| {
            let closed = ctx.settings::<Settings>(g, KEY).map(|s| s.closed).unwrap_or(false);
            json!({ "id": g.to_string(), "name": ctx.guild_name(g), "icon": ctx.guild_icon_large(g), "closed": closed })
        })
        .collect();
    let (version, _) = tiers_now();
    Json(json!({ "guilds": guilds, "tiers": version, "game": crate::game::GAME.updated }))
        .into_response()
}

/// The tier files as this bot reads them, and a version that changes when either does.
fn tiers_now() -> (String, std::sync::Arc<(String, String)>) {
    #[cfg(debug_assertions)]
    {
        let files = std::sync::Arc::new((
            crate::crowd::source().into_owned(),
            crate::expected::source().into_owned(),
        ));
        (version(&files), files)
    }
    #[cfg(not(debug_assertions))]
    {
        static NOW: LazyLock<(String, std::sync::Arc<(String, String)>)> = LazyLock::new(|| {
            let files = std::sync::Arc::new((
                crate::crowd::source().into_owned(),
                crate::expected::source().into_owned(),
            ));
            (version(&files), files)
        });
        NOW.clone()
    }
}

fn version(files: &(String, String)) -> String {
    use std::hash::{Hash, Hasher};
    // Fixed keys: the same files give the same version on every run and every deploy.
    #[allow(deprecated)]
    let mut h = std::hash::SipHasher::new();
    files.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// The tiers. An app that already has this version (`If-None-Match`) gets a 304 and no body.
pub(crate) async fn tiers(headers: HeaderMap) -> Response {
    let (version, files) = tiers_now();
    let tag = format!("\"{version}\"");
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == tag)
    {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, tag)]).into_response();
    }
    (
        [
            (header::ETAG, tag),
            (header::CACHE_CONTROL, "no-cache".to_string()),
        ],
        Json(json!({ "version": version, "crowd": files.0, "expected": files.1 })),
    )
        .into_response()
}

fn random(n: usize, chars: &[u8]) -> String {
    let mut rng = bot_core::rand::rng();
    (0..n)
        .map(|_| chars[rng.random_range(0..chars.len())] as char)
        .collect()
}

/// A new link code for an app to show, and the secret it collects the sign-in with.
pub(crate) async fn link_start(headers: HeaderMap) -> Response {
    let from = address(&headers);
    let mut l = links();
    if l.len() >= LINKS_MAX || l.values().filter(|p| p.from == from).count() >= LINKS_EACH {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "Too many sign-ins waiting. Try again in a few minutes.",
        )
            .into_response();
    }
    let code = loop {
        let c = random(8, CODE_CHARS);
        if !l.contains_key(&c) {
            break c;
        }
    };
    let secret = random(48, b"0123456789abcdef");
    l.insert(
        code.clone(),
        Pending {
            secret: secret.clone(),
            from,
            at: Instant::now(),
            done: None,
        },
    );
    Json(json!({ "code": code, "secret": secret, "expires_in": LINK_TTL.as_secs() }))
        .into_response()
}

#[derive(Deserialize)]
pub(crate) struct Collect {
    secret: String,
}

/// The app asking whether its code was confirmed: 202 while it waits, the session once (the
/// code is gone after), 404 when it expired or the secret is wrong.
pub(crate) async fn link_collect(Path(code): Path<String>, Query(q): Query<Collect>) -> Response {
    let mut l = links();
    let Some(p) = l.get(&code).filter(|p| same(&p.secret, &q.secret)) else {
        return (
            StatusCode::NOT_FOUND,
            "That sign-in expired. Start again from the app.",
        )
            .into_response();
    };
    if p.done.is_none() {
        return StatusCode::ACCEPTED.into_response();
    }
    let Some(Pending {
        done: Some((session, name)),
        ..
    }) = l.remove(&code)
    else {
        return StatusCode::ACCEPTED.into_response();
    };
    Json(json!({ "session": session, "name": name })).into_response()
}

/// Compares secrets in the same time whatever they hold.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

#[derive(Deserialize)]
pub(crate) struct LinkQuery {
    code: Option<String>,
}

fn clean_code(c: &str) -> String {
    c.trim()
        .to_ascii_uppercase()
        .chars()
        .filter(|c| CODE_CHARS.contains(&(*c as u8)))
        .take(8)
        .collect()
}

/// The browser page an app opens: sign in with Discord, then confirm the code the app shows.
pub(crate) async fn link_page(
    State(ctx): State<Ctx>,
    member: MaybeMember,
    Path(guild): Path<u64>,
    Query(q): Query<LinkQuery>,
) -> WebResult {
    open(&ctx, guild)?;
    let code = clean_code(q.code.as_deref().unwrap_or(""));
    let waiting = links().contains_key(&code);
    let here = format!("/p/{guild}/idleon/link?code={code}");
    let body = html! {
        @if !waiting {
            p { "This sign-in code expired or was already used. Start again from the app." }
        } @else if let Some(m) = &member.0 {
            p { "The account check app on your computer asked to send reports as you. Check that it shows this code:" }
            p { b style="font: 600 28px var(--mono); letter-spacing: .12em" { (code) } }
            form method="post" action={ "/p/" (guild) "/idleon/link" } {
                input type="hidden" name="code" value=(code);
                p { button.btn.primary type="submit" { "Sign the app in as " (m.name) } }
            }
            p.muted { "Only if you started this from the app yourself. It can then send reports to " (ctx.guild_name(guild)) "'s staff in your name, nothing else." }
        } @else {
            p { "Sign in with Discord to let the account check app send reports in your name." }
            p { a.btn.primary href=(bot_core::web::auth::member_sign_in(&here)) { "Sign in with Discord" } }
        }
    };
    Ok(ui::public_layout(
        "Sign in the account check app",
        Some(&ctx.guild_name(guild)),
        body,
    )
    .into_response())
}

/// The confirm button: a member session for the app to collect. A form post with the page's
/// SameSite cookie, so another site can't confirm a code for someone.
pub(crate) async fn link_confirm(
    State(ctx): State<Ctx>,
    member: MaybeMember,
    Path(guild): Path<u64>,
    form: FormData,
) -> WebResult {
    open(&ctx, guild)?;
    let Some(m) = member.0 else {
        return Err(WebError::NotFound);
    };
    let code = clean_code(form.get("code").unwrap_or(""));
    let waiting = links().get(&code).is_some_and(|p| p.done.is_none());
    let said = if waiting {
        let session =
            bot_core::web::auth::new_member_session(&ctx, m.id, &m.name, m.avatar.as_deref())?;
        match links().get_mut(&code) {
            Some(p) => {
                p.done = Some((session, m.name.clone()));
                "Done. Go back to the app: it's signed in as you."
            }
            None => "This sign-in code expired. Start again from the app.",
        }
    } else {
        "This sign-in code expired or was already used. Start again from the app."
    };
    Ok(ui::public_layout(
        "Sign in the account check app",
        Some(&ctx.guild_name(guild)),
        html! { p { (said) } },
    )
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_read_back_the_way_they_were_shown() {
        assert_eq!(clean_code(" abcd-efgh "), "ABCDEFGH");
        assert_eq!(clean_code("A0O1IL"), "A");
        assert!(same("abc", "abc") && !same("abc", "abd") && !same("abc", "abcd"));
        let c = random(8, CODE_CHARS);
        assert_eq!(clean_code(&c), c);
    }

    #[test]
    fn the_tiers_version_follows_the_files() {
        let a = version(&("a".into(), "b".into()));
        assert_eq!(a, version(&("a".into(), "b".into())));
        assert_ne!(a, version(&("a".into(), "c".into())));
    }
}
