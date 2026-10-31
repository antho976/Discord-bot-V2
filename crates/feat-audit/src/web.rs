use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::get;
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::twilight_model::id::Id;
use bot_core::web::ui;
use bot_core::web::{FormData, Level, Page, User, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};
use serde::Deserialize;

use crate::store::{self, Filter, Group, Row, Today};
use crate::{Diff, KEY, Kind, RoleChange, RoleRef, Settings, channel_label};

const SHOWN: i64 = 100;

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/audit", get(recent_page))
        .route("/g/{guild}/audit/settings", get(settings_page).post(save_settings))
}

fn tabs(page: &Page, active: &str) -> Markup {
    ui::tabs(&[("Recent", page.path(KEY)), ("Settings", page.path("audit/settings"))], active)
}

#[derive(Deserialize)]
struct RecentQuery {
    group: Option<String>,
    q: Option<String>,
}

async fn recent_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<RecentQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let group = q.group.as_deref().and_then(Group::from_key).unwrap_or(Group::All);
    let search = q.q.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned);
    let filter = Filter { group, search: search.clone() };
    let filtered = group != Group::All || search.is_some();
    let (rows, total) = store::recent(&ctx, guild, &filter, SHOWN)?;
    // "Today" starts at midnight in the viewer's own timezone.
    let since = page.user.local(now_ms()).start_of_day()?.timestamp().as_millisecond();
    let today = store::today(&ctx, guild, since)?;
    let mods = store::mod_actions(&ctx, guild, now_ms() - crate::KEEP_DAYS * 86_400_000)?;
    let week_ago = now_ms() - 7 * 86_400_000;
    let mut week: Vec<(u64, i64)> = Vec::new();
    for a in mods.iter().filter(|a| a.at >= week_ago) {
        match week.iter_mut().find(|w| w.0 == a.moderator) {
            Some(w) => w.1 += 1,
            None => week.push((a.moderator, 1)),
        }
    }
    week.sort_by_key(|w| -w.1);
    let has_channel = s.channel.is_some() || s.message_channel.is_some() || s.member_channel.is_some();
    let search_value = search.as_deref().unwrap_or_default();

    let body = html! {
        (style())
        (tabs(&page, "Recent"))
        div.with-rail {
            div.au-main {
                div.au-filters {
                    form.au-chips method="get" action=(page.path(KEY)) {
                        input type="hidden" name="q" value=(search_value);
                        @for g in Group::ALL {
                            button.au-chip.on[g == group] type="submit" name="group" value=(g.key()) { (g.label()) }
                        }
                    }
                    form method="get" action=(page.path(KEY)) {
                        input type="hidden" name="group" value=(group.key());
                        input.input.au-search type="search" name="q" value=(search_value) placeholder="Member name or ID" aria-label="Search the log";
                    }
                }
                @if rows.is_empty() {
                    @if filtered {
                        (ui::empty(Icon::Log, "No match", "Nothing in the last 14 days fits that filter."))
                    } @else if has_channel {
                        (ui::empty(Icon::Log, "Nothing logged yet", "Edits, deletes, joins and the rest show up here for 14 days as they happen."))
                    } @else {
                        (ui::empty(Icon::Log, "No log channel yet", "Pick one in Settings and events start appearing here and in Discord."))
                    }
                } @else {
                    (timeline(&ctx, &page, guild, &rows, &mods))
                    @if total > rows.len() as i64 {
                        p.muted.small { "Showing the latest " (rows.len()) " of " (text::num(total)) ". Filter to narrow it down." }
                    }
                }
            }
            aside.rail {
                (today_block(&today))
                div.rail-block {
                    h2 { "Moderator actions this week" }
                    @if week.is_empty() {
                        p.muted.small { "Bans, kicks, timeouts, deletions and role changes by moderators show up here. The bot needs the View Audit Log permission to see who did them." }
                    } @else {
                        div.au-mods {
                            @for (m, n) in week.iter().take(6) {
                                div.au-mod {
                                    img.avatar src=(ctx.member_avatar(guild, *m)) alt="" width="36" height="36" loading="lazy";
                                    span.muted.small { (ctx.display_name(guild, *m)) " · " (n) }
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Audit log", None, body).into_response())
}

/// The day's counts, each with a meter against the busiest of them.
fn today_block(t: &Today) -> Markup {
    let max = [t.joined, t.left, t.edits, t.deleted].into_iter().max().unwrap_or(0).max(1);
    let pct = |n: i64| n * 100 / max;
    html! {
        div.rail-block {
            h2 { "Today" }
            (stat("Joined", t.joined, pct(t.joined), "au-live"))
            (stat("Left", t.left, pct(t.left), "au-held"))
            (stat("Edits", t.edits, pct(t.edits), "au-data"))
            (stat("Deletions", t.deleted, pct(t.deleted), "au-data"))
        }
    }
}

fn stat(label: &str, n: i64, pct: i64, tone: &str) -> Markup {
    html! {
        div.au-stat {
            span { (label) }
            span class={ "meter " (tone) } { span style={ "width:" (pct) "%" } {} }
            b { (text::num(n)) }
        }
    }
}

/// Entries grouped by day, in the viewer's own timezone.
fn timeline(ctx: &Ctx, page: &Page, guild: u64, rows: &[Row], mods: &[store::ModAction]) -> Markup {
    let today = page.user.local(now_ms()).date();
    let day_label = |at: i64| {
        let d = page.user.local(at).date();
        match (today - d).get_days() {
            0 => "Today".to_owned(),
            1 => "Yesterday".to_owned(),
            2..=6 => d.strftime("%A").to_string(),
            _ => d.strftime("%b %-d").to_string(),
        }
    };
    html! {
        div.timeline data-local {
            @for (i, r) in rows.iter().enumerate() {
                @let label = day_label(r.at);
                @if i == 0 || day_label(rows[i - 1].at) != label {
                    h2.day { (label) }
                }
                (entry(ctx, page, guild, r, mods))
            }
        }
    }
}

fn entry(ctx: &Ctx, page: &Page, guild: u64, r: &Row, mods: &[store::ModAction]) -> Markup {
    let time = page.user.local(r.at).strftime("%H:%M").to_string();
    let name = r.user_id.map(|u| member_name(ctx, guild, u, &r.user_name));
    let chan = r.channel_id.map(|c| channel_label(ctx, c)).unwrap_or_default();
    html! {
        div.au-row {
            span.mono.muted.au-time { (time) }
            @if let Some(u) = r.user_id {
                img.avatar src=(ctx.member_avatar(guild, u)) alt="" width="26" height="26" loading="lazy";
            } @else {
                span.au-icon { (ui::icon(Icon::Log)) }
            }
            div.au-body {
                @let by = store::moderator_for(mods, r.kind, r.user_id, r.at).filter(|m| Some(*m) != r.user_id).map(|m| ctx.display_name(guild, m));
                (headline(r.kind, name.as_deref(), &chan, by.as_deref()))
                (detail(r))
            }
        }
    }
}

/// The bold name, then what happened.
fn headline(kind: Kind, name: Option<&str>, chan: &str, by: Option<&str>) -> Markup {
    if let Some(by) = by {
        let rest = match kind {
            Kind::MessageDelete => format!("had a message in {chan} deleted by"),
            Kind::Leave => "was kicked by".into(),
            Kind::Ban => "was banned by".into(),
            Kind::Unban => "was unbanned by".into(),
            Kind::Timeout => "was timed out by".into(),
            Kind::TimeoutEnd => "had their timeout removed by".into(),
            _ => "had their roles changed by".into(),
        };
        return html! {
            div.au-line {
                @if let Some(n) = name { strong { (n) } " " }
                (rest) " " strong { (by) }
            }
        };
    }
    let rest = match (kind, name.is_some()) {
        (Kind::MessageDelete, true) => format!("deleted a message in {chan}"),
        (Kind::MessageDelete, false) => format!("A message was deleted in {chan}"),
        (Kind::MessageEdit, _) => format!("edited a message in {chan}"),
        (Kind::BulkDelete, _) => format!("Bulk delete in {chan}"),
        (Kind::Join, _) => "joined".into(),
        (Kind::Leave, _) => "left".into(),
        (Kind::Nickname, _) => "changed their nickname".into(),
        (Kind::Roles, _) => "changed roles".into(),
        (Kind::Ban, _) => "was banned".into(),
        (Kind::Unban, _) => "was unbanned".into(),
        (Kind::Timeout, _) => "was timed out".into(),
        (Kind::TimeoutEnd, _) => "had their timeout removed".into(),
        (Kind::Boost, _) => "boosted the server".into(),
        (Kind::VoiceJoin, _) => format!("joined voice in {chan}"),
        (Kind::VoiceLeave, _) => format!("left voice in {chan}"),
        (Kind::VoiceMove, _) => format!("moved voice to {chan}"),
    };
    html! {
        div.au-line {
            @if let Some(n) = name { strong { (n) } " " }
            (rest)
        }
    }
}

/// The line under the headline: the deleted text, the edit, the role changes, or the reason.
fn detail(r: &Row) -> Markup {
    match r.kind {
        Kind::MessageDelete => {
            if matches!(r.summary.as_str(), "" | "Content not available" | "No text") {
                html! { p.muted.small { (if r.summary.is_empty() { "No text" } else { r.summary.as_str() }) } }
            } else {
                html! { div.au-quote { (r.summary) } }
            }
        }
        Kind::MessageEdit => match bot_core::serde_json::from_str::<Diff>(&r.summary) {
            Ok(d) => edit_detail(&d),
            Err(_) => plain_detail(&r.summary),
        },
        Kind::Nickname => match bot_core::serde_json::from_str::<Diff>(&r.summary) {
            Ok(d) => nickname_detail(&d),
            Err(_) => plain_detail(&r.summary),
        },
        Kind::Roles => match bot_core::serde_json::from_str::<RoleChange>(&r.summary) {
            Ok(c) => role_detail(&c),
            Err(_) => plain_detail(&r.summary),
        },
        _ => plain_detail(&r.summary),
    }
}

fn plain_detail(summary: &str) -> Markup {
    html! { @if !summary.is_empty() { p.muted.small { (summary) } } }
}

/// Old text struck through, then the new text.
fn edit_detail(d: &Diff) -> Markup {
    let after = d.after.as_deref().unwrap_or_default();
    html! {
        @if let Some(before) = &d.before {
            div.au-diff {
                s { (before) }
                span.muted { " → " }
                (after)
            }
        } @else {
            div.au-diff {
                span.muted { "Now: " }
                (after)
            }
        }
    }
}

fn nickname_detail(d: &Diff) -> Markup {
    let show = |name: &Option<String>| match name {
        Some(n) => html! { (n) },
        None => html! { span.muted { "No nickname" } },
    };
    html! {
        div.au-diff {
            (show(&d.before))
            span.muted { " → " }
            (show(&d.after))
        }
    }
}

fn role_detail(c: &RoleChange) -> Markup {
    html! {
        @if !c.added.is_empty() {
            div.au-roles { span.muted.small { "Added" } @for r in &c.added { (role_chip(r)) } }
        }
        @if !c.removed.is_empty() {
            div.au-roles { span.muted.small { "Removed" } @for r in &c.removed { (role_chip(r)) } }
        }
    }
}

fn role_chip(r: &RoleRef) -> Markup {
    let dot = if r.color == 0 { "var(--ink-3)".to_owned() } else { format!("#{:06x}", r.color) };
    html! { span.au-role { span.dot style={ "background:" (dot) } {} (r.name) } }
}

/// The name now if the cache knows them, otherwise the one saved with the entry.
fn member_name(ctx: &Ctx, guild: u64, user: u64, saved_name: &str) -> String {
    if saved_name.is_empty() || ctx.cache.user(Id::new(user)).is_some() {
        ctx.display_name(guild, user)
    } else {
        saved_name.to_owned()
    }
}

/// The timeline, the chips and the rail. Classes are prefixed `au-`.
fn style() -> Markup {
    html! { style { (PreEscaped(STYLE)) } }
}

const STYLE: &str = r#"
.au-main { display: flex; flex-direction: column; gap: 18px; min-width: 0; }
.au-filters { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 10px 16px; }
.au-chips { display: flex; flex-wrap: wrap; gap: 8px; }
.au-mods { display: flex; flex-wrap: wrap; gap: 14px; }
.au-mod { display: flex; flex-direction: column; align-items: center; gap: 4px; }
.au-chip { height: 26px; padding: 0 12px; border: 0; border-radius: 13px; background: var(--control); color: var(--ink-2); font: 12px var(--sans); cursor: pointer; white-space: nowrap; }
.au-chip:hover { background: var(--hover); color: var(--ink); }
.au-chip.on { background: var(--ink); color: var(--ground); }
.au-search { width: 210px; max-width: 100%; }
.au-row { display: grid; grid-template-columns: 44px 26px minmax(0, 1fr); column-gap: 12px; align-items: start; }
.au-time { padding-top: 3px; font-size: 12px; }
.au-row > .avatar { width: 26px; height: 26px; margin-top: 1px; }
.au-icon { display: grid; place-items: center; width: 26px; height: 26px; margin-top: 1px; border-radius: 50%; background: var(--control); color: var(--ink-2); }
.au-icon .icon { width: 14px; height: 14px; }
.au-body { display: flex; flex-direction: column; gap: 4px; min-width: 0; padding-top: 2px; }
.au-line { font-size: 13px; color: var(--ink-2); }
.au-line strong { font-weight: 500; color: var(--ink); }
.au-body p { font-size: 12px; }
.au-quote { border-left: 2px solid var(--held); padding-left: 10px; font-size: 12.5px; color: var(--ink-2); white-space: pre-wrap; overflow-wrap: anywhere; }
.au-diff { font-size: 12.5px; color: var(--ink-2); overflow-wrap: anywhere; }
.au-diff s { color: var(--ink-3); }
.au-roles { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; }
.au-roles > .muted { margin-right: 2px; }
.au-role { display: inline-flex; align-items: center; gap: 6px; height: 22px; padding: 0 9px; border-radius: 11px; background: var(--control); font-size: 12px; color: var(--ink-2); }
.au-role .dot { width: 8px; height: 8px; }
.au-stat { display: grid; grid-template-columns: 66px minmax(0, 1fr) 34px; align-items: center; gap: 12px; font-size: 13px; color: var(--ink-2); }
.au-stat b { font-weight: 500; text-align: right; color: var(--ink); font-variant-numeric: tabular-nums; }
.au-stat .meter.au-live > span { background: var(--live); }
.au-stat .meter.au-held > span { background: var(--held); }
@media (max-width: 640px) {
  .au-filters { flex-direction: column; align-items: stretch; }
  .au-search { width: 100%; }
}
"#;

async fn settings_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let body = html! {
        (tabs(&page, "Settings"))
        (ui::settings_form(&page, &page.path("audit/settings"), html! {
            (ui::section("Where it goes", Some("Nothing is logged until you pick a channel."), ui::card(html! {
                (ui::field_channel(&ctx, guild, "channel", "Log channel", Some("Every event goes here unless one of the two below is set."), s.channel))
                (ui::field_channel(&ctx, guild, "message_channel", "Message events", Some("Edits and deletes. None uses the log channel."), s.message_channel))
                (ui::field_channel(&ctx, guild, "member_channel", "Member events", Some("Joins, leaves, roles, bans, timeouts and voice. None uses the log channel."), s.member_channel))
            })))
            (ui::section("Messages", None, ui::card(html! {
                (ui::field_toggle("message_delete", "Deleted", Some("The author, channel, text and file names, when the bot had seen the message."), s.message_delete))
                (ui::field_toggle("message_edit", "Edited", Some("Before and after, with a link to the message."), s.message_edit))
                (ui::field_toggle("bulk_delete", "Bulk deleted", Some("How many messages a purge removed, and where."), s.bulk_delete))
            })))
            (ui::section("Members", None, ui::card(html! {
                (ui::field_toggle("join", "Joined", Some("With the account's age. Accounts under 7 days old are flagged."), s.join))
                (ui::field_toggle("leave", "Left", Some("With the roles they had."), s.leave))
                (ui::field_toggle("nickname", "Nickname changed", None, s.nickname))
                (ui::field_toggle("roles", "Roles added or removed", None, s.roles))
                (ui::field_toggle("ban", "Banned", None, s.ban))
                (ui::field_toggle("unban", "Unbanned", None, s.unban))
                (ui::field_toggle("timeout", "Timed out or released", None, s.timeout))
                (ui::field_toggle("boost", "Boosted", Some("When a member starts boosting the server."), s.boost))
                (ui::field_toggle("voice", "Voice joins, leaves and moves", Some("Off by default. In a busy server this is most of the log."), s.voice))
            })))
            (ui::section("Skip", None, ui::card(html! {
                (ui::field_toggle("ignore_bots", "Ignore bots", Some("Nothing is logged about bot accounts, including their messages."), s.ignore_bots))
                (ui::field_channels(&ctx, guild, "ignored_channels", "Ignored channels", Some("Edits and deletes in these channels aren't logged. The log channels themselves are always skipped."), &s.ignored_channels))
            })))
        }))
    };
    Ok(ui::layout(&ctx, &page, "Audit log", None, body).into_response())
}

async fn save_settings(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = Settings {
        channel: form.channel(&ctx, guild, "channel"),
        message_channel: form.channel(&ctx, guild, "message_channel"),
        member_channel: form.channel(&ctx, guild, "member_channel"),
        message_delete: form.bool("message_delete"),
        message_edit: form.bool("message_edit"),
        bulk_delete: form.bool("bulk_delete"),
        join: form.bool("join"),
        leave: form.bool("leave"),
        nickname: form.bool("nickname"),
        roles: form.bool("roles"),
        ban: form.bool("ban"),
        unban: form.bool("unban"),
        timeout: form.bool("timeout"),
        boost: form.bool("boost"),
        voice: form.bool("voice"),
        ignore_bots: form.bool("ignore_bots"),
        ignored_channels: form.channels(&ctx, guild, "ignored_channels"),
    };
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed audit log settings");
    Ok(saved(&page.path("audit/settings")))
}
