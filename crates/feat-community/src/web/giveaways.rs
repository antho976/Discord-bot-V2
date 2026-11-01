use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::maud::{Markup, html};
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};

use super::{
    TabQuery, channel_in, channel_name, channel_select, countdown, discord_message, discord_relative, face_row, header_pop, month_start, open_link, plural,
    post_block, rail_figures, role_name, script, style, until, when,
};
use crate::giveaways::{self as gw, EditOutcome, EndOutcome, Giveaway, RerollOutcome};
use crate::{CORAL, KEY, NEUTRAL};

pub async fn page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<TabQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let now = now_ms();
    let edit = page.can_edit();
    let ended_tab = q.tab.as_deref() == Some("ended");
    let since = month_start(&page.user, now);
    let (running, ended, (n_running, _), totals, faces, last_hour) = ctx.db.with(|c| {
        let running = gw::list(c, guild, false, 50)?;
        let faces = running.iter().map(|(g, _)| gw::newest_entrants(c, g.id, 8)).collect::<bot_core::rusqlite::Result<Vec<_>>>()?;
        let last_hour = match running.first() {
            Some((g, _)) => gw::entered_since(c, g.id, now - 3_600_000)?,
            None => 0,
        };
        Ok((running, gw::list(c, guild, true, 25)?, gw::counts(c, guild)?, gw::since_totals(c, guild, since)?, faces, last_hour))
    })?;
    let winners: Vec<(u64, &Giveaway, i64)> = ended.iter().flat_map(|(g, n)| g.winner_ids.iter().map(move |w| (*w, g, *n))).take(5).collect();
    // More than one running: the others fill the main column under the hero. With none, the
    // recent winners take it over and the rail keeps the month.
    let others = running.len() > 1;

    let month = rail_figures(&[("giveaways", totals.0), ("entries", totals.1), ("members", totals.2)]);
    let actions = edit.then(|| header_pop("Start a giveaway", start_form(&ctx, &page, guild)));

    let body = html! {
        (style())
        (ui::tabs_counted(
            &[("Running", page.path("community/giveaways"), Some(n_running)), ("Ended", page.path("community/giveaways?tab=ended"), None)],
            if ended_tab { "Ended" } else { "Running" },
        ))
        @if ended_tab {
            div.with-rail {
                div.cm-main { (ended_list(&ctx, &page, guild, &ended, edit)) }
                aside.rail.cm-rail {
                    div.rail-block {
                        h2 { "This month" }
                        (month)
                    }
                    @if let Some((g, n)) = ended.first() {
                        div.rail-block {
                            h2 { "The last result" span.muted { (channel_name(&ctx, guild, g.channel_id)) } }
                            (discord_message(&ctx, &page, g.finished_at(), post_preview(&ctx, guild, g, *n)))
                        }
                    }
                }
            }
        } @else {
            @if let Some(((g, n), people)) = running.first().zip(faces.first()) {
                (hero(&ctx, &page, guild, g, *n, people, last_hour, edit))
            } @else {
                (ui::empty(Icon::Gift, "Nothing running", "Start one with the button above or /giveaway start. Members enter with the button on the message."))
            }
            div.with-rail {
                div.cm-main {
                    @if others {
                        section.cm-block {
                            h2 { "Also running" }
                            div.tiles.gv-grid {
                                @for (i, ((g, n), people)) in running.iter().zip(&faces).skip(1).enumerate() {
                                    (card(&ctx, &page, guild, g, *n, people, i, edit))
                                }
                            }
                        }
                    } @else {
                        section.cm-block {
                            h2 { "Recent winners" }
                            (winner_lines(&ctx, guild, &winners, true))
                        }
                    }
                }
                aside.rail.cm-rail {
                    @if others {
                        div.rail-block {
                            h2 { "Recent winners" }
                            (winner_lines(&ctx, guild, &winners, false))
                        }
                    }
                    div.rail-block {
                        h2 { "This month" }
                        (month)
                    }
                }
            }
        }
        (script())
    };
    Ok(ui::layout(&ctx, &page, "Giveaways", actions, body).into_response())
}

