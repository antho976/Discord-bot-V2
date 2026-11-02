use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::rusqlite::params;
use bot_core::twilight_model::id::Id;
use bot_core::web::dc;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};
use serde::Deserialize;

use crate::tickets::{self, Filter, Ticket, Turn};
use crate::{DEFAULT_PANEL_TEXT, DEFAULT_PANEL_TITLE, KEY, Settings, starboard};

const DAY_MS: i64 = 86_400_000;
/// The coral edge on the panel and starboard embeds, as the bot sends them.
const EMBED_CORAL: &str = "#f0806a";

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/tickets", get(tickets_page))
        .route("/g/{guild}/tickets/settings", post(save_tickets))
        .route("/g/{guild}/tickets/panel", post(send_panel))
        .route("/g/{guild}/tickets/{number}/close", post(close_ticket))
        .route("/g/{guild}/tickets/{number}/reopen", post(reopen_ticket))
        .route("/g/{guild}/tickets/starboard", get(starboard_page).post(save_starboard))
        .route("/g/{guild}/tickets/starboard/test", post(send_star_test))
}

fn thread_url(guild: u64, thread: u64) -> String {
    format!("https://discord.com/channels/{guild}/{thread}")
}

/// Like `text::duration`, but in days once it passes two of them.
fn span(secs: i64) -> String {
    if secs >= 2 * 86_400 {
        let (d, h) = (secs / 86_400, (secs % 86_400) / 3600);
        if h == 0 { format!("{d} days") } else { format!("{d} days {h} h") }
    } else {
        text::duration(secs)
    }
}

fn channel_name(ctx: &Ctx, guild: u64, channel: u64) -> String {
    ctx.text_channels(guild).into_iter().find(|(id, _)| *id == channel).map(|(_, n)| format!("#{n}")).unwrap_or_else(|| "a deleted channel".into())
}

/// The emoji a starboard post carries in its count. A custom emoji, pasted as `<:name:id>`, can't
/// be drawn here, so it stands for a plain star.
fn star_glyph(emoji: &str) -> &str {
    if emoji.trim().is_empty() || emoji.starts_with('<') { "★" } else { emoji.trim() }
}

/// A filled star for the counts on the starboard.
fn star() -> Markup {
    html! {
        svg.icon.sb-star viewBox="0 0 16 16" width="14" height="14" aria-hidden="true" {
            path d="M8 1.6l1.9 3.95 4.35.6-3.16 3.03.77 4.32L8 11.43 4.14 13.5l.77-4.32L1.75 6.15l4.35-.6z" fill="currentColor" {}
        }
    }
}

/// A link out to Discord: the label with the external icon.
fn out_link(href: &str, label: &str) -> Markup {
    html! {
        a.btn.small.tk-out href=(href) target="_blank" rel="noopener" { (ui::icon(Icon::External)) (label) }
    }
}

// ---- Discord, as members see it ----

/// A message from the bot as Discord draws it, sent at `at`, alone on Discord's grey (no channel
/// around it: the rail's heading names the channel).
fn discord_message(ctx: &Ctx, page: &Page, at: i64, body: Markup) -> Markup {
    let today = page.user.local(now_ms()).date() == page.user.local(at).date();
    let time = page.user.local(at).strftime(if today { "Today at %-I:%M %p" } else { "%m/%d/%Y %-I:%M %p" }).to_string();
    html! { div.tk-dc { (dc::message(ctx, page).time_text(time).render(body)) } }
}

// ---- tickets ----

#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
}

/// What the thread says: the last reply, and whether it's the member's turn or staff's. The turn
/// comes from what the bot stored; the quote, and the turn for tickets opened before the bot kept
/// it, come from the message cache, which is empty after a restart.
#[derive(Default)]
struct Activity {
    last_reply_at: Option<i64>,
    /// `(who, what they wrote)` for the latest reply from someone other than the opener.
    staff_reply: Option<(u64, String)>,
    /// The badge, and whether staff owes the reply.
    waiting: Option<(String, bool)>,
}

fn activity(ctx: &Ctx, guild: u64, t: &Ticket) -> Activity {
    let mut act = Activity::default();
    if t.thread_id == 0 {
        return act;
    }
    let thread = Id::new(t.thread_id);
    act.last_reply_at = ctx.cache.channel(thread).and_then(|c| c.last_message_id).map(|id| tickets::snowflake_ms(id.get()));
    let bot = ctx.bot_id();
    let messages: Vec<(u64, String)> = ctx
        .cache
        .channel_messages(thread)
        .map(|ids| {
            ids.iter()
                .rev()
                .filter_map(|id| ctx.cache.message(*id))
                .filter(|m| Some(m.author().get()) != bot)
                .map(|m| (m.author().get(), m.content().to_owned()))
                .collect()
        })
        .unwrap_or_default();
    act.staff_reply = messages.iter().find(|(author, _)| *author != t.opener_id).map(|(author, said)| (*author, text::clip(said, 180)));
    let on_member = || (format!("Waiting on {}", ctx.display_name(guild, t.opener_id)), false);
    act.waiting = match t.waiting_on() {
        Some(Turn::NoReplyYet) => Some(("No staff reply yet".to_owned(), true)),
        Some(Turn::Staff) => Some(("Waiting on staff".to_owned(), true)),
        Some(Turn::Member) => Some(on_member()),
        None => messages.first().map(|(author, _)| if *author == t.opener_id { ("Waiting on staff".to_owned(), true) } else { on_member() }),
    };
    if let Some(at) = t.last_message_at {
        act.last_reply_at = Some(act.last_reply_at.map_or(at, |cached| cached.max(at)));
    }
    act
}

/// Tickets opened on each of the last `days` days in the viewer's time, oldest first.
fn opened_per_day(ctx: &Ctx, page: &Page, guild: u64, now: i64, days: i64) -> Vec<(String, i64)> {
    let opened: Vec<i64> = ctx
        .db
        .with(|c| {
            let mut st = c.prepare_cached("SELECT opened_at FROM tickets WHERE guild_id = ?1 AND opened_at > ?2")?;
            st.query_map(params![guild as i64, now - (days + 1) * DAY_MS], |r| r.get(0))?.collect()
        })
        .unwrap_or_default();
    let today = page.user.local(now).date();
    (0..days)
        .rev()
        .filter_map(|back| today.checked_sub(bot_core::jiff::Span::new().days(back)).ok())
        .map(|day| {
            let n = opened.iter().filter(|at| page.user.local(**at).date() == day).count() as i64;
            (day.strftime("%b %-d").to_string(), n)
        })
        .collect()
}

