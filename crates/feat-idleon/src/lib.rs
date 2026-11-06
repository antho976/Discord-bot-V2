//! Idleon account check: paste your save and see how far each mechanic is from the game's own
//! maxes, and what to do next.
//!
//! A public page per server (`/p/{guild}/idleon`), like the pet pages. The save is read and
//! answered in the same request and never stored. The maxes, names and pictures come from the
//! game itself (`tools/idleon-extract`), so they follow game updates.

mod checks;
mod game;
mod page;
mod pictures;
pub mod review;
pub mod save;

use std::sync::LazyLock;

use bot_core::axum::Router;
use bot_core::axum::extract::{DefaultBodyLimit, Path, State};
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

/// A paste is URL-encoded on the way in, which can triple JSON's size.
const BODY_LIMIT: usize = save::MAX_BYTES * 3;

/// Reviews read at once. A save is a few MB in memory while it's read; the rest wait their turn
/// briefly or are told to try again.
static READING: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(4));

/// Longest problem report, like a suggestion.
const REPORT_MAX: usize = 1500;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Closed test: only signed-in staff and members with `testers_role` can open the page,
    /// and it says it's a test.
    pub closed: bool,
    pub testers_role: Option<u64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { closed: true, testers_role: None }
    }
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
            .route("/p/{guild}/idleon", get(form).post(check).layer(DefaultBodyLimit::max(BODY_LIMIT)))
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
                (ui::field_role(&ctx, guild, "testers_role", "Testers", Some("Members with this role can use the page while it's closed. Staff always can."), s.testers_role))
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

async fn check(State(ctx): State<Ctx>, viewer: MaybeUser, Path(guild): Path<u64>, form: FormData) -> WebResult {
    open(&ctx, guild)?;
    let a = access(&ctx, guild, &viewer);
    if !a.allowed {
        return Ok(page::render(&ctx, guild, &a, page::State::Ask { error: None }).into_response());
    }
    let Ok(_slot) = READING.try_acquire() else {
        return Ok(page::render(&ctx, guild, &a, page::State::Ask { error: Some("A lot of saves are being read right now. Try again in a few seconds.") }).into_response());
    };
    let text = form.text("save");
    if text.is_empty() {
        return Ok(page::render(&ctx, guild, &a, page::State::Ask { error: Some("Paste your save first.") }).into_response());
    }
    let review = match save::Save::parse(&text) {
        Ok(s) => review::Review::new(&s, checks::all(&s), checks::world_reached(&s)),
        Err(e) => return Ok(page::render(&ctx, guild, &a, page::State::Ask { error: Some(e.message()) }).into_response()),
    };
    Ok(page::render(&ctx, guild, &a, page::State::Done(&review)).into_response())
}

/// "Report a problem": goes to the server's suggestion box as the signed-in member, marked as an
/// account check report. Answers a line of text for the page to show.
async fn report(State(ctx): State<Ctx>, viewer: MaybeUser, Path(guild): Path<u64>, form: FormData) -> WebResult {
    open(&ctx, guild)?;
    let a = access(&ctx, guild, &viewer);
    let Some(user) = viewer.0.as_ref().filter(|_| a.allowed) else {
        return Ok((bot_core::axum::http::StatusCode::FORBIDDEN, "Sign in with Discord first, then send it again.").into_response());
    };
    let what: String = form.text("text").trim().chars().take(REPORT_MAX).collect();
    if what.is_empty() {
        return Ok((bot_core::axum::http::StatusCode::BAD_REQUEST, "Write what looks wrong first.").into_response());
    }
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
