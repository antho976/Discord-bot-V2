//! Pages every install has: the overview, the per-server feature switches, people and their
//! access, the activity log and notifications.

use axum::Router;
use axum::extract::{Path, State};
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

async fn overview(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let guilds = ctx.guilds_for(&user);
    if !guilds.contains(&guild) {
        return Err(if ctx.guild_ids().contains(&guild) { WebError::Forbidden } else { WebError::NotFound });
    }
    let page = super::Page { user: user.clone(), guild_id: guild, key: "overview", level: Level::View };
    let messages = daily(&ctx, guild, "messages", 30);
    let joins = daily(&ctx, guild, "joins", 7);
    let leaves = daily(&ctx, guild, "leaves", 7);
    let on = ctx.features().iter().filter(|f| ctx.enabled(guild, f.meta().key)).count();
    let notes = recent_notifications(&ctx, &user, guild, 6);

    let body = html! {
        (ui::readings(&[
            ("members", ctx.member_count(guild).map(|n| text::num(n as i64)).unwrap_or_else(|| "…".into())),
            ("joined this week", text::num(joins.iter().map(|d| d.1).sum())),
            ("left this week", text::num(leaves.iter().map(|d| d.1).sum())),
            ("messages today", text::num(messages.last().map(|d| d.1).unwrap_or(0))),
        ]))
        (ui::section("Messages", Some("Per day, last 30 days."), ui::bar_chart(&messages, "var(--data)", "messages")))
        (ui::section("Recent", None, html! {
            @if notes.is_empty() {
                (ui::empty(Icon::Bell, "Nothing new", "Notices from features show up here: pet requests, new tickets, caught scams, app approvals."))
            } @else {
                div.list { @for n in &notes { (notification_row(n)) } }
            }
        }))
        @if user.admin && on == 0 {
            (ui::section("Get started", None, html! {
                p.muted { "Every feature starts off in a new server. Switch on what this server should have." }
                div { a.btn.primary href={ "/g/" (guild) "/features" } { "Choose features" } }
            }))
        }
    };
    Ok(ui::layout(&ctx, &page, &ctx.guild_name(guild), None, body).into_response())
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
    feats.sort_by_key(|m| (m.group, m.name));
    let mut groups: Vec<(crate::Group, Vec<crate::FeatureMeta>)> = Vec::new();
    for m in feats {
        match groups.iter_mut().find(|(g, _)| *g == m.group) {
            Some((_, list)) => list.push(m),
            None => groups.push((m.group, vec![m])),
        }
    }
    let body = html! {
        p.muted { "What runs in " strong { (ctx.guild_name(guild)) } ". A feature that's off ignores this server and its slash commands disappear here." }
        @for (group, list) in &groups {
            (ui::section(group.label(), None, ui::card(html! {
                @for m in list {
                    form method="post" action={ "/g/" (guild) "/features/" (m.key) } {
                        (ui::field_toggle(&format!("on-{}", m.key), m.name, Some(m.description), ctx.enabled(guild, m.key)))
                    }
                }
            })))
        }
    };
    let body = html! { div.stack.autosubmit-switches { (body) } };
    Ok(ui::layout(&ctx, &page, "Features", None, mark_autosubmit(body)).into_response())
}

/// Make every switch on the page save on change.
fn mark_autosubmit(body: Markup) -> Markup {
    let s = body.into_string().replace("type=\"checkbox\"", "type=\"checkbox\" data-autosubmit");
    maud::PreEscaped(s)
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
}

fn people(ctx: &Ctx) -> Vec<Person> {
    ctx.db
        .with(|c| {
            let mut s = c.prepare(
                "SELECT u.id, u.name, u.avatar, u.admin, u.created_at,
                        (SELECT COUNT(*) FROM dash_perms p WHERE p.user_id = u.id AND p.level > 0)
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
    let body = html! {
        (ui::section("Add someone", Some("They sign in with Discord. Until you give them pages, they see nothing."), html! {
            form.inline-form method="post" action="/users" {
                input.input.mono type="text" name="id" placeholder="Discord user ID" inputmode="numeric" pattern="[0-9]{15,21}" required;
                button.btn.primary type="submit" { (ui::icon(Icon::Plus)) "Add" }
            }
            p.muted.small { "In Discord: Settings, Advanced, turn on Developer Mode, then right-click their name and Copy User ID. People who try to sign in see their ID too." }
        }))
        (ui::section("People", None, html! {
            div.table-wrap {
                table.table {
                    thead { tr { th { "Name" } th { "Access" } th { "Added" } th {} } }
                    tbody {
                        @for p in &list {
                            tr {
                                td { (ui::person(if p.name.is_empty() { "Not signed in yet" } else { &p.name }, &crate::discord::avatar_url(p.id, p.avatar.as_deref()), Some(&p.id.to_string()))) }
                                td {
                                    @if p.id == owner { (ui::badge("Owner", Tone::Accent)) }
                                    @else if p.admin { (ui::badge("Admin", Tone::Accent)) }
                                    @else if p.grants == 0 { (ui::badge("No pages", Tone::Warn)) }
                                    @else { (ui::badge(&format!("{} {}", p.grants, if p.grants == 1 { "page" } else { "pages" }), Tone::Muted)) }
                                }
                                td.muted.small { (text::ago(p.created_at)) }
                                td.actions { a.btn.small href={ "/users/" (p.id) } { "Edit access" } }
                            }
                        }
                    }
                }
            }
        }))
    };
    Ok(ui::layout(&ctx, &page, "People", None, body).into_response())
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
        div { (ui::person(&display, &crate::discord::avatar_url(id, person.avatar.as_deref()), Some(&id.to_string()))) }
        form.settings method="post" action={ "/users/" (id) } {
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
                                div.tabs role="radiogroup" aria-label=(m.name) {
                                    @for (v, label) in [(0, "No access"), (1, "View"), (2, "Edit")] {
                                        label.tab.on[cur == v] {
                                            input type="radio" name=(name) value=(v) checked[cur == v] style="display:none" onchange="this.closest('.tabs').querySelectorAll('.tab').forEach(t=>t.classList.toggle('on',t.contains(this)))";
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
    Ok(ui::layout(&ctx, &page, "Edit access", Some(html! { a.btn.small href="/users" { "All people" } }), body).into_response())
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
    let rows: Vec<(i64, String, Option<i64>, String, String)> = ctx.db.with(|c| {
        let mut s = c.prepare("SELECT at, user_name, guild_id, feature, action FROM dash_audit ORDER BY at DESC LIMIT 200")?;
        let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })?;
    let body = html! {
        @if rows.is_empty() {
            (ui::empty(Icon::Activity, "No changes yet", "Every change made on the dashboard is listed here, with who made it."))
        } @else {
            div.table-wrap {
                table.table {
                    thead { tr { th { "When" } th { "Who" } th { "Where" } th { "What" } } }
                    tbody {
                        @for (at, who, g, feature, action) in &rows {
                            tr {
                                td.muted.small style="white-space:nowrap" { (text::ago(*at)) }
                                td { (who) }
                                td.muted.small { @if let Some(g) = g { (ctx.guild_name(*g as u64)) " · " } (feature) }
                                td { (action) }
                            }
                        }
                    }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Activity", None, body).into_response())
}
