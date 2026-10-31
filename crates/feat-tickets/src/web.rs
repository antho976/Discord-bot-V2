use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::twilight_model::id::Id;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};
use serde::Deserialize;

use crate::tickets::{self, Filter, Ticket};
use crate::{DEFAULT_PANEL_TEXT, DEFAULT_PANEL_TITLE, KEY, Settings, starboard};

const DAY_MS: i64 = 86_400_000;

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/tickets", get(tickets_page))
        .route("/g/{guild}/tickets/settings", post(save_tickets))
        .route("/g/{guild}/tickets/panel", post(send_panel))
        .route("/g/{guild}/tickets/{number}/close", post(close_ticket))
        .route("/g/{guild}/tickets/starboard", get(starboard_page).post(save_starboard))
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

// ---- tickets ----

#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
}

/// What the thread says, read from the message cache: the last reply, and whether it's the
/// member's turn or staff's. Empty when the thread's messages aren't cached (after a restart,
/// or a thread nobody has spoken in).
#[derive(Default)]
struct Activity {
    last_reply_at: Option<i64>,
    /// `(who, what they wrote)` for the latest reply from someone other than the opener.
    staff_reply: Option<(String, String)>,
    waiting: Option<String>,
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
    act.staff_reply = messages
        .iter()
        .find(|(author, _)| *author != t.opener_id)
        .map(|(author, said)| (ctx.display_name(guild, *author), text::clip(said, 180)));
    act.waiting = messages.first().map(|(author, _)| {
        if *author == t.opener_id { "Waiting on staff".to_owned() } else { format!("Waiting on {}", ctx.display_name(guild, t.opener_id)) }
    });
    act
}

