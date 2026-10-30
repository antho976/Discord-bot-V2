use bot_core::axum::extract::{Path, State};
use bot_core::axum::response::IntoResponse;
use bot_core::maud::html;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, text};

use super::open_link;
use crate::suggestions::{self as sg, Status};
use crate::{KEY, Settings};

fn tone(s: Status) -> Tone {
    match s {
        Status::Open => Tone::Muted,
        Status::Considering => Tone::Warn,
        Status::Approved | Status::Implemented => Tone::Ok,
        Status::Denied => Tone::Bad,
    }
}

pub async fn page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let rows = ctx.db.with(|c| sg::list(c, guild, 100))?;
    let count = |st: Status| rows.iter().filter(|(r, _)| r.status == st).count().to_string();
    let edit = page.can_edit();
    let options: Vec<(String, String)> = Status::ALL.iter().map(|st| (st.as_str().to_owned(), st.label().to_owned())).collect();

    let body = html! {
        (ui::readings(&[
            ("open", count(Status::Open)),
            ("considering", count(Status::Considering)),
            ("approved", count(Status::Approved)),
            ("denied", count(Status::Denied)),
            ("implemented", count(Status::Implemented)),
        ]))
        (ui::section("Suggestions", None, html! {
            @if rows.is_empty() {
                @if s.suggestions_channel.is_some() {
                    (ui::empty(Icon::Bulb, "No suggestions yet", "Members post them with /suggest. They show up here with their votes."))
                } @else {
                    (ui::empty(Icon::Bulb, "Suggestions are off", "Pick a channel under Settings and members can use /suggest."))
                }
            } @else {
                div.table-wrap { table.table {
                    thead { tr { th { "Suggestion" } th.num { "Up" } th.num { "Down" } th { "Status" } th {} } }
                    tbody { @for (r, t) in &rows {
                        tr {
                            td {
                                div.clip style="max-width:min(360px,48vw)" title=(r.content) { (text::clip(&r.content, 120)) }
                                span.muted.small { "#" (r.id) " · " (ctx.display_name(guild, r.author_id)) @if r.anonymous { " · posted anonymously" } }
                            }
                            td.num { (t.up) }
                            td.num { (t.down) }
                            td {
                                (ui::badge(r.status.label(), tone(r.status)))
                                @if !r.reason.is_empty() { div.muted.small.clip style="max-width:220px" title=(r.reason) { (r.reason) } }
                            }
                            td.actions {
                                (open_link(&page, r.channel_id, r.message_id))
                                @if edit {
                                    details style="display:inline-block;vertical-align:top;text-align:left" {
                                        summary.btn.small style="cursor:pointer" { "Set status" }
                                        form.stack method="post" action=(page.path(&format!("community/suggestions/{}/status", r.id))) style="margin-top:8px;width:260px" {
                                            (ui::select("status", &options, r.status.as_str()))
                                            input.input type="text" name="reason" value=(r.reason) placeholder="Reason, sent to the author" maxlength="800";
                                            button.btn.small.primary type="submit" style="align-self:flex-start" { "Save status" }
                                        }
                                    }
                                }
                            }
                        }
                    } }
                } }
            }
        }))
        (ui::section("Settings", None, ui::settings_form(&page, &page.path("community/suggestions"), ui::card(html! {
            (ui::field_channel(&ctx, guild, "suggestions_channel", "Channel", Some("Where /suggest posts. None turns suggestions off."), s.suggestions_channel))
            (ui::field_role(&ctx, guild, "staff_role", "Staff role", Some("Can review suggestions from Discord, besides anyone with Manage Messages."), s.staff_role))
            (ui::field_toggle("threads", "Open a thread", Some("Gives every suggestion a thread to talk it over in."), s.threads))
            (ui::field_toggle("anonymous", "Hide the author", Some("Suggestions show no name. The dashboard still shows who sent them."), s.anonymous))
        }))))
    };
    Ok(ui::layout(&ctx, &page, "Suggestions", None, body).into_response())
}

pub async fn save_settings(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let channel = form.id("suggestions_channel");
    if let Some(c) = channel {
        super::channel_in(&ctx, guild, Some(c))?;
    }
    let staff_role = form.id("staff_role").filter(|r| ctx.roles(guild).iter().any(|(id, _, _)| id == r));
    let s = Settings { suggestions_channel: channel, staff_role, threads: form.bool("threads"), anonymous: form.bool("anonymous") };
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed suggestion settings");
    Ok(saved(&page.path("community/suggestions")))
}

pub async fn set_status(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    match ctx.db.with(|c| sg::get(c, id))? {
        Some(s) if s.guild_id == guild => {}
        _ => return Err(WebError::NotFound),
    }
    let status = Status::parse(&form.string("status")).ok_or_else(|| WebError::Bad("Pick a status.".into()))?;
    sg::set_status(&ctx, id, status, &form.string("reason"), page.user.id).await?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Marked suggestion #{id} {}", status.label().to_lowercase()));
    Ok(saved(&page.path("community/suggestions")))
}
