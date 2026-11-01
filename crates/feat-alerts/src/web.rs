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
use crate::{KEY, Settings, clips, schedule, youtube};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/alerts", get(twitch_page).post(save_twitch))
        .route("/g/{guild}/alerts/test", post(send_test))
        .route("/g/{guild}/alerts/schedule", get(schedule_page).post(save_schedule))
        .route("/g/{guild}/alerts/schedule.png", get(schedule_png))
        .route("/g/{guild}/alerts/clips", get(clips_page).post(save_clips))
        .route("/g/{guild}/alerts/youtube", get(youtube_page).post(save_youtube))
        .route("/g/{guild}/alerts/youtube/test", post(send_youtube_test))
        .route("/g/{guild}/alerts/stats", get(stats_page))
        .route("/g/{guild}/alerts/streams", get(streams_redirect))
        .route("/webhooks/twitch", post(twitch::webhook))
}

fn tabs(page: &Page, active: &str) -> Markup {
    ui::tabs(
        &[
            ("Twitch", page.path("alerts")),
            ("Schedule", page.path("alerts/schedule")),
            ("Clips", page.path("alerts/clips")),
            ("YouTube", page.path("alerts/youtube")),
            ("Stats", page.path("alerts/stats")),
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
fn until(ms: i64, now: i64) -> String {
    let mins = (ms - now) / 60_000;
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

/// The day `now` falls on in the zone.
fn today(now: i64, tz: &TimeZone) -> Date {
    zoned(now, tz).map(|z| z.date()).unwrap_or_else(|| Timestamp::now().to_zoned(tz.clone()).date())
}

fn month_start(tz: &TimeZone, now: i64) -> i64 {
    let today = today(now, tz);
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

/// Readings whose numbers count up as they come into view.
fn readings(items: &[(&str, String)]) -> Markup {
    html! {
        div.readings.sa-readings {
            @for (label, value) in items {
                div.reading { b data-count { (value) } span { (label) } }
            }
        }
    }
}

/// A Discord channel as Discord draws it: the channel's name in its bar (or `none` when there
/// isn't one), then the messages.
fn discord(name: Option<String>, none: &str, body: Markup) -> Markup {
    html! {
        div.sa-dc {
            div.sa-dc-bar {
                (ui::icon(Icon::Hash))
                @match name {
                    Some(c) => span.clip { (c.trim_start_matches('#')) },
                    None => span.none { (none) },
                }
                span.sa-dc-tag { "Preview" }
            }
            div.sa-dc-body { (body) }
        }
    }
}

/// Where posts go and who they ping, as tags: the channel, then the role in its colour.
fn route(ctx: &Ctx, guild: u64, channel: Option<u64>, role: Option<u64>) -> Markup {
    let role = role.and_then(|r| ctx.roles(guild).into_iter().find(|(id, _, _)| *id == r));
    html! {
        div.sa-route {
            @match channel_name(ctx, guild, channel) {
                Some(c) => span.sa-tag { (ui::icon(Icon::Hash)) (c.trim_start_matches('#')) },
                None => span.sa-tag.none { "No channel yet" },
            }
            @if let Some((_, name, color)) = role {
                span.sa-tag style=[(color != 0).then(|| format!("--role:#{color:06x}"))] { span.dot {} "@" (name) }
            }
        }
    }
}

/// A broadcaster's Twitch picture, or their initial when Twitch hasn't given one.
fn face(b: &Broadcaster, size: u32) -> Markup {
    html! {
        @if b.avatar.is_empty() {
            span.sa-letter style={ "width:" (size) "px;height:" (size) "px;font-size:" (size * 2 / 5) "px" } { (b.name.chars().next().unwrap_or('?')) }
        } @else {
            img.avatar src=(b.avatar) alt="" width=(size) height=(size) loading="lazy";
        }
    }
}

/// A stream's picture for a card, or the drawn placeholder.
fn stream_pic(r: Option<&StreamRow>) -> Markup {
    html! {
        @match r.and_then(StreamRow::thumb_640) {
            Some(src) => img src=(src) alt="" loading="lazy";,
            None => span.stream-ph { (ui::icon(Icon::Live)) },
        }
    }
}

/// The play mark over a video: YouTube's red one, or a glass circle for clips.
fn play(youtube: bool) -> Markup {
    html! {
        span.sa-play.glass[!youtube] aria-hidden="true" {
            svg viewBox="0 0 24 24" fill="currentColor" { path d="M9 6.8v10.4c0 .6.7 1 1.2.7l8-5.2a.8.8 0 0 0 0-1.4l-8-5.2c-.5-.3-1.2.1-1.2.7z" {} }
        }
    }
}

/// Sample pictures the preview's videos and clips wear, in turn.
const SAMPLE_PICTURES: &[&str] = &["stream.jpg", "banner.jpg", "lake.jpg"];

/// A video's picture: YouTube's, or a sample picture in the preview, whose videos aren't real.
fn video_thumb(id: &str) -> String {
    if bot_core::preview::on() {
        let n = id.bytes().map(usize::from).sum::<usize>() % SAMPLE_PICTURES.len();
        return bot_core::preview::picture(SAMPLE_PICTURES[n]);
    }
    youtube::thumb(id)
}

/// A message from the bot as Discord shows it: avatar, name with its APP tag and the time,
/// then `body`. `used` is the slash command it answers, shown above it as Discord does.
fn bot_message(ctx: &Ctx, page: &Page, used: Option<&str>, body: Markup) -> Markup {
    let bot = ctx.cache.current_user();
    let bot_name = bot.as_ref().map(|u| u.name.clone()).unwrap_or_else(|| "Bot".into());
    let bot_avatar = bot.as_ref().map(|u| bot_core::discord::avatar_url(u.id.get(), u.avatar.map(|a| a.to_string()).as_deref()));
    let time = page.user.local(now_ms()).strftime("%-I:%M %p").to_string();
    html! {
        div.sa-msg {
            @if let Some(cmd) = used {
                div.sa-used { img src=(page.user.avatar_url()) alt=""; b { (page.user.name) } " used " span.cmd { (cmd) } }
            }
            div.sa-post {
                @if let Some(url) = &bot_avatar {
                    img.sa-av src=(url) alt="" width="40" height="40" loading="lazy";
                } @else {
                    span.sa-av.mark { img src="/static/icon.svg" alt=""; }
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
}

/// The ping in front of a post, when a role is set.
fn ping(role: Option<String>) -> Markup {
    html! { @if let Some(r) = role { span.sa-ping { (r) } " " } }
}

/// The live post as Discord shows it: the ping and the message, the embed with its coral edge,
/// its fields, picture and footer, and the Watch on Twitch button.
fn live_post(ctx: &Ctx, page: &Page, role: Option<String>, line: &str, b: &Broadcaster, sample: &Sample) -> Markup {
    bot_message(ctx, page, None, html! {
        div.sa-line { (ping(role)) (line) }
        div.sa-embed style="--embed:#f0806a" {
            div.sa-author {
                @if !b.avatar.is_empty() { img src=(b.avatar) alt="" width="24" height="24" loading="lazy"; }
                (b.name)
            }
            a.sa-title href=(b.url()) target="_blank" rel="noopener" { @if sample.title.is_empty() { (b.name) " is live" } @else { (sample.title) } }
            div.sa-fields {
                @if !sample.game.is_empty() { div { b { "Playing" } span { (sample.game) } } }
                div { b { "Started" } span { span.sa-ts { (sample.started) } } }
            }
            div.sa-media {
                @match &sample.thumb {
                    Some(src) => { img src=(src) alt="" width="640" height="360" loading="lazy"; }
                    None => { div.sa-thumb { (ui::icon(Icon::Live)) span { "The stream's picture" } } }
                }
            }
            div.sa-foot { "Twitch" }
        }
        div.sa-buttons { a.sa-watch href=(b.url()) target="_blank" rel="noopener" { "Watch on Twitch" (ui::icon(Icon::External)) } }
    })
}

/// The closest reminder before a stream, as Discord shows it. Discord fills the start time in for
/// each reader; the preview gives it in the schedule's zone, like the rest of the tab.
fn reminder_post(ctx: &Ctx, page: &Page, tz: &TimeZone, role: Option<String>, b: &Broadcaster, seg: Option<&schedule::Segment>, minutes: i64) -> Markup {
    let start = seg.map(|g| g.starts_at).unwrap_or_else(|| now_ms() + minutes * 60_000);
    let at = zoned(start, tz).map(|z| z.strftime("%-I:%M %p").to_string()).unwrap_or_default();
    let (title, game) = seg.map(|g| (g.title.as_str(), g.game.as_str())).unwrap_or(("Your stream title", "Just Chatting"));
    let when = in_minutes(minutes);
    bot_message(ctx, page, None, html! {
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

/// How Discord words a relative time this far ahead: "in 10 minutes", "in 1 hour", "in 2 hours".
fn in_minutes(minutes: i64) -> String {
    match minutes {
        1 => "in a minute".into(),
        60 => "in 1 hour".into(),
        m if m % 60 == 0 => format!("in {} hours", m / 60),
        m => format!("in {m} minutes"),
    }
}

/// A new-video post as Discord shows it: the message, then the card Discord draws for the link.
/// `reward` is the claim button's label, when posts carry one.
fn video_post(ctx: &Ctx, page: &Page, role: Option<String>, template: &str, v: &youtube::Video, reward: Option<String>) -> Markup {
    let url = v.url();
    let filled = youtube::fill_video(template, &v.author, &v.title, &url);
    let unfurls = filled.contains(&url);
    bot_message(ctx, page, None, html! {
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
                div.sa-media {
                    @if v.id.is_empty() {
                        div.sa-thumb { (ui::icon(Icon::Play)) span { "The video's picture" } }
                    } @else {
                        img src=(video_thumb(&v.id)) alt="" width="320" height="180" loading="lazy";
                    }
                    (play(true))
                }
            }
        }
        @if let Some(label) = reward { div.sa-buttons { span.sa-claim { (label) } } }
    })
}

/// The Monday of this week, in the zone.
fn this_monday(tz: &TimeZone, now: i64) -> Date {
    schedule::monday_of(today(now, tz))
}

/// The Monday that starts the oldest of the last twelve weeks.
fn first_monday(tz: &TimeZone, now: i64) -> Date {
    let this = this_monday(tz, now);
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
    let first = first_monday(tz, now);
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

/// Hours a week as the rail's bars, oldest first: the latest week lilac, the rest dimmer.
fn week_bars(series: &[(String, i64)]) -> Markup {
    let bars: Vec<(String, Option<i64>)> = series.iter().map(|(label, v)| (label.clone(), Some(*v))).collect();
    ui::mini_bars(&bars, "h", &format!("Hours streamed in each of the last {} weeks", series.len()))
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
        Some("version_removed") => (2, "Twitch retired this event; the bot subscribes again within a day".into()),
        Some(s) if s.starts_with("refused") => (2, "Twitch refused the subscription".into()),
        Some(_) => (2, "Stopped by Twitch".into()),
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
                    @if bs.len() > 1 { p.small.sa-who-rail { (b.name) } }
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

/// This week's hours, stream count and average peak: the channel strip's numbers.
fn week_numbers(rows: &[StreamRow], tz: &TimeZone, now: i64) -> (i64, usize, Option<i64>) {
    let monday = day_ms(this_monday(tz, now), tz);
    let week: Vec<&StreamRow> = rows.iter().filter(|r| r.started_at >= monday).collect();
    let secs: i64 = week.iter().map(|r| stream_secs(r, now)).sum();
    ((secs + 1800) / 3600, week.len(), avg_peak(&week))
}

/// The rail's "Hours streamed": the last twelve weeks as bars.
fn hours_block(rows: &[StreamRow], tz: &TimeZone, now: i64) -> Markup {
    let series = weekly_hours(rows, tz, now);
    let total: i64 = series.iter().map(|(_, h)| h).sum();
    html! {
        div.rail-block {
            h2 { "Hours streamed" span.muted.small { "12 weeks" } }
            @if rows.is_empty() {
                p.muted.small { "Streams show up here once they end." }
            } @else {
                span.sa-big { b data-count { (text::num(total)) " h" } }
                (week_bars(&series))
                span.muted.small { "Each bar is a week, this one lit." }
            }
        }
    }
}

const CSS: &str = include_str!("style.css");

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
    let rows = twitch::streams_since(&ctx, &ids, day_ms(first_monday(&tz, now), &tz), 1000)?;
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
            let since = twitch::online_since(&ctx, &b.user_id);
            start.filter(|st| twitch::missed_stream(Some(*st), since, last, recorded, now)).map(|st| (st, last))
        }
        _ => None,
    };
    let role = role_name(&ctx, guild, s.live_role);
    let (week_h, week_n, week_peak) = week_numbers(&rows, &tz, now);
    // The channel at the top: the one that's live, else the first the server follows.
    let top = live.as_ref().and_then(|r| bs.iter().find(|b| b.user_id == r.broadcaster_id)).or(bs.first());
    let others: Vec<String> = bs.iter().filter(|b| Some(b.login.as_str()) != top.map(|t| t.login.as_str())).map(|b| b.name.clone()).collect();

    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, "Twitch"))
        (toast_if(&q, "sent", "Test alert sent"))
        div.with-rail {
            div.stack.sa-main {
                @if mode == Mode::Off {
                    div.sa-note {
                        span.need-icon { (ui::icon(Icon::Plug)) }
                        div {
                            strong { "Twitch isn't connected" }
                            p { "Set " code { "TWITCH_CLIENT_ID" } " and " code { "TWITCH_CLIENT_SECRET" } " in the bot's environment and restart it. Live alerts, viewer counts and the schedule all need them." }
                            p { "Add " code { "TWITCH_EVENTSUB_SECRET" } " and an https " code { "PUBLIC_URL" } " to get alerts the moment you go live instead of within 2 minutes." }
                        }
                    }
                }
                @if let Some((start, last)) = missed {
                    div.sa-note.warn role="status" {
                        span.need-icon { (ui::icon(Icon::Plug)) }
                        div {
                            strong { "A stream went without an alert" }
                            p {
                                "Twitch shows a stream that started " (text::ago(start)) ", but no event about it reached the bot"
                                @match last { Some(at) => { " (the last one came " (text::ago(at)) ")" } None => {} }
                                ", so no alert was posted."
                            }
                            p { "Twitch has to reach " code { (twitch::callback_url(&ctx)) } " from the internet. Check the address and that the bot is reachable there." }
                        }
                    }
                }
                section.sa-chan.live[live.is_some()] {
                    a.stream-card href=[top.map(Broadcaster::url)] target="_blank" rel="noopener" aria-label=[top.map(|b| format!("{} on Twitch", b.name))] {
                        (stream_pic(shown))
                        @if live.is_some() { span.live-tag { "LIVE" } }
                        div.stream-cap {
                            @match (&live, last) {
                                (Some(r), _) => {
                                    span { @if r.game.is_empty() { "Live now" } @else { (r.game) } }
                                    strong { @if r.title.is_empty() { "Untitled stream" } @else { (r.title) } }
                                }
                                (None, Some(r)) => {
                                    span { "Last stream · " (zoned(r.started_at, &tz).map(|z| z.strftime("%b %-d").to_string()).unwrap_or_default()) @if !r.game.is_empty() { " · " (r.game) } }
                                    strong { @if r.title.is_empty() { "Untitled stream" } @else { (r.title) } }
                                }
                                _ => {
                                    span { "No streams yet" }
                                    strong { "Each one is recorded when it ends" }
                                }
                            }
                        }
                    }
                    div.sa-chan-text {
                        @match top {
                            Some(b) => {
                                div.sa-who {
                                    (face(b, 50))
                                    div {
                                        strong { (b.name) }
                                        a href=(b.url()) target="_blank" rel="noopener" { "twitch.tv/" (b.login) }
                                    }
                                }
                            }
                            None => {
                                div.sa-who {
                                    span.sa-letter style="width:50px;height:50px" { (ui::icon(Icon::Live)) }
                                    div {
                                        strong { "No Twitch channel yet" }
                                        span.muted.small { "Add yours below. The bot looks it up on Twitch when you save." }
                                    }
                                }
                            }
                        }
                        p.sa-state {
                            @match (&live, last) {
                                (Some(r), _) => {
                                    span.dot.live {}
                                    "Live now, started " (text::ago(r.started_at))
                                    @if r.samples > 0 { " · peak " (text::num(r.peak)) " viewers" }
                                }
                                (None, Some(r)) => { span.dot {} "Offline · last streamed " (text::ago(r.started_at)) " for " (text::duration(r.duration_secs)) }
                                _ => { span.dot {} "Offline · no stream recorded yet" }
                            }
                        }
                        @if !others.is_empty() { p.sa-others { "Also follows " (join_and(&others)) "." } }
                        (route(&ctx, guild, s.live_channel, s.live_role))
                        div.sa-nums {
                            div { b data-count { (week_h) " h" } span { "this week" } }
                            div { b data-count { (week_n) } span { (if week_n == 1 { "stream" } else { "streams" }) } }
                            div { b data-count { (week_peak.map(text::num).unwrap_or_else(|| "none yet".into())) } span { "average peak" } }
                        }
                    }
                }
                // The settings and what they produce, side by side on a wide screen.
                div.compose {
                    section.block.compose-preview {
                        div.sa-sechead {
                            div {
                                h2 { "What members see when you go live" }
                                p.muted { "The post appears the moment Twitch says you're live, with your stream's picture." }
                            }
                            @if page.can_edit() { (ui::action_button(&page.path("alerts/test"), "Send a test alert", Tone::Muted, false)) }
                        }
                        (discord(channel_name(&ctx, guild, s.live_channel), "No channel picked", live_post(&ctx, &page, role, &line, &sample, &preview)))
                        p.sa-foot-note {
                            @if page.can_edit() {
                                @match channel_name(&ctx, guild, s.live_channel) {
                                    Some(c) => { "The test posts a sample in " (c) " without pinging anyone. " }
                                    None => { "Pick a channel below and save first. " }
                                }
                            }
                            (mode_line(mode))
                        }
                        @if mode != Mode::Off && !missing.is_empty() {
                            p.small { (ui::badge("Not found", Tone::Warn)) " Twitch hasn't confirmed " (missing.iter().map(|l| l.as_str()).collect::<Vec<_>>().join(", ")) " yet. Check the spelling." }
                        }
                    }
                    (ui::settings_form(&page, &page.path("alerts"), ui::section("Settings", None, ui::card(html! {
                        (ui::field_text("twitch_logins", "Twitch channel", Some("The name in twitch.tv/name. Separate several with commas."), &s.twitch_logins.join(", "), "yourname"))
                        (ui::field_channel(&ctx, guild, "live_channel", "Post in", None, s.live_channel))
                        (ui::field_role(&ctx, guild, "live_role", "Ping", Some("Only this role is pinged."), s.live_role))
                        (ui::field_text("live_message", "Message", Some("{streamer}, {title}, {game} and {url} are filled in."), &s.live_message, crate::DEFAULT_LIVE_MESSAGE))
                    }))))
                }
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
        let path = part.split(['?', '#']).next().unwrap_or(part);
        let login = path.trim_end_matches('/').rsplit('/').next().unwrap_or(path).trim_start_matches('@').to_lowercase();
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
        return Err(WebError::Bad(bot_core::discord::why_refused_any(&e)));
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
    let (bs, monday, week) = schedule::week(&ctx, &s);
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
    let today_d = today(now, &tz);
    let next_id = next.map(|g| g.id.as_str());
    // This week a day at a time, Monday first, with what's on each.
    let days: Vec<(Date, Vec<&schedule::Segment>)> = (0..7)
        .map(|d| {
            let date = monday.checked_add(Span::new().days(d)).unwrap_or(monday);
            (date, week.iter().filter(|g| zoned(g.starts_at, &tz).map(|z| z.date()) == Some(date)).collect())
        })
        .collect();
    let remind_at = |start: i64| -> Vec<String> { minutes.iter().filter_map(|m| zoned(start - m * 60_000, &tz)).map(|z| z.strftime("%-I:%M %p").to_string()).collect() };
    let reminders_ready = s.reminders && s.reminder_channel.is_some() && !minutes.is_empty();
    let weekly = s.weekly_post && s.weekly_channel.is_some();

    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, "Schedule"))
        @if bs.is_empty() {
            (ui::empty(Icon::Calendar, "No Twitch channel yet", "Add your channel on the Twitch tab and its schedule shows up here."))
        } @else if let Some(g) = next {
            @let z = zoned(g.starts_at, &tz);
            @let left = until(g.starts_at, now);
            section.sa-next {
                div.sa-cal aria-hidden="true" {
                    span.m { (z.as_ref().map(|z| z.strftime("%b").to_string()).unwrap_or_default()) }
                    b { (z.as_ref().map(|z| z.day().to_string()).unwrap_or_default()) }
                    span.d { (z.as_ref().map(|z| z.strftime("%a").to_string()).unwrap_or_default()) }
                }
                div.grow {
                    h2 { @if g.title.is_empty() { "Untitled stream" } @else { (g.title) } }
                    p { "Next stream · " (when(g.starts_at, &tz)) @if !g.game.is_empty() { " · " (g.game) } }
                    div.sa-route {
                        @if reminders_ready {
                            span.sa-tag { (ui::icon(Icon::Bell)) "Reminders at " (join_and(&remind_at(g.starts_at))) }
                        } @else {
                            span.sa-tag.none { "No reminders" }
                        }
                    }
                }
                div.sa-left {
                    @if left == "now" { b { "Now" } span { "starting" } } @else { b data-count { (left.trim_start_matches("in ")) } span { "to go" } }
                }
            }
        } @else {
            (ui::empty(Icon::Calendar, "Nothing scheduled", if mode == Mode::Off { "Segments come from your Twitch schedule once Twitch is connected." } else { "Add segments to your schedule on Twitch. They show up here within 30 minutes." }))
        }
        @if !bs.is_empty() {
            section.block {
                div.sa-sechead {
                    div {
                        h2 { "This week" }
                        p.muted { "From your Twitch schedule, read every 30 minutes. Times in " (s.timezone) "." }
                    }
                }
                div.sa-days {
                    @for (i, (date, segs)) in days.iter().enumerate() {
                        @let is_today = *date == today_d;
                        div.sa-day.today[is_today] style={ "--i:" (i) } {
                            div.sa-day-h {
                                span { (date.strftime("%a")) @if is_today { " · " span.today-tag { "today" } } }
                                b { (date.day()) }
                            }
                            @for g in segs {
                                @let what = if g.title.is_empty() { g.game.as_str() } else { g.title.as_str() };
                                div.sa-slot.next[next_id == Some(g.id.as_str())].past[!g.canceled && g.starts_at < now].off[g.canceled] {
                                    span.t { @if g.canceled { "Cancelled" } @else { (clock(g.starts_at, &tz)) } }
                                    span.w { (what) }
                                    @if !g.title.is_empty() && !g.game.is_empty() { span.g { (g.game) } }
                                }
                            }
                            @if segs.is_empty() { span.sa-none { "No stream" } }
                        }
                    }
                }
            }
        }
        div.with-rail {
            div.stack.sa-main {
                (ui::section("Coming up", Some("The next two weeks."), html! {
                    @if upcoming.is_empty() {
                        (ui::empty(Icon::Calendar, "Nothing scheduled", if mode == Mode::Off { "Segments come from your Twitch schedule once Twitch is connected." } else { "Add segments to your schedule on Twitch. They show up here within 30 minutes." }))
                    } @else {
                        div.sa-rows { @for g in &upcoming {
                            @let z = zoned(g.starts_at, &tz);
                            div.sa-row.off[g.canceled] {
                                div.sa-date {
                                    span { (z.as_ref().map(|z| z.strftime("%b").to_string()).unwrap_or_default()) }
                                    b { (z.as_ref().map(|z| z.day().to_string()).unwrap_or_default()) }
                                }
                                div.grow {
                                    strong { @if g.title.is_empty() { "Untitled" } @else { (g.title) } }
                                    span { @if g.game.is_empty() { "No category" } @else { (g.game) } }
                                }
                                @if g.canceled {
                                    (ui::badge("Cancelled", Tone::Muted))
                                } @else {
                                    div.sa-at {
                                        b { (clock(g.starts_at, &tz)) }
                                        span { (z.as_ref().map(|z| z.strftime("%a").to_string()).unwrap_or_default()) " · " (until(g.starts_at, now)) }
                                    }
                                }
                            }
                        } }
                    }
                }))
                (ui::settings_form(&page, &page.path("alerts/schedule"), html! {
                    // Each group of settings beside the post it shapes.
                    div.compose {
                        (ui::section("Reminders", None, ui::card(html! {
                            (ui::field_toggle("reminders", "Post reminders", Some("Skipped when you're already live."), s.reminders))
                            (ui::field_channel(&ctx, guild, "reminder_channel", "Post in", None, s.reminder_channel))
                            (ui::field_role(&ctx, guild, "reminder_role", "Role to ping", None, s.reminder_role))
                            (ui::field_text("reminder_minutes", "When", Some("Minutes before a stream starts, separated by commas."), &minutes.iter().map(|m| m.to_string()).collect::<Vec<_>>().join(", "), "60, 10"))
                        })))
                        section.block.compose-preview {
                            div.sa-sechead {
                                div {
                                    h2 { "What members see before a stream" }
                                    p.muted { "The closest reminder. Discord shows each reader the time in their own zone." }
                                }
                            }
                            (discord(channel_name(&ctx, guild, s.reminder_channel), "No channel picked", reminder_post(&ctx, &page, &tz, role, &sample, next, closest)))
                            p.sa-foot-note { (reminder_line(&s)) }
                        }
                    }
                    div.compose {
                        (ui::section("Weekly image", None, ui::card(html! {
                            (ui::field_toggle("weekly_post", "Post every Monday", None, s.weekly_post))
                            (ui::field_channel(&ctx, guild, "weekly_channel", "Post in", None, s.weekly_channel))
                            (ui::field_select("weekly_hour", "At", None, &hours, &s.weekly_hour.to_string()))
                            (ui::field_text("timezone", "Time zone", Some("For the image, the Monday post and this page. A name like America/Toronto."), &s.timezone, crate::DEFAULT_TIMEZONE))
                        })))
                        @if !bs.is_empty() {
                            section.block.compose-preview {
                                div.sa-sechead {
                                    div {
                                        h2 { "What /schedule posts" }
                                        p.muted { @if weekly { "The same picture goes up every Monday." } @else { "Members can ask for it any time. The Monday post is off." } }
                                    }
                                }
                                (discord(if weekly { channel_name(&ctx, guild, s.weekly_channel) } else { None }, "Wherever someone uses it", bot_message(&ctx, &page, (!weekly).then_some("/schedule"), html! {
                                    div.sa-media.sheet {
                                        img src=(page.path("alerts/schedule.png")) alt="This week's stream schedule" width="960" height="884" loading="lazy";
                                    }
                                })))
                            }
                        }
                    }
                }))
            }
            aside.rail {
                div.rail-block {
                    h2 { "Left this week" }
                    span.sa-big { b data-count { (week_left) } }
                    p.muted.small { (if week_left == 1 { "stream still to come" } else { "streams still to come" }) }
                }
                div.rail-block {
                    h2 { "Reminders" (ui::status(if reminders_ready { ui::Dot::Live } else { ui::Dot::Off }, if reminders_ready { "On" } else { "Off" })) }
                    @if !minutes.is_empty() && s.reminders {
                        div.sa-route { @for m in &minutes { span.sa-tag { (ui::icon(Icon::Clock)) (m) " min before" } } }
                    }
                    (route(&ctx, guild, s.reminder_channel, s.reminder_role))
                }
                div.rail-block {
                    h2 { "Monday post" (ui::status(if weekly { ui::Dot::Live } else { ui::Dot::Off }, if weekly { "On" } else { "Off" })) }
                    p.muted.small {
                        @if weekly { "Mondays at " (format!("{:02}:00", s.weekly_hour)) " in " (channel_name(&ctx, guild, s.weekly_channel).unwrap_or_default()) ", if anything is scheduled." }
                        @else { "Turn it on below to post the week's picture every Monday." }
                    }
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

// ---- Clips ----

/// A clip post as Discord shows it: the line with the link, then the card Discord draws for it.
fn clip_post(ctx: &Ctx, page: &Page, b: &Broadcaster, c: &clips::Clip) -> Markup {
    let text = clips::headline(c);
    bot_message(ctx, page, None, html! {
        div.sa-line { (text) br; a href=(c.url) target="_blank" rel="noopener" { (c.url) } }
        div.sa-embed style="--embed:#9146ff" {
            div.sa-provider { "Twitch" }
            a.sa-title href=(c.url) target="_blank" rel="noopener" { (c.title) " - " (b.name) }
            div.sa-media {
                @if c.thumbnail.is_empty() {
                    div.sa-thumb { (ui::icon(Icon::Play)) span { "The clip's picture" } }
                } @else {
                    img src=(c.thumbnail) alt="" width="480" height="272" loading="lazy";
                }
                (play(false))
            }
        }
    })
}

/// How many cells are left empty in the gallery's last row of three, with the first tile
/// taking four when there are at least three.
fn gallery_gap(n: usize) -> usize {
    let cells = if n >= 3 { n + 3 } else { n };
    (3 - cells % 3) % 3
}

/// The tile that fills the rest of the gallery's last row: a link to everything on the site.
fn more_tile(n: usize, href: &str, label: &str, host: &str) -> Markup {
    let gap = gallery_gap(n);
    html! {
        @if gap > 0 {
            a.sa-more href=(href) target="_blank" rel="noopener" style={ "grid-column:span " (gap) ";--i:" (n) } {
                span.need-icon { (ui::icon(Icon::External)) }
                strong { (label) }
                span { (host) }
            }
        }
    }
}

/// Clips as a gallery: the newest large when there are enough to fill around it.
fn clip_gallery(list: &[clips::Clip], channel: Option<&Broadcaster>) -> Markup {
    let big = list.len() >= 3;
    html! {
        div.sa-gallery {
            @for (i, c) in list.iter().enumerate() {
                a.stream-card.sa-tile.big[big && i == 0] href=(c.url) target="_blank" rel="noopener" style={ "--i:" (i) } {
                    @if c.thumbnail.is_empty() {
                        span.stream-ph { (ui::icon(Icon::Play)) }
                    } @else {
                        img src=(c.thumbnail) alt="" loading="lazy";
                    }
                    span.sa-glass.l { (text::num(c.views)) (if c.views == 1 { " view" } else { " views" }) }
                    (play(false))
                    div.stream-cap {
                        span { @if !c.creator.is_empty() { "Clipped by " (c.creator) " · " } (text::ago(c.created_at)) }
                        strong { (if c.title.is_empty() { "Untitled clip" } else { c.title.as_str() }) }
                    }
                }
            }
            @if let Some(b) = channel {
                (more_tile(list.len(), &format!("{}/clips", b.url()), "Every clip on Twitch", &format!("twitch.tv/{}/clips", b.login)))
            }
        }
    }
}

async fn clips_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let mode = twitch::mode(&ctx);
    let bs = twitch::for_settings(&ctx, &s);
    let ids: Vec<String> = bs.iter().map(|b| b.user_id.clone()).collect();
    let list = clips::recent(&ctx, &ids, 12);
    let now = now_ms();
    let month = clips::count_since(&ctx, &ids, now - 30 * 86_400_000);
    let checked = ids.iter().filter_map(|id| clips::last_check(&ctx, id)).max();
    let b = sample_broadcaster(&bs, &s);
    let sample = list.first().cloned().unwrap_or_else(|| clips::Clip {
        id: String::new(),
        title: "Your clip's title".into(),
        creator: "a viewer".into(),
        url: format!("https://clips.twitch.tv/{}", b.login),
        thumbnail: String::new(),
        views: 0,
        created_at: now,
    });
    let posting = s.clips && s.clips_channel.is_some();
    // Who made the recent clips, most first.
    let mut clippers: Vec<(String, usize)> = Vec::new();
    for c in list.iter().filter(|c| !c.creator.is_empty()) {
        match clippers.iter_mut().find(|(n, _)| *n == c.creator) {
            Some((_, n)) => *n += 1,
            None => clippers.push((c.creator.clone(), 1)),
        }
    }
    clippers.sort_by_key(|c| std::cmp::Reverse(c.1));
    let views: i64 = list.iter().map(|c| c.views).sum();

    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, "Clips"))
        @if mode == Mode::Off {
            div.sa-note {
                span.need-icon { (ui::icon(Icon::Plug)) }
                div {
                    strong { "Twitch isn't connected" }
                    p { "Clips can't be read until it is. Set " code { "TWITCH_CLIENT_ID" } " and " code { "TWITCH_CLIENT_SECRET" } " in the bot's environment and restart it." }
                }
            }
        }
        section.block {
            div.sa-sechead {
                div {
                    h2 { "Recent clips" }
                    p.muted { "Clips made after posting was turned on. Older ones are never posted." }
                }
            }
            @if list.is_empty() {
                (ui::empty(Icon::Play, "No clips yet", if bs.is_empty() { "Add your Twitch channel on the Twitch tab, then turn clips on below." } else if !posting { "Turn clips on below. New clips show up here and in the channel you pick." } else { "New clips show up here within 15 minutes." }))
            } @else {
                (clip_gallery(&list, bs.first()))
            }
        }
        div.with-rail {
            div.stack.sa-main {
                div.compose {
                    section.block.compose-preview {
                        div.sa-sechead {
                            div {
                                h2 { "What members see for a new clip" }
                                p.muted { "Discord turns the link into a player members can watch in the channel." }
                            }
                        }
                        (discord(channel_name(&ctx, guild, s.clips_channel), "No channel picked", clip_post(&ctx, &page, &b, &sample)))
                        p.sa-foot-note {
                            @match (posting, channel_name(&ctx, guild, s.clips_channel)) {
                                (true, Some(c)) => { "Posted in " (c) " within 15 minutes of Twitch listing the clip. Nobody is pinged." }
                                (false, Some(_)) if !s.clips => { "Clip posts are off." }
                                _ => { "Pick a channel below and save to start posting." }
                            }
                        }
                    }
                    (ui::settings_form(&page, &page.path("alerts/clips"), html! {
                        (ui::section("Settings", None, ui::card(html! {
                            (ui::field_toggle("clips", "Post new clips", Some("Clips of the channels on the Twitch tab, made by anyone."), s.clips))
                            (ui::field_channel(&ctx, guild, "clips_channel", "Post in", None, s.clips_channel))
                        })))
                    }))
                }
            }
            aside.rail {
                div.rail-block {
                    h2 { "Clips" (ui::status(if posting { ui::Dot::Live } else { ui::Dot::Off }, if posting { "Posting" } else { "Off" })) }
                    div.rail-nums {
                        div { b data-count { (text::num(month)) } span { "in 30 days" } }
                        div { b data-count { (text::num(views)) } span { "views on recent clips" } }
                    }
                    p.muted.small {
                        @if !posting { "Twitch is checked every 15 minutes while posting is on." }
                        @else {
                            "Checked every 15 minutes."
                            @match checked { Some(at) => { " Last check " (text::ago(at)) "." } None => { " The first check marks where posting starts." } }
                        }
                    }
                }
                @if !clippers.is_empty() {
                    div.rail-block {
                        h2 { "Who clips" }
                        @for (name, n) in clippers.iter().take(5) {
                            div.mini {
                                span.sa-letter style="width:30px;height:30px;font-size:12px" { (name.chars().next().unwrap_or('?')) }
                                span.grow { span { (name) } span.muted { (plural(*n, "clip", "clips")) } }
                            }
                        }
                    }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Stream alerts", Some(status_pill(&ctx)), body).into_response())
}

async fn save_clips(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let old = ctx.settings::<Settings>(guild, KEY)?;
    let s = Settings { clips: form.bool("clips"), clips_channel: form.channel(&ctx, guild, "clips_channel"), ..(*old).clone() };
    if s.clips && s.clips_channel.is_none() {
        return Err(WebError::Bad("Pick a channel for clips.".into()));
    }
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, if s.clips { "Changed Twitch clip posts" } else { "Turned off Twitch clip posts" });
    Ok(saved(&page.path("alerts/clips")))
}

// ---- YouTube ----

async fn youtube_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<HashMap<String, String>>) -> WebResult {
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
    let levels_on = ctx.enabled(guild, feat_levels::KEY);
    let rewards = youtube::rewards_on(&ctx, guild, &s);
    let claims = youtube::claims(&ctx, guild, &videos.iter().map(|v| v.id.clone()).collect::<Vec<_>>());
    let (claimed, xp_given) = youtube::claim_totals(&ctx, guild, now_ms() - 30 * 86_400_000);
    let big = videos.len() >= 3;
    // The newest upload's claimers, for the rail.
    let newest_claims: Vec<u64> = videos.first().and_then(|v| claims.get(&v.id)).cloned().unwrap_or_default();
    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, "YouTube"))
        (toast_if(&q, "sent", "Test post sent"))
        section.block {
            div.sa-sechead {
                div {
                    h2 { "Recent uploads" }
                    p.muted { "A channel's first check only records what's there, so old videos aren't posted." }
                }
            }
            @if videos.is_empty() {
                (ui::empty(Icon::Play, "No videos yet", if s.youtube_ids.is_empty() { "Add a YouTube channel below and its uploads show up here." } else { "Videos show up here after the next check." }))
            } @else {
                div.sa-gallery {
                    @for (i, v) in videos.iter().enumerate() {
                        @let who = claims.get(&v.id).map(Vec::as_slice).unwrap_or_default();
                        a.stream-card.sa-tile.big[big && i == 0] href=(v.url()) target="_blank" rel="noopener" style={ "--i:" (i) } {
                            img src=(video_thumb(&v.id)) alt="" loading="lazy";
                            @if !who.is_empty() {
                                span.sa-glass.r data-tip={ "Claimed the reward: " (who.len()) } {
                                    (ui::avatars(&who.iter().map(|u| ctx.member_avatar(guild, *u)).collect::<Vec<_>>(), if big && i == 0 { 5 } else { 3 }))
                                    (who.len()) " claimed"
                                }
                            }
                            (play(true))
                            div.stream-cap {
                                span { (v.author) " · " (text::ago(v.published_ms)) }
                                strong { (v.title) }
                            }
                        }
                    }
                    @if let Some(id) = s.youtube_ids.first().filter(|id| youtube::is_channel_id(id)) {
                        (more_tile(videos.len(), &format!("https://www.youtube.com/channel/{id}/videos"), "Every upload on YouTube", "youtube.com"))
                    }
                }
            }
        }
        div.with-rail {
            div.stack.sa-main {
                div.compose {
                    section.block.compose-preview {
                        div.sa-sechead {
                            div {
                                h2 { "What members see when you upload" }
                                p.muted { "Discord turns the link into the video's card." }
                            }
                            @if page.can_edit() && !videos.is_empty() { (ui::action_button(&page.path("alerts/youtube/test"), "Send a test post", Tone::Muted, false)) }
                        }
                        (discord(channel_name(&ctx, guild, s.youtube_channel), "No channel picked", video_post(&ctx, &page, role, &s.youtube_message, &sample, rewards.then(|| format!("Claim {} XP", text::num(s.reward_xp))))))
                        p.sa-foot-note {
                            @match channel_name(&ctx, guild, s.youtube_channel) {
                                Some(c) => {
                                    "Posted in " (c) " within 5 minutes of the upload."
                                    @if page.can_edit() && !videos.is_empty() { " The test posts your newest video there without pinging anyone." }
                                }
                                None => { "Pick a channel below and save to start posting." }
                            }
                            @if rewards {
                                " Each member can claim " (text::num(s.reward_xp)) " XP once per video, for " (plural(s.reward_hours as usize, "hour", "hours")) " after the post."
                            }
                        }
                        @if s.reward && !levels_on {
                            p.small { (ui::badge("Not added", Tone::Warn)) " XP and levels is off on this server, so posts don't carry the claim button." }
                        }
                    }
                    (ui::settings_form(&page, &page.path("alerts/youtube"), html! {
                        (ui::section("Settings", None, ui::card(html! {
                            (ui::field_text("youtube_ids", "YouTube channels", Some("A channel link, @handle or ID (starts with UC). Separate several with commas."), &s.youtube_ids.join(", "), "@yourchannel"))
                            (ui::field_channel(&ctx, guild, "youtube_channel", "Post in", None, s.youtube_channel))
                            (ui::field_role(&ctx, guild, "youtube_role", "Role to ping", Some("Only this role is pinged."), s.youtube_role))
                            (ui::field_textarea("youtube_message", "Message", Some("{channel}, {title} and {url} are filled in. Keep {url} so Discord shows the video."), &s.youtube_message, crate::DEFAULT_VIDEO_MESSAGE))
                        })))
                        (ui::section("Upload rewards", Some("A button on each new video post that gives XP through XP and levels."), ui::card(html! {
                            (ui::field_toggle("reward", "Add a claim button", Some("Once per member per video."), s.reward))
                            (ui::field_number("reward_xp", "XP", None, s.reward_xp, 1, 10_000, Some("XP")))
                            (ui::field_number("reward_hours", "Claim window", Some("How long after the post the button pays out."), s.reward_hours, 1, 168, Some("hours")))
                        })))
                    }))
                }
            }
            aside.rail {
                div.rail-block {
                    h2 { "Channels" }
                    @if feeds.is_empty() {
                        (ui::empty(Icon::Play, "No channels yet", "Add a channel below. It's checked every 5 minutes."))
                    } @else {
                        @for (n, (_, latest)) in feeds.iter().enumerate() {
                            div.mini {
                                span.sa-chip-icon.yt { (ui::icon(Icon::Play)) }
                                span.grow {
                                    @match latest {
                                        Some(v) => { span { (v.author) } span.muted { "Last upload " (text::ago(v.published_ms)) } }
                                        None => { span { "Channel " (n + 1) } span.muted { "Waiting for the first check" } }
                                    }
                                }
                            }
                        }
                        (route(&ctx, guild, s.youtube_channel, s.youtube_role))
                    }
                }
                @if s.reward || claimed > 0 {
                    div.rail-block {
                        h2 { "Rewards" (ui::status(if rewards { ui::Dot::Live } else { ui::Dot::Off }, if rewards { "On" } else { "Off" })) }
                        div.rail-nums {
                            div { b data-count { (text::num(claimed)) } span { "claims in 30 days" } }
                            div { b data-count { (text::num(xp_given)) } span { "XP given" } }
                        }
                    }
                }
                @if !newest_claims.is_empty() {
                    div.rail-block {
                        h2 { "Claimed the newest" span.muted.small { (newest_claims.len()) } }
                        div.sa-claimers {
                            @for u in newest_claims.iter().take(6) {
                                div.mini {
                                    img.avatar src=(ctx.member_avatar(guild, *u)) alt="" width="30" height="30" loading="lazy";
                                    span.grow { span { (ctx.display_name(guild, *u)) } span.muted { "+" (text::num(s.reward_xp)) " XP" } }
                                }
                            }
                        }
                        @if newest_claims.len() > 6 { p.muted.small { "and " (newest_claims.len() - 6) " more" } }
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
        reward: form.bool("reward"),
        reward_xp: form.int_in("reward_xp", 1, 10_000, old.reward_xp),
        reward_hours: form.int_in("reward_hours", 1, 168, old.reward_hours),
        ..(*old).clone()
    };
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed YouTube alert settings");
    Ok(saved(&page.path("alerts/youtube")))
}

async fn send_youtube_test(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let Some(channel) = s.youtube_channel else {
        return Err(WebError::Bad("Pick a channel for new videos and save first.".into()));
    };
    let Some(v) = youtube::recent(&ctx, &s.youtube_ids, 1).into_iter().next() else {
        return Err(WebError::Bad("There's no video to post yet. Uploads show up after the first check.".into()));
    };
    if let Err(e) = youtube::test_post(&ctx, guild, channel, &s, &v).await {
        tracing::warn!(guild, error = %e, "YouTube test post failed");
        return Err(WebError::Bad(bot_core::discord::why_refused_any(&e)));
    }
    ctx.audit(&page.user, Some(guild), KEY, "Sent a test YouTube post");
    Ok(bot_core::axum::response::Redirect::to(&page.path("alerts/youtube?sent=1")).into_response())
}

// ---- Stats ----

/// Average viewers across streams, weighted by how often each was read: `None` when no stream
/// had a viewer count. Uses each stream's exact sum of counts, so nothing is lost to rounding.
fn avg_viewers<'a>(rows: impl IntoIterator<Item = &'a StreamRow>) -> Option<i64> {
    let (sum, samples) = rows.into_iter().filter(|r| r.samples > 0).fold((0, 0), |(s, n), r| (s + r.viewer_sum, n + r.samples));
    (samples > 0).then(|| (sum + samples / 2) / samples)
}

/// Average viewers in each of the last twelve weeks, oldest first, labelled by their Monday.
/// A week without a viewer count is `None`, drawn blank rather than as a drop to 0.
fn weekly_viewers(rows: &[StreamRow], tz: &TimeZone, now: i64) -> Vec<(String, Option<i64>)> {
    let first = first_monday(tz, now);
    (0..WEEKS)
        .map(|w| {
            let start = first.checked_add(Span::new().days(7 * w)).unwrap_or(first);
            let end = start.checked_add(Span::new().days(7)).unwrap_or(start);
            let (a, b) = (day_ms(start, tz), day_ms(end, tz));
            let avg = avg_viewers(rows.iter().filter(|r| r.started_at >= a && r.started_at < b));
            (start.strftime("%b %-d").to_string(), avg)
        })
        .collect()
}

/// Follower totals by day (`YYYY-MM-DD`, total), oldest first: what the bot read once a day.
fn follower_days(ctx: &Ctx, guild: u64, since_day: &str) -> Vec<(String, i64)> {
    ctx.db
        .with(|c| {
            let mut s = c.prepare_cached("SELECT day, value FROM daily_stats WHERE guild_id = ?1 AND key = 'followers' AND day >= ?2 ORDER BY day")?;
            s.query_map(bot_core::rusqlite::params![guild as i64, since_day], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
        })
        .unwrap_or_default()
}

/// What the followers reading says: gained across the readings when there are two or more,
/// the total when there's one.
fn followers_reading(days: &[(String, i64)]) -> Option<(&'static str, String)> {
    match days {
        [] => None,
        [(_, only)] => Some(("followers", text::num(*only))),
        [(_, first), .., (_, last)] => {
            let gained = last - first;
            Some(("followers in 30 days", if gained > 0 { format!("+{}", text::num(gained)) } else { text::num(gained) }))
        }
    }
}

/// Why there's no follower total to show yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoFollowers {
    /// Twitch isn't set up on the bot.
    TwitchOff,
    /// Twitch refused the app token and no broadcaster of this server signed in.
    Connect,
    /// The daily read hasn't stored one yet.
    Soon,
}

fn no_followers(mode: Mode, app_refused: bool, signed_in: bool) -> NoFollowers {
    match mode {
        Mode::Off => NoFollowers::TwitchOff,
        _ if app_refused && !signed_in => NoFollowers::Connect,
        _ => NoFollowers::Soon,
    }
}

/// The rail's "Followers": the latest total, or why there isn't one. `reviews_link` is the
/// Account reviews settings page, for people who can open it.
fn followers_block(days: &[(String, i64)], why: NoFollowers, reviews_link: Option<&str>) -> Markup {
    html! {
        div.rail-block {
            h2 { "Followers" }
            @if let Some((day, total)) = days.last() {
                span.sa-big { b data-count { (text::num(*total)) } }
                p.muted.small { "Read once a day. Last read " (day.parse::<Date>().map(|d| d.strftime("%b %-d").to_string()).unwrap_or_default()) "." }
            } @else {
                @match why {
                    NoFollowers::TwitchOff => p.muted.small { "Followers are read once a day once Twitch is connected." },
                    NoFollowers::Connect => p.muted.small.sa-linked {
                        "Connect Twitch on "
                        @if let Some(href) = reviews_link { a href=(href) { "Account reviews > Settings" } } @else { "Account reviews > Settings" }
                        " to see followers."
                    },
                    NoFollowers::Soon => p.muted.small { "The first count shows up within a day." },
                }
            }
        }
    }
}

/// Streams per category, most first. A stream that changed category counts for each.
fn top_categories(rows: &[StreamRow]) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for r in rows {
        let mut games: Vec<&str> = r.games.lines().filter(|g| !g.is_empty()).collect();
        if games.is_empty() && !r.game.is_empty() {
            games.push(&r.game);
        }
        for g in games {
            match out.iter_mut().find(|(n, _)| n == g) {
                Some((_, n)) => *n += 1,
                None => out.push((g.to_owned(), 1)),
            }
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

async fn streams_redirect(Path(guild): Path<u64>) -> bot_core::axum::response::Redirect {
    bot_core::axum::response::Redirect::permanent(&format!("/g/{guild}/alerts/stats"))
}

async fn stats_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let tz = s.tz();
    let mode = twitch::mode(&ctx);
    let ids: Vec<String> = twitch::for_settings(&ctx, &s).into_iter().map(|b| b.user_id).collect();
    let now = now_ms();
    let month_from = month_start(&tz, now);
    // One query covers the twelve-week charts and this month.
    let all = twitch::streams_since(&ctx, &ids, day_ms(first_monday(&tz, now), &tz).min(month_from), 1000)?;
    let month: Vec<&StreamRow> = all.iter().filter(|r| r.started_at >= month_from).collect();
    let month_secs: i64 = month.iter().map(|r| stream_secs(r, now)).sum();
    let since_day = Timestamp::from_millisecond(now - 30 * 86_400_000).map(|t| t.to_zoned(TimeZone::UTC).date().to_string()).unwrap_or_default();
    let followers = follower_days(&ctx, guild, &since_day);
    let no_followers = no_followers(mode, crate::state_get(&ctx, "followers_unsupported").is_some(), ids.iter().any(|id| feat_reviews::twitch::signed_in_as(&ctx, id)));
    let followers_value = match followers_reading(&followers) {
        Some(r) => r,
        None if no_followers == NoFollowers::Connect => ("followers in 30 days", "not tracked".into()),
        None => ("followers in 30 days", "none yet".into()),
    };
    let reviews_link = (ctx.enabled(guild, feat_reviews::KEY) && ctx.level(&page.user, guild, feat_reviews::KEY) >= Level::View).then(|| page.path("reviews/settings"));
    let viewers = weekly_viewers(&all, &tz, now);
    let table = twitch::streams_since(&ctx, &ids, 0, 30)?;
    let top_peak = table.iter().filter(|r| r.samples > 0).map(|r| r.peak).max().unwrap_or(0).max(1);
    // Followers gained each day, from the daily totals. Drawn as an area ending today when the
    // last total was read today.
    let gains: Vec<(String, i64)> = followers
        .windows(2)
        .map(|w| (w[1].0.parse::<Date>().map(|d| d.strftime("%b %-d").to_string()).unwrap_or_default(), w[1].1 - w[0].1))
        .collect();
    let today_utc = Timestamp::from_millisecond(now).map(|t| t.to_zoned(TimeZone::UTC).date().to_string()).unwrap_or_default();
    let gains_end_today = followers.last().is_some_and(|(d, _)| *d == today_utc);
    let categories = top_categories(&all);
    let cat_max = categories.first().map(|c| c.1).unwrap_or(1).max(1);

    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, "Stats"))
        div.with-rail {
            div.stack.sa-main {
                (readings(&[
                    ("streams this month", month.len().to_string()),
                    ("hours this month", format!("{} h", (month_secs + 1800) / 3600)),
                    ("average viewers", avg_viewers(month.iter().copied()).map(text::num).unwrap_or_else(|| "none yet".into())),
                    followers_value,
                ]))
                // The two charts side by side once there's room (style.css, .sa-pair).
                div.sa-pair {
                    section.block.sa-chart {
                        div.sa-sechead {
                            div {
                                h2 { "Average viewers" }
                                p.muted { "Each week, from counts read every few minutes while you're live." }
                            }
                        }
                        @if viewers.iter().all(|(_, v)| v.is_none()) {
                            (ui::empty(Icon::Chart, "No viewer counts yet", "The bot reads viewers every few minutes while you're live, and the chart fills in week by week."))
                        } @else {
                            (ui::column_bars(&viewers, "average viewers", &format!("Average viewers in each of the last {} weeks", viewers.len())))
                        }
                    }
                    @if gains.len() >= 2 {
                        section.block {
                            div.sa-sechead {
                                div {
                                    h2 { "Followers gained" }
                                    p.muted { "Each day, from the total Twitch reports once a day." }
                                }
                            }
                            @if gains_end_today {
                                (ui::area_chart(&gains, "followers"))
                            } @else {
                                (ui::column_bars(&gains.iter().map(|(l, v)| (l.clone(), Some(*v))).collect::<Vec<_>>(), "followers", "Followers gained each day"))
                            }
                        }
                    }
                }
                section.block {
                    div.sa-sechead {
                        div {
                            h2 { "Streams" }
                            p.muted { "Your latest 30, each recorded when it ends." }
                        }
                    }
                    @if table.is_empty() {
                        (ui::empty(Icon::Live, "No streams yet", "Each stream is recorded when it ends: date, length, categories, peak and average viewers."))
                    } @else {
                        div.sa-rows.sa-streams {
                            @for r in &table {
                                @let live = r.ended_at.is_none();
                                @let games = r.games_line();
                                div.sa-row {
                                    div.sa-vt {
                                        @match r.thumbnail.is_empty() {
                                            false => { img src=(twitch::sized(&r.thumbnail, 320, 180)) alt="" width="112" height="63" loading="lazy"; }
                                            true => { (ui::icon(Icon::Live)) }
                                        }
                                        @if live { span.live-tag { "LIVE" } }
                                    }
                                    div.grow {
                                        strong { @if r.title.is_empty() { "Untitled stream" } @else { (r.title) } }
                                        span {
                                            (zoned(r.started_at, &tz).map(|z| z.strftime("%b %-d").to_string()).unwrap_or_default())
                                            " · " @if live { "live for " (text::duration((now - r.started_at) / 1000)) } @else { (text::duration(r.duration_secs)) }
                                            @if !games.is_empty() { " · " (games) }
                                        }
                                    }
                                    div.sa-peak.wide-only {
                                        @if r.samples > 0 {
                                            div { span { "peak " b { (text::num(r.peak)) } } span { "avg " (text::num(r.average)) } }
                                            span.meter { span style={ "width:" (r.peak * 100 / top_peak) "%" } {} }
                                        } @else {
                                            div { span { "No viewer counts" } }
                                        }
                                    }
                                    div.sa-vod {
                                        @if let Some(v) = &r.vod_url { a.btn.small href=(v) target="_blank" rel="noopener" { "VOD" } }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            aside.rail {
                (hours_block(&all, &tz, now))
                (followers_block(&followers, no_followers, reviews_link.as_deref()))
                @if !categories.is_empty() {
                    div.rail-block {
                        h2 { "Categories" span.muted.small { "12 weeks" } }
                        @for (i, (game, n)) in categories.iter().take(5).enumerate() {
                            div.sa-cat style={ "--i:" (i) } {
                                div { span { (game) } span.mono { (plural(*n, "stream", "streams")) } }
                                span.meter { span style={ "width:" (n * 100 / cat_max) "%" } {} }
                            }
                        }
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
            a.stream-card href=(page.path("alerts/stats")) {
                @if thumb.is_empty() {
                    span.stream-ph { (ui::icon(Icon::Live)) }
                } @else {
                    img src=(twitch::sized(thumb, 640, 360)) alt="" loading="lazy";
                }
                @if live { span.live-tag { "LIVE" } }
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
        assert_eq!(parse_logins("https://twitch.tv/antho?sr=a, @Other_One, antho").unwrap(), vec!["antho", "other_one"]);
        assert!(matches!(parse_logins("aaa, bbb, ccc, ddd, eee, fff"), Err(WebError::Bad(m)) if m.starts_with("Up to 5")));
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
        // Nothing Twitch can send renders as a machine word.
        assert_eq!(sub_status(Some("refused (400)")).1, "Twitch refused the subscription");
        assert_eq!(sub_status(Some("beta_maintenance")).1, "Stopped by Twitch");
        assert!(!sub_status(Some("version_removed")).1.contains('_'));
    }

    fn toronto() -> TimeZone {
        TimeZone::get("America/Toronto").unwrap()
    }

    /// Epoch ms of a wall-clock time in Toronto.
    fn local_ms(at: &str) -> i64 {
        at.parse::<bot_core::jiff::civil::DateTime>().unwrap().to_zoned(toronto()).unwrap().timestamp().as_millisecond()
    }

    #[test]
    fn weeks_follow_local_midnight_across_dst() {
        // Clocks go back at 2 AM on Sunday, Nov 1 2026, so that week is 169 hours long.
        let tz = toronto();
        let now = local_ms("2026-11-04T12:00:00");
        let row = |at: &str| StreamRow { started_at: local_ms(at), ended_at: Some(0), duration_secs: 3600, ..Default::default() };
        let rows = vec![row("2026-10-26T00:10:00"), row("2026-11-01T23:30:00"), row("2026-11-02T00:30:00")];
        let series = weekly_hours(&rows, &tz, now);
        assert_eq!(series.len(), WEEKS as usize);
        assert_eq!(series[series.len() - 2], ("Oct 26".to_owned(), 2));
        assert_eq!(series[series.len() - 1], ("Nov 2".to_owned(), 1));
        assert_eq!(this_monday(&tz, now).to_string(), "2026-11-02");
        assert_eq!(first_monday(&tz, now).to_string(), "2026-08-17");
    }

    #[test]
    fn month_starts_at_local_midnight() {
        assert_eq!(month_start(&toronto(), local_ms("2026-11-04T12:00:00")), local_ms("2026-11-01T00:00:00"));
        // Just after midnight on the 1st is already the new month.
        assert_eq!(month_start(&toronto(), local_ms("2027-01-01T00:05:00")), local_ms("2027-01-01T00:00:00"));
    }

    #[test]
    fn relative_times_read_naturally() {
        let now = 1_000_000_000_000;
        let min = 60_000;
        assert_eq!(until(now - min, now), "now");
        assert_eq!(until(now + 40 * min, now), "in 40 min");
        assert_eq!(until(now + 5 * 60 * min, now), "in 5 h");
        assert_eq!(until(now + 3 * 1440 * min, now), "in 3 days");
        assert_eq!(in_minutes(10), "in 10 minutes");
        assert_eq!(in_minutes(60), "in 1 hour");
        assert_eq!(in_minutes(120), "in 2 hours");
        assert_eq!(in_minutes(90), "in 90 minutes");
    }

    #[test]
    fn viewers_average_by_reading() {
        let row = |viewer_sum, samples| StreamRow { viewer_sum, samples, average: viewer_sum / samples.max(1), ..Default::default() };
        // A 4-hour stream counts for more than a 1-hour one; streams without counts are skipped.
        let rows = [row(40 * 48, 48), row(10 * 12, 12), row(999, 0)];
        assert_eq!(avg_viewers(rows.iter()), Some(34));
        assert_eq!(avg_viewers([row(5, 0)].iter()), None);
        // The sums are used as they are: 3 streams of 1.5 viewers each average 2 (rounded), not 1.
        let rows = [row(3, 2), row(3, 2), row(3, 2)];
        assert_eq!(avg_viewers(rows.iter()), Some(2));
    }

    #[test]
    fn followers_read_as_gained() {
        let d = |day: &str, v| (day.to_owned(), v);
        assert_eq!(followers_reading(&[]), None);
        assert_eq!(followers_reading(&[d("2026-10-01", 1200)]), Some(("followers", "1,200".into())));
        assert_eq!(followers_reading(&[d("2026-10-01", 1200), d("2026-10-15", 1210), d("2026-10-31", 1242)]), Some(("followers in 30 days", "+42".into())));
        assert_eq!(followers_reading(&[d("2026-10-01", 1200), d("2026-10-31", 1195)]), Some(("followers in 30 days", "-5".into())));
    }

    #[test]
    fn missing_followers_say_why() {
        assert_eq!(no_followers(Mode::Off, true, false), NoFollowers::TwitchOff);
        // Twitch refused the app token: connecting the broadcaster is the way to a count.
        assert_eq!(no_followers(Mode::Push, true, false), NoFollowers::Connect);
        assert_eq!(no_followers(Mode::Poll, true, false), NoFollowers::Connect);
        // Signed in, or not refused yet: the daily read brings one.
        assert_eq!(no_followers(Mode::Push, true, true), NoFollowers::Soon);
        assert_eq!(no_followers(Mode::Push, false, false), NoFollowers::Soon);
    }

    #[test]
    fn weekly_viewers_leave_weeks_without_counts_blank() {
        let tz = toronto();
        let now = local_ms("2026-11-04T12:00:00");
        let row = |at: &str, sum| StreamRow { started_at: local_ms(at), viewer_sum: sum, samples: 10, ..Default::default() };
        let rows = vec![
            row("2026-10-12T19:00:00", 200),
            row("2026-11-02T19:00:00", 300),
            // Streamed, but no count was read: not a week of zero viewers.
            StreamRow { started_at: local_ms("2026-10-26T19:00:00"), ..Default::default() },
        ];
        let series = weekly_viewers(&rows, &tz, now);
        assert_eq!(series.len(), WEEKS as usize);
        assert_eq!(series[0], ("Aug 17".to_owned(), None));
        assert_eq!(&series[8..], &[("Oct 12".to_owned(), Some(20)), ("Oct 19".to_owned(), None), ("Oct 26".to_owned(), None), ("Nov 2".to_owned(), Some(30))]);
        // A stream with counts that were all zero is a real zero.
        let series = weekly_viewers(&[row("2026-11-02T19:00:00", 0)], &tz, now);
        assert_eq!(series[11], ("Nov 2".to_owned(), Some(0)));
    }

    #[test]
    fn gallery_rows_fill() {
        // The first tile takes four cells from three tiles on; the filler takes what's left.
        assert_eq!(gallery_gap(1), 2);
        assert_eq!(gallery_gap(2), 1);
        assert_eq!(gallery_gap(3), 0);
        assert_eq!(gallery_gap(4), 2);
        assert_eq!(gallery_gap(5), 1);
        assert_eq!(gallery_gap(6), 0);
        assert_eq!(gallery_gap(12), 0);
    }

    #[test]
    fn categories_count_each_stream_once_per_category() {
        let row = |game: &str, games: &str| StreamRow { game: game.into(), games: games.into(), ..Default::default() };
        let rows = [row("Hades II", "Hades II\nJust Chatting"), row("Hades II", "Hades II"), row("Balatro", "")];
        assert_eq!(top_categories(&rows), vec![("Hades II".to_owned(), 2), ("Balatro".to_owned(), 1), ("Just Chatting".to_owned(), 1)]);
    }

    #[test]
    fn lists_join() {
        assert_eq!(join_and(&["60".into(), "10".into()]), "60 and 10");
        assert_eq!(join_and(&["60".into(), "30".into(), "10".into()]), "60, 30 and 10");
    }
}
