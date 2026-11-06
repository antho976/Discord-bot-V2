//! Idleon account check: paste your save and see how far each mechanic is from the game's own
//! maxes, and what to do next.
//!
//! A public page per server (`/p/{guild}/idleon`), like the pet pages. The save is read and
//! answered in the same request and never stored. The maxes, names and pictures come from the
//! game itself (`tools/idleon-extract`), so they follow game updates.

mod checks;
pub mod expected;
mod game;
mod limit;
mod page;
mod pictures;
pub mod review;
pub mod save;

use std::sync::LazyLock;

use bot_core::axum::Router;
use bot_core::axum::body::Body;
use bot_core::axum::extract::{Path, State};
use bot_core::axum::http::HeaderMap;
use bot_core::axum::http::header;
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::{get, post};
use bot_core::maud::html;
use bot_core::tokio::sync::Semaphore;
use bot_core::twilight_model::id::Id;
use bot_core::web::auth::MaybeUser;
use bot_core::web::{FormData, Level, User, WebError, WebResult, saved, ui};
use bot_core::{Ctx, Feature, FeatureMeta, Group, Icon, NavItem, feature_async};
use serde::{Deserialize, Serialize};

pub use checks::{all as run_checks, world_reached};
pub use pictures::has as has_picture;

pub const KEY: &str = "idleon";

/// The page sends the paste as plain text (the save plus "save="); a URL-encoded one (older
/// pages, scripts) can be up to three times the save's size.
const PLAIN_LIMIT: usize = save::MAX_BYTES + 16;
const BODY_LIMIT: usize = save::MAX_BYTES * 3;

/// Checks read at once. Each holds a save's worth of memory (about 15 MB at its peak) and a
/// core for ~14 ms; more wait their turn, up to `WAIT`, then are told to try again. Two keep the
/// bot's peak where it was before checks moved off the runtime threads.
static READING: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(2));
const WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Longest problem report with its header, the suggestion box's own limit.
const REPORT_MAX: usize = feat_community::SUGGESTION_MAX;

/// Open to anyone with the link by default (owner, 2026-11-06); the dashboard switch closes it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Closed test: only signed-in staff and members with `testers_role` can open the page,
    /// and it says it's a test.
    pub closed: bool,
    pub testers_role: Option<u64>,
}

#[derive(Default)]
pub struct Idleon;

#[feature_async]
impl Feature for Idleon {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Account check",
            description: "A page where members paste their Idleon save and see what to work on next.",
            group: Group::Pets,
            nav: vec![NavItem { label: "Account check", path: KEY, icon: Icon::Review, about: "Members paste their save and see how far each mechanic is from max." }],
        }
    }

    fn routes(&self) -> Router<Ctx> {
        Router::new()
            .route("/g/{guild}/idleon", get(dashboard).post(save_settings))
            .route("/p/{guild}/idleon", get(form).post(check))
            .route("/p/{guild}/idleon/report", post(report))
            .route("/idleon/pic/{id}", get(picture))
    }
}

fn open(ctx: &Ctx, guild: u64) -> WebResult<()> {
    if !ctx.guild_ids().contains(&guild) || !ctx.enabled(guild, KEY) {
        return Err(WebError::NotFound);
    }
    Ok(())
}

/// Who may use the page, and how it should say so.
fn access(ctx: &Ctx, guild: u64, viewer: &MaybeUser) -> page::Access {
    let s = ctx.settings::<Settings>(guild, KEY).map(|s| (*s).clone()).unwrap_or_default();
    let user = viewer.0.as_ref();
    let staff = user.is_some_and(|u| ctx.level(u, guild, KEY) >= Level::View);
    let tester = user.is_some_and(|u| {
        s.testers_role.is_some_and(|role| {
            match (Id::new_checked(guild), Id::new_checked(u.id), Id::new_checked(role)) {
                (Some(g), Some(m), Some(r)) => ctx.cache.member(g, m).is_some_and(|mem| mem.roles().contains(&r)),
                _ => false,
            }
        })
    });
    page::Access { closed: s.closed, allowed: !s.closed || staff || tester, signed_in: user.is_some() }
}

