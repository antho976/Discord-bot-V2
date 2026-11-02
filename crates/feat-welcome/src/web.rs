use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use bot_core::axum::response::{IntoResponse, Redirect};
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::twilight_model::id::Id;
use bot_core::web::dc;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};

use crate::banners::{self, Kind};
use crate::{DEFAULT_GOODBYE, DEFAULT_WELCOME, KEY, Settings, card, invites, roles};

const DAY_MS: i64 = 86_400_000;

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/welcome", get(messages_page).post(save_messages))
        .route("/g/{guild}/welcome/card.png", get(card_png))
        .route("/g/{guild}/welcome/pictures/{kind}", post(add_picture))
        .route("/g/{guild}/welcome/picture/{id}", get(picture))
        .route("/g/{guild}/welcome/picture/{id}/remove", post(remove_picture))
        .route("/g/{guild}/welcome/test", post(send_test))
        .route("/g/{guild}/welcome/test-goodbye", post(send_goodbye_test))
        .route("/g/{guild}/welcome/roles", get(roles_page).post(save_roles))
        .route("/g/{guild}/welcome/invites", get(invites_page).post(save_invites))
        .route("/g/{guild}/welcome/invites/names", post(save_invite_names))
}

fn tabs(page: &Page, active: &str) -> Markup {
    ui::tabs(&[("Messages", page.path("welcome")), ("Roles", page.path("welcome/roles")), ("Invites", page.path("welcome/invites"))], active)
}

/// Where a test posts: "#welcome", or "the saved channel" before one is picked.
fn test_channel(ctx: &Ctx, guild: u64, id: Option<u64>) -> String {
    id.and_then(|id| ctx.text_channels(guild).into_iter().find(|(c, _)| *c == id)).map(|(_, n)| format!("#{n}")).unwrap_or_else(|| "the saved channel".into())
}

/// "@Regulars, @Idleon", with roles deleted since counted at the end: "@Regulars and 1 deleted role".
fn role_names(ctx: &Ctx, guild: u64, ids: &[u64]) -> String {
    let known = ctx.roles(guild);
    let names: Vec<String> = ids.iter().filter_map(|id| known.iter().find(|(r, _, _)| r == id)).map(|(_, n, _)| format!("@{n}")).collect();
    let gone = ids.len() - names.len();
    match (names.is_empty(), gone) {
        (_, 0) => names.join(", "),
        (true, _) => counted(gone as i64, "deleted role", "deleted roles"),
        (false, _) => format!("{} and {}", names.join(", "), counted(gone as i64, "deleted role", "deleted roles")),
    }
}

/// "1 new member", "8 new members".
fn counted(n: i64, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{} {many}", text::num(n)) }
}

/// "12 min", "1 h": the short form for times within the day.
fn short_ago(ms: i64) -> String {
    let mins = ((now_ms() - ms) / 60_000).max(0);
    match mins {
        0 => "just now".into(),
        1..60 => format!("{mins} min"),
        _ => format!("{} h", mins / 60),
    }
}

/// The viewer's midnight today, in epoch milliseconds.
fn day_start(page: &Page) -> i64 {
    let now = page.user.local(now_ms());
    now.start_of_day().map(|d| d.timestamp().as_millisecond()).unwrap_or_else(|_| now.timestamp().as_millisecond() - DAY_MS)
}

/// Members who left since `start`. The daily counters are kept by UTC day, so this takes every
/// UTC day from the one `start` falls on, which can include a few hours of the day before.
fn left_since(ctx: &Ctx, guild: u64, start: i64) -> i64 {
    let from = bot_core::jiff::Timestamp::from_millisecond(start).unwrap_or_default().to_zoned(bot_core::jiff::tz::TimeZone::UTC).date().to_string();
    ctx.db
        .with(|c| {
            c.query_row(
                "SELECT COALESCE(SUM(value), 0) FROM daily_stats WHERE guild_id = ?1 AND key = 'leaves' AND day >= ?2",
                bot_core::rusqlite::params![guild as i64, from],
                |r| r.get(0),
            )
        })
        .unwrap_or(0)
}

/// How many of today's faces the lead shows: narrow, wider, widest (CSS, .wl-faces).
const FACES: [usize; 3] = [5, 8, 11];

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// "October 31, 2026": the line Discord draws between days.
fn long_date(page: &Page, ms: i64) -> String {
    let z = page.user.local(ms);
    format!("{} {}, {}", MONTHS[(z.month() - 1).clamp(0, 11) as usize], z.day(), z.year())
}



/// `(name, colour)` for roles that still exist, in the order given.
fn role_list(ctx: &Ctx, guild: u64, ids: &[u64]) -> Vec<(String, String)> {
    let known = ctx.roles(guild);
    ids.iter()
        .filter_map(|id| known.iter().find(|(r, _, _)| r == id))
        .map(|(_, name, color)| (name.clone(), if *color == 0 { "var(--ink-3)".to_owned() } else { format!("#{color:06x}") }))
        .collect()
}

/// Role pills with their colour dots.
fn role_pills(roles: &[(String, String)]) -> Markup {
    html! { span.wl-roles { @for (name, color) in roles { span.wl-role style={ "--c:" (color) } { i {} (name) } } } }
}

/// A message template as Discord shows it: `{user}` becomes a mention when `mention` is on, the
/// rest is filled like the bot fills it.
fn preview_text(template: &str, name: &str, server: &str, count: &str, mention: bool) -> Markup {
    let vars = [("name", name), ("server", server), ("count", count)];
    html! {
        @for (i, part) in template.split("{user}").enumerate() {
            @if i > 0 { @if mention { span.dp-ping { "@" (name) } } @else { (name) } }
            (crate::fill(part, &vars))
        }
    }
}

/// A member's profile card as Discord shows it, with the roles they hold.
fn popout(avatar: &str, name: &str, username: &str, roles: &[(String, String)], tone: &str) -> Markup {
    html! {
        div.wl-pop {
            div class={ "wl-pop-banner " (tone) } {}
            img.wl-pop-av src=(avatar) alt="";
            div.wl-pop-body {
                strong.wl-pop-name { (name) }
                @if !username.is_empty() { span.wl-pop-user { (username) } }
                div.wl-pop-sec {
                    h3 { "Roles" }
                    div.wl-pop-roles {
                        @if roles.is_empty() { span.wl-pop-none { "No roles" } }
                        @for (n, c) in roles { span.wl-pop-role style={ "--c:" (c) } { i {} (n) } }
                    }
                }
            }
        }
    }
}

const CSS: &str = r#"
/* Welcome, in the house style (2026-11-02). Every section and side block sits in one kind of
   bordered panel, never a panel inside a panel. Two columns: the main one, and a right column
   sized to a Discord preview. The Messages tab leads with today's faces; Roles and Invites get the
   same panels and colour, no lead. Nothing counts up, floats or shines by itself. */
.wl-panel { display: flex; flex-direction: column; gap: 14px; min-width: 0; background: var(--raised); border: 1px solid var(--edge-1); border-radius: 14px; padding: 18px 20px; }
.wl-panel h2 { font-size: 14px; font-weight: 500; }
.wl-panel .block-head p { font-size: 13px; max-width: 68ch; }
.wl-split { display: grid; grid-template-columns: minmax(0, 1fr); gap: 16px; align-items: start; }
@container col (min-width: 1000px) { .wl-split { grid-template-columns: minmax(0, 1fr) clamp(340px, 32%, 480px); } }
.page-body > .wl-lead + .wl-split, .page-body > .wl-needs + .wl-split { margin-top: -8px; }
.wl-col { display: flex; flex-direction: column; gap: 16px; min-width: 0; container: wlcol / inline-size; }
/* An empty state inside a panel is a line, not a second box; its icon is neutral and still. */
.wl-panel .empty { padding: 6px 0; border-radius: 0; background: none; box-shadow: none; }
.wl-panel .empty-icon { background: var(--control); color: var(--ink-2); box-shadow: none; animation: none; }

