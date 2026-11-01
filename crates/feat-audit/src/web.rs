use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::{IntoResponse, Redirect};
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::twilight_model::id::Id;
use bot_core::web::ui;
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};
use serde::Deserialize;

use crate::store::{self, Filter, Group, Row, Today};
use crate::{Diff, KEY, Kind, RoleChange, RoleRef, Settings, channel_label};

const SHOWN: i64 = 100;

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/audit", get(recent_page))
        .route("/g/{guild}/audit/settings", get(settings_page).post(save_settings))
        .route("/g/{guild}/audit/test", post(send_test))
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
    let filter = Filter { group, search: search.as_deref().map(|q| store::Search::new(&ctx, guild, q)) };
    let filtered = group != Group::All || search.is_some();
    let (rows, total) = store::recent(&ctx, guild, &filter, SHOWN)?;
    // "Today" starts at midnight in the viewer's own timezone.
    let since = page.user.local(now_ms()).start_of_day()?.timestamp().as_millisecond();
    let today = store::today(&ctx, guild, since)?;
    // Only the actions that can explain the entries shown.
    let oldest = rows.last().map_or(now_ms(), |r| r.at);
    let mods = store::mod_actions(&ctx, guild, oldest - store::MATCH_MS)?;
    let week = store::moderator_counts(&ctx, guild, now_ms() - 7 * 86_400_000, ctx.bot_id())?;
    let daily = store::daily(&ctx, guild, now_ms(), 14)?;
    let has_channel = s.channel.is_some() || s.message_channel.is_some() || s.member_channel.is_some();
    // Set but gone from the server. Only once the server is cached, so a restart doesn't flash it.
    let cached = ctx.cache.guild(Id::new(guild)).is_some();
    let gone: Vec<&str> = [(s.channel, "log channel"), (s.message_channel, "channel for message events"), (s.member_channel, "channel for member events")]
        .into_iter()
        // The log channel isn't used when both of the others are set.
        .filter(|(c, _)| *c != s.channel || s.message_channel.is_none() || s.member_channel.is_none())
        .filter(|(c, _)| cached && c.is_some_and(|c| ctx.cache.channel(Id::new(c)).is_none()))
        .map(|(_, name)| name)
        .collect();
    let search_value = search.as_deref().unwrap_or_default();

    let body = html! {
        (style())
        (tabs(&page, "Recent"))
        div.with-rail {
            div.au-main {
                @for name in &gone {
                    div.need.alert {
                        span.need-icon.alert { (ui::icon(Icon::Hash)) }
                        div.grow {
                            strong { "The " (name) " was deleted" }
                            p.muted.small { "Its events aren't posted in Discord until you pick another." }
                        }
                        a.btn.small.primary href=(page.path("audit/settings")) { "Pick another" }
                    }
                }
                (today_readings(&today))
                div.au-filters {
                    form.seg.au-groups method="get" action=(page.path(KEY)) aria-label="Show" {
                        input type="hidden" name="q" value=(search_value);
                        @for g in Group::ALL {
                            button.on[g == group] type="submit" name="group" value=(g.key()) aria-pressed=(if g == group { "true" } else { "false" }) {
                                @if let Some(c) = group_color(g) { span.au-swatch style={ "--c:" (c) } {} }
                                (g.label())
                            }
                        }
                    }
                    form.au-search method="get" action=(page.path(KEY)) role="search" {
                        input type="hidden" name="group" value=(group.key());
                        (ui::icon(Icon::Search))
                        input.input type="search" name="q" value=(search_value) placeholder="Name, channel or text" aria-label="Search the log";
                    }
                }
                @if rows.is_empty() {
                    @if filtered {
                        (ui::empty(Icon::Search, "No match", "Nothing in the last 14 days fits that filter."))
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
                (mods_block(&ctx, guild, &week))
                (days_block(&daily))
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Audit log", None, body).into_response())
}

/// The day's counts, each in its kind's colour, counting up.
fn today_readings(t: &Today) -> Markup {
    let items = [("Joined today", t.joined, "var(--live)"), ("Left today", t.left, LEAVE), ("Edits today", t.edits, "var(--data)"), ("Deletions today", t.deleted, "var(--signal)")];
    html! {
        div.readings.au-readings {
            @for (label, n, color) in items {
                div.reading style={ "--hue:" (color) } {
                    b data-count { (text::num(n)) }
                    span { span.au-swatch style={ "--c:" (color) } {} (label) }
                }
            }
        }
    }
}

/// Who moderated this week, most actions first, each with a bar against the busiest.
fn mods_block(ctx: &Ctx, guild: u64, week: &[(u64, i64)]) -> Markup {
    let max = week.iter().map(|(_, n)| *n).max().unwrap_or(1).max(1);
    html! {
        div.rail-block {
            h2 { "Moderators this week" }
            @if week.is_empty() {
                p.muted.small { "Bans, kicks, timeouts, deletions and role changes by moderators show up here. The bot needs the View Audit Log permission to see who did them." }
            } @else {
                div.au-mods {
                    @for (i, (m, n)) in week.iter().take(6).enumerate() {
                        div.mini.au-mod style={ "--s:" (i) } {
                            img.avatar src=(ctx.member_avatar(guild, *m)) alt="" width="30" height="30" loading="lazy";
                            div.grow {
                                span { (ctx.display_name(guild, *m)) }
                                span.meter { span style={ "width:" ((n * 100 / max).max(6)) "%" } {} }
                            }
                            b data-count { (n) }
                        }
                    }
                }
            }
        }
    }
}

/// Entries per day for two weeks, the latest brightest.
fn days_block(daily: &[i64]) -> Markup {
    let n = daily.len();
    let series: Vec<(String, Option<i64>)> = daily
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let ago = n - 1 - i;
            let label = match ago {
                0 => "Last 24 hours".to_owned(),
                1 => "1 to 2 days ago".to_owned(),
                _ => format!("{ago} to {} days ago", ago + 1),
            };
            (label, Some(*v))
        })
        .collect();
    let total: i64 = daily.iter().sum();
    html! {
        div.rail-block {
            h2 { "Two weeks" span.muted.small { (text::num(total)) " entries" } }
            (ui::mini_bars(&series, "entries", "Entries logged in each of the last 14 days"))
            div.au-axis { span { "14 days ago" } span { "Today" } }
            p.muted.small { "Entries are kept for 14 days." }
        }
    }
}

/// The colour that marks a kind of entry on the timeline: green for arrivals, red for bans,
/// amber for timeouts, lilac for edits, coral for deletions, sky for roles and names.
fn kind_color(kind: Kind) -> &'static str {
    match kind {
        Kind::Join | Kind::Unban | Kind::TimeoutEnd => "var(--live)",
        Kind::Leave => LEAVE,
        Kind::Ban => "var(--held)",
        Kind::Timeout => "var(--waiting)",
        Kind::MessageEdit => "var(--data)",
        Kind::MessageDelete | Kind::BulkDelete => "var(--signal)",
        Kind::Nickname | Kind::Roles => "var(--h-pets)",
        // Discord's own boost pink.
        Kind::Boost => "#ff73fa",
        Kind::VoiceJoin | Kind::VoiceLeave | Kind::VoiceMove => "var(--h-members)",
    }
}

/// Leaving: a quiet periwinkle, so it reads apart from bans.
const LEAVE: &str = "var(--h-tools)";

fn group_color(g: Group) -> Option<&'static str> {
    match g {
        Group::All => None,
        Group::Messages => Some("var(--data)"),
        Group::Members => Some("var(--live)"),
        Group::Roles => Some("var(--h-pets)"),
        Group::Bans => Some("var(--held)"),
    }
}

