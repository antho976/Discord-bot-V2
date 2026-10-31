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
    let today = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC).date();
    let rows: Vec<(String, i64)> = ctx
        .db
        .with(|c| {
            let mut s = c.prepare("SELECT day, value FROM daily_stats WHERE guild_id = ?1 AND key = ?2 AND day >= ?3")?;
            let start = today.checked_sub(jiff::Span::new().days(days - 1)).unwrap_or(today).to_string();
            let rows = s.query_map(params![guild as i64, key, start], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap_or_default();
    (0..days)
        .rev()
        .map(|back| {
            let d = today.checked_sub(jiff::Span::new().days(back)).unwrap_or(today);
            let key = d.to_string();
            let v = rows.iter().find(|(k, _)| *k == key).map(|(_, v)| *v).unwrap_or(0);
            (d.strftime("%b %-d").to_string(), v)
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
    let messages = daily(&ctx, guild, "messages", days);
    let joins = daily(&ctx, guild, "joins", 14);
    let leaves = daily(&ctx, guild, "leaves", 7);
    let active = daily(&ctx, guild, "active", days);
    let voice = daily(&ctx, guild, "voice_minutes", days.max(14));
    let last = |s: &[(String, i64)]| s.last().map(|d| d.1).unwrap_or(0);
    let sum_last = |s: &[(String, i64)], n: usize| s.iter().rev().take(n).map(|d| d.1).sum::<i64>();
    let members = ctx.member_count(guild).map(|n| n as i64);
    let net = sum_last(&joins, 7) - sum_last(&leaves, 7);
    let today_msgs = last(&messages);
    let now = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC);
    let day_share = (now.hour() as f64 * 60.0 + now.minute() as f64) / 1440.0;
    let pace = if day_share > 0.08 { Some((today_msgs as f64 / day_share) as i64) } else { None };
    let active_today = last(&active);
    let voice_week = sum_last(&voice, 7);
    let voice_prev = voice.iter().rev().skip(7).take(7).map(|d| d.1).sum::<i64>();
    let hours = |m: i64| format!("{} h", text::num((m as f64 / 60.0).round() as i64));
    let signed = |n: i64, unit: &str| if n >= 0 { format!("+{} {unit}", text::num(n)) } else { format!("{} {unit}", text::num(n)) };

    struct Stat { key: &'static str, label: &'static str, value: String, note: String, up: bool }
    let stats = [
        Stat { key: "members", label: "Members", value: members.map(text::num).unwrap_or_else(|| "…".into()), note: signed(net, "this week"), up: net > 0 },
        Stat { key: "messages", label: "Messages today", value: text::num(today_msgs), note: pace.map(|p| format!("On pace for {}", text::num(p))).unwrap_or_else(|| "Since midnight UTC".into()), up: false },
        Stat {
            key: "active",
            label: "Active today",
            value: text::num(active_today),
            note: members.filter(|m| *m > 0).map(|m| format!("{}% of members", (active_today * 100 / m).min(100))).unwrap_or_default(),
            up: false,
        },
        Stat {
            key: "voice",
            label: "Voice this week",
            value: hours(voice_week),
            note: format!("{} on last week", if voice_week >= voice_prev { format!("+{}", hours(voice_week - voice_prev)) } else { format!("-{}", hours(voice_prev - voice_week)) }),
            up: voice_week > voice_prev,
        },
    ];
    let (series, unit, caption): (Vec<(String, i64)>, &str, &str) = match show {
        "members" => (daily(&ctx, guild, "joins", days), "joins", "Joins per day"),
        "active" => (active.clone(), "members", "Active members per day"),
        "voice" => (voice.iter().rev().take(days as usize).rev().map(|(l, m)| (l.clone(), m / 60)).collect(), "hours", "Voice hours per day"),
        _ => (messages.clone(), "messages", "Messages per day"),
    };
    let show = if ["members", "active", "voice"].contains(&show) { show } else { "messages" };
    let avg = series.iter().map(|d| d.1).sum::<i64>() / series.len().max(1) as i64;

    // What the features add, for the features this person can see.
    let mut needs = Vec::new();
    let mut feed = Vec::new();
    let mut panel = Vec::new();
    for f in ctx.features() {
        let key = f.meta().key;
        let level = ctx.level(&user, guild, key);
        if !ctx.enabled(guild, key) || level == Level::None {
            continue;
        }
        let fp = super::Page { user: user.clone(), guild_id: guild, key, level };
        let o = f.overview(&ctx, &fp);
        needs.extend(o.needs);
        feed.extend(o.feed);
        panel.extend(o.panel);
    }
    // Alerts first, then whatever has waited longest.
    needs.sort_by_key(|n| (!n.alert, n.since));
    feed.sort_by_key(|h| -h.at);
    feed.truncate(6);
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
                a.ov-stat.on[st.key == show] href=(link(st.key, days)) {
                    span.muted.small { (st.label) }
                    b { (st.value) }
                    span.small.up[st.up] { (st.note) }
                }
            }
        }
        div.ov-chart {
            div.ov-chart-head {
                span.muted.small { (caption) " · " (text::num(avg)) " on average" }
                div.seg {
                    a.on[days == 7] href=(link(show, 7)) { "7 days" }
                    a.on[days == 30] href=(link(show, 30)) { "30 days" }
                }
            }
            (ui::area_chart(&series, unit))
        }
        section.ov-block {
            h2 { "Needs you" @if !needs.is_empty() { " " span.count-pill { (needs.len()) } } }
            @if needs.is_empty() {
                p.muted.small { "Nothing is waiting on you." }
            } @else {
                div.ov-rows {
                    @for n in &needs {
                        div.need {
                            @if n.people.is_empty() {
                                span.need-icon.alert[n.alert] { (ui::icon(n.icon)) }
                            } @else {
                                (ui::avatars(&n.people, 2))
                            }
                            div.grow { strong { (n.title) } p.muted.small { (n.detail) } }
                            a.btn.small.outline href=(n.href) { (n.action) }
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
                            img.avatar src=(h.avatar) alt="" width="26" height="26" loading="lazy";
                            span.grow { strong { (h.who) } " " (h.text) }
                            span.muted.small { (short_ago(h.at)) }
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

    let body = html! {
        header.ov-hero data-local {
            @match (&banner, &icon_large) {
                (Some(b), _) => div.ov-bg style={ "background-image:url(" (b) ")" } {},
                (None, Some(i)) => div.ov-bg.wash style={ "background-image:url(" (i) ")" } {},
                (None, None) => {},
            }
            div.ov-top {
                @if ctx.cfg.preview {
                    span.status { span.dot.waiting {} "Preview" }
                } @else if ctx.bot_id().is_some() {
                    span.status { span.dot.live {} "Online" }
                } @else {
                    span.status { span.dot.waiting {} "Connecting" }
                }
                (ui::bell(&ctx, &page))
            }
            div.ov-hello {
                @match &icon_large {
                    Some(i) => img.ov-icon src=(i) alt="" width="56" height="56";,
                    None => span.ov-icon.letter { (name.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default()) },
                }
                div {
                    h1 { (greeting) ", " (user.name) }
                    p {
                        (name)
                        @if let Some(m) = members { " · " (text::num(m)) " members" }
                        @if active_today > 0 { " · " (text::num(active_today)) " here today" }
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
    notes.retain(|n| match n.global {
        true => user.admin,
        false => ctx.level(user, guild, &n.feature) >= Level::View,
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

fn notification_row(n: &Note) -> Markup {
    html! {
        div.list-item {
            span.grow {
                @match &n.link {
                    Some(l) => a href=(l) { (n.text) },
                    None => (n.text),
                }
            }
            span.meta { (n.feature) " · " (text::ago(n.at)) }
        }
    }
}

async fn notifications(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    if !ctx.guilds_for(&user).contains(&guild) {
        return Err(WebError::Forbidden);
    }
    let page = super::Page { user: user.clone(), guild_id: guild, key: "notifications", level: Level::View };
    let notes = recent_notifications(&ctx, &user, guild, 100);
    let body = html! {
        @if notes.is_empty() {
            (ui::empty(Icon::Bell, "No notifications", "Features post here when something needs you."))
        } @else {
            div.list { @for n in &notes { (notification_row(n)) } }
        }
    };
    let markup = ui::layout(&ctx, &page, "Notifications", None, body);
    ctx.db.with(|c| c.execute("UPDATE dash_users SET notif_seen = ?2 WHERE id = ?1", params![user.id as i64, crate::now_ms()]))?;
    Ok(markup.into_response())
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
                section.card.fgroup {
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
    let add = html! {
        details.pop {
            summary.btn.primary { (ui::icon(Icon::Plus)) "Add someone" }
            div.pop-body {
                form.inline-form method="post" action="/users" {
                    input.input.mono type="text" name="id" placeholder="Discord user ID" inputmode="numeric" pattern="[0-9]{15,21}" required autofocus;
                    button.btn.primary type="submit" { "Add" }
                }
                p.muted.small { "In Discord: Settings, Advanced, turn on Developer Mode, then right-click their name and Copy User ID. People who try to sign in see their ID too. Until you give them pages, they see nothing." }
            }
        }
    };
    let body = html! {
        div.people {
            @for p in &list {
                @let seen = match p.signed_in {
                    Some(at) if p.id == user.id && crate::now_ms() - at < 3_600_000 => "Signed in now".to_owned(),
                    Some(at) => format!("Signed in {}", text::ago(at)),
                    None => format!("Added {} · hasn't signed in yet", text::ago(p.created_at)),
                };
                div.prow {
                    img.avatar src=(crate::discord::avatar_url(p.id, p.avatar.as_deref())) alt="" width="36" height="36" loading="lazy";
                    div.grow {
                        strong { (if p.name.is_empty() { p.id.to_string() } else { p.name.clone() }) }
                        p.muted.small { (seen) @if let Some(by) = p.added_by.as_ref().filter(|_| p.id != owner) { " · added by " (by) } }
                    }
                    @if p.id == owner { (ui::badge("Owner", Tone::Accent)) }
                    @else if p.admin { (ui::badge("Admin", Tone::Muted)) }
                    @else if p.grants == 0 { (ui::badge("No pages yet", Tone::Warn)) }
                    @else { (ui::badge(&format!("{} {}", p.grants, if p.grants == 1 { "page" } else { "pages" }), Tone::Muted)) }
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

    let body = html! {
        form.settings.narrow method="post" action={ "/users/" (id) } {
            (ui::card(html! {
                @if is_owner {
                    (ui::field_custom("Admin", Some("The owner is always an admin."), ui::badge("Owner", Tone::Accent)))
                } @else {
                    (ui::field_toggle("admin", "Admin", Some("Everything on every server, including features, people and the activity log."), person.admin))
                }
            }))
            @if !person.admin && !is_owner {
                @for g in ctx.guild_ids() {
                    (ui::section(&ctx.guild_name(g), Some("No access hides the page. View shows it read-only. Edit lets them change it."), ui::card(html! {
                        @for f in ctx.features().iter().filter(|f| ctx.enabled(g, f.meta().key) && !f.admin_only()) {
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
                        @if ctx.features().iter().all(|f| !ctx.enabled(g, f.meta().key) || f.admin_only()) {
                            (ui::empty(Icon::Toggle, "No features on here", "Turn features on for this server first."))
                        }
                    })))
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
    Ok(ui::layout(&ctx, &page, &title, Some(html! { a.btn.small.outline href="/users" { "All people" } }), body).into_response())
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
    ctx.audit(&user, None, "users", format!("Changed access for {id}"));
    Ok(saved(&format!("/users/{id}")))
}

async fn delete_user(State(ctx): State<Ctx>, user: User, Path(id): Path<u64>) -> WebResult {
    let guild = any_guild(&ctx, &user)?;
    admin_page(&ctx, &user, guild, "users")?;
    if id == ctx.cfg.owner_id {
        return Err(WebError::Bad("The owner can't be removed.".into()));
    }
    ctx.db.with(|c| {
        c.execute("DELETE FROM dash_perms WHERE user_id = ?1", [id as i64])?;
        c.execute("DELETE FROM dash_sessions WHERE user_id = ?1", [id as i64])?;
        c.execute("DELETE FROM dash_users WHERE id = ?1", [id as i64])
    })?;
    ctx.audit(&user, None, "users", format!("Removed {id}"));
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
    let today = user.local(crate::now_ms()).date();
    let day_label = |at: i64| {
        let d = user.local(at).date();
        let back = (today - d).get_days();
        match back {
            0 => "Today".to_owned(),
            1 => "Yesterday".to_owned(),
            2..=6 => d.strftime("%A").to_string(),
            _ => d.strftime("%b %-d").to_string(),
        }
    };
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