/* Settings: the form is the panel, its groups split by a hairline, Save at its foot. */
.wl-form .settings { gap: 16px; }
.wl-form .settings fieldset { gap: 0; }
.wl-group { display: flex; flex-direction: column; gap: 4px; }
.wl-group + .wl-group { margin-top: 18px; padding-top: 18px; border-top: 1px solid var(--edge-1); }
.wl-rows .row { padding: 12px 0; }
.wl-rows .row:last-child { padding-bottom: 0; }
@container wlcol (max-width: 560px) {
  .wl-rows .row { grid-template-columns: minmax(0, 1fr); gap: 8px; }
  .wl-rows .row:has(.row-control > .switch) { grid-template-columns: minmax(0, 1fr) auto; }
}

/* Messages: two sections, Welcome then Goodbye. Each opens with its switch and its name, so
   whether it posts at all reads first; under it, indented to the name, where it posts beside
   what it says, then its card. Labels sit over their controls. A section that is off dims. */
.wl-sec { display: flex; flex-direction: column; gap: 20px; min-width: 0; }
.wl-sec + .wl-sec { margin-top: 6px; padding-top: 26px; border-top: 1px solid var(--edge-1); }
.wl-sec-head { display: flex; align-items: flex-start; gap: 14px; }
.wl-sec-head .switch { flex: none; margin-top: 1px; }
.wl-sec-title { display: flex; flex-direction: column; gap: 2px; cursor: pointer; }
.wl-sec-title h2 { font-size: 16px; font-weight: 600; letter-spacing: -0.01em; line-height: 24px; }
.wl-sec-title span { font-size: 13px; color: var(--ink-3); }
.wl-sec-body { display: flex; flex-direction: column; gap: 20px; padding-left: 54px; min-width: 0; transition: opacity 0.2s var(--ease); }
.wl-sec:has(> .wl-sec-head input:not(:checked)) > .wl-sec-body { opacity: 0.5; }
.wl-field { display: flex; flex-direction: column; gap: 7px; min-width: 0; }
.wl-field > label, .wl-cardswitch label { font-size: 13px; font-weight: 500; }
.wl-field .help { margin-top: 0; }
.wl-what { display: grid; grid-template-columns: minmax(0, 1fr); gap: 16px; }
.wl-msg textarea.input { min-height: 112px; }
.wl-cardopts { display: grid; grid-template-columns: minmax(0, 1fr); gap: 14px; padding: 14px 16px 16px; border-radius: 10px; background: var(--well); }
.wl-cardswitch { display: flex; align-items: flex-start; gap: 12px; min-width: 0; }
.wl-cardswitch .switch { flex: none; }
.wl-cardswitch .help { margin-top: 2px; }
.wl-cardopts > .wl-field { padding-left: 52px; }
.wl-cardopts .select-wrap { max-width: 320px; }
.wl-cardopts:has(.wl-cardswitch input:not(:checked)) > .wl-field { opacity: 0.5; }
.wl-what .wl-field:first-child .select-wrap { max-width: 320px; }
@container wlcol (min-width: 620px) {
  .wl-what { grid-template-columns: 220px minmax(0, 1fr); gap: 24px; }
}
@container wlcol (max-width: 480px) { .wl-sec-body, .wl-cardopts > .wl-field { padding-left: 0; } }

/* Small figures: a panel's head on the tabs without a lead, and beside the lead's number. */
.wl-head { display: flex; align-items: flex-start; justify-content: space-between; gap: 12px 32px; flex-wrap: wrap; }
.wl-head > .block-head { flex: 1 1 260px; min-width: 0; }
.wl-figs { display: flex; gap: 12px 28px; flex-wrap: wrap; }
.wl-figs > div { display: flex; flex-direction: column; min-width: 0; }
.wl-figs b { font-size: 20px; font-weight: 600; line-height: 1.2; font-variant-numeric: tabular-nums; white-space: nowrap; }
.wl-figs span { font-size: 12px; color: var(--ink-3); white-space: nowrap; }
.wl-figs-wide { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 12px 16px; }
.wl-figs-wide span { white-space: normal; }

/* Messages: the lead. Today's count, then a face per member, as many as the page holds in one
   row. A wider page shows more faces and swaps in the "more" face that counts the rest. */
