//! Pages every install has: the overview, the per-server feature switches, people and their
//! access, the activity log and notifications.

use axum::Router;
use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use maud::{Markup, html};
use rusqlite::params;

use super::auth::{Level, User};
use super::ui::{self, Icon, Tone};
use super::{FormData, WebError, WebResult, saved};
use crate::{Ctx, text};

pub(super) fn routes() -> Router<Ctx> {
    Router::new()
        .route("/", get(home))
        .route("/g/{guild}", get(overview))
        .route("/g/{guild}/features", get(features_page))
        .route("/g/{guild}/features/{key}", post(toggle_feature))
        .route("/g/{guild}/notifications", get(notifications))
        .route("/g/{guild}/notifications/peek", get(notifications_peek))
        .route("/users", get(users_page).post(add_user))
        .route("/users/{id}", get(user_page).post(save_user))
        .route("/users/{id}/delete", post(delete_user))
        .route("/users/{id}/copy", post(copy_access))
        .route("/activity", get(activity))
}

async fn home(State(ctx): State<Ctx>, user: User) -> WebResult {
    match ctx.guilds_for(&user).first() {
        Some(g) => Ok(Redirect::to(&format!("/g/{g}")).into_response()),
        None => Ok(super::ui::public_layout(
            "Nothing to show yet",
            Some(if user.admin {
                "The bot isn't in any server yet, or it's still connecting. Refresh in a moment."
            } else {
                "You haven't been given access to a server yet. Ask the owner."
            }),
            html! {},
        )
        .into_response()),
    }
}

fn admin_page(ctx: &Ctx, user: &User, guild: u64, key: &'static str) -> WebResult<super::Page> {
    if !user.admin {
        return Err(WebError::Forbidden);
    }
    ctx.page(user, guild, key, Level::Edit)
}

/// Any server the admin can see, for admin pages that aren't tied to one.
fn any_guild(ctx: &Ctx, user: &User) -> WebResult<u64> {
    ctx.guilds_for(user).first().copied().ok_or(WebError::NotFound)
}

fn daily(ctx: &Ctx, guild: u64, key: &str, days: i64) -> Vec<(String, i64)> {
    daily_keys(ctx, guild, &[key], days).remove(0)
}

/// [`daily`] for several keys in one query, in the order asked.
fn daily_keys(ctx: &Ctx, guild: u64, keys: &[&str], days: i64) -> Vec<Vec<(String, i64)>> {
    let today = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC).date();
    let start = today.checked_sub(jiff::Span::new().days(days - 1)).unwrap_or(today).to_string();
    let rows: HashMap<(String, String), i64> = ctx
        .db
        .with(|c| {
            // The primary key is (guild, key, day), so naming the keys keeps this to an index range.
            let marks = (3..keys.len() + 3).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
            let mut s = c.prepare(&format!("SELECT key, day, value FROM daily_stats WHERE guild_id = ?1 AND day >= ?2 AND key IN ({marks})"))?;
            let mut args: Vec<rusqlite::types::Value> = vec![(guild as i64).into(), start.clone().into()];
            args.extend(keys.iter().map(|k| k.to_string().into()));
            let rows = s
                .query_map(rusqlite::params_from_iter(args), |r| Ok(((r.get(0)?, r.get(1)?), r.get(2)?)))?
                .collect::<rusqlite::Result<HashMap<_, _>>>()?;
            Ok(rows)
        })
        .unwrap_or_default();
    let days: Vec<jiff::civil::Date> = (0..days).rev().map(|back| today.checked_sub(jiff::Span::new().days(back)).unwrap_or(today)).collect();
    keys.iter()
        .map(|key| {
            days.iter()
                .map(|d| (d.strftime("%b %-d").to_string(), rows.get(&(key.to_string(), d.to_string())).copied().unwrap_or(0)))
                .collect()
        })
        .collect()
}

/// Daily series for a stat key, oldest first, labelled like "Oct 30". Shared with the stats page.
pub fn daily_series(ctx: &Ctx, guild: u64, key: &str, days: i64) -> Vec<(String, i64)> {
    daily(ctx, guild, key, days)
}