/// The dashboard's page: the closed-test switch and who tests, and a link to the page itself.
async fn dashboard(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let body = html! {
        (ui::settings_form(&page, &page.path(KEY), html! {
            (ui::section("Closed test", Some("While it's closed, only staff and testers can open the page, and it says it's a test with a way to report problems to the suggestion box."), ui::card(html! {
                (ui::field_toggle("closed", "Closed test", Some("Off opens the page to anyone with the link."), s.closed))
                (ui::field_role(&ctx, guild, "testers_role", "Testers", Some("Members with this role can use the page while it's closed. Staff always can. Testers sign in on the page with Discord, so they also need dashboard access (People)."), s.testers_role))
            })))
        }))
        p { a.btn href={ "/p/" (guild) "/idleon" } { "Open the account check" (ui::icon(Icon::Enter)) } }
    };
    Ok(ui::layout(&ctx, &page, "Account check", None, body).into_response())
}

async fn save_settings(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = Settings { closed: form.bool("closed"), testers_role: form.id("testers_role") };
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, if s.closed { "Kept the account check closed for testing" } else { "Opened the account check to everyone" });
    Ok(saved(&page.path(KEY)))
}

async fn form(State(ctx): State<Ctx>, viewer: MaybeUser, Path(guild): Path<u64>) -> WebResult {
    open(&ctx, guild)?;
    let a = access(&ctx, guild, &viewer);
    Ok(page::render(&ctx, guild, &a, page::State::Ask { error: None }).into_response())
}

/// The paste from the request body. The page's form is `text/plain` (`save=<the JSON>`, nothing
/// escaped), which skips percent-decoding a megabyte; a URL-encoded body still works.
fn paste(headers: &HeaderMap, body: &[u8]) -> String {
    if plain(headers) {
        let raw = String::from_utf8_lossy(body);
        let raw = raw.strip_prefix("save=").unwrap_or(&raw);
        return raw.trim().to_string();
    }
    FormData::parse(body).get("save").map(str::to_string).unwrap_or_default()
}

fn plain(headers: &HeaderMap) -> bool {
    headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|t| t.starts_with("text/plain"))
}

/// What the per-address limits count by: the client's address from X-Forwarded-For, as the
/// proxy in front writes it (Caddy, behind the Cloudflare Tunnel).
fn address(headers: &HeaderMap) -> String {
    let forwarded: Vec<&str> = headers.get_all("x-forwarded-for").iter().filter_map(|v| v.to_str().ok()).collect();
    let ip = feat_connect::domain::client_ip((!forwarded.is_empty()).then(|| forwarded.join(",")).as_deref());
    feat_connect::domain::limit_key(ip.as_deref())
}

/// The paste box again, saying what went wrong.
fn ask_again(ctx: &Ctx, guild: u64, a: &page::Access, error: &str) -> WebResult {
    Ok(page::render(ctx, guild, a, page::State::Ask { error: Some(error) }).into_response())
}

async fn check(State(ctx): State<Ctx>, viewer: MaybeUser, Path(guild): Path<u64>, headers: HeaderMap, body: Body) -> WebResult {
    open(&ctx, guild)?;
    let a = access(&ctx, guild, &viewer);
    if !a.allowed {
        return Ok(page::render(&ctx, guild, &a, page::State::Ask { error: None }).into_response());
    }
    // Counted before the paste is read, so pastes still arriving count too (see `limit`). The
    // pass goes with the work below: a visitor who leaves doesn't end what they started.
    let pass = match limit::enter(address(&headers)) {
        Ok(p) => p,
        Err(why) => return ask_again(&ctx, guild, &a, why.message()),
    };
    // Read up to the cap and no further. A plain-text paste is the save itself, so its cap is
    // the save's. Either kind is decoded before it waits in line, so a slot is never held by a
    // paste that's too big, and what waits is the paste, not its (up to three times bigger) body.
    let cap = if plain(&headers) { PLAIN_LIMIT } else { BODY_LIMIT };
    let Ok(body) = bot_core::axum::body::to_bytes(body, cap).await else {
        return ask_again(&ctx, guild, &a, save::SaveError::TooBig.message());
    };
    let text = paste(&headers, &body);
    drop(body);
    if text.is_empty() {
        return ask_again(&ctx, guild, &a, "Paste your save first.");
    }
    if text.len() > save::MAX_BYTES {
        return ask_again(&ctx, guild, &a, save::SaveError::TooBig.message());
    }
    let Ok(Ok(slot)) = bot_core::tokio::time::timeout(WAIT, READING.acquire()).await else {
        return ask_again(&ctx, guild, &a, limit::Refused::Busy.message());
    };
    // About 14 ms of CPU: run it off the bot's two runtime threads, so Discord events and other
    // pages never wait behind a check (at most two at once, `READING`).
    let html = bot_core::tokio::task::spawn_blocking(move || {
        // The slot and the pass are held by the work, not the request: a visitor who leaves
        // mid-check drops the request, but the check runs on, and must keep counting.
        let _slot = slot;
        let _pass = pass;
        let review = match save::Save::parse(&text) {
            Ok(s) => review::Review::new(&s, checks::all(&s), checks::world_reached(&s)),
            Err(e) => return page::render(&ctx, guild, &a, page::State::Ask { error: Some(e.message()) }).into_string(),
        };
        drop(text);
        page::render(&ctx, guild, &a, page::State::Done(&review)).into_string()
    })
    .await
    .map_err(|e| WebError::Internal(e.into()))?;
    give_back_memory();
    Ok(bot_core::axum::response::Html(html).into_response())
}

