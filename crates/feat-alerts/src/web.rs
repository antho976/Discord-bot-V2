use std::collections::HashMap;

use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::http::header;
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::{get, post};
use bot_core::jiff::civil::Date;
use bot_core::jiff::tz::TimeZone;
use bot_core::jiff::{Span, Timestamp};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::rusqlite::OptionalExtension;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text, tracing};

use crate::twitch::{self, Broadcaster, Mode, StreamRow};
use crate::{KEY, Settings, schedule, youtube};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/alerts", get(twitch_page).post(save_twitch))
        .route("/g/{guild}/alerts/test", post(send_test))
        .route("/g/{guild}/alerts/schedule", get(schedule_page).post(save_schedule))
        .route("/g/{guild}/alerts/schedule.png", get(schedule_png))
        .route("/g/{guild}/alerts/youtube", get(youtube_page).post(save_youtube))
        .route("/g/{guild}/alerts/streams", get(streams_page))
        .route("/webhooks/twitch", post(twitch::webhook))
}

fn tabs(page: &Page, active: &str) -> Markup {
    ui::tabs(
        &[
            ("Twitch", page.path("alerts")),
            ("Schedule", page.path("alerts/schedule")),
            ("YouTube", page.path("alerts/youtube")),
            ("Streams", page.path("alerts/streams")),
        ],
        active,
    )
}

fn zoned(ms: i64, tz: &TimeZone) -> Option<bot_core::jiff::Zoned> {
    Timestamp::from_millisecond(ms).ok().map(|t| t.to_zoned(tz.clone()))
}

/// "Sat, Nov 1 · 7:00 PM"
fn when(ms: i64, tz: &TimeZone) -> String {
    zoned(ms, tz).map(|z| z.strftime("%a, %b %-d · %-I:%M %p").to_string()).unwrap_or_default()
}

/// "in 40 min", "in 5 h", "in 2 days".
fn until(ms: i64) -> String {
    let mins = (ms - now_ms()) / 60_000;
    match mins {
        ..1 => "now".into(),
        1..60 => format!("in {mins} min"),
        60..2880 => format!("in {} h", mins / 60),
        _ => format!("in {} days", mins / 1440),
    }
}

fn role_name(ctx: &Ctx, guild: u64, role: Option<u64>) -> Option<String> {
    let role = role?;
    ctx.roles(guild).into_iter().find(|(id, _, _)| *id == role).map(|(_, n, _)| format!("@{n}"))
}

fn channel_name(ctx: &Ctx, guild: u64, channel: Option<u64>) -> Option<String> {
    let channel = channel?;
    ctx.text_channels(guild).into_iter().find(|(id, _)| *id == channel).map(|(_, n)| format!("#{n}"))
}

fn month_start(tz: &TimeZone) -> i64 {
    let today = Timestamp::now().to_zoned(tz.clone()).date();
    today.first_of_month().to_zoned(tz.clone()).map(|z| z.timestamp().as_millisecond()).unwrap_or(0)
}

fn toast_if(q: &HashMap<String, String>, key: &str, msg: &str) -> Markup {
    html! {
        @if q.contains_key(key) {
            script { (PreEscaped(format!(
                "addEventListener('DOMContentLoaded',()=>{{window.botToast&&window.botToast({msg:?});history.replaceState(null,'',location.pathname)}})"
            ))) }
        }
    }
}

// ---- Twitch helpers ----

/// Weeks in the "Hours streamed" chart, this one last.
const WEEKS: i64 = 12;

/// The Twitch connection and how fast alerts arrive, at the right of the header.
fn status_pill(ctx: &Ctx) -> Markup {
    let (live, label, more) = match twitch::mode(ctx) {
        Mode::Off => (false, "Twitch not connected", ""),
        Mode::Poll => (true, "Twitch connected", " · checks every 2 min"),
        Mode::Push => (true, "Twitch connected", " · instant alerts"),
    };
    // The detail drops on a phone so the pill doesn't crowd the page title.
    html! {
        style { "@media (max-width: 600px) { .sa-pill-more { display: none; } }" }
        span.status { span.dot.live[live] {} (label) span.sa-pill-more { (more) } }
    }
}

/// How alerts reach Discord, under the preview. Empty when Twitch isn't connected.
fn mode_line(mode: Mode) -> &'static str {
    match mode {
        Mode::Off => "",
        Mode::Poll => "The bot checks Twitch every 2 minutes. Set TWITCH_EVENTSUB_SECRET and an https PUBLIC_URL to get alerts the moment you go live.",
        Mode::Push => "Twitch tells the bot the moment you go live. Viewer counts are read every 5 minutes while you're live.",
    }
}

/// What the preview of the live post shows.
struct Sample<'a> {
    title: &'a str,
    game: &'a str,
    /// "just now", or how long ago a live stream started.
    started: String,
    /// A real picture of the stream when one is stored, else a placeholder is drawn.
    thumb: Option<String>,
}

/// A message from the bot as Discord shows it: avatar, name with its APP tag and the time,
/// then `body`.
fn bot_message(ctx: &Ctx, page: &Page, body: Markup) -> Markup {
    let bot = ctx.cache.current_user();
    let bot_name = bot.as_ref().map(|u| u.name.clone()).unwrap_or_else(|| "Bot".into());
    let bot_avatar = bot.as_ref().map(|u| bot_core::discord::avatar_url(u.id.get(), u.avatar.map(|a| a.to_string()).as_deref()));
    let time = page.user.local(now_ms()).strftime("%-I:%M %p").to_string();
    html! {
        div.sa-post {
            @if let Some(url) = &bot_avatar {
                img.sa-av src=(url) alt="" width="40" height="40" loading="lazy";
            } @else {
                span.sa-av.letter { (bot_name.chars().next().unwrap_or('B')) }
            }
            div.sa-post-main {
                div.sa-head {
                    b.sa-name { (bot_name) }
                    span.sa-app { "APP" }
                    span.sa-time { "Today at " (time) }
                }
                (body)
            }
        }
    }
}

/// The ping in front of a post, when a role is set.
fn ping(role: Option<String>) -> Markup {
    html! { @if let Some(r) = role { span.sa-ping { (r) } " " } }
}

/// The live post as Discord shows it: the ping and the message, the embed with its coral edge,
/// its fields, picture and footer, and the Watch on Twitch button.
fn live_post(ctx: &Ctx, page: &Page, role: Option<String>, line: &str, b: &Broadcaster, sample: &Sample) -> Markup {
    bot_message(ctx, page, html! {
        div.sa-line { (ping(role)) (line) }
        div.sa-embed style="--embed:#f0806a" {
            div.sa-author {
                @if !b.avatar.is_empty() { img src=(b.avatar) alt="" width="24" height="24" loading="lazy"; }
                (b.name)
            }
            div.sa-title { @if sample.title.is_empty() { (b.name) " is live" } @else { (sample.title) } }
            div.sa-fields {
                @if !sample.game.is_empty() { div { b { "Playing" } span { (sample.game) } } }
                div { b { "Started" } span { (sample.started) } }
            }
            @match &sample.thumb {
                Some(src) => { img.sa-thumb src=(src) alt="" width="640" height="360" loading="lazy"; }
                None => { div.sa-thumb { (ui::icon(Icon::Live)) span { "Stream thumbnail" } } }
            }
            div.sa-foot { "Twitch" }
        }
        a.sa-watch href=(b.url()) target="_blank" rel="noopener" { "Watch on Twitch" (ui::icon(Icon::External)) }
    })
}

