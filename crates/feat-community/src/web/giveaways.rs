use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::maud::{Markup, html};
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};

use super::{TabQuery, channel_in, channel_name, channel_select, face_row, header_pop, month_start, open_link, role_name, style, tabs, until};
use crate::KEY;
use crate::giveaways::{self as gw, EndOutcome, Giveaway, RerollOutcome};

pub async fn page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<TabQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let now = now_ms();
    let edit = page.can_edit();
    let ended_tab = q.tab.as_deref() == Some("ended");
    let since = month_start(&page.user, now);
    let (running, ended, (n_running, _), totals, faces) = ctx.db.with(|c| {
        let running = gw::list(c, guild, false, 50)?;
        let faces = running.iter().map(|(g, _)| gw::newest_entrants(c, g.id, 6)).collect::<bot_core::rusqlite::Result<Vec<_>>>()?;
        Ok((running, gw::list(c, guild, true, 25)?, gw::counts(c, guild)?, gw::since_totals(c, guild, since)?, faces))
    })?;
    let winners: Vec<(u64, &Giveaway, i64)> = ended.iter().flat_map(|(g, n)| g.winner_ids.iter().map(move |w| (*w, g, *n))).take(5).collect();

    let main = if ended_tab {
        ended_list(&ctx, &page, guild, &ended, edit)
    } else {
        running_view(&ctx, &page, guild, &running, &faces, edit)
    };
    let actions = edit.then(|| header_pop("Start a giveaway", start_form(&ctx, &page, guild)));

    let body = html! {
        (style())
        (tabs(
            &[("Running", page.path("community/giveaways"), Some(n_running)), ("Ended", page.path("community/giveaways?tab=ended"), None)],
            if ended_tab { "Ended" } else { "Running" },
        ))
        div.with-rail {
            div.stack.cm-main { (main) }
            aside.rail {
                div.rail-block {
                    h2 { "Recent winners" }
                    @if winners.is_empty() {
                        (ui::empty(Icon::Trophy, "No winners yet", "Winners show up here once a giveaway ends."))
                    } @else {
                        @for (w, g, n) in &winners {
                            div.mini {
                                img.avatar src=(ctx.member_avatar(guild, *w)) alt="" width="32" height="32" loading="lazy";
                                div.grow {
                                    span { strong { (ctx.display_name(guild, *w)) } " won " (g.prize) }
                                    span.muted.small { (text::ago(g.finished_at())) " · out of " (entries_text(*n)) }
                                }
                            }
                        }
                    }
                    p.muted.small {
                        (text::num(totals.0)) " " (plural(totals.0, "giveaway", "giveaways")) " this month · "
                        (text::num(totals.1)) " " (plural(totals.1, "entry", "entries")) " from "
                        (text::num(totals.2)) " " (plural(totals.2, "member", "members"))
                    }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Giveaways", actions, body).into_response())
}

fn running_view(ctx: &Ctx, page: &Page, guild: u64, running: &[(Giveaway, i64)], faces: &[Vec<u64>], edit: bool) -> Markup {
    html! {
        @if let Some(((g, n), people)) = running.first().zip(faces.first()) {
            (hero(ctx, page, guild, g, *n, people, edit))
        } @else {
            (ui::empty(Icon::Gift, "Nothing running", "Start one with the button above or /giveaway start. Members enter with the button on the message."))
        }
        @if running.len() > 1 {
            div.cm-also {
                h2 { "Also running" }
                div.cm-list {
                    @for ((g, n), people) in running.iter().zip(faces).skip(1) {
                        (row_running(ctx, page, guild, g, *n, people, edit))
                    }
                }
            }
        }
    }
}

/// The giveaway ending soonest, with its progress.
fn hero(ctx: &Ctx, page: &Page, guild: u64, g: &Giveaway, n: i64, people: &[u64], edit: bool) -> Markup {
    html! {
        article.cm-hero {
            div.cm-tile.big { (ui::icon(Icon::Gift)) }
            div.cm-hero-body {
                span.cm-chip.warn { (ui::icon(Icon::Clock)) (until("Ends in", g.ends_at)) }
                h2.cm-hero-title { (g.prize) }
                p.muted { (winners_text(g)) " · in " (channel_name(ctx, guild, g.channel_id)) " · started by " (ctx.display_name(guild, g.created_by)) ", " (text::ago(g.created_at)) }
                div.cm-faces {
                    (face_row(ctx, guild, people, 5))
                    @let recent = ctx.db.with(|c| gw::entered_since(c, g.id, bot_core::now_ms() - 3_600_000)).unwrap_or(0);
                    @if n == 0 { span { "No entries yet" } } @else {
                        span {
                            strong.mono { (text::num(n)) } " " (plural(n, "entry", "entries"))
                            @if recent > 0 { ", " (text::num(recent)) " in the last hour" }
                        }
                    }
                }
                span.meter { span style=(format!("width:{}%", elapsed(g))) {} }
                div.cm-actions {
                    @if edit { (ui::action_button(&page.path(&format!("community/giveaways/{}/end", g.id)), "End now", Tone::Muted, true)) }
                    (open_link(page, g.channel_id, g.message_id, "Open in Discord"))
                }
            }
        }
    }
}

fn row_running(ctx: &Ctx, page: &Page, guild: u64, g: &Giveaway, n: i64, people: &[u64], edit: bool) -> Markup {
    html! {
        div.cm-row {
            div.cm-tile { (ui::icon(Icon::Gift)) }
            div.cm-row-main {
                strong.clip { (g.prize) }
                span.muted.small {
                    (until("Ends in", g.ends_at)) " · " (winners_text(g))
                    @if let Some(r) = g.required_role { " · needs " (role_name(ctx, guild, r)) }
                }
            }
            (face_row(ctx, guild, people, 4))
            span.cm-entries { (entries_text(n)) }
            div.cm-row-actions {
                (open_link(page, g.channel_id, g.message_id, "Open in Discord"))
                @if edit { (ui::action_button(&page.path(&format!("community/giveaways/{}/end", g.id)), "End now", Tone::Muted, true)) }
            }
        }
    }
}

fn ended_list(ctx: &Ctx, page: &Page, guild: u64, ended: &[(Giveaway, i64)], edit: bool) -> Markup {
    html! {
        @if ended.is_empty() {
            (ui::empty(Icon::Trophy, "Nothing ended yet", "Finished giveaways and their winners show up here."))
        } @else {
            div.cm-list {
                @for (g, n) in ended {
                    div.cm-row {
                        div.cm-tile { (ui::icon(Icon::Gift)) }
                        div.cm-row-main {
                            strong.clip { (g.prize) }
                            span.muted.small { "Ended " (text::ago(g.finished_at())) " · " (channel_name(ctx, guild, g.channel_id)) " · " (winners_text(g)) }
                        }
                        div.cm-winners {
                            @if g.winner_ids.is_empty() {
                                span.muted { "Nobody entered" }
                            } @else {
                                (face_row(ctx, guild, &g.winner_ids, 4))
                                span { (g.winner_ids.iter().map(|u| ctx.display_name(guild, *u)).collect::<Vec<_>>().join(", ")) }
                            }
                        }
                        span.cm-entries { (entries_text(*n)) }
                        div.cm-row-actions {
                            (open_link(page, g.channel_id, g.message_id, "Open in Discord"))
                            @if edit && *n > (g.winner_ids.len() + g.passed_ids.len()) as i64 {
                                (ui::action_button(&page.path(&format!("community/giveaways/{}/reroll", g.id)), "Reroll", Tone::Muted, false))
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The form in the "Start a giveaway" popover.
fn start_form(ctx: &Ctx, page: &Page, guild: u64) -> Markup {
    html! {
        form.settings method="post" action=(page.path("community/giveaways")) {
            (ui::card(html! {
                (ui::field_text("prize", "Prize", None, "", "Discord Nitro, 3 months"))
                (ui::field_text("duration", "Runs for", Some("Like 30m, 2h or 1d 12h. A bare number is minutes."), "", "1d"))
                (ui::field_number("winners", "Winners", None, 1, 1, gw::MAX_WINNERS, None))
                (channel_select(ctx, guild, "channel", "Channel", None))
                (ui::field_role(ctx, guild, "role", "Required role", Some("Only members with this role can enter."), None))
            }))
            div.cm-pop-foot { button.btn.primary type="submit" { "Start giveaway" } }
        }
    }
}

/// How far from the start to the end, 0 to 100.
fn elapsed(g: &Giveaway) -> i64 {
    let span = (g.ends_at - g.created_at).max(1);
    ((now_ms() - g.created_at) * 100 / span).clamp(0, 100)
}

fn winners_text(g: &Giveaway) -> String {
    format!("{} {}", g.winners, plural(g.winners, "winner", "winners"))
}

fn entries_text(n: i64) -> String {
    format!("{} {}", text::num(n), plural(n, "entry", "entries"))
}

fn plural(n: i64, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 { one } else { many }
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
    Ok(saved(&page.path("community/giveaways?tab=ended")))
}