/// The latest winners, one line each: in the rail, or as tiles when they fill the main column.
fn winner_lines(ctx: &Ctx, guild: u64, winners: &[(u64, &Giveaway, i64)], tiles: bool) -> Markup {
    let lines = html! {
        @for (w, g, n) in winners {
            div.mini.cm-winner {
                img.avatar src=(ctx.member_avatar(guild, *w)) alt="" width="32" height="32" loading="lazy";
                div.grow {
                    span { strong { (ctx.display_name(guild, *w)) } " won " (g.prize) }
                    span.muted { (text::ago(g.finished_at())) " · out of " (entries_text(*n)) }
                }
            }
        }
    };
    html! {
        @if winners.is_empty() {
            (ui::empty(Icon::Trophy, "No winners yet", "Winners show up here once a giveaway ends."))
        } @else if tiles {
            div.tiles.cm-tiles.gv-winners { (lines) }
        } @else {
            (lines)
        }
    }
}

/// The giveaway ending soonest, across the page: its prize, a clock counting down to the draw,
/// how far along it is, the latest faces to enter, and its post as members see it.
#[allow(clippy::too_many_arguments)]
fn hero(ctx: &Ctx, page: &Page, guild: u64, g: &Giveaway, n: i64, people: &[u64], last_hour: i64, edit: bool) -> Markup {
    let secs = ((g.ends_at - now_ms()) / 1000).max(0);
    let units = [(secs / 86_400, "d", "days"), (secs % 86_400 / 3600, "h", "hours"), (secs % 3600 / 60, "m", "min"), (secs % 60, "s", "sec")];
    html! {
        article.gv-hero {
            (art(g, "big"))
            div.gv-hero-body {
                h2.gv-prize { (g.prize) }
                div.gv-meta {
                    span { (winners_text(g)) " · " (channel_name(ctx, guild, g.channel_id)) " ·" }
                    span.gv-by {
                        img.avatar src=(ctx.member_avatar(guild, g.created_by)) alt="" width="20" height="20" loading="lazy";
                        span { "started by " strong { (ctx.display_name(guild, g.created_by)) } ", " (text::ago(g.created_at)) }
                    }
                    @if let Some(r) = g.required_role { span.gv-role { "Only " (role_name(ctx, guild, r)) } }
                }
                div.gv-clock data-ends=(g.ends_at) data-clock role="timer" aria-label=(until("Ends in", g.ends_at)) {
                    // Units start at the first one that isn't zero; minutes and seconds always show.
                    @for (i, (v, u, label)) in units.iter().enumerate() {
                        @if i >= 2 || units[..=i].iter().any(|x| x.0 > 0) {
                            div.gv-unit { b data-u=(u) { (format!("{v:02}")) } span { (label) } }
                        }
                    }
                }
                div.gv-time {
                    span.meter { span style=(format!("width:{}%", elapsed(g))) {} }
                    div.gv-time-cap {
                        span { "Started " (text::ago(g.created_at)) }
                        span { "Draws " strong { (when(page, g.ends_at)) } }
                    }
                }
                div.gv-people {
                    @if n == 0 {
                        span { "No entries yet. Members join with Enter on the post." }
                    } @else {
                        (face_row(ctx, guild, people, 8))
                        span {
                            b data-count { (text::num(n)) } " " (plural(n, "entry", "entries"))
                            @if last_hour > 0 { " · " (text::num(last_hour)) " in the last hour" }
                        }
                    }
                }
                div.cm-actions {
                    @if edit {
                        (edit_pop(page, g, true))
                        (ui::action_button(&page.path(&format!("community/giveaways/{}/end", g.id)), "End now", Tone::Muted, true))
                    }
                    (open_link(page, g.channel_id, g.message_id, false))
                }
            }
            (post_block("The post", &channel_name(ctx, guild, g.channel_id), discord_message(ctx, page, g.created_at, post_preview(ctx, guild, g, n))))
        }
    }
}

