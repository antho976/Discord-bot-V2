use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::http::{HeaderMap, header};
use bot_core::axum::response::{IntoResponse, Redirect};
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::rusqlite::params;
use bot_core::twilight_model::channel::Message;
use bot_core::twilight_model::channel::message::component::{ButtonStyle, Component};
use bot_core::twilight_model::channel::message::{AllowedMentions, MessageType};
use bot_core::twilight_model::id::Id;
use bot_core::web::ui::{self, Dot, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};
use bot_core::rand::RngExt as _;
use serde::Deserialize;

use crate::store::{self, Move, Review, Tally};
use crate::twitch::{self, Conn};
use crate::words;
use crate::{DEFAULT_DONE, DEFAULT_NUDGE, DEFAULT_PING, KEY, Settings};

const STATE_COOKIE: &str = "rv_twitch";
const DAY_MS: i64 = 86_400_000;

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/reviews", get(queue_page))
        .route("/g/{guild}/reviews/waitlist", get(waitlist_page))
        .route("/g/{guild}/reviews/done", get(done_page))
        .route("/g/{guild}/reviews/settings", get(settings_page).post(save_settings))
        .route("/g/{guild}/reviews/scan", post(scan))
        .route("/g/{guild}/reviews/sync", post(sync))
        .route("/g/{guild}/reviews/test", post(send_test))
        .route("/g/{guild}/reviews/{id}/ping", post(ping))
        .route("/g/{guild}/reviews/{id}/complete", post(complete))
        .route("/g/{guild}/reviews/{id}/remove", post(remove))
        .route("/g/{guild}/reviews/{id}/promote", post(promote))
        .route("/g/{guild}/reviews/{id}/reopen", post(reopen))
        .route("/g/{guild}/reviews/{id}/lock", post(lock))
        .route("/g/{guild}/reviews/{id}/notes", post(save_notes))
        .route("/g/{guild}/reviews/{id}/thread", get(thread_view))
        .route("/g/{guild}/reviews/twitch/connect", get(twitch_connect))
        .route("/g/{guild}/reviews/twitch/disconnect", post(twitch_disconnect))
        .route("/g/{guild}/reviews/twitch/reward", post(pick_reward))
        .route("/g/{guild}/reviews/twitch/reward/new", post(new_reward))
        .route("/reviews/twitch/callback", get(twitch_callback))
}

/// The sub-pages, with how many wait in the queue and on the waitlist. The Waitlist tab shows
/// while the waitlist is on or someone is still on it.
fn tabs(ctx: &Ctx, page: &Page, s: &Settings, active: &str) -> WebResult<Markup> {
    let (open, waiting) = store::counts(ctx, page.guild_id)?;
    let count = |n: usize| Some(n as i64).filter(|n| *n > 0);
    let mut items = vec![("Queue", page.path("reviews"), count(open))];
    if s.wait_days > 0 || waiting > 0 {
        items.push(("Waitlist", page.path("reviews/waitlist"), count(waiting)));
    }
    items.extend([("Done", page.path("reviews/done"), None), ("Settings", page.path("reviews/settings"), None)]);
    Ok(ui::tabs_counted(&items, active))
}

/// "today, 14:00", "tomorrow", "Friday", then "Nov 12": when a waitlisted request joins the
/// queue, in the viewer's timezone.
fn joins_words(page: &Page, at: i64, now: i64) -> String {
    let z = page.user.local(at);
    match (z.date() - page.user.local(now).date()).get_days() {
        ..=0 if at <= now => "in a minute".to_owned(),
        ..=0 => format!("today, {}", z.strftime("%H:%M")),
        1 => "tomorrow".to_owned(),
        2..=6 => z.strftime("%A").to_string(),
        _ => z.strftime("%b %-d").to_string(),
    }
}

/// "1 hour", "24 hours".
fn hours(n: i64) -> String {
    format!("{n} {}", if n == 1 { "hour" } else { "hours" })
}

/// "40 min", "5 h", "3 days".
fn wait_words(ms: i64) -> String {
    let mins = (ms / 60_000).max(0);
    match mins {
        0..60 => format!("{mins} min"),
        60..1440 => format!("{} h", mins / 60),
        1440..2880 => "1 day".into(),
        _ => format!("{} days", mins / 1440),
    }
}

/// "Oct 31, 14:00" in the viewer's timezone, for tooltips.
fn stamp(page: &Page, ms: i64) -> String {
    page.user.local(ms).strftime("%b %-d, %H:%M").to_string()
}

/// Channel points, as a dot in Twitch's purple and the words.
fn points_flag() -> Markup {
    html! { span.rv-flag.cp { span.dot {} "Channel points" } }
}

fn thread_url(guild: u64, thread: u64) -> String {
    format!("https://discord.com/channels/{guild}/{thread}")
}

/// "Open post": the page's script tries the Discord app (`discord://`) first and falls back to
/// discord.com in a new tab, which is also what a middle click or a copied link opens.
fn post_link(guild: u64, thread: u64) -> Markup {
    html! {
        a.rv-link href=(thread_url(guild, thread)) data-app={ "discord://-/channels/" (guild) "/" (thread) } target="_blank" rel="noreferrer noopener" {
            (ui::icon(Icon::External)) "Open post"
        }
    }
}

/// The member's IdleonToolbox profile as a link named after it, or that there isn't one yet
/// (and when the bot asked for it).
fn toolbox_meta(page: &Page, r: &Review) -> Option<Markup> {
    if let Some(url) = r.toolbox_link() {
        let name = words::toolbox_profile(&url).unwrap_or_else(|| "profile".into());
        return Some(html! { a.rv-link href=(url) target="_blank" rel="noreferrer noopener" title=(url) { (ui::icon(Icon::External)) "Toolbox: " (name) } });
    }
    // Only a member with a post can share one.
    (r.thread_id.is_some() && r.user_id.is_some()).then(|| match r.nudged_at {
        Some(at) => html! { span.rv-nolink data-tip={ "Asked in the post: " (stamp(page, at)) } { "No toolbox link" } },
        None => html! { span.rv-nolink { "No toolbox link" } },
    })
}

/// The tags read from the post, as small pills.
fn tag_pills(r: &Review) -> Markup {
    html! { @for t in &r.tags { span.rv-tag { (t) } } }
}

/// The face on a row: the member's avatar, or the name's first letter on Twitch's purple for a
/// Twitch viewer.
fn face(ctx: &Ctx, guild: u64, r: &Review, class: &str) -> Markup {
    html! {
        @match r.user_id {
            Some(u) => img class={ "avatar rv-face " (class) } src=(ctx.member_avatar(guild, u)) alt="" width="40" height="40" loading="lazy";,
            None => span class={ "rv-face rv-letter " (class) } { (r.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default()) },
        }
    }
}

/// Who asked, in words: "Mira", "pavel_tv on Twitch", or both. The member's name is left out
/// when the request already carries it.
fn who(ctx: &Ctx, guild: u64, r: &Review) -> Option<String> {
    let member = r.user_id.map(|u| ctx.display_name(guild, u)).filter(|m| !m.eq_ignore_ascii_case(r.name.trim()));
    let viewer = r.twitch_user.as_ref().map(|t| format!("{t} on Twitch"));
    match (member, viewer) {
        (Some(m), Some(v)) => Some(format!("{m}, {v}")),
        (m, v) => m.or(v),
    }
}

fn matches_search(ctx: &Ctx, guild: u64, r: &Review, q: &str) -> bool {
    let q = q.to_lowercase();
    r.name.to_lowercase().contains(&q)
        || r.notes.to_lowercase().contains(&q)
        || r.tags.iter().any(|t| t.to_lowercase().contains(&q))
        || r.toolbox.as_ref().is_some_and(|t| t.to_lowercase().contains(&q))
        || r.twitch_user.as_ref().is_some_and(|t| t.to_lowercase().contains(&q))
        || r.user_id.is_some_and(|u| ctx.display_name(guild, u).to_lowercase().contains(&q))
}

#[derive(Deserialize, Default)]
struct QueueQuery {
    q: Option<String>,
    added: Option<usize>,
    skipped: Option<usize>,
    moved: Option<usize>,
    waiting: Option<usize>,
    from: Option<String>,
}