/// The closest reminder before a stream, as Discord shows it. Discord fills the relative time
/// and the start time in for each reader.
fn reminder_post(ctx: &Ctx, page: &Page, role: Option<String>, b: &Broadcaster, seg: Option<&schedule::Segment>, minutes: i64) -> Markup {
    let start = seg.map(|g| g.starts_at).unwrap_or_else(|| now_ms() + minutes * 60_000);
    let at = page.user.local(start).strftime("%-I:%M %p").to_string();
    let (title, game) = seg.map(|g| (g.title.as_str(), g.game.as_str())).unwrap_or(("Your stream title", "Just Chatting"));
    let when = if minutes == 60 { "in 1 hour".to_owned() } else if minutes % 60 == 0 { format!("in {} hours", minutes / 60) } else { format!("in {minutes} minutes") };
    bot_message(ctx, page, html! {
        div.sa-line {
            (ping(role))
            (b.name) " streams " span.sa-ts { (when) } " at " span.sa-ts { (at) }
            @if !title.is_empty() { ": " b { (text::clip(title, 140)) } }
            @if !game.is_empty() { " (" (game) ")" }
            br;
            a href=(b.url()) target="_blank" rel="noopener" { (b.url()) }
        }
    })
}

/// A new-video post as Discord shows it: the message, then the card Discord draws for the link.
fn video_post(ctx: &Ctx, page: &Page, role: Option<String>, template: &str, v: &youtube::Video) -> Markup {
    let url = v.url();
    let filled = youtube::fill_video(template, &v.author, &v.title, &url);
    let unfurls = filled.contains(&url);
    bot_message(ctx, page, html! {
        div.sa-line.sa-pre {
            (ping(role))
            @for (i, part) in filled.split(url.as_str()).enumerate() {
                @if i > 0 { a href=(url) target="_blank" rel="noopener" { (url) } }
                (part)
            }
        }
        @if unfurls {
            div.sa-embed style="--embed:#ff0033" {
                div.sa-provider { "YouTube" }
                div.sa-author { (v.author) }
                a.sa-title href=(url) target="_blank" rel="noopener" { (v.title) }
                div.sa-yt {
                    @if v.id.is_empty() {
                        div.sa-thumb {}
                    } @else {
                        img.sa-thumb src=(youtube::thumb(&v.id)) alt="" width="320" height="180" loading="lazy";
                    }
                    span.sa-play { (ui::icon(Icon::Play)) }
                }
            }
        }
    })
}

/// The Monday of this week, in the zone.
fn this_monday(tz: &TimeZone) -> Date {
    schedule::monday_of(Timestamp::now().to_zoned(tz.clone()).date())
}

/// The Monday that starts the oldest of the last twelve weeks.
fn first_monday(tz: &TimeZone) -> Date {
    let this = this_monday(tz);
    this.checked_sub(Span::new().days(7 * (WEEKS - 1))).unwrap_or(this)
}

/// Midnight at the start of a day in the zone, in epoch milliseconds.
fn day_ms(day: Date, tz: &TimeZone) -> i64 {
    day.to_zoned(tz.clone()).map(|z| z.timestamp().as_millisecond()).unwrap_or(0)
}

/// How long a stream ran: its recorded length, or so far for one that is still live.
fn stream_secs(r: &StreamRow, now: i64) -> i64 {
    if r.ended_at.is_some() { r.duration_secs } else { (now - r.started_at) / 1000 }
}

/// Hours streamed in each of the last twelve weeks, oldest first, labelled by their Monday.
fn weekly_hours(rows: &[StreamRow], tz: &TimeZone, now: i64) -> Vec<(String, i64)> {
    let first = first_monday(tz);
    (0..WEEKS)
        .map(|w| {
            let start = first.checked_add(Span::new().days(7 * w)).unwrap_or(first);
            let end = start.checked_add(Span::new().days(7)).unwrap_or(start);
            let (a, b) = (day_ms(start, tz), day_ms(end, tz));
            let secs: i64 = rows.iter().filter(|r| r.started_at >= a && r.started_at < b).map(|r| stream_secs(r, now)).sum();
            (start.strftime("%b %-d").to_string(), (secs + 1800) / 3600)
        })
        .collect()
}