fn kind_icon(kind: Kind) -> Icon {
    match kind {
        Kind::MessageDelete | Kind::BulkDelete => Icon::Trash,
        Kind::MessageEdit => Icon::Brush,
        Kind::Join => Icon::Enter,
        Kind::Leave => Icon::Door,
        Kind::Nickname => Icon::Member,
        Kind::Roles => Icon::Users,
        Kind::Ban => Icon::Shield,
        Kind::Unban => Icon::Check,
        Kind::Timeout | Kind::TimeoutEnd => Icon::Clock,
        Kind::Boost => Icon::Star,
        Kind::VoiceJoin | Kind::VoiceLeave | Kind::VoiceMove => Icon::Wave,
    }
}

/// Entries grouped by day, in the viewer's own timezone, on a spine of faces.
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
    let mut days: Vec<(String, Vec<&Row>)> = Vec::new();
    for r in rows {
        let label = day_label(r.at);
        match days.last_mut() {
            Some((l, list)) if *l == label => list.push(r),
            _ => days.push((label, vec![r])),
        }
    }
    let mut n = 0usize;
    html! {
        div.au-timeline data-local {
            @for (label, list) in &days {
                section.au-day {
                    h2.au-day-head { (label) span { (list.len()) } }
                    div.au-list {
                        @for r in list {
                            @let i = { n += 1; n };
                            (entry(ctx, page, guild, r, mods, i))
                        }
                    }
                }
            }
        }
    }
}

