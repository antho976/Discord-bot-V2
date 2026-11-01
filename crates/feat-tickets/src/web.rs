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
        a.btn.small.ghost.tk-out href=(href) target="_blank" rel="noopener" { (ui::icon(Icon::External)) (label) }
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
fn opened_per_day(ctx: &Ctx, page: &Page, guild: u64, now: i64, days: i64) -> Vec<(String, Option<i64>)> {
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
            (day.strftime("%b %-d").to_string(), Some(n))
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
                (ui::settings_form(&page, &page.path("tickets/settings"), ui::card(html! {
                    (ui::field_channel(&ctx, guild, "ticket_channel", "Ticket channel", Some("Tickets are private threads in this channel. The panel is posted here too."), s.ticket_channel))
                    (ui::field_role(&ctx, guild, "staff_role", "Staff role", Some("Pinged once when a ticket opens, and allowed to close any ticket."), s.staff_role))
                    (ui::field_number("max_open", "Open tickets per member", Some("A member must close one before opening another past this number. Members also wait 10 minutes between tickets; staff don't."), s.max_open, 1, 10, None))
                    (ui::field_number("auto_close_days", "Close idle tickets after", Some("Days with no messages in the thread. 0 leaves them open."), s.auto_close_days, 0, 90, Some("days")))
                    (ui::field_text("panel_title", "Panel title", None, &s.panel_title, DEFAULT_PANEL_TITLE))
                    (ui::field_textarea("panel_text", "Panel text", Some("Shown above the Open a ticket button."), &s.panel_text, DEFAULT_PANEL_TEXT))
                })))
                aside.rail.tk-rail.compose-preview {
                    div.rail-block {
                        h2 {
                            "The panel"
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
                            div.tk-acts {
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
                }
            }
        } @else {
            // The answer first, across the page: how many are open, how many wait on staff.
            div.readings.tk-readings {
                div.reading { b.mono data-count { (text::num(stats.open)) } span { "open now" } }
                div.reading.tk-hot[stats.waiting > 0] {
                    b.mono data-count { (text::num(stats.waiting)) }
                    span { @if stats.waiting > 0 { span.dot.waiting {} } "waiting on staff" }
                }
                div.reading { b.mono data-count { (text::num(stats.this_week)) } span { "opened this week" } }
                div.reading { b.mono { (stats.median_close_secs.map(span).unwrap_or_else(|| "none yet".into())) } span { "usual time to close" } }
            }
            div.with-rail {
                div.tk-main {
                    @if rows.is_empty() {
                        @if view == "closed" {
                            (ui::empty(Icon::Ticket, "Nothing closed yet", "Closed tickets stay here with who closed them and how long they took."))
                        } @else {
                            (ui::empty(Icon::Ticket, "No open tickets", "Pick a ticket channel and staff role in Settings, send the panel, and tickets appear here as members open them."))
                        }
                    } @else {
                        div.tk-list {
                            @for (i, t) in rows.iter().enumerate() { (ticket_card(&ctx, &page, guild, t, now, i)) }
                        }
                        @if rows.len() == 100 { p.muted.small { "Showing the latest 100." } }
                    }
                }
                aside.rail.tk-rail {
                    div.rail-block {
                        h2 { "Opened, last 14 days" span.muted { (text::num(stats.this_week)) " this week" } }
                        (ui::mini_bars(&opened_per_day(&ctx, &page, guild, now, 14), "opened", "Tickets opened on each of the last 14 days"))
                    }
                    (answering(&ctx, guild, now))
                    (closed_recently(&ctx, guild, now))
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Tickets", actions, body).into_response())
}

/// One ticket as a short conversation: the opener's face and words, the latest staff reply
/// with its face, whose turn it is, and how it ended once closed.
fn ticket_card(ctx: &Ctx, page: &Page, guild: u64, t: &Ticket, now: i64, i: usize) -> Markup {
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
    let has_convo = !t.is_open() || !t.details.is_empty() || act.staff_reply.is_some() || answered.is_some();
    html! {
        article.tk-card.wait[staff_turn] style={ "--i:" (i.min(10)) } {
            div.tk-face {
                img.avatar src=(ctx.member_avatar(guild, t.opener_id)) alt="" width="44" height="44" loading="lazy";
                @if t.is_open() { span class={ "tk-turn " (if staff_turn { "staff" } else { "member" }) } {} }
            }
            div.tk-body {
                div.tk-top {
                    span.tk-num { (format!("{:04}", t.number)) }
                    strong.tk-subject { (t.subject) }
                }
                div.tk-by {
                    strong { (name) } " · opened " (text::ago(t.opened_at))
                    @if t.is_open() {
                        @if let Some(at) = act.last_reply_at { " · last reply " (text::ago(at)) }
                    }
                }
            }
            // What was said and how it ended: its own column on a wide page.
            @if has_convo {
                div.tk-convo {
                    @if !t.details.is_empty() || act.staff_reply.is_some() || answered.is_some() {
                        div.tk-thread {
                            @if !t.details.is_empty() { p.tk-said { (text::clip(&t.details, 240)) } }
                            @if let Some((who, said)) = &act.staff_reply {
                                div.tk-reply {
                                    img.avatar src=(ctx.member_avatar(guild, *who)) alt="" width="24" height="24" loading="lazy";
                                    p.tk-said.staff { strong { (ctx.display_name(guild, *who)) } (said) }
                                }
                            }
                            @if let Some(who) = answered {
                                div.tk-reply.quiet {
                                    img.avatar src=(ctx.member_avatar(guild, who)) alt="" width="24" height="24" loading="lazy";
                                    span {
                                        strong { (ctx.display_name(guild, who)) } " answered"
                                        @if let Some(at) = t.last_message_at { " " (text::ago(at)) }
                                    }
                                }
                            }
                        }
                    }
                    @if !t.is_open() {
                        div.tk-ended {
                            @if let Some(by) = t.closed_by {
                                img.avatar src=(ctx.member_avatar(guild, by)) alt="" width="22" height="22" loading="lazy";
                            }
                            span {
                                (label) " " (text::ago(t.closed_at.unwrap_or(now)))
                                @if let Some(by) = t.closed_by { " by " strong { (ctx.display_name(guild, by)) } }
                                @if let Some(took) = &took { " · took " (took) }
                                @if let Some(reason) = t.reason.as_deref() { " · " (reason) }
                            }
                        }
                    }
                }
            }
            div.tk-side {
                @if t.is_open() {
                    @if let Some((w, staff)) = &act.waiting { (ui::badge(w, if *staff { Tone::Warn } else { Tone::Muted })) }
                } @else {
                    (ui::badge(label, tone))
                }
                div.tk-acts {
                    @if t.thread_id != 0 { (out_link(&thread_url(guild, t.thread_id), "Open thread")) }
                    @if page.can_edit() && t.is_open() { (ui::action_button(&page.path(&format!("tickets/{}/close", t.number)), "Close", Tone::Bad, true)) }
                    @if page.can_edit() && t.can_reopen(now) { (ui::action_button(&page.path(&format!("tickets/{}/reopen", t.number)), "Reopen", Tone::Muted, false)) }
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
                div.mini {
                    img.avatar src=(ctx.member_avatar(guild, t.opener_id)) alt="" width="30" height="30" loading="lazy";
                    div.grow {
                        span { (t.subject) }
                        span.muted {
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
    html! {
        div.rail-block {
            h2 { "Who's answering" span.muted { "last 30 days" } }
            @if who.is_empty() {
                p.muted.small { "Nobody has closed a ticket in the last 30 days." }
            } @else {
                div.tk-who {
                    @for (i, (member, closed)) in who.iter().enumerate() {
                        div.tk-who-one.first[i == 0] style={ "--i:" (i) } {
                            img.avatar src=(ctx.member_avatar(guild, *member)) alt="" width="52" height="52" loading="lazy";
                            strong { (ctx.display_name(guild, *member)) }
                            span { (closed) " closed" }
                        }
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
    let most_posts = authors.first().map(|a| a.1).unwrap_or(1).max(1);
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
                (ui::settings_form(&page, &page.path("tickets/starboard"), ui::card(html! {
                    (ui::field_channel(&ctx, guild, "star_channel", "Starboard channel", Some("Where popular messages are reposted. None turns the starboard off."), s.star_channel))
                    (ui::field_text("star_emoji", "Emoji", Some("Paste the emoji itself. For a custom one, paste it as <:name:id>."), &s.star_emoji, "⭐"))
                    (ui::field_number("star_threshold", "Stars needed", Some("A message is reposted once it has this many."), s.star_threshold, 1, 100, Some("stars")))
                    (ui::field_toggle("star_remove_below", "Remove when it drops below", Some("Takes the post down if stars fall under the number above, once it has been up for 10 minutes. Off keeps it with the lower count."), s.star_remove_below))
                    (ui::field_toggle("star_self", "Count the author's own star", Some("Off: a member starring their own message doesn't count."), s.star_self))
                    (ui::field_channels(&ctx, guild, "star_ignored", "Ignored channels", Some("Stars in these channels and their threads never reach the starboard. Neither do the starboard itself, private threads, channels that some people who can see the starboard can't see, or age-restricted channels unless the starboard is one."), &s.star_ignored))
                })))
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
                            div { (ui::action_button(&page.path("tickets/starboard/test"), "Send a test post", Tone::Muted, false)) }
                        }
                    }
                }
            }
        } @else {
            div.with-rail {
                div.tk-main {
                    @if rows.is_empty() {
                        (ui::empty(Icon::Star, "Nothing starred yet", if s.star_channel.is_some() {
                            "A message shows up here once it gets enough stars."
                        } else {
                            "Pick a starboard channel in Settings. Messages show up here once they get enough stars."
                        }))
                    } @else {
                        div.sb-grid {
                            @for (i, p) in rows.iter().enumerate() {
                                @let image = p.image.as_ref().and_then(|img| images.get(img));
                                // "Attachment" is what a post without text stores; the picture says it better.
                                @let words = (!p.preview.is_empty() && !(image.is_some() && p.preview == "Attachment")).then_some(p.preview.as_str());
                                article.sb-card.lead[i == 0 && view != "newest"].quote[image.is_none()] style={ "--i:" (i.min(10)) } {
                                    @if let Some(src) = image {
                                        figure.sb-pic {
                                            img src=(src) alt="" loading="lazy";
                                            span.sb-count { (star()) (p.stars) }
                                        }
                                    }
                                    div.sb-body {
                                        @match words {
                                            Some(w) => p.sb-text { (w) },
                                            None => @if image.is_none() { p.sb-text.muted { "No text" } },
                                        }
                                        footer.sb-by {
                                            img.avatar src=(ctx.member_avatar(guild, p.author_id)) alt="" width="30" height="30" loading="lazy";
                                            div.grow {
                                                strong { (ctx.display_name(guild, p.author_id)) }
                                                span { (channel_name(&ctx, guild, p.channel_id)) " · " (text::ago(p.created_at)) }
                                            }
                                            @if image.is_none() { span.sb-count.inline { (star()) (p.stars) } }
                                        }
                                        div.sb-links {
                                            (out_link(&format!("{}/{}", thread_url(guild, p.channel_id), p.message_id), "Original"))
                                            (out_link(&format!("{}/{}", thread_url(guild, p.post_channel_id), p.post_id), "Repost"))
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                aside.rail.tk-rail {
                    div.rail-block {
                        h2 { "On the starboard" }
                        div.rail-nums {
                            div { b.mono data-count { (text::num(stats.posts)) } span { "messages" } }
                            div { b.mono data-count { (text::num(stats.this_week)) } span { "this week" } }
                            div { b.mono { (s.star_threshold) } span { "stars to get on" } }
                        }
                    }
                    div.rail-block {
                        h2 { "Most starred members" }
                        @if authors.is_empty() {
                            p.muted.small { "Members show up here once a message of theirs is reposted." }
                        }
                        @for (author, posts) in &authors {
                            div.sb-author {
                                img.avatar src=(ctx.member_avatar(guild, *author)) alt="" width="30" height="30" loading="lazy";
                                div.grow {
                                    div { strong { (ctx.display_name(guild, *author)) } span.mono { (posts) @if *posts == 1 { " post" } @else { " posts" } } }
                                    span.meter { span style={ "width:" (posts * 100 / most_posts) "%" } {} }
                                }
                            }
                        }
                    }
                    p.muted.small.sb-rules {
                        (glyph) " at " (s.star_threshold) " · posts to "
                        @if let Some(ch) = s.star_channel { (channel_name(&ctx, guild, ch)) } @else { "no channel" }
                        " · own stars " (if s.star_self { "count" } else { "don't count" })
                    }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Starboard", None, body).into_response())
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
.tk-main { min-width: 0; display: flex; flex-direction: column; gap: 22px; }
/* The phone layout's single column grows to its widest child; keep the rail from widening it. */
.tk-rail { min-width: 0; }
.tk-out .icon { width: 14px; height: 14px; }
.tk-acts { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; }
.tk-rail .rail-block > h2 .muted { font-size: 12px; font-weight: 400; }
.tk-rail .rail-nums b { font-size: 24px; }
.tk-readings .reading { flex: 1 1 140px; }
.tk-readings .reading > span { display: inline-flex; align-items: center; gap: 7px; }
.reading.tk-hot { background: var(--well); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--waiting) 34%, transparent); }
.reading.tk-hot b { color: var(--waiting); }
.reading.tk-hot .dot { width: 6px; height: 6px; }
/* Settings beside the message they shape: the preview keeps about Discord's own width, the form
   takes the rest. */
@container col (min-width: 1000px) {
  .compose.tk-compose { grid-template-columns: minmax(0, 1fr) min(600px, 42%); }
}

/* ---- a ticket, as a short conversation ----
   Narrow: face, subject, what was said under it, badge and actions in a foot. From 600 px the
   badge and actions sit at the right. From 1000 px what was said takes a column of its own, so a
   row reads across: who and what, the conversation, where it stands. */
.tk-list { display: flex; flex-direction: column; gap: 10px; }
.tk-card { display: grid; grid-template-columns: 40px minmax(0, 1fr); grid-template-areas: "face body" "face convo" "side side"; gap: 10px 14px; align-items: start; padding: 14px; border-radius: 18px; background: linear-gradient(180deg, rgba(243, 242, 248, 0.045), rgba(243, 242, 248, 0.015)); box-shadow: inset 0 0 0 1px var(--edge-1); transition: box-shadow 0.25s; animation: rise 0.6s var(--ease) both; animation-delay: calc(var(--i, 0) * 60ms + 200ms); }
.tk-card:hover { box-shadow: inset 0 0 0 1px var(--edge-2), var(--shadow-lift); }
.tk-card.wait { background: linear-gradient(180deg, rgba(243, 242, 248, 0.045), rgba(243, 242, 248, 0.015)); }
.tk-face { grid-area: face; position: relative; width: 40px; height: 40px; }
.tk-face .avatar { width: 40px; height: 40px; }
.tk-turn { position: absolute; right: -2px; bottom: -2px; width: 14px; height: 14px; border-radius: 50%; box-shadow: 0 0 0 3px var(--page); }
.tk-turn.staff { background: var(--waiting); }
.tk-turn.staff::after { content: ""; position: absolute; inset: 0; border-radius: 50%; background: var(--waiting); animation: ping 2.2s ease-out 4; }
.tk-turn.member { background: var(--live); }
.tk-body { grid-area: body; display: flex; flex-direction: column; gap: 8px; min-width: 0; }
.tk-convo { grid-area: convo; display: flex; flex-direction: column; gap: 8px; min-width: 0; }
.tk-top { display: flex; align-items: baseline; gap: 10px; min-width: 0; }
.tk-num { flex: none; padding: 1px 7px; border-radius: 6px; background: color-mix(in oklab, var(--hue) 14%, transparent); color: var(--hue); font: 600 11.5px var(--mono); }
.tk-subject { font-size: 15px; font-weight: 600; letter-spacing: -0.01em; min-width: 0; overflow-wrap: anywhere; }
.tk-by { font-size: 12.5px; color: var(--ink-3); margin-top: -4px; }
.tk-by strong { color: var(--ink-2); font-weight: 500; }
.tk-thread { display: flex; flex-direction: column; gap: 8px; max-width: 560px; }
.tk-said { align-self: flex-start; padding: 8px 12px; border-radius: 4px 14px 14px 14px; background: rgba(243, 242, 248, 0.055); box-shadow: inset 0 0 0 1px var(--edge-1); color: var(--ink-2); font-size: 13px; line-height: 1.5; overflow-wrap: anywhere; display: -webkit-box; -webkit-line-clamp: 3; -webkit-box-orient: vertical; overflow: hidden; }
.tk-reply { display: flex; align-items: flex-start; gap: 8px; }
.tk-reply .avatar { width: 24px; height: 24px; margin-top: 3px; }
.tk-reply.quiet { align-items: center; font-size: 12.5px; color: var(--ink-3); }
.tk-reply.quiet .avatar { margin-top: 0; box-shadow: 0 0 0 2px var(--page), 0 0 0 3.5px color-mix(in oklab, var(--accent) 60%, transparent); }
.tk-reply.quiet strong { color: var(--ink-2); font-weight: 600; }
.tk-said.staff { background: color-mix(in oklab, var(--accent) 12%, transparent); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--accent) 24%, transparent); }
.tk-said strong { display: block; color: var(--ink); font-weight: 600; font-size: 12.5px; }
.tk-ended { display: flex; align-items: center; gap: 8px; font-size: 12.5px; color: var(--ink-3); }
.tk-ended .avatar { width: 22px; height: 22px; }
.tk-ended strong { color: var(--ink-2); font-weight: 500; }
.tk-side { grid-area: side; display: flex; flex-direction: row; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 10px; }
.tk-side .tk-acts { flex-wrap: nowrap; }
@container col (min-width: 600px) {
  .tk-card { grid-template-columns: 44px minmax(0, 1fr) auto; grid-template-areas: "face body side" "face convo side"; gap: 8px 16px; padding: 16px 18px; }
  .tk-face, .tk-face .avatar { width: 44px; height: 44px; }
  .tk-side { flex-direction: column; flex-wrap: nowrap; align-items: flex-end; justify-content: flex-start; }
  .tk-reply { padding-left: 18px; }
}
@container col (min-width: 1000px) {
  .tk-card { grid-template-columns: 44px minmax(0, 0.85fr) minmax(0, 1.15fr) auto; grid-template-areas: "face body convo side"; column-gap: 28px; }
  .tk-convo { padding-top: 1px; }
}

.tk-who { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 8px; align-items: end; }
.tk-who-one { display: flex; flex-direction: column; align-items: center; gap: 3px; min-width: 0; text-align: center; font-size: 12.5px; animation: rise 0.6s var(--ease) both; animation-delay: calc(var(--i, 0) * 90ms + 300ms); }
.tk-who-one .avatar { width: 44px; height: 44px; margin-bottom: 5px; box-shadow: 0 0 0 2px var(--page), 0 0 0 3.5px var(--edge-3); }
.tk-who-one.first .avatar { width: 54px; height: 54px; box-shadow: 0 0 0 2px var(--page), 0 0 0 3.5px var(--hue); }
.tk-who-one strong { max-width: 100%; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tk-who-one span { font-size: 11.5px; color: var(--ink-3); }

/* ---- a message as Discord draws it. Discord's own greys, since this is Discord. ---- */
.tk-dc { padding: 14px 14px 16px; border-radius: 14px; background: #313338; box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.05), 0 18px 40px -24px rgba(0, 0, 0, 0.9); animation: tk-arrive 0.8s var(--ease) both 0.35s; }
.tk-star-img { display: block; width: 100%; max-height: 200px; object-fit: cover; border-radius: 4px; }
.tk-jump { color: #00a8fc; }
.tk-rail .tk-dc + p { margin-top: -2px; }

/* ---- starboard: the posts as pictures and quotes ---- */
/* A gallery that gains columns with the width; posts differ in height, so they pack. */
.sb-grid { columns: 300px; column-gap: 14px; }
.sb-card { position: relative; break-inside: avoid; display: flex; flex-direction: column; margin: 0 0 14px; border-radius: 18px; overflow: hidden; isolation: isolate; background: linear-gradient(180deg, rgba(243, 242, 248, 0.05), rgba(243, 242, 248, 0.018)); box-shadow: inset 0 0 0 1px var(--edge-1); animation: sb-in 0.75s var(--ease) both; animation-delay: calc(var(--i, 0) * 80ms + 150ms); transition: box-shadow 0.25s; }
.sb-card:hover { box-shadow: inset 0 0 0 1px var(--edge-2), var(--shadow-lift); }
.sb-card.lead { box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--hue) 45%, transparent); background: linear-gradient(180deg, rgba(243, 242, 248, 0.05), rgba(243, 242, 248, 0.018)); }
.sb-pic { position: relative; margin: 0; overflow: hidden; background: var(--well); }
.sb-pic img { display: block; width: 100%; height: auto; max-height: 340px; object-fit: cover; transition: transform 1.1s var(--ease), filter 0.4s; }
.sb-card:hover .sb-pic img { transform: scale(1.05); filter: saturate(1.12); }
.sb-count { position: absolute; top: 10px; right: 10px; display: inline-flex; align-items: center; gap: 5px; height: 26px; padding: 0 10px 0 8px; border-radius: 13px; background: rgba(7, 7, 11, 0.62); backdrop-filter: blur(10px); color: #fff; font: 600 12.5px var(--sans); font-variant-numeric: tabular-nums; box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.14); animation: pop-in 0.55s var(--spring) both; animation-delay: calc(var(--i, 0) * 80ms + 650ms); }
.sb-count .sb-star { color: var(--hue); width: 14px; height: 14px; }
.sb-count.inline { position: static; flex: none; background: color-mix(in oklab, var(--hue) 14%, transparent); color: var(--hue); backdrop-filter: none; box-shadow: none; }
.sb-card.lead .sb-count { background: var(--hue); color: #1c1305; box-shadow: none; }
.sb-card.lead .sb-count .sb-star { color: inherit; }
.sb-body { display: flex; flex-direction: column; gap: 12px; padding: 14px 16px 10px; }
.sb-text { font-size: 14px; line-height: 1.55; overflow-wrap: anywhere; color: var(--ink); }
.sb-card.quote .sb-text { font-size: 15.5px; font-weight: 500; line-height: 1.5; letter-spacing: -0.01em; text-wrap: pretty; }
.sb-card.lead.quote .sb-text { font-size: 17px; }
.sb-by { display: flex; align-items: center; gap: 10px; min-width: 0; }
.sb-by .avatar { width: 30px; height: 30px; }
.sb-by .grow { display: flex; flex-direction: column; min-width: 0; }
.sb-by strong { font-size: 13px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sb-by .grow span { font-size: 12px; color: var(--ink-3); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sb-links { display: flex; gap: 2px; margin: -4px -8px 0; }
.sb-links .btn { color: var(--ink-3); }
.sb-links .btn:hover { color: var(--ink); }
.sb-author { display: flex; align-items: center; gap: 11px; padding: 3px 0; }
.sb-author .avatar { width: 30px; height: 30px; }
.sb-author .grow { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.sb-author .grow > div { display: flex; justify-content: space-between; gap: 8px; font-size: 13px; }
.sb-author strong { font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sb-author .mono { color: var(--ink-3); flex: none; }
.sb-author .meter { height: 4px; }
.sb-rules { margin-top: 2px; }

@keyframes tk-arrive { from { opacity: 0; transform: translateY(16px) scale(0.97); filter: blur(4px); } }
@keyframes sb-in { from { opacity: 0; transform: translateY(18px) scale(0.96); filter: blur(6px); } }

"#;
