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
    let show = q.get("show").map(String::as_str).unwrap_or("messages");
    // Every series in one read of the last 30 days, then cut to length.
    let [messages, joins, leaves, active, voice]: [Vec<(String, i64)>; 5] =
        daily_keys(&ctx, guild, &["messages", "joins", "leaves", "active", "voice_minutes"], 30).try_into().unwrap_or_default();
    let tail = |s: &[(String, i64)], n: i64| s[s.len().saturating_sub(n as usize)..].to_vec();
    let joins_long = joins.clone();
    let (messages, joins, leaves, active, voice) = (tail(&messages, days), tail(&joins, 14), tail(&leaves, 7), tail(&active, days), tail(&voice, days.max(14)));
    let last = |s: &[(String, i64)]| s.last().map(|d| d.1).unwrap_or(0);
    let sum_last = |s: &[(String, i64)], n: usize| s.iter().rev().take(n).map(|d| d.1).sum::<i64>();
    let members = ctx.member_count(guild).map(|n| n as i64);
    let net = sum_last(&joins, 7) - sum_last(&leaves, 7);
    let today_msgs = last(&messages);
    let now = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC);
    let day_share = (now.hour() as f64 * 60.0 + now.minute() as f64) / 1440.0;
    let pace = if day_share > 0.08 { Some((today_msgs as f64 / day_share) as i64) } else { None };
    // Daily numbers are counted per UTC day. Anyone in another timezone is told where it starts.
    let day_starts = now
        .start_of_day()
        .ok()
        .map(|m| user.local(m.timestamp().as_millisecond()))
        .filter(|l| l.offset() != jiff::tz::Offset::UTC)
        .map(|l| l.strftime("%H:%M").to_string());
    let active_today = last(&active);
    let voice_week = sum_last(&voice, 7);
    let voice_prev = voice.iter().rev().skip(7).take(7).map(|d| d.1).sum::<i64>();
    let hours = |m: i64| format!("{} h", text::num((m as f64 / 60.0).round() as i64));
    let signed = |n: i64, unit: &str| if n >= 0 { format!("+{} {unit}", text::num(n)) } else { format!("{} {unit}", text::num(n)) };

    struct Stat { key: &'static str, label: &'static str, value: String, note: String, up: bool, down: bool }
    let stats = [
        Stat { key: "members", label: "Members", value: members.map(text::num).unwrap_or_else(|| "…".into()), note: signed(net, "this week"), up: net > 0, down: net < 0 },
        Stat {
            key: "messages",
            label: "Messages today",
            value: text::num(today_msgs),
            note: match pace {
                Some(p) if today_msgs > 0 => format!("On pace for {}", text::num(p)),
                _ => "Since midnight UTC".into(),
            },
            up: false,
            down: false,
        },
        Stat {
            key: "active",
            label: "Active today",
            value: text::num(active_today),
            note: members.filter(|m| *m > 0).map(|m| format!("{}% of members", (active_today * 100 / m).min(100))).unwrap_or_default(),
            up: false,
            down: false,
        },
        Stat {
            key: "voice",
            label: "Voice this week",
            value: hours(voice_week),
            note: format!("{} on last week", if voice_week >= voice_prev { format!("+{}", hours(voice_week - voice_prev)) } else { format!("-{}", hours(voice_prev - voice_week)) }),
            up: voice_week > voice_prev,
            down: voice_week < voice_prev,
        },
    ];
    let (series, unit, caption): (Vec<(String, i64)>, &str, &str) = match show {
        "members" => (tail(&joins_long, days), "joins", "Joins per day"),
        "active" => (active.clone(), "members", "Active members per day"),
        "voice" => (voice.iter().rev().take(days as usize).rev().map(|(l, m)| (l.clone(), m / 60)).collect(), "hours", "Voice hours per day"),
        _ => (messages.clone(), "messages", "Messages per day"),
    };
    let show = if ["members", "active", "voice"].contains(&show) { show } else { "messages" };
    let avg = series.iter().map(|d| d.1).sum::<i64>() / series.len().max(1) as i64;

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
        let hue = ui::feature_hue(&ctx, key);
        needs.extend(o.needs.into_iter().map(|n| (hue, n)));
        feeds.push(o.feed);
        panel.extend(o.panel);
    }
    // Alerts first, then whatever has waited longest.
    needs.sort_by_key(|(_, n)| (!n.alert, n.since));
    let feed = mix_feed(feeds, 6);
    panel.sort_by_key(|p| p.0);
    let on = ctx.features().iter().filter(|f| ctx.enabled(guild, f.meta().key)).count();

    let hour = user.local(crate::now_ms()).hour();
    let greeting = match hour {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        _ => "Good evening",
    };
    let name = ctx.guild_name(guild);
    let banner = ctx.guild_banner_url(guild);
    let icon_large = ctx.guild_icon_large(guild);
    let link = |show: &str, days: i64| format!("/g/{guild}?show={show}&days={days}");

    let main = html! {
        nav.ov-stats aria-label="Readings" {
            @for st in &stats {
                a.ov-stat.on[st.key == show] href=(link(st.key, days)) aria-current=[(st.key == show).then_some("true")] data-swap="#ov-caption, #ov-plot" {
                    span.muted.small { (st.label) }
                    b data-count { (st.value) }
                    span.small.up[st.up].down[st.down] { (st.note) }
                    @if st.key == show { span.stat-line {} }
                }
            }
        }
        div.ov-chart {
            div.ov-chart-head {
                div #ov-caption {
                    h2 { (caption) }
                    p {
                        (text::num(avg)) " on average over " (days) " days"
                        @if let Some(t) = &day_starts { " · days start at " (t) " your time" }
                    }
                }
                div.seg {
                    a.on[days == 7] href=(link(show, 7)) data-swap="#ov-caption, #ov-plot" { "7 days" }
                    a.on[days == 30] href=(link(show, 30)) data-swap="#ov-caption, #ov-plot" { "30 days" }
                }
            }
            div #ov-plot {
                @if series.iter().all(|d| d.1 == 0) {
                    (ui::empty(Icon::Chart, "Nothing counted in these days", "The bot counts as things happen, and the chart fills in day by day."))
                } @else {
                    (ui::area_chart(&series, unit))
                }
            }
        }
        // Side by side once the column is wide enough (app.css, .ov-pair).
        div.ov-pair {
            section.ov-block {
                h2 { "Needs you" @if !needs.is_empty() { span.count-pill { (needs.len()) } } }
                @if needs.is_empty() {
                    (ui::empty(Icon::Check, "Nothing is waiting on you", "Scam catches, pet requests, tickets and app approvals land here when they need someone."))
                } @else {
                    div.ov-rows {
                        @for (hue, n) in &needs {
                            div.need.alert[n.alert] data-hue=(hue) {
                                @if n.people.is_empty() {
                                    span.need-icon.alert[n.alert] { (ui::icon(n.icon)) }
                                } @else {
                                    (ui::avatars(&n.people, 2))
                                }
                                div.grow { strong { (n.title) } p.muted.small { (n.detail) } }
                                a class=(if n.alert { "btn small primary" } else { "btn small outline" }) href=(n.href) { (n.action) }
                            }
                        }
                    }
                }
            }
            section.ov-block {
                h2 { "Around the server" }
                @if feed.is_empty() {
                    (ui::empty(Icon::Activity, "Quiet so far", "Level-ups, new members, pet additions and suggestion results show up here."))
                } @else {
                    div.ov-feed {
                        @for h in &feed {
                            div.happen {
                                img.avatar src=(h.avatar) alt="" width="32" height="32" loading="lazy";
                                span.grow { strong { (h.who) } " " (h.text) }
                                span.muted.small { (short_ago(h.at)) }
                            }
                        }
                    }
                }
            }
        }
        @if user.admin && on == 0 {
            section.ov-block {
                h2 { "Get started" }
                p.muted { "Every feature starts off in a new server. Switch on what this server should have." }
                div { a.btn.primary href={ "/g/" (guild) "/features" } { "Choose features" } }
            }
        }
    };

    let uploaded = super::banner::uploaded_at(&ctx, guild).is_some();
    let body = html! {
        header.ov-hero data-local {
            div.ov-bg {
                @match (&banner, &icon_large) {
                    (Some(b), _) => img src=(b) alt="" fetchpriority="high";,
                    (None, i) => (ui::art(i.as_deref(), false)),
                }
            }
            div.ov-top {
                @if ctx.cfg.preview {
                    (ui::status(ui::Dot::Waiting, "Preview"))
                } @else if ctx.bot_id().is_some() {
                    (ui::status(ui::Dot::Live, "Online"))
                } @else {
                    (ui::status(ui::Dot::Waiting, "Connecting"))
                }
                @if user.admin {
                    details.pop.banner-pop {
                        summary.btn.small aria-label="Change the banner" { (ui::icon(Icon::Image)) span.pop-label { "Banner" } }
                        div.pop-body {
                            strong { "The picture behind this header" }
                            p.muted.small {
                                @if uploaded { "You uploaded this one. It also heads the sidebar." }
                                @else if banner.is_some() { "This is the banner from Discord's server settings. An upload replaces it here." }
                                @else { "No banner yet, so this is drawn from the server icon. Upload a wide picture, at least 1600 px across." }
                            }
                            div.inline-form {
                                button.btn.small.primary type="button" data-banner-pick { (ui::icon(Icon::Image)) "Upload a picture" }
                                @if uploaded {
                                    form.inline method="post" action={ "/g/" (guild) "/banner/remove" } {
                                        button.btn.small.ghost type="submit" { "Remove upload" }
                                    }
                                }
                            }
                            input #banner-file type="file" accept="image/png,image/jpeg,image/webp,image/gif" hidden data-action={ "/g/" (guild) "/banner" };
                        }
                    }
                }
            }
            div.ov-hello {
                @match &icon_large {
                    Some(i) => img.ov-icon src=(i) alt="" width="84" height="84";,
                    None => span.ov-icon.letter { (name.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default()) },
                }
                div {
                    h1 { (greeting) ", " (user.name) }
                    p {
                        span { strong { (name) } }
                        @if let Some(m) = members { span { (ui::icon(Icon::Member)) (text::num(m)) " members" } }
                        @if active_today > 0 { span { span.dot.live {} (text::num(active_today)) " here today" } }
                    }
                }
            }
        }
        div.page-body.ov-body {
            @if panel.is_empty() {
                div.ov-main { (main) }
            } @else {
                div.with-rail {
                    div.ov-main { (main) }
                    aside.rail.ov-rail { @for (_, block) in &panel { div.rail-block { (block.clone()) } } }
                }
            }
        }
    };
    Ok(ui::layout_custom(&ctx, &page, &name, body).into_response())
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
/// server-less notices (app connections) only for admins.
fn recent_notifications(ctx: &Ctx, user: &User, guild: u64, limit: i64) -> Vec<Note> {
    let mut notes = all_notifications(ctx, guild, limit * 4);
    let levels = ctx.levels(user, guild);
    notes.retain(|n| match n.global {
        true => user.admin,
        false => levels.get(&n.feature) >= Level::View,
    });
    notes.truncate(limit as usize);
    notes
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
    let body = html! {
        @if notes.is_empty() {
            (ui::empty(Icon::Bell, "No notifications", "Features post here when something needs you: a caught scam, a new ticket, a pet request."))
        } @else {
            div.with-rail {
                // No data-local: its one-time reload for the timezone would count as a second
                // visit and clear the new marks. The overview, where everyone lands, sets it.
                div.timeline.notes {
                    @for (i, n) in notes.iter().enumerate() {
                        @let label = day_label(&user, n.at);
                        @if i == 0 || day_label(&user, notes[i - 1].at) != label { h2.day { (label) } }
                        (notification_row(&ctx, &user, n, is_new(n)))
                    }
                }
                aside.rail {
                    div.rail-block {
                        h2 { "Where they came from" span.muted.small { (notes.len()) } }
                        @for (key, total, new) in &from {
                            @let f = ctx.feature(key);
                            div.mini data-hue=(ui::feature_hue(&ctx, key)) {
                                span.nav-ic { (ui::icon(f.and_then(|f| f.meta().nav.first().map(|n| n.icon)).unwrap_or(Icon::Bell))) }
                                span.grow {
                                    span { (f.map(|f| f.meta().name).unwrap_or("Dashboard")) }
                                    span.muted { @if *new > 0 { (new) " new" } @else { "Nothing new" } }
                                }
                                span.mono { (total) }
                            }
                        }
                    }
                }
            }
        }
    };
    // Seen from now on, so the bell on this page is already clear.
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO dash_notif_seen (user_id, guild_id, seen_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (user_id, guild_id) DO UPDATE SET seen_at = excluded.seen_at",
            params![user.id as i64, guild as i64, crate::now_ms()],
        )
    })?;
    let actions = (fresh > 0).then(|| ui::badge(&format!("{fresh} new"), Tone::Accent));
    Ok(ui::layout(&ctx, &page, "Notifications", actions, body).into_response())
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
    let mut feats: Vec<_> = ctx.features().iter().map(|f| f.meta()).collect();
    feats.sort_by_key(|m| m.group);
    let mut groups: Vec<(crate::Group, Vec<crate::FeatureMeta>)> = Vec::new();
    for m in feats {
        match groups.iter_mut().find(|(g, _)| *g == m.group) {
            Some((_, list)) => list.push(m),
            None => groups.push((m.group, vec![m])),
        }
    }
    let guilds = ctx.guilds_for(&user);
    let count_on = |g: u64| ctx.features().iter().filter(|f| ctx.enabled(g, f.meta().key)).count();
    let body = html! {
        @if guilds.len() > 1 {
            nav.server-pick aria-label="Server" {
                @for g in &guilds {
                    a.on[*g == guild] href={ "/g/" (g) "/features" } {
                        (ui::guild_avatar(&ctx, *g)) span { (ctx.guild_name(*g)) } span.mono.muted { (count_on(*g)) " on" }
                    }
                }
            }
        }
        div.fgroups {
            @for (group, list) in &groups {
                section.card.fgroup data-hue=(group.hue()) {
                    h2.fgroup-label { (group.label()) }
                    @for m in list {
                        @let on = ctx.enabled(guild, m.key);
                        form.frow.off[!on] method="post" action={ "/g/" (guild) "/features/" (m.key) } {
                            @if let Some(n) = m.nav.first() { (ui::icon(n.icon)) }
                            label.grow for={ "on-" (m.key) } { strong { (m.name) } span.help { (m.description) } }
                            label.switch {
                                input type="checkbox" id={ "on-" (m.key) } name={ "on-" (m.key) } checked[on] data-autosubmit;
                                span.track { span.thumb {} }
                            }
                        }
                    }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Features", None, body).into_response())
}

async fn toggle_feature(State(ctx): State<Ctx>, user: User, Path((guild, key)): Path<(u64, String)>, form: FormData) -> WebResult {
    admin_page(&ctx, &user, guild, "features")?;
    let feature = ctx.feature(&key).ok_or(WebError::NotFound)?;
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
    created_at: i64,
    grants: i64,
    signed_in: Option<i64>,
    added_by: Option<String>,
}

fn people(ctx: &Ctx) -> Vec<Person> {
    ctx.db
        .with(|c| {
            let mut s = c.prepare(
                "SELECT u.id, u.name, u.avatar, u.admin, u.created_at,
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
                        grants: r.get(5)?,
                        signed_in: r.get(6)?,
                        added_by: r.get(7)?,
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
    let body = html! {
        div.readings {
            div.reading { b data-count { (list.len()) } span { "people can sign in" } }
            div.reading { b data-count { (admins) } span { "see everything" } }
            div.reading { b data-count { (today) } span { "signed in today" } }
        }
        div.people {
            @for p in &list {
                @let seen = match p.signed_in {
                    Some(at) if p.id == user.id && crate::now_ms() - at < 3_600_000 => "Signed in now".to_owned(),
                    Some(at) => format!("Signed in {}", text::ago(at)),
                    None => format!("Added {} · hasn't signed in yet", text::ago(p.created_at)),
                };
                @let online = p.signed_in.is_some_and(|at| crate::now_ms() - at < 3_600_000);
                @let pages = access.get(&p.id).map(Vec::as_slice).unwrap_or_default();
                div.prow {
                    span.prow-face {
                        img.avatar src=(crate::discord::avatar_url(p.id, p.avatar.as_deref())) alt="" width="40" height="40" loading="lazy";
                        @if online { span.dot.live title="Signed in within the hour" {} }
                    }
                    div.grow {
                        strong { (name_or_id(p)) }
                        p.muted.small { (seen) @if let Some(by) = p.added_by.as_ref().filter(|_| p.id != owner) { " · added by " (by) } }
                    }
                    @if p.id == owner { (ui::badge("Owner", Tone::Accent)) }
                    @else if p.admin { (ui::badge("Admin, every page", Tone::Muted)) }
                    @else if pages.is_empty() { (ui::badge("No pages yet", Tone::Warn)) }
                    @else {
                        span.access-tiles {
                            @for (key, edit) in pages.iter().take(7) {
                                @let f = ctx.feature(key);
                                @let label = f.and_then(|f| f.meta().nav.first().map(|n| n.label)).unwrap_or(key.as_str());
                                @let ic = f.and_then(|f| f.meta().nav.first().map(|n| n.icon)).unwrap_or(Icon::Toggle);
                                span.nav-ic.edit[*edit] data-hue=(ui::feature_hue(&ctx, key)) data-tip={ (label) ": " (if *edit { "can edit" } else { "can view" }) } { (ui::icon(ic)) }
                            }
                            @if pages.len() > 7 { span.more { "+" (pages.len() - 7) } }
                        }
                    }
                    @if p.id != owner { a.btn.small.outline href={ "/users/" (p.id) } { "Edit access" } }
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
    let display = if person.name.is_empty() { "Not signed in yet".to_owned() } else { person.name.clone() };
    let grantable = |g: u64| ctx.features().iter().filter(|f| ctx.enabled(g, f.meta().key) && !f.admin_only()).collect::<Vec<_>>();
    // People whose pages can be copied over: not admins (they have no grants to copy), not this one.
    let others: Vec<(String, String)> = people(&ctx)
        .into_iter()
        .filter(|p| p.id != id && !p.admin && p.id != ctx.cfg.owner_id && p.grants > 0)
        .map(|p| (p.id.to_string(), format!("{} · {} {}", name_or_id(&p), p.grants, if p.grants == 1 { "page" } else { "pages" })))
        .collect();

    let body = html! {
        form.settings method="post" action={ "/users/" (id) } {
            (ui::card(html! {
                @if is_owner {
                    (ui::field_custom("Admin", Some("The owner is always an admin."), ui::badge("Owner", Tone::Accent)))
                } @else {
                    (ui::field_toggle("admin", "Admin", Some("Everything on every server, including features, people and the activity log."), person.admin))
                }
            }))
            @if !person.admin && !is_owner {
                p.muted.small { "Per server: None hides a page, View shows it read-only, Edit lets them change it." }
                div.access-grid {
                @for g in ctx.guild_ids() {
                    @let feats = grantable(g);
                    @let given = feats.iter().filter(|f| level_of(g, f.meta().key) > 0).count();
                    section.block {
                        div.access-head {
                            (ui::guild_avatar(&ctx, g))
                            h2.grow { (ctx.guild_name(g)) }
                            @if !feats.is_empty() { span.muted.small { (given) " of " (feats.len()) " pages" } }
                        }
                        (ui::card(html! {
                        @for f in &feats {
                            @let m = f.meta();
                            @let name = format!("p:{}:{}", g, m.key);
                            @let cur = level_of(g, m.key);
                            (ui::field_custom(m.name, None, html! {
                                div.seg role="radiogroup" aria-label=(m.name) {
                                    @for (v, label) in [(0, "None"), (1, "View"), (2, "Edit")] {
                                        label {
                                            input type="radio" name=(name) value=(v) checked[cur == v];
                                            (label)
                                        }
                                    }
                                }
                            }))
                        }
                        @if feats.is_empty() {
                            (ui::empty(Icon::Toggle, "No features on here", "Turn features on for this server first."))
                        }
                        }))
                    }
                }
                }
            }
            div.save-bar { button.btn.primary type="submit" { "Save" } }
        }
        @if !is_owner {
            (ui::section("Remove", None, html! {
                p.muted { "Signs them out and removes all their access." }
                div { (ui::action_button(&format!("/users/{id}/delete"), "Remove from dashboard", Tone::Bad, true)) }
            }))
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
    let admin = is_owner || form.bool("admin");
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    tx.execute("UPDATE dash_users SET admin = ?2 WHERE id = ?1", params![id as i64, admin])?;
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
    let week_ago = crate::now_ms() - 7 * 86_400_000;
    let mut week: Vec<(u64, String, Option<String>, i64)> = Vec::new();
    for r in rows.iter().filter(|r| r.at >= week_ago) {
        match week.iter_mut().find(|w| w.0 == r.who) {
            Some(w) => w.3 += 1,
            None => week.push((r.who, r.name.clone(), r.avatar.clone(), 1)),
        }
    }
    week.sort_by_key(|w| -w.3);
    let top = week.first().map(|w| w.3).unwrap_or(1).max(1);
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
            div.with-rail {
                div.timeline data-local {
                    @for (i, r) in rows.iter().enumerate() {
                        @let label = day_label(r.at);
                        @if i == 0 || day_label(rows[i - 1].at) != label { h2.day { (label) } }
                        div.change {
                            span.mono.muted { (user.local(r.at).strftime("%H:%M")) }
                            img.avatar src=(crate::discord::avatar_url(r.who, r.avatar.as_deref())) alt="" width="26" height="26" loading="lazy";
                            span.grow { strong { (r.name) } " " (lower_first(&r.action)) }
                            span.muted.small.clip {
                                @if let Some(g) = r.guild.filter(|_| ctx.guild_ids().len() > 1) { (ctx.guild_name(g as u64)) " · " }
                                (page_name(&r.feature))
                            }
                        }
                    }
                }
                aside.rail {
                    h2 { "Changes this week" }
                    @if week.is_empty() { p.muted.small { "Nobody changed anything in the last 7 days." } }
                    @for (who, name, avatar, n) in &week {
                        div.rail-person {
                            img.avatar src=(crate::discord::avatar_url(*who, avatar.as_deref())) alt="" width="26" height="26" loading="lazy";
                            div.grow {
                                span { (name) }
                                span.meter { span style={ "width:" (n * 100 / top) "%" } {} }
                            }
                            span.mono.muted { (n) }
                        }
                    }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Activity", None, body).into_response())
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