async fn queue_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<QueueQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let open = store::open(&ctx, guild)?;
    let conn = twitch::conn(&ctx, guild)?;
    let now = now_ms();
    let search = q.q.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let shown: Vec<&Review> = open.iter().filter(|r| search.is_none_or(|t| matches_search(&ctx, guild, r, t))).collect();
    let redeemed = open.iter().filter(|r| r.redeemed_at.is_some()).count();
    let longest = open.iter().map(|r| r.eligible_at.unwrap_or(r.requested_at)).min().map(|t| wait_words(now - t)).unwrap_or_else(|| "None".into());
    let done_week = store::done_since(&ctx, guild, now - 7 * DAY_MS)?;
    let can_sync = conn.as_ref().is_some_and(|c| c.connected() && c.can_manage()) && s.reward_id.is_some();
    let actions = page.can_edit().then(|| {
        html! {
            @if can_sync {
                form.inline method="post" action=(page.path("reviews/sync")) { button.btn type="submit" { (ui::icon(Icon::Live)) "Sync Twitch" } }
            }
            @if s.channel.is_some() {
                form.inline method="post" action=(page.path("reviews/scan")) { button.btn.primary type="submit" title="Adds open posts, and archived ones from the last 30 days, that aren't in the queue" { (ui::icon(Icon::Search)) "Scan posts" } }
            }
        }
    });
    let recent = store::closed(&ctx, guild)?;
    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&ctx, &page, &s, "Queue")?)
        div.with-rail {
            div.rv-main {
                div.readings.rv-readings {
                    div.reading { b data-count { (open.len()) } span { "Waiting in the queue" } }
                    div.reading { b data-count { (redeemed) } span { "With channel points" } }
                    div.reading { b data-count { (longest) } span { "Longest wait" } }
                    div.reading { b data-count { (text::num(done_week)) } span { "Done this week" } }
                }
                @if let Some(note) = result_note(&q) { p.rv-note { (ui::icon(Icon::Check)) (note) } }
                @if !open.is_empty() {
                    form.rv-search method="get" action=(page.path("reviews")) role="search" {
                        (ui::icon(Icon::Search))
                        input.input type="search" name="q" value=(search.unwrap_or_default()) placeholder="Search names, members, tags and notes" aria-label="Search the queue";
                        button.btn type="submit" { "Search" }
                        @if search.is_some() { a.btn.ghost href=(page.path("reviews")) { "Clear" } }
                    }
                }
                @if open.is_empty() {
                    (ui::empty(Icon::Review, "No one is waiting", &if s.channel.is_some() { "New posts in the review channel, and channel points redemptions once Twitch is connected, show up here.".to_owned() } else { "Pick the forum channel people post requests in on the Settings tab, and new posts show up here.".to_owned() }))
                } @else if shown.is_empty() {
                    (ui::empty(Icon::Search, "Nothing matches", "Try part of the account name, the member, a tag like Late game, or a word from the notes."))
                } @else {
                    ol.rv-queue.numbered[search.is_none()] {
                        @for (i, r) in shown.iter().enumerate() {
                            (queue_row(&ctx, &page, &s, r, i + 1, search.is_none(), now))
                        }
                    }
                    script { (PreEscaped(PAGE_JS)) }
                }
            }
            aside.rail {
                (sources_block(&ctx, &page, &s, conn.as_ref()))
                (recent_block(&ctx, &page, &recent))
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Account reviews", actions, body).into_response())
}

/// What a scan or sync just did.
fn result_note(q: &QueueQuery) -> Option<String> {
    let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut note = match q.from.as_deref()? {
        "scan" => format!(
            "The scan added {}; {} already here.",
            plural(q.added.unwrap_or(0), "request", "requests"),
            plural(q.skipped.unwrap_or(0), "post was", "posts were")
        ),
        "sync" => format!(
            "Twitch sync added {}; {} requests already waiting.",
            plural(q.added.unwrap_or(0), "request", "requests"),
            plural(q.moved.unwrap_or(0), "redemption joined", "redemptions joined")
        ),
        _ => return None,
    };
    if let Some(n) = q.waiting.filter(|n| *n > 0) {
        note.push_str(&format!(" {} to the waitlist.", plural(n, "request went", "requests went")));
    }
    Some(note)
}

/// "Pinged 5 min ago", or once the member answered, "Ready for review 2 min ago", which waits
/// on staff. The tooltip says who pressed Ping.
fn ping_flag(ctx: &Ctx, page: &Page, r: &Review) -> Option<Markup> {
    let pinged = r.pinged_at?;
    let by = r.pinged_by.map(|u| format!(" by {}", ctx.display_name(page.guild_id, u))).unwrap_or_default();
    Some(match r.ready_at {
        Some(at) => html! { span.rv-flag.ready title={ "Pinged" (by) " " (stamp(page, pinged)) ", answered " (stamp(page, at)) } { span.dot {} "Ready for review " (text::ago(at)) } },
        None => html! { span.rv-flag.pinged title={ "Pinged" (by) " " (stamp(page, pinged)) } { span.dot {} "Pinged " (text::ago(pinged)) } },
    })
}

/// One request in the queue. The first, when the queue isn't filtered, is drawn larger: it's
/// the one you take next. A request with a post opens on a click to what the member wrote there.
fn queue_row(ctx: &Ctx, page: &Page, s: &Settings, r: &Review, place: usize, numbered: bool, now: i64) -> Markup {
    let guild = page.guild_id;
    let base = format!("reviews/{}", r.id);
    let next = numbered && place == 1;
    let since = r.eligible_at.unwrap_or(r.requested_at);
    let talk = r.thread_id.map(|_| page.path(&format!("{base}/thread")));
    html! {
        li.rv-row.next[next].ready[r.ready_at.is_some()] data-talk=[talk.as_deref()] {
            @if numbered { span.rv-place { (place) } }
            (face(ctx, guild, r, if next { "big" } else { "" }))
            div.rv-body {
                div.rv-title {
                    @if talk.is_some() {
                        button.rv-name type="button" aria-expanded="false" aria-controls={ "talk-" (r.id) } title="Show what they wrote in the post" { (r.name) }
                    } @else {
                        strong { (r.name) }
                    }
                    @if next { span.rv-next { "Up next" } }
                    @if r.redeemed_at.is_some() { (points_flag()) }
                    @if let Some(f) = ping_flag(ctx, page, r) { (f) }
                    (tag_pills(r))
                }
                @let who = who(ctx, guild, r);
                @let toolbox = toolbox_meta(page, r);
                @if who.is_some() || toolbox.is_some() || r.thread_id.is_some() {
                    p.rv-meta {
                        @if let Some(w) = who { span { (w) } }
                        @if let Some(t) = toolbox { (t) }
                        @if let Some(t) = r.thread_id { (post_link(guild, t)) }
                    }
                }
            }
            // Under the name on a narrow row, a column of its own on a wide one (CSS).
            @if !r.notes.trim().is_empty() { p.rv-notes { (r.notes.trim()) } }
            div.rv-side {
                span.rv-wait data-tip={ "Asked: " (stamp(page, r.requested_at)) } { b { (wait_words(now - since)) } " waiting" }
                @if page.can_edit() {
                    div.rv-acts {
                        @if r.thread_id.is_some() {
                            (ui::action_button(&page.path(&format!("{base}/ping")), "Ping", Tone::Muted, false))
                        }
                        (ui::pop("rv-pop", "Complete", None, if next { "small primary" } else { "small" }, html! {
                            form.stack method="post" action=(page.path(&format!("{base}/complete"))) {
                                @if r.thread_id.is_some() {
                                    label.small for={ "done-" (r.id) } { "Closing message in the post" }
                                    textarea.input id={ "done-" (r.id) } name="message" rows="3" { (s.done_message) }
                                    p.help { "Clear it to complete without posting." }
                                    p.help {
                                        "The post gets a check mark reaction"
                                        @if s.lock_hours > 0 { " and locks in " (hours(s.lock_hours)) ", or sooner if they press Lock post under the message" }
                                        "."
                                    }
                                } @else {
                                    p.help { "This request has no post, so nothing is posted." }
                                }
                                @if r.redemption_id.is_some() { p.help { "Marks the channel points redemption fulfilled on Twitch." } }
                                div { button.btn.small.primary type="submit" { "Complete" } }
                            }
                        }))
                        (ui::pop("rv-pop", "More", None, "small ghost", html! {
                            form.stack method="post" action=(page.path(&format!("{base}/notes"))) {
                                label.small for={ "notes-" (r.id) } { "Notes for " (r.name) }
                                textarea.input id={ "notes-" (r.id) } name="notes" rows="4" placeholder="Profile link, what to look at" { (r.notes) }
                                div { button.btn.small.primary type="submit" { "Save notes" } }
                            }
                            div.rv-pop-foot { (remove_button(page, r, store::refundable(ctx, page.guild_id, r.id))) }
                        }))
                    }
                }
            }
            @if talk.is_some() {
                div.rv-talk id={ "talk-" (r.id) } hidden { p.rv-talk-note { "Loading the post" } }
            }
        }
    }
}

/// Remove, asking for a second click. With channel points, the second click says the points go
/// back to the viewer.
fn remove_button(page: &Page, r: &Review, refundable: bool) -> Markup {
    let confirm = if refundable { "Click again to remove and refund the points" } else { "Click again to confirm" };
    html! {
        form.inline method="post" action=(page.path(&format!("reviews/{}/remove", r.id))) {
            button.btn.small.danger type="submit" data-confirm=(confirm) { "Remove" }
        }
    }
}

/// Where requests come from, and whether each source is working.
fn sources_block(ctx: &Ctx, page: &Page, s: &Settings, conn: Option<&Conn>) -> Markup {
    let guild = page.guild_id;
    let channel = s.channel.map(|c| ctx.text_channels(guild).into_iter().find(|(id, _)| *id == c).map(|(_, n)| format!("#{n}")).unwrap_or_else(|| "a deleted channel".into()));
    let twitch_on = conn.is_some_and(|c| c.connected() && c.can_manage()) && s.reward_id.is_some();
    html! {
        div.rail-block {
            h2 { "Requests come from" }
            div.mini {
                span.rv-src.on[channel.is_some()] { (ui::icon(Icon::Hash)) }
                div.grow {
                    @match &channel {
                        Some(c) => { span { (c) } span.muted { "Each new post joins the queue" } }
                        None => { span { "No channel yet" } span.muted { a href=(page.path("reviews/settings")) { "Pick one in Settings" } } }
                    }
                }
            }
            div.mini {
                span.rv-src.twitch.on[twitch_on] { (ui::icon(Icon::Live)) }
                div.grow {
                    @match conn {
                        Some(c) if c.connected() && c.can_manage() && s.reward_id.is_some() => {
                            span { "Twitch, " (s.reward_title) }
                            span.muted { @if let Some(t) = c.synced_at { "Synced " (text::ago(t)) ", every 5 min" } @else { "Syncs every 5 min" } }
                        }
                        Some(c) if c.connected() && c.can_manage() => { span { "Twitch, no reward picked" } span.muted { a href=(page.path("reviews/settings")) { "Pick one in Settings" } } }
                        Some(c) if c.connected() => { span { "Twitch needs permission" } span.muted { a href=(page.path("reviews/settings")) { "Connect again" } } }
                        _ => { span { "Twitch not connected" } span.muted { a href=(page.path("reviews/settings")) { "Connect in Settings" } } }
                    }
                }
            }
            @if let Some(e) = conn.and_then(|c| c.error.as_deref()) { p.rv-warn { (e) } }
        }
    }
}

/// The last few reviews done, with who did them.
fn recent_block(ctx: &Ctx, page: &Page, closed: &[Review]) -> Markup {
    let done: Vec<&Review> = closed.iter().filter(|r| r.status == "done").take(4).collect();
    html! {
        div.rail-block {
            h2 {
                "Recently done"
                @if !done.is_empty() { a.small.muted href=(page.path("reviews/done")) { "All done" } }
            }
            @if done.is_empty() {
                p.muted.small { "Reviews you complete show up here." }
            }
            @for r in &done {
                div.mini {
                    (face(ctx, page.guild_id, r, "small"))
                    div.grow {
                        span { (r.name) }
                        @if let Some(by) = &r.closed_by { span.muted { "by " (by) } }
                    }
                    @if let Some(at) = r.closed_at {
                        span.rv-ago data-tip={ "Waited " (wait_words(at - r.requested_at)) ": " (stamp(page, at)) } { (text::ago(at)) }
                    }
                }
            }
        }
    }
}

async fn waitlist_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let waiting = store::waiting(&ctx, guild)?;
    let now = now_ms();
    let days = |n: i64| format!("{n} {}", if n == 1 { "day" } else { "days" });
    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&ctx, &page, &s, "Waitlist")?)
        div.rv-main {
            @if s.wait_days > 0 {
                p.rv-intro { "Someone who asks again within " strong { (days(s.wait_days)) } " of their last review waits here, and joins the back of the queue on their own when the time is up." }
            } @else {
                p.rv-intro { "The waitlist is off, so new requests go straight to the queue. The ones still here join it at their time." }
            }
            @if waiting.is_empty() {
                (ui::empty(Icon::Review, "No one is on the waitlist", &format!("Someone who asks again within {} of their last review shows up here.", days(s.wait_days.max(1)))))
            } @else {
                div.rv-wl {
                    @for r in &waiting { (waitlist_card(&ctx, &page, r, now)) }
                }
                script { (PreEscaped(PAGE_JS)) }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Account reviews", None, body).into_response())
}

/// Someone on the waitlist, with how far through their wait they are.
fn waitlist_card(ctx: &Ctx, page: &Page, r: &Review, now: i64) -> Markup {
    let guild = page.guild_id;
    let last = ctx.db.with(|c| store::last_done(c, guild, r.user_id, r.twitch_user.as_deref())).ok().flatten();
    // How much of the wait has passed, from their last review to when they join.
    let through = match (last, r.eligible_at) {
        (Some(l), Some(e)) if e > l => Some(((now - l) * 100 / (e - l)).clamp(3, 100)),
        _ => None,
    };
    html! {
        article.rv-card {
            div.rv-card-head {
                (face(ctx, guild, r, ""))
                div.rv-body {
                    div.rv-title {
                        strong { (r.name) }
                        @if r.redeemed_at.is_some() { (points_flag()) }
                        (tag_pills(r))
                    }
                    p.rv-meta {
                        @if let Some(w) = who(ctx, guild, r) { span { (w) } }
                        span title=(stamp(page, r.requested_at)) { "asked " (text::ago(r.requested_at)) }
                        @if let Some(t) = toolbox_meta(page, r) { (t) }
                        @if let Some(t) = r.thread_id { (post_link(guild, t)) }
                    }
                }
            }
            @if let Some(at) = r.eligible_at {
                div.rv-countdown {
                    div.rv-cd-text {
                        span { "Joins the queue " strong title=(stamp(page, at)) { (joins_words(page, at, now)) } }
                        @if let Some(l) = last { span.rv-cd-last title=(stamp(page, l)) { "Last review " (text::ago(l)) } }
                    }
                    @if let Some(p) = through { span.meter { span style={ "width:" (p) "%" } {} } }
                }
            }
            @if !r.notes.trim().is_empty() { p.rv-notes { (r.notes.trim()) } }
            @if page.can_edit() {
                div.rv-acts {
                    (ui::action_button(&page.path(&format!("reviews/{}/promote", r.id)), "Move to queue now", Tone::Muted, false))
                    (ui::pop("rv-pop", "More", None, "small ghost", html! {
                        @let refundable = store::refundable(ctx, page.guild_id, r.id);
                        p.help { "Removing takes them off the waitlist" @if refundable { " and refunds their channel points" } "." }
                        div { (remove_button(page, r, refundable)) }
                    }))
                }
            }
        }
    }
}

async fn done_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let closed = store::closed(&ctx, guild)?;
    let done_week = store::done_since(&ctx, guild, now_ms() - 7 * DAY_MS)?;
    // The middle wait of the reviews done in this list.
    let mut waits: Vec<i64> = closed.iter().filter(|r| r.status == "done").filter_map(|r| r.closed_at.map(|at| at - r.requested_at)).collect();
    waits.sort_unstable();
    let typical = waits.get(waits.len() / 2).map(|w| wait_words(*w)).unwrap_or_else(|| "None".into());
    let removed_n = closed.iter().filter(|r| r.status == "removed").count();
    // Posts finished before this version are left for staff to lock.
    let timer_from = store::since_v4(&ctx);
    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&ctx, &page, &s, "Done")?)
        div.rv-main {
            div.readings.rv-readings.three {
                div.reading { b data-count { (text::num(done_week)) } span { "Done this week" } }
                div.reading { b data-count { (typical) } span { "Typical wait before a review" } }
                div.reading { b data-count { (removed_n) } span { "Removed, of the last 50" } }
            }
            p.rv-intro {
                "The last 50 completed or removed. Removed requests stay here so their post or redemption isn't added again. A removed one with channel points was refunded, so it can't go back to the queue."
                @if s.lock_hours > 0 { " A finished post gets a check mark reaction and locks " (hours(s.lock_hours)) " after the review, or sooner when the member presses Lock post." }
            }
            @if closed.is_empty() {
                (ui::empty(Icon::Review, "Nothing finished yet", "Requests you complete or remove from the queue are listed here."))
            } @else {
                div.rv-done {
                    @for r in &closed {
                        @let removed = r.status == "removed";
                        div.rv-drow.removed[removed] {
                            span.rv-dface data-tip=(if removed { "Removed" } else { "Done" }) {
                                (face(&ctx, guild, r, ""))
                                span.rv-dmark { (ui::icon(if removed { Icon::Close } else { Icon::Check })) }
                            }
                            div.rv-body {
                                div.rv-title {
                                    strong { (r.name) }
                                    @if r.redeemed_at.is_some() { (points_flag()) }
                                }
                                p.rv-meta {
                                    @if let Some(w) = who(&ctx, guild, r) { span { (w) } }
                                    @if let Some(at) = r.closed_at {
                                        span data-tip={ "Waited " (wait_words(at - r.requested_at)) ": " (stamp(&page, at)) } {
                                            (if removed { "Removed" } else { "Done" })
                                            @if let Some(by) = &r.closed_by { " by " (by) }
                                            " " (text::ago(at))
                                        }
                                    }
                                    @if !removed && r.thread_id.is_some() {
                                        @match r.locked_at {
                                            Some(l) => span title=(stamp(&page, l)) { "Post locked" },
                                            None if s.lock_hours > 0 && r.closed_at.is_some_and(|c| c >= timer_from) => span title={ "Locks " (stamp(&page, r.closed_at.unwrap_or_default() + s.lock_ms())) } { "Post open" },
                                            None => span { "Post open" },
                                        }
                                    }
                                }
                                @if !r.notes.trim().is_empty() { p.rv-notes { (r.notes.trim()) } }
                            }
                            // A request whose points were refunded doesn't go back.
                            @if page.can_edit() && (r.status == "done" || !store::refunded(&ctx, guild, r.id)) {
                                div.rv-acts {
                                    @if r.status == "done" && r.thread_id.is_some() && r.locked_at.is_none() {
                                        (ui::action_button(&page.path(&format!("reviews/{}/lock", r.id)), "Lock post", Tone::Muted, false))
                                    }
                                    (ui::action_button(&page.path(&format!("reviews/{}/reopen", r.id)), "Back to queue", Tone::Muted, false))
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Account reviews", None, body).into_response())
}

// ---- actions ----

async fn find(ctx: &Ctx, guild: u64, id: i64) -> WebResult<Review> {
    store::get(ctx, guild, id)?.ok_or(WebError::NotFound)
}

/// Post in a request's thread, pinging only `user` (its member, or the staff member told
/// they're ready). Nothing is sent in the preview.
pub(crate) async fn post_in(ctx: &Ctx, thread: u64, content: &str, user: Option<u64>) -> Result<(), String> {
    post_with(ctx, thread, content, user, &[]).await
}

/// [`post_in`] with buttons under the message.
async fn post_with(ctx: &Ctx, thread: u64, content: &str, user: Option<u64>, components: &[Component]) -> Result<(), String> {
    if ctx.cfg.preview {
        return Ok(());
    }
    let thread = Id::new_checked(thread).ok_or_else(|| "The post was deleted.".to_owned())?;
    let mentions = AllowedMentions { users: user.and_then(Id::new_checked).into_iter().collect(), ..Default::default() };
    ctx.http
        .create_message(thread)
        .content(&text::clip(content, 2000))
        .allowed_mentions(Some(&mentions))
        .components(components)
        .await
        .map(|_| ())
        .map_err(|e| if bot_core::discord::is_not_found(&e) { "The post was deleted.".to_owned() } else { bot_core::discord::why_refused(&e) })
}

/// The name is a post title or a Twitch name, so it is escaped: a post titled
/// `[x](https://evil)` must not become a masked link under the bot's name.
pub(crate) fn fill(template: &str, r: &Review) -> String {
    let name = text::escape_markdown(&r.name);
    let mention = r.user_id.map(|u| format!("<@{u}>")).unwrap_or_else(|| format!("**{name}**"));
    text::fill(template, &[("mention", &mention), ("name", &name)])
}

async fn ping(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let r = find(&ctx, guild, id).await?;
    let thread = r.thread_id.ok_or_else(|| WebError::Bad("This request has no post to ping in.".into()))?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let template = if s.ping_message.trim().is_empty() { DEFAULT_PING } else { s.ping_message.as_str() };
    post_in(&ctx, thread, &fill(template, &r), r.user_id).await.map_err(WebError::Bad)?;
    // Who pressed it is told in the post when the member answers ready.
    store::mark_pinged(&ctx, guild, id, page.user.id)?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Pinged {} for their account review", r.name));
    Ok(saved(&page.path("reviews")))
}

async fn complete(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let r = find(&ctx, guild, id).await?;
    // Claim it first, so a second click doesn't post the closing message twice.
    if !store::claim(&ctx, guild, id, Move::Complete, &page.user.name)? {
        return Ok(Redirect::to(&page.path("reviews")).into_response());
    }
    let message = form.text("message");
    let s = ctx.settings::<Settings>(guild, KEY)?;
    // The member can lock the post when they have no more questions; the timer does otherwise.
    let lock = [bot_core::discord::button_row(&[(&format!("{KEY}:lock:{id}"), "Lock post", ButtonStyle::Secondary)])];
    if let Some(thread) = r.thread_id
        && !message.trim().is_empty()
        && let Err(e) = post_with(&ctx, thread, &fill(message.trim(), &r), r.user_id, if s.lock_hours > 0 { &lock } else { &[] }).await
    {
        store::claim(&ctx, guild, id, Move::Reopen, &page.user.name)?;
        return Err(WebError::Bad(format!("The closing message didn't post: {e} Clear the message to complete without it.")));
    }
    if let Some(thread) = r.thread_id {
        crate::mark_done(&ctx, guild, s.channel, thread, true).await;
    }
    ctx.audit(&page.user, Some(guild), KEY, format!("Completed {}'s account review", r.name));
    let redemptions = store::set_twitch(&ctx, guild, id, "FULFILLED")?;
    twitch::settle(&ctx, guild, &redemptions).await;
    Ok(saved(&page.path("reviews")))
}

async fn remove(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let r = find(&ctx, guild, id).await?;
    let back = page.path(if r.status == "waiting" { "reviews/waitlist" } else { "reviews" });
    if !store::claim(&ctx, guild, id, Move::Remove, &page.user.name)? {
        return Ok(Redirect::to(&back).into_response());
    }
    ctx.audit(&page.user, Some(guild), KEY, format!("Removed {} from the review queue", r.name));
    // Cancelling gives the viewer their points back and takes it off Twitch's queue.
    let redemptions = store::set_twitch(&ctx, guild, id, "CANCELED")?;
    twitch::settle(&ctx, guild, &redemptions).await;
    Ok(saved(&back))
}

/// Staff move someone off the waitlist before their time.
async fn promote(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let r = find(&ctx, guild, id).await?;
    if !store::claim(&ctx, guild, id, Move::Promote, &page.user.name)? {
        return Ok(Redirect::to(&page.path("reviews/waitlist")).into_response());
    }
    ctx.audit(&page.user, Some(guild), KEY, format!("Moved {} from the waitlist to the review queue", r.name));
    Ok(saved(&page.path("reviews/waitlist")))
}

async fn reopen(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    find(&ctx, guild, id).await?;
    if !store::claim(&ctx, guild, id, Move::Reopen, &page.user.name)? {
        return Ok(Redirect::to(&page.path("reviews/done")).into_response());
    }
    // Back in the queue, the post is open again and loses its done mark. Read again after the
    // claim: the timer may have locked it in between.
    let r = find(&ctx, guild, id).await?;
    crate::unlock_post(&ctx, guild, &r, false).await;
    if let Some(thread) = r.thread_id {
        crate::mark_done(&ctx, guild, ctx.settings::<Settings>(guild, KEY)?.channel, thread, false).await;
    }
    ctx.audit(&page.user, Some(guild), KEY, format!("Put {} back in the review queue", r.name));
    Ok(saved(&page.path("reviews/done")))
}

/// Lock a finished request's post from the Done tab.
async fn lock(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let r = find(&ctx, guild, id).await?;
    if r.status != "done" || r.thread_id.is_none() || r.locked_at.is_some() {
        return Ok(Redirect::to(&page.path("reviews/done")).into_response());
    }
    crate::lock_post(&ctx, guild, &r).await.map_err(|e| WebError::Bad(format!("The post didn't lock: {e} The bot needs Manage Threads in the review channel.")))?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Locked {}'s review post", r.name));
    Ok(saved(&page.path("reviews/done")))
}

async fn save_notes(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let r = find(&ctx, guild, id).await?;
    store::set_notes(&ctx, guild, id, &text::clip(form.text("notes").trim(), 1000))?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Changed the notes on {}'s review", r.name));
    Ok(saved(&page.path("reviews")))
}

// ---- what the member wrote ----

/// One message in a post, as the queue's panel shows it.
#[derive(Debug, Clone, Default)]
struct Said {
    author: u64,
    /// A bot's message (the ping, the reminder) or Discord's own (a pin, a rename).
    bot: bool,
    at: i64,
    content: String,
    /// (full size, thumbnail) of each picture.
    images: Vec<(String, String)>,
    /// (link, file name) of anything else attached.
    files: Vec<(String, String)>,
}

impl Said {
    fn from_message(m: &Message) -> Said {
        let (images, files) = m.attachments.iter().partition::<Vec<_>, _>(|a| a.width.is_some() || a.content_type.as_deref().is_some_and(|t| t.starts_with("image/")));
        Said {
            author: m.author.id.get(),
            bot: m.author.bot || !matches!(m.kind, MessageType::Regular | MessageType::Reply),
            at: m.timestamp.as_micros() / 1000,
            content: m.content.clone(),
            images: images.into_iter().map(|a| (a.url.clone(), a.proxy_url.clone())).collect(),
            files: files.into_iter().map(|a| (a.url.clone(), a.filename.clone())).collect(),
        }
    }
}

/// What helps a review: the member's own messages with something in them. Bots (the ping, the
/// reminder), other people and chatter ("thanks", "ready", "bump") are left out.
fn useful(said: &Said, member: Option<u64>) -> bool {
    !said.bot && member.is_none_or(|m| said.author == m) && (!said.images.is_empty() || !said.files.is_empty() || !words::is_chatter(&said.content))
}

/// How many messages of a post the panel reads: one request to Discord.
const TALK_LIMIT: u16 = 100;

/// How long a post's messages are reused, so opening and closing a row doesn't ask Discord
/// each time.
const TALK_CACHE: std::time::Duration = std::time::Duration::from_secs(30);

type TalkCache = std::sync::Mutex<std::collections::HashMap<u64, (std::time::Instant, Vec<Said>)>>;
static TALKS: std::sync::LazyLock<TalkCache> = std::sync::LazyLock::new(Default::default);

/// A post's latest messages, oldest first, from the last 30 seconds' read if there was one.
async fn fetch_talk(ctx: &Ctx, thread: u64) -> Result<Vec<Said>, String> {
    if let Some((_, list)) = TALKS.lock().unwrap().get(&thread).filter(|(at, _)| at.elapsed() < TALK_CACHE) {
        return Ok(list.clone());
    }
    let list = read_talk(ctx, thread).await?;
    let mut cache = TALKS.lock().unwrap();
    cache.retain(|_, (at, _)| at.elapsed() < TALK_CACHE);
    cache.insert(thread, (std::time::Instant::now(), list.clone()));
    Ok(list)
}

async fn read_talk(ctx: &Ctx, thread: u64) -> Result<Vec<Said>, String> {
    let id = Id::new_checked(thread).ok_or_else(|| "The post was deleted.".to_owned())?;
    let res = ctx.http.channel_messages(id).limit(TALK_LIMIT).await.map_err(|e| if bot_core::discord::is_not_found(&e) { "The post was deleted.".to_owned() } else { bot_core::discord::why_refused(&e) })?;
    let mut list: Vec<Said> = res.models().await.map_err(|e| e.to_string())?.iter().map(Said::from_message).collect();
    list.reverse();
    Ok(list)
}

/// Text with its web links clickable. Only http and https become links.
fn linked(text: &str) -> Markup {
    html! {
        @for piece in text.split_inclusive(char::is_whitespace) {
            @let word = piece.trim_end();
            @let url = word.trim_start_matches('<').trim_end_matches(['>', ')', '.', ',']);
            @if url.starts_with("https://") || url.starts_with("http://") {
                a href=(url) target="_blank" rel="noreferrer noopener" { (word) } (&piece[word.len()..])
            } @else {
                (piece)
            }
        }
    }
}

/// The panel under a queue row: what the member wrote in their post, without the clutter. For
/// someone who can edit, it also picks up tags the bot missed while it was away, and a toolbox
/// link when the request has none (a newer one may have come from a merged second post, which
/// this panel doesn't read).
async fn thread_view(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let r = find(&ctx, guild, id).await?;
    let thread = r.thread_id.ok_or(WebError::NotFound)?;
    let talk = if ctx.cfg.preview { Ok(sample_talk(&r)) } else { fetch_talk(&ctx, thread).await };
    let member = r.user_id.map(|u| ctx.display_name(guild, u)).unwrap_or_else(|| "The member".into());
    let body = match talk {
        Err(e) => html! { p.rv-talk-note { "Couldn't read the post. " (e) } },
        Ok(all) => {
            let shown: Vec<&Said> = all.iter().filter(|m| useful(m, r.user_id)).collect();
            if !ctx.cfg.preview && page.can_edit() {
                let mine = all.iter().filter(|m| !m.bot && r.user_id == Some(m.author));
                let tags: Vec<String> = mine.clone().flat_map(|m| words::tags_in(&m.content)).collect();
                store::add_tags(&ctx, guild, id, &tags)?;
                if r.toolbox.is_none()
                    && let Some(link) = mine.filter_map(|m| words::toolbox_link(&m.content)).next_back()
                {
                    store::set_toolbox(&ctx, guild, id, &link)?;
                }
            }
            let hidden = all.len() - shown.len();
            html! {
                div.rv-talk-head {
                    span { "What " (member) " wrote" @if all.len() >= TALK_LIMIT as usize { ", from the last " (TALK_LIMIT) " messages" } }
                    (post_link(guild, thread))
                }
                @if shown.is_empty() {
                    p.rv-talk-note { "Nothing beyond short replies yet." }
                }
                @for m in &shown {
                    div.rv-said {
                        span.rv-said-at title=(stamp(&page, m.at)) { (text::ago(m.at)) }
                        div.rv-said-body {
                            @if !m.content.trim().is_empty() { p.rv-said-text { (linked(text::clip(m.content.trim(), 2000).as_str())) } }
                            @if !m.images.is_empty() {
                                div.rv-said-pics {
                                    @for (full, thumb) in &m.images {
                                        a href=(full) target="_blank" rel="noreferrer noopener" { img src=(thumb) alt="Picture from the post" loading="lazy"; }
                                    }
                                }
                            }
                            @for (url, name) in &m.files {
                                a.rv-link href=(url) target="_blank" rel="noreferrer noopener" { (ui::icon(Icon::External)) (name) }
                            }
                        }
                    }
                }
                @if hidden > 0 {
                    p.rv-talk-note { (hidden) " hidden: bot posts, short replies like thanks or ready, and other people's messages." }
                }
            }
        }
    };
    Ok(([(header::CACHE_CONTROL, "no-store")], body).into_response())
}

async fn scan(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let channel = s.channel.ok_or_else(|| WebError::Bad("Pick the review channel on the Settings tab first.".into()))?;
    let t = if ctx.cfg.preview {
        Tally::default()
    } else {
        crate::scan(&ctx, guild, channel, s.wait_ms()).await.map_err(|e| WebError::Bad(format!("The scan didn't work: {}", bot_core::discord::why_refused_any(&e))))?
    };
    ctx.audit(&page.user, Some(guild), KEY, format!("Scanned the review channel: {} added", t.added));
    Ok(Redirect::to(&format!("{}?from=scan&added={}&skipped={}&waiting={}", page.path("reviews"), t.added, t.merged + t.known, t.waiting)).into_response())
}

async fn sync(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let reward = s.reward_id.clone().ok_or_else(|| WebError::Bad("Pick the channel points reward on the Settings tab first.".into()))?;
    let t = if ctx.cfg.preview {
        Tally::default()
    } else {
        twitch::sync(&ctx, guild, &reward, s.wait_ms()).await.map_err(|e| {
            twitch::note_error(&ctx, guild, Some(&e.to_string()));
            WebError::Bad(format!("The Twitch sync didn't work. {e}"))
        })?
    };
    ctx.audit(&page.user, Some(guild), KEY, format!("Synced channel points: {} added", t.added));
    Ok(Redirect::to(&format!("{}?from=sync&added={}&moved={}&waiting={}", page.path("reviews"), t.added, t.merged, t.waiting)).into_response())
}

// ---- settings and Twitch ----

#[derive(Deserialize, Default)]
struct SettingsQuery {
    twitch: Option<String>,
    /// After "Send a test".
    sent: Option<String>,
}

async fn settings_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<SettingsQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let conn = twitch::conn(&ctx, guild)?;
    let ready = conn.as_ref().is_some_and(|c| c.connected() && c.can_manage());
    let rewards = if ready && page.can_edit() { Some(twitch::rewards(&ctx, guild).await.map_err(|e| e.to_string())) } else { None };
    let body = html! {
        style { (PreEscaped(CSS)) }
        (ui::sent_toast(q.sent.is_some(), "Test sent to your DMs"))
        (tabs(&ctx, &page, &s, "Settings")?)
        div.with-rail {
        div.rv-main.rv-settings {
        (ui::settings_form(&page, &page.path("reviews/settings"), html! {
            (ui::section("Requests", None, ui::card(html! {
                (ui::field_channel(&ctx, guild, "channel", "Review channel", Some("Each new post in this forum joins the queue, named after its title. A text channel works too: each new thread joins. Scan posts on the Queue tab picks up open posts and archived ones from the last 30 days."), s.channel))
                (ui::field_number("wait_days", "Wait before another review", Some("Someone who asks again this soon after their last review waits on the waitlist, then joins the queue on their own. 0 turns the waitlist off."), s.wait_days, 0, 365, Some("days")))
                (ui::field_number("lock_hours", "Lock finished posts after", Some("Hours after you complete a review, so the post can't fill up with chatter. The member can lock it sooner with the button under the closing message. Only their own messages ever count in a post. 0 leaves posts open."), s.lock_hours, 0, 720, Some("hours")))
            })))
            div.rv-msgs {
                (ui::section("Messages", Some("{mention} pings the member, or names them when there's no member. {name} is the request's name."), ui::card(html! {
                    (ui::field_textarea("nudge_message", "Toolbox link reminder", Some("Posted once in a new post that has no IdleonToolbox profile link after 3 minutes. Clear it to not ask."), &s.nudge_message, DEFAULT_NUDGE))
                    (ui::field_textarea("ping_message", "Ping", Some("Posted in their post when you press Ping. When they answer ready, the bot pings whoever pressed it."), &s.ping_message, DEFAULT_PING))
                    (ui::field_textarea("done_message", "Closing message", Some("Filled in when you complete a request. You can change or clear it each time."), &s.done_message, DEFAULT_DONE))
                    @if page.can_edit() {
                        // A form can't hold another form, so the test posts this one to its own
                        // route, which reads the saved messages.
                        div.row.rv-test {
                            div.row-text {
                                label { "Try them" }
                                p.help { "Sends each to your DMs, filled in with your name, as saved." }
                            }
                            div.row-control { button.btn.small type="submit" formaction=(page.path("reviews/test")) formmethod="post" formnovalidate { "Send a test" } }
                        }
                    }
                })))
            }
        }))
        (ui::section("Twitch channel points", Some("Redemptions of a channel points reward join the queue ahead of everyone else. Completing one marks it fulfilled on Twitch; removing one cancels it and refunds the points."), twitch_block(&ctx, &page, &s, conn.as_ref(), rewards, q.twitch.as_deref())))
        }
            aside.rail { (flow(&ctx, &page, &s, conn.as_ref())) }
        }
    };
    Ok(ui::layout(&ctx, &page, "Account reviews", None, body).into_response())
}

/// How a request moves from asking to done, as the saved settings have it.
fn flow(ctx: &Ctx, page: &Page, s: &Settings, conn: Option<&Conn>) -> Markup {
    let channel = s.channel.map(|c| ctx.text_channels(page.guild_id).into_iter().find(|(id, _)| *id == c).map(|(_, n)| format!("#{n}")).unwrap_or_else(|| "a deleted channel".into()));
    let twitch = conn.is_some_and(|c| c.connected() && c.can_manage()) && s.reward_id.is_some();
    let days = |n: i64| format!("{n} {}", if n == 1 { "day" } else { "days" });
    html! {
        div.rail-block {
            h2 { "How a request moves" }
            ol.rv-flow {
                li {
                    span.rv-flow-dot {}
                    div {
                        strong { "Asks" }
                        span {
                            @match &channel { Some(c) => { "A new post in " (c) }, None => { "No review channel yet" } }
                            @if twitch { ", or redeeming " (s.reward_title) " on Twitch" }
                        }
                    }
                }
                li.muted[s.wait_days == 0] {
                    span.rv-flow-dot {}
                    div {
                        strong { "Waits, if they asked lately" }
                        span { @if s.wait_days > 0 { "On the waitlist until " (days(s.wait_days)) " after their last review" } @else { "Waitlist off: everyone goes straight to the queue" } }
                    }
                }
                li {
                    span.rv-flow-dot {}
                    div { strong { "Queues" } span { "Channel points first, then whoever has waited longest" } }
                }
                li.muted[s.nudge_message.trim().is_empty()] {
                    span.rv-flow-dot {}
                    div {
                        strong { "Asked for a toolbox link" }
                        span { @if s.nudge_message.trim().is_empty() { "Reminder off: nobody is asked" } @else { "Once, if their post has no IdleonToolbox link after 3 minutes" } }
                    }
                }
                li {
                    span.rv-flow-dot {}
                    div { strong { "Pinged" } span { "Your ping message in their post, when you press Ping" } }
                }
                li {
                    span.rv-flow-dot {}
                    div { strong { "Ready" } span { "They answer ready: the bot pings whoever pressed Ping, and the bell tells you" } }
                }
                li {
                    span.rv-flow-dot {}
                    div {
                        strong { "Done" }
                        span {
                            "The closing message is posted, the post gets a check mark" @if twitch { ", the redemption is fulfilled" }
                            @if s.lock_hours > 0 { ", and the post locks after " (hours(s.lock_hours)) }
                        }
                    }
                }
            }
        }
    }
}

fn twitch_block(ctx: &Ctx, page: &Page, s: &Settings, conn: Option<&Conn>, rewards: Option<Result<Vec<twitch::Reward>, String>>, notice: Option<&str>) -> Markup {
    let redirect = twitch::redirect_uri(ctx);
    let connect = html! {
        @if page.can_edit() { a.btn.primary href=(page.path("reviews/twitch/connect")) { "Connect Twitch" } }
    };
    html! {
        div.card.rv-twitch {
            @if notice == Some("denied") { p.rv-warn { "Twitch didn't connect: the sign-in was cancelled." } }
            @if let Some(e) = conn.and_then(|c| c.error.as_deref()) { p.rv-warn { (e) } }
            @if !twitch::configured(ctx) && conn.is_none() {
                (ui::empty(Icon::Live, "Twitch isn't set up on this bot", &format!("Set TWITCH_CLIENT_ID and TWITCH_CLIENT_SECRET on the server, the same ones the stream alerts use, and add {redirect} to the Twitch app's OAuth redirect URLs.")))
            } @else {
                @match conn {
                    Some(c) if c.connected() && c.can_manage() => {
                        div.rv-tw-head {
                            (ui::status(Dot::Live, &format!("Connected as {}", c.login)))
                            @if page.can_edit() { (ui::action_button(&page.path("reviews/twitch/disconnect"), "Disconnect", Tone::Bad, true)) }
                        }
                        @if let Some(t) = c.synced_at { p.muted.small { "Last synced " (text::ago(t)) ". The bot checks every 5 minutes." } }
                        (reward_picker(page, s, rewards))
                    }
                    Some(c) if c.connected() => {
                        div.rv-tw-head { (ui::status(Dot::Waiting, &format!("Connected as {}, without channel points", c.login))) }
                        p.small { "Twitch didn't give the bot permission to manage channel points redemptions. Connect again and allow it." }
                        div { (connect) }
                    }
                    _ => {
                        div.rv-tw-head { (ui::status(Dot::Off, "Not connected")) }
                        p.small { "Sign in as the broadcaster so the bot can read and fulfil redemptions. Twitch only shows the bot redemptions of rewards it created, so you pick or create the reward here after connecting. Stream alerts reads the follower total with the same sign-in." }
                        p.muted.small { "The Twitch app needs " code { (redirect) } " in its OAuth redirect URLs." }
                        div { (connect) }
                    }
                }
            }
        }
    }
}

fn reward_options(list: &[twitch::Reward]) -> Vec<(String, String)> {
    let mut options = vec![(String::new(), "Pick a reward".to_owned())];
    options.extend(list.iter().map(|r| (r.id.clone(), format!("{} · {} points{}", r.title, text::num(r.cost), if r.enabled { "" } else { " · paused" }))));
    options
}

fn reward_picker(page: &Page, s: &Settings, rewards: Option<Result<Vec<twitch::Reward>, String>>) -> Markup {
    html! {
        @match rewards {
            None => {
                @if s.reward_id.is_some() { p.small { "Reward: " strong { (s.reward_title) } } } @else { p.muted.small { "No reward picked yet." } }
            }
            Some(Err(e)) => { p.rv-warn { "Couldn't list the channel's rewards. " (e) } }
            Some(Ok(list)) => {
                @if list.is_empty() {
                    p.small { "The channel has no reward the bot made yet. Create one here; rewards made on Twitch itself can't be read by the bot." }
                } @else {
                    @let options = reward_options(&list);
                    form.rv-reward method="post" action=(page.path("reviews/twitch/reward")) {
                        label.small for="reward" { "Reward" }
                        div.inline-form {
                            (ui::select("reward", &options, s.reward_id.as_deref().unwrap_or_default()))
                            button.btn type="submit" { "Use this reward" }
                        }
                        p.help { "Only rewards the bot created are listed; Twitch doesn't let it read the others." }
                    }
                }
                (ui::pop("rv-pop", "Create a reward", Some(Icon::Plus), "", html! {
                    form.stack method="post" action=(page.path("reviews/twitch/reward/new")) {
                        label.small for="rw-title" { "Title" }
                        input.input #rw-title type="text" name="title" value="Account review" maxlength="45" required;
                        label.small for="rw-cost" { "Cost in points" }
                        input.input.num #rw-cost type="number" name="cost" value="10000" min="1" max="1000000" required;
                        p.help { "Viewers type the account to review when they redeem it." }
                        div { button.btn.small.primary type="submit" { "Create and use" } }
                    }
                }))
            }
        }
    }
}

/// "Send a test" under the messages: the ping and the closing message, as saved, in the
/// editor's DMs, filled in as if the request were theirs.
async fn send_test(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let me = page.user.id;
    let name = text::escape_markdown(&ctx.display_name(guild, me));
    let ping = if s.ping_message.trim().is_empty() { DEFAULT_PING } else { s.ping_message.as_str() };
    let fill_me = |t: &str| text::fill(t, &[("mention", &format!("<@{me}>")), ("name", &name)]);
    let mut sends = Vec::new();
    if !s.nudge_message.trim().is_empty() {
        sends.push(format!("{}\n-# Test from the dashboard: the toolbox link reminder, posted once in a post without a link.", fill_me(s.nudge_message.trim())));
    }
    sends.push(format!("{}\n-# Test from the dashboard: the ping, as it posts in their post.", fill_me(ping)));
    if !s.done_message.trim().is_empty() {
        sends.push(format!("{}\n-# Test from the dashboard: the closing message, posted when you complete a request.", fill_me(s.done_message.trim())));
    }
    // The preview has no Discord to send to.
    if !ctx.cfg.preview {
        for content in &sends {
            match ctx.dm(me, &text::clip(content, 2000), &[], &[]).await {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return Err(WebError::Bad(
                        "Discord didn't let the bot DM you. Allow direct messages from members of this server (the server's Privacy Settings in Discord), then try again.".into(),
                    ));
                }
                Err(e) => {
                    bot_core::tracing::warn!(guild, error = %e, "couldn't send a test review message");
                    return Err(WebError::Bad("The test didn't send. Discord turned it down; try again in a minute.".into()));
                }
            }
        }
    }
    ctx.audit(&page.user, Some(guild), KEY, "Sent themselves the account review messages as a test");
    Ok(Redirect::to(&format!("{}?sent=1", page.path("reviews/settings"))).into_response())
}

async fn save_settings(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let mut s = (*ctx.settings::<Settings>(guild, KEY)?).clone();
    s.channel = form.channel(&ctx, guild, "channel");
    s.ping_message = text::clip(form.text("ping_message").trim(), 1500);
    s.done_message = text::clip(form.text("done_message").trim(), 1500);
    s.nudge_message = text::clip(form.text("nudge_message").trim(), 1500);
    s.wait_days = form.int_in("wait_days", 0, 365, 7);
    s.lock_hours = form.int_in("lock_hours", 0, 720, 24);
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed the account review settings");
    Ok(saved(&page.path("reviews/settings")))
}

async fn twitch_connect(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    ctx.page(&user, guild, KEY, Level::Edit)?;
    let nonce: [u8; 16] = bot_core::rand::rng().random();
    let state = format!("{guild}.{}", nonce.iter().map(|b| format!("{b:02x}")).collect::<String>());
    let url = twitch::authorize_url(&ctx, &state).ok_or_else(|| WebError::Bad("Twitch isn't set up on this bot. Set TWITCH_CLIENT_ID and TWITCH_CLIENT_SECRET first.".into()))?;
    Ok(([(header::SET_COOKIE, state_cookie(&ctx, &state, 600))], Redirect::to(&url)).into_response())
}

fn state_cookie(ctx: &Ctx, value: &str, max_age: i64) -> String {
    let secure = if ctx.cfg.secure_cookies() { "; Secure" } else { "" };
    format!("{STATE_COOKIE}={value}; Path=/reviews/twitch; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}")
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// Twitch sends the broadcaster back here. The state names the server and must match the
/// cookie set when the sign-in started.
async fn twitch_callback(State(ctx): State<Ctx>, user: User, headers: HeaderMap, Query(q): Query<CallbackQuery>) -> WebResult {
    let expected = bot_core::web::auth::cookie(&headers, STATE_COOKIE);
    let state = q.state.unwrap_or_default();
    if state.is_empty() || expected.as_deref() != Some(state.as_str()) {
        return Err(WebError::Bad("The Twitch sign-in link expired. Start again from the Account reviews settings.".into()));
    }
    let guild: u64 = state.split('.').next().and_then(|g| g.parse().ok()).ok_or(WebError::NotFound)?;
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let clear = state_cookie(&ctx, "", 0);
    let Some(code) = q.code.filter(|_| q.error.is_none()) else {
        return Ok(([(header::SET_COOKIE, clear)], Redirect::to(&format!("{}?twitch=denied", page.path("reviews/settings")))).into_response());
    };
    let login = twitch::connect(&ctx, guild, &code).await.map_err(|e| WebError::Bad(format!("Twitch didn't connect. {e}")))?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Connected Twitch as {login} for channel points"));
    Ok(([(header::SET_COOKIE, clear)], saved(&page.path("reviews/settings"))).into_response())
}

async fn twitch_disconnect(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    twitch::disconnect(&ctx, guild).await?;
    ctx.audit(&page.user, Some(guild), KEY, "Disconnected Twitch");
    Ok(saved(&page.path("reviews/settings")))
}

async fn pick_reward(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let id = form.string("reward");
    let mut s = (*ctx.settings::<Settings>(guild, KEY)?).clone();
    if id.is_empty() {
        s.reward_id = None;
        s.reward_title.clear();
    } else {
        let list = twitch::rewards(&ctx, guild).await.map_err(|e| WebError::Bad(format!("Couldn't list the channel's rewards. {e}")))?;
        let r = list.into_iter().find(|r| r.id == id).ok_or_else(|| WebError::Bad("That reward is gone from Twitch. Pick another.".into()))?;
        s.reward_id = Some(r.id);
        s.reward_title = r.title;
    }
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, if s.reward_id.is_some() { format!("Set the review reward to {}", s.reward_title) } else { "Cleared the review reward".into() });
    Ok(saved(&page.path("reviews/settings")))
}

async fn new_reward(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let title = text::clip(form.string("title").trim(), 45);
    if title.is_empty() {
        return Err(WebError::Bad("Give the reward a title.".into()));
    }
    let cost = form.int_in("cost", 1, 1_000_000, 10_000);
    let r = twitch::create_reward(&ctx, guild, &title, cost).await.map_err(|e| WebError::Bad(format!("Twitch didn't create the reward. {e}")))?;
    let mut s = (*ctx.settings::<Settings>(guild, KEY)?).clone();
    s.reward_id = Some(r.id);
    s.reward_title = r.title;
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Created the {} reward on Twitch", s.reward_title));
    Ok(saved(&page.path("reviews/settings")))
}

// ---- preview ----

pub fn seed(ctx: &Ctx, guild: u64) -> bot_core::anyhow::Result<()> {
    use bot_core::preview::MEMBERS;
    let ch = |n: u64| guild + n * 10;
    let s = Settings { channel: Some(ch(6)), reward_id: Some("preview-reward".into()), reward_title: "Account review".into(), ..Settings::default() };
    ctx.save_settings(guild, KEY, &s)?;
    let now = now_ms();
    let h = 3_600_000;
    let m = |i: usize| Some(MEMBERS[i].0 as i64);
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    // (name, member, thread, twitch viewer, redeemed, asked hours ago, status, notes, pinged, closed hours ago, by)
    type Row<'a> = (&'a str, Option<i64>, Option<i64>, Option<&'a str>, bool, i64, &'a str, &'a str, Option<i64>, Option<i64>, Option<&'a str>);
    let rows: [Row; 11] = [
        ("Kasia", m(4), Some(901), Some("kasia_plays"), true, 3, "open", "https://idleontoolbox.com/?profile=Kasia\nWants help with W5 sailing.", None, None, None),
        ("Moonfrost", None, None, Some("pavel_tv"), true, 1, "open", "Redeemed by Pavel_TV on Twitch", None, None, None),
        ("bramblepaw", m(5), Some(902), None, false, 77, "open", "", Some(2), None, None),
        ("Noor's main", m(6), Some(903), None, false, 52, "open", "Second account is Noorlet.", None, None, None),
        ("Teo", m(3), Some(904), None, false, 20, "open", "", None, None, None),
        ("Ines", m(8), Some(905), None, false, 5, "open", "", None, None, None),
        ("Oskar", m(1), Some(906), None, false, 120, "done", "Alchemy bubbles behind, told him to level cauldrons.", None, Some(6), Some("Antho")),
        ("juniper.fern", m(2), Some(907), Some("juniper_fern"), true, 60, "done", "", None, Some(30), Some("Mira")),
        ("Review please", m(9), Some(908), None, false, 90, "removed", "Duplicate of Pavel's earlier post.", None, Some(48), Some("Antho")),
        ("Oskar", m(1), Some(909), None, false, 2, "waiting", "Did the cauldrons, wants a second look.", None, None, None),
        ("juniper.fern", None, None, Some("juniper_fern"), true, 1, "waiting", "", None, None, None),
    ];
    for (name, user, thread, viewer, redeemed, ago, status, notes, pinged, closed, by) in rows {
        let asked = now - ago * h;
        let thread = thread.map(|t| guild as i64 + t);
        let redemption = redeemed.then(|| format!("preview-{name}-{status}"));
        tx.execute(
            "INSERT INTO reviews (guild_id, name, user_id, thread_id, twitch_user, reward_id, redemption_id, redeemed_at, requested_at, status, notes, source, pinged_at, closed_at, closed_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                guild as i64,
                name,
                user,
                thread,
                viewer,
                redeemed.then_some("preview-reward"),
                redemption,
                redeemed.then_some(asked),
                asked,
                status,
                notes,
                if viewer.is_some() && thread.is_none() { "twitch" } else { "thread" },
                pinged.map(|p| now - p * h),
                closed.map(|c| now - c * h),
                by,
            ],
        )?;
        let id = tx.last_insert_rowid();
        if let Some(t) = thread {
            store::add_source(&tx, guild, "thread", &t.to_string(), id, None)?;
        }
        if let Some(d) = &redemption {
            store::add_source(&tx, guild, "redemption", d, id, Some("preview-reward"))?;
        }
    }
    // Waitlisted ones join a week after their last review.
    tx.execute(
        "UPDATE reviews SET eligible_at = (SELECT MAX(d.closed_at) FROM reviews d WHERE d.guild_id = reviews.guild_id AND d.status = 'done' AND d.name = reviews.name) + ?2
         WHERE guild_id = ?1 AND status = 'waiting'",
        params![guild as i64, s.wait_ms()],
    )?;
    tx.execute("UPDATE reviews SET locked_at = closed_at + 86400000 WHERE guild_id = ?1 AND name = 'juniper.fern' AND status = 'done'", [guild as i64])?;
    // Toolbox links, tags, and the ping states: bramblepaw answered ready, Teo was just pinged.
    let staff = MEMBERS[0].0 as i64;
    for (name, toolbox, tags, nudged, pinged, by, ready) in [
        ("Kasia", Some("https://idleontoolbox.com/account/misc/general?profile=Kasia"), "Late game,W5", None, None, None, None),
        ("bramblepaw", Some("https://idleontoolbox.com/?profile=bramblepaw"), "Mid game,W4", None, Some(2 * h), Some(staff), Some(20 * 60_000)),
        ("Noor's main", Some("https://idleontoolbox.com/dashboard?profile=Noorlet"), "Early game,W2", None, None, None, None),
        ("Teo", None, "W3", Some(19 * h), Some(10 * 60_000), Some(staff), None),
        ("Ines", None, "", Some(5 * h - 3 * 60_000), None, None, None),
    ] {
        tx.execute(
            "UPDATE reviews SET toolbox = ?3, tags = ?4, nudged_at = ?5, pinged_at = COALESCE(?6, pinged_at), pinged_by = ?7, ready_at = ?8 WHERE guild_id = ?1 AND name = ?2 AND status = 'open'",
            params![guild as i64, name, toolbox, tags, nudged.map(|n: i64| now - n), pinged.map(|p: i64| now - p), by, ready.map(|r: i64| now - r)],
        )?;
    }
    tx.execute(
        "INSERT OR REPLACE INTO reviews_twitch (guild_id, broadcaster_id, login, access_token, refresh_token, expires_at, scopes, synced_at) VALUES (?1, '1', 'antho', NULL, 'preview', 0, ?2, ?3)",
        params![guild as i64, twitch::SCOPE, now - 3 * 60_000],
    )?;
    tx.commit()?;
    Ok(())
}

/// Sample messages for the preview's panel, some of which the panel hides.
fn sample_talk(r: &Review) -> Vec<Said> {
    let now = now_ms();
    let member = r.user_id.unwrap_or(1);
    let min = 60_000;
    let start = r.requested_at.min(now - 40 * min);
    let link = r.toolbox_link().unwrap_or_else(|| format!("https://idleontoolbox.com/account/misc/general?profile={}", r.name));
    let said = |author: u64, bot: bool, after: i64, content: &str| Said { author, bot, at: start + after * min, content: content.into(), ..Said::default() };
    vec![
        said(member, false, 0, &format!("Hi, could I get a review of {}? {}mostly stuck on sailing and my alchemy feels behind.", r.name, r.tags.first().map(|t| format!("{t}, ")).unwrap_or_default())),
        said(1, true, 3, "Please share your IdleonToolbox profile link in this post so it's ready for your review."),
        said(member, false, 5, &format!("here you go {link}")),
        said(2, false, 6, "same boat as you lol"),
        said(member, false, 8, "thanks!"),
        said(member, false, 12, "Also not sure about my Bowman's talents, I put most points into the first tab."),
        said(member, false, 30, "bump"),
    ]
}

/// The reviews pages' script, run on every load and swap. "Open post" tries the Discord app
/// first: if the page doesn't lose focus within a second (the app didn't take over), it opens
/// discord.com in a new tab instead. A queue row opens on a click to what the member wrote,
/// loaded the first time.
const PAGE_JS: &str = r##"(() => {
  const main = document.querySelector("main.page");
  if (!main || main.dataset.rvBound) return;
  main.dataset.rvBound = "1";
  const signal = window.botPage?.signal;
  const openApp = (a) => {
    let left = false;
    const gone = () => { left = true; };
    const opts = { once: true, signal };
    window.addEventListener("blur", gone, opts);
    window.addEventListener("pagehide", gone, opts);
    document.addEventListener("visibilitychange", gone, opts);
    // A hidden frame, so a browser without the app shows nothing rather than an error page.
    const f = document.createElement("iframe");
    f.hidden = true;
    f.src = a.dataset.app;
    document.body.append(f);
    setTimeout(() => {
      f.remove();
      window.removeEventListener("blur", gone);
      window.removeEventListener("pagehide", gone);
      document.removeEventListener("visibilitychange", gone);
      // The app took over, or a prompt asking to open it has the focus: no browser tab too.
      if (left || signal?.aborted || !document.hasFocus()) return;
      const w = window.open(a.href, "_blank");
      if (w) { w.opener = null; return; }
      // The browser blocked the tab. The dashboard stays put; the next click opens the post in
      // the browser the ordinary way.
      a.dataset.web = "1";
      window.botToast?.("Discord didn't open. Click Open post again to open it in the browser.", true);
    }, 1000);
  };
  const toggle = (row) => {
    const panel = row.querySelector(".rv-talk");
    if (!panel) return;
    const open = panel.hidden;
    panel.hidden = !open;
    row.classList.toggle("open", open);
    row.querySelector(".rv-name")?.setAttribute("aria-expanded", String(open));
    if (!open || panel.dataset.loaded) return;
    panel.dataset.loaded = "1";
    fetch(row.dataset.talk, { credentials: "same-origin", signal })
      .then((res) => (res.ok ? res.text() : Promise.reject(res.status)))
      .then((html) => { panel.innerHTML = html; })
      .catch(() => {
        if (signal?.aborted) return;
        delete panel.dataset.loaded;
        panel.innerHTML = `<p class="rv-talk-note">The post didn't load. Close it and open it again to retry.</p>`;
      });
  };
  main.addEventListener("click", (e) => {
    const a = e.target.closest("a[data-app]");
    if (a) {
      if (e.button || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || a.dataset.web) return;
      e.preventDefault();
      openApp(a);
      return;
    }
    const row = e.target.closest("[data-talk]");
    if (!row || e.target.closest("a, button:not(.rv-name), form, details, input, textarea, select, label, .rv-talk")) return;
    // Selecting text in a row isn't a click on it.
    if (String(window.getSelection?.() || "").trim()) return;
    toggle(row);
  });
})();"##;