/// A check reads a megabyte of save into many small pieces and frees them all at once, and
/// glibc keeps freed memory for later: without this the bot stays ~15-25 MB bigger after its
/// first checks. Handing it back walks the heap (about 1 ms a time, measured), so it runs once,
/// two seconds after the last check of a burst, not after every check.
fn give_back_memory() {
    static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mine = LAST.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    bot_core::tokio::spawn(async move {
        bot_core::tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        if LAST.load(std::sync::atomic::Ordering::Relaxed) == mine {
            trim();
        }
    });
}

fn trim() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trim only releases free heap pages back to the system; it touches no live
    // allocation and is safe to call from any thread.
    unsafe {
        libc::malloc_trim(0);
    }
}

/// "Report a problem": goes to the server's suggestion box as the signed-in member, marked as an
/// account check report. Answers a line of text for the page to show.
async fn report(State(ctx): State<Ctx>, viewer: MaybeUser, Path(guild): Path<u64>, form: FormData) -> WebResult {
    open(&ctx, guild)?;
    let a = access(&ctx, guild, &viewer);
    let Some(user) = viewer.0.as_ref().filter(|_| a.allowed) else {
        return Ok((bot_core::axum::http::StatusCode::FORBIDDEN, "Sign in with Discord first, then send it again.").into_response());
    };
    let account: String = form.text("account").trim().chars().take(40).collect();
    let card: String = form.text("card").trim().chars().take(60).collect();
    let mut content = String::from("Account check report");
    if !account.is_empty() {
        content.push_str(&format!(" ({account})"));
    }
    if !card.is_empty() {
        content.push_str(&format!(", {card}"));
    }
    content.push_str(":\n");
    // The suggestion box keeps REPORT_MAX characters in all, header included: cut the member's
    // text to what's left, so nothing is lost silently off the end.
    let room = REPORT_MAX.saturating_sub(content.chars().count());
    let what: String = form.text("text").trim().chars().take(room).collect();
    if what.is_empty() {
        return Ok((bot_core::axum::http::StatusCode::BAD_REQUEST, "Write what looks wrong first.").into_response());
    }
    content.push_str(&what);
    let msg = match feat_community::submit_suggestion(&ctx, guild, user.id, &content).await {
        Ok(Some(n)) => format!("Sent to staff as suggestion #{n}. Thanks."),
        Ok(None) => "The suggestion box is off on this server, so it couldn't be sent. Tell staff directly.".to_owned(),
        Err(e) => {
            bot_core::tracing::warn!(guild, error = %e, "couldn't send an account check report");
            "Couldn't send it right now. Try again in a minute.".to_owned()
        }
    };
    Ok(msg.into_response())
}

async fn picture(Path(id): Path<String>) -> WebResult {
    let id = id.strip_suffix(".png").unwrap_or(&id);
    let bytes = pictures::get(id).ok_or(WebError::NotFound)?;
    Ok(([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "public, max-age=604800")], bytes).into_response())
}