/// Another running giveaway, as a card in the grid.
#[allow(clippy::too_many_arguments)]
fn card(ctx: &Ctx, page: &Page, guild: u64, g: &Giveaway, n: i64, people: &[u64], i: usize, edit: bool) -> Markup {
    html! {
        article.gv-card style={ "--i:" (i.min(8)) } {
            div.gv-card-top {
                (art(g, ""))
                div.grow {
                    strong title=(g.prize) { (g.prize) }
                    span.muted { (winners_text(g)) " · " (channel_name(ctx, guild, g.channel_id)) }
                }
                (open_link(page, g.channel_id, g.message_id, true))
            }
            div.stack style="gap:8px" {
                div.gv-left {
                    span { (ui::icon(Icon::Clock)) (countdown("Ends in", g.ends_at)) }
                    @if let Some(r) = g.required_role { span.gv-role { "Only " (role_name(ctx, guild, r)) } }
                }
                span.meter { span style=(format!("width:{}%", elapsed(g))) {} }
            }
            div.gv-card-foot {
                div.gv-people {
                    @if n == 0 { span { "No entries yet" } } @else {
                        (face_row(ctx, guild, people, 4))
                        span { b { (text::num(n)) } " " (plural(n, "entry", "entries")) }
                    }
                }
                div.cm-actions {
                    @if edit {
                        (edit_pop(page, g, false))
                        (ui::action_button(&page.path(&format!("community/giveaways/{}/end", g.id)), "End now", Tone::Muted, true))
                    }
                }
            }
        }
    }
}