const CSS: &str = r#"
.rv-main { display: flex; flex-direction: column; gap: 20px; min-width: 0; }
.rv-readings { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 10px; }
.rv-readings .reading { min-width: 0; }
.rv-note { display: flex; align-items: center; gap: 8px; font-size: 13px; color: var(--ink-2); }
.rv-note .icon { width: 15px; height: 15px; flex: none; color: var(--live); }
.rv-warn { font-size: 12.5px; color: var(--waiting); }
.rv-search { position: relative; display: flex; gap: 8px; align-items: center; }
.rv-search > .icon { position: absolute; left: 12px; top: 10px; width: 15px; height: 15px; color: var(--ink-3); pointer-events: none; }
.rv-search .input { flex: 1; max-width: 420px; padding-left: 36px; }

/* A list of requests: one surface, a hairline between rows. */
.rv-queue, .rv-wl, .rv-done { list-style: none; margin: 0; padding: 0; border-radius: var(--r-card); background: var(--well); box-shadow: inset 0 0 0 1px var(--edge-1); }

/* The queue, in the order you take them. The first, when it isn't filtered, is a size up. */
.rv-queue { display: flex; flex-direction: column; }
.rv-row { position: relative; display: grid; grid-template-columns: 40px minmax(0, 1fr) auto; gap: 8px 14px; align-items: start; padding: 14px 18px; transition: background 0.2s; }
.rv-row + .rv-row { border-top: 1px solid var(--edge-1); }
.rv-row:first-child { border-radius: var(--r-card) var(--r-card) 0 0; }
.rv-row:last-child { border-radius: 0 0 var(--r-card) var(--r-card); }
.rv-row:only-child { border-radius: var(--r-card); }
.rv-row:hover { background: var(--well); }
.rv-row:has(.pop[open]), .rv-card:has(.pop[open]) { z-index: 5; }
/* Numbered, the faces share a column as wide as the first one's, centred, so every name starts
   on one line. */