/// The average of the peak viewer counts that were read, or `None` when none were.
fn avg_peak(rows: &[&StreamRow]) -> Option<i64> {
    let peaks: Vec<i64> = rows.iter().filter(|r| r.samples > 0).map(|r| r.peak).collect();
    (!peaks.is_empty()).then(|| peaks.iter().sum::<i64>() / peaks.len() as i64)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

/// "7 PM" on the hour, "7:30 PM" otherwise.
fn clock(ms: i64, tz: &TimeZone) -> String {
    match zoned(ms, tz) {
        Some(z) if z.minute() == 0 => z.strftime("%-I %p").to_string(),
        Some(z) => z.strftime("%-I:%M %p").to_string(),
        None => String::new(),
    }
}

/// The reminders as one line: what they're set to, or why they're off.
fn reminder_line(cfg: &Settings) -> String {
    let mut minutes = cfg.reminder_minutes.clone();
    minutes.sort_unstable_by(|a, b| b.cmp(a));
    if !cfg.reminders {
        "Reminders are off.".into()
    } else if cfg.reminder_channel.is_none() || minutes.is_empty() {
        "Reminders need a channel and at least one time on the Schedule tab.".into()
    } else {
        let list: Vec<String> = minutes.iter().map(|m| m.to_string()).collect();
        format!("Reminders {} minutes before each stream.", join_and(&list))
    }
}

/// Hours a week as bars, oldest first. The latest week is lilac and the rest are dimmer.
fn week_bars(series: &[(String, i64)]) -> Markup {
    let max = series.iter().map(|(_, v)| *v).max().unwrap_or(0).max(1) as f64;
    let n = series.len();
    html! {
        div.sa-bars {
            svg viewBox={ "0 0 " ((n * 10).saturating_sub(4)) " 48" } preserveAspectRatio="none" role="img" aria-label={ "Hours streamed in each of the last " (n) " weeks" } {
                @for (i, (label, v)) in series.iter().enumerate() {
                    @let h = (*v as f64 / max * 46.0).max(if *v > 0 { 2.0 } else { 0.0 });
                    rect x=(i * 10) y=(48.0 - h) width="6" height=(h) fill=(if i + 1 == n { "var(--data)" } else { "var(--data-dim)" }) {
                        title { (label) ": " (v) " h" }
                    }
                }
            }
        }
    }
}

/// The rail's "This week": the week's segments with the next one picked out, then the reminders.
fn week_block(tz: &TimeZone, has_channel: bool, week: &[schedule::Segment], cfg: &Settings, now: i64) -> Markup {
    let next = week.iter().find(|g| !g.canceled && g.starts_at >= now).map(|g| g.id.as_str());
    html! {
        div.rail-block {
            h2 { "This week" }
            @if !has_channel {
                (ui::empty(Icon::Calendar, "No Twitch channel yet", "Add your channel on the Twitch tab and its schedule shows up here."))
            } @else if week.is_empty() {
                (ui::empty(Icon::Calendar, "Nothing this week", "Segments from your Twitch schedule show up here within 30 minutes."))
            } @else {
                div.sa-week {
                    @for g in week {
                        @let what = if g.title.is_empty() { g.game.as_str() } else { g.title.as_str() };
                        div.sa-seg.on[next == Some(g.id.as_str())].off[g.canceled].past[!g.canceled && g.starts_at < now] {
                            span.day { (zoned(g.starts_at, tz).map(|z| z.strftime("%a").to_string()).unwrap_or_default()) }
                            span.what { @if g.canceled { s { (what) } } @else { (what) } }
                            span.when { @if g.canceled { "Off" } @else { (clock(g.starts_at, tz)) } }
                        }
                    }
                }
            }
            p.muted.small { (reminder_line(cfg)) }
        }
    }
}

/// The subscriptions the Twitch tab reports on, with what they do.
const EVENT_KINDS: &[(&str, &str)] = &[("stream.online", "Going live"), ("stream.offline", "Going offline"), ("channel.update", "Title and category")];

/// A subscription status in words, and how it reads: 0 working, 1 waiting, 2 broken.
fn sub_status(status: Option<&str>) -> (u8, String) {
    match status {
        Some("enabled") => (0, "Active".into()),
        Some("webhook_callback_verification_pending") => (1, "Waiting for Twitch to check the address".into()),
        Some("webhook_callback_verification_failed") => (2, "Twitch couldn't reach the bot".into()),
        Some("notification_failures_exceeded") => (2, "Twitch stopped after failed deliveries".into()),
        Some("authorization_revoked" | "user_removed" | "moderator_removed") => (2, "Revoked by Twitch".into()),
        Some(other) => (2, format!("Not working: {other}")),
        None => (2, "Not set up yet".into()),
    }
}

/// The rail's "Twitch events" in push mode: each subscription's state and the last event.
/// In polling mode, when the last check ran.
fn events_block(ctx: &Ctx, mode: Mode, bs: &[Broadcaster]) -> Markup {
    let (states, checked) = twitch::sub_states(ctx);
    html! {
        @if mode == Mode::Push && !bs.is_empty() {
            div.rail-block {
                h2 { "Twitch events" }
                @for b in bs {
                    @if bs.len() > 1 { p.small.sa-who { (b.name) } }
                    @for (kind, label) in EVENT_KINDS {
                        @let (tone, text) = sub_status(states.get(&b.user_id).and_then(|m| m.get(*kind)).map(String::as_str));
                        div.mini {
                            span.dot.live[tone == 0].waiting[tone == 1].sa-bad[tone == 2] {}
                            span.grow { span { (label) } span.muted { (text) } }
                        }
                    }
                    p.muted.small {
                        @match twitch::last_event(ctx, &b.user_id) {
                            Some((at, _)) => { "Last event " (text::ago(at)) "." }
                            None => { "No event has arrived yet." }
                        }
                        @if let Some(at) = checked { " Checked " (text::ago(at)) "." }
                    }
                }
            }
        } @else if mode == Mode::Poll && !bs.is_empty() {
            div.rail-block {
                h2 { "Twitch checks" }
                div.mini {
                    span.dot.live {}
                    span.grow {
                        span { "Every 2 minutes" }
                        span.muted {
                            @match crate::state_get(ctx, "at:poll").and_then(|v| v.parse::<i64>().ok()) {
                                Some(at) => { "Last check " (text::ago(at)) }
                                None => { "No check yet" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The rail's "Hours streamed": this week's hours, stream count and average peak, then the bars.
fn hours_block(rows: &[StreamRow], tz: &TimeZone, now: i64) -> Markup {
    let monday = day_ms(this_monday(tz), tz);
    let week: Vec<&StreamRow> = rows.iter().filter(|r| r.started_at >= monday).collect();
    let secs: i64 = week.iter().map(|r| stream_secs(r, now)).sum();
    let peak = avg_peak(&week).map(|p| format!(" · average peak {}", text::num(p))).unwrap_or_default();
    let series = weekly_hours(rows, tz, now);
    html! {
        div.rail-block {
            h2 { "Hours streamed" }
            span.big-num { ((secs + 1800) / 3600) " h" }
            p.muted.small { "This week · " (plural(week.len(), "stream", "streams")) (peak) }
            @if rows.is_empty() {
                p.muted.small { "Streams show up here once they end." }
            } @else {
                (week_bars(&series))
                span.muted.small { "Last 12 weeks" }
            }
        }
    }
}

const CSS: &str = r#"
.sa-main { min-width: 0; }
.sa-sechead { display: flex; align-items: center; justify-content: space-between; gap: 12px; flex-wrap: wrap; }
.sa-sechead h2 { font-size: 14px; }
.sa-post { display: flex; align-items: flex-start; gap: 14px; min-width: 0; color: #dbdee1; font: 15px/1.4 var(--sans); }
.sa-av { flex: none; width: 40px; height: 40px; border-radius: 50%; object-fit: cover; }
.sa-av.letter { display: grid; place-items: center; background: var(--signal); color: var(--signal-ink); font-weight: 600; font-size: 17px; text-transform: uppercase; }
.sa-post-main { display: flex; flex-direction: column; align-items: flex-start; gap: 4px; min-width: 0; flex: 1; }
.sa-head { display: flex; align-items: center; flex-wrap: wrap; gap: 8px; }
.sa-name { color: #f2f3f5; font-weight: 500; }
.sa-app { background: #5865f2; color: #fff; font-size: 10px; font-weight: 600; border-radius: 3px; padding: 1px 4px; line-height: 12px; }
.sa-time { color: #949ba4; font-size: 12px; }
.sa-line { overflow-wrap: anywhere; max-width: 100%; }
.sa-line a, a.sa-title { color: #00a8fc; }
.sa-pre { white-space: pre-wrap; }
.sa-ts { background: rgba(255, 255, 255, 0.06); border-radius: 3px; padding: 0 2px; }
.sa-provider { font-size: 12px; color: #b5bac1; }
.sa-yt { position: relative; }
.sa-play { position: absolute; inset: 0; margin: auto; width: 48px; height: 48px; border-radius: 50%; background: rgba(0, 0, 0, 0.6); color: #fff; display: grid; place-items: center; }
.sa-play .icon { width: 20px; height: 20px; }
.sa-ping { background: rgba(88, 101, 242, 0.3); color: #c9cdfb; border-radius: 3px; padding: 0 2px; }
.sa-embed { align-self: stretch; max-width: 480px; display: flex; flex-direction: column; gap: 6px; margin-top: 4px; padding: 12px 16px 16px 14px; background: #2b2d31; border-left: 4px solid var(--embed, #1e1f22); border-radius: 4px; }
.sa-author { display: flex; align-items: center; gap: 8px; font-size: 13px; font-weight: 600; color: #f2f3f5; }
.sa-author img { width: 24px; height: 24px; border-radius: 50%; object-fit: cover; }
.sa-title { font-size: 15px; font-weight: 600; color: #00a8fc; overflow-wrap: anywhere; }
.sa-fields { display: flex; flex-wrap: wrap; gap: 8px 32px; font-size: 13.5px; }
.sa-fields div { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.sa-fields b { color: #f2f3f5; font-weight: 600; }
.sa-fields span { overflow-wrap: anywhere; }
.sa-thumb { width: 100%; height: auto; margin-top: 4px; aspect-ratio: 16 / 9; object-fit: cover; border-radius: 4px; background: linear-gradient(160deg, #3a3d44, #1e1f22); display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 8px; color: #80848e; font-size: 12px; }
.sa-thumb .icon { width: 26px; height: 26px; }
.sa-foot { font-size: 12px; color: #b5bac1; }
.sa-watch { display: inline-flex; align-items: center; gap: 6px; height: 32px; padding: 0 16px; border-radius: 4px; background: #4e5058; color: #fff; font-size: 14px; font-weight: 500; }
.sa-watch .icon { width: 14px; height: 14px; }
.sa-watch:hover { background: #6d6f78; }
.sa-note { display: flex; gap: 10px; align-items: flex-start; font-size: 12.5px; color: var(--ink-3); max-width: 72ch; }
.sa-note > .icon { margin-top: 3px; flex: none; }
.sa-note p + p { margin-top: 4px; }
.sa-note code { color: var(--ink-2); }
.sa-live { display: flex; align-items: center; flex-wrap: wrap; gap: 6px 12px; font-size: 13px; color: var(--ink-2); }
.sa-live strong { font-weight: 500; color: var(--ink); }
.sa-live .muted { font-size: 12px; }
.sa-week { display: flex; flex-direction: column; gap: 2px; margin: 0 -8px; }
.sa-seg { display: grid; grid-template-columns: 34px minmax(0, 1fr) auto; align-items: center; gap: 10px; padding: 7px 8px; border-radius: var(--r-key); font-size: 13px; color: var(--ink-2); }
.sa-seg .day { font: 12px var(--mono); color: var(--ink-3); }
.sa-seg .what { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sa-seg .when { font: 12px var(--mono); white-space: nowrap; }
.sa-seg.past { color: var(--ink-3); }
.sa-seg.on { background: var(--selected); color: var(--ink); }
.sa-seg.on .day, .sa-seg.on .when { color: var(--ink); }
.sa-seg.off .what { text-decoration: line-through; color: var(--ink-3); }
.sa-seg.off .when { font-family: var(--sans); color: var(--ink-3); }
.sa-stream { display: flex; align-items: center; gap: 12px; min-width: 0; }
.sa-vt { flex: none; width: 64px; height: 36px; border-radius: 4px; object-fit: cover; background: var(--well); display: grid; place-items: center; color: var(--ink-3); }
.sa-vt .icon { width: 16px; height: 16px; }
.sa-bad { background: var(--held); }
.sa-who { font-weight: 500; margin-top: 4px; }
.sa-note.sa-warn { color: var(--ink-2); }
.sa-note.sa-warn > .icon { color: var(--waiting); }
.sa-bars svg { display: block; width: 100%; height: 52px; overflow: visible; }
"#;

/// Columns marked `.wide-only` drop on a phone so tables fit without scrolling sideways.
fn narrow_css() -> Markup {
    html! { style { "@media (max-width: 820px) { .wide-only { display: none; } }" } }
}

// ---- Twitch ----

/// The first broadcaster a server follows, for previews. A stand-in until Twitch has confirmed one.
fn sample_broadcaster(bs: &[Broadcaster], s: &Settings) -> Broadcaster {
    bs.first().cloned().unwrap_or_else(|| Broadcaster {
        login: s.twitch_logins.first().cloned().unwrap_or_else(|| "yourname".into()),
        user_id: String::new(),
        name: s.twitch_logins.first().cloned().unwrap_or_else(|| "You".into()),
        avatar: String::new(),
    })
}

async fn twitch_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<HashMap<String, String>>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let mode = twitch::mode(&ctx);
    let tz = s.tz();
    let now = now_ms();
    let bs = twitch::for_settings(&ctx, &s);
    let ids: Vec<String> = bs.iter().map(|b| b.user_id.clone()).collect();
    let live = twitch::open_streams(&ctx)?.into_iter().find(|r| ids.contains(&r.broadcaster_id));
    let recent = twitch::streams_since(&ctx, &ids, 0, 5)?;
    let last = recent.iter().find(|r| r.ended_at.is_some());
    let rows = twitch::streams_since(&ctx, &ids, day_ms(first_monday(&tz), &tz), 1000)?;
    let (_, _, week) = schedule::week(&ctx, &s);
    let missing: Vec<&String> = s.twitch_logins.iter().filter(|l| !bs.iter().any(|b| &&b.login == l)).collect();
    let sample = sample_broadcaster(&bs, &s);
    // While live, the preview is the stream that's on; otherwise the last one, with its VOD's
    // picture when Twitch has made one.
    let shown = live.as_ref().or(last);
    let preview = Sample {
        title: shown.map(|r| r.title.as_str()).unwrap_or("Your stream title"),
        game: shown.map(|r| r.game.as_str()).unwrap_or("Just Chatting"),
        started: live.as_ref().map(|r| text::ago(r.started_at)).unwrap_or_else(|| "just now".into()),
        thumb: shown.and_then(StreamRow::thumb_640),
    };
    let line = twitch::fill_live(&s.live_message, &sample, preview.title, preview.game);
    // Push mode with nothing arriving looks like it works. One Helix call says whether Twitch
    // has a stream the bot never heard about.
    let missed = match (mode, bs.first()) {
        (Mode::Push, Some(b)) => {
            let start = twitch::latest_vod_start(&ctx, &b.user_id).await;
            let recorded = start.is_some_and(|st| recent.iter().any(|r| r.broadcaster_id == b.user_id && (r.started_at - st).abs() < 15 * 60_000));
            let last = twitch::last_event(&ctx, &b.user_id).map(|e| e.0);
            start.filter(|st| twitch::missed_stream(Some(*st), last, recorded, now)).map(|st| (st, last))
        }
        _ => None,
    };
    let role = role_name(&ctx, guild, s.live_role);

    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, "Twitch"))
        (toast_if(&q, "sent", "Test alert sent"))
        div.with-rail {
            div.stack.sa-main {
                @if mode == Mode::Off {
                    div.sa-note {
                        (ui::icon(Icon::Plug))
                        div {
                            p { "Twitch isn't connected. Set " code { "TWITCH_CLIENT_ID" } " and " code { "TWITCH_CLIENT_SECRET" } " in the bot's environment and restart it. Live alerts, viewer counts and the schedule all need them." }
                            p { "Add " code { "TWITCH_EVENTSUB_SECRET" } " and an https " code { "PUBLIC_URL" } " to get alerts the moment you go live instead of within 2 minutes." }
                        }
                    }
                }
                @if let Some((start, last)) = missed {
                    div.sa-note.sa-warn role="status" {
                        (ui::icon(Icon::Plug))
                        div {
                            p {
                                "Twitch shows a stream that started " (text::ago(start)) ", but no event about it reached the bot"
                                @match last { Some(at) => { " (the last one came " (text::ago(at)) ")" } None => {} }
                                ", so no alert was posted."
                            }
                            p { "Twitch has to reach " code { (twitch::callback_url(&ctx)) } " from the internet. Check the address and that the bot is reachable there." }
                        }
                    }
                }
                @if let Some(r) = &live {
                    div.sa-live {
                        span.dot.live {}
                        span { "Live now · " strong { (r.title) } }
                        span.muted { "started " (text::ago(r.started_at)) @if r.samples > 0 { " · peak " (text::num(r.peak)) } }
                    }
                }
                section.block {
                    div.sa-sechead {
                        h2 { "What members see when you go live" }
                        @if page.can_edit() { (ui::action_button(&page.path("alerts/test"), "Send a test alert", Tone::Muted, false)) }
                    }
                    (live_post(&ctx, &page, role, &line, &sample, &preview))
                    p.muted.small {
                        @if page.can_edit() {
                            @match channel_name(&ctx, guild, s.live_channel) {
                                Some(c) => { "Posts a sample in " (c) " without pinging anyone. " }
                                None => { "Pick a channel below and save first. " }
                            }
                        }
                        (mode_line(mode))
                    }
                    @if mode != Mode::Off && !missing.is_empty() {
                        p.small { (ui::badge("Not found", Tone::Warn)) " Twitch hasn't confirmed " (missing.iter().map(|l| l.as_str()).collect::<Vec<_>>().join(", ")) " yet. Check the spelling." }
                    }
                }
                (ui::settings_form(&page, &page.path("alerts"), ui::card(html! {
                    (ui::field_text("twitch_logins", "Twitch channel", Some("The name in twitch.tv/name. Separate several with commas."), &s.twitch_logins.join(", "), "yourname"))
                    (ui::field_channel(&ctx, guild, "live_channel", "Post in", None, s.live_channel))
                    (ui::field_role(&ctx, guild, "live_role", "Ping", Some("Only this role is pinged."), s.live_role))
                    (ui::field_text("live_message", "Message", Some("{streamer}, {title}, {game} and {url} are filled in."), &s.live_message, crate::DEFAULT_LIVE_MESSAGE))
                })))
            }
            aside.rail {
                (week_block(&tz, !bs.is_empty(), &week, &s, now))
                (hours_block(&rows, &tz, now))
                (events_block(&ctx, mode, &bs))
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Stream alerts", Some(status_pill(&ctx)), body).into_response())
}

/// "https://twitch.tv/Name, other" -> ["name", "other"].
fn parse_logins(raw: &str) -> Result<Vec<String>, WebError> {
    let mut out: Vec<String> = Vec::new();
    for part in raw.split([',', ' ', '\n']).map(str::trim).filter(|p| !p.is_empty()) {
        let login = part.trim_end_matches('/').rsplit('/').next().unwrap_or(part).trim_start_matches('@').to_lowercase();
        if !(3..=25).contains(&login.len()) || !login.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(WebError::Bad(format!("\"{part}\" isn't a Twitch channel name.")));
        }
        if !out.contains(&login) {
            out.push(login);
        }
    }
    if out.len() > 5 {
        return Err(WebError::Bad("Up to 5 Twitch channels per server.".into()));
    }
    Ok(out)
}

async fn save_twitch(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let old = ctx.settings::<Settings>(guild, KEY)?;
    let logins = parse_logins(&form.string("twitch_logins"))?;
    let mode = twitch::mode(&ctx);
    if mode != Mode::Off && logins != old.twitch_logins {
        match twitch::resolve(&ctx, &logins).await {
            Ok(missing) if !missing.is_empty() => return Err(WebError::Bad(format!("Twitch has no channel called {}.", missing.join(", ")))),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "couldn't check Twitch logins"),
        }
    }
    let message = text::clip(&form.string("live_message"), 500);
    let s = Settings {
        twitch_logins: logins.clone(),
        live_channel: form.channel(&ctx, guild, "live_channel"),
        live_role: form.role(&ctx, guild, "live_role"),
        live_message: if message.is_empty() { crate::DEFAULT_LIVE_MESSAGE.into() } else { message },
        ..(*old).clone()
    };
    ctx.save_settings(guild, KEY, &s)?;
    if mode == Mode::Push && logins != old.twitch_logins {
        let ctx = ctx.clone();
        bot_core::tokio::spawn(async move {
            if let Err(e) = twitch::ensure_subscriptions(&ctx).await {
                tracing::warn!(error = %e, "couldn't update Twitch subscriptions");
            }
        });
    }
    ctx.audit(&page.user, Some(guild), KEY, "Changed Twitch alert settings");
    Ok(saved(&page.path("alerts")))
}

async fn send_test(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    if s.live_channel.is_none() {
        return Err(WebError::Bad("Pick a channel for live posts and save first.".into()));
    }
    if let Err(e) = twitch::test_alert(&ctx, &s).await {
        tracing::warn!(guild, error = %e, "test alert failed");
        return Err(WebError::Bad("Discord refused the post. Check that the bot can send messages and embeds in that channel.".into()));
    }
    ctx.audit(&page.user, Some(guild), KEY, "Sent a test live alert");
    Ok(bot_core::axum::response::Redirect::to(&page.path("alerts?sent=1")).into_response())
}

// ---- Schedule ----

async fn schedule_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let tz = s.tz();
    let mode = twitch::mode(&ctx);
    let (bs, _, week) = schedule::week(&ctx, &s);
    let ids: Vec<String> = bs.iter().map(|b| b.user_id.clone()).collect();
    let now = now_ms();
    let upcoming = schedule::segments(&ctx, &ids, now, now + 14 * 86_400_000);
    let next = upcoming.iter().find(|g| !g.canceled);
    let week_left = week.iter().filter(|g| !g.canceled && g.starts_at >= now).count();
    let mut minutes = s.reminder_minutes.clone();
    minutes.sort_unstable_by(|a, b| b.cmp(a));
    let closest = minutes.last().copied().unwrap_or(10);
    let sample = sample_broadcaster(&bs, &s);
    let role = role_name(&ctx, guild, s.reminder_role);
    let hours: Vec<(String, String)> = (0..24).map(|h| (h.to_string(), format!("{h:02}:00"))).collect();

    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, "Schedule"))
        div.with-rail {
            div.stack.sa-main {
                (ui::section("This week", Some("What /schedule posts. The bot reads your Twitch schedule every 30 minutes."), html! {
                    @if bs.is_empty() {
                        (ui::empty(Icon::Calendar, "No Twitch channel yet", "Add your channel on the Twitch tab and its schedule shows up here."))
                    } @else {
                        img src=(page.path("alerts/schedule.png")) alt="This week's stream schedule" width="960" height="884" loading="lazy"
                            style="width:100%;max-width:720px;height:auto;border-radius:10px;border:1px solid var(--edge-1);display:block";
                    }
                }))
                section.block {
                    div.sa-sechead { h2 { "What members see before a stream" } }
                    (reminder_post(&ctx, &page, role, &sample, next, closest))
                    p.muted.small {
                        (reminder_line(&s))
                        @if let Some(c) = channel_name(&ctx, guild, s.reminder_channel).filter(|_| s.reminders) { " Posted in " (c) "." }
                    }
                }
                (narrow_css())
                (ui::section("Coming up", None, html! {
                    @if upcoming.is_empty() {
                        (ui::empty(Icon::Calendar, "Nothing scheduled", if mode == Mode::Off { "Segments come from your Twitch schedule once Twitch is connected." } else { "Add segments to your schedule on Twitch. They show up here within 30 minutes." }))
                    } @else {
                        div.table-wrap { table.table {
                            thead { tr { th { "When" } th { "Title" } th.wide-only { "Category" } th {} } }
                            tbody { @for g in &upcoming {
                                tr {
                                    td style="white-space:nowrap" { (when(g.starts_at, &tz)) }
                                    td { @if g.title.is_empty() { span.muted { "Untitled" } } @else { (g.title) } }
                                    td.muted.wide-only { (g.game) }
                                    td.actions { @if g.canceled { (ui::badge("Cancelled", Tone::Muted)) } }
                                }
                            } }
                        } }
                    }
                }))
                (ui::settings_form(&page, &page.path("alerts/schedule"), html! {
                    (ui::section("Reminders", None, ui::card(html! {
                        (ui::field_toggle("reminders", "Post reminders", Some("Skipped when you're already live."), s.reminders))
                        (ui::field_channel(&ctx, guild, "reminder_channel", "Post in", None, s.reminder_channel))
                        (ui::field_role(&ctx, guild, "reminder_role", "Role to ping", None, s.reminder_role))
                        (ui::field_text("reminder_minutes", "When", Some("Minutes before a stream starts, separated by commas."), &minutes.iter().map(|m| m.to_string()).collect::<Vec<_>>().join(", "), "60, 10"))
                    })))
                    (ui::section("Weekly image", None, ui::card(html! {
                        (ui::field_toggle("weekly_post", "Post every Monday", None, s.weekly_post))
                        (ui::field_channel(&ctx, guild, "weekly_channel", "Post in", None, s.weekly_channel))
                        (ui::field_select("weekly_hour", "At", None, &hours, &s.weekly_hour.to_string()))
                        (ui::field_text("timezone", "Time zone", Some("For the image, the Monday post and this page. A name like America/Toronto."), &s.timezone, crate::DEFAULT_TIMEZONE))
                    })))
                }))
            }
            aside.rail {
                div.rail-block {
                    h2 { "Next stream" }
                    @if let Some(g) = next {
                        div.next-stream {
                            strong { (when(g.starts_at, &tz)) }
                            span { (until(g.starts_at)) }
                            @if !g.title.is_empty() { span.muted { (g.title) } }
                        }
                    } @else {
                        p.muted.small { (if mode == Mode::Off { "Segments come from your Twitch schedule once Twitch is connected." } else { "Nothing on the schedule yet." }) }
                    }
                }
                div.rail-block {
                    h2 { "Left this week" }
                    span.big-num { (week_left) }
                    p.muted.small { (reminder_line(&s)) }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Stream alerts", Some(status_pill(&ctx)), body).into_response())
}


fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// "60, 10 15" -> [60, 15, 10]: 1 minute to 1 day, at most 5.
fn parse_minutes(raw: &str) -> Result<Vec<i64>, WebError> {
    let mut out = Vec::new();
    for part in raw.split([',', ' ']).map(str::trim).filter(|p| !p.is_empty()) {
        let m: i64 = part.trim_end_matches("min").trim_end_matches('m').trim().parse().map_err(|_| WebError::Bad(format!("\"{part}\" isn't a number of minutes.")))?;
        if !(1..=1440).contains(&m) {
            return Err(WebError::Bad("Reminders can be 1 to 1440 minutes before.".into()));
        }
        if !out.contains(&m) {
            out.push(m);
        }
    }
    if out.len() > 5 {
        return Err(WebError::Bad("Up to 5 reminders per stream.".into()));
    }
    out.sort_unstable_by(|a, b| b.cmp(a));
    Ok(out)
}

async fn save_schedule(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let old = ctx.settings::<Settings>(guild, KEY)?;
    let tz = form.string("timezone");
    let tz = if tz.is_empty() { crate::DEFAULT_TIMEZONE.to_owned() } else { tz };
    if TimeZone::get(&tz).is_err() {
        return Err(WebError::Bad(format!("\"{tz}\" isn't a time zone. Use a name like America/Toronto or Europe/Paris.")));
    }
    let s = Settings {
        reminders: form.bool("reminders"),
        reminder_channel: form.channel(&ctx, guild, "reminder_channel"),
        reminder_role: form.role(&ctx, guild, "reminder_role"),
        reminder_minutes: parse_minutes(&form.string("reminder_minutes"))?,
        weekly_post: form.bool("weekly_post"),
        weekly_channel: form.channel(&ctx, guild, "weekly_channel"),
        weekly_hour: form.int_in("weekly_hour", 0, 23, 10),
        timezone: tz,
        ..(*old).clone()
    };
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed stream schedule settings");
    Ok(saved(&page.path("alerts/schedule")))
}

async fn schedule_png(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let png = schedule::week_png(&ctx, &s).await?;
    Ok(([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "private, max-age=60")], png).into_response())
}

// ---- YouTube ----

async fn youtube_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let videos = youtube::recent(&ctx, &s.youtube_ids, 10);
    let feeds = youtube::latest_per_feed(&ctx, &s.youtube_ids);
    let sample = videos.first().cloned().unwrap_or_else(|| youtube::Video {
        id: String::new(),
        title: "Your video title".into(),
        author: "Your channel".into(),
        published_ms: now_ms(),
    });
    let role = role_name(&ctx, guild, s.youtube_role);
    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, "YouTube"))
        div.with-rail {
            div.stack.sa-main {
                section.block {
                    div.sa-sechead { h2 { "What members see when you upload" } }
                    (video_post(&ctx, &page, role, &s.youtube_message, &sample))
                    p.muted.small {
                        @match channel_name(&ctx, guild, s.youtube_channel) {
                            Some(c) => { "Posted in " (c) " within 5 minutes of the upload." }
                            None => { "Pick a channel below and save to start posting." }
                        }
                    }
                }
                (ui::section("Recent videos", Some("A channel's first check only records what's there, so old videos aren't posted."), html! {
                    @if videos.is_empty() {
                        (ui::empty(Icon::Play, "No videos yet", if s.youtube_ids.is_empty() { "Add a YouTube channel below and its uploads show up here." } else { "Videos show up here after the next check." }))
                    } @else {
                        div.list { @for v in &videos {
                            div.list-item {
                                img.sa-vt src=(youtube::thumb(&v.id)) alt="" width="64" height="36" loading="lazy";
                                div.grow style="min-width:0" {
                                    a.clip style="display:block" href=(v.url()) target="_blank" rel="noopener" { (v.title) }
                                    p.muted.small { (v.author) }
                                }
                                span.meta { (text::ago(v.published_ms)) }
                            }
                        } }
                    }
                }))
                (ui::settings_form(&page, &page.path("alerts/youtube"), html! {
                    (ui::section("Settings", None, ui::card(html! {
                        (ui::field_text("youtube_ids", "YouTube channels", Some("A channel link, @handle or ID (starts with UC). Separate several with commas."), &s.youtube_ids.join(", "), "@yourchannel"))
                        (ui::field_channel(&ctx, guild, "youtube_channel", "Post in", None, s.youtube_channel))
                        (ui::field_role(&ctx, guild, "youtube_role", "Role to ping", Some("Only this role is pinged."), s.youtube_role))
                        (ui::field_textarea("youtube_message", "Message", Some("{channel}, {title} and {url} are filled in. Keep {url} so Discord shows the video."), &s.youtube_message, crate::DEFAULT_VIDEO_MESSAGE))
                    })))
                }))
            }
            aside.rail {
                div.rail-block {
                    h2 { "Channels" }
                    @if feeds.is_empty() {
                        (ui::empty(Icon::Play, "No channels yet", "Add a channel below. It's checked every 5 minutes."))
                    } @else {
                        @for (n, (_, latest)) in feeds.iter().enumerate() {
                            div.mini {
                                (ui::icon(Icon::Play))
                                span.grow {
                                    @match latest {
                                        Some(v) => { span { (v.author) } span.muted { "Last upload " (text::ago(v.published_ms)) } }
                                        None => { span { "Channel " (n + 1) } span.muted { "Waiting for the first check" } }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Stream alerts", Some(status_pill(&ctx)), body).into_response())
}


async fn save_youtube(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let old = ctx.settings::<Settings>(guild, KEY)?;
    let raw = form.string("youtube_ids");
    let parts: Vec<&str> = raw.split([',', ' ', '\n']).map(str::trim).filter(|p| !p.is_empty()).collect();
    // Checked before looking anything up: each new handle is a request to youtube.com.
    if parts.len() > 10 {
        return Err(WebError::Bad("Up to 10 YouTube channels per server.".into()));
    }
    let mut ids: Vec<String> = Vec::new();
    for part in parts {
        let id = match old.youtube_ids.iter().find(|i| i.as_str() == part) {
            Some(known) => known.clone(),
            None => youtube::resolve_input(&ctx, part).await.ok_or_else(|| WebError::Bad(format!("Couldn't find a YouTube channel for \"{part}\". Try the channel's link or its ID.")))?,
        };
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.len() > 10 {
        return Err(WebError::Bad("Up to 10 YouTube channels per server.".into()));
    }
    let message = text::clip(&form.text("youtube_message"), 500);
    let s = Settings {
        youtube_ids: ids,
        youtube_channel: form.channel(&ctx, guild, "youtube_channel"),
        youtube_role: form.role(&ctx, guild, "youtube_role"),
        youtube_message: if message.is_empty() { crate::DEFAULT_VIDEO_MESSAGE.into() } else { message },
        ..(*old).clone()
    };
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed YouTube alert settings");
    Ok(saved(&page.path("alerts/youtube")))
}

// ---- Streams ----

async fn streams_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let tz = s.tz();
    let ids: Vec<String> = twitch::for_settings(&ctx, &s).into_iter().map(|b| b.user_id).collect();
    let now = now_ms();
    // One query covers the twelve-week chart and this month.
    let since = day_ms(first_monday(&tz), &tz).min(month_start(&tz));
    let all = twitch::streams_since(&ctx, &ids, since, 1000)?;
    let month: Vec<&StreamRow> = all.iter().filter(|r| r.started_at >= month_start(&tz)).collect();
    let month_secs: i64 = month.iter().map(|r| stream_secs(r, now)).sum();
    let peak = avg_peak(&month).map(|p| format!("average peak {}", text::num(p))).unwrap_or_else(|| "no viewer counts yet".into());
    let followers: Option<i64> = ctx
        .db
        .with(|c| {
            c.query_row(
                "SELECT value FROM daily_stats WHERE guild_id = ?1 AND key = 'followers' ORDER BY day DESC LIMIT 1",
                [guild as i64],
                |r| r.get(0),
            )
            .optional()
        })
        .unwrap_or(None);
    let series = weekly_hours(&all, &tz, now);
    let table = twitch::streams_since(&ctx, &ids, 0, 30)?;

    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, "Streams"))
        div.with-rail {
            div.stack.sa-main {
                @if table.is_empty() {
                    (ui::empty(Icon::Live, "No streams yet", "Each stream is recorded when it ends: length, category, title, peak and average viewers."))
                } @else {
                    (narrow_css())
                    (ui::section("Past streams", None, html! {
                        div.table-wrap { table.table {
                            thead { tr { th { "Date" } th { "Stream" } th.num { "Length" } th.num { "Peak" } th.num.wide-only { "Average" } th.wide-only {} } }
                            tbody { @for r in &table {
                                tr {
                                    td style="white-space:nowrap" { (zoned(r.started_at, &tz).map(|z| z.strftime("%b %-d").to_string()).unwrap_or_default()) }
                                    td {
                                        div.sa-stream {
                                            @match r.thumbnail.is_empty() {
                                                false => { img.sa-vt.wide-only src=(twitch::sized(&r.thumbnail, 320, 180)) alt="" width="64" height="36" loading="lazy"; }
                                                true => { span.sa-vt.wide-only { (ui::icon(Icon::Live)) } }
                                            }
                                            div {
                                                div { @if r.title.is_empty() { "Untitled stream" } @else { (r.title) } }
                                                @if !r.game.is_empty() { div.muted.small { (r.game) } }
                                            }
                                        }
                                    }
                                    td.num style="white-space:nowrap" { @if r.ended_at.is_some() { (text::duration(r.duration_secs)) } @else { (ui::badge("Live", Tone::Ok)) } }
                                    td.num { @if r.samples > 0 { (text::num(r.peak)) } }
                                    td.num.wide-only { @if r.samples > 0 { (text::num(r.average)) } }
                                    td.actions.wide-only { @if let Some(v) = &r.vod_url { a.btn.small href=(v) target="_blank" rel="noopener" { "VOD" } } }
                                }
                            } }
                        } }
                    }))
                }
            }
            aside.rail {
                div.rail-block {
                    h2 { "This month" }
                    span.big-num { (if month_secs == 0 { "0 h".to_owned() } else { text::duration(month_secs) }) }
                    p.muted.small { (plural(month.len(), "stream", "streams")) " · " (peak) }
                    @if let Some(f) = followers {
                        div.mini { span.grow { span { "Followers" } } span.mono { (text::num(f)) } }
                    }
                }
                div.rail-block {
                    h2 { "Hours streamed" }
                    @if all.is_empty() {
                        p.muted.small { "Hours show up here once a stream ends." }
                    } @else {
                        (week_bars(&series))
                        span.muted.small { "Last 12 weeks" }
                    }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Stream alerts", Some(status_pill(&ctx)), body).into_response())
}


/// The overview's Stream block: live now or the last stream, and the next one on the schedule.
pub(crate) fn overview(ctx: &Ctx, page: &Page) -> bot_core::Overview {
    let mut o = bot_core::Overview::default();
    let Ok(s) = ctx.settings::<Settings>(page.guild_id, KEY) else { return o };
    let bs = twitch::for_settings(ctx, &s);
    if bs.is_empty() {
        return o;
    }
    let ids: Vec<String> = bs.iter().map(|b| b.user_id.clone()).collect();
    let marks = vec!["?"; ids.len()].join(", ");
    let sql = format!(
        "SELECT game, title, thumbnail, ended_at, duration_secs, peak FROM alerts_streams
         WHERE broadcaster_id IN ({marks}) ORDER BY started_at DESC LIMIT 1"
    );
    let last: Option<(String, String, String, Option<i64>, i64, i64)> = ctx
        .db
        .with(|c| {
            c.query_row(&sql, bot_core::rusqlite::params_from_iter(ids.iter()), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
                .optional()
        })
        .ok()
        .flatten();
    let now = now_ms();
    let next = schedule::segments(ctx, &ids, now, now + 14 * 86_400_000).into_iter().find(|g| !g.canceled);
    let live = last.as_ref().is_some_and(|l| l.3.is_none());
    let tz = page.user.tz.clone();
    let day_time = |ms: i64| {
        let Some(z) = zoned(ms, &tz) else { return String::new() };
        let today = Timestamp::now().to_zoned(tz.clone()).date();
        let day = match (z.date() - today).get_days() {
            0 => "Today".to_owned(),
            1 => "Tomorrow".to_owned(),
            2..=6 => z.strftime("%A").to_string(),
            _ => z.strftime("%a, %b %-d").to_string(),
        };
        format!("{day}, {}", z.strftime("%-I:%M %p"))
    };
    let mut minutes = s.reminder_minutes.clone();
    minutes.sort_unstable_by(|a, b| b.cmp(a));
    let block = html! {
        h2 {
            "Stream"
            span.status { span.dot.live[live] {} (if live { "Live" } else { "Offline" }) }
        }
        @if let Some((game, title, thumb, _, secs, peak)) = &last {
            a.stream-card href=(page.path("alerts/streams")) {
                @if thumb.is_empty() {
                    span.stream-ph { (ui::icon(Icon::Live)) }
                } @else {
                    img src=(twitch::sized(thumb, 640, 360)) alt="" loading="lazy";
                }
                div.stream-cap {
                    span { (if live { "Live now" } else { "Last stream" }) }
                    strong {
                        (if game.is_empty() { title.as_str() } else { game.as_str() })
                        @if !live && *secs > 0 { " · " (text::duration(*secs)) }
                        @if *peak > 0 { " · peak " (text::num(*peak)) }
                    }
                }
            }
        }
        @if let Some(g) = &next {
            div.next-stream {
                span.muted.small { "Next stream" }
                strong { (day_time(g.starts_at)) }
                @if !g.title.is_empty() { span { (g.title) } }
                // Reminders are skipped while live, so they aren't promised then.
                @if s.reminders && !minutes.is_empty() && !live {
                    @if let Some(ch) = channel_name(ctx, page.guild_id, s.reminder_channel) {
                        @let times: Vec<String> = minutes.iter().filter_map(|m| zoned(g.starts_at - m * 60_000, &tz)).map(|z| z.strftime("%-I:%M %p").to_string()).collect();
                        span.muted.small { "Reminders at " (join_and(&times)) " in " (ch) }
                    }
                }
            }
        } @else {
            p.muted.small { "Nothing on the schedule for the next two weeks." }
        }
    };
    o.panel.push((10, block));
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logins_parse() {
        assert_eq!(parse_logins("https://www.twitch.tv/Antho/, other_one").unwrap(), vec!["antho", "other_one"]);
        assert!(parse_logins("not a name!").is_err());
        assert!(parse_logins("").unwrap().is_empty());
    }

    #[test]
    fn minutes_parse() {
        assert_eq!(parse_minutes("10, 60 10").unwrap(), vec![60, 10]);
        assert_eq!(parse_minutes("15m").unwrap(), vec![15]);
        assert!(parse_minutes("0").is_err());
        assert!(parse_minutes("soon").is_err());
    }

    #[test]
    fn statuses_read_plainly() {
        assert_eq!(sub_status(Some("enabled")).0, 0);
        assert_eq!(sub_status(Some("webhook_callback_verification_pending")).0, 1);
        assert_eq!(sub_status(Some("notification_failures_exceeded")), (2, "Twitch stopped after failed deliveries".into()));
        assert_eq!(sub_status(None), (2, "Not set up yet".into()));
    }

    #[test]
    fn lists_join() {
        assert_eq!(join_and(&["60".into(), "10".into()]), "60 and 10");
        assert_eq!(join_and(&["60".into(), "30".into(), "10".into()]), "60, 30 and 10");
    }
}