async fn tickets_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<ListQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let now = now_ms();
    let view = q.status.as_deref().unwrap_or("open");
    let stats = ctx.db.with(|c| tickets::stats(c, guild, now))?;
    let open_label = format!("Open {}", stats.open);
    let rows = match view {
        "closed" => ctx.db.with(|c| tickets::list(c, guild, Filter::Closed, 100))?,
        "settings" => Vec::new(),
        _ => ctx.db.with(|c| tickets::list(c, guild, Filter::Open, 100))?,
    };
    let active = match view {
        "closed" => "Closed",
        "settings" => "Settings",
        _ => open_label.as_str(),
    };
    let tabs = ui::tabs(
        &[(open_label.as_str(), page.path("tickets")), ("Closed", format!("{}?status=closed", page.path("tickets"))), ("Settings", format!("{}?status=settings", page.path("tickets")))],
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
        } @else {
            div.with-rail {
                div.stack.tk-main {
                    (ui::readings(&[
                        ("open now", stats.open.to_string()),
                        ("opened this week", stats.this_week.to_string()),
                        ("usual time to close", stats.median_close_secs.map(span).unwrap_or_else(|| "none yet".into())),
                    ]))
                    @if rows.is_empty() {
                        @if view == "closed" {
                            (ui::empty(Icon::Ticket, "Nothing closed yet", "Closed tickets stay here with who closed them and how long they took."))
                        } @else {
                            (ui::empty(Icon::Ticket, "No open tickets", "Pick a ticket channel and staff role in Settings, send the panel, and tickets appear here as members open them."))
                        }
                    } @else {
                        div.tk-list {
                            @for t in &rows { (ticket_row(&ctx, &page, guild, t, now)) }
                        }
                        @if rows.len() == 100 { p.muted.small { "Showing the latest 100." } }
                    }
                }
                aside.rail {
                    (closed_recently(&ctx, guild, now))
                    (answering(&ctx, guild, now))
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Tickets", actions, body).into_response())
}

fn ticket_row(ctx: &Ctx, page: &Page, guild: u64, t: &Ticket, now: i64) -> Markup {
    let name = ctx.display_name(guild, t.opener_id);
    let act = if t.is_open() { activity(ctx, guild, t) } else { Activity::default() };
    let (label, tone) = match t.status.as_str() {
        tickets::OPEN => ("Open", Tone::Ok),
        tickets::IDLE => ("Auto-closed", Tone::Warn),
        _ => ("Closed", Tone::Muted),
    };
    let closer = t.closed_by.filter(|_| !t.is_open()).map(|u| ctx.display_name(guild, u));
    html! {
        div.tk-row {
            img.avatar src=(ctx.member_avatar(guild, t.opener_id)) alt="" width="36" height="36" loading="lazy";
            div.tk-body {
                div.tk-title {
                    span.tk-num { (format!("{:04}", t.number)) }
                    strong.tk-subject.clip { (t.subject) }
                }
                @if !t.details.is_empty() { p.tk-quote { "“" (text::clip(&t.details, 240)) "”" } }
                @if let Some((who, said)) = &act.staff_reply { p.tk-reply { strong { (who) } ": " span { "“" (said) "”" } } }
                div.tk-meta {
                    (name) " · opened " (text::ago(t.opened_at))
                    @if t.is_open() {
                        @if let Some(at) = act.last_reply_at { " · last reply " (text::ago(at)) }
                    } @else {
                        " · " (label.to_lowercase()) " " (text::ago(t.closed_at.unwrap_or(now)))
                        @if let Some(c) = &closer { " by " (c) }
                    }
                }
            }
            div.tk-side {
                @if t.is_open() {
                    @if let Some(w) = &act.waiting { (ui::badge(w, Tone::Warn)) }
                } @else {
                    (ui::badge(label, tone))
                }
                div.tk-acts {
                    @if t.thread_id != 0 { a.btn.small href=(thread_url(guild, t.thread_id)) target="_blank" rel="noopener" { "Open thread" } }
                    @if page.can_edit() && t.is_open() { (ui::action_button(&page.path(&format!("tickets/{}/close", t.number)), "Close", Tone::Bad, true)) }
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
                div.tk-mini {
                    img.avatar src=(ctx.member_avatar(guild, t.opener_id)) alt="" width="32" height="32" loading="lazy";
                    div.grow {
                        strong.clip { (t.subject) }
                        span.clip {
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
            h2 { "Who's answering" }
            @if who.is_empty() {
                p.muted.small { "Nobody has closed a ticket in the last 30 days." }
            } @else {
                div.tk-who {
                    @for (member, closed) in &who {
                        div.tk-who-one {
                            img.avatar src=(ctx.member_avatar(guild, *member)) alt="" width="40" height="40" loading="lazy";
                            span { (ctx.display_name(guild, *member)) " · " (closed) }
                        }
                    }
                }
                p.muted.small { "Closed the most tickets in the last 30 days." }
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

// ---- starboard ----

#[derive(Deserialize)]
struct StarQuery {
    view: Option<String>,
}

async fn starboard_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<StarQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let now = now_ms();
    let view = q.view.as_deref().unwrap_or("most");
    let stats = ctx.db.with(|c| starboard::stats(c, guild, now))?;
    let rows = match view {
        "newest" => ctx.db.with(|c| starboard::recent(c, guild, 25))?,
        "settings" => Vec::new(),
        _ => ctx.db.with(|c| starboard::top(c, guild, 25))?,
    };
    let authors = ctx.db.with(|c| starboard::top_authors(c, guild, 3))?;
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
        @if view == "settings" {
            (ui::section("Settings", None, ui::settings_form(&page, &page.path("tickets/starboard"), ui::card(html! {
                (ui::field_channel(&ctx, guild, "star_channel", "Starboard channel", Some("Where popular messages are reposted. None turns the starboard off."), s.star_channel))
                (ui::field_text("star_emoji", "Emoji", Some("Paste the emoji itself. For a custom one, paste it as <:name:id>."), &s.star_emoji, "⭐"))
                (ui::field_number("star_threshold", "Stars needed", Some("A message is reposted once it has this many."), s.star_threshold, 1, 100, Some("stars")))
                (ui::field_toggle("star_remove_below", "Remove when it drops below", Some("Takes the post down if stars fall under the number above. Off keeps it with the lower count."), s.star_remove_below))
                (ui::field_toggle("star_self", "Count the author's own star", Some("Off: a member starring their own message doesn't count."), s.star_self))
                (ui::field_channels(&ctx, guild, "star_ignored", "Ignored channels", Some("Stars in these channels never reach the starboard. The starboard channel is always ignored."), &s.star_ignored))
            }))))
        } @else {
            div.with-rail {
                div.stack.sb-main {
                    @if rows.is_empty() {
                        (ui::empty(Icon::Star, "Nothing starred yet", if s.star_channel.is_some() {
                            "A message shows up here once it gets enough stars."
                        } else {
                            "Pick a starboard channel in Settings. Messages show up here once they get enough stars."
                        }))
                    } @else {
                        div.sb-grid {
                            @for p in &rows {
                                article.sb-card {
                                    header.sb-head {
                                        img.avatar src=(ctx.member_avatar(guild, p.author_id)) alt="" width="28" height="28" loading="lazy";
                                        div.sb-who {
                                            strong.clip { (ctx.display_name(guild, p.author_id)) }
                                            span.clip { (channel_name(&ctx, guild, p.channel_id)) " · " (text::ago(p.created_at)) }
                                        }
                                        span.sb-stars { (glyph) " " (p.stars) }
                                    }
                                    @if p.preview.is_empty() {
                                        p.sb-text.muted { "No text" }
                                    } @else {
                                        p.sb-text { (p.preview) }
                                    }
                                    div.sb-links {
                                        a href={ (thread_url(guild, p.channel_id)) "/" (p.message_id) } target="_blank" rel="noopener" { "Original" }
                                        a href={ (thread_url(guild, p.post_channel_id)) "/" (p.post_id) } target="_blank" rel="noopener" { "Post" }
                                    }
                                }
                            }
                        }
                    }
                }
                aside.rail {
                    div.rail-block {
                        h2 { "On the starboard" }
                        div.sb-totals {
                            div { span.big-num { (text::num(stats.posts)) } span.muted.small { "messages" } }
                            div { span.big-num { (text::num(stats.this_week)) } span.muted.small { "this week" } }
                        }
                    }
                    div.rail-block {
                        h2 { "Most starred members" }
                        @if authors.is_empty() {
                            p.muted.small { "Members show up here once a message of theirs is reposted." }
                        }
                        @for (author, posts) in &authors {
                            div.mini {
                                img.avatar src=(ctx.member_avatar(guild, *author)) alt="" width="28" height="28" loading="lazy";
                                div.grow { span.clip { (ctx.display_name(guild, *author)) } }
                                span.mono { (posts) @if *posts == 1 { " post" } @else { " posts" } }
                            }
                        }
                    }
                    p.muted.small {
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

const CSS: &str = "
.tk-main, .sb-main { gap: 0; }
.tk-main .readings { padding-bottom: 22px; }
.tk-list { display: flex; flex-direction: column; }
.tk-row { display: grid; grid-template-columns: 36px minmax(0, 1fr) auto; gap: 14px; align-items: start; padding: 16px 0; border-top: 1px solid var(--edge-1); }
.tk-row:last-child { border-bottom: 1px solid var(--edge-1); }
.tk-body { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.tk-title { display: flex; align-items: baseline; gap: 10px; min-width: 0; }
.tk-num { font: 12px var(--mono); color: var(--ink-3); flex: none; }
.tk-subject { font-weight: 500; font-size: 14px; min-width: 0; }
.tk-quote { color: var(--ink-2); font-size: 13px; line-height: 1.5; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
.tk-reply { font-size: 13px; color: var(--ink-2); line-height: 1.45; }
.tk-reply strong { color: var(--ink); font-weight: 500; }
.tk-meta { font-size: 12px; color: var(--ink-3); }
.tk-side { display: flex; flex-direction: column; align-items: flex-end; gap: 10px; }
.tk-acts { display: flex; gap: 6px; align-items: center; }
.tk-mini { display: grid; grid-template-columns: 32px minmax(0, 1fr); gap: 10px; align-items: center; }
.tk-mini .grow { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.tk-mini strong { font-weight: 500; font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tk-mini span { font-size: 12px; color: var(--ink-3); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tk-who { display: flex; flex-wrap: wrap; gap: 14px; }
.tk-who-one { display: flex; flex-direction: column; align-items: center; gap: 6px; font-size: 12px; color: var(--ink-2); }
.tk-who-one .avatar { width: 40px; height: 40px; }
.sb-grid { columns: 2 300px; column-gap: 14px; }
.sb-card { break-inside: avoid; margin: 0 0 14px; padding: 14px 16px; display: flex; flex-direction: column; gap: 10px; background: var(--veil); border: 1px solid var(--edge-1); border-radius: var(--r-card); }
.sb-head { display: flex; align-items: center; gap: 10px; min-width: 0; }
.sb-who { display: flex; flex-direction: column; min-width: 0; flex: 1; }
.sb-who strong { font-weight: 500; font-size: 13px; }
.sb-who span { font-size: 12px; color: var(--ink-3); }
.sb-stars { flex: none; font-size: 13px; font-weight: 500; color: var(--waiting); }
.sb-text { font-size: 13.5px; line-height: 1.5; overflow-wrap: anywhere; }
.sb-links { display: flex; gap: 12px; font-size: 12px; color: var(--ink-3); }
.sb-links a:hover { color: var(--ink); }
.sb-totals { display: flex; gap: 32px; }
.sb-totals > div { display: flex; flex-direction: column; gap: 2px; }
.sb-totals .big-num { font-size: 26px; }
@media (max-width: 560px) {
  .tk-row { grid-template-columns: 32px minmax(0, 1fr); }
  .tk-row .avatar { width: 32px; height: 32px; }
  .tk-side { grid-column: 2; flex-direction: row; justify-content: space-between; align-items: center; flex-wrap: wrap; }
}
";