fn entry(ctx: &Ctx, page: &Page, guild: u64, r: &Row, mods: &[store::ModAction], i: usize) -> Markup {
    let time = page.user.local(r.at).strftime("%H:%M").to_string();
    let name = r.user_id.map(|u| member_name(ctx, guild, u, &r.user_name));
    let chan = r.channel_id.map(|c| channel_label(ctx, c)).unwrap_or_default();
    let action = store::moderator_for(mods, r.kind, r.user_id, r.channel_id, r.at).filter(|a| Some(a.moderator) != r.user_id);
    html! {
        div.au-entry style={ "--c:" (kind_color(r.kind)) ";--i:" (i.min(10)) } {
            time.au-time datetime=(r.at) title=(page.user.local(r.at).strftime("%b %-d, %H:%M").to_string()) { (time) }
            div.au-face {
                @if let Some(u) = r.user_id {
                    img.avatar src=(ctx.member_avatar(guild, u)) alt="" width="36" height="36" loading="lazy";
                } @else {
                    span.au-noface { (ui::icon(Icon::Hash)) }
                }
                span.au-kind title=(r.kind.label()) { (ui::icon(kind_icon(r.kind))) }
            }
            // What happened, then what it was: under each other in a narrow column, side by side in a
            // wide one so the details line up in a column of their own.
            div.au-head { (headline(ctx, guild, r.kind, name.as_deref(), &chan, action.map(|a| a.moderator))) }
            div.au-more {
                (detail(r))
                @if let Some(reason) = action.and_then(|a| a.reason.as_deref()) {
                    p.au-reason { span.muted { "Reason " } (reason) }
                }
            }
        }
    }
}

/// The bold name, then what happened, and the moderator with their face when one did it.
fn headline(ctx: &Ctx, guild: u64, kind: Kind, name: Option<&str>, chan: &str, by: Option<u64>) -> Markup {
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
                (rest) " "
                span.au-by {
                    img.avatar src=(ctx.member_avatar(guild, by)) alt="" width="18" height="18" loading="lazy";
                    strong { (ctx.display_name(guild, by)) }
                }
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
        // A young account is flagged when it joins.
        Kind::Join if r.summary.starts_with("New account") => html! {
            p.au-plain { span.au-new { "New account" } ", " (r.summary.trim_start_matches("New account").trim_start_matches(',').trim()) }
        },
        _ => plain_detail(&r.summary),
    }
}

fn plain_detail(summary: &str) -> Markup {
    html! { @if !summary.is_empty() { p.au-plain { (summary) } } }
}

/// The old text struck through on red, then the new text on green.
fn edit_detail(d: &Diff) -> Markup {
    let after = d.after.as_deref().unwrap_or_default();
    html! {
        div.au-diff {
            @if let Some(before) = &d.before {
                del { span.au-sign { "−" } (before) }
            }
            ins { span.au-sign { "+" } (after) }
        }
    }
}

fn nickname_detail(d: &Diff) -> Markup {
    let show = |name: &Option<String>, class: &str| match name {
        Some(n) => html! { span class=(class) { (n) } },
        None => html! { span class={ (class) " none" } { "No nickname" } },
    };
    html! {
        div.au-names {
            (show(&d.before, "au-name was"))
            (ui::icon(Icon::Chevron))
            (show(&d.after, "au-name now"))
        }
    }
}

