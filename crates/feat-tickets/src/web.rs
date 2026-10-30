use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, html};
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};
use serde::Deserialize;

use crate::tickets::{self, Filter, Ticket};
use crate::{DEFAULT_PANEL_TEXT, DEFAULT_PANEL_TITLE, KEY, Settings, starboard};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/tickets", get(tickets_page))
        .route("/g/{guild}/tickets/settings", post(save_tickets))
        .route("/g/{guild}/tickets/panel", post(send_panel))
        .route("/g/{guild}/tickets/{number}/close", post(close_ticket))
        .route("/g/{guild}/tickets/starboard", get(starboard_page).post(save_starboard))
}

/// What the tables drop on a phone: secondary columns move under the main one.
const RESPONSIVE: &str = ".subj{max-width:340px}.pv{max-width:460px}.only-s{display:none}.row-actions{display:inline-flex;gap:6px;justify-content:flex-end}\
@media (max-width:700px){.hide-s{display:none}.only-s{display:block;margin-top:4px}.subj{max-width:170px}.pv{max-width:170px}\
.row-actions{flex-direction:column;align-items:stretch}.row-actions .btn{justify-content:center}}";

fn responsive() -> Markup {
    html! { style { (bot_core::maud::PreEscaped(RESPONSIVE)) } }
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

fn avatar(ctx: &Ctx, user: u64) -> String {
    let hash = ctx.cache.user(bot_core::twilight_model::id::Id::new(user)).and_then(|u| u.avatar.map(|a| a.to_string()));
    bot_core::discord::avatar_url(user, hash.as_deref())
}

fn channel_name(ctx: &Ctx, guild: u64, channel: u64) -> String {
    ctx.text_channels(guild).into_iter().find(|(id, _)| *id == channel).map(|(_, n)| format!("#{n}")).unwrap_or_else(|| "a deleted channel".into())
}

// ---- tickets ----

#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
}

async fn tickets_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<ListQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let now = now_ms();
    let (filter, active) = match q.status.as_deref() {
        Some("open") => (Filter::Open, "Open"),
        Some("closed") => (Filter::Closed, "Closed"),
        _ => (Filter::All, "All"),
    };
    let stats = ctx.db.with(|c| tickets::stats(c, guild, now))?;
    let rows = ctx.db.with(|c| tickets::list(c, guild, filter, 100))?;

    let body = html! {
        (responsive())
        (ui::readings(&[
            ("open now", stats.open.to_string()),
            ("opened this week", stats.this_week.to_string()),
            ("median time to close", stats.median_close_secs.map(span).unwrap_or_else(|| "none yet".into())),
        ]))
        (ui::section("Tickets", None, html! {
            (ui::tabs(
                &[
                    ("All", page.path("tickets")),
                    ("Open", format!("{}?status=open", page.path("tickets"))),
                    ("Closed", format!("{}?status=closed", page.path("tickets"))),
                ],
                active,
            ))
            @if rows.is_empty() {
                @if filter == Filter::All {
                    (ui::empty(Icon::Ticket, "No tickets yet", "Pick a ticket channel and staff role below, send the panel, and tickets appear here as members open them."))
                } @else {
                    (ui::empty(Icon::Ticket, "Nothing here", "No tickets match this filter."))
                }
            } @else {
                div.table-wrap { table.table {
                    thead { tr {
                        th.num style="text-align:left" { "#" } th { "Subject" } th.hide-s { "Member" } th.hide-s { "Status" }
                        th.hide-s { "Opened" } th.hide-s { "Open for" } th {}
                    } }
                    tbody { @for t in &rows { (ticket_row(&ctx, &page, guild, t, now)) } }
                } }
                @if rows.len() == 100 { p.muted.small { "Showing the latest 100." } }
            }
        }))
        (ui::section("Settings", None, ui::settings_form(&page, &page.path("tickets/settings"), ui::card(html! {
            (ui::field_channel(&ctx, guild, "ticket_channel", "Ticket channel", Some("Tickets are private threads in this channel. The panel is posted here too."), s.ticket_channel))
            (ui::field_role(&ctx, guild, "staff_role", "Staff role", Some("Pinged once when a ticket opens, and allowed to close any ticket."), s.staff_role))
            (ui::field_number("max_open", "Open tickets per member", Some("A member must close one before opening another past this number."), s.max_open, 1, 10, None))
            (ui::field_number("auto_close_days", "Close idle tickets after", Some("Days with no messages in the thread. 0 leaves them open."), s.auto_close_days, 0, 90, Some("days")))
            (ui::field_text("panel_title", "Panel title", None, &s.panel_title, DEFAULT_PANEL_TITLE))
            (ui::field_textarea("panel_text", "Panel text", Some("Shown above the Open a ticket button."), &s.panel_text, DEFAULT_PANEL_TEXT))
        }))))
        (ui::section("Panel", Some("Save the settings first. Sending a new panel removes the previous one."), html! {
            @let title = if s.panel_title.trim().is_empty() { DEFAULT_PANEL_TITLE } else { s.panel_title.trim() };
            @let blurb = if s.panel_text.trim().is_empty() { DEFAULT_PANEL_TEXT } else { s.panel_text.trim() };
            (ui::embed_preview("#f0806a", title, blurb))
            @if page.can_edit() {
                div.inline-form style="margin-top:12px" {
                    @if s.ticket_channel.is_some() {
                        (ui::action_button(&page.path("tickets/panel"), if s.panel.is_some() { "Send a new panel" } else { "Send panel" }, Tone::Accent, false))
                    } @else {
                        span.muted.small { "Pick a ticket channel to send the panel." }
                    }
                    @if let Some((channel, message)) = s.panel {
                        a.btn.small href={ (thread_url(guild, channel)) "/" (message) } target="_blank" rel="noopener" { "View current panel" }
                    }
                }
            }
        }))
    };
    Ok(ui::layout(&ctx, &page, "Tickets", None, body).into_response())
}