async fn overview(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<HashMap<String, String>>) -> WebResult {
    let guilds = ctx.guilds_for(&user);
    if !guilds.contains(&guild) {
        return Err(if ctx.guild_ids().contains(&guild) { WebError::Forbidden } else { WebError::NotFound });
    }
    let page = super::Page { user: user.clone(), guild_id: guild, key: "overview", level: Level::View };
    let days: i64 = if q.get("days").map(String::as_str) == Some("7") { 7 } else { 30 };
    // "members" was the old name of the joins reading; old links still land on it.
    let show = match q.get("show").map(String::as_str) {
        Some("active") => "active",
        Some("voice") => "voice",
        Some("joins" | "members") => "joins",
        _ => "messages",
    };
    // Every series over twice the longest range in one read: the range, and the one before it.
    let [messages, joins, active, voice]: [Vec<(String, i64)>; 4] =
        daily_keys(&ctx, guild, &["messages", "joins", "active", "voice_minutes"], 60).try_into().unwrap_or_default();
    let voice: Vec<(String, i64)> = voice.into_iter().map(|(l, m)| (l, (m as f64 / 60.0).round() as i64)).collect();
    type Days = Vec<(String, i64)>;
    let window = |s: &[(String, i64)]| -> (Days, Days) {
        let n = s.len();
        let d = days as usize;
        (s[n.saturating_sub(d)..].to_vec(), s[n.saturating_sub(2 * d)..n.saturating_sub(d)].to_vec())
    };
    let members = ctx.member_count(guild).map(|n| n as i64);
    let active_today = active.last().map(|d| d.1).unwrap_or(0);
    // Daily numbers are counted per UTC day. Anyone in another timezone is told where it starts.
    let now = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC);
    let day_starts = now
        .start_of_day()
        .ok()
        .map(|m| user.local(m.timestamp().as_millisecond()))
        .filter(|l| l.offset() != jiff::tz::Offset::UTC)
        .map(|l| l.strftime("%H:%M").to_string());

    // The four readings. Each is the range's total (active members: its daily average), set
    // against the same number of days before it.
    struct Metric { key: &'static str, label: &'static str, unit: &'static str, tip_unit: &'static str, cur: Vec<(String, i64)>, prev: Vec<(String, i64)>, value: i64, before: i64 }
    let metric = |key, label, unit, tip_unit, s: &[(String, i64)], avg: bool| {
        let (cur, prev) = window(s);
        let total = |v: &[(String, i64)]| {
            let sum: i64 = v.iter().map(|d| d.1).sum();
            if avg { sum / v.len().max(1) as i64 } else { sum }
        };
        let (value, before) = (total(&cur), total(&prev));
        Metric { key, label, unit, tip_unit, cur, prev, value, before }
    };
    let metrics = [
        metric("messages", "Messages", "messages", "messages", &messages, false),
        metric("active", "Active members", "active a day", "active", &active, true),
        metric("voice", "Voice hours", "hours in voice", "hours", &voice, false),
        metric("joins", "Joins", "joined", "joined", &joins, false),
    ];
    let m = metrics.iter().find(|m| m.key == show).unwrap_or(&metrics[0]);
    let period = if days == 7 { "the week before".to_owned() } else { format!("the {days} days before") };
    let change = (m.before > 0).then(|| ((m.value - m.before) as f64 * 100.0 / m.before as f64).round() as i64);

    // What the features add, for the features this person can see.
    let mut needs = Vec::new();
    let mut feeds = Vec::new();
    let mut panel = Vec::new();
    let levels = ctx.levels(&user, guild);
    for f in ctx.features() {
        let key = f.meta().key;
        let level = levels.get(key);
        if !ctx.enabled(guild, key) || level == Level::None {
            continue;
        }
        let fp = super::Page { user: user.clone(), guild_id: guild, key, level };
        let o = f.overview(&ctx, &fp);
        needs.extend(o.needs);
        feeds.push(o.feed);
        panel.extend(o.panel);
    }
    // Alerts first, then whatever has waited longest.
    needs.sort_by_key(|n| (!n.alert, n.since));
    let feed = mix_feed(feeds, 6);
    panel.sort_by_key(|p| p.0);
    let on = ctx.features().iter().filter(|f| ctx.enabled(guild, f.meta().key)).count();

    let name = ctx.guild_name(guild);
    let banner = ctx.guild_banner_url(guild);
    let icon_large = ctx.guild_icon_large(guild);
    let today = user.local(crate::now_ms()).strftime("%A %-d %B").to_string();
    let link = |show: &str, days: i64| format!("/g/{guild}?show={show}&days={days}");
    let swap = "#ov-metrics, #ov-fig, #ov-plot, #ov-legend";

    let answer = html! {
        section.ov-answer aria-label="Activity" {
            nav.ov-metrics #ov-metrics aria-label="Readings" {
                @for x in &metrics {
                    a.ov-metric.on[x.key == m.key] href=(link(x.key, days)) aria-current=[(x.key == m.key).then_some("true")] data-swap=(swap) {
                        (x.label)
                        span.mono { (text::num(x.value)) }
                    }
                }
            }
            div.ov-top-row {
                div.ov-fig #ov-fig {
                    b { (text::num(m.value)) }
                    span.ov-unit { (m.unit) }
                    @if let Some(c) = change {
                        span.ov-change { span.up[c > 0].down[c < 0] { (if c >= 0 { "+" } else { "−" }) (c.abs()) "%" } " vs " (period) }
                    }
                }
                div.seg {
                    a.on[days == 7] href=(link(m.key, 7)) data-swap=(swap) { "7 days" }
                    a.on[days == 30] href=(link(m.key, 30)) data-swap=(swap) { "30 days" }
                }
            }
            div #ov-plot {
                @if m.cur.iter().all(|d| d.1 == 0) {
                    (ui::empty(Icon::Chart, "Nothing counted in these days", "The bot counts as things happen, and the chart fills in day by day."))
                } @else {
                    (ui::area_chart_vs(&m.cur, Some(&m.prev), m.tip_unit))
                }
            }
            div.ov-legend #ov-legend {
                span { i {} (if days == 7 { "This week" } else { "Last 30 days" }) }
                @if m.prev.iter().any(|d| d.1 > 0) { span { i.prev {} (if days == 7 { "Week before" } else { "30 days before" }) } }
                @if let Some(t) = &day_starts { span { "Days start at " (t) " your time" } }
            }
        }
    };
    let needs_block = html! {
        section.ov-block {
            h2 { "Needs you" @if !needs.is_empty() { span.ov-count { (needs.len()) } } }
            @if needs.is_empty() {
                (ui::empty(Icon::Check, "Nothing is waiting on you", "Scam catches, pet requests, tickets and app approvals land here when they need someone."))
            } @else {
                // The first few, then the rest folded under one line.
                div.ov-rows {
                    @for n in needs.iter().take(NEEDS_SHOWN) { (need_row(n)) }
                    @if needs.len() > NEEDS_SHOWN {
                        details.ov-more {
                            summary { "Show " (needs.len() - NEEDS_SHOWN) " more" }
                            @for n in needs.iter().skip(NEEDS_SHOWN) { (need_row(n)) }
                        }
                    }
                }
            }
        }
    };
    let feed_block = html! {
        section.ov-block {
            h2 { "Around the server" }
            @if feed.is_empty() {
                (ui::empty(Icon::Activity, "Quiet so far", "Level-ups, new members, pet additions and suggestion results show up here."))
            } @else {
                div.ov-feed {
                    @for h in &feed {
                        div.happen {
                            img.avatar src=(h.avatar) alt="" width="28" height="28" loading="lazy";
                            span.grow { strong { (h.who) } " " (h.text) }
                            span.mono.muted { (short_ago(h.at)) }
                        }
                    }
                }
            }
        }
    };

    let uploaded = super::banner::uploaded_at(&ctx, guild).is_some();
    let body = html! {
        header.ov-head data-local {
            // For admins the banner itself is the control: clicking it picks a new picture.
            @let bg = html! {
                @match (&banner, &icon_large) {
                    (Some(b), _) => img src=(b) alt="" fetchpriority="high";,
                    (None, i) => (ui::art(i.as_deref(), true)),
                }
            };
            @if user.admin {
                button.ov-bg.editable type="button" data-banner-pick aria-label="Change the banner" { (bg) }
                div.ov-tools {
                    span.ov-hint { (ui::icon(Icon::Image)) "Change banner" }
                    @if uploaded {
                        form.inline method="post" action={ "/g/" (guild) "/banner/remove" } data-full {
                            button.ov-hint.btnlike type="submit" { "Remove upload" }
                        }
                    }
                }
                input #banner-file type="file" accept="image/png,image/jpeg,image/webp,image/gif" hidden data-action={ "/g/" (guild) "/banner" };
            } @else {
                div.ov-bg { (bg) }
            }
            div.ov-hello {
                @match &icon_large {
                    Some(i) => img.ov-icon src=(i) alt="" width="80" height="80";,
                    None => span.ov-icon.letter { (name.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default()) },
                }
                div.grow {
                    h1 { (name) }
                    p {
                        span { (today) }
                        @if let Some(n) = members { span { (text::num(n)) " members" } }
                        @if active_today > 0 { span { span.dot.live {} (text::num(active_today)) " here today" } }
                    }
                }
            }
        }
        div.page-body.ov-body {
            div.ov-grid {
                div.ov-left {
                    (answer)
                    div.ov-sub { (feed_block) (needs_block) }
                }
                @if !panel.is_empty() {
                    aside.ov-right { @for (_, block) in &panel { section.ov-block { (block.clone()) } } }
                }
            }
            @if user.admin && on == 0 {
                section.ov-block {
                    h2 { "Get started" }
                    p.muted { "Every feature starts off in a new server. Switch on what this server should have." }
                    div { a.btn.primary href={ "/g/" (guild) "/features" } { "Choose features" } }
                }
            }
        }
    };
    Ok(ui::layout_custom(&ctx, &page, &name, body).into_response())
}

/// How many "Needs you" rows show before the rest fold away.
const NEEDS_SHOWN: usize = 5;

/// One row of "Needs you": a face (or the feature's icon), what waits, and its drawn button. The
/// whole row is the link.
fn need_row(n: &crate::Need) -> Markup {
    html! {
        a.need href=(n.href) {
            @match n.people.as_slice() {
                [] => span.need-icon.alert[n.alert] { (ui::icon(n.icon)) },
                [one] => img.avatar src=(one) alt="" width="28" height="28" loading="lazy";,
                more => span.need-two { @for u in more.iter().take(2) { img.avatar src=(u) alt="" loading="lazy"; } },
            }
            div.grow { strong.clip { (n.title) } span.clip { (n.detail) } }
            span.need-btn.primary[n.alert] { (n.action) }
        }
    }
}

/// Up to `n` happenings, newest first, taken from each source in turn so one busy feature (a run
/// of level-ups) can't push the others out. Sources with the freshest news go first in a round.
fn mix_feed(mut sources: Vec<Vec<crate::Happening>>, n: usize) -> Vec<crate::Happening> {
    // Oldest first, so pop() gives each source's newest.
    for s in &mut sources {
        s.sort_by_key(|h| h.at);
    }
    sources.retain(|s| !s.is_empty());
    sources.sort_by_key(|s| std::cmp::Reverse(s.last().map(|h| h.at)));
    let mut out = Vec::new();
    while out.len() < n && !sources.is_empty() {
        for s in &mut sources {
            if out.len() == n {
                break;
            }
            out.extend(s.pop());
        }
        sources.retain(|s| !s.is_empty());
    }
    out.sort_by_key(|h| std::cmp::Reverse(h.at));
    out
}