fn ended_list(ctx: &Ctx, page: &Page, guild: u64, ended: &[(Giveaway, i64)], edit: bool) -> Markup {
    html! {
        @if ended.is_empty() {
            (ui::empty(Icon::Trophy, "Nothing ended yet", "Finished giveaways and their winners show up here."))
        } @else {
            // Cards in a grid that gains columns with the width: the prize, who won, the totals
            // and the reroll in the card's own foot.
            div.tiles.gv-done-grid {
                @for (i, (g, n)) in ended.iter().enumerate() {
                    article.gv-card.done style={ "--i:" (i.min(8)) } {
                        div.gv-card-top {
                            (art(g, "ended"))
                            div.grow {
                                strong title=(g.prize) { (g.prize) }
                                span.muted { "Ended " (text::ago(g.finished_at())) " · " (channel_name(ctx, guild, g.channel_id)) }
                            }
                            (open_link(page, g.channel_id, g.message_id, true))
                        }
                        @if g.winner_ids.is_empty() {
                            span.gv-nobody { "Nobody entered" }
                        } @else {
                            div.gv-won {
                                (ui::icon(Icon::Trophy))
                                (face_row(ctx, guild, &g.winner_ids, 5))
                                span { strong { (names(ctx, guild, &g.winner_ids)) } " won" }
                            }
                        }
                        div.gv-card-foot {
                            span { (winners_text(g)) " · " (entries_text(*n)) }
                            @if edit && *n > (g.winner_ids.len() + g.passed_ids.len()) as i64 {
                                div.cm-actions { (ui::action_button(&page.path(&format!("community/giveaways/{}/reroll", g.id)), "Reroll", Tone::Muted, false)) }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// "Mira", "Mira and Kasia", "Mira, Kasia and bramble".
fn names(ctx: &Ctx, guild: u64, users: &[u64]) -> String {
    let all: Vec<String> = users.iter().map(|u| ctx.display_name(guild, *u)).collect();
    match all.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => all.join(""),
    }
}

/// The prize art: the creator's emoji on a glowing tile, or the gift icon.
fn art(g: &Giveaway, class: &str) -> Markup {
    html! {
        div class={ "gv-art " (class) } aria-hidden="true" {
            @if g.emoji.is_empty() { (ui::icon(Icon::Gift)) } @else { span.gv-emoji { (g.emoji) } }
        }
    }
}

/// The giveaway's post as members see it: the embed, and the Enter button while it runs.
fn post_preview(ctx: &Ctx, guild: u64, g: &Giveaway, n: i64) -> Markup {
    let title = if g.emoji.is_empty() { g.prize.clone() } else { format!("{} {}", g.emoji, g.prize) };
    let color = if g.ended { NEUTRAL } else { CORAL };
    html! {
        div.cm-dc-embed style={ "--embed:#" (format!("{color:06x}")) } {
            div.cm-dc-title { (text::clip(&title, 250)) }
            @if g.ended {
                div.cm-dc-desc { "Ended " span.cm-dc-ts { (discord_relative(g.finished_at())) } }
                div.cm-dc-fields {
                    div style="flex-basis:100%" {
                        b { (if g.winner_ids.len() == 1 { "Winner" } else { "Winners" }) }
                        span {
                            @if g.winner_ids.is_empty() { "Nobody entered" }
                            @for w in &g.winner_ids { span.cm-dc-ping { "@" (ctx.display_name(guild, *w)) } " " }
                        }
                    }
                    div { b { "Entries" } span { (text::num(n)) } }
                }
            } @else {
                div.cm-dc-desc {
                    "Press Enter to join. Press it again to leave."
                    @if let Some(r) = g.required_role { "\nOnly for " span.cm-dc-ping { (role_name(ctx, guild, r)) } "." }
                }
                div.cm-dc-fields {
                    div { b { "Ends" } span { span.cm-dc-ts { (discord_relative(g.ends_at)) } } }
                    div { b { "Winners" } span { (g.winners) } }
                    div { b { "Entries" } span { (text::num(n)) } }
                }
            }
            div.cm-dc-foot { "Giveaway #" (g.id) }
        }
        @if !g.ended { div.cm-dc-btns { span.cm-dc-btn.blurple { "Enter" } } }
    }
}

/// "Edit" on a running giveaway: prize, emoji, end and winners. `left` opens the popover
/// rightwards, for buttons near the left edge.
fn edit_pop(page: &Page, g: &Giveaway, left: bool) -> Markup {
    let left_secs = ((g.ends_at - now_ms()) / 1000).max(0);
    html! {
        details.pop.cm-edit.left[left] {
            summary.btn.small { "Edit" }
            div.pop-body {
                form.settings method="post" action=(page.path(&format!("community/giveaways/{}/edit", g.id))) {
                    (ui::card(html! {
                        (ui::field_text("prize", "Prize", None, &g.prize, "Discord Nitro, 3 months"))
                        (ui::field_text("emoji", "Emoji", None, &g.emoji, "🎁"))
                        (ui::field_text("ends", "Ends in", Some("From now, like 2h or 1d."), "", &text::duration(left_secs)))
                        (ui::field_number("winners", "Winners", None, g.winners, 1, gw::MAX_WINNERS, None))
                    }))
                    div.cm-pop-foot { button.btn.primary type="submit" { "Save changes" } }
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
                (ui::field_text("emoji", "Emoji", Some("Shown on the prize tile and the post. Empty uses the gift icon."), "", "🎁"))
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

pub async fn create(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let prize = form.string("prize");
    if prize.is_empty() {
        return Err(WebError::Bad("Say what's being given away.".into()));
    }
    let emoji = gw::parse_emoji(&form.string("emoji")).map_err(|e| WebError::Bad(e.into()))?;
    let secs = gw::parse_span(&form.string("duration")).map_err(|e| WebError::Bad(e.into()))?;
    let winners = form.int_in("winners", 1, gw::MAX_WINNERS, 1);
    let channel = channel_in(&ctx, guild, form.id("channel"))?;
    let role = form.id("role").filter(|r| ctx.roles(guild).iter().any(|(id, _, _)| id == r));
    let id = gw::create(&ctx, gw::New { guild, channel, prize: &prize, emoji: &emoji, winners, ends_at: now_ms() + secs * 1000, role, by: page.user.id })
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

pub async fn edit(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    owned(&ctx, guild, id).await?;
    let prize = form.string("prize");
    if prize.is_empty() {
        return Err(WebError::Bad("Say what's being given away.".into()));
    }
    let emoji = gw::parse_emoji(&form.string("emoji")).map_err(|e| WebError::Bad(e.into()))?;
    let ends = form.string("ends");
    let ends_at = if ends.is_empty() {
        ctx.db.with(|c| gw::get(c, id))?.map(|g| g.ends_at).ok_or(WebError::NotFound)?
    } else {
        now_ms() + gw::parse_span(&ends).map_err(|e| WebError::Bad(e.into()))? * 1000
    };
    let winners = form.int_in("winners", 1, gw::MAX_WINNERS, 1);
    match gw::edit(&ctx, id, gw::Changes { prize: &prize, emoji: &emoji, ends_at, winners }).await? {
        EditOutcome::Edited => ctx.audit(&page.user, Some(guild), KEY, format!("Edited giveaway #{id}: {}", text::clip(&prize, 60))),
        EditOutcome::AlreadyEnded => return Err(WebError::Bad("That giveaway has already ended, so it can't be changed.".into())),
        EditOutcome::Missing => return Err(WebError::NotFound),
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
