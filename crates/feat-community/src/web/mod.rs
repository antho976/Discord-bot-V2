//! Dashboard pages: giveaways, polls and suggestions.

mod giveaways;
mod polls;
mod suggestions;

use bot_core::axum::Router;
use bot_core::axum::extract::Path;
use bot_core::axum::response::Redirect;
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, html};
use bot_core::web::ui;
use bot_core::web::{Page, WebError};
use bot_core::Ctx;

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/community", get(|Path(guild): Path<u64>| async move { Redirect::to(&format!("/g/{guild}/community/giveaways")) }))
        .route("/g/{guild}/community/giveaways", get(giveaways::page).post(giveaways::create))
        .route("/g/{guild}/community/giveaways/{id}/end", post(giveaways::end))
        .route("/g/{guild}/community/giveaways/{id}/reroll", post(giveaways::reroll))
        .route("/g/{guild}/community/polls", get(polls::page).post(polls::create))
        .route("/g/{guild}/community/polls/{id}/end", post(polls::end))
        .route("/g/{guild}/community/suggestions", get(suggestions::page).post(suggestions::save_settings))
        .route("/g/{guild}/community/suggestions/{id}/status", post(suggestions::set_status))
}

/// A channel that belongs to this server, from a form field.
fn channel_in(ctx: &Ctx, guild: u64, id: Option<u64>) -> Result<u64, WebError> {
    let id = id.ok_or_else(|| WebError::Bad("Pick a channel.".into()))?;
    if ctx.text_channels(guild).iter().any(|(c, _)| *c == id) {
        Ok(id)
    } else {
        Err(WebError::Bad("That channel isn't in this server.".into()))
    }
}

fn channel_name(ctx: &Ctx, guild: u64, id: u64) -> String {
    ctx.text_channels(guild).into_iter().find(|(c, _)| *c == id).map(|(_, n)| format!("#{n}")).unwrap_or_else(|| "Deleted channel".into())
}

fn role_name(ctx: &Ctx, guild: u64, id: u64) -> String {
    ctx.roles(guild).into_iter().find(|(r, _, _)| *r == id).map(|(_, n, _)| format!("@{n}")).unwrap_or_else(|| "Deleted role".into())
}

/// A channel picker with no "None" choice, for forms that must post somewhere.
fn channel_select(ctx: &Ctx, guild: u64, name: &str, label: &str, help: Option<&str>) -> Markup {
    let opts: Vec<(String, String)> = ctx.text_channels(guild).into_iter().map(|(id, n)| (id.to_string(), format!("#{n}"))).collect();
    let first = opts.first().map(|o| o.0.clone()).unwrap_or_default();
    ui::field_select(name, label, help, &opts, &first)
}

/// A link to the message in Discord.
fn open_link(page: &Page, channel: u64, message: Option<u64>) -> Markup {
    html! {
        @if let Some(m) = message {
            a.btn.small href=(crate::message_url(page.guild_id, channel, m)) target="_blank" rel="noopener" { "Open" }
        }
    }
}