/// "12 min", "3 h", "2 d" for the overview's feed.
fn short_ago(ms: i64) -> String {
    let secs = ((crate::now_ms() - ms) / 1000).max(0);
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{} min", secs / 60),
        3600..86400 => format!("{} h", secs / 3600),
        _ => format!("{} d", secs / 86400),
    }
}

struct Note {
    at: i64,
    feature: String,
    text: String,
    link: Option<String>,
    global: bool,
}

/// Notices for a server that this person may see: only features they can at least view, and
/// server-less notices (app connections) only for the owner and Admin+ people.
fn recent_notifications(ctx: &Ctx, user: &User, guild: u64, limit: i64) -> Vec<Note> {
    let mut notes = all_notifications(ctx, guild, limit * 4);
    keep_visible(ctx, user, guild, &mut notes);
    notes.truncate(limit as usize);
    notes
}

fn keep_visible(ctx: &Ctx, user: &User, guild: u64, notes: &mut Vec<Note>) {
    let levels = ctx.levels(user, guild);
    notes.retain(|n| match n.global {
        // A notice with no server (app connections, Twitch losing its subscription) follows the
        // feature that posted it: Admin+ for kept features, else admins and its editors.
        true => match ctx.feature(&n.feature) {
            Some(f) if f.admin_plus_only() => user.admin_plus,
            _ => user.admin || levels.get(&n.feature) >= Level::View,
        },
        false => levels.get(&n.feature) >= Level::View,
    });
}