.rv-queue.numbered .rv-row { grid-template-columns: 16px 48px minmax(0, 1fr) auto; }
.rv-queue.numbered .rv-face { justify-self: center; }
.rv-place { font-size: 13px; line-height: 20px; font-weight: 500; color: var(--ink-3); font-variant-numeric: tabular-nums; text-align: right; }
.rv-row.next { padding-top: 18px; padding-bottom: 18px; }
.rv-row.next .rv-place { line-height: 22px; color: var(--ink); }
.rv-next { font-size: 12px; font-weight: 600; color: var(--hue); }
.rv-face { width: 40px; height: 40px; }
.rv-face.big { width: 48px; height: 48px; }
.rv-face.small { width: 30px; height: 30px; font-size: 12px; }
/* A Twitch viewer with no member: their first letter on Twitch's purple. */
.rv-letter { display: grid; place-items: center; border-radius: 50%; color: #fff; font-weight: 600; font-size: 16px; background: #9146ff; }
.rv-letter.big { font-size: 19px; }
.rv-body { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.rv-title { display: flex; align-items: center; gap: 4px 12px; flex-wrap: wrap; min-height: 20px; }
.rv-title strong { font-size: 14.5px; font-weight: 600; overflow-wrap: anywhere; }
.rv-row.next .rv-title { min-height: 22px; }
.rv-row.next .rv-title strong { font-size: 16px; letter-spacing: -0.01em; }
.rv-flag { display: inline-flex; align-items: center; gap: 6px; font-size: 12px; color: var(--ink-2); white-space: nowrap; }
.rv-flag .dot { width: 6px; height: 6px; }
.rv-flag.cp .dot { background: #a970ff; }
.rv-flag.pinged .dot { background: var(--waiting); }
/* Answered ready: waiting on staff now, so it reads like a count that waits on someone. */
.rv-flag.ready { color: var(--signal); font-weight: 600; }
.rv-flag.ready .dot { background: var(--signal); }
.rv-tag { display: inline-flex; align-items: center; height: 20px; padding: 0 8px; border-radius: 999px; font-size: 11.5px; font-weight: 500; color: var(--ink-2); background: var(--veil); box-shadow: inset 0 0 0 1px var(--edge-1); white-space: nowrap; }
.rv-nolink { color: var(--waiting); cursor: default; }
/* The name opens what the member wrote; it reads as the name, not a button. */
.rv-name { all: unset; font-size: 14.5px; font-weight: 600; overflow-wrap: anywhere; cursor: pointer; border-radius: 4px; }
.rv-row.next .rv-name { font-size: 16px; letter-spacing: -0.01em; }
.rv-name:hover { text-decoration: underline; text-decoration-color: var(--edge-3); text-underline-offset: 3px; }
.rv-name:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
.rv-row[data-talk] { cursor: pointer; }
.rv-row[data-talk] :is(.rv-side, .rv-notes) { cursor: auto; }
.rv-row.open { background: var(--well); }
/* What the member wrote, under the row across its width. */
.rv-row > .rv-talk { grid-column: 1 / -1; cursor: auto; }
.rv-talk:not([hidden]) { display: flex; flex-direction: column; gap: 12px; margin-top: 6px; padding: 14px 16px; border-radius: 10px; background: var(--page); box-shadow: inset 0 0 0 1px var(--edge-1); animation: rv-talk-in 0.28s var(--ease); }
@keyframes rv-talk-in { from { opacity: 0; transform: translateY(-4px); } }
.rv-talk-head { display: flex; align-items: center; justify-content: space-between; gap: 12px; font-size: 12.5px; color: var(--ink-3); }
.rv-talk-note { font-size: 12.5px; color: var(--ink-3); }
.rv-said { display: grid; grid-template-columns: 76px minmax(0, 1fr); gap: 12px; }
.rv-said + .rv-said { padding-top: 12px; border-top: 1px solid var(--edge-1); }
.rv-said-at { font-size: 12px; line-height: 20px; color: var(--ink-3); font-variant-numeric: tabular-nums; white-space: nowrap; cursor: default; }
.rv-said-body { display: flex; flex-direction: column; gap: 8px; min-width: 0; }
.rv-said-text { font-size: 13.5px; line-height: 1.5; color: var(--ink); white-space: pre-line; overflow-wrap: anywhere; max-width: 80ch; }
.rv-said-text a { color: var(--accent-hi); text-decoration: underline; text-underline-offset: 2px; }
.rv-said-pics { display: flex; flex-wrap: wrap; gap: 8px; }
.rv-said-pics img { display: block; max-width: 260px; max-height: 180px; border-radius: 8px; object-fit: cover; box-shadow: inset 0 0 0 1px var(--edge-1); }
.rv-meta { display: flex; flex-wrap: wrap; align-items: center; gap: 2px 0; font-size: 12.5px; color: var(--ink-3); }
.rv-meta > :not(:last-child)::after { content: "·"; margin: 0 7px; }
.rv-link { display: inline-flex; align-items: center; gap: 5px; color: var(--ink-2); transition: color 0.18s; }
.rv-link .icon { width: 12px; height: 12px; }
.rv-link:hover { color: var(--ink); }
.rv-notes { margin-top: 2px; font-size: 13px; line-height: 1.5; color: var(--ink-2); white-space: pre-line; overflow-wrap: anywhere; max-width: 62ch; }
.rv-side { grid-column: -2 / -1; display: flex; flex-direction: column; align-items: flex-end; gap: 10px; }
/* A request's notes sit under its name, beside the face and the actions. */
.rv-row > .rv-notes { grid-column: -3 / -2; grid-row: 2; margin-top: 0; }
.rv-row:has(> .rv-notes) > :is(.rv-place, .rv-face, .rv-side) { grid-row: 1 / span 2; }
/* On a wide column the queue reads like a table: who asked, their notes, the wait and actions. */
@container col (min-width: 1200px) {
  .rv-row { grid-template-columns: 40px minmax(0, 0.85fr) minmax(0, 1.15fr) auto; column-gap: 24px; }
  .rv-queue.numbered .rv-row { grid-template-columns: 16px 48px minmax(0, 0.85fr) minmax(0, 1.15fr) auto; }
  .rv-queue.numbered .rv-row > .rv-place { margin-right: -10px; }
  .rv-row > .rv-notes { grid-row: 1; padding-top: 1px; }
  /* The same width for the actions on every row, so the notes start on one line. */
  .rv-row > .rv-side { min-width: 210px; }
  .rv-row:has(> .rv-notes) > :is(.rv-place, .rv-face, .rv-side) { grid-row: 1; }
}
.rv-wait { font-size: 12px; line-height: 20px; color: var(--ink-3); white-space: nowrap; cursor: default; }
.rv-wait b { font-size: 14px; font-weight: 600; color: var(--ink); font-variant-numeric: tabular-nums; }
.rv-acts { position: relative; display: flex; gap: 6px; align-items: center; justify-content: flex-end; }
.rv-acts .pop-body { width: 290px; right: 0; left: auto; }
.rv-pop-foot { border-top: 1px solid var(--edge-1); padding-top: 10px; }

/* The rail. A source's icon takes its colour when it's working. */
.rv-src { display: grid; place-items: center; width: 30px; height: 30px; flex: none; color: var(--ink-3); }
.rv-src .icon { width: 18px; height: 18px; }
.rv-src.on { color: var(--hue); }
.rv-src.twitch.on { color: #a970ff; }
.rv-ago { font-size: 12px; color: var(--ink-3); white-space: nowrap; cursor: default; }
.rail-block h2 a { font-weight: 400; }

/* Waitlist and done. */
.rv-intro { font-size: 13.5px; line-height: 1.55; color: var(--ink-2); max-width: 80ch; }
.rv-intro strong { color: var(--ink); font-weight: 600; }
.rv-wl { display: flex; flex-direction: column; }
.rv-card { position: relative; display: flex; flex-direction: column; gap: 12px; padding: 16px 18px; }
.rv-card + .rv-card { border-top: 1px solid var(--edge-1); }
.rv-card-head { display: flex; align-items: flex-start; gap: 14px; min-width: 0; }
/* Stacked, the notes come right under the name they belong to. */
.rv-card > .rv-notes { order: 1; margin: 0 0 0 54px; }
.rv-card > .rv-countdown { order: 2; }
.rv-card > .rv-acts { order: 3; }
.rv-countdown { display: flex; flex-direction: column; gap: 8px; min-width: 0; }
.rv-cd-text { display: flex; align-items: baseline; justify-content: space-between; gap: 4px 12px; flex-wrap: wrap; font-size: 13px; color: var(--ink-2); }
.rv-cd-text strong { color: var(--ink); font-weight: 600; }
.rv-cd-last { font-size: 12px; color: var(--ink-3); }
.rv-countdown .meter { height: 4px; }
.rv-countdown .meter > span { background: var(--data); }
.rv-card .rv-acts { justify-content: flex-start; }
.rv-card .rv-acts .pop-body { left: 0; right: auto; }
/* A wide page lists the waitlist across: who, their countdown, then the actions at the end. */
@container col (min-width: 900px) {
  .rv-card { display: grid; grid-template-columns: minmax(0, 1fr) minmax(240px, 0.9fr) auto; grid-template-areas: "head cd acts"; align-items: center; gap: 10px 32px; }
  .rv-card:has(> .rv-notes) { grid-template-areas: "head cd acts" "notes cd acts"; }
  .rv-card > .rv-card-head { grid-area: head; }
  .rv-card > .rv-countdown { grid-area: cd; justify-self: end; width: 100%; max-width: 480px; }
  .rv-card > .rv-notes { grid-area: notes; }
  .rv-card > .rv-acts { grid-area: acts; justify-content: flex-end; }
  .rv-card .rv-acts .pop-body { left: auto; right: 0; }
}
.rv-readings.three { grid-template-columns: repeat(3, minmax(0, 1fr)); }
/* Finished requests: one column, two once there's room. */
.rv-done { display: grid; grid-template-columns: minmax(0, 1fr); padding: 0 18px; }
.rv-drow { display: grid; grid-template-columns: 40px minmax(0, 1fr) auto; gap: 14px; align-items: center; padding: 14px 0; }
.rv-drow + .rv-drow { border-top: 1px solid var(--edge-1); }
@container col (min-width: 1100px) {
  .rv-done { grid-template-columns: repeat(2, minmax(0, 1fr)); column-gap: 40px; }
  .rv-drow:nth-child(2) { border-top: 0; }
}
.rv-dface { position: relative; width: 40px; height: 40px; }
.rv-dmark { position: absolute; right: -4px; bottom: -3px; display: grid; place-items: center; width: 18px; height: 18px; border-radius: 50%; background: var(--live); color: #062014; box-shadow: 0 0 0 2.5px var(--page); }
.rv-dmark .icon { width: 10px; height: 10px; stroke-width: 2.6; }
.rv-drow.removed .rv-dmark { background: var(--ink-3); color: var(--ground); }
.rv-drow.removed .rv-face { filter: saturate(0.4); opacity: 0.75; }
.rv-drow.removed .rv-title strong { color: var(--ink-2); }

/* Settings: the two messages side by side once the column is wide. */
@container col (min-width: 1100px) {
  .rv-msgs .card { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); column-gap: 28px; }
  .rv-msgs .row + .row { border-top: 0; }
  .rv-msgs .row.rv-test { grid-column: 1 / -1; border-top: 1px solid var(--edge-1); }
}
@container col (min-width: 1300px) {
  .rv-msgs .card { grid-template-columns: repeat(3, minmax(0, 1fr)); }
}

/* Settings: how a request moves, as dots on a line. */
.rv-settings { gap: 30px; }
.rv-flow { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 14px; }
.rv-flow li { position: relative; display: flex; gap: 14px; align-items: flex-start; }
.rv-flow li:not(:last-child)::before { content: ""; position: absolute; left: 4px; top: 17px; bottom: -15px; width: 1.5px; border-radius: 1px; background: var(--edge-3); }
.rv-flow-dot { width: 9px; height: 9px; margin-top: 5px; flex: none; border-radius: 50%; background: var(--hue); }
.rv-flow li.muted .rv-flow-dot { background: transparent; box-shadow: inset 0 0 0 1.5px var(--ink-3); }
.rv-flow li div { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.rv-flow strong { font-size: 13.5px; font-weight: 600; }
.rv-flow span { font-size: 12.5px; line-height: 1.45; color: var(--ink-2); }
.rv-flow li.muted strong { color: var(--ink-2); }
.rv-flow li.muted span { color: var(--ink-3); }

/* Settings: Twitch. */
.rv-twitch { padding: 18px 20px; display: flex; flex-direction: column; gap: 12px; }
.rv-tw-head { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; }
.rv-tw-head > form, .rv-tw-head > .inline { margin-left: auto; }
.rv-twitch code { font-size: 11.5px; overflow-wrap: anywhere; }
.rv-reward { display: flex; flex-direction: column; gap: 6px; }
.rv-reward .select-wrap { flex: 1; min-width: 200px; max-width: 360px; }
.rv-reward .select-wrap .input { width: 100%; max-width: none; }
.rv-twitch .pop-body { width: 260px; }

@media (max-width: 1180px) {
  .rv-readings { grid-template-columns: repeat(2, minmax(0, 1fr)); }
}
@media (max-width: 640px) {
  .rv-row, .rv-queue.numbered .rv-row, .rv-queue.numbered .rv-row.next { grid-template-columns: 40px minmax(0, 1fr); padding: 14px; }
  .rv-place { display: none; }
  .rv-row.next .rv-face.big { width: 40px; height: 40px; }
  .rv-side { grid-column: 1 / -1; flex-direction: row; align-items: center; justify-content: space-between; }
  .rv-row > .rv-notes { grid-column: 2 / -1; }
  .rv-row:has(> .rv-notes) > :is(.rv-place, .rv-face, .rv-side) { grid-row: auto; }
  .rv-acts { flex-wrap: wrap; }
  .rv-acts .pop { position: static; }
  .rv-side { position: relative; }
  .rv-acts .pop-body { left: 0; right: 0; width: auto; }
  .rv-search .input { min-width: 0; }
  .rv-card { padding: 14px; }
  .rv-card > .rv-notes { margin-left: 54px; }
  .rv-readings.three { grid-template-columns: repeat(2, minmax(0, 1fr)); }
  .rv-readings.three > :last-child { grid-column: 1 / -1; }
  .rv-done { padding: 0 14px; }
  .rv-drow { grid-template-columns: 40px minmax(0, 1fr); align-items: start; }
  .rv-drow .rv-acts { grid-column: 2; justify-content: flex-start; }
  .rv-said { grid-template-columns: minmax(0, 1fr); gap: 4px; }
  .rv-talk:not([hidden]) { padding: 12px; }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_read_in_words() {
        assert_eq!(wait_words(40 * 60_000), "40 min");
        assert_eq!(wait_words(5 * 3_600_000), "5 h");
        assert_eq!(wait_words(30 * 3_600_000), "1 day");
        assert_eq!(wait_words(3 * DAY_MS + 1), "3 days");
        assert_eq!(wait_words(-5), "0 min");
    }

    #[test]
    fn notes_after_a_scan_or_sync() {
        let q = QueueQuery { from: Some("scan".into()), added: Some(1), skipped: Some(3), ..QueueQuery::default() };
        assert_eq!(result_note(&q).as_deref(), Some("The scan added 1 request; 3 posts were already here."));
        let q = QueueQuery { from: Some("sync".into()), added: Some(0), moved: Some(1), ..QueueQuery::default() };
        assert_eq!(result_note(&q).as_deref(), Some("Twitch sync added 0 requests; 1 redemption joined requests already waiting."));
        let q = QueueQuery { from: Some("scan".into()), added: Some(0), skipped: Some(1), waiting: Some(2), ..QueueQuery::default() };
        assert_eq!(result_note(&q).as_deref(), Some("The scan added 0 requests; 1 post was already here. 2 requests went to the waitlist."));
        assert_eq!(result_note(&QueueQuery::default()), None);
    }

    #[test]
    fn pings_name_people_without_a_member() {
        let mut r = Review { id: 1, name: "Moonfrost".into(), status: "open".into(), source: "twitch".into(), ..Review::default() };
        assert_eq!(fill("{mention}, your turn ({name})", &r), "**Moonfrost**, your turn (Moonfrost)");
        r.user_id = Some(42);
        assert_eq!(fill("{mention}, your turn", &r), "<@42>, your turn");
        // A name can't turn into a masked link or bold under the bot's name.
        r.name = "[x](https://evil)".into();
        assert_eq!(fill("{name}", &r), r"\[x\]\(https://evil\)");
        r.user_id = None;
        r.name = "a**b".into();
        assert_eq!(fill("{mention}", &r), r"**a\*\*b**");
    }

    fn said(author: u64, content: &str) -> Said {
        Said { author, content: content.into(), ..Said::default() }
    }

    #[test]
    fn the_panel_keeps_what_helps_a_review() {
        assert!(useful(&said(10, "Late game, stuck on W5 sailing"), Some(10)));
        assert!(useful(&said(10, "https://idleontoolbox.com/?profile=Mira"), Some(10)));
        // Chatter, the bot's posts and other people's messages are left out.
        assert!(!useful(&said(10, "thanks!"), Some(10)));
        assert!(!useful(&said(10, "ready"), Some(10)));
        assert!(!useful(&said(11, "Late game too, same problem"), Some(10)));
        assert!(!useful(&Said { bot: true, ..said(10, "Share your toolbox link") }, Some(10)));
        // A screenshot with no words is kept.
        assert!(useful(&Said { images: vec![("a".into(), "b".into())], ..said(10, "") }, Some(10)));
        // With no member known, anyone's helpful message shows.
        assert!(useful(&said(11, "Bowman talents"), None));
    }

    #[test]
    fn links_in_messages_are_clickable() {
        let html = linked("see <https://idleontoolbox.com/?profile=Mira>, ok\njavascript:alert(1) <b>").into_string();
        assert!(html.contains(r#"<a href="https://idleontoolbox.com/?profile=Mira" target="_blank" rel="noreferrer noopener">&lt;https://idleontoolbox.com/?profile=Mira&gt;,</a> "#), "{html}");
        assert!(html.contains("javascript:alert(1) &lt;b&gt;"));
        assert!(!html.contains(r#"href="javascript"#));
        // Characters of any width don't panic.
        assert_eq!(linked("ミラ 🐸 é").into_string(), "ミラ 🐸 é");
    }
}