.wl-lead { gap: 18px; }
.wl-lead-head { display: flex; align-items: flex-end; justify-content: space-between; gap: 12px 32px; flex-wrap: wrap; }
.wl-lead-num { display: flex; align-items: baseline; gap: 10px; }
.wl-lead-num b { font-size: 40px; font-weight: 600; line-height: 1; letter-spacing: -0.02em; font-variant-numeric: tabular-nums; }
.wl-lead-num span { font-size: 14px; color: var(--ink-2); }
.wl-faces { display: grid; grid-template-columns: repeat(auto-fill, minmax(100px, 1fr)); gap: 16px 12px; }
.wl-face.x8, .wl-face.x11, .wl-face.m8, .wl-face.m11 { display: none; }
@container col (min-width: 1000px) {
  .wl-face.x8, .wl-face.m8 { display: flex; }
  .wl-face.m5 { display: none; }
}
@container col (min-width: 1340px) {
  .wl-face.x11, .wl-face.m11 { display: flex; }
  .wl-face.m8 { display: none; }
}
.wl-face { display: flex; flex-direction: column; align-items: center; gap: 2px; min-width: 0; text-align: center; }
.wl-face .avatar, .wl-more { width: 56px; height: 56px; margin-bottom: 8px; }
.wl-face strong { max-width: 100%; font-weight: 500; font-size: 14px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.wl-when { max-width: 100%; font-size: 13px; color: var(--ink-2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.wl-face.back .wl-when { color: var(--data); }
.wl-more { display: grid; place-items: center; border-radius: 50%; background: var(--control); color: var(--ink-2); font: 600 15px var(--sans); font-variant-numeric: tabular-nums; }

/* The preview column: one post (or profile) at a time, switched in its head, at three quarters
   of Discord's size. */
.wl-preview { gap: 12px; }
.wl-pv-head { display: flex; align-items: center; justify-content: space-between; gap: 10px 16px; flex-wrap: wrap; }
.wl-pv { display: flex; flex-direction: column; gap: 12px; min-width: 0; }
.wl-preview:has(input[value="goodbye"]:checked) .wl-pv-welcome,
.wl-preview:has(input[value="welcome"]:checked) .wl-pv-goodbye,
.wl-preview:has(input[value="back"]:checked) .wl-pv-new,
.wl-preview:has(input[value="new"]:checked) .wl-pv-back { display: none; }
.wl-preview .dp { zoom: 0.75; max-width: none; }
.wl-send { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
.wl-send .muted { font-size: 12.5px; }

/* Discord, drawn the way members see it (bot_core::web::dc), with the line it draws between days. */
.wl-day { display: flex; align-items: center; gap: 8px; margin: -2px 0; color: #949ba4; font-size: 12px; font-weight: 600; }
.wl-day::before, .wl-day::after { content: ""; flex: 1; height: 1px; background: #3f4147; }
.wl-card { position: relative; display: block; width: 100%; max-width: 440px; margin-top: 8px; border-radius: 8px; overflow: hidden; aspect-ratio: 3 / 1; background: #2b2d31; }
.wl-card[hidden] { display: none; }
.wl-card img { display: block; width: 100%; height: 100%; object-fit: cover; }

/* A card's pictures: a strip of small tiles under its preview. */
.wl-pics-group { display: flex; flex-direction: column; gap: 8px; padding-top: 14px; border-top: 1px solid var(--edge-1); }
.wl-pics-head { display: flex; align-items: baseline; justify-content: space-between; gap: 8px; }
.wl-pics-head strong { font-size: 13px; font-weight: 500; }
.wl-pics { display: grid; grid-template-columns: repeat(auto-fill, minmax(130px, 1fr)); gap: 8px; }
.wl-pic { position: relative; margin: 0; aspect-ratio: 3 / 1; border-radius: 8px; overflow: hidden; background: var(--well); box-shadow: inset 0 0 0 1px var(--edge-1); }
.wl-pic img { display: block; width: 100%; height: 100%; object-fit: cover; }
.wl-pic form { position: absolute; top: 4px; right: 4px; }
.wl-pic .icon-btn { background: rgba(8, 8, 14, 0.72); }
.wl-pic.add { display: grid; place-items: center; }

/* People in the side column: 28 px faces, 14 px names, 13 px lines. */
.wl-inv { display: grid; grid-template-columns: 28px minmax(0, 1fr) auto; gap: 12px; align-items: center; font-size: 14px; }
.wl-inv .avatar { width: 28px; height: 28px; }
.wl-inv .grow { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.wl-inv .grow > span:first-child { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.wl-inv .meter { height: 4px; }
.wl-inv .meter > span, .wl-uses .meter > span { background: var(--data); }
.wl-n { min-width: 22px; text-align: right; font-size: 13px; font-weight: 600; font-variant-numeric: tabular-nums; }
.wl-back { display: flex; align-items: center; gap: 12px; }
.wl-back p { font-size: 13px; color: var(--ink-2); }
.wl-back .avatars .avatar { width: 28px; height: 28px; }
.wl-panel .mini > .avatar { width: 28px !important; height: 28px !important; }
.wl-panel .mini .grow > span:first-child { font-size: 14px; }
.wl-panel .mini .grow > .muted { font-size: 13px; }
/* Roles as Discord shows them: a dot in the role's colour on a quiet chip. */
.wl-roles { display: flex; flex-wrap: wrap; gap: 6px; }
.wl-role { display: inline-flex; align-items: center; gap: 7px; height: 24px; padding: 0 9px; border-radius: 7px; background: var(--control); color: var(--ink); font-size: 12.5px; font-weight: 500; white-space: nowrap; }
.wl-role i { flex: none; width: 8px; height: 8px; border-radius: 50%; background: var(--c); }

/* Roles: the profile a member sees, as Discord's popout, under its caption. */
.wl-cap { display: flex; align-items: center; gap: 8px; font-size: 13px; color: var(--ink-2); }
.wl-cap b { color: var(--ink); font-weight: 500; }
.wl-pop { position: relative; max-width: 340px; border-radius: 12px; overflow: hidden; background: #232428; color: #dbdee1; box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.06); }
.wl-pop-banner { height: 60px; background: color-mix(in oklab, var(--hue) 55%, #000); }
.wl-pop-banner.back { background: color-mix(in oklab, var(--data-2) 60%, #000); }
.wl-pop-av { position: absolute; top: 20px; left: 14px; width: 72px; height: 72px; border-radius: 50%; object-fit: cover; box-shadow: 0 0 0 6px #232428; }
.wl-pop-body { display: flex; flex-direction: column; margin: 44px 12px 12px; padding: 12px; border-radius: 8px; background: #111214; }
.wl-pop-name { color: #f2f3f5; font-size: 17px; font-weight: 700; letter-spacing: -0.01em; }
.wl-pop-user { color: #b5bac1; font-size: 13px; }
.wl-pop-sec { margin-top: 12px; padding-top: 12px; border-top: 1px solid #2e2f34; }
.wl-pop-sec h3 { font-size: 12px; font-weight: 700; color: #f2f3f5; }
.wl-pop-roles { display: flex; flex-wrap: wrap; gap: 4px; margin-top: 8px; }
.wl-pop-role { display: inline-flex; align-items: center; gap: 5px; height: 24px; padding: 0 8px; border-radius: 4px; background: #2b2d31; font-size: 12px; font-weight: 500; color: #dbdee1; }
.wl-pop-role i { width: 12px; height: 12px; border-radius: 50%; background: var(--c); }
.wl-pop-none { font-size: 12px; color: #949ba4; }
.wl-waiting { gap: 16px; }
.wl-sub { display: flex; flex-direction: column; gap: 8px; padding-top: 16px; border-top: 1px solid var(--edge-1); }

/* Lists of people in a panel (Recently restored, Recent joins): rows with a hairline between,
   down two columns when the column is wide. */
.wl-list-in { display: flex; flex-direction: column; }
.wl-row { display: flex; align-items: center; gap: 12px; min-width: 0; padding: 10px 0; }
.wl-list-in > .wl-row + .wl-row { border-top: 1px solid var(--edge-1); }
.wl-row > .avatar { width: 28px; height: 28px; }
.wl-row .grow { display: flex; flex-direction: column; gap: 3px; min-width: 0; flex: 1; }
.wl-row strong { font-weight: 500; font-size: 14px; }
.wl-row .meta { color: var(--ink-3); font-size: 12.5px; white-space: nowrap; }
.wl-row.gone > .avatar { filter: grayscale(1); opacity: 0.6; }
.wl-row.gone strong { color: var(--ink-2); }
@container wlcol (min-width: 640px) {
  .wl-list-in { display: block; columns: 2; column-gap: 32px; margin-top: -1px; clip-path: inset(1px -20px -40px -20px); }
  .wl-list-in > .wl-row { break-inside: avoid; border-top: 1px solid var(--edge-1); }
}
/* Recent joins are short rows: three across on a wide column, so the list ends near the side. */
@container wlcol (min-width: 1020px) { .wl-list-in.wl-joins { columns: 3; column-gap: 28px; } }
.wl-via { display: inline-flex; align-items: center; gap: 6px; color: var(--ink-2); font-size: 13px; min-width: 0; flex-wrap: wrap; }
.wl-via .avatar { width: 18px; height: 18px; }
.wl-via .wl-tag { color: var(--ink); font-weight: 500; }
.wl-via .wl-tag::before { content: "·"; margin-right: 6px; color: var(--ink-3); font-weight: 400; }
.wl-via .wl-c { color: var(--ink-3); }

/* Invites: tracking, then the links, each a row with its uses and a name field under them. */
.wl-status { display: flex; align-items: center; gap: 8px 10px; flex-wrap: wrap; font-size: 13px; }
.wl-status .status { background: var(--control); backdrop-filter: none; }
.wl-track .settings { gap: 12px; }
.wl-track .switch-row { padding: 0; }
.wl-track .save-bar { padding-top: 2px; }
.wl-chart .area svg { height: 200px; }
.wl-links { display: flex; flex-direction: column; }
.wl-link { display: grid; grid-template-columns: minmax(0, 1fr) minmax(90px, 140px); gap: 8px 16px; align-items: center; padding: 12px 0; }
.wl-link + .wl-link { border-top: 1px solid var(--edge-1); }
.wl-link .input { grid-column: 1 / -1; width: 100%; }
.wl-link .grow { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
.wl-code { font: 600 13.5px var(--mono); color: var(--ink); }
.wl-uses { display: flex; flex-direction: column; gap: 6px; font-size: 12.5px; color: var(--ink-2); }
.wl-uses b { color: var(--ink); font-weight: 600; font-variant-numeric: tabular-nums; }
.wl-uses .meter { height: 4px; }
.wl-links-foot { display: flex; justify-content: flex-end; padding-top: 4px; }
@media (max-width: 920px) { .wl-row { flex-wrap: wrap; } }
"#;

/// The preview switch (welcome or goodbye, new or returning member) is kept for this tab, so an
/// upload, a test or a save comes back to the same one.
const KEEP_JS: &str = r##"
(() => {
  document.querySelectorAll('input[name="wl-which"], input[name="wl-who"]').forEach((r) => {
    try { if (sessionStorage.getItem(r.name) === r.value) r.checked = true; } catch (_) {}
    r.addEventListener("change", () => { try { sessionStorage.setItem(r.name, r.value); } catch (_) {} });
  });
})();
"##;

const SCRIPT: &str = r##"
(() => {
  // A card picture: shrunk in the browser to twice the card's size at most, then sent as is.
  document.querySelectorAll("[data-wl-pick]").forEach((btn) => {
    const file = btn.parentElement.querySelector('input[type="file"]');
    btn.addEventListener("click", () => file.click());
    file.addEventListener("change", async () => {
      const f = file.files?.[0];
      if (!f) return;
      if (!f.type.startsWith("image/")) return window.botToast?.("Pick a PNG, JPEG or WebP image", true);
      btn.classList.add("busy");
      try {
        let body = f, type = f.type;
        if (f.type !== "image/gif") {
          const bmp = await createImageBitmap(f);
          const k = Math.min(1, 1920 / bmp.width, 1280 / bmp.height);
          const c = document.createElement("canvas");
          c.width = Math.max(1, Math.round(bmp.width * k));
          c.height = Math.max(1, Math.round(bmp.height * k));
          const g = c.getContext("2d");
          g.imageSmoothingQuality = "high";
          g.drawImage(bmp, 0, 0, c.width, c.height);
          body = await new Promise((res) => c.toBlob(res, "image/webp", 0.86));
          type = "image/webp";
          if (!body) { body = await new Promise((res) => c.toBlob(res, "image/jpeg", 0.88)); type = "image/jpeg"; }
        }
        const r = await fetch(file.dataset.action, { method: "POST", headers: { "content-type": type }, body, credentials: "same-origin" });
        if (!r.ok) throw new Error((await r.text()).slice(0, 200) || "The upload didn't go through");
        location.href = location.pathname + "?saved=1#wl-pictures";
      } catch (err) {
        btn.classList.remove("busy");
        window.botToast?.(err.message || "The upload didn't go through", true);
      } finally {
        file.value = "";
      }
    });
  });
  const box = document.getElementById("wl-preview");
  if (!box) return;
  const v = { name: box.dataset.name, server: box.dataset.server, count: box.dataset.members };
  const $ = (id) => document.getElementById(id);
  const draw = (el, tpl, mention) => {
    el.textContent = "";
    tpl.split("{user}").forEach((part, i) => {
      if (i > 0) {
        if (mention) { const s = document.createElement("span"); s.className = "dp-ping"; s.textContent = "@" + v.name; el.append(s); }
        else el.append(v.name);
      }
      el.append(part.replace(/\{(name|server|count)\}/g, (_, k) => v[k]));
    });
  };
  // The channel's name in the bar above a message.
  const chan = (sel, msg) => { const o = $(sel).selectedOptions[0]; $(msg).closest(".dp").querySelector(".dp-bar .clip").textContent = o && o.value ? o.text.replace(/^#/, "") : "pick a channel"; };
  const sync = () => {
    draw($("wl-welcome-text"), $("welcome_text").value.trim() || box.dataset.defaultWelcome, true);
    draw($("wl-goodbye-text"), $("goodbye_text").value.trim() || box.dataset.defaultGoodbye, false);
    $("wl-card").hidden = !$("welcome_card").checked;
    $("wl-gcard").hidden = !$("goodbye_card").checked;
    $("wl-goodbye").classList.toggle("off", !$("goodbye_on").checked);
    $("wl-welcome").classList.toggle("off", !$("welcome_on").checked);
    chan("welcome_channel", "wl-welcome");
    chan("goodbye_channel", "wl-goodbye");
  };
  ["welcome_text", "goodbye_text", "welcome_card", "goodbye_card", "goodbye_on", "welcome_on", "welcome_channel", "goodbye_channel"].forEach((id) => {
    $(id).addEventListener("input", sync);
    $(id).addEventListener("change", sync);
  });
})();
"##;

#[derive(serde::Deserialize)]
struct SentQuery {
    /// Set after a test post went out, for the toast.
    sent: Option<String>,
}

async fn messages_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<SentQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let now = now_ms();
    let count = ctx.member_count(guild).map(|n| text::num(n as i64)).unwrap_or_else(|| "1".into());
    let today = day_start(&page);
    let (joins, _) = invites::join_counts(&ctx, guild, today)?;
    // Up to eleven faces: a wider column shows more of them in its one row (CSS, .wl-faces).
    let shown = invites::joins_since(&ctx, guild, today, FACES[2] as i64)?;
    let left = left_since(&ctx, guild, today);
    let top: Vec<(u64, i64, i64)> = invites::top_inviters(&ctx, guild, now - 30 * DAY_MS, 6)?.into_iter().filter(|t| t.1 > 0).take(4).collect();
    let back: Vec<roles::Restored> = roles::recently_restored(&ctx, guild, 25)?.into_iter().filter(|r| r.at >= now - 7 * DAY_MS).collect();
    let given = role_list(&ctx, guild, &s.autoroles);
    let max_recent = top.iter().map(|t| t.1).max().unwrap_or(1).max(1);
    let names = invites::names(&ctx, guild)?;
    let server = ctx.guild_name(guild);
    // Each face: who, how long ago, and where they came from for the tooltip.
    let faces: Vec<(u64, String, String, Option<String>, bool)> = shown
        .iter()
        .map(|(j, came_back)| {
            let ago = short_ago(j.at);
            let when = if *came_back { format!("Came back {ago}") } else if ago == "just now" { "Just now".to_owned() } else { format!("{ago} ago") };
            let via = if *came_back {
                Some("Rejoined and got their roles back".to_owned())
            } else if let Some(name) = j.code.as_ref().and_then(|c| names.get(c)) {
                Some(format!("Came in through {name}"))
            } else {
                j.inviter.map(|i| format!("Invited by {}", ctx.display_name(guild, i)))
            };
            (j.user, ctx.display_name(guild, j.user), when, via, *came_back)
        })
        .collect();
    let channel = |id: Option<u64>| id.and_then(|c| ctx.text_channels(guild).into_iter().find(|x| x.0 == c)).map(|(_, n)| n).unwrap_or_else(|| "pick a channel".into());
    let welcome_tpl = if s.welcome_text.trim().is_empty() { DEFAULT_WELCOME } else { s.welcome_text.as_str() };
    let goodbye_tpl = if s.goodbye_text.trim().is_empty() { DEFAULT_GOODBYE } else { s.goodbye_text.as_str() };
    let welcome_pictures = banners::list(&ctx, guild, Kind::Welcome)?;
    let goodbye_pictures = banners::list(&ctx, guild, Kind::Goodbye)?;

    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, "Messages"))
        // The lead: who came in today, their faces under the day's figure.
        section.wl-panel.wl-lead {
            div.wl-lead-head {
                div.wl-lead-num { b { (text::num(joins)) } span { (if joins == 1 { "member joined today" } else { "members joined today" }) } }
                div.wl-figs {
                    div { b { (text::num(left)) } span { "left today" } }
                    div { b { (text::num(back.len() as i64)) } span { "came back this week" } }
                }
            }
            @if shown.is_empty() {
                (ui::empty(Icon::Door, "Nobody has joined yet today", "New members show up here with how long ago they came in and who invited them."))
            } @else {
                div.wl-faces {
                    @for (i, (user_id, name, when, via, came_back)) in faces.iter().enumerate() {
                        div.wl-face.back[*came_back].x8[(FACES[0]..FACES[1]).contains(&i)].x11[i >= FACES[1]] data-tip=[via.as_deref()] {
                            img.avatar src=(ctx.member_avatar(guild, *user_id)) alt="" loading="lazy";
                            strong { (name) }
                            span.wl-when { (when) }
                        }
                    }
                    // One "more" tile per width, counting the faces that width leaves out.
                    @for (tier, n) in ["m5", "m8", "m11"].iter().zip(FACES) {
                        @let more = joins - (faces.len().min(n)) as i64;
                        @if more > 0 {
                            div class={ "wl-face " (tier) } {
                                span.wl-more { "+" (more) }
                                strong { (more) " more" }
                                span.wl-when { "Earlier today" }
                            }
                        }
                    }
                }
            }
        }

        // The settings on the left, Save inside their panel; on the right the post they shape,
        // one at a time, with its card's pictures, then who brings members in.
        div.wl-split {
            div.wl-col {
                div.wl-panel.wl-form {
                    (ui::settings_form(&page, &page.path("welcome"), html! {
                        (message_section(&ctx, guild, MessageSection {
                            key: "welcome",
                            title: "Welcome",
                            when: "Posts when someone joins.",
                            on: s.welcome_on,
                            channel: s.welcome_channel,
                            text: &s.welcome_text,
                            placeholder: DEFAULT_WELCOME,
                            text_help: "{user} mentions them, {name} is their name, {server} the server and {count} their member number. Only they are mentioned.",
                            card: s.welcome_card,
                            card_label: "Attach the welcome card",
                            card_help: "Their avatar, name and member number, drawn over one of the card's pictures when there are any.",
                            in_order: s.welcome_pictures_in_order,
                        }))
                        (message_section(&ctx, guild, MessageSection {
                            key: "goodbye",
                            title: "Goodbye",
                            when: "Posts when someone leaves.",
                            on: s.goodbye_on,
                            channel: s.goodbye_channel,
                            text: &s.goodbye_text,
                            placeholder: DEFAULT_GOODBYE,
                            text_help: "{name}, {server} and {count} work here. Nobody is mentioned.",
                            card: s.goodbye_card,
                            card_label: "Attach a goodbye card",
                            card_help: "Their avatar and name, drawn over one of the goodbye pictures when there are any.",
                            in_order: s.goodbye_pictures_in_order,
                        }))
                    }))
                }
            }
            div.wl-col.wl-side {
                section.wl-panel.wl-preview #wl-pictures {
                    div.wl-pv-head {
                        h2 { "What members see" }
                        div.seg role="group" aria-label="Which post" {
                            label { input type="radio" name="wl-which" value="welcome" checked; "Welcome" }
                            label { input type="radio" name="wl-which" value="goodbye"; "Goodbye" }
                        }
                    }
                    div #wl-preview data-name=(user.name) data-server=(server) data-members=(count)
                        data-default-welcome=(DEFAULT_WELCOME) data-default-goodbye=(DEFAULT_GOODBYE) {
                        div.wl-pv.wl-pv-welcome {
                            p.muted.small { "Your own name and card, as a newcomer gets theirs." }
                            (dc::channel(Some(&channel(s.welcome_channel)), "", html! {
                                div.wl-day { span { (long_date(&page, now)) } }
                                (dc::message(&ctx, &page).id("wl-welcome").off(!s.welcome_on).render(html! {
                                    div.dp-line #wl-welcome-text { (preview_text(welcome_tpl, &user.name, &server, &count, true)) }
                                    span.wl-card #wl-card hidden[!s.welcome_card] {
                                        img src=(card_src(&page, Kind::Welcome, welcome_pictures.first().copied())) alt="The welcome card, drawn for you" width="960" height="320";
                                    }
                                }))
                            }))
                            @if page.can_edit() {
                                form.wl-send method="post" action=(page.path("welcome/test")) {
                                    button.btn.small.outline type="submit" { (ui::icon(Icon::Mail)) "Send a test welcome" }
                                    span.muted { "For you, in " (test_channel(&ctx, guild, s.welcome_channel)) ", with the saved settings" }
                                }
                            }
                            (pictures(&page, Kind::Welcome, "Pictures behind the welcome card", &welcome_pictures))
                        }
                        div.wl-pv.wl-pv-goodbye {
                            p.muted.small { "Your own name, as if you had left." }
                            (dc::channel(Some(&channel(s.goodbye_channel)), "", dc::message(&ctx, &page).id("wl-goodbye").off(!s.goodbye_on).render(html! {
                                div.dp-line #wl-goodbye-text { (preview_text(goodbye_tpl, &user.name, &server, &count, false)) }
                                span.wl-card #wl-gcard hidden[!s.goodbye_card] {
                                    img src=(card_src(&page, Kind::Goodbye, goodbye_pictures.first().copied())) alt="The goodbye card, drawn for you" width="960" height="320" loading="lazy";
                                }
                            })))
                            @if page.can_edit() {
                                form.wl-send method="post" action=(page.path("welcome/test-goodbye")) {
                                    button.btn.small.outline type="submit" { (ui::icon(Icon::Mail)) "Send a test goodbye" }
                                    span.muted { "With your name, in " (test_channel(&ctx, guild, s.goodbye_channel)) ", with the saved settings" }
                                }
                            }
                            (pictures(&page, Kind::Goodbye, "Pictures behind the goodbye card", &goodbye_pictures))
                        }
                    }
                }
                section.wl-panel {
                    h2 { "Top inviters, 30 days" }
                    @if top.is_empty() {
                        p.muted.small { "Nobody has brought anyone in this month. Counts start once invites are read." }
                    }
                    @for (inviter, recent, _) in &top {
                        div.wl-inv {
                            img.avatar src=(ctx.member_avatar(guild, *inviter)) alt="" width="28" height="28" loading="lazy";
                            div.grow {
                                span { (ctx.display_name(guild, *inviter)) }
                                span.meter { span style={ "width:" (*recent * 100 / max_recent) "%" } {} }
                            }
                            span.wl-n { (text::num(*recent)) }
                        }
                    }
                }
                section.wl-panel {
                    h2 { "Back again" }
                    @if back.is_empty() {
                        p.muted.small { "Nobody has rejoined with their roles this week. Members who come back get their saved roles here." }
                    } @else {
                        div.wl-back {
                            (ui::avatars(&back.iter().take(3).map(|r| ctx.member_avatar(guild, r.user)).collect::<Vec<_>>(), 3))
                            p { (counted(back.len() as i64, "member has", "members have")) " rejoined this week and got their roles back." }
                        }
                    }
                }
                section.wl-panel {
                    h2 { "Given on join" }
                    @if given.is_empty() {
                        p.muted.small { "Nothing is given on join. Pick auto-roles on the Roles tab." }
                    }
                    (role_pills(&given))
                }
            }
        }
        script { (PreEscaped(KEEP_JS)) (PreEscaped(SCRIPT)) }
        (ui::sent_toast(q.sent.is_some(), if q.sent.as_deref() == Some("goodbye") { "Test goodbye sent" } else { "Test welcome sent" }))
    };
    Ok(ui::layout(&ctx, &page, "Welcome", None, body).into_response())
}

async fn save_messages(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let old = ctx.settings::<Settings>(guild, KEY)?;
    let template = |name: &str, default: &str| {
        let t = form.text(name);
        if t.is_empty() { default.to_owned() } else { text::clip(&t, 1000) }
    };
    let s = Settings {
        welcome_on: form.bool("welcome_on"),
        welcome_channel: form.channel(&ctx, guild, "welcome_channel"),
        welcome_text: template("welcome_text", DEFAULT_WELCOME),
        welcome_card: form.bool("welcome_card"),
        welcome_pictures_in_order: form.string("welcome_pictures_in_order") == "order",
        goodbye_on: form.bool("goodbye_on"),
        goodbye_channel: form.channel(&ctx, guild, "goodbye_channel"),
        goodbye_text: template("goodbye_text", DEFAULT_GOODBYE),
        goodbye_card: form.bool("goodbye_card"),
        goodbye_pictures_in_order: form.string("goodbye_pictures_in_order") == "order",
        ..(*old).clone()
    };
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed the welcome and goodbye messages");
    Ok(saved(&page.path("welcome")))
}

/// The signed-in person's Discord username, when the cache has it.
fn username(ctx: &Ctx, user: u64) -> String {
    Id::new_checked(user).and_then(|id| ctx.cache.user(id)).map(|u| u.name.clone()).unwrap_or_default()
}

#[derive(serde::Deserialize)]
struct CardQuery {
    /// "goodbye" for the goodbye card; the welcome card otherwise.
    kind: Option<String>,
    /// The picture to draw it over.
    picture: Option<i64>,
}

/// The welcome or goodbye card for whoever is signed in, as the page's preview, over the
/// picture asked for.
async fn card_png(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<CardQuery>) -> WebResult {
    ctx.page(&user, guild, KEY, Level::View)?;
    let kind = q.kind.as_deref().and_then(Kind::parse).unwrap_or(Kind::Welcome);
    let number = (kind == Kind::Welcome).then(|| ctx.member_count(guild).map(|n| n as i64)).flatten();
    let picture = match q.picture {
        Some(id) => banners::get(&ctx, guild, id)?.map(|(_, data)| data),
        None => None,
    };
    let names = [user.name.as_str(), &username(&ctx, user.id)];
    let png = card::render(&ctx, kind, &names, &ctx.guild_name(guild), number, &user.avatar_url(), picture.as_deref()).await?;
    Ok(([(CONTENT_TYPE, "image/png"), (CACHE_CONTROL, "no-store")], png).into_response())
}

async fn send_test(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    if s.welcome_channel.is_none() {
        return Err(WebError::Bad("Pick a welcome channel and save first.".into()));
    }
    crate::post_welcome(&ctx, &s, guild, user.id, &user.name, &username(&ctx, user.id), &user.avatar_url())
        .await
        .map_err(|e| WebError::Bad(e.to_string()))?;
    ctx.audit(&page.user, Some(guild), KEY, "Sent a test welcome");
    Ok(Redirect::to(&format!("{}?sent=welcome", page.path("welcome"))).into_response())
}

/// Post the saved goodbye with the viewer's name, as if they had left.
async fn send_goodbye_test(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let channel = s.goodbye_channel.ok_or_else(|| WebError::Bad("Pick a goodbye channel and save first.".into()))?;
    crate::post_goodbye(&ctx, &s, guild, channel, &user.name, &username(&ctx, user.id), &user.avatar_url()).await.map_err(|e| {
        bot_core::tracing::warn!(guild, error = %e, "test goodbye failed");
        WebError::Bad(bot_core::discord::why_refused_any(&e))
    })?;
    ctx.audit(&page.user, Some(guild), KEY, "Sent a test goodbye");
    Ok(Redirect::to(&format!("{}?sent=goodbye", page.path("welcome"))).into_response())
}

/// The card preview's address: `kind`'s card over `picture`.
fn card_src(page: &Page, kind: Kind, picture: Option<i64>) -> String {
    let mut src = format!("{}?kind={}", page.path("welcome/card.png"), kind.key());
    if let Some(id) = picture {
        src.push_str(&format!("&picture={id}"));
    }
    src
}

/// "At random" or "In turn", for each card's picture.
/// One of the two messages in the Messages form, with its field names (`{key}_on`,
/// `{key}_channel`, `{key}_text`, `{key}_card`, `{key}_pictures_in_order`).
struct MessageSection<'a> {
    key: &'a str,
    title: &'a str,
    when: &'a str,
    on: bool,
    channel: Option<u64>,
    text: &'a str,
    placeholder: &'a str,
    text_help: &'a str,
    card: bool,
    card_label: &'a str,
    card_help: &'a str,
    in_order: bool,
}

/// A message's section: its switch and name at the head, then where and what it posts side by
/// side, then its card. Labels sit over their controls, so nothing is a row's width away.
fn message_section(ctx: &Ctx, guild: u64, m: MessageSection) -> Markup {
    let id = |field: &str| format!("{}_{field}", m.key);
    let mut channels = vec![(String::new(), "None".to_owned())];
    channels.extend(ctx.text_channels(guild).into_iter().map(|(c, n)| (c.to_string(), format!("#{n}"))));
    let picked = m.channel.map(|c| c.to_string()).unwrap_or_default();
    html! {
        section.wl-sec {
            div.wl-sec-head {
                label.switch {
                    input type="checkbox" id=(id("on")) name=(id("on")) checked[m.on] aria-label={ "Post the " (m.title.to_lowercase()) " message" };
                    span.track { span.thumb {} }
                }
                label.wl-sec-title for=(id("on")) {
                    h2 { (m.title) }
                    span { (m.when) }
                }
            }
            div.wl-sec-body {
                div.wl-what {
                    div.wl-field {
                        label for=(id("channel")) { "Post in" }
                        (ui::select(&id("channel"), &channels, &picked))
                    }
                    div.wl-field.wl-msg {
                        label for=(id("text")) { "Message" }
                        textarea.input id=(id("text")) name=(id("text")) rows="4" placeholder=(m.placeholder) { (m.text) }
                        p.help { (m.text_help) }
                    }
                }
                div.wl-cardopts {
                    div.wl-cardswitch {
                        label.switch {
                            input type="checkbox" id=(id("card")) name=(id("card")) checked[m.card];
                            span.track { span.thumb {} }
                        }
                        div {
                            label for=(id("card")) { (m.card_label) }
                            p.help { (m.card_help) }
                        }
                    }
                    div.wl-field {
                        label for=(id("pictures_in_order")) { "Which picture" }
                        (ui::select(&id("pictures_in_order"), &order_choices(), if m.in_order { "order" } else { "random" }))
                    }
                }
            }
        }
    }
}

fn order_choices() -> Vec<(String, String)> {
    vec![("random".into(), "At random, never the same twice running".into()), ("order".into(), "In turn, one after another".into())]
}

/// One card's pictures, a strip of small tiles with a way to add and remove them. Outside the
/// settings form: each change is saved at once.
fn pictures(page: &Page, kind: Kind, title: &str, ids: &[i64]) -> Markup {
    html! {
        div.wl-pics-group {
            div.wl-pics-head {
                strong { (title) }
                span.muted.small { (ids.len()) " of " (banners::MAX) }
            }
            p.muted.small { "Darkened on the left so the name reads. Wide pictures fit best: the card is 3 to 1." }
            div.wl-pics {
                @for id in ids {
                    figure.wl-pic {
                        a href=(card_src(page, kind, Some(*id))) target="_blank" rel="noopener" data-tip="See the card over this picture" {
                            img src=(page.path(&format!("welcome/picture/{id}"))) alt="" loading="lazy";
                        }
                        @if page.can_edit() {
                            form.inline method="post" action=(page.path(&format!("welcome/picture/{id}/remove"))) {
                                button.icon-btn.small type="submit" aria-label="Remove this picture" data-confirm="Click again to remove" { (ui::icon(Icon::Trash)) }
                            }
                        }
                    }
                }
                @if page.can_edit() && ids.len() < banners::MAX {
                    div.wl-pic.add {
                        button.btn.small.outline type="button" data-wl-pick { (ui::icon(Icon::Plus)) "Add a picture" }
                        input type="file" accept="image/png,image/jpeg,image/webp,image/gif" hidden data-action=(page.path(&format!("welcome/pictures/{}", kind.key())));
                    }
                }
            }
        }
    }
}

/// Larger than the browser ever sends after shrinking; a guard, not a target.
const MAX_PICTURE_BYTES: usize = 6 * 1024 * 1024;

/// The body is the picture itself, shrunk by the page's script. It's read only after the access
/// check, so someone without access can't make the bot buffer uploads.
async fn add_picture(State(ctx): State<Ctx>, user: User, Path((guild, kind)): Path<(u64, String)>, body: bot_core::axum::body::Body) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let kind = Kind::parse(&kind).ok_or(WebError::NotFound)?;
    let Ok(body) = bot_core::axum::body::to_bytes(body, MAX_PICTURE_BYTES).await else {
        return Err(WebError::Bad("That picture is too large. Pick one under 6 MB.".into()));
    };
    let mime = bot_core::web::banner::sniff(&body).ok_or_else(|| WebError::Bad("That file isn't a PNG, JPEG, WebP or GIF picture.".into()))?;
    banners::add(&ctx, guild, kind, mime, &body).map_err(|e| WebError::Bad(e.to_string()))?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Added a {} card picture", kind.key()));
    Ok(bot_core::axum::http::StatusCode::NO_CONTENT.into_response())
}

/// A card picture, for the dashboard. Its id never names another picture, so it's kept for good.
async fn picture(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    ctx.page(&user, guild, KEY, Level::View)?;
    let (mime, data) = banners::get(&ctx, guild, id)?.ok_or(WebError::NotFound)?;
    Ok(([(CONTENT_TYPE, mime), (CACHE_CONTROL, "private, max-age=31536000, immutable".to_owned())], data).into_response())
}

async fn remove_picture(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    if banners::remove(&ctx, guild, id)? {
        ctx.audit(&page.user, Some(guild), KEY, "Removed a card picture");
    }
    Ok(saved(&format!("{}#wl-pictures", page.path("welcome"))))
}

// ---- roles ----

async fn roles_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let now = now_ms();
    let days = s.rejoin_days.max(1);
    let waiting = roles::waiting_count(&ctx, guild, days)?;
    let waiting_list = roles::waiting_members(&ctx, guild, days, 5)?;
    let restored = roles::recently_restored(&ctx, guild, 25)?;
    let this_week = restored.iter().filter(|r| r.at >= now - 7 * DAY_MS).count() as i64;
    let exists = |r: &u64| Id::new_checked(*r).and_then(|id| ctx.cache.role(id)).is_some();
    let stuck: Vec<u64> = s.autoroles.iter().copied().filter(|r| exists(r) && !roles::assignable(&ctx, guild, *r)).collect();
    let deleted = s.autoroles.iter().filter(|r| !exists(r)).count() as i64;
    let given = role_list(&ctx, guild, &s.autoroles);
    // The newest member stands in for every newcomer; without one, the viewer does.
    let newest = invites::recent_joins(&ctx, guild, 1)?.into_iter().next().map(|j| j.user).filter(|u| ctx.known_name(guild, *u).is_some());
    let (new_face, new_name, new_user) = match newest {
        Some(u) => (ctx.member_avatar(guild, u), ctx.display_name(guild, u), username(&ctx, u)),
        None => (user.avatar_url(), user.name.clone(), username(&ctx, user.id)),
    };

    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, "Roles"))
        @if !stuck.is_empty() || deleted > 0 {
            div.ov-rows.wl-needs {
                @if !stuck.is_empty() {
                    div.need.alert {
                        span.need-icon.alert { (ui::icon(Icon::Shield)) }
                        div.grow {
                            strong { "The bot can't give " (role_names(&ctx, guild, &stuck)) }
                            p.muted.small { "These sit above the bot's own role, or the server manages them. Move the bot's role higher in Discord." }
                        }
                    }
                }
                @if deleted > 0 {
                    div.need {
                        span.need-icon { (ui::icon(Icon::Trash)) }
                        div.grow {
                            strong { (counted(deleted, "auto-role was", "auto-roles were")) " deleted in Discord" }
                            p.muted.small { "Save this tab to take " (if deleted == 1 { "it" } else { "them" }) " off the list." }
                        }
                    }
                }
            }
        }
        // The settings and who got their roles back on the left; on the right the profile they
        // decide, one at a time, then the figures and who is waiting.
        div.wl-split {
            div.wl-col {
                div.wl-panel.wl-form {
                    (ui::settings_form(&page, &page.path("welcome/roles"), html! {
                        div.wl-group {
                            h2 { "Auto-roles" }
                            div.wl-rows {
                                (ui::field_roles(&ctx, guild, "autoroles", "Roles for new members", Some("The bot can only give roles below its own role."), &s.autoroles))
                                (ui::field_toggle("autorole_bots", "Include bots", Some("Off: bots that join get no auto-roles."), s.autorole_bots))
                            }
                        }
                        div.wl-group {
                            h2 { "Roles back on rejoin" }
                            div.wl-rows {
                                (ui::field_toggle("rejoin_on", "Give roles back", Some("A member who leaves and returns gets the roles they left with. Roles with moderation or admin permissions are never given back."), s.rejoin_on))
                                (ui::field_number("rejoin_days", "Keep for", Some("After this long, a leaver's saved roles are deleted."), days, 1, 3650, Some("days")))
                                (ui::field_roles(&ctx, guild, "rejoin_exclude", "Never give back", Some("Useful for staff or punishment roles."), &s.rejoin_exclude))
                            }
                        }
                    }))
                }
                section.wl-panel {
                    div.block-head { h2 { "Recently restored" } p.muted { "Members who came back and got their roles." } }
                    @if restored.is_empty() {
                        (ui::empty(Icon::Users, "Nobody restored yet", "Members who leave and return show up here with the roles they got back."))
                    } @else {
                        div.wl-list-in {
                            @for r in &restored {
                                div.wl-row {
                                    img.avatar src=(ctx.member_avatar(guild, r.user)) alt="" loading="lazy";
                                    div.grow {
                                        strong.clip { (ctx.display_name(guild, r.user)) }
                                        (role_pills(&role_list(&ctx, guild, &r.roles)))
                                    }
                                    span.meta { (text::ago(r.at)) }
                                }
                            }
                        }
                    }
                }
            }
            div.wl-col.wl-side {
                section.wl-panel.wl-preview {
                    div.wl-pv-head {
                        h2 { "What members see" }
                        div.seg role="group" aria-label="Whose profile" {
                            label { input type="radio" name="wl-who" value="new" checked; "New member" }
                            label { input type="radio" name="wl-who" value="back"; "Came back" }
                        }
                    }
                    div.wl-pv.wl-pv-new {
                        p.wl-cap { b { "A new member" } " gets these when they join" }
                        (popout(&new_face, &new_name, &new_user, &given, ""))
                    }
                    div.wl-pv.wl-pv-back {
                        @match restored.first() {
                            Some(r) => {
                                p.wl-cap { b { (ctx.display_name(guild, r.user)) } " came back " (text::ago(r.at)) }
                                (popout(&ctx.member_avatar(guild, r.user), &ctx.display_name(guild, r.user), &username(&ctx, r.user), &role_list(&ctx, guild, &r.roles), "back"))
                            },
                            None => {
                                p.wl-cap { b { "A member coming back" } " gets the roles they left with" }
                                (ui::empty(Icon::Users, "Nobody has come back yet", "When someone leaves and returns, their profile shows here with the roles they got back."))
                            },
                        }
                    }
                }
                section.wl-panel.wl-waiting {
                    div.wl-figs.wl-figs-wide {
                        div { b { (text::num(given.len() as i64)) } span { "given on join" } }
                        div { b { (text::num(waiting)) } span { "waiting to come back" } }
                        div { b { (text::num(this_week)) } span { "restored this week" } }
                    }
                    div.wl-sub {
                        h2 { "Waiting to come back" }
                        p.muted.small { (counted(waiting, "member", "members")) " left with roles saved, in the last " (days) " days." }
                        @for (u, left_at) in &waiting_list {
                            div.mini {
                                img.avatar src=(ctx.member_avatar(guild, *u)) alt="" loading="lazy";
                                div.grow {
                                    span { (ctx.known_name(guild, *u).unwrap_or_else(|| "A former member".into())) }
                                    span.muted { "Left " (text::ago(*left_at)) }
                                }
                            }
                        }
                    }
                    div.wl-sub {
                        h2 { "Never given back" }
                        @if s.rejoin_exclude.is_empty() {
                            p.muted.small { "Every saved role comes back except roles with moderation or admin permissions. Exclude other staff or punishment roles in the form." }
                        } @else {
                            (role_pills(&role_list(&ctx, guild, &s.rejoin_exclude)))
                        }
                    }
                }
            }
        }
        script { (PreEscaped(KEEP_JS)) }
    };
    Ok(ui::layout(&ctx, &page, "Welcome", None, body).into_response())
}

async fn save_roles(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let old = ctx.settings::<Settings>(guild, KEY)?;
    let autoroles = form.roles(&ctx, guild, "autoroles");
    if !page.user.admin {
        // Only roles being added are checked, so saving other settings doesn't trip over a role an admin chose.
        if let Some(r) = autoroles.iter().find(|r| !old.autoroles.contains(r) && !ctx.safe_role(guild, **r)) {
            return Err(WebError::Bad(format!(
                "{} can't be an auto-role. Only a server admin can hand out roles with moderation permissions, managed roles, or roles at or above the bot's own.",
                role_names(&ctx, guild, &[*r])
            )));
        }
    }
    let s = Settings {
        autoroles,
        autorole_bots: form.bool("autorole_bots"),
        rejoin_on: form.bool("rejoin_on"),
        rejoin_days: form.int_in("rejoin_days", 1, 3650, 90),
        rejoin_exclude: form.roles(&ctx, guild, "rejoin_exclude"),
        ..(*old).clone()
    };
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed auto-roles and rejoin roles");
    Ok(saved(&page.path("welcome/roles")))
}

// ---- invites ----

/// Joins on each of the last 30 days in the viewer's timezone, oldest first, labelled "Oct 2".
fn joins_by_day(ctx: &Ctx, guild: u64, page: &Page) -> Vec<(String, i64)> {
    use bot_core::jiff;
    let today = page.user.local(now_ms()).date();
    let days: Vec<jiff::civil::Date> = (0..30i64).rev().filter_map(|b| today.checked_sub(jiff::Span::new().days(b)).ok()).collect();
    let since = day_start(page) - 31 * DAY_MS;
    let ats: Vec<i64> = ctx
        .db
        .with(|c| {
            let mut s = c.prepare_cached("SELECT joined_at FROM welcome_joins WHERE guild_id = ?1 AND joined_at >= ?2")?;
            s.query_map(bot_core::rusqlite::params![guild as i64, since], |r| r.get(0))?.collect()
        })
        .unwrap_or_default();
    let mut per: std::collections::HashMap<jiff::civil::Date, i64> = std::collections::HashMap::new();
    for at in ats {
        *per.entry(page.user.local(at).date()).or_default() += 1;
    }
    days.iter().map(|d| (format!("{} {}", &MONTHS[(d.month() - 1).clamp(0, 11) as usize][..3], d.day()), per.get(d).copied().unwrap_or(0))).collect()
}

async fn invites_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let now = now_ms();
    let month = now - 30 * DAY_MS;
    let (joins, known) = invites::join_counts(&ctx, guild, month)?;
    let top = invites::top_inviters(&ctx, guild, month, 15)?;
    let recent = invites::recent_joins(&ctx, guild, 25)?;
    let state = invites::state(&ctx, guild)?;
    let live = invites::load(&ctx, guild)?;
    let names = invites::names(&ctx, guild)?;
    let series = joins_by_day(&ctx, guild, &page);
    let most_uses = live.iter().map(|i| i.uses).max().unwrap_or(1).max(1);
    let top_recent = top.iter().map(|t| t.1).max().unwrap_or(1).max(1);

    let status = if !s.track_invites {
        html! { (ui::status(ui::Dot::Off, "Not tracking")) span.muted { "Invites aren't being read. Joins are still listed, without who invited them." } }
    } else {
        match &state {
            None => html! { (ui::status(ui::Dot::Waiting, "Not read yet")) span.muted { "The bot reads the invites soon after it starts, then every hour." } },
            Some(st) if st.ok => html! { (ui::status(ui::Dot::Live, "Reading invites")) span.muted { (st.codes) " invites, last read " (text::ago(st.checked_at)) "." } },
            Some(st) => html! {
                (ui::badge("Can't read invites", Tone::Bad))
                span { (st.error.as_deref().unwrap_or("Discord didn't say why.")) }
                span.muted { "Tried " (text::ago(st.checked_at)) ". It tries again every hour." }
            },
        }
    };

    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, "Invites"))
        // What came in on the left: the joins per day with their figures, then each join. On the
        // right what is set: tracking, the links' names, and who brings members in.
        div.wl-split {
            div.wl-col {
                section.wl-panel {
                    div.wl-head {
                        div.block-head { h2 { "Joins per day" } p.muted { "The last 30 days, in your timezone." } }
                        div.wl-figs {
                            div { b { (text::num(joins)) } span { "joins in 30 days" } }
                            div { b { (if joins > 0 { format!("{}%", known * 100 / joins) } else { "None yet".into() }) } span { "matched to an invite" } }
                            div { b { (text::num(state.as_ref().map(|s| s.codes).unwrap_or(0))) } span { "invites watched" } }
                        }
                    }
                    @if joins == 0 {
                        (ui::empty(Icon::Door, "Nobody has joined in 30 days", "Each join lands here on its day, whether or not an invite is matched."))
                    } @else {
                        div.wl-chart { (ui::area_chart(&series, "joins")) }
                    }
                }
                section.wl-panel {
                    h2 { "Recent joins" }
                    @if recent.is_empty() {
                        (ui::empty(Icon::Door, "No joins recorded", "New members appear here with the invite they came through."))
                    } @else {
                        div.wl-list-in.wl-joins {
                            @for j in &recent {
                                @let here = ctx.known_name(guild, j.user);
                                div.wl-row.gone[here.is_none()] {
                                    img.avatar src=(ctx.member_avatar(guild, j.user)) alt="" loading="lazy";
                                    div.grow {
                                        strong.clip { (here.unwrap_or_else(|| "Left the server".into())) }
                                        span.wl-via {
                                            @match j.inviter {
                                                Some(i) => { img.avatar src=(ctx.member_avatar(guild, i)) alt="" loading="lazy"; "invited by " (ctx.display_name(guild, i)) },
                                                None => { "invite unknown" },
                                            }
                                            @if let Some(c) = &j.code {
                                                @match names.get(c) { Some(n) => { span.wl-tag { (n) } }, None => { code.wl-c { (c) } } }
                                            }
                                        }
                                    }
                                    span.meta { (text::ago(j.at)) }
                                }
                            }
                        }
                    }
                }
            }
            div.wl-col.wl-side {
                section.wl-panel.wl-track {
                    h2 { "Tracking" }
                    div.wl-status { (status) }
                    (ui::settings_form(&page, &page.path("welcome/invites"), ui::switch_row("track_invites", "Track invites", Some("Works out which invite each new member used. The bot needs the Manage Server permission to read invites."), s.track_invites)))
                }
                section.wl-panel {
                    div.block-head { h2 { "Invite links" } p.muted { "Name a link to see where members come from, like \"Twitch link\". The name shows on Joined today and in recent joins." } }
                    @if live.is_empty() {
                        (ui::empty(Icon::Door, "No invites read yet", "The server's invites show up here once the bot has read them."))
                    } @else {
                        form.wl-links method="post" action=(page.path("welcome/invites/names")) {
                            @for inv in &live {
                                @let name = names.get(&inv.code).map(String::as_str).unwrap_or_default();
                                div.wl-link {
                                    div.grow {
                                        span.wl-code { (inv.code) }
                                        span.wl-via {
                                            @match inv.inviter {
                                                Some(u) => { img.avatar src=(ctx.member_avatar(guild, u)) alt="" loading="lazy"; "by " (ctx.display_name(guild, u)) },
                                                None => { "made by the server" },
                                            }
                                            @if !name.is_empty() && !page.can_edit() { span.wl-tag { (name) } }
                                        }
                                    }
                                    div.wl-uses {
                                        span { b { (text::num(inv.uses)) } (if inv.uses == 1 { " use" } else { " uses" }) @if inv.max_uses > 0 { " of " (inv.max_uses) } }
                                        @let pct = if inv.max_uses > 0 { inv.uses * 100 / inv.max_uses } else { inv.uses * 100 / most_uses };
                                        span.meter { span style={ "width:" (pct.clamp(0, 100)) "%" } {} }
                                    }
                                    @if page.can_edit() {
                                        input type="hidden" name="code" value=(inv.code);
                                        input.input type="text" name={ "name-" (inv.code) } value=(name) maxlength="40" placeholder="Name this link" aria-label={ "Name for invite " (inv.code) };
                                    }
                                }
                            }
                            @if page.can_edit() {
                                div.wl-links-foot { button.btn.small type="submit" { "Save names" } }
                            }
                        }
                    }
                }
                section.wl-panel {
                    h2 { "Top inviters" }
                    @if top.is_empty() {
                        p.muted.small { "Joins through a member's invite are counted here once tracking has taken its first reading." }
                    }
                    @for (inviter, recent, all) in &top {
                        div.wl-inv {
                            img.avatar src=(ctx.member_avatar(guild, *inviter)) alt="" width="28" height="28" loading="lazy";
                            div.grow {
                                span { (ctx.display_name(guild, *inviter)) }
                                span.meter { span style={ "width:" (*recent * 100 / top_recent) "%" } {} }
                            }
                            span.wl-n data-tip={ "All time: " (text::num(*all)) } { (text::num(*recent)) }
                        }
                    }
                    @if !top.is_empty() { p.muted.small { "Joins in the last 30 days. Hover a figure for all time." } }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Welcome", None, body).into_response())
}

async fn save_invite_names(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    // Only invites the bot knows of, live or already named, can be named.
    let known: Vec<String> = invites::load(&ctx, guild)?.into_iter().map(|i| i.code).chain(invites::names(&ctx, guild)?.into_keys()).collect();
    // Each name has its own field, since an emptied one (clearing the name) isn't sent as a value.
    let pairs: Vec<(String, String)> = form
        .all("code")
        .into_iter()
        .filter(|code| known.iter().any(|k| k == code))
        .map(|code| (code.to_owned(), text::clip(&form.string(&format!("name-{code}")).split_whitespace().collect::<Vec<_>>().join(" "), 40)))
        .collect();
    invites::set_names(&ctx, guild, &pairs)?;
    ctx.audit(&page.user, Some(guild), KEY, "Named invite links");
    Ok(saved(&page.path("welcome/invites")))
}

async fn save_invites(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let old = ctx.settings::<Settings>(guild, KEY)?;
    let s = Settings { track_invites: form.bool("track_invites"), ..(*old).clone() };
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed invite tracking");
    Ok(saved(&page.path("welcome/invites")))
}