/// When each notice this person may see arrived since `since`, for the chart.
fn notification_times(ctx: &Ctx, user: &User, guild: u64, since: i64) -> Vec<i64> {
    let mut notes: Vec<Note> = ctx
        .db
        .with(|c| {
            let mut s = c.prepare("SELECT at, feature, guild_id IS NULL FROM notifications WHERE (guild_id IS NULL OR guild_id = ?1) AND at >= ?2")?;
            let rows = s
                .query_map(params![guild as i64, since], |r| Ok(Note { at: r.get(0)?, feature: r.get(1)?, text: String::new(), link: None, global: r.get(2)? }))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap_or_default();
    keep_visible(ctx, user, guild, &mut notes);
    notes.into_iter().map(|n| n.at).collect()
}

fn all_notifications(ctx: &Ctx, guild: u64, limit: i64) -> Vec<Note> {
    ctx.db
        .with(|c| {
            let mut s = c.prepare(
                "SELECT at, feature, text, link, guild_id IS NULL FROM notifications WHERE guild_id IS NULL OR guild_id = ?1 ORDER BY at DESC LIMIT ?2",
            )?;
            let rows = s
                .query_map(params![guild as i64, limit], |r| {
                    Ok(Note { at: r.get(0)?, feature: r.get(1)?, text: r.get(2)?, link: r.get(3)?, global: r.get(4)? })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap_or_default()
}

/// One notice: the feature's icon, the text, the feature and time under it, a dot while it's
/// new. The whole row opens the notice's link.
fn notification_row(ctx: &Ctx, user: &User, n: &Note, new: bool) -> Markup {
    let feature = ctx.feature(&n.feature);
    let ic = feature.and_then(|f| f.meta().nav.first().map(|n| n.icon)).unwrap_or(Icon::Bell);
    let name = feature.map(|f| f.meta().name).unwrap_or("Dashboard");
    let inner = html! {
        span.note-icon { (ui::icon(ic)) }
        span.grow {
            span.note-text { (n.text) }
            span.muted.small { (name) " · " (user.local(n.at).strftime("%H:%M")) }
        }
        @if new { span.unread title="New" {} span.sr { "New" } }
        @if n.link.is_some() { span.go { (ui::icon(Icon::Chevron)) } }
    };
    html! {
        @match &n.link {
            Some(l) => a.note.new[new] href=(l) data-hue=(ui::feature_hue(ctx, &n.feature)) { (inner) },
            None => div.note.new[new] data-hue=(ui::feature_hue(ctx, &n.feature)) { (inner) },
        }
    }
}

/// One notice in the bell's dropdown: compact, two lines of text at most, the feature and when
/// underneath, a dot when it's new. The whole row is the link when there is one.
fn peek_row(ctx: &Ctx, n: &Note, new: bool) -> Markup {
    let feature = ctx.feature(&n.feature);
    let ic = feature.and_then(|f| f.meta().nav.first().map(|n| n.icon)).unwrap_or(Icon::Bell);
    let name = feature.map(|f| f.meta().name).unwrap_or("Dashboard");
    let inner = html! {
        span.pk-ic { (ui::icon(ic)) }
        span {
            span.pk-text title=(n.text) { (n.text) }
            span.pk-meta { (name) " · " @let ago = short_ago(n.at); (ago) @if ago != "just now" { " ago" } }
        }
        span.pk-dot {}
        @if new { span.sr { "New" } }
    };
    html! {
        @match &n.link {
            Some(l) => a.pk-row.new[new] href=(l) { (inner) },
            None => div.pk-row.new[new] { (inner) },
        }
    }
}

async fn notifications(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    if !ctx.guilds_for(&user).contains(&guild) {
        return Err(WebError::Forbidden);
    }
    let page = super::Page { user: user.clone(), guild_id: guild, key: "notifications", level: Level::View };
    let seen = ui::notifications_seen(&ctx, user.id, guild);
    let global_seen = ui::notifications_seen_anywhere(&ctx, user.id);
    let notes = recent_notifications(&ctx, &user, guild, 100);
    let is_new = |n: &Note| n.at > if n.global { global_seen } else { seen };
    let fresh = notes.iter().filter(|n| is_new(n)).count();
    // Which features the notices came from, busiest first: (feature, notices, new).
    let mut from: Vec<(&str, usize, usize)> = Vec::new();
    for n in &notes {
        match from.iter_mut().find(|f| f.0 == n.feature) {
            Some(f) => {
                f.1 += 1;
                f.2 += usize::from(is_new(n));
            }
            None => from.push((n.feature.as_str(), 1, usize::from(is_new(n)))),
        }
    }
    from.sort_by_key(|f| std::cmp::Reverse(f.1));
    let per_day = per_day(&user, &notification_times(&ctx, &user, guild, crate::now_ms() - 15 * 86_400_000), 14);
    let fortnight: i64 = per_day.iter().filter_map(|d| d.1).sum();
    let top = from.first().map(|f| f.1).unwrap_or(1).max(1);
    let body = html! {
        @if notes.is_empty() {
            (ui::empty(Icon::Bell, "No notifications", "Features post here when something needs you: a caught scam, a new ticket, a pet request."))
        } @else {
            div.cp-cols {
                div.cp-main {
                    // No data-local: its one-time reload for the timezone would count as a second
                    // visit and clear the new marks. The overview, where everyone lands, sets it.
                    section.cp-panel {
                        h2 { "Notices" span.muted { (notes.len()) @if notes.len() == 100 { " newest" } } }
                        div.cp-days.cp-notes {
                            @for (i, n) in notes.iter().enumerate() {
                                @let label = day_label(&user, n.at);
                                @if i == 0 || day_label(&user, notes[i - 1].at) != label { h3.cp-glabel { (label) } }
                                (notification_row(&ctx, &user, n, is_new(n)))
                            }
                        }
                    }
                }
                aside.cp-side.cp-stick {
                    section.cp-panel {
                        h2 { "Last 14 days" span.muted { (fortnight) @if fortnight == 1 { " notice" } @else { " notices" } } }
                        (ui::column_bars(&per_day, "notices", "Notices on each of the last 14 days"))
                    }
                    section.cp-panel {
                        h2 { "Where they came from" }
                        div.cp-list {
                            @for (key, total, new) in &from {
                                @let f = ctx.feature(key);
                                div.cp-row.cp-bar-row {
                                    span.cp-ic { (ui::icon(f.and_then(|f| f.meta().nav.first().map(|n| n.icon)).unwrap_or(Icon::Bell))) }
                                    span.cp-txt {
                                        strong { (f.map(|f| f.meta().name).unwrap_or("Dashboard")) @if *new > 0 { span.cp-new { (new) " new" } } }
                                        span.meter { span style={ "width:" (total * 100 / top) "%" } {} }
                                    }
                                    span.cp-n { (total) }
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    // Seen from now on, so the bell on this page is already clear.
    mark_seen(&ctx, &user, guild)?;
    let actions = (fresh > 0).then(|| ui::badge(&format!("{fresh} new"), Tone::Accent));
    Ok(ui::layout(&ctx, &page, "Notifications", actions, body).into_response())
}

/// Every notice for this server counts as seen from now.
fn mark_seen(ctx: &Ctx, user: &User, guild: u64) -> WebResult<()> {
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO dash_notif_seen (user_id, guild_id, seen_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (user_id, guild_id) DO UPDATE SET seen_at = excluded.seen_at",
            params![user.id as i64, guild as i64, crate::now_ms()],
        )
    })?;
    Ok(())
}

/// The bell's dropdown: the newest notices as a fragment app.js puts in place. Opening it
/// counts as seeing them, as the full page does.
async fn notifications_peek(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    if !ctx.guilds_for(&user).contains(&guild) {
        return Err(WebError::Forbidden);
    }
    let seen = ui::notifications_seen(&ctx, user.id, guild);
    let global_seen = ui::notifications_seen_anywhere(&ctx, user.id);
    let notes = recent_notifications(&ctx, &user, guild, 8);
    let is_new = |n: &Note| n.at > if n.global { global_seen } else { seen };
    let body = html! {
        @let fresh = notes.iter().filter(|n| is_new(n)).count();
        div.peek-head {
            strong { "Notifications" @if fresh > 0 { span.muted { (fresh) " new" } } }
            a href={ "/g/" (guild) "/notifications" } { "See all" }
        }
        @if notes.is_empty() {
            p.peek-none { "Nothing yet. Features post here when something needs you." }
        } @else {
            div.peek-list {
                @for n in &notes { (peek_row(&ctx, n, is_new(n))) }
            }
        }
    };
    mark_seen(&ctx, &user, guild)?;
    Ok(([(axum::http::header::CACHE_CONTROL, "no-store")], body).into_response())
}

/// "Today", "Yesterday", a weekday for the last week, then "Oct 3": a day heading in the
/// viewer's timezone.
fn day_label(user: &User, at: i64) -> String {
    let today = user.local(crate::now_ms()).date();
    let d = user.local(at).date();
    match (today - d).get_days() {
        0 => "Today".to_owned(),
        1 => "Yesterday".to_owned(),
        2..=6 => d.strftime("%A").to_string(),
        _ => d.strftime("%b %-d").to_string(),
    }
}

// ---- features ----

async fn features_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = admin_page(&ctx, &user, guild, "features")?;
    // A feature kept for Admin+ (App approvals) isn't listed for a plain admin to switch.
    let mut feats: Vec<_> = ctx.features().iter().filter(|f| user.admin_plus || !f.admin_plus_only()).map(|f| f.meta()).collect();
    feats.sort_by_key(|m| m.group);
    let mut groups: Vec<(crate::Group, Vec<crate::FeatureMeta>)> = Vec::new();
    for m in feats {
        match groups.iter_mut().find(|(g, _)| *g == m.group) {
            Some((_, list)) => list.push(m),
            None => groups.push((m.group, vec![m])),
        }
    }
    let shown = groups.iter().map(|(_, l)| l.len()).sum::<usize>();
    let on_here = groups.iter().flat_map(|(_, l)| l).filter(|m| ctx.enabled(guild, m.key)).count();
    let guilds = ctx.guilds_for(&user);
    let count_on = |g: u64| ctx.features().iter().filter(|f| ctx.enabled(g, f.meta().key)).count();
    let switched = audit_lines(&ctx, "features", Some(guild), 6);
    // What members can type here: every switched-on feature's commands, named once.
    let mut commands: Vec<String> = ctx
        .features()
        .iter()
        .filter(|f| ctx.enabled(guild, f.meta().key))
        .flat_map(|f| f.commands().into_iter().chain(f.guild_commands(&ctx, guild)))
        .filter(|c| c.kind == twilight_model::application::command::CommandType::ChatInput)
        .map(|c| c.name)
        .collect();
    commands.sort();
    commands.dedup();
    let body = html! {
        div.cp-cols {
            div.cp-main {
                section.cp-panel {
                    h2 { "On " (ctx.guild_name(guild)) span.muted { (on_here) " of " (shown) " on" } }
                    // One list in two balanced columns when there's room; a group's label stays
                    // with its first feature.
                    div.cp-feats {
                        @for (group, list) in &groups {
                            div.cp-fgroup {
                            h3.cp-glabel { (group.label()) }
                            @for m in list {
                                @let on = ctx.enabled(guild, m.key);
                                // A full load: switching a feature changes the sidebar itself.
                                form.cp-frow.off[!on] method="post" action={ "/g/" (guild) "/features/" (m.key) } data-full {
                                    span.cp-ic { @if let Some(n) = m.nav.first() { (ui::icon(n.icon)) } }
                                    label.cp-txt for={ "on-" (m.key) } { strong { (m.name) } span { (m.description) } }
                                    label.switch {
                                        input type="checkbox" id={ "on-" (m.key) } name={ "on-" (m.key) } checked[on] data-autosubmit;
                                        span.track { span.thumb {} }
                                    }
                                }
                            }
                            }
                        }
                    }
                }
            }
            aside.cp-side {
                @if guilds.len() > 1 {
                    section.cp-panel {
                        h2 { "Servers" }
                        nav.cp-list aria-label="Server" {
                            @for g in &guilds {
                                a.cp-row.cp-pick.on[*g == guild] href={ "/g/" (g) "/features" } aria-current=[(*g == guild).then_some("page")] {
                                    (ui::guild_avatar(&ctx, *g))
                                    span.cp-txt { strong { (ctx.guild_name(*g)) } span { (count_on(*g)) " features on" } }
                                }
                            }
                        }
                    }
                }
                section.cp-panel {
                    h2 { "Slash commands here" span.muted { (commands.len()) } }
                    @if commands.is_empty() {
                        p.cp-none { "None. Turn on a feature with commands and they show up in Discord." }
                    } @else {
                        div.cp-cmds { @for c in &commands { code { "/" (c) } } }
                    }
                }
                section.cp-panel {
                    h2 { "Recently switched" }
                    (audit_list(&switched, "Nobody has switched a feature here yet."))
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Features", None, body).into_response())
}

/// A dashboard change for a side list: when, who (id, name, avatar) and what they did.
struct AuditLine {
    at: i64,
    who: u64,
    name: String,
    avatar: Option<String>,
    action: String,
}

/// The newest changes made on one dashboard page (`features`, `users`), on one server or all.
fn audit_lines(ctx: &Ctx, page_key: &str, guild: Option<u64>, limit: i64) -> Vec<AuditLine> {
    ctx.db
        .with(|c| {
            let mut s = c.prepare(
                "SELECT a.at, a.user_id, a.user_name, u.avatar, a.action FROM dash_audit a LEFT JOIN dash_users u ON u.id = a.user_id
                 WHERE a.feature = ?1 AND (?2 IS NULL OR a.guild_id = ?2) ORDER BY a.at DESC LIMIT ?3",
            )?;
            let rows = s
                .query_map(params![page_key, guild.map(|g| g as i64), limit], |r| {
                    Ok(AuditLine { at: r.get(0)?, who: r.get::<_, i64>(1)? as u64, name: r.get(2)?, avatar: r.get(3)?, action: r.get(4)? })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap_or_default()
}

/// Changes as rows: the face, "Antho turned Starboard off", how long ago.
fn audit_list(lines: &[AuditLine], none: &str) -> Markup {
    html! {
        @if lines.is_empty() {
            p.cp-none { (none) }
        } @else {
            div.cp-list {
                @for l in lines {
                    div.cp-row {
                        img.avatar src=(crate::discord::avatar_url(l.who, l.avatar.as_deref())) alt="" width="28" height="28" loading="lazy";
                        span.cp-txt { span.cp-line { strong { (l.name) } " " (lower_first(&l.action)) } span { (text::ago(l.at)) } }
                    }
                }
            }
        }
    }
}

async fn toggle_feature(State(ctx): State<Ctx>, user: User, Path((guild, key)): Path<(u64, String)>, form: FormData) -> WebResult {
    admin_page(&ctx, &user, guild, "features")?;
    let feature = ctx.feature(&key).ok_or(WebError::NotFound)?;
    if feature.admin_plus_only() && !user.admin_plus {
        return Err(WebError::Forbidden);
    }
    let meta = feature.meta();
    let on = form.bool(&format!("on-{key}"));
    ctx.set_enabled(guild, meta.key, on)?;
    ctx.audit(&user, Some(guild), "features", format!("Turned {} {}", meta.name, if on { "on" } else { "off" }));
    let ctx2 = ctx.clone();
    tokio::spawn(async move {
        if let Err(e) = crate::discord::register_commands(&ctx2, guild).await {
            tracing::warn!(error = %e, "couldn't update commands");
        }
    });
    Ok(saved(&format!("/g/{guild}/features")))
}

// ---- people ----

struct Person {
    id: u64,
    name: String,
    avatar: Option<String>,
    admin: bool,
    admin_plus: bool,
    created_at: i64,
    grants: i64,
    signed_in: Option<i64>,
    added_by: Option<String>,
}

fn people(ctx: &Ctx) -> Vec<Person> {
    ctx.db
        .with(|c| {
            let mut s = c.prepare(
                "SELECT u.id, u.name, u.avatar, u.admin, u.created_at, u.admin_plus,
                        (SELECT COUNT(*) FROM dash_perms p WHERE p.user_id = u.id AND p.level > 0),
                        (SELECT MAX(s.created_at) FROM dash_sessions s WHERE s.user_id = u.id),
                        (SELECT a.name FROM dash_users a WHERE a.id = u.added_by AND a.name != '')
                 FROM dash_users u ORDER BY u.admin DESC, u.name COLLATE NOCASE",
            )?;
            let rows = s
                .query_map([], |r| {
                    Ok(Person {
                        id: r.get::<_, i64>(0)? as u64,
                        name: r.get(1)?,
                        avatar: r.get(2)?,
                        admin: r.get(3)?,
                        created_at: r.get(4)?,
                        admin_plus: r.get(5)?,
                        grants: r.get(6)?,
                        signed_in: r.get(7)?,
                        added_by: r.get(8)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap_or_default()
}

async fn users_page(State(ctx): State<Ctx>, user: User) -> WebResult {
    let guild = any_guild(&ctx, &user)?;
    let page = admin_page(&ctx, &user, guild, "users")?;
    let list = people(&ctx);
    let owner = ctx.cfg.owner_id;
    let add = ui::pop("", "Add someone", Some(Icon::Plus), "primary", html! {
        form.inline-form method="post" action="/users" {
            input.input.mono type="text" name="id" placeholder="Discord user ID" inputmode="numeric" pattern="[0-9]{15,21}" required autofocus;
            button.btn.primary type="submit" { "Add" }
        }
        p.muted.small { "In Discord: Settings, Advanced, turn on Developer Mode, then right-click their name and Copy User ID. People who try to sign in see their ID too. Until you give them pages, they see nothing." }
    });
    // Which pages each person holds, across servers: (page key, edit?) by person.
    let mut access: HashMap<u64, Vec<(String, bool)>> = HashMap::new();
    let rows: Vec<(i64, String, i64)> = ctx
        .db
        .with(|c| {
            let mut s = c.prepare("SELECT user_id, page, MAX(level) FROM dash_perms WHERE level > 0 GROUP BY user_id, page ORDER BY page")?;
            let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap_or_default();
    for (uid, key, level) in rows {
        access.entry(uid as u64).or_default().push((key, level >= 2));
    }
    let admins = list.iter().filter(|p| p.admin || p.id == owner).count();
    let today = list.iter().filter(|p| p.signed_in.is_some_and(|at| crate::now_ms() - at < 86_400_000)).count();
    let changes = audit_lines(&ctx, "users", None, 6);
    let body = html! {
        div.cp-cols {
            div.cp-main {
                section.cp-panel {
                    h2 { "Who can sign in" span.muted { (list.len()) } }
                    div.cp-list.cp-people {
                        @for p in &list {
                            @let seen = match p.signed_in {
                                Some(at) if p.id == user.id && crate::now_ms() - at < 3_600_000 => "Signed in now".to_owned(),
                                Some(at) => format!("Signed in {}", text::ago(at)),
                                None => format!("Added {} · hasn't signed in yet", text::ago(p.created_at)),
                            };
                            @let online = p.signed_in.is_some_and(|at| crate::now_ms() - at < 3_600_000);
                            @let pages = access.get(&p.id).map(Vec::as_slice).unwrap_or_default();
                            @let inner = html! {
                                span.cp-face {
                                    img.avatar src=(crate::discord::avatar_url(p.id, p.avatar.as_deref())) alt="" width="28" height="28" loading="lazy";
                                    @if online { span.dot.live title="Signed in within the hour" {} }
                                }
                                span.cp-txt {
                                    strong { (name_or_id(p)) }
                                    span { (seen) @if let Some(by) = p.added_by.as_ref().filter(|_| p.id != owner) { " · added by " (by) } }
                                }
                                span.cp-access {
                                    @if p.id == owner { (ui::badge("Owner", Tone::Muted)) }
                                    @else if p.admin && p.admin_plus { (ui::badge("Admin+, every page", Tone::Muted)) }
                                    @else if p.admin { (ui::badge("Admin, every page", Tone::Muted)) }
                                    @else if pages.is_empty() { (ui::badge("No pages yet", Tone::Muted)) }
                                    @else {
                                        @for (key, edit) in pages.iter().take(7) {
                                            @let f = ctx.feature(key);
                                            @let label = f.and_then(|f| f.meta().nav.first().map(|n| n.label)).unwrap_or(key.as_str());
                                            @let ic = f.and_then(|f| f.meta().nav.first().map(|n| n.icon)).unwrap_or(Icon::Toggle);
                                            span.cp-page.edit[*edit] data-tip={ (label) ": " (if *edit { "can edit" } else { "can view" }) } { (ui::icon(ic)) }
                                        }
                                        @if pages.len() > 7 { span.cp-more { "+" (pages.len() - 7) } }
                                    }
                                }
                                @if p.id != owner { span.btn.small.outline.cp-btn { "Edit access" } } @else { span.cp-btn {} }
                            };
                            @if p.id == owner {
                                div.cp-row.cp-person { (inner) }
                            } @else {
                                a.cp-row.cp-person.cp-link href={ "/users/" (p.id) } { (inner) }
                            }
                        }
                    }
                }
            }
            aside.cp-side {
                section.cp-panel {
                    h2 { "Access" }
                    div.rail-nums {
                        div { b { (list.len()) } span { "can sign in" } }
                        div { b { (admins) } span { "see everything" } }
                        div { b { (today) } span { "signed in today" } }
                    }
                }
                section.cp-panel {
                    h2 { "Recent changes" }
                    (audit_list(&changes, "Nobody's access has changed yet."))
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "People", Some(add), body).into_response())
}

async fn add_user(State(ctx): State<Ctx>, user: User, form: FormData) -> WebResult {
    let guild = any_guild(&ctx, &user)?;
    admin_page(&ctx, &user, guild, "users")?;
    let id: u64 = form.id("id").filter(|id| *id > 1_000_000).ok_or_else(|| WebError::Bad("That isn't a Discord user ID. It's a long number, like 123456789012345678.".into()))?;
    let name = ctx
        .http
        .user(twilight_model::id::Id::new(id))
        .await
        .ok()
        .map(|r| async move { r.model().await.ok() });
    let (name, avatar) = match name {
        Some(f) => match f.await {
            Some(u) => (u.global_name.unwrap_or(u.name), u.avatar.map(|a| a.to_string())),
            None => (String::new(), None),
        },
        None => (String::new(), None),
    };
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO dash_users (id, name, avatar, admin, added_by, created_at) VALUES (?1, ?2, ?3, 0, ?4, ?5)
             ON CONFLICT (id) DO NOTHING",
            params![id as i64, name, avatar, user.id as i64, crate::now_ms()],
        )
    })?;
    ctx.audit(&user, None, "users", format!("Added {}", if name.is_empty() { id.to_string() } else { name }));
    Ok(saved(&format!("/users/{id}")))
}

async fn user_page(State(ctx): State<Ctx>, user: User, Path(id): Path<u64>) -> WebResult {
    let guild = any_guild(&ctx, &user)?;
    let page = admin_page(&ctx, &user, guild, "users")?;
    let person = people(&ctx).into_iter().find(|p| p.id == id).ok_or(WebError::NotFound)?;
    let grants: Vec<(u64, String, i64)> = ctx.db.with(|c| {
        let mut s = c.prepare("SELECT guild_id, page, level FROM dash_perms WHERE user_id = ?1")?;
        let rows = s.query_map([id as i64], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })?;
    let level_of = |g: u64, key: &str| grants.iter().find(|(gg, k, _)| *gg == g && k == key).map(|x| x.2).unwrap_or(0);
    let is_owner = id == ctx.cfg.owner_id;
    let viewer_is_owner = user.id == ctx.cfg.owner_id;
    let display = if person.name.is_empty() { "Not signed in yet".to_owned() } else { person.name.clone() };
    let grantable = |g: u64| ctx.features().iter().filter(|f| ctx.enabled(g, f.meta().key) && !f.admin_plus_only()).collect::<Vec<_>>();
    // People whose pages can be copied over: not admins (they have no grants to copy), not this one.
    let others: Vec<(String, String)> = people(&ctx)
        .into_iter()
        .filter(|p| p.id != id && !p.admin && p.id != ctx.cfg.owner_id && p.grants > 0)
        .map(|p| (p.id.to_string(), format!("{} · {} {}", name_or_id(&p), p.grants, if p.grants == 1 { "page" } else { "pages" })))
        .collect();
    let seen = match person.signed_in {
        Some(at) => format!("Signed in {}", text::ago(at)),
        None => format!("Added {} · hasn't signed in yet", text::ago(person.created_at)),
    };
    let given = grants.iter().filter(|g| g.2 > 0).count();
    let editable = grants.iter().filter(|g| g.2 >= 2).count();
    let every = person.admin || is_owner;
    // Their own latest changes (App approvals' only for those who may see that page).
    let theirs: Vec<(i64, Option<i64>, String, String)> = if every {
        ctx.db
            .with(|c| {
                let mut s = c.prepare("SELECT at, guild_id, feature, action FROM dash_audit WHERE user_id = ?1 ORDER BY at DESC LIMIT 20")?;
                let rows = s.query_map([id as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, _, f, _): &(i64, Option<i64>, String, String)| user.admin_plus || !ctx.feature(f).is_some_and(|f| f.admin_plus_only()))
            .take(8)
            .collect()
    } else {
        Vec::new()
    };

    // One form across both columns: the pages on the left, their role and Save on the right.
    // Remove is a formaction button in the same form, so no form sits inside another.
    let body = html! {
        form.settings.cp-cols method="post" action={ "/users/" (id) } {
            div.cp-main {
                @if every {
                    section.cp-panel {
                        h2 { "Pages" }
                        div.cp-list {
                            @for g in ctx.guild_ids() {
                                div.cp-row {
                                    (ui::guild_avatar(&ctx, g))
                                    span.cp-txt {
                                        strong { (ctx.guild_name(g)) }
                                        span { "Every page, and can change them" @if person.admin_plus || is_owner { ", App approvals too" } }
                                    }
                                }
                            }
                        }
                    }
                    // An admin's grants say little; what they've been changing says more.
                    section.cp-panel {
                        h2 { "What they changed lately" }
                        @if theirs.is_empty() {
                            p.cp-none { "Nothing yet. Their changes on the dashboard show up here." }
                        } @else {
                            div.cp-list {
                                @for (at, g, key, action) in &theirs {
                                    @let f = ctx.feature(key);
                                    @let ic = f.and_then(|f| f.meta().nav.first().map(|n| n.icon)).unwrap_or(match key.as_str() { "users" => Icon::Users, _ => Icon::Toggle });
                                    @let page_name = match key.as_str() { "features" => "Features", "users" => "People", _ => f.map(|f| f.meta().name).unwrap_or("Dashboard") };
                                    div.cp-row {
                                        span.cp-ic { (ui::icon(ic)) }
                                        span.cp-txt {
                                            span.cp-line { (action) }
                                            span {
                                                @if let Some(g) = g.filter(|_| ctx.guild_ids().len() > 1) { (ctx.guild_name(g as u64)) " · " }
                                                (page_name) " · " (text::ago(*at))
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } @else {
                    @for (i, g) in ctx.guild_ids().into_iter().enumerate() {
                        @let feats = grantable(g);
                        @let on = feats.iter().filter(|f| level_of(g, f.meta().key) > 0).count();
                        section.cp-panel {
                            div.cp-server {
                                (ui::guild_avatar(&ctx, g))
                                h2.grow { (ctx.guild_name(g)) }
                                @if !feats.is_empty() { span.muted { (on) " of " (feats.len()) " pages" } }
                            }
                            @if i == 0 { p.cp-note { "None hides a page, View shows it read-only, Edit lets them change it." } }
                            @if feats.is_empty() {
                                p.cp-none { "No features are on here. Turn some on in Features first." }
                            } @else {
                                div.cp-grants {
                                    @for f in &feats {
                                        @let m = f.meta();
                                        @let name = format!("p:{}:{}", g, m.key);
                                        @let cur = level_of(g, m.key);
                                        div.cp-grant {
                                            span.cp-ic { @if let Some(n) = m.nav.first() { (ui::icon(n.icon)) } }
                                            span.grow.clip { (m.name) }
                                            div.seg role="radiogroup" aria-label=(m.name) {
                                                @for (v, label) in [(0, "None"), (1, "View"), (2, "Edit")] {
                                                    label {
                                                        input type="radio" name=(name) value=(v) checked[cur == v];
                                                        (label)
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            aside.cp-side {
                section.cp-panel.cp-who {
                    img.avatar src=(crate::discord::avatar_url(person.id, person.avatar.as_deref())) alt="" width="44" height="44" loading="lazy";
                    span.cp-txt {
                        strong { (display) }
                        span { (seen) @if let Some(by) = person.added_by.as_ref().filter(|_| !is_owner) { " · added by " (by) } }
                        span {
                            @if is_owner { "The owner" }
                            @else if person.admin_plus { "Admin+, every page" }
                            @else if person.admin { "Admin, every page" }
                            @else if given == 0 { "No pages yet" }
                            @else { (given) @if given == 1 { " page" } @else { " pages" } ", " (editable) " to edit" }
                        }
                    }
                }
                section.cp-panel {
                    h2 { "Role" }
                    div.cp-fields {
                        @if is_owner {
                            (ui::field_custom("Admin", Some("The owner is always an admin, with App approvals."), ui::badge("Owner", Tone::Muted)))
                        } @else if person.admin_plus && !viewer_is_owner {
                            (ui::field_custom("Admin+", Some("Only the owner changes an Admin+ person's access."), ui::badge("Admin+", Tone::Muted)))
                        } @else {
                            (ui::field_toggle("admin", "Admin", Some("Everything on every server, including features, people and the activity log, except App approvals."), person.admin))
                            @if viewer_is_owner {
                                (ui::field_toggle("admin_plus", "Admin+", Some("Admin, plus App approvals, which is otherwise yours alone. On, it keeps them an admin whatever the switch above says. Only you can give it."), person.admin_plus))
                            }
                        }
                    }
                    div.save-bar { button.btn.primary type="submit" { "Save" } }
                }
                @if !is_owner {
                    section.cp-panel {
                        h2 { "Remove" }
                        p.cp-note { "Signs them out and removes all their access." }
                        div { button.btn.small.danger type="submit" formaction={ "/users/" (id) "/delete" } data-confirm="Click again to confirm" { "Remove from dashboard" } }
                    }
                }
            }
        }
    };
    let title = if person.name.is_empty() { "Access".to_owned() } else { format!("{display}'s access") };
    let actions = html! {
        @if !is_owner && !person.admin && !others.is_empty() {
            (ui::pop("", "Same as…", None, "small outline", html! {
                form.stack method="post" action={ "/users/" (id) "/copy" } {
                    (ui::select("from", &others, ""))
                    p.muted.small { "Replaces their pages on every server with that person's. Admin stays as it is." }
                    div { button.btn.primary.small type="submit" { "Copy access" } }
                }
            }))
        }
        a.btn.small.outline.wide-only href="/users" { "All people" }
    };
    Ok(ui::layout(&ctx, &page, &title, Some(actions), body).into_response())
}

async fn save_user(State(ctx): State<Ctx>, user: User, Path(id): Path<u64>, form: FormData) -> WebResult {
    let guild = any_guild(&ctx, &user)?;
    admin_page(&ctx, &user, guild, "users")?;
    let is_owner = id == ctx.cfg.owner_id;
    let by_owner = user.id == ctx.cfg.owner_id;
    let was_plus = people(&ctx).iter().any(|p| p.id == id && p.admin_plus);
    if was_plus && !by_owner {
        return Err(WebError::Bad("Only the owner changes an Admin+ person's access.".into()));
    }
    // Admin+ is the owner's to give, and always comes with admin.
    let plus = if by_owner { form.bool("admin_plus") } else { was_plus };
    let admin = is_owner || plus || form.bool("admin");
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    tx.execute("UPDATE dash_users SET admin = ?2, admin_plus = ?3 WHERE id = ?1", params![id as i64, admin, plus && !is_owner])?;
    if !admin {
        for (k, v) in &form.0 {
            let Some(rest) = k.strip_prefix("p:") else { continue };
            let Some((g, page_key)) = rest.split_once(':') else { continue };
            let (Ok(g), Ok(level)) = (g.parse::<u64>(), v.parse::<i64>()) else { continue };
            if ctx.feature(page_key).is_none() {
                continue;
            }
            tx.execute(
                "INSERT INTO dash_perms (user_id, guild_id, page, level) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (user_id, guild_id, page) DO UPDATE SET level = excluded.level",
                params![id as i64, g as i64, page_key, level.clamp(0, 2)],
            )?;
        }
    }
    tx.commit()?;
    drop(conn);
    ctx.audit(&user, None, "users", format!("Changed {}'s access", person_name(&ctx, id)));
    Ok(saved(&format!("/users/{id}")))
}

/// Give someone exactly the pages another person has, on every server.
async fn copy_access(State(ctx): State<Ctx>, user: User, Path(id): Path<u64>, form: FormData) -> WebResult {
    let guild = any_guild(&ctx, &user)?;
    admin_page(&ctx, &user, guild, "users")?;
    let list = people(&ctx);
    let target = list.iter().find(|p| p.id == id).ok_or(WebError::NotFound)?;
    let from = form.id("from").and_then(|f| list.iter().find(|p| p.id == f)).ok_or_else(|| WebError::Bad("Pick whose access to copy.".into()))?;
    if from.id == id || id == ctx.cfg.owner_id {
        return Err(WebError::Bad("Pick someone else to copy from.".into()));
    }
    // Same rule as saving their access by hand: an Admin+ person's grants are the owner's to set.
    if target.admin_plus && user.id != ctx.cfg.owner_id {
        return Err(WebError::Bad("Only the owner changes an Admin+ person's access.".into()));
    }
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM dash_perms WHERE user_id = ?1", [id as i64])?;
    tx.execute(
        "INSERT INTO dash_perms (user_id, guild_id, page, level) SELECT ?1, guild_id, page, level FROM dash_perms WHERE user_id = ?2",
        params![id as i64, from.id as i64],
    )?;
    tx.commit()?;
    drop(conn);
    ctx.audit(&user, None, "users", format!("Gave {} the same access as {}", name_or_id(target), name_or_id(from)));
    Ok(saved(&format!("/users/{id}")))
}

fn name_or_id(p: &Person) -> String {
    if p.name.is_empty() { p.id.to_string() } else { p.name.clone() }
}

/// A dashboard person's name for the activity log; their id until they've signed in.
fn person_name(ctx: &Ctx, id: u64) -> String {
    ctx.db
        .with(|c| c.query_row("SELECT name FROM dash_users WHERE id = ?1 AND name != ''", [id as i64], |r| r.get::<_, String>(0)))
        .unwrap_or_else(|_| id.to_string())
}

async fn delete_user(State(ctx): State<Ctx>, user: User, Path(id): Path<u64>) -> WebResult {
    let guild = any_guild(&ctx, &user)?;
    admin_page(&ctx, &user, guild, "users")?;
    if id == ctx.cfg.owner_id {
        return Err(WebError::Bad("The owner can't be removed.".into()));
    }
    if user.id != ctx.cfg.owner_id && people(&ctx).iter().any(|p| p.id == id && p.admin_plus) {
        return Err(WebError::Bad("Only the owner removes an Admin+ person.".into()));
    }
    let name = person_name(&ctx, id);
    ctx.db.with(|c| {
        c.execute("DELETE FROM dash_perms WHERE user_id = ?1", [id as i64])?;
        c.execute("DELETE FROM dash_sessions WHERE user_id = ?1", [id as i64])?;
        c.execute("DELETE FROM dash_users WHERE id = ?1", [id as i64])
    })?;
    ctx.audit(&user, None, "users", format!("Removed {name} from the dashboard"));
    Ok(saved("/users"))
}

// ---- activity ----

async fn activity(State(ctx): State<Ctx>, user: User) -> WebResult<Response> {
    let guild = any_guild(&ctx, &user)?;
    let page = admin_page(&ctx, &user, guild, "activity")?;
    struct Change {
        at: i64,
        who: u64,
        name: String,
        avatar: Option<String>,
        guild: Option<i64>,
        feature: String,
        action: String,
    }
    let rows: Vec<Change> = ctx.db.with(|c| {
        let mut s = c.prepare(
            "SELECT a.at, a.user_id, a.user_name, u.avatar, a.guild_id, a.feature, a.action
             FROM dash_audit a LEFT JOIN dash_users u ON u.id = a.user_id ORDER BY a.at DESC LIMIT 200",
        )?;
        let rows = s
            .query_map([], |r| {
                Ok(Change {
                    at: r.get(0)?,
                    who: r.get::<_, i64>(1)? as u64,
                    name: r.get(2)?,
                    avatar: r.get(3)?,
                    guild: r.get(4)?,
                    feature: r.get(5)?,
                    action: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })?;
    // App approvals' own actions are for the owner and Admin+ only, like the page.
    let hidden = |feature: &str| !user.admin_plus && ctx.feature(feature).is_some_and(|f| f.admin_plus_only());
    let rows: Vec<Change> = rows.into_iter().filter(|r| !hidden(&r.feature)).collect();
    // Every change in the last 14 days for the chart, not just the 200 listed.
    let since = crate::now_ms() - 15 * 86_400_000;
    let recent: Vec<i64> = ctx
        .db
        .with(|c| {
            let mut s = c.prepare("SELECT at, feature FROM dash_audit WHERE at >= ?1")?;
            let rows = s.query_map([since], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, f)| !hidden(f))
        .map(|(at, _)| at)
        .collect();
    let per_day = per_day(&user, &recent, 14);
    let fortnight: i64 = per_day.iter().filter_map(|d| d.1).sum();
    let week_ago = crate::now_ms() - 7 * 86_400_000;
    let mut week: Vec<(u64, String, Option<String>, i64)> = Vec::new();
    let mut pages: Vec<(String, i64)> = Vec::new();
    for r in rows.iter().filter(|r| r.at >= week_ago) {
        match week.iter_mut().find(|w| w.0 == r.who) {
            Some(w) => w.3 += 1,
            None => week.push((r.who, r.name.clone(), r.avatar.clone(), 1)),
        }
        match pages.iter_mut().find(|p| p.0 == r.feature) {
            Some(p) => p.1 += 1,
            None => pages.push((r.feature.clone(), 1)),
        }
    }
    week.sort_by_key(|w| -w.3);
    pages.sort_by_key(|p| -p.1);
    pages.truncate(6);
    let top = week.first().map(|w| w.3).unwrap_or(1).max(1);
    let top_page = pages.first().map(|p| p.1).unwrap_or(1).max(1);
    let day_label = |at: i64| day_label(&user, at);
    let page_name = |key: &str| match key {
        "features" => "Features",
        "users" => "People",
        k => ctx.feature(k).map(|f| f.meta().name).unwrap_or("Dashboard"),
    };
    let body = html! {
        @if rows.is_empty() {
            (ui::empty(Icon::Activity, "No changes yet", "Every change made on the dashboard is listed here, with who made it."))
        } @else {
            div.cp-cols {
                div.cp-main {
                    section.cp-panel {
                        h2 { "Changes" span.muted { (rows.len()) @if rows.len() == 200 { " newest" } } }
                        div.cp-days data-local {
                            @for (i, r) in rows.iter().enumerate() {
                                @let label = day_label(r.at);
                                @if i == 0 || day_label(rows[i - 1].at) != label { h3.cp-glabel { (label) } }
                                div.cp-row.cp-change {
                                    span.cp-time { (user.local(r.at).strftime("%H:%M")) }
                                    img.avatar src=(crate::discord::avatar_url(r.who, r.avatar.as_deref())) alt="" width="28" height="28" loading="lazy";
                                    span.cp-line.grow { strong { (r.name) } " " (lower_first(&r.action)) }
                                    span.cp-where {
                                        @if let Some(g) = r.guild.filter(|_| ctx.guild_ids().len() > 1) { (ctx.guild_name(g as u64)) " · " }
                                        (page_name(&r.feature))
                                    }
                                }
                            }
                        }
                    }
                }
                aside.cp-side.cp-stick {
                    section.cp-panel {
                        h2 { "Last 14 days" span.muted { (fortnight) @if fortnight == 1 { " change" } @else { " changes" } } }
                        (ui::column_bars(&per_day, "changes", "Dashboard changes on each of the last 14 days"))
                    }
                    section.cp-panel {
                        h2 { "Who changed things this week" }
                        @if week.is_empty() { p.cp-none { "Nobody changed anything in the last 7 days." } }
                        div.cp-list {
                            @for (who, name, avatar, n) in &week {
                                div.cp-row.cp-bar-row {
                                    img.avatar src=(crate::discord::avatar_url(*who, avatar.as_deref())) alt="" width="28" height="28" loading="lazy";
                                    span.cp-txt { strong { (name) } span.meter { span style={ "width:" (n * 100 / top) "%" } {} } }
                                    span.cp-n { (n) }
                                }
                            }
                        }
                    }
                    @if !pages.is_empty() {
                        section.cp-panel {
                            h2 { "Pages changed this week" }
                            div.cp-list {
                                @for (key, n) in &pages {
                                    @let ic = ctx.feature(key).and_then(|f| f.meta().nav.first().map(|n| n.icon)).unwrap_or(match key.as_str() { "users" => Icon::Users, _ => Icon::Toggle });
                                    div.cp-row.cp-bar-row {
                                        span.cp-ic { (ui::icon(ic)) }
                                        span.cp-txt { strong { (page_name(key)) } span.meter { span style={ "width:" (n * 100 / top_page) "%" } {} } }
                                        span.cp-n { (n) }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Activity", None, body).into_response())
}

/// How many of `ats` fell on each of the last `days` days in the viewer's timezone, oldest
/// first, labelled "Oct 3", for [`ui::column_bars`].
fn per_day(user: &User, ats: &[i64], days: i64) -> Vec<(String, Option<i64>)> {
    let today = user.local(crate::now_ms()).date();
    let mut counts = vec![0i64; days as usize];
    for at in ats {
        let back = (today - user.local(*at).date()).get_days() as i64;
        if (0..days).contains(&back) {
            counts[(days - 1 - back) as usize] += 1;
        }
    }
    (0..days)
        .map(|i| {
            let d = today.checked_sub(jiff::Span::new().days(days - 1 - i)).unwrap_or(today);
            (d.strftime("%b %-d").to_string(), Some(counts[i as usize]))
        })
        .collect()
}

/// "Turned Starboard on" reads as "turned Starboard on" after a name.
fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) if !c.as_str().starts_with(|x: char| x.is_uppercase()) => f.to_lowercase().chain(c).collect(),
        _ => s.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::mix_feed;
    use crate::Happening;

    fn h(at: i64, who: &str) -> Happening {
        Happening { at, avatar: String::new(), who: who.into(), text: String::new() }
    }

    #[test]
    fn a_busy_source_shares_the_feed() {
        let levels: Vec<_> = (0..10).map(|i| h(100 + i, "levels")).collect();
        let welcome = vec![h(5, "welcome"), h(3, "welcome")];
        let pets = vec![h(1, "pets")];
        let feed = mix_feed(vec![levels, welcome, pets, vec![]], 6);
        let who: Vec<_> = feed.iter().map(|h| h.who.as_str()).collect();
        assert_eq!(feed.len(), 6);
        assert_eq!(who.iter().filter(|w| **w == "levels").count(), 3);
        assert!(who.contains(&"pets") && who.contains(&"welcome"));
        assert!(feed.windows(2).all(|w| w[0].at >= w[1].at), "newest first");
    }

    #[test]
    fn short_feeds_are_kept_whole() {
        let feed = mix_feed(vec![vec![h(2, "a")], vec![h(1, "b")]], 6);
        assert_eq!(feed.len(), 2);
    }
}
