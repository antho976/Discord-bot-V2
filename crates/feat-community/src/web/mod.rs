//! Dashboard pages: giveaways, polls and suggestions.

mod giveaways;
mod polls;
mod suggestions;

use bot_core::axum::Router;
use bot_core::axum::extract::Path;
use bot_core::axum::response::Redirect;
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::web::ui;
use bot_core::web::{Page, User, WebError};
use bot_core::{Ctx, Icon, now_ms, text};
use serde::Deserialize;

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

/// `?tab=` on the three pages. Each page reads the tab names it knows and ignores the rest.
#[derive(Deserialize)]
pub struct TabQuery {
    pub tab: Option<String>,
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

/// Sub-page tabs. Each item is `(label, href, count)`; `active` is the label of the open one.
fn tabs(items: &[(&str, String, Option<i64>)], active: &str) -> Markup {
    html! {
        nav.tabs {
            @for (label, href, count) in items {
                a.tab.on[*label == active] href=(href) {
                    (label)
                    @if let Some(n) = count { span.cm-count { (text::num(*n)) } }
                }
            }
        }
    }
}

/// A header button that opens a form in a popover, for editors only.
fn header_pop(label: &str, body: Markup) -> Markup {
    html! {
        details.pop.cm-pop {
            summary.btn.primary aria-label=(label) { (ui::icon(Icon::Plus)) span.cm-label { (label) } }
            div.pop-body { (body) }
        }
    }
}

/// A link to the message in Discord. Nothing is shown when the message isn't known.
fn open_link(page: &Page, channel: u64, message: Option<u64>, label: &str) -> Markup {
    html! {
        @if let Some(m) = message {
            a.cm-link href=(crate::message_url(page.guild_id, channel, m)) target="_blank" rel="noopener" { (label) }
        }
    }
}

/// Up to `max` overlapping member avatars.
fn face_row(ctx: &Ctx, guild: u64, users: &[u64], max: usize) -> Markup {
    let urls: Vec<String> = users.iter().map(|u| ctx.member_avatar(guild, *u)).collect();
    ui::avatars(&urls, max)
}

/// "Ends in 5 h 12 min" for a time still to come, "Ending now" once it has passed.
fn until(prefix: &str, at: i64) -> String {
    let secs = (at - now_ms()) / 1000;
    if secs <= 0 { "Ending now".to_owned() } else { format!("{prefix} {}", text::duration(secs)) }
}

/// Midnight on the first day of this month for the viewer, in epoch milliseconds.
fn month_start(user: &User, now: i64) -> i64 {
    let local = user.local(now);
    let midnight = local.date().first_of_month().at(0, 0, 0, 0);
    midnight.to_zoned(local.time_zone().clone()).map(|z| z.timestamp().as_millisecond()).unwrap_or(now)
}

pub(super) fn style() -> Markup {
    html! { style { (PreEscaped(STYLE)) } }
}

const STYLE: &str = r#"
.cm-count { margin-left: 6px; font: 12px var(--mono); color: var(--ink-3); }
.cm-pop > summary .cm-label { white-space: nowrap; }
.cm-pop .pop-body { width: min(560px, calc(100vw - 32px)); padding: 16px; }
.cm-pop .pop-body .card { padding: 0 14px; }
.cm-pop-foot { display: flex; justify-content: flex-end; }
.cm-link { display: inline-flex; align-items: center; height: 28px; padding: 0 9px; border-radius: var(--r-key); font-size: 12.5px; color: var(--ink-2); white-space: nowrap; }
.cm-link:hover { color: var(--ink); background: var(--hover); }
.cm-actions { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; }
.cm-main { min-width: 0; }
.cm-tile { display: grid; place-items: center; width: 44px; height: 44px; flex: none; border-radius: var(--r-key); background: var(--control); color: var(--ink-2); }
.cm-tile .icon { width: 20px; height: 20px; }
.cm-chip { display: inline-flex; align-items: center; gap: 6px; height: 24px; width: fit-content; padding: 0 10px; border-radius: 12px; font-size: 12px; white-space: nowrap; }
.cm-chip .icon { width: 14px; height: 14px; }
.cm-chip.warn { color: var(--waiting); background: rgba(227, 169, 72, 0.14); }
.cm-chip.data { color: var(--data); background: var(--data-wash); }
.cm-list { display: flex; flex-direction: column; }
.cm-row { display: flex; align-items: center; gap: 14px; padding: 12px 0; border-top: 1px solid var(--edge-1); min-width: 0; }
.cm-row:last-child { border-bottom: 1px solid var(--edge-1); }
.cm-row-main { display: flex; flex-direction: column; gap: 2px; flex: 1; min-width: 0; }
.cm-row-main strong { font-size: 13.5px; font-weight: 500; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.cm-row-actions { display: flex; align-items: center; gap: 6px; flex: none; }
.cm-entries { font-size: 12px; color: var(--ink-2); white-space: nowrap; text-align: right; min-width: 84px; }
.cm-winners { display: flex; align-items: center; gap: 10px; min-width: 0; max-width: 260px; font-size: 12.5px; color: var(--ink-2); }
.cm-winners > span:last-child { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.cm-also { display: flex; flex-direction: column; gap: 6px; margin-top: 26px; }
.cm-also > h2 { margin-bottom: 4px; }
.cm-hero { display: flex; align-items: center; gap: 22px; padding: 22px; border-radius: var(--r-card); background: var(--veil); border: 1px solid var(--edge-1); }
.cm-tile.big { width: 128px; height: 128px; border-radius: var(--r-card); flex: none; color: var(--ink); background: linear-gradient(160deg, var(--data-wash), var(--control)); }
.cm-tile.big .icon { width: 44px; height: 44px; }
.cm-hero-body { display: flex; flex-direction: column; align-items: flex-start; gap: 10px; flex: 1; min-width: 0; }
.cm-hero-title { font-size: 20px; font-weight: 600; letter-spacing: -0.015em; text-wrap: balance; }
.cm-hero-body .muted { font-size: 12.5px; }
.cm-faces { display: flex; align-items: center; gap: 10px; font-size: 13px; color: var(--ink-3); }
.cm-faces strong { font-weight: 500; color: var(--ink); }
.cm-hero-body .meter { width: 100%; max-width: 520px; margin-top: 2px; }
.cm-poll { display: flex; flex-direction: column; gap: 14px; padding: 20px 22px; border-radius: var(--r-card); background: var(--veil); border: 1px solid var(--edge-1); }
.cm-poll-head { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; font-size: 13px; color: var(--ink-3); }
.cm-poll-head strong { font-weight: 500; color: var(--ink-2); }
.cm-poll-head .status { margin-left: auto; }
.cm-question { font-size: 20px; font-weight: 600; letter-spacing: -0.015em; text-wrap: balance; }
.cm-answers { display: flex; flex-direction: column; gap: 6px; }
.cm-answer { display: flex; align-items: center; min-height: 40px; padding: 0 14px; border-radius: 10px; background: var(--well); border: 1px solid var(--edge-1); font-size: 13.5px; color: var(--ink-2); }
.cm-poll-foot { display: flex; align-items: center; justify-content: space-between; gap: 12px; flex-wrap: wrap; }
.cm-poll-foot > p { max-width: 62ch; }
.cm-bars { display: flex; flex-direction: column; gap: 6px; width: min(460px, 100%); }
.cm-bar-row { display: flex; align-items: center; gap: 12px; }
.cm-bar { position: relative; flex: 1; display: flex; align-items: center; justify-content: space-between; gap: 12px; height: 36px; padding: 0 12px; border-radius: 10px; background: var(--well); overflow: hidden; font-size: 13px; min-width: 0; }
.cm-bar > i { position: absolute; left: 0; top: 0; bottom: 0; background: var(--edge-2); }
.cm-bar.win > i { background: var(--data-dim); }
.cm-bar > span { position: relative; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
.cm-bar .pct { font: 12px var(--mono); color: var(--ink-2); flex: none; }
.cm-bar.win .pct { color: var(--data); }
.cm-bar-row .votes { font: 12px var(--mono); color: var(--ink-3); width: 76px; text-align: right; flex: none; }
.cm-results { width: 360px; flex: none; }
.cm-recent { display: flex; flex-direction: column; align-items: flex-start; gap: 6px; font-size: 13px; }
.cm-recent > strong { font-weight: 500; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.cm-recent .muted { font-size: 12px; }
.cm-month { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 8px; }
.cm-month > div { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.cm-month b { font-size: 24px; font-weight: 500; letter-spacing: -0.02em; line-height: 1.15; }
.cm-month span { font-size: 12px; color: var(--ink-3); }
.cm-voters { display: flex; justify-content: space-between; gap: 6px; }
.cm-voter { display: flex; flex-direction: column; align-items: center; gap: 4px; min-width: 0; font-size: 12.5px; text-align: center; }
.cm-voter .avatar { width: 44px; height: 44px; }
.cm-voter strong { font-weight: 500; max-width: 92px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.cm-voter span { font-size: 11.5px; color: var(--ink-3); }
.cm-sg { display: grid; grid-template-columns: 52px minmax(0, 1fr) auto; gap: 14px; align-items: start; padding: 14px 0; border-top: 1px solid var(--edge-1); }
.cm-sg:first-child { border-top: 0; padding-top: 4px; }
.cm-sg:last-child { border-bottom: 1px solid var(--edge-1); }
.cm-vote { display: flex; flex-direction: column; align-items: center; gap: 3px; width: 52px; padding: 8px 0; border-radius: var(--r-key); background: var(--control); font: 13px var(--mono); color: var(--ink); }
.cm-vote .icon { width: 14px; height: 14px; color: var(--ink-2); }
.cm-vote.down, .cm-vote.down .icon { color: var(--held); }
.cm-sg-title { display: block; font-size: 14px; font-weight: 500; line-height: 1.4; text-wrap: pretty; overflow-wrap: anywhere; }
.cm-sg-meta { display: flex; align-items: center; flex-wrap: wrap; gap: 4px 8px; margin-top: 5px; font-size: 12.5px; color: var(--ink-3); }
.cm-sg-meta .avatar { width: 20px; height: 20px; }
.cm-sg-meta strong { font-weight: 500; color: var(--ink-2); }
.cm-reply { display: flex; align-items: flex-start; gap: 8px; margin: 10px 0 0; padding: 2px 0 2px 12px; border-left: 2px solid var(--rule, var(--edge-3)); font-size: 13px; color: var(--ink-2); }
.cm-reply.ok { --rule: var(--live); }
.cm-reply.warn { --rule: var(--waiting); }
.cm-reply.bad { --rule: var(--held); }
.cm-reply .avatar { width: 20px; height: 20px; margin-top: 2px; }
.cm-reply strong { font-weight: 500; color: var(--ink); }
.cm-sg-side { display: flex; flex-direction: column; align-items: flex-end; gap: 8px; }
.cm-review .pop-body { width: 280px; }
.cm-review .stack { gap: 8px; }
.cm-sg-side .badge { height: 22px; }

@media (max-width: 560px) {
  .cm-pop > summary .cm-label { display: none; }
  .cm-pop > summary { padding: 0 10px; }
}

@media (max-width: 820px) {
  .cm-hero { flex-direction: column; align-items: flex-start; padding: 16px; gap: 16px; }
  .cm-tile.big { width: 96px; height: 96px; }
  .cm-row { flex-wrap: wrap; gap: 10px 14px; }
  .cm-row-main { flex-basis: calc(100% - 60px); }
  .cm-entries { min-width: 0; text-align: left; }
  .cm-row-actions { padding-left: 58px; }
  .cm-winners { max-width: none; }
  .cm-poll { padding: 16px; }
  .cm-poll-head .status { margin-left: 0; }
  .cm-results { min-width: 0; width: 100%; }
  .cm-bars { width: 100%; }
  .cm-bar-row .votes { width: 60px; }
  .cm-sg { grid-template-columns: 44px minmax(0, 1fr); gap: 12px; }
  .cm-vote { width: 44px; }
  .cm-sg-side { grid-column: 2; flex-direction: row; align-items: center; justify-content: space-between; }
  .cm-pop .pop-body { width: calc(100vw - 32px); }
}
"#;
