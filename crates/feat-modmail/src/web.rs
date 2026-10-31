use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};
use serde::Deserialize;

use crate::mail::{self, Conv};
use crate::{DEFAULT_CLOSING, DEFAULT_GREETING, KEY, Settings};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/modmail", get(page))
        .route("/g/{guild}/modmail/settings", post(save))
        .route("/g/{guild}/modmail/{id}/close", post(close))
}

fn thread_url(guild: u64, thread: u64) -> String {
    format!("https://discord.com/channels/{guild}/{thread}")
}

fn channel_name(ctx: &Ctx, guild: u64, channel: u64) -> String {
    ctx.text_channels(guild).into_iter().find(|(id, _)| *id == channel).map(|(_, n)| format!("#{n}")).unwrap_or_else(|| "a deleted channel".into())
}

#[derive(Deserialize)]
struct ViewQuery {
    view: Option<String>,
}

async fn page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<ViewQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let now = now_ms();
    let view = q.view.as_deref().unwrap_or("open");
    let stats = ctx.db.with(|c| mail::stats(c, guild, now))?;
    let rows = match view {
        "closed" => ctx.db.with(|c| mail::list_closed(c, guild, 100))?,
        "settings" => Vec::new(),
        _ => ctx.db.with(|c| mail::list_open(c, guild, 100))?,
    };
    let base = page.path("modmail");
    let active = match view {
        "closed" => "Closed",
        "settings" => "Settings",
        _ => "Open",
    };
    let tabs = ui::tabs_counted(
        &[("Open", base.clone(), Some(stats.open)), ("Closed", format!("{base}?view=closed"), None), ("Settings", format!("{base}?view=settings"), None)],
        active,
    );

    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs)
        @if view == "settings" {
            (ui::section("Settings", None, ui::settings_form(&page, &page.path("modmail/settings"), ui::card(html! {
                (ui::field_channel(&ctx, guild, "channel", "Staff channel", Some("A text channel gets a thread per conversation, a forum a post. Keep it private to staff. None turns modmail off."), s.channel))
                (ui::field_role(&ctx, guild, "staff_role", "Staff role", Some("Pinged once when a conversation opens, and allowed to close one."), s.staff_role))
                (ui::field_textarea("greeting", "Greeting", Some("Sent to the member when a conversation opens. {server} is the server's name."), &s.greeting, DEFAULT_GREETING))
                (ui::field_textarea("closing", "Closing text", Some("Sent to the member when staff close the conversation."), &s.closing, DEFAULT_CLOSING))
            }))))
        } @else {
            div.with-rail {
                div.stack.mm-main {
                    (ui::readings(&[
                        ("open now", stats.open.to_string()),
                        ("waiting on staff", stats.waiting.to_string()),
                        ("opened this week", stats.opened_week.to_string()),
                        ("closed this week", stats.closed_week.to_string()),
                    ]))
                    @if rows.is_empty() {
                        @if view == "closed" {
                            (ui::empty(Icon::Mail, "Nothing closed yet", "Closed conversations stay here with who closed them."))
                        } @else if s.channel.is_none() {
                            (ui::empty(Icon::Mail, "Modmail is off", "Pick a staff channel in Settings. Conversations show up here when members DM the bot."))
                        } @else {
                            (ui::empty(Icon::Mail, "No open conversations", "Conversations show up here when members DM the bot."))
                        }
                    } @else {
                        div.mm-list {
                            @for c in &rows { (row(&ctx, &page, guild, c, now)) }
                        }
                        @if rows.len() == 100 { p.muted.small { "Showing the latest 100." } }
                    }
                }
                aside.rail {
                    (closed_recently(&ctx, &page, guild))
                    p.muted.small {
                        @match s.channel {
                            Some(ch) => { "Conversations open in " (channel_name(&ctx, guild, ch)) },
                            None => { "No staff channel picked" },
                        }
                        @if let Some(role) = s.staff_role {
                            @if let Some((_, name, _)) = ctx.roles(guild).into_iter().find(|(id, _, _)| *id == role) { " · pings @" (name) }
                        }
                    }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Modmail", None, body).into_response())
}

fn row(ctx: &Ctx, page: &Page, guild: u64, c: &Conv, now: i64) -> Markup {
    let name = c.name(ctx, guild);
    let last = page.user.local(c.last_at).strftime("%b %-d, %H:%M").to_string();
    let staff_wrote = (!c.waiting_on_staff()).then(|| ctx.display_name(guild, c.last_author));
    html! {
        div.mm-row {
            img.avatar src=(ctx.member_avatar(guild, c.user_id)) alt="" width="36" height="36" loading="lazy";
            div.mm-body {
                strong.mm-name.clip { (name) }
                @if !c.preview.is_empty() {
                    p.mm-quote {
                        @if let Some(who) = &staff_wrote { strong { (who) } ": " }
                        "“" (c.preview) "”"
                    }
                }
                div.mm-meta {
                    (c.messages) @if c.messages == 1 { " message" } @else { " messages" }
                    " · last " span title=(last) { (text::ago(c.last_at)) }
                    @if c.is_open() {
                        " · opened " (text::ago(c.opened_at))
                    } @else {
                        " · closed " (text::ago(c.closed_at.unwrap_or(now)))
                        @match c.closed_by {
                            Some(by) => { " by " (ctx.display_name(guild, by)) },
                            None => { " in Discord" },
                        }
                    }
                }
            }
            div.mm-side {
                @if c.is_open() {
                    @if c.waiting_on_staff() {
                        (ui::badge(&format!("Waiting {}", mail::span((now - c.last_at) / 1000)), Tone::Warn))
                    } @else {
                        (ui::badge(&format!("Waiting on {name}"), Tone::Muted))
                    }
                } @else {
                    (ui::badge("Closed", Tone::Muted))
                }
                div.mm-acts {
                    a.btn.small href=(thread_url(guild, c.thread_id)) target="_blank" rel="noopener" { "Open thread" }
                    @if page.can_edit() && c.is_open() { (ui::action_button(&page.path(&format!("modmail/{}/close", c.id)), "Close", Tone::Bad, true)) }
                }
            }
        }
    }
}

/// The last three conversations to close.
fn closed_recently(ctx: &Ctx, page: &Page, guild: u64) -> Markup {
    let recent = ctx.db.with(|c| mail::list_closed(c, guild, 3)).unwrap_or_default();
    html! {
        div.rail-block {
            h2 { "Closed recently" }
            @if recent.is_empty() {
                p.muted.small { "Closed conversations show up here with who closed them." }
            }
            @for c in &recent {
                div.mini {
                    img.avatar src=(ctx.member_avatar(guild, c.user_id)) alt="" width="28" height="28" loading="lazy";
                    div.grow {
                        span.clip { (c.name(ctx, guild)) }
                        span.clip.muted.small {
                            @match c.closed_by {
                                Some(by) => { "by " (ctx.display_name(guild, by)) },
                                None => { "in Discord" },
                            }
                            " · " (c.messages) " messages"
                        }
                    }
                    @if let Some(at) = c.closed_at { span.mono title=(page.user.local(at).strftime("%b %-d, %H:%M").to_string()) { (text::ago(at)) } }
                }
            }
            @if !recent.is_empty() { a.small.muted href={ (page.path("modmail")) "?view=closed" } { "All closed" } }
        }
    }
}

async fn save(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let mut s = (*ctx.settings::<Settings>(guild, KEY)?).clone();
    s.channel = form.channel(&ctx, guild, "channel");
    s.staff_role = form.role(&ctx, guild, "staff_role");
    s.greeting = text::clip(&form.text("greeting"), 1500);
    s.closing = text::clip(&form.text("closing"), 1500);
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed modmail settings");
    Ok(saved(&page.path("modmail?view=settings")))
}

async fn close(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    if let Some(c) = ctx.db.with(|c| mail::by_id(c, guild, id))?
        && mail::close_from_dashboard(&ctx, guild, id, page.user.id).await?
    {
        ctx.audit(&page.user, Some(guild), KEY, format!("Closed the modmail conversation with {}", c.name(&ctx, guild)));
    }
    Ok(saved(&page.path("modmail")))
}

const CSS: &str = "
.mm-main { gap: 0; }
.mm-main .readings { padding-bottom: 22px; }
.mm-list { display: flex; flex-direction: column; }
.mm-row { display: grid; grid-template-columns: 36px minmax(0, 1fr) auto; gap: 14px; align-items: start; padding: 16px 0; border-top: 1px solid var(--edge-1); }
.mm-row:last-child { border-bottom: 1px solid var(--edge-1); }
.mm-body { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.mm-name { font-weight: 500; font-size: 14px; }
.mm-quote { color: var(--ink-2); font-size: 13px; line-height: 1.5; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
.mm-quote strong { color: var(--ink); font-weight: 500; }
.mm-meta { font-size: 12px; color: var(--ink-3); }
.mm-side { display: flex; flex-direction: column; align-items: flex-end; gap: 10px; }
.mm-acts { display: flex; gap: 6px; align-items: center; }
.mm-main + .rail .mini .grow { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
@media (max-width: 560px) {
  .mm-row { grid-template-columns: 32px minmax(0, 1fr); }
  .mm-row .avatar { width: 32px; height: 32px; }
  .mm-side { grid-column: 2; flex-direction: row; justify-content: space-between; align-items: center; flex-wrap: wrap; }
}
";