async fn tickets_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<ListQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let now = now_ms();
    let view = q.status.as_deref().unwrap_or("open");
    let stats = ctx.db.with(|c| tickets::stats(c, guild, now))?;
    let rows = match view {
        "closed" => ctx.db.with(|c| tickets::list(c, guild, Filter::Closed, 100))?,
        "settings" => Vec::new(),
        _ => ctx.db.with(|c| tickets::list(c, guild, Filter::Open, 100))?,
    };
    let active = match view {
        "closed" => "Closed",
        "settings" => "Settings",
        _ => "Open",
    };
    let tabs = ui::tabs_counted(
        &[
            ("Open", page.path("tickets"), Some(stats.open)),
            ("Closed", format!("{}?status=closed", page.path("tickets")), None),
            ("Settings", format!("{}?status=settings", page.path("tickets")), None),
        ],
        active,
    );
    let actions = (page.can_edit() && s.ticket_channel.is_some()).then(|| {
        html! {
            form.inline method="post" action=(page.path("tickets/panel")) {
                button.btn.primary type="submit" { "Send the panel" }
            }
        }
    });

    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs)
        @if view == "settings" {
            // The settings beside the panel they shape; sending it belongs to the panel.
            div.compose.tk-compose {
                (ui::settings_form(&page, &page.path("tickets/settings"), html! {
                    (ui::section("Tickets", None, ui::card(html! {
                        (ui::field_channel(&ctx, guild, "ticket_channel", "Ticket channel", Some("Tickets are private threads in this channel. The panel is posted here too."), s.ticket_channel))
                        (ui::field_role(&ctx, guild, "staff_role", "Staff role", Some("Pinged once when a ticket opens, and allowed to close any ticket."), s.staff_role))
                        (ui::field_number("max_open", "Open tickets per member", Some("A member must close one before opening another past this number. Members also wait 10 minutes between tickets; staff don't."), s.max_open, 1, 10, None))
                        (ui::field_number("auto_close_days", "Close idle tickets after", Some("Days with no messages in the thread. 0 leaves them open."), s.auto_close_days, 0, 90, Some("days")))
                    })))
                    (ui::section("The panel", None, ui::card(html! {
                        (ui::field_text("panel_title", "Panel title", None, &s.panel_title, DEFAULT_PANEL_TITLE))
                        (ui::field_textarea("panel_text", "Panel text", Some("Shown above the Open a ticket button."), &s.panel_text, DEFAULT_PANEL_TEXT))
                    })))
                }))
                aside.rail.tk-rail.compose-preview {
                    div.rail-block {
                        h2 {
                            "How the panel looks"
                            @if let Some(ch) = s.ticket_channel { span.muted { (channel_name(&ctx, guild, ch)) } }
                        }
                        @let title = if s.panel_title.trim().is_empty() { DEFAULT_PANEL_TITLE } else { s.panel_title.trim() };
                        @let blurb = if s.panel_text.trim().is_empty() { DEFAULT_PANEL_TEXT } else { s.panel_text.trim() };
                        (discord_message(&ctx, &page, now, html! {
                            (dc::embed(EMBED_CORAL, html! {
                                div.dp-title { (title) }
                                div.dp-desc { (blurb) }
                            }))
                            (dc::buttons(dc::button("Open a ticket", dc::Style::Primary)))
                        }))
                        p.muted.small { "Save the settings first. Sending a new panel removes the previous one." }
                        @if page.can_edit() {
                            div.tk-pv-acts {
                                @if s.ticket_channel.is_some() {
                                    (ui::action_button(&page.path("tickets/panel"), if s.panel.is_some() { "Send a new panel" } else { "Send panel" }, Tone::Muted, false))
                                } @else {
                                    span.muted.small { "Pick a ticket channel to send the panel." }
                                }
                                @if let Some((channel, message)) = s.panel {
                                    (out_link(&format!("{}/{message}", thread_url(guild, channel)), "View current panel"))
                                }
                            }
                        }
                    }
                    // The side blocks of the other tabs, so this column ends with the settings.
                    div.rail-block {
                        h2 { "Tickets" }
                        (figures(&stats, true))
                    }
                    (answering(&ctx, guild, now))
                }
            }
        } @else if view == "closed" {
            // A sub-tab: the closed tickets, with the numbers and who answers on the right.
            div.with-rail.tk-cols {
                section.tk-panel {
                    h2 { "Closed tickets" @if rows.len() == 100 { span.muted { "the latest 100" } } }
                    @if rows.is_empty() {
                        (ui::empty(Icon::Ticket, "Nothing closed yet", "Closed tickets stay here with who closed them and how long they took."))
                    } @else {
                        div.tk-list {
                            @for t in &rows { (ticket_row(&ctx, &page, guild, t, now)) }
                        }
                    }
                }
                aside.rail.tk-rail {
                    div.rail-block {
                        h2 { "Tickets" }
                        (figures(&stats, true))
                    }
                    div.rail-block {
                        h2 { "Opened, last 14 days" span.muted { (text::num(stats.this_week)) " this week" } }
                        (ui::area_chart(&opened_per_day(&ctx, &page, guild, now, 14), "opened"))
                    }
                    (answering(&ctx, guild, now))
                }
            }
        } @else {
            // The answer first: how many are open, the two weeks they came from, and who answers;
            // then the open tickets themselves.
            @let days = opened_per_day(&ctx, &page, guild, now, 28);
            @let (before, recent) = days.split_at(14);
            section.tk-panel.tk-lead {
                div.tk-lead-main {
                    div.tk-lead-head {
                        div.tk-big {
                            b { (text::num(stats.open)) }
                            span { @if stats.open == 1 { "ticket open" } @else { "tickets open" } }
                        }
                        (figures(&stats, false))
                    }
                    div.tk-chart {
                        h2 {
                            "Opened per day"
                            span.muted { "last 14 days" @if before.iter().any(|d| d.1 > 0) { ", the 14 before dashed" } }
                        }
                        (ui::area_chart_vs(recent, Some(before), "opened"))
                    }
                }
                div.tk-lead-side {
                    (answering(&ctx, guild, now))
                    (closed_recently(&ctx, guild, now))
                }
            }
            section.tk-panel {
                h2 { "Open tickets" @if rows.len() == 100 { span.muted { "the latest 100" } } }
                @if rows.is_empty() {
                    (ui::empty(Icon::Ticket, "No open tickets", "Pick a ticket channel and staff role in Settings, send the panel, and tickets appear here as members open them."))
                } @else {
                    div.tk-list {
                        @for t in &rows { (ticket_row(&ctx, &page, guild, t, now)) }
                    }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Tickets", actions, body).into_response())
}

/// The figures beside the lead number, or in the Closed tab's rail with the open count too.
fn figures(stats: &tickets::Stats, with_open: bool) -> Markup {
    html! {
        div.tk-figs {
            @if with_open { div { b { (text::num(stats.open)) } span { "open now" } } }
            div.hot[stats.waiting > 0] {
                b { (text::num(stats.waiting)) }
                span { @if stats.waiting > 0 { span.dot.waiting {} } "waiting on staff" }
            }
            div { b { (text::num(stats.this_week)) } span { "opened this week" } }
            div { b { (stats.median_close_secs.map(span).unwrap_or_else(|| "none yet".into())) } span { "usual time to close" } }
        }
    }
}

/// One ticket as a row: the opener's face with whose turn it is, the subject and who opened
/// it, then what was said (the latest staff reply under it) and how it ended once closed, then
/// where it stands and its buttons.
fn ticket_row(ctx: &Ctx, page: &Page, guild: u64, t: &Ticket, now: i64) -> Markup {
    let name = ctx.display_name(guild, t.opener_id);
    let act = if t.is_open() { activity(ctx, guild, t) } else { Activity::default() };
    let (label, tone) = match t.status.as_str() {
        tickets::OPEN => ("Open", Tone::Ok),
        tickets::IDLE => ("Auto-closed", Tone::Warn),
        _ => ("Closed", Tone::Muted),
    };
    let staff_turn = act.waiting.as_ref().is_some_and(|w| w.1);
    // Staff wrote last but the words aren't cached: still say who answered, and when.
    let answered = (t.is_open() && act.staff_reply.is_none() && t.waiting_on() == Some(Turn::Member)).then_some(t.last_author).flatten();
    let took = t.closed_at.map(|at| span(((at - t.opened_at) / 1000).max(0)));
    html! {
        article.tk-row {
            div.tk-face {
                img.avatar src=(ctx.member_avatar(guild, t.opener_id)) alt="" width="28" height="28" loading="lazy";
                @if t.is_open() { span class={ "tk-turn " (if staff_turn { "staff" } else { "member" }) } {} }
            }
            div.tk-body {
                div.tk-top {
                    span.tk-num { (format!("{:04}", t.number)) }
                    strong.tk-subject { (t.subject) }
                }
                div.tk-by {
                    (name) " · opened " (text::ago(t.opened_at))
                    @if t.is_open() {
                        @if let Some(at) = act.last_reply_at { " · last reply " (text::ago(at)) }
                    }
                }
            }
            div.tk-convo {
                @if !t.details.is_empty() { p.tk-said { (text::clip(&t.details, 240)) } }
                @if let Some((who, said)) = &act.staff_reply {
                    div.tk-reply {
                        img.avatar src=(ctx.member_avatar(guild, *who)) alt="" width="20" height="20" loading="lazy";
                        p { strong { (ctx.display_name(guild, *who)) } " " (said) }
                    }
                }
                @if let Some(who) = answered {
                    div.tk-reply {
                        img.avatar src=(ctx.member_avatar(guild, who)) alt="" width="20" height="20" loading="lazy";
                        p {
                            strong { (ctx.display_name(guild, who)) } " answered"
                            @if let Some(at) = t.last_message_at { " " (text::ago(at)) }
                        }
                    }
                }
                @if !t.is_open() {
                    div.tk-reply {
                        @if let Some(by) = t.closed_by {
                            img.avatar src=(ctx.member_avatar(guild, by)) alt="" width="20" height="20" loading="lazy";
                        }
                        p {
                            (label) " " (text::ago(t.closed_at.unwrap_or(now)))
                            @if let Some(by) = t.closed_by { " by " strong { (ctx.display_name(guild, by)) } }
                            @if let Some(took) = &took { " · took " (took) }
                            @if let Some(reason) = t.reason.as_deref() { " · " (reason) }
                        }
                    }
                }
            }
            div.tk-state {
                @if t.is_open() {
                    @if let Some((w, staff)) = &act.waiting { (ui::badge(w, if *staff { Tone::Warn } else { Tone::Muted })) }
                } @else {
                    (ui::badge(label, tone))
                }
            }
            // One slot per button, so "Open thread" lines up down the list even where a row
            // has nothing to close or reopen.
            div.tk-acts {
                @if t.thread_id != 0 { (out_link(&thread_url(guild, t.thread_id), "Open thread")) } @else { span {} }
                @if page.can_edit() && t.is_open() {
                    (ui::action_button(&page.path(&format!("tickets/{}/close", t.number)), "Close", Tone::Bad, true))
                } @else if page.can_edit() && t.can_reopen(now) {
                    (ui::action_button(&page.path(&format!("tickets/{}/reopen", t.number)), "Reopen", Tone::Muted, false))
                }
            }
        }
    }
}

/// The last three tickets to close, and how each one ended.
fn closed_recently(ctx: &Ctx, guild: u64, now: i64) -> Markup {
    let recent = ctx.db.with(|c| tickets::recently_closed(c, guild, 3)).unwrap_or_default();
    html! {
        div.rail-block {
            h2 { "Closed recently" }
            @if recent.is_empty() {
                p.muted.small { "Closed tickets show up here with who closed them." }
            }
            @for t in &recent {
                @let took = span(((t.closed_at.unwrap_or(now) - t.opened_at) / 1000).max(0));
                div.tk-line {
                    img.avatar src=(ctx.member_avatar(guild, t.opener_id)) alt="" width="28" height="28" loading="lazy";
                    div.grow {
                        strong { (t.subject) }
                        span {
                            (ctx.display_name(guild, t.opener_id)) " · "
                            @match t.closed_by {
                                Some(by) => { "closed by " (ctx.display_name(guild, by)) " in " (took) },
                                None => { @if t.status == tickets::IDLE { "closed when idle, after " (took) } @else { "closed in " (took) } },
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Who closed the most tickets this month. Closing is the one answer the bot keeps, so that's
/// what this shows.
fn answering(ctx: &Ctx, guild: u64, now: i64) -> Markup {
    let who = ctx.db.with(|c| tickets::closers(c, guild, now - 30 * DAY_MS, 3)).unwrap_or_default();
    let most = who.first().map(|w| w.1).unwrap_or(1).max(1);
    html! {
        div.rail-block {
            h2 { "Who's answering" span.muted { "last 30 days" } }
            @if who.is_empty() {
                p.muted.small { "Nobody has closed a ticket in the last 30 days." }
            }
            @for (member, closed) in &who {
                div.tk-line {
                    img.avatar src=(ctx.member_avatar(guild, *member)) alt="" width="28" height="28" loading="lazy";
                    div.grow {
                        div.tk-line-top { strong { (ctx.display_name(guild, *member)) } span.mono { (closed) " closed" } }
                        span.meter { span style={ "width:" (closed * 100 / most) "%" } {} }
                    }
                }
            }
        }
    }
}

async fn save_tickets(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let mut s = (*ctx.settings::<Settings>(guild, KEY)?).clone();
    s.ticket_channel = form.channel(&ctx, guild, "ticket_channel");
    s.staff_role = form.role(&ctx, guild, "staff_role");
    s.max_open = form.int_in("max_open", 1, 10, 1);
    s.auto_close_days = form.int_in("auto_close_days", 0, 90, 0);
    s.panel_title = text::clip(&form.string("panel_title"), 200);
    s.panel_text = text::clip(&form.text("panel_text"), 1500);
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed ticket settings");
    Ok(saved(&page.path("tickets?status=settings")))
}

async fn send_panel(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let channel = tickets::send_panel(&ctx, guild).await.map_err(WebError::Bad)?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Sent the ticket panel to {}", channel_name(&ctx, guild, channel)));
    Ok(saved(&page.path("tickets")))
}

async fn close_ticket(State(ctx): State<Ctx>, user: User, Path((guild, number)): Path<(u64, i64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    if tickets::close_from_dashboard(&ctx, guild, number, page.user.id).await? {
        ctx.audit(&page.user, Some(guild), KEY, format!("Closed ticket {number:04}"));
    }
    Ok(saved(&page.path("tickets")))
}

async fn reopen_ticket(State(ctx): State<Ctx>, user: User, Path((guild, number)): Path<(u64, i64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    tickets::reopen_from_dashboard(&ctx, guild, number, page.user.id).await.map_err(WebError::Bad)?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Reopened ticket {number:04}"));
    Ok(saved(&page.path("tickets")))
}

// ---- starboard ----

#[derive(Deserialize)]
struct StarQuery {
    view: Option<String>,
    sent: Option<String>,
}

async fn send_star_test(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    if s.star_channel.is_none() {
        return Err(WebError::Bad("Pick a starboard channel and save first.".into()));
    }
    if let Err(e) = starboard::test_post(&ctx, &s, &page.user.name, &page.user.avatar_url()).await {
        bot_core::tracing::warn!(guild, error = %e, "test starboard post failed");
        return Err(WebError::Bad(bot_core::discord::why_refused_any(&e)));
    }
    ctx.audit(&page.user, Some(guild), KEY, "Sent a test starboard post");
    Ok(bot_core::axum::response::Redirect::to(&page.path("tickets/starboard?view=settings&sent=1")).into_response())
}

async fn starboard_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<StarQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let now = now_ms();
    let view = q.view.as_deref().unwrap_or("most");
    let stats = ctx.db.with(|c| starboard::stats(c, guild, now))?;
    let mut rows = match view {
        "newest" => ctx.db.with(|c| starboard::recent(c, guild, 25))?,
        // The settings tab shows the most starred one as it was reposted.
        "settings" => ctx.db.with(|c| starboard::top(c, guild, 1))?,
        _ => ctx.db.with(|c| starboard::top(c, guild, 25))?,
    };
    starboard::fill_missing_images(&ctx, guild, &mut rows).await;
    let images = starboard::shown_images(&ctx, &rows).await;
    let authors = ctx.db.with(|c| starboard::top_authors(c, guild, 4))?;
    let active = match view {
        "newest" => "Newest",
        "settings" => "Settings",
        _ => "Most starred",
    };
    let tabs = ui::tabs(
        &[
            ("Most starred", page.path("tickets/starboard")),
            ("Newest", format!("{}?view=newest", page.path("tickets/starboard"))),
            ("Settings", format!("{}?view=settings", page.path("tickets/starboard"))),
        ],
        active,
    );
    let glyph = star_glyph(&s.star_emoji);

    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs)
        (ui::sent_toast(q.sent.is_some(), "Test post sent"))
        @if view == "settings" {
            // The settings beside the repost they shape.
            div.compose.tk-compose {
                (ui::settings_form(&page, &page.path("tickets/starboard"), ui::section("Settings", None, ui::card(html! {
                    (ui::field_channel(&ctx, guild, "star_channel", "Starboard channel", Some("Where popular messages are reposted. None turns the starboard off."), s.star_channel))
                    (ui::field_text("star_emoji", "Emoji", Some("Paste the emoji itself. For a custom one, paste it as <:name:id>."), &s.star_emoji, "⭐"))
                    (ui::field_number("star_threshold", "Stars needed", Some("A message is reposted once it has this many."), s.star_threshold, 1, 100, Some("stars")))
                    (ui::field_toggle("star_remove_below", "Remove when it drops below", Some("Takes the post down if stars fall under the number above, once it has been up for 10 minutes. Off keeps it with the lower count."), s.star_remove_below))
                    (ui::field_toggle("star_self", "Count the author's own star", Some("Off: a member starring their own message doesn't count."), s.star_self))
                    (ui::field_channels(&ctx, guild, "star_ignored", "Ignored channels", Some("Stars in these channels and their threads never reach the starboard. Neither do the starboard itself, private threads, channels that some people who can see the starboard can't see, or age-restricted channels unless the starboard is one."), &s.star_ignored))
                }))))
                aside.rail.tk-rail.compose-preview {
                    div.rail-block {
                        h2 {
                            "How a repost looks"
                            @if let Some(ch) = s.star_channel { span.muted { (channel_name(&ctx, guild, ch)) } }
                        }
                        @match rows.first() {
                            Some(p) => {
                                (discord_message(&ctx, &page, p.created_at, repost_preview(&ctx, guild, p, p.image.as_ref().and_then(|i| images.get(i)).map(String::as_str), glyph)))
                                p.muted.small { "The most starred message, as the bot reposted it." }
                            },
                            None => p.muted.small { "Once a message gets enough stars, its repost shows here as Discord draws it." },
                        }
                        @if page.can_edit() {
                            div.tk-pv-acts { (ui::action_button(&page.path("tickets/starboard/test"), "Send a test post", Tone::Muted, false)) }
                        }
                    }
                    (star_side(&ctx, guild, &s, &stats, &authors, glyph))
                }
            }
        } @else if rows.is_empty() {
            div.with-rail.tk-cols {
                section.tk-panel {
                    (ui::empty(Icon::Star, "Nothing starred yet", if s.star_channel.is_some() {
                        "A message shows up here once it gets enough stars."
                    } else {
                        "Pick a starboard channel in Settings. Messages show up here once they get enough stars."
                    }))
                }
                aside.rail.tk-rail { (star_side(&ctx, guild, &s, &stats, &authors, glyph)) }
            }
        } @else if view == "newest" {
            // A sub-tab: the newest posts as a strip of tiles, the numbers on the right.
            div.with-rail.tk-cols {
                section.tk-panel {
                    h2 { "Newest on the starboard" }
                    (strip(&ctx, guild, &rows, &images, 12))
                }
                aside.rail.tk-rail { (star_side(&ctx, guild, &s, &stats, &authors, glyph)) }
            }
        } @else {
            // The most starred post leads, with the numbers and the members beside it; the rest
            // follow as a strip of tiles.
            @let p = &rows[0];
            @let image = p.image.as_ref().and_then(|img| images.get(img));
            section.tk-panel.tk-lead.sb-lead {
                article.sb-hero.with-pic[image.is_some()] {
                    @if let Some(src) = image {
                        figure.sb-hero-pic { img src=(src) alt="" loading="lazy"; }
                    } @else {
                        (star_mark(p.stars))
                    }
                    div.sb-hero-text {
                        @if image.is_some() { (star_mark(p.stars)) }
                        @if !p.preview.is_empty() && !(image.is_some() && p.preview == "Attachment") {
                            p.sb-quote { (text::clip(&p.preview, 280)) }
                        }
                        div.sb-hero-foot {
                            (post_by(&ctx, guild, p, false))
                            (post_links(guild, p))
                        }
                    }
                }
                div.tk-lead-side { (star_members(&ctx, guild, &authors)) }
            }
            // The rest as a strip of tiles, the starboard's numbers in its head.
            section.tk-panel {
                div.sb-head {
                    div.sb-head-title {
                        h2 { @if rows.len() > 1 { "Also starred" } @else { "On the starboard" } }
                        (star_rules(&ctx, guild, &s, glyph))
                    }
                    (star_figures(&s, &stats))
                }
                @if rows.len() > 1 {
                    (strip(&ctx, guild, &rows[1..], &images, 16))
                } @else {
                    p.muted.small { "Nothing else is on the starboard yet." }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Starboard", None, body).into_response())
}

/// The starboard's numbers and its most starred members, for the rail.
fn star_side(ctx: &Ctx, guild: u64, s: &Settings, stats: &starboard::Stats, authors: &[(u64, i64)], glyph: &str) -> Markup {
    html! {
        div.rail-block {
            h2 { "On the starboard" }
            (star_figures(s, stats))
            (star_rules(ctx, guild, s, glyph))
        }
        div.rail-block { (star_members(ctx, guild, authors)) }
    }
}

/// Messages on the starboard, this week's, and the stars it takes.
fn star_figures(s: &Settings, stats: &starboard::Stats) -> Markup {
    html! {
        div.tk-figs.three {
            div { b { (text::num(stats.posts)) } span { "messages" } }
            div { b { (text::num(stats.this_week)) } span { "this week" } }
            div { b { (s.star_threshold) } span { "stars to get on" } }
        }
    }
}

/// How the starboard is set, in one muted line.
fn star_rules(ctx: &Ctx, guild: u64, s: &Settings, glyph: &str) -> Markup {
    html! {
        p.muted.small {
            (glyph) " at " (s.star_threshold) " · posts to "
            @if let Some(ch) = s.star_channel { (channel_name(ctx, guild, ch)) } @else { "no channel" }
            " · own stars " (if s.star_self { "count" } else { "don't count" })
        }
    }
}

/// The members with the most posts on the starboard, each with a bar.
fn star_members(ctx: &Ctx, guild: u64, authors: &[(u64, i64)]) -> Markup {
    let most = authors.first().map(|a| a.1).unwrap_or(1).max(1);
    html! {
        h2 { "Most starred members" }
        @if authors.is_empty() {
            p.muted.small { "Members show up here once a message of theirs is reposted." }
        }
        div.sb-members {
            @for (author, posts) in authors {
                div.tk-line {
                    img.avatar src=(ctx.member_avatar(guild, *author)) alt="" width="28" height="28" loading="lazy";
                    div.grow {
                        div.tk-line-top { strong { (ctx.display_name(guild, *author)) } span.mono { (posts) @if *posts == 1 { " post" } @else { " posts" } } }
                        span.meter { span style={ "width:" (posts * 100 / most) "%" } {} }
                    }
                }
            }
        }
    }
}

/// The lead's count: a gold star, as Discord's starboards draw it, beside the number.
fn star_mark(stars: i64) -> Markup {
    html! {
        div.sb-mark {
            svg.sb-mark-star viewBox="0 0 24 24" aria-hidden="true" {
                path d="M12 2.6l2.9 5.87 6.48.95-4.69 4.57 1.11 6.45L12 17.4l-5.8 3.04 1.11-6.45-4.69-4.57 6.48-.95z" {}
            }
            div.sb-mark-n { b { (text::num(stars)) } span { @if stars == 1 { "star" } @else { "stars" } } }
        }
    }
}

/// Who wrote a post, where and when, with its stars at the end when `stars`.
fn post_by(ctx: &Ctx, guild: u64, p: &starboard::Post, stars: bool) -> Markup {
    html! {
        div.tk-line {
            img.avatar src=(ctx.member_avatar(guild, p.author_id)) alt="" width="28" height="28" loading="lazy";
            div.grow {
                @if stars {
                    div.tk-line-top { strong { (ctx.display_name(guild, p.author_id)) } span.sb-count { (star()) (p.stars) } }
                } @else {
                    strong { (ctx.display_name(guild, p.author_id)) }
                }
                span { (channel_name(ctx, guild, p.channel_id)) " · " (text::ago(p.created_at)) }
            }
        }
    }
}

/// The original message and the repost, two buttons of one width.
fn post_links(guild: u64, p: &starboard::Post) -> Markup {
    html! {
        div.sb-links {
            (out_link(&format!("{}/{}", thread_url(guild, p.channel_id), p.message_id), "Original"))
            (out_link(&format!("{}/{}", thread_url(guild, p.post_channel_id), p.post_id), "Repost"))
        }
    }
}

/// Posts as a strip of small tiles: the picture, or the words when there's none, with the
/// stars on it, then who and where. Past `shown`, the rest fold under "Show N more".
fn strip(ctx: &Ctx, guild: u64, posts: &[starboard::Post], images: &std::collections::HashMap<String, String>, shown: usize) -> Markup {
    let tile = |p: &starboard::Post| {
        let image = p.image.as_ref().and_then(|img| images.get(img));
        // "Attachment" is what a post without text stores; the picture says it better.
        let words = (!p.preview.is_empty() && !(image.is_some() && p.preview == "Attachment")).then_some(p.preview.as_str());
        html! {
            article.sb-tile {
                div.sb-tile-top {
                    @if let Some(src) = image {
                        img src=(src) alt="" loading="lazy";
                        @if let Some(w) = words { p.sb-cap { (w) } }
                    } @else {
                        p.sb-tile-text.muted[words.is_none()] { (words.unwrap_or("No text")) }
                    }
                }
                (post_by(ctx, guild, p, true))
                (post_links(guild, p))
            }
        }
    };
    let (first, rest) = posts.split_at(shown.min(posts.len()));
    html! {
        div.sb-strip { @for p in first { (tile(p)) } }
        @if !rest.is_empty() {
            details.sb-more {
                summary.btn.small { "Show " (rest.len()) " more" }
                div.sb-strip { @for p in rest { (tile(p)) } }
            }
        }
    }
}

/// A starboard repost as Discord draws it: the count line, then the embed with the author, the
/// words, the picture and the link back.
fn repost_preview(ctx: &Ctx, guild: u64, p: &starboard::Post, image: Option<&str>, glyph: &str) -> Markup {
    html! {
        (dc::line(html! { (glyph) " " (p.stars) " in " span.dp-ping { (channel_name(ctx, guild, p.channel_id)) } }))
        (dc::embed(EMBED_CORAL, html! {
            div.dp-author {
                img src=(ctx.member_avatar(guild, p.author_id)) alt="" width="24" height="24" loading="lazy";
                (ctx.display_name(guild, p.author_id))
            }
            @if !p.preview.is_empty() && p.preview != "Attachment" { div.dp-desc { (p.preview) } }
            @if let Some(src) = image { img.tk-star-img src=(src) alt="" loading="lazy"; }
            (dc::fields([("Source", html! { span.tk-jump { "Jump to message" } })]))
        }))
    }
}

async fn save_starboard(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let mut s = (*ctx.settings::<Settings>(guild, KEY)?).clone();
    let emoji = form.string("star_emoji");
    if starboard::parse_emoji(&emoji).is_none() {
        return Err(WebError::Bad("Paste the emoji itself, like ⭐. A custom emoji looks like <:name:123456>.".into()));
    }
    s.star_channel = form.channel(&ctx, guild, "star_channel");
    s.star_emoji = emoji;
    s.star_threshold = form.int_in("star_threshold", 1, 100, 3);
    s.star_remove_below = form.bool("star_remove_below");
    s.star_self = form.bool("star_self");
    s.star_ignored = form.channels(&ctx, guild, "star_ignored");
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed starboard settings");
    Ok(saved(&page.path("tickets/starboard?view=settings")))
}

const CSS: &str = r#"
/* ---- Tickets and Starboard in the house style (2026-11-02) ----
   Every section and side block is one kind of bordered panel; never a panel inside a panel.
   The main tab of each page leads with one thing (the open count and its chart, the most
   starred post); the other tabs get the same panels without it. Flat: no gradients, glows or
   entrances of their own. */
.tk-panel { min-width: 0; display: flex; flex-direction: column; gap: 14px; background: var(--raised); border: 1px solid var(--edge-1); border-radius: 14px; padding: 18px 20px; }
.tk-panel h2, .tk-rail .rail-block > h2, .tk-compose h2 { display: flex; align-items: baseline; justify-content: space-between; gap: 8px; font-size: 14px; font-weight: 500; }
.tk-panel h2 .muted, .tk-rail h2 .muted, .tk-compose h2 .muted { font-size: 12px; font-weight: 400; }
.tk-out .icon { width: 14px; height: 14px; }
.tk-panel .empty, .tk-rail .empty { background: none; box-shadow: none; padding: 6px 0; }
.tk-panel .empty-icon, .tk-rail .empty-icon { animation: none; background: var(--control); color: var(--ink-2); box-shadow: none; }

/* Two columns: the main one and a right column of one fixed width, each side block a panel. */
.with-rail.tk-cols { grid-template-columns: minmax(0, 1fr) 400px; gap: 16px; }
.tk-rail { min-width: 0; background: none; box-shadow: none; padding: 0; gap: 16px; border-radius: 0; }
.tk-rail > .rail-block { background: var(--raised); border: 1px solid var(--edge-1); border-radius: 14px; padding: 18px 20px; }
.tk-rail > .rail-block + .rail-block { margin-top: 0; padding-top: 18px; border-top: 1px solid var(--edge-1); }
@media (max-width: 1180px) { .with-rail.tk-cols { grid-template-columns: minmax(0, 1fr) 320px; } }
@media (max-width: 920px) { .with-rail.tk-cols { grid-template-columns: minmax(0, 1fr); } }

/* The lead: one panel, the answer on the left, the people on the right past a hairline. */
.tk-lead { display: grid; grid-template-columns: minmax(0, 1fr) 400px; gap: 0; padding: 0; }
.tk-lead-main { min-width: 0; display: flex; flex-direction: column; gap: 22px; padding: 20px 22px 18px; }
.tk-lead-side { min-width: 0; display: flex; flex-direction: column; padding: 18px 20px; border-left: 1px solid var(--edge-1); }
.tk-lead-head { display: flex; align-items: flex-end; justify-content: space-between; flex-wrap: wrap; gap: 16px 40px; }
.tk-big { display: flex; align-items: baseline; gap: 12px; }
.tk-big b { font: 600 48px/1 var(--sans); letter-spacing: -0.03em; font-variant-numeric: tabular-nums; }
.tk-big span { font-size: 14px; color: var(--ink-2); }
.tk-chart { display: flex; flex-direction: column; gap: 12px; }
.tk-lead .area svg { height: 170px; }
.tk-rail .area svg { height: 80px; }
/* Only the lead chart draws in; a side chart is just there. */
.tk-rail .area .line, .tk-rail .area .fill, .tk-rail .area-end { animation: none; }
@container col (max-width: 1100px) {
  .tk-lead { grid-template-columns: minmax(0, 1fr); }
  .tk-lead-side { border-left: 0; border-top: 1px solid var(--edge-1); }
}

/* Figures: the lead's small ones in a row, two or three across in a side block. */
.tk-figs { display: flex; flex-wrap: wrap; gap: 12px 36px; }
.tk-figs > div { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.tk-figs b { font: 600 20px/1.2 var(--sans); letter-spacing: -0.01em; white-space: nowrap; font-variant-numeric: tabular-nums; }
.tk-figs span { display: inline-flex; align-items: center; gap: 6px; font-size: 12px; color: var(--ink-3); white-space: nowrap; }
.tk-figs .hot b { color: var(--waiting); }
.tk-figs .dot { width: 6px; height: 6px; }
.tk-rail .tk-figs, .tk-lead-side .tk-figs { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 14px 16px; }
.tk-figs.three, .tk-rail .tk-figs.three, .tk-lead-side .tk-figs.three { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 14px 16px; }

/* Side lines: a 28 px face, a 14 px name, a 13 px muted line or a thin bar. */
.tk-line { display: flex; align-items: center; gap: 12px; min-width: 0; }
.tk-line > .avatar { width: 28px; height: 28px; flex: none; }
.tk-line .grow { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.tk-line strong { font-size: 14px; font-weight: 500; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tk-line .grow > span:not(.meter) { font-size: 13px; color: var(--ink-3); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tk-line-top { display: flex; align-items: baseline; justify-content: space-between; gap: 8px; min-width: 0; }
.tk-line-top .mono { flex: none; font-size: 12px; color: var(--ink-3); }
.tk-line .meter { height: 4px; margin-top: 4px; }
.tk-line .meter > span { animation: none; }

/* ---- a ticket, as a row ----
   Narrow: face, subject and its state, what was said under it, the buttons under that. From
   900 px of column a row reads across: who and what, the conversation, where it stands, the
   buttons, in fixed columns so each lines up down the list. */
.tk-list { display: flex; flex-direction: column; }
.tk-row { display: grid; grid-template-columns: 28px minmax(0, 1fr) auto; grid-template-areas: "face body state" "face convo convo" "face acts acts"; gap: 8px 14px; align-items: start; padding: 14px 0; }
.tk-row + .tk-row { border-top: 1px solid var(--edge-1); }
.tk-row:first-child { padding-top: 2px; }
.tk-row:last-child { padding-bottom: 0; }
.tk-face { grid-area: face; position: relative; width: 28px; height: 28px; }
.tk-face .avatar { width: 28px; height: 28px; }
.tk-turn { position: absolute; right: -2px; bottom: -2px; width: 10px; height: 10px; border-radius: 50%; box-shadow: 0 0 0 2px var(--raised); }
.tk-turn.staff { background: var(--waiting); }
.tk-turn.member { background: var(--live); }
.tk-body { grid-area: body; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.tk-top { display: flex; align-items: baseline; gap: 8px; min-width: 0; }
.tk-num { flex: none; font: 500 12px var(--mono); color: var(--ink-3); }
.tk-subject { min-width: 0; font-size: 14px; font-weight: 500; overflow-wrap: anywhere; }
.tk-by { font-size: 13px; color: var(--ink-3); }
.tk-convo { grid-area: convo; display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.tk-convo:empty { display: none; }
.tk-said { align-self: flex-start; max-width: 100%; padding: 6px 11px; border-radius: 4px 12px 12px 12px; background: var(--well); color: var(--ink-2); font-size: 13px; line-height: 1.5; overflow-wrap: anywhere; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
.tk-reply { display: flex; align-items: flex-start; gap: 8px; min-width: 0; font-size: 13px; line-height: 20px; color: var(--ink-3); }
.tk-reply .avatar { width: 20px; height: 20px; flex: none; }
.tk-reply p { min-width: 0; overflow-wrap: anywhere; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
.tk-reply strong { color: var(--ink-2); font-weight: 500; }
.tk-state { grid-area: state; display: flex; }
.tk-acts { grid-area: acts; display: grid; grid-template-columns: 112px 112px; gap: 6px; }
.tk-acts form { display: flex; }
.tk-acts .btn { width: 100%; }
@container col (min-width: 700px) {
  .tk-row { grid-template-columns: 28px minmax(0, 1fr) auto auto; grid-template-areas: "face body state acts" "face convo convo convo"; column-gap: 20px; }
  .tk-state { padding-top: 3px; }
}
@container col (min-width: 1250px) {
  .tk-row { grid-template-columns: 28px minmax(0, 1fr) minmax(0, 1.2fr) 140px auto; grid-template-areas: "face body convo state acts"; column-gap: 24px; }
}

/* ---- settings beside the message they shape ----
   The form is the panel with its Save inside; the preview takes a fixed column and draws
   Discord at three quarters. */
.compose.tk-compose { gap: 16px; }
@container col (min-width: 1000px) {
  .compose.tk-compose:has(> .compose-preview) { grid-template-columns: minmax(0, 1fr) 500px; row-gap: 0; }
}
@container col (min-width: 1000px) and (max-width: 1200px) {
  .compose.tk-compose:has(> .compose-preview) { grid-template-columns: minmax(0, 1fr) 440px; }
}
.tk-compose > form.settings { background: var(--raised); border: 1px solid var(--edge-1); border-radius: 14px; padding: 18px 20px; display: flex; flex-direction: column; gap: 6px; }
.tk-compose > form.settings fieldset { display: flex; flex-direction: column; gap: 18px; }
.tk-compose > form.settings .block { gap: 4px; }
.tk-compose > form.settings .block + .block { padding-top: 18px; border-top: 1px solid var(--edge-1); }
.tk-compose > form.settings .card { background: none; border: 0; padding: 0; border-radius: 0; }
.tk-compose > form.settings .save-bar { padding-top: 8px; }
.tk-compose > form.settings textarea { min-height: 84px; }
/* The preview column holds the side blocks under the preview, so it scrolls with the page. */
.tk-compose > .compose-preview.tk-rail { position: static; }
.tk-compose .compose-preview .rail-block { gap: 12px; }

/* ---- a message as Discord draws it. Discord's own greys, since this is Discord. ---- */
.tk-dc { zoom: 0.75; padding: 14px 14px 16px; border-radius: 12px; background: #313338; }
.tk-dc .dp-post { color: #dbdee1; }
.tk-star-img { display: block; width: 100%; max-height: 200px; object-fit: cover; border-radius: 4px; }
.tk-jump { color: #00a8fc; }
.tk-rail .tk-dc + p { margin-top: -2px; }

/* ---- starboard ---- */
/* Starboards are gold on Discord, so the starboard's own colour is gold: its stars and the
   members' bars. Flat, one fill, no glow. */
.tk-panel, .tk-rail { --star: #f2b544; }
/* The most starred post leads: a gold star and its count (or the post's picture) beside the
   words, who and the links. The panel is as tall as that and the members beside it. */
.sb-hero { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: 28px; align-items: center; min-width: 0; padding: 20px 24px; }
.sb-hero-pic { margin: 0; width: 208px; aspect-ratio: 4 / 3; border-radius: 10px; overflow: hidden; background: var(--well); }
.sb-hero-pic img { display: block; width: 100%; height: 100%; object-fit: cover; }
.sb-hero-text { display: flex; flex-direction: column; gap: 14px; min-width: 0; }
.sb-mark { display: flex; align-items: center; gap: 14px; padding-right: 8px; }
.sb-mark-star { flex: none; width: 64px; height: 64px; fill: var(--star); stroke: var(--star); stroke-width: 1.6; stroke-linejoin: round; }
.sb-mark-n { display: flex; flex-direction: column; gap: 4px; }
.sb-mark-n b { font: 600 48px/1 var(--sans); letter-spacing: -0.03em; font-variant-numeric: tabular-nums; }
.sb-mark-n span { font-size: 13px; color: var(--ink-3); }
.with-pic .sb-mark { gap: 10px; }
.with-pic .sb-mark-star { width: 30px; height: 30px; }
.with-pic .sb-mark-n { flex-direction: row; align-items: baseline; gap: 8px; }
.with-pic .sb-mark-n b { font-size: 30px; }
.sb-quote { max-width: 68ch; font-size: 19px; font-weight: 500; line-height: 1.45; letter-spacing: -0.015em; text-wrap: pretty; overflow-wrap: anywhere; display: -webkit-box; -webkit-line-clamp: 4; -webkit-box-orient: vertical; overflow: hidden; }
.sb-hero-foot { display: flex; align-items: center; flex-wrap: wrap; gap: 12px 28px; }
.sb-hero-foot > .tk-line { flex: 0 1 auto; }
.sb-lead .tk-lead-side { gap: 12px; justify-content: center; }
.sb-members { display: flex; flex-direction: column; gap: 12px; }
.sb-members .meter > span { background: var(--star); }
/* Beside the lead, two across, so the members don't make the lead taller than its post. */
.sb-lead .sb-members { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 14px 20px; }
@container col (max-width: 760px) {
  .sb-hero { grid-template-columns: minmax(0, 1fr); }
  .sb-hero-pic { width: 100%; }
}
/* The second panel's head: its name and how the starboard is set, the numbers on the right. */
.sb-head { display: flex; align-items: flex-start; justify-content: space-between; flex-wrap: wrap; gap: 12px 32px; }
.sb-head-title { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
.sb-head .tk-figs.three { display: flex; gap: 12px 32px; }
.sb-head .tk-figs b { font-size: 18px; }

/* The rest: a strip of small tiles of one size. */
.sb-strip { display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); gap: 18px 14px; }
.sb-tile { display: flex; flex-direction: column; gap: 10px; min-width: 0; }
.sb-tile-top { position: relative; aspect-ratio: 4 / 3; border-radius: 10px; overflow: hidden; background: var(--well); }
.sb-tile-top > img { display: block; width: 100%; height: 100%; object-fit: cover; }
/* The margin sits outside the clamped box, so no part of a sixth line shows under it. */
.sb-tile-text { margin: 11px 13px; font-size: 13px; line-height: 1.5; color: var(--ink); overflow-wrap: anywhere; display: -webkit-box; -webkit-line-clamp: 4; -webkit-box-orient: vertical; overflow: hidden; }
.sb-tile-text.muted { color: var(--ink-3); }
.sb-cap { position: absolute; left: 0; right: 0; bottom: 0; padding: 6px 10px; background: rgba(10, 10, 12, 0.72); color: #f2f2f3; font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.sb-count { flex: none; display: inline-flex; align-items: center; gap: 4px; font: 500 12px var(--mono); color: var(--ink-2); }
.sb-count .sb-star { width: 12px; height: 12px; color: var(--star); }
.sb-tile .tk-line { gap: 10px; }
.sb-tile .tk-line strong { font-size: 13px; }
.sb-tile .tk-line .grow > span:not(.meter) { font-size: 12px; }
.sb-links { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 6px; }
.sb-hero .sb-links { grid-template-columns: repeat(2, 112px); }
.tk-pv-acts { display: flex; align-items: center; flex-wrap: wrap; gap: 6px; }
.sb-links .btn { width: 100%; }
.sb-more { display: flex; flex-direction: column; gap: 18px; }
.sb-more > summary { align-self: flex-start; list-style: none; }
.sb-more > summary::-webkit-details-marker { display: none; }
.sb-more[open] > summary { display: none; }
"#;