fn role_detail(c: &RoleChange) -> Markup {
    html! {
        div.au-roles {
            @for r in &c.added { (role_chip(r, true)) }
            @for r in &c.removed { (role_chip(r, false)) }
        }
    }
}

fn role_chip(r: &RoleRef, added: bool) -> Markup {
    let dot = if r.color == 0 { "var(--ink-3)".to_owned() } else { format!("#{:06x}", r.color) };
    html! {
        span.au-role.removed[!added] style={ "--role:" (dot) } title=(if added { "Added" } else { "Removed" }) {
            span.au-sign { (if added { "+" } else { "−" }) }
            span.dot {}
            (r.name)
        }
    }
}

/// The name now if the cache knows them, otherwise the one saved with the entry.
fn member_name(ctx: &Ctx, guild: u64, user: u64, saved_name: &str) -> String {
    if saved_name.is_empty() || ctx.cache.user(Id::new(user)).is_some() {
        ctx.display_name(guild, user)
    } else {
        saved_name.to_owned()
    }
}

/// The timeline, the filters and the rail. Classes are prefixed `au-`.
fn style() -> Markup {
    html! { style { (PreEscaped(STYLE)) } }
}

const STYLE: &str = r#"
.au-main { display: flex; flex-direction: column; gap: 22px; min-width: 0; }
.au-swatch { display: inline-block; width: 8px; height: 8px; border-radius: 50%; background: var(--c); box-shadow: none; flex: none; }
.au-readings { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 10px; }
.au-readings .reading { min-width: 0; }
.au-readings .reading span { display: inline-flex; align-items: center; gap: 7px; }

/* Filters: a segmented choice of groups, each with its colour, and a search with its glass. */
.au-filters { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 10px 16px; }
.au-groups { max-width: 100%; overflow-x: auto; scrollbar-width: none; }
.au-groups button { display: inline-flex; align-items: center; gap: 7px; border: 0; background: none; font: inherit; font-size: 12.5px; }
.au-search { position: relative; width: 240px; max-width: 100%; }
.au-search .icon { position: absolute; left: 11px; top: 10px; width: 15px; height: 15px; color: var(--ink-3); pointer-events: none; }
.au-search .input { padding-left: 34px; }

