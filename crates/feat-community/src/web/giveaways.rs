use bot_core::axum::extract::{Path, State};
use bot_core::axum::response::IntoResponse;
use bot_core::maud::html;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};

use super::{channel_in, channel_name, channel_select, open_link, role_name};
use crate::KEY;
use crate::giveaways::{self as gw, EndOutcome, RerollOutcome};

pub async fn page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let (running, ended) = ctx.db.with(|c| Ok((gw::list(c, guild, false, 50)?, gw::list(c, guild, true, 25)?)))?;
    let entries: i64 = running.iter().map(|(_, n)| n).sum();
    let edit = page.can_edit();

    let body = html! {
        (ui::readings(&[
            ("running", running.len().to_string()),
            ("entries so far", text::num(entries)),
            ("ended", ended.len().to_string()),
        ]))
        (ui::section("Running", None, html! {
            @if running.is_empty() {
                (ui::empty(Icon::Gift, "Nothing running", "Start one below or with /giveaway start. Members enter with a button on the message."))
            } @else {
                div.table-wrap { table.table {
                    thead { tr { th { "Prize" } th { "Ends" } th.num { "Entries" } th {} } }
                    tbody { @for (g, n) in &running {
                        tr {
                            td {
                                div.clip style="max-width:min(300px,50vw)" { (g.prize) }
                                span.muted.small {
                                    (channel_name(&ctx, guild, g.channel_id)) " · " (g.winners) @if g.winners == 1 { " winner" } @else { " winners" }
                                    " · by " (ctx.display_name(guild, g.created_by))
                                    @if let Some(r) = g.required_role { " · only " (role_name(&ctx, guild, r)) }
                                }
                            }
                            td style="white-space:nowrap" { (crate::relative(g.ends_at)) }
                            td.num { (text::num(*n)) }
                            td.actions {
                                (open_link(&page, g.channel_id, g.message_id))
                                @if edit { (ui::action_button(&page.path(&format!("community/giveaways/{}/end", g.id)), "End now", Tone::Muted, true)) }
                            }
                        }
                    } }
                } }
            }
        }))
        @if edit {
            (ui::section("Start a giveaway", Some("Posts the message with an Enter button. The winners are picked within a minute of the end time."), html! {
                form.settings method="post" action=(page.path("community/giveaways")) {
                    (ui::card(html! {
                        (ui::field_text("prize", "Prize", None, "", "Discord Nitro, 3 months"))
                        (ui::field_text("duration", "Runs for", Some("Like 30m, 2h or 1d 12h. A bare number is minutes."), "", "1d"))
                        (ui::field_number("winners", "Winners", None, 1, 1, gw::MAX_WINNERS, None))
                        (channel_select(&ctx, guild, "channel", "Channel", None))
                        (ui::field_role(&ctx, guild, "role", "Required role", Some("Only members with this role can enter."), None))
                    }))
                    div.save-bar { button.btn.primary type="submit" { "Start giveaway" } }
                }
            }))
        }
        (ui::section("Ended", None, html! {
            @if ended.is_empty() {
                (ui::empty(Icon::Trophy, "Nothing ended yet", "Finished giveaways and their winners show up here."))
            } @else {
                div.table-wrap { table.table {
                    thead { tr { th { "Prize" } th { "Winners" } th.num { "Entries" } th { "Ended" } th {} } }
                    tbody { @for (g, n) in &ended {
                        tr {
                            td { div.clip style="max-width:min(320px,50vw)" { (g.prize) } span.muted.small { (channel_name(&ctx, guild, g.channel_id)) } }
                            td {
                                @if g.winner_ids.is_empty() { span.muted { "Nobody entered" } }
                                @else { (g.winner_ids.iter().map(|u| ctx.display_name(guild, *u)).collect::<Vec<_>>().join(", ")) }
                            }
                            td.num { (text::num(*n)) }
                            td style="white-space:nowrap" { (text::ago(g.ends_at)) }
                            td.actions {
                                (open_link(&page, g.channel_id, g.message_id))
                                @if edit && *n > g.winner_ids.len() as i64 { (ui::action_button(&page.path(&format!("community/giveaways/{}/reroll", g.id)), "Reroll", Tone::Muted, false)) }
                            }
                        }
                    } }
                } }
            }
        }))
    };
    Ok(ui::layout(&ctx, &page, "Giveaways", None, body).into_response())
}

pub async fn create(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let prize = form.string("prize");
    if prize.is_empty() {
        return Err(WebError::Bad("Say what's being given away.".into()));
    }
    let secs = gw::parse_span(&form.string("duration")).map_err(|e| WebError::Bad(e.into()))?;
    let winners = form.int_in("winners", 1, gw::MAX_WINNERS, 1);
    let channel = channel_in(&ctx, guild, form.id("channel"))?;
    let role = form.id("role").filter(|r| ctx.roles(guild).iter().any(|(id, _, _)| id == r));
    let id = gw::create(&ctx, gw::New { guild, channel, prize: &prize, winners, ends_at: now_ms() + secs * 1000, role, by: page.user.id })
        .await
        .map_err(|e| {
            bot_core::tracing::warn!(guild, error = %e, "couldn't start a giveaway from the dashboard");
            WebError::Bad("Couldn't post in that channel. Check that the bot can send messages and embed links there.".into())
        })?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Started giveaway #{id}: {}", text::clip(&prize, 60)));
    Ok(saved(&page.path("community/giveaways")))
}

async fn owned(ctx: &Ctx, guild: u64, id: i64) -> Result<(), WebError> {
    match ctx.db.with(|c| gw::get(c, id))? {
        Some(g) if g.guild_id == guild => Ok(()),
        _ => Err(WebError::NotFound),
    }
}

pub async fn end(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    owned(&ctx, guild, id).await?;
    match gw::end(&ctx, id).await? {
        EndOutcome::Ended(_) => ctx.audit(&page.user, Some(guild), KEY, format!("Ended giveaway #{id}")),
        EndOutcome::AlreadyEnded | EndOutcome::Missing => {}
    }
    Ok(saved(&page.path("community/giveaways")))
}

pub async fn reroll(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    owned(&ctx, guild, id).await?;
    match gw::reroll(&ctx, id).await? {
        RerollOutcome::Rolled(_) => ctx.audit(&page.user, Some(guild), KEY, format!("Rerolled giveaway #{id}")),
        RerollOutcome::NoOneElse => return Err(WebError::Bad("Nobody else entered, so there's no one to pick.".into())),
        RerollOutcome::StillRunning | RerollOutcome::Missing => {}
    }
    Ok(saved(&page.path("community/giveaways")))
}
