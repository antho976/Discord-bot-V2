use bot_core::axum::Router;
use bot_core::axum::extract::{Path, State};
use bot_core::axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use bot_core::axum::response::{IntoResponse, Redirect};
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::twilight_model::id::Id;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};

use crate::{DEFAULT_GOODBYE, DEFAULT_WELCOME, KEY, Settings, card, invites, roles};

const DAY_MS: i64 = 86_400_000;

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/welcome", get(messages_page).post(save_messages))
        .route("/g/{guild}/welcome/card.png", get(card_png))
        .route("/g/{guild}/welcome/test", post(send_test))
        .route("/g/{guild}/welcome/roles", get(roles_page).post(save_roles))
        .route("/g/{guild}/welcome/invites", get(invites_page).post(save_invites))
        .route("/g/{guild}/welcome/invites/names", post(save_invite_names))
}

fn tabs(page: &Page, active: &str) -> Markup {
    ui::tabs(&[("Messages", page.path("welcome")), ("Roles", page.path("welcome/roles")), ("Invites", page.path("welcome/invites"))], active)
}

fn channel_name(ctx: &Ctx, guild: u64, id: Option<u64>) -> String {
    id.and_then(|id| ctx.text_channels(guild).into_iter().find(|(c, _)| *c == id)).map(|(_, n)| format!("#{n}")).unwrap_or_else(|| "Off".into())
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

/// The bot's name and avatar, as a member sees them in the channel.
fn bot_identity(ctx: &Ctx) -> (String, String) {
    let Some(bot) = ctx.bot_id() else { return ("Bot".into(), bot_core::discord::avatar_url(0, None)) };
    match ctx.cache.user(Id::new(bot)) {
        Some(u) => (
            u.global_name.clone().unwrap_or_else(|| u.name.clone()),
            bot_core::discord::avatar_url(bot, u.avatar.map(|a| a.to_string()).as_deref()),
        ),
        None => ("Bot".into(), bot_core::discord::avatar_url(bot, None)),
    }
}

/// `9:05 PM` in the viewer's timezone.
fn clock(page: &Page, ms: i64) -> String {
    let z = page.user.local(ms);
    let (h, m) = (z.hour() as i64, z.minute() as i64);
    format!("{}:{m:02} {}", if h % 12 == 0 { 12 } else { h % 12 }, if h < 12 { "AM" } else { "PM" })
}

const CSS: &str = "
.wl-main { gap: 30px; }
.wl-block { display: flex; flex-direction: column; gap: 14px; min-width: 0; }
.wl-head { display: flex; align-items: baseline; justify-content: space-between; gap: 6px 12px; flex-wrap: wrap; }
.wl-faces { display: grid; grid-template-columns: repeat(auto-fill, minmax(104px, 1fr)); gap: 18px 10px; }
.wl-face { display: flex; flex-direction: column; align-items: center; gap: 2px; text-align: center; min-width: 0; }
.wl-face .avatar { width: 56px; height: 56px; margin-bottom: 6px; }
.wl-face strong { font-weight: 500; font-size: 13px; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.wl-face .muted { font-size: 12px; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.wl-more { display: grid; place-items: center; width: 56px; height: 56px; margin-bottom: 6px; border-radius: 50%; background: var(--control); color: var(--ink-2); font: 500 15px var(--mono); }
.wl-preview { display: flex; flex-direction: column; gap: 18px; padding: 18px 20px; border-radius: var(--r-card); background: var(--veil); border: 1px solid var(--edge-1); }
.wl-msg { display: flex; gap: 12px; align-items: flex-start; font-size: 14px; line-height: 1.45; min-width: 0; overflow-wrap: anywhere; }
.wl-msg.off { opacity: .45; }
.wl-msg > .avatar { width: 40px; height: 40px; margin-top: 2px; }
.wl-msg-body { display: flex; flex-direction: column; gap: 4px; min-width: 0; flex: 1; }
.wl-msg-head { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.wl-msg-head strong { font-weight: 600; }
.wl-app { display: inline-flex; align-items: center; height: 16px; padding: 0 5px; border-radius: 4px; background: var(--data); color: var(--ground); font: 600 10px var(--sans); letter-spacing: .02em; }
.wl-msg-text { color: var(--ink); }
.wl-mention { background: var(--data-wash); color: var(--data); border-radius: 3px; padding: 0 3px; }
.wl-bye { font-size: 13px; color: var(--ink-2); }
.wl-bye .avatar { width: 28px; height: 28px; }
.wl-card { display: block; width: 100%; max-width: 440px; height: auto; border-radius: 10px; margin-top: 4px; }
.wl-label { font-size: 12px; color: var(--ink-3); }
.wl-inv { display: grid; grid-template-columns: 28px minmax(0, 1fr) auto; gap: 10px; align-items: center; font-size: 13px; }
.wl-inv .grow { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.wl-inv .grow > span:first-child { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.wl-inv .mono { color: var(--ink-2); }
.wl-back { display: flex; align-items: center; gap: 12px; }
.wl-back p { font-size: 13px; color: var(--ink-2); }
.wl-chips { display: flex; flex-wrap: wrap; gap: 6px; }
.wl-chip { display: inline-flex; align-items: center; gap: 7px; height: 26px; padding: 0 11px; border-radius: 13px; background: var(--control); color: var(--ink-2); font-size: 12px; }
.wl-dot { width: 8px; height: 8px; border-radius: 50%; background: var(--ink-3); flex: none; }
.wl-send { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; }
.wl-settings { display: flex; flex-direction: column; gap: 22px; }
.wl-settings .settings { gap: 22px; }
.wl-big { display: flex; flex-direction: column; gap: 2px; }
.wl-big .big-num { font-size: 26px; }
.wl-invite .input { width: 200px; flex: none; }
@media (max-width: 560px) { .wl-invite { flex-wrap: wrap; } .wl-invite .input { width: 100%; } }
";

const SCRIPT: &str = r#"
(() => {
  const box = document.getElementById("wl-preview");
  if (!box) return;
  const v = { name: box.dataset.name, server: box.dataset.server, count: box.dataset.count };
  const $ = (id) => document.getElementById(id);
  const draw = (el, tpl, mention) => {
    el.textContent = "";
    tpl.split(/(\{user\})/).forEach((part) => {
      if (part === "{user}") {
        if (mention) { const s = document.createElement("span"); s.className = "wl-mention"; s.textContent = "@" + v.name; el.append(s); }
        else el.append(v.name);
      } else el.append(part.replace(/\{(name|server|count)\}/g, (_, k) => v[k]));
    });
  };
  const sync = () => {
    draw($("wl-welcome-text"), $("welcome_text").value.trim() || box.dataset.defaultWelcome, true);
    draw($("wl-goodbye-text"), $("goodbye_text").value.trim() || box.dataset.defaultGoodbye, false);
    $("wl-card").hidden = !$("welcome_card").checked;
    $("wl-goodbye").classList.toggle("off", !$("goodbye_on").checked);
    $("wl-welcome").classList.toggle("off", !$("welcome_on").checked);
  };
  ["welcome_text", "goodbye_text", "welcome_card", "goodbye_on", "welcome_on"].forEach((id) => {
    $(id).addEventListener("input", sync);
    $(id).addEventListener("change", sync);
  });
  sync();
  const url = new URL(location.href);
  if (url.searchParams.has("sent")) {
    window.botToast && window.botToast("Test sent");
    url.searchParams.delete("sent");
    history.replaceState(null, "", url.pathname + url.search + url.hash);
  }
})();
"#;

async fn messages_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let now = now_ms();
    let count = ctx.member_count(guild).map(|n| text::num(n as i64)).unwrap_or_else(|| "1".into());
    let today = day_start(&page);
    let (joins, _) = invites::join_counts(&ctx, guild, today)?;
    let shown = invites::joins_since(&ctx, guild, today, 5)?;
    let left = left_since(&ctx, guild, today);
    let top: Vec<(u64, i64, i64)> = invites::top_inviters(&ctx, guild, now - 30 * DAY_MS, 6)?.into_iter().filter(|t| t.1 > 0).take(4).collect();
    let back: Vec<roles::Restored> = roles::recently_restored(&ctx, guild, 25)?.into_iter().filter(|r| r.at >= now - 7 * DAY_MS).collect();
    let known_roles = ctx.roles(guild);
    let given: Vec<(String, String)> = s
        .autoroles
        .iter()
        .filter_map(|id| known_roles.iter().find(|(r, _, _)| r == id))
        .map(|(_, name, color)| (name.clone(), if *color == 0 { "var(--ink-3)".to_owned() } else { format!("#{color:06x}") }))
        .collect();
    let (bot_name, bot_avatar) = bot_identity(&ctx);
    let max_recent = top.iter().map(|t| t.1).max().unwrap_or(1).max(1);
    let more = joins - shown.len() as i64;
    let names = invites::names(&ctx, guild)?;
    let faces: Vec<(u64, String, String)> = shown
        .iter()
        .map(|(j, came_back)| {
            let mut parts = vec![short_ago(j.at)];
            if *came_back {
                parts.push("came back".to_owned());
            } else if let Some(name) = j.code.as_ref().and_then(|c| names.get(c)) {
                parts.push(format!("via {name}"));
            } else if let Some(i) = j.inviter {
                parts.push(format!("via {}", ctx.display_name(guild, i)));
            }
            (j.user, ctx.display_name(guild, j.user), parts.join(" · "))
        })
        .collect();

    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, "Messages"))
        div.with-rail {
            div.stack.wl-main {
                div.wl-block {
                    div.wl-head {
                        h2 { "Joined today" }
                        span.muted.small { (counted(joins, "new member", "new members")) " · " (text::num(left)) " left" }
                    }
                    @if shown.is_empty() {
                        (ui::empty(Icon::Door, "Nobody has joined yet today", "New members show up here with how long ago they came in and who invited them."))
                    } @else {
                        div.wl-faces {
                            @for (user_id, name, detail) in &faces {
                                div.wl-face {
                                    img.avatar src=(ctx.member_avatar(guild, *user_id)) alt="" loading="lazy";
                                    strong { (name) }
                                    span.muted { (detail) }
                                }
                            }
                            @if more > 0 {
                                div.wl-face {
                                    span.wl-more { "+" (more) }
                                    strong { (more) " more" }
                                    span.muted { "earlier today" }
                                }
                            }
                        }
                    }
                }

                div.wl-block {
                    h2 { "What new members see" }
                    div.wl-preview id="wl-preview" data-name=(user.name) data-server=(ctx.guild_name(guild)) data-count=(count)
                        data-default-welcome=(DEFAULT_WELCOME) data-default-goodbye=(DEFAULT_GOODBYE) {
                        div.wl-msg id="wl-welcome" {
                            img.avatar src=(bot_avatar) alt="" loading="lazy";
                            div.wl-msg-body {
                                div.wl-msg-head {
                                    strong { (bot_name) }
                                    span.wl-app { "APP" }
                                    span.muted.small { "Today at " (clock(&page, now)) }
                                }
                                div.wl-msg-text id="wl-welcome-text" {}
                                img.wl-card id="wl-card" src=(page.path("welcome/card.png")) alt="The welcome card" width="960" height="320";
                            }
                        }
                        div.wl-label { "When someone leaves" }
                        div.wl-msg.wl-bye id="wl-goodbye" {
                            img.avatar src=(bot_avatar) alt="" loading="lazy";
                            div.wl-msg-body {
                                div.wl-msg-head { strong { (bot_name) } span.wl-app { "APP" } }
                                div id="wl-goodbye-text" {}
                            }
                        }
                    }
                    @if page.can_edit() && s.welcome_channel.is_some() {
                        div.wl-send {
                            form.inline method="post" action=(page.path("welcome/test")) {
                                button.btn.small type="submit" { "Send a test" }
                            }
                            span.muted.small { "Posts the welcome for you in " (channel_name(&ctx, guild, s.welcome_channel)) ", using the saved settings." }
                        }
                    } @else if page.can_edit() {
                        p.muted.small { "Pick a welcome channel and save to send a test." }
                    }
                }

                div.wl-settings {
                    (ui::settings_form(&page, &page.path("welcome"), html! {
                        (ui::card(html! {
                            (ui::field_toggle("welcome_on", "Welcome new members", None, s.welcome_on))
                            (ui::field_channel(&ctx, guild, "welcome_channel", "Post in", None, s.welcome_channel))
                            (ui::field_textarea("welcome_text", "Message", Some("{user} mentions them, {name} is their name, {server} the server and {count} their member number. Only they are mentioned."), &s.welcome_text, DEFAULT_WELCOME))
                            (ui::field_toggle("welcome_card", "Attach the welcome card", Some("An image with their avatar, name and member number."), s.welcome_card))
                        }))
                        (ui::card(html! {
                            (ui::field_toggle("goodbye_on", "Say goodbye", Some("Posts when someone leaves."), s.goodbye_on))
                            (ui::field_channel(&ctx, guild, "goodbye_channel", "Goodbye in", None, s.goodbye_channel))
                            (ui::field_textarea("goodbye_text", "Goodbye message", Some("{name}, {server} and {count} work here. Nobody is mentioned."), &s.goodbye_text, DEFAULT_GOODBYE))
                        }))
                    }))
                }
                script { (PreEscaped(SCRIPT)) }
            }

            aside.rail {
                div.rail-block {
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
                            span.mono { (text::num(*recent)) }
                        }
                    }
                }
                div.rail-block {
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
                div.rail-block {
                    h2 { "Given on join" }
                    @if given.is_empty() {
                        p.muted.small { "Nothing is given on join. Pick auto-roles on the Roles tab." }
                    }
                    div.wl-chips {
                        @for (name, color) in &given {
                            span.wl-chip { span.wl-dot style={ "background:" (color) } {} (name) }
                        }
                    }
                }
            }
        }
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
        goodbye_on: form.bool("goodbye_on"),
        goodbye_channel: form.channel(&ctx, guild, "goodbye_channel"),
        goodbye_text: template("goodbye_text", DEFAULT_GOODBYE),
        ..(*old).clone()
    };
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed the welcome and goodbye messages");
    Ok(saved(&page.path("welcome")))
}

/// The signed-in person's Discord username, when the cache has it.
fn username(ctx: &Ctx, user: u64) -> String {
    ctx.cache.user(Id::new(user)).map(|u| u.name.clone()).unwrap_or_default()
}

/// The welcome card for whoever is signed in, as the page's preview.
async fn card_png(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    ctx.page(&user, guild, KEY, Level::View)?;
    let number = ctx.member_count(guild).map(|n| n as i64);
    let png = card::render(&ctx, &[&user.name, &username(&ctx, user.id)], &ctx.guild_name(guild), number, &user.avatar_url()).await?;
    Ok(([(CONTENT_TYPE, "image/png"), (CACHE_CONTROL, "no-store")], png).into_response())
}

async fn send_test(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    crate::post_welcome(&ctx, &s, guild, user.id, &user.name, &username(&ctx, user.id), &user.avatar_url())
        .await
        .map_err(|e| WebError::Bad(e.to_string()))?;
    ctx.audit(&page.user, Some(guild), KEY, "Sent a test welcome");
    Ok(Redirect::to(&format!("{}?sent=1", page.path("welcome"))).into_response())
}

// ---- roles ----

async fn roles_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let days = s.rejoin_days.max(1);
    let waiting = roles::waiting_count(&ctx, guild, days)?;
    let restored = roles::recently_restored(&ctx, guild, 25)?;
    let exists = |r: &u64| ctx.cache.role(Id::new(*r)).is_some();
    let stuck: Vec<u64> = s.autoroles.iter().copied().filter(|r| exists(r) && !roles::assignable(&ctx, guild, *r)).collect();
    let deleted = s.autoroles.iter().filter(|r| !exists(r)).count() as i64;

    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, "Roles"))
        div.with-rail {
            div.stack.wl-main {
                @if !stuck.is_empty() {
                    p.small { (ui::badge("Can't give", Tone::Warn)) " " (role_names(&ctx, guild, &stuck)) ". These sit above the bot's own role, or the server manages them." }
                }
                @if deleted > 0 {
                    p.small {
                        (ui::badge("Deleted", Tone::Warn)) " "
                        (counted(deleted, "auto-role was", "auto-roles were")) " deleted in Discord. Save this tab to take "
                        (if deleted == 1 { "it" } else { "them" }) " off the list."
                    }
                }
                (ui::settings_form(&page, &page.path("welcome/roles"), html! {
                    (ui::section("Auto-roles", None, ui::card(html! {
                        (ui::field_roles(&ctx, guild, "autoroles", "Roles for new members", Some("The bot can only give roles below its own role."), &s.autoroles))
                        (ui::field_toggle("autorole_bots", "Include bots", Some("Off: bots that join get no auto-roles."), s.autorole_bots))
                    })))
                    (ui::section("Roles back on rejoin", None, ui::card(html! {
                        (ui::field_toggle("rejoin_on", "Give roles back", Some("A member who leaves and returns gets the roles they left with."), s.rejoin_on))
                        (ui::field_number("rejoin_days", "Keep for", Some("After this long, a leaver's saved roles are deleted."), days, 1, 3650, Some("days")))
                        (ui::field_roles(&ctx, guild, "rejoin_exclude", "Never give back", Some("Useful for staff or punishment roles."), &s.rejoin_exclude))
                    })))
                }))
                div.block-head {
                    h2 { "Recently restored" }
                    p.muted { "Members who came back and got their roles." }
                }
                @if restored.is_empty() {
                    (ui::empty(Icon::Users, "Nobody restored yet", "Members who leave and return show up here with the roles they got back."))
                } @else {
                    div.list {
                        @for r in &restored {
                            div.list-item {
                                img.avatar src=(ctx.member_avatar(guild, r.user)) alt="" width="32" height="32" loading="lazy";
                                div.grow {
                                    strong.clip { (ctx.display_name(guild, r.user)) }
                                    div.muted.small.clip { (role_names(&ctx, guild, &r.roles)) }
                                }
                                span.meta { (text::ago(r.at)) }
                            }
                        }
                    }
                }
            }
            aside.rail {
                div.rail-block {
                    h2 { "Waiting to come back" }
                    div.wl-big {
                        span.big-num { (text::num(waiting)) }
                        span.muted.small { (if waiting == 1 { "member" } else { "members" }) " left with roles saved, in the last " (days) " days" }
                    }
                }
            }
        }
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

    let status = if !s.track_invites {
        html! { (ui::badge("Off", Tone::Muted)) " " span.muted.small { "Invites aren't being read. Joins are still listed, without who invited them." } }
    } else {
        match &state {
            None => html! { (ui::badge("Not read yet", Tone::Muted)) " " span.muted.small { "The bot reads the invites soon after it starts, then every hour." } },
            Some(st) if st.ok => html! { (ui::badge("Reading invites", Tone::Ok)) " " span.muted.small { (st.codes) " invites, last read " (text::ago(st.checked_at)) "." } },
            Some(st) => html! {
                (ui::badge("Can't read invites", Tone::Bad)) " "
                span.small { (st.error.as_deref().unwrap_or("Discord didn't say why.")) }
                " " span.muted.small { "Tried " (text::ago(st.checked_at)) ". It tries again every hour." }
            },
        }
    };

    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, "Invites"))
        div.with-rail {
            div.stack.wl-main {
                p { (status) }
                (ui::settings_form(&page, &page.path("welcome/invites"), ui::card(html! {
                    (ui::field_toggle("track_invites", "Track invites", Some("Works out which invite each new member used. The bot needs the Manage Server permission to read invites."), s.track_invites))
                })))
                (ui::section("Invites", Some("Name a link to see where members come from, like \"Twitch link\". The name shows on Joined today and in recent joins."), html! {
                    @if live.is_empty() {
                        (ui::empty(Icon::Door, "No invites read yet", "The server's invites show up here once the bot has read them."))
                    } @else {
                        form method="post" action=(page.path("welcome/invites/names")) {
                            div.list {
                                @for inv in &live {
                                    div.list-item.wl-invite {
                                        div.grow {
                                            strong { code { (inv.code) } }
                                            div.muted.small.clip {
                                                @match inv.inviter { Some(u) => { "by " (ctx.display_name(guild, u)) " · " }, None => {} }
                                                (counted(inv.uses, "use", "uses"))
                                                @if inv.max_uses > 0 { " of " (inv.max_uses) }
                                            }
                                        }
                                        @let name = names.get(&inv.code).map(String::as_str).unwrap_or_default();
                                        @if page.can_edit() {
                                            input type="hidden" name="code" value=(inv.code);
                                            input.input type="text" name={ "name-" (inv.code) } value=(name) maxlength="40" placeholder="Name this link" aria-label={ "Name for invite " (inv.code) };
                                        } @else if !name.is_empty() {
                                            span.meta { (name) }
                                        }
                                    }
                                }
                            }
                            @if page.can_edit() {
                                div.wl-send { button.btn.small type="submit" { "Save names" } }
                            }
                        }
                    }
                }))
                (ui::section("Top inviters", Some("Members whose invites brought people in."), html! {
                    @if top.is_empty() {
                        (ui::empty(Icon::Users, "No invited joins yet", "Joins through a member's invite are counted here once tracking has taken its first reading."))
                    } @else {
                        div.list {
                            @for (inviter, recent, all) in &top {
                                div.list-item {
                                    img.avatar src=(ctx.member_avatar(guild, *inviter)) alt="" width="32" height="32" loading="lazy";
                                    div.grow { strong.clip { (ctx.display_name(guild, *inviter)) } }
                                    span.meta { (text::num(*recent)) " in 30 days · " (text::num(*all)) " all time" }
                                }
                            }
                        }
                    }
                }))
                (ui::section("Recent joins", None, html! {
                    @if recent.is_empty() {
                        (ui::empty(Icon::Door, "No joins recorded", "New members appear here with the invite they came through."))
                    } @else {
                        div.list {
                            @for j in &recent {
                                div.list-item {
                                    img.avatar src=(ctx.member_avatar(guild, j.user)) alt="" width="32" height="32" loading="lazy";
                                    div.grow {
                                        strong.clip { (ctx.display_name(guild, j.user)) }
                                        div.muted.small.clip {
                                            @match j.inviter {
                                                Some(i) => { "invited by " (ctx.display_name(guild, i)) },
                                                None => { "invite unknown" },
                                            }
                                            @if let Some(c) = &j.code {
                                                " · "
                                                @match names.get(c) { Some(n) => { (n) }, None => { code { (c) } } }
                                            }
                                        }
                                    }
                                    span.meta { (text::ago(j.at)) }
                                }
                            }
                        }
                    }
                }))
            }
            aside.rail {
                div.rail-block {
                    h2 { "Joins, 30 days" }
                    div.wl-big {
                        span.big-num { (text::num(joins)) }
                        span.muted.small { "members came in" }
                    }
                }
                div.rail-block {
                    h2 { "With a known inviter" }
                    @if joins > 0 {
                        div.wl-big {
                            span.big-num { (known * 100 / joins) "%" }
                            span.muted.small { "of joins matched to an invite" }
                        }
                    } @else {
                        p.muted.small { "Nobody has joined in the last 30 days." }
                    }
                }
                div.rail-block {
                    h2 { "Invites watched" }
                    div.wl-big {
                        span.big-num { (state.as_ref().map(|s| s.codes).unwrap_or(0)) }
                        span.muted.small { "invites the bot reads each hour" }
                    }
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