/* The timeline: days, then entries on a spine that runs through the faces. */
.au-timeline { display: flex; flex-direction: column; gap: 22px; }
.au-day { display: flex; flex-direction: column; gap: 4px; }
.au-day-head { position: sticky; top: 8px; z-index: 2; align-self: flex-start; display: inline-flex; align-items: baseline; gap: 8px; height: 28px; line-height: 28px; padding: 0 12px; margin-left: -12px; border-radius: 14px; font-size: 13.5px; font-weight: 600; background: color-mix(in oklab, var(--page) 85%, transparent); backdrop-filter: blur(10px); margin-bottom: 4px; }
.au-day-head span { color: var(--ink-3); font-weight: 400; font-size: 12.5px; font-variant-numeric: tabular-nums; }
.au-list { position: relative; display: flex; flex-direction: column; }
.au-list::before { content: ""; position: absolute; left: 78px; top: 18px; bottom: 18px; width: 1px; background: var(--edge-2); }
.au-entry { position: relative; display: grid; grid-template-columns: 42px 36px minmax(0, 1fr); column-gap: 18px; align-items: start; padding: 9px 10px 9px 0; border-radius: 14px; transition: background 0.18s; animation: au-in 0.24s var(--ease) both; animation-delay: calc(var(--i, 0) * 12ms); }
.au-entry > .au-time, .au-entry > .au-face { grid-row: 1 / span 2; }
.au-entry:hover { background: linear-gradient(90deg, transparent 50px, var(--well) 90px); }
@keyframes au-in { from { opacity: 0; transform: translateY(4px); } }
/* Another filter or search is the same page: the rows are simply there. */
html.same .au-entry, html.same .au-mod, html.same .au-mod .meter > span { animation: none; }
.au-time { padding-top: 9px; font-size: 12.5px; color: var(--ink-3); text-align: right; font-variant-numeric: tabular-nums; }
.au-face { position: relative; width: 36px; height: 36px; }
.au-face .avatar { width: 36px; height: 36px; box-shadow: 0 0 0 3px var(--page); }
.au-noface { display: grid; place-items: center; width: 36px; height: 36px; border-radius: 50%; background: var(--raised); color: var(--ink-2); box-shadow: 0 0 0 3px var(--page), inset 0 0 0 1px var(--edge-2); }
.au-noface .icon { width: 16px; height: 16px; }
/* The kind, in its colour, pinned to the face. */
.au-kind { position: absolute; right: -6px; bottom: -4px; display: grid; place-items: center; width: 20px; height: 20px; border-radius: 50%; background: var(--c); color: #0b0b10; box-shadow: 0 0 0 2.5px var(--page); }
.au-kind .icon { width: 11px; height: 11px; stroke-width: 2.4; }
.au-head { grid-column: 3; min-width: 0; padding-top: 8px; }
.au-more { grid-column: 3; display: flex; flex-direction: column; gap: 6px; min-width: 0; margin-top: 6px; }
.au-more:empty { display: none; }
.au-plain, .au-reason { max-width: 70ch; }
.au-line { font-size: 13.5px; color: var(--ink-2); line-height: 1.45; }
.au-line strong { font-weight: 600; color: var(--ink); }
.au-by { display: inline-flex; align-items: center; gap: 6px; vertical-align: -4px; }
.au-by .avatar { width: 18px; height: 18px; }
.au-plain { font-size: 12.5px; color: var(--ink-2); }
.au-new { color: var(--waiting); font-weight: 500; }
.au-reason { font-size: 12.5px; color: var(--ink-2); }
/* A deleted message, on Discord's grey. */
.au-quote { align-self: flex-start; max-width: 100%; padding: 7px 12px; border-radius: 4px 12px 12px 12px; background: #2b2d31; color: #dbdee1; font-size: 13px; line-height: 1.45; white-space: pre-wrap; overflow-wrap: anywhere; }
.au-diff { display: flex; flex-direction: column; gap: 3px; align-self: flex-start; max-width: 100%; font-size: 13px; }
.au-diff del, .au-diff ins { display: flex; gap: 8px; padding: 4px 10px 4px 8px; border-radius: 8px; text-decoration: none; overflow-wrap: anywhere; white-space: pre-wrap; }
.au-diff del { background: color-mix(in oklab, var(--held) 11%, transparent); color: color-mix(in oklab, var(--ink) 70%, var(--held)); }
.au-diff del { text-decoration: line-through; text-decoration-color: color-mix(in oklab, var(--held) 60%, transparent); }
.au-diff ins { background: color-mix(in oklab, var(--live) 11%, transparent); color: var(--ink); }
.au-sign { font-family: var(--mono); font-weight: 600; opacity: 0.8; text-decoration: none; display: inline-block; }
.au-diff del .au-sign { color: var(--held); }
.au-diff ins .au-sign { color: var(--live); }
.au-names { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; font-size: 13px; }
.au-names .icon { width: 13px; height: 13px; color: var(--ink-3); transform: rotate(-90deg); }
.au-name { padding: 2px 10px; border-radius: 11px; background: var(--control); }
.au-name.was { color: var(--ink-3); text-decoration: line-through; text-decoration-color: var(--edge-3); }
.au-name.now { color: var(--ink); background: color-mix(in oklab, var(--c) 16%, transparent); }
.au-name.none { font-style: italic; text-decoration: none; }
.au-roles { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; }
.au-role { display: inline-flex; align-items: center; gap: 6px; height: 24px; padding: 0 10px 0 7px; border-radius: 12px; background: color-mix(in oklab, var(--role) 14%, transparent); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--role) 30%, transparent); font-size: 12px; color: var(--ink); }
.au-role .dot { width: 8px; height: 8px; background: var(--role); }
.au-role .au-sign { color: var(--live); }
.au-role.removed { background: var(--control); box-shadow: none; color: var(--ink-3); text-decoration: line-through; }
.au-role.removed .au-sign { color: var(--held); }

