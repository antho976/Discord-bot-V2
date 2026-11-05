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
use bot_core::axum::response::{IntoResponse, Redirect};
use bot_core::axum::routing::get;
use bot_core::web::{FormData, WebError, WebResult};
use bot_core::{Ctx, Feature, FeatureMeta, Group, Icon, NavItem, feature_async};
use bot_core::tokio::sync::Semaphore;

pub use checks::all as run_checks;
pub use pictures::has as has_picture;

pub const KEY: &str = "idleon";

/// A paste is URL-encoded on the way in, which can triple JSON's size.
const BODY_LIMIT: usize = save::MAX_BYTES * 3;

/// Reviews read at once. A save is a few MB in memory while it's read; the rest wait their turn
/// briefly or are told to try again.
static READING: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(4));

#[derive(Default)]
pub struct Idleon;

#[feature_async]
impl Feature for Idleon {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Account check",
            description: "A public page where members paste their Idleon save and see what to work on next.",
            group: Group::Pets,
            nav: vec![NavItem { label: "Account check", path: KEY, icon: Icon::Review, about: "Members paste their save and see how far each mechanic is from max." }],
        }
    }

    fn routes(&self) -> Router<Ctx> {
        Router::new()
            .route("/g/{guild}/idleon", get(dashboard))
            .route("/p/{guild}/idleon", get(form).post(check).layer(DefaultBodyLimit::max(BODY_LIMIT)))
            .route("/idleon/pic/{id}", get(picture))
    }
}

fn open(ctx: &Ctx, guild: u64) -> WebResult<()> {
    if !ctx.guild_ids().contains(&guild) || !ctx.enabled(guild, KEY) {
        return Err(WebError::NotFound);
    }
    Ok(())
}

/// The dashboard's page is the public one: staff use the same check members do.
async fn dashboard(State(ctx): State<Ctx>, Path(guild): Path<u64>) -> WebResult {
    open(&ctx, guild)?;
    Ok(Redirect::to(&format!("/p/{guild}/idleon")).into_response())
}

async fn form(State(ctx): State<Ctx>, Path(guild): Path<u64>) -> WebResult {
    open(&ctx, guild)?;
    Ok(page::render(&ctx, guild, page::State::Ask { error: None }).into_response())
}

async fn check(State(ctx): State<Ctx>, Path(guild): Path<u64>, form: FormData) -> WebResult {
    open(&ctx, guild)?;
    let Ok(_slot) = READING.try_acquire() else {
        return Ok(page::render(&ctx, guild, page::State::Ask { error: Some("A lot of saves are being read right now. Try again in a few seconds.") }).into_response());
    };
    let text = form.text("save");
    if text.is_empty() {
        return Ok(page::render(&ctx, guild, page::State::Ask { error: Some("Paste your save first.") }).into_response());
    }
    let review = match save::Save::parse(&text) {
        Ok(s) => review::Review::new(&s, checks::all(&s)),
        Err(e) => return Ok(page::render(&ctx, guild, page::State::Ask { error: Some(e.message()) }).into_response()),
    };
    Ok(page::render(&ctx, guild, page::State::Done(&review)).into_response())
}

async fn picture(Path(id): Path<String>) -> WebResult {
    let id = id.strip_suffix(".png").unwrap_or(&id);
    let bytes = pictures::get(id).ok_or(WebError::NotFound)?;
    Ok(([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "public, max-age=604800")], bytes).into_response())
}