fn ticket_row(ctx: &Ctx, page: &Page, guild: u64, t: &Ticket, now: i64) -> Markup {
    let name = ctx.display_name(guild, t.opener_id);
    let open_for = span(((t.closed_at.unwrap_or(now) - t.opened_at) / 1000).max(0));
    let (label, tone) = match t.status.as_str() {
        tickets::OPEN => ("Open", Tone::Ok),
        tickets::IDLE => ("Auto-closed", Tone::Warn),
        _ => ("Closed", Tone::Muted),
    };
    let closer = t.closed_by.filter(|_| t.status == tickets::CLOSED).map(|u| format!("by {}", ctx.display_name(guild, u)));
    html! {
        tr {
            td.num style="text-align:left" { (format!("{:04}", t.number)) }
            td title=[t.reason.as_deref()] {
                div.clip.subj { (t.subject) }
                div.only-s.small { (ui::badge(label, tone)) " " span.muted { (name) " · " (open_for) } }
            }
            td.hide-s { (ui::person(&name, &avatar(ctx, t.opener_id), None)) }
            td.hide-s { (ui::badge(label, tone)) @if let Some(c) = &closer { " " span.muted.small { (c) } } }
            td.muted.hide-s { (text::ago(t.opened_at)) }
            td.num.hide-s { (open_for) }
            td.actions { div.row-actions {
                @if t.thread_id != 0 { a.btn.small href=(thread_url(guild, t.thread_id)) target="_blank" rel="noopener" { "Open thread" } }
                @if page.can_edit() && t.is_open() { (ui::action_button(&page.path(&format!("tickets/{}/close", t.number)), "Close", Tone::Bad, true)) }
            } }
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
    Ok(saved(&page.path("tickets")))
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

// ---- starboard ----

async fn starboard_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let now = now_ms();
    let stats = ctx.db.with(|c| starboard::stats(c, guild, now))?;
    let rows = ctx.db.with(|c| starboard::top(c, guild, 25))?;

    let body = html! {
        (responsive())
        (ui::readings(&[
            ("on the starboard", text::num(stats.posts)),
            ("added this week", text::num(stats.this_week)),
            ("most stars", text::num(stats.most_stars)),
        ]))
        (ui::section("Most starred", None, html! {
            @if rows.is_empty() {
                (ui::empty(Icon::Star, "Nothing starred yet", if s.star_channel.is_some() {
                    "A message shows up here once it gets enough stars."
                } else {
                    "Pick a starboard channel below. Messages show up here once they get enough stars."
                }))
            } @else {
                div.table-wrap { table.table {
                    thead { tr { th.num { "Stars" } th { "Message" } th.hide-s { "Channel" } th.hide-s { "Added" } th {} } }
                    tbody { @for p in &rows {
                        tr {
                            td.num { (p.stars) }
                            td {
                                (ui::person(&ctx.display_name(guild, p.author_id), &avatar(&ctx, p.author_id), None))
                                @if !p.preview.is_empty() { div.muted.small.clip.pv style="margin-top:2px" { (p.preview) } }
                                div.only-s.small.muted { (channel_name(&ctx, guild, p.channel_id)) " · " (text::ago(p.created_at)) }
                            }
                            td.muted.hide-s { (channel_name(&ctx, guild, p.channel_id)) }
                            td.muted.hide-s { (text::ago(p.created_at)) }
                            td.actions { div.row-actions {
                                a.btn.small href={ (thread_url(guild, p.channel_id)) "/" (p.message_id) } target="_blank" rel="noopener" { "Original" }
                                a.btn.small href={ (thread_url(guild, p.post_channel_id)) "/" (p.post_id) } target="_blank" rel="noopener" { "Post" }
                            } }
                        }
                    } }
                } }
            }
        }))
        (ui::section("Settings", None, ui::settings_form(&page, &page.path("tickets/starboard"), ui::card(html! {
            (ui::field_channel(&ctx, guild, "star_channel", "Starboard channel", Some("Where popular messages are reposted. None turns the starboard off."), s.star_channel))
            (ui::field_text("star_emoji", "Emoji", Some("Paste the emoji itself. For a custom one, paste it as <:name:id>."), &s.star_emoji, "⭐"))
            (ui::field_number("star_threshold", "Stars needed", Some("A message is reposted once it has this many."), s.star_threshold, 1, 100, Some("stars")))
            (ui::field_toggle("star_remove_below", "Remove when it drops below", Some("Takes the post down if stars fall under the number above. Off keeps it with the lower count."), s.star_remove_below))
            (ui::field_toggle("star_self", "Count the author's own star", Some("Off: a member starring their own message doesn't count."), s.star_self))
            (ui::field_channels(&ctx, guild, "star_ignored", "Ignored channels", Some("Stars in these channels never reach the starboard. The starboard channel is always ignored."), &s.star_ignored))
        }))))
    };
    Ok(ui::layout(&ctx, &page, "Starboard", None, body).into_response())
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
    Ok(saved(&page.path("tickets/starboard")))
}