/* The rail. */
.au-mods { display: flex; flex-direction: column; gap: 6px; }
.au-mod { animation: rise 0.3s var(--ease) both; animation-delay: calc(var(--s, 0) * 30ms + 100ms); }
.au-mod .grow { gap: 5px; }
.au-mod .meter { height: 4px; }
.au-mod .meter > span { background: var(--data); animation-duration: 0.6s; animation-delay: calc(var(--s, 0) * 30ms + 150ms); }
.au-mod > b { font-size: 15px; font-weight: 600; color: var(--ink); min-width: 22px; text-align: right; font-variant-numeric: tabular-nums; }
.au-axis { display: flex; justify-content: space-between; margin-top: -4px; color: var(--ink-3); font-size: 11.5px; }

.au-lane-ic { display: grid; place-items: center; width: 30px; height: 30px; flex: none; border-radius: 50%; box-shadow: inset 0 0 0 1px var(--edge-2); color: var(--c); }
.au-lane-ic .icon { width: 15px; height: 15px; }
.au-send { display: flex; flex-direction: column; align-items: flex-start; gap: 8px; padding-top: 4px; }
.au-kinds { display: flex; flex-wrap: wrap; gap: 6px; }
.au-kindchip { display: inline-flex; align-items: center; gap: 7px; height: 26px; padding: 0 10px; border-radius: 13px; font-size: 12.5px; color: var(--ink-2); background: var(--control); }

/* A wide column: what happened on the left, what it was in a column of its own beside it, so a
   line of the log reads across once instead of leaving most of the width empty. */
@container col (min-width: 1000px) {
  .au-entry { grid-template-columns: 42px 36px minmax(0, 420px) minmax(0, 1fr); }
  /* A hairline under the text of each line, so the eye carries across to its details. */
  .au-entry + .au-entry::before { content: ""; position: absolute; top: 0; left: 114px; right: 10px; height: 1px; background: var(--edge-1); }
  .au-more { grid-column: 4; grid-row: 1 / span 2; margin-top: 0; padding-top: 5px; }
  .au-more > .au-plain:first-child, .au-more > p.muted:first-child { padding-top: 3px; }
  .au-search { width: 300px; }
}
/* Laid out for the column's own width, so the sidebar and rail are already counted. */
@container col (max-width: 700px) {
  .au-readings { grid-template-columns: repeat(2, minmax(0, 1fr)); }
}
@container col (max-width: 600px) {
  .au-filters { flex-direction: column; align-items: stretch; }
  .au-search { width: 100%; }
  .au-entry { grid-template-columns: 36px minmax(0, 1fr); column-gap: 14px; }
  .au-time { display: none; }
  .au-entry > .au-face { grid-column: 1; }
  .au-head, .au-more { grid-column: 2; }
  .au-list::before { left: 18px; }
  .au-entry:hover { background: none; }
}

/* Settings: the sections in two columns once each can hold a label beside its switch. Members,
   the longest, has the right column to itself; the rest stack on the left without gaps. */
.au-sets { display: flex; flex-direction: column; gap: 18px; }
@container col (min-width: 1060px) {
  .au-sets { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); grid-template-rows: auto auto 1fr; gap: 18px 24px; align-items: start; }
  .au-sets > :nth-child(1) { grid-area: 1 / 1; }
  .au-sets > :nth-child(2) { grid-area: 2 / 1; }
  .au-sets > :nth-child(3) { grid-area: 1 / 2 / 4 / 3; }
  .au-sets > :nth-child(4) { grid-area: 3 / 1; }
}
"#;

#[derive(Deserialize)]
struct SentQuery {
    sent: Option<String>,
}

async fn settings_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<SentQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let body = html! {
        (style())
        @if q.sent.is_some() {
            @let to: Vec<String> = crate::test_destinations(&s).iter().map(|(c, _)| channel_label(&ctx, *c)).collect();
            (sent_toast(&format!("Test entry sent to {}", to.join(" and "))))
        }
        (tabs(&page, "Settings"))
        div.with-rail {
            (ui::settings_form(&page, &page.path("audit/settings"), html! {
                // Members, the longest section, gets the right column to itself on a wide screen.
                div.au-sets {
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
                }
            }))
            aside.rail { (routing(&ctx, &page, &s)) }
        }
    };
    Ok(ui::layout(&ctx, &page, "Audit log", None, body).into_response())
}

/// Where each kind of entry is posted, as the saved settings have it, in the timeline's colours,
/// and a test entry to see a post land there.
fn routing(ctx: &Ctx, page: &Page, s: &Settings) -> Markup {
    let on: Vec<Kind> = Kind::ALL.into_iter().filter(|k| s.on(*k)).collect();
    // The switches: timeouts and voice each turn on several kinds at once.
    let switches = [s.message_delete, s.message_edit, s.bulk_delete, s.join, s.leave, s.nickname, s.roles, s.ban, s.unban, s.timeout, s.boost, s.voice];
    let lanes = [("Messages", Kind::MessageEdit, Icon::Brush), ("Members", Kind::Join, Icon::Member)];
    html! {
        div.rail-block {
            h2 { "Where entries go" }
            @for (label, kind, ic) in lanes {
                @let to = if kind == Kind::MessageEdit { s.message_channel.or(s.channel) } else { s.member_channel.or(s.channel) };
                div.mini.au-lane {
                    span.au-lane-ic style={ "--c:" (kind_color(kind)) } { (ui::icon(ic)) }
                    div.grow {
                        span { (label) }
                        span.muted { @match to { Some(c) => { "Posted in " (channel_label(ctx, c)) }, None => { "Only here: no channel picked" } } }
                    }
                }
            }
            @if page.can_edit() {
                div.au-send {
                    form.inline method="post" action=(page.path("audit/test")) {
                        button.btn.small.outline type="submit" { (ui::icon(Icon::Log)) "Send a test entry" }
                    }
                    p.muted.small {
                        @if crate::test_destinations(s).is_empty() { "Pick a log channel and save, then send a test." }
                        @else { "Posts a test entry where entries go, as saved. Nothing is added to the log." }
                    }
                }
            }
        }
        div.rail-block {
            h2 { "Logging" span.muted.small { (switches.iter().filter(|x| **x).count()) " of " (switches.len()) } }
            @if on.is_empty() {
                p.muted.small { "Every switch is off, so nothing is logged." }
            } @else {
                div.au-kinds {
                    @for k in &on {
                        span.au-kindchip style={ "--c:" (kind_color(*k)) } { span.au-swatch style={ "--c:" (kind_color(*k)) } {} (k.label()) }
                    }
                }
            }
            p.muted.small { "Entries stay on the Recent tab for 14 days." }
        }
    }
}

/// Post a test entry in each log channel, as saved.
async fn send_test(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    if crate::test_destinations(&s).is_empty() {
        return Err(WebError::Bad("Pick a log channel and save first.".into()));
    }
    let sent = match crate::send_test(&ctx, guild, &s, page.user.id).await {
        Ok(sent) => sent,
        Err(e) => {
            bot_core::tracing::warn!(guild, error = %e, "test audit entry failed");
            return Err(WebError::Bad(bot_core::discord::why_refused_any(&e)));
        }
    };
    let names: Vec<String> = sent.iter().map(|c| channel_label(&ctx, *c)).collect();
    ctx.audit(&page.user, Some(guild), KEY, format!("Sent a test audit log entry to {}", names.join(" and ")));
    Ok(Redirect::to(&format!("{}?sent=1", page.path("audit/settings"))).into_response())
}

/// A toast once the page is back, then the flag leaves the address.
fn sent_toast(msg: &str) -> Markup {
    // A JS string; `<` escaped so the text can't close the script.
    let msg = format!("{msg:?}").replace('<', "\\u003c");
    html! {
        script { (PreEscaped(format!(
            "(()=>{{const go=()=>{{window.botToast&&window.botToast({msg});const u=new URL(location.href);u.searchParams.delete('sent');history.replaceState(history.state,'',u.pathname+u.search+u.hash)}};document.readyState==='loading'?addEventListener('DOMContentLoaded',go,{{once:true}}):setTimeout(go)}})()"
        ))) }
    }
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
