//! Dashboard pages: giveaways, polls and suggestions.

mod giveaways;
mod polls;
mod suggestions;

use bot_core::axum::Router;
use bot_core::axum::extract::Path;
use bot_core::axum::response::Redirect;
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::web::ui;
use bot_core::web::{Page, User, WebError};
use bot_core::{Ctx, Icon, now_ms, text};
use serde::Deserialize;

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/community", get(|Path(guild): Path<u64>| async move { Redirect::to(&format!("/g/{guild}/community/giveaways")) }))
        .route("/g/{guild}/community/giveaways", get(giveaways::page).post(giveaways::create))
        .route("/g/{guild}/community/giveaways/{id}/end", post(giveaways::end))
        .route("/g/{guild}/community/giveaways/{id}/reroll", post(giveaways::reroll))
        .route("/g/{guild}/community/giveaways/{id}/edit", post(giveaways::edit))
        .route("/g/{guild}/community/giveaways/{id}/test", post(giveaways::send_test))
        .route("/g/{guild}/community/polls", get(polls::page).post(polls::create))
        .route("/g/{guild}/community/polls/{id}/end", post(polls::end))
        .route("/g/{guild}/community/suggestions", get(suggestions::page).post(suggestions::save_settings))
        .route("/g/{guild}/community/suggestions/{id}/status", post(suggestions::set_status))
        .route("/g/{guild}/community/suggestions/test", post(suggestions::send_test))
}

/// `?tab=` on the three pages. Each page reads the tab names it knows and ignores the rest.
/// `?sent=1` follows a test the dashboard sent.
#[derive(Deserialize)]
pub struct TabQuery {
    pub tab: Option<String>,
    pub sent: Option<String>,
}

/// A test the bot couldn't DM to the person who asked for it.
fn dm_refused() -> WebError {
    WebError::Bad("Discord didn't let the bot DM you. Allow direct messages from members of this server (the server's Privacy Settings in Discord), then try again.".into())
}

/// A channel that belongs to this server, from a form field.
fn channel_in(ctx: &Ctx, guild: u64, id: Option<u64>) -> Result<u64, WebError> {
    let id = id.ok_or_else(|| WebError::Bad("Pick a channel.".into()))?;
    if ctx.text_channels(guild).iter().any(|(c, _)| *c == id) {
        Ok(id)
    } else {
        Err(WebError::Bad("That channel isn't in this server.".into()))
    }
}

fn channel_name(ctx: &Ctx, guild: u64, id: u64) -> String {
    ctx.text_channels(guild).into_iter().find(|(c, _)| *c == id).map(|(_, n)| format!("#{n}")).unwrap_or_else(|| "Deleted channel".into())
}

fn role_name(ctx: &Ctx, guild: u64, id: u64) -> String {
    ctx.roles(guild).into_iter().find(|(r, _, _)| *r == id).map(|(_, n, _)| format!("@{n}")).unwrap_or_else(|| "Deleted role".into())
}

/// A channel picker with no "None" choice, for forms that must post somewhere.
fn channel_select(ctx: &Ctx, guild: u64, name: &str, label: &str, help: Option<&str>) -> Markup {
    let opts: Vec<(String, String)> = ctx.text_channels(guild).into_iter().map(|(id, n)| (id.to_string(), format!("#{n}"))).collect();
    let first = opts.first().map(|o| o.0.clone()).unwrap_or_default();
    ui::field_select(name, label, help, &opts, &first)
}

/// A header button that opens a form in a popover, for editors only.
fn header_pop(label: &str, body: Markup) -> Markup {
    ui::pop("cm-pop", label, Some(Icon::Plus), "primary", body)
}

/// A link to the message in Discord. Nothing is shown when the message isn't known. `compact`
/// draws the icon alone, named by its tooltip.
fn open_link(page: &Page, channel: u64, message: Option<u64>, compact: bool) -> Markup {
    html! {
        @if let Some(m) = message {
            @if compact {
                a.icon-btn.cm-out href=(crate::message_url(page.guild_id, channel, m)) target="_blank" rel="noopener" data-tip="Open in Discord" aria-label="Open in Discord" {
                    (ui::icon(Icon::External))
                }
            } @else {
                a.btn.small.ghost.cm-out href=(crate::message_url(page.guild_id, channel, m)) target="_blank" rel="noopener" {
                    (ui::icon(Icon::External)) "Open in Discord"
                }
            }
        }
    }
}

/// Up to `max` overlapping member avatars.
fn face_row(ctx: &Ctx, guild: u64, users: &[u64], max: usize) -> Markup {
    let urls: Vec<String> = users.iter().map(|u| ctx.member_avatar(guild, *u)).collect();
    ui::avatars(&urls, max)
}

/// "2 days 5 h", "5 h 12 min", "39 min", "42 s": how long is left. The countdown script writes
/// the same shape, so the text doesn't jump when it takes over.
fn left(secs: i64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    let days = |d: i64| if d == 1 { "1 day".to_owned() } else { format!("{d} days") };
    match (d, h, m) {
        (0, 0, 0) => format!("{secs} s"),
        (0, 0, m) => format!("{m} min"),
        (0, h, 0) => format!("{h} h"),
        (0, h, m) => format!("{h} h {m} min"),
        (d, 0, _) => days(d),
        (d, h, _) => format!("{} {h} h", days(d)),
    }
}

/// "Ends in 5 h 12 min" for a time still to come, "Ending now" once it has passed.
fn until(prefix: &str, at: i64) -> String {
    let secs = (at - now_ms()) / 1000;
    if secs <= 0 { "Ending now".to_owned() } else { format!("{prefix} {}", left(secs)) }
}

/// [`until`] that keeps counting down in the browser.
fn countdown(prefix: &str, at: i64) -> Markup {
    html! { span.cm-cd data-ends=(at) data-pre=(prefix) { (until(prefix, at)) } }
}

/// A moment in the viewer's time: "Fri Oct 31, 3:00 PM".
fn when(page: &Page, ms: i64) -> String {
    page.user.local(ms).strftime("%a %b %-d, %-I:%M %p").to_string()
}

/// Midnight on the first day of this month for the viewer, in epoch milliseconds.
fn month_start(user: &User, now: i64) -> i64 {
    let local = user.local(now);
    let midnight = local.date().first_of_month().at(0, 0, 0, 0);
    midnight.to_zoned(local.time_zone().clone()).map(|z| z.timestamp().as_millisecond()).unwrap_or(now)
}

fn plural(n: i64, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 { one } else { many }
}

/// Up to three figures that count up, side by side in a rail block: `(label, value)`.
fn rail_figures(items: &[(&str, i64)]) -> Markup {
    html! {
        div.rail-nums {
            @for (label, value) in items {
                div { b.mono data-count { (text::num(*value)) } span { (label) } }
            }
        }
    }
}

// ---- Discord, as members see it ----

/// The bot's name and avatar. Before the bot has connected (and in the preview) the avatar is
/// `None` and the dashboard's mark stands in.
fn bot_identity(ctx: &Ctx) -> (String, Option<String>) {
    match ctx.cache.current_user() {
        Some(u) => (u.global_name.clone().unwrap_or_else(|| u.name.clone()), Some(bot_core::discord::avatar_url(u.id.get(), u.avatar.map(|a| a.to_string()).as_deref()))),
        None => ("Bot".into(), None),
    }
}

/// A message from the bot as Discord draws it: avatar, name with its APP tag and the time,
/// then `body` (embeds, a poll, buttons).
fn discord_message(ctx: &Ctx, page: &Page, at: i64, body: Markup) -> Markup {
    let (name, avatar) = bot_identity(ctx);
    let today = page.user.local(now_ms()).date() == page.user.local(at).date();
    let time = page.user.local(at).strftime(if today { "Today at %-I:%M %p" } else { "%m/%d/%Y %-I:%M %p" }).to_string();
    html! {
        div.cm-dc {
            @match &avatar {
                Some(url) => img.cm-dc-av src=(url) alt="" width="36" height="36" loading="lazy";,
                None => span.cm-dc-av.mark { (ui::mark()) },
            }
            div.cm-dc-main {
                div.cm-dc-head { b.cm-dc-name { (name) } span.cm-dc-app { "APP" } span.cm-dc-time { (time) } }
                (body)
            }
        }
    }
}

/// Discord's relative timestamp, as a member sees `<t:…:R>`: "in 39 minutes", "5 hours ago".
/// The message a hero is about, as members see it, under what it is and where it was posted:
/// "The post · #announcements". Sits beside the hero's body on a wide page, under it on a narrow one.
fn post_block(label: &str, channel: &str, message: Markup, foot: Option<Markup>) -> Markup {
    html! {
        div.cm-post {
            h3.cm-post-h { (label) span.muted { (channel) } }
            (message)
            @if let Some(f) = foot { div.cm-post-foot { (f) } }
        }
    }
}

fn discord_relative(at: i64) -> String {
    let secs = (at - now_ms()) / 1000;
    let a = secs.abs();
    let (n, unit) = match a {
        0..60 => (a, "second"),
        60..3600 => (a / 60, "minute"),
        3600..86_400 => (a / 3600, "hour"),
        86_400..2_592_000 => (a / 86_400, "day"),
        _ => (a / 2_592_000, "month"),
    };
    let unit = if n == 1 { unit.to_owned() } else { format!("{unit}s") };
    let n = if n == 1 && unit == "hour" { "an".to_owned() } else if n == 1 { "a".to_owned() } else { n.to_string() };
    if secs >= 0 { format!("in {n} {unit}") } else { format!("{n} {unit} ago") }
}

pub(super) fn style() -> Markup {
    html! { style { (PreEscaped(STYLE)) } }
}

/// Countdowns: every `[data-ends]` counts down each second. A `[data-clock]` fills its
/// `[data-u]` cells (days, hours, minutes, seconds), one `.gv-dg` slot per digit, and rolls
/// only the digit that changed. Ticks land just after each whole second of the clock's own
/// deadline, so a second never shows twice or gets skipped.
pub(super) fn script() -> Markup {
    html! { script { (PreEscaped(SCRIPT)) } }
}

const SCRIPT: &str = r#"
(() => {
  const els = [...document.querySelectorAll("[data-ends]")];
  if (!els.length) return;
  const signal = window.botPage?.signal;
  const calm = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const pad = (n) => String(n).padStart(2, "0");
  const days = (d) => d === 1 ? "1 day" : d + " days";
  const left = (s) => {
    const d = Math.floor(s / 86400), h = Math.floor(s % 86400 / 3600), m = Math.floor(s % 3600 / 60);
    if (d) return h ? days(d) + " " + h + " h" : days(d);
    if (h) return m ? h + " h " + m + " min" : h + " h";
    return m ? m + " min" : s + " s";
  };
  // One digit: the old one rolls down and out as the new one rolls in from above.
  const roll = (slot, ch, still) => {
    const old = slot.lastElementChild;
    if (old && old.textContent === ch) return;
    const next = document.createElement("span");
    next.textContent = ch;
    slot.append(next);
    if (!old) return;
    if (still) { old.remove(); return; }
    const o = { duration: 520, easing: "cubic-bezier(0.22, 1, 0.36, 1)" };
    next.animate([{ transform: "translateY(-100%)", opacity: 0 }, { transform: "none", opacity: 1 }], o);
    const out = old.animate([{ transform: "none", opacity: 1 }, { transform: "translateY(100%)", opacity: 0 }], { ...o, fill: "forwards" });
    out.finished.then(() => old.remove(), () => old.remove());
  };
  const cell = (u, t, still) => {
    let slots = u.querySelectorAll(":scope > .gv-dg");
    if (slots.length !== t.length) {
      u.replaceChildren(...[...t].map(() => Object.assign(document.createElement("span"), { className: "gv-dg" })));
      slots = u.querySelectorAll(":scope > .gv-dg");
      still = true;
    }
    [...t].forEach((ch, i) => roll(slots[i], ch, still));
  };
  const tick = (still) => {
    const now = Date.now();
    els.forEach((el) => {
      const s = Math.max(0, Math.floor((Number(el.dataset.ends) - now) / 1000));
      const t = s ? el.dataset.pre + " " + left(s) : "Ending now";
      if (el.hasAttribute("data-clock")) {
        const v = { d: Math.floor(s / 86400), h: Math.floor(s % 86400 / 3600), m: Math.floor(s % 3600 / 60), s: s % 60 };
        el.querySelectorAll("[data-u]").forEach((u) => cell(u, pad(v[u.dataset.u]), still));
        const label = s ? "Ends in " + left(s) : "Ending now";
        if (el.getAttribute("aria-label") !== label) el.setAttribute("aria-label", label);
      } else if (el.textContent !== t) el.textContent = t;
    });
  };
  // Every countdown shows whole seconds, so each tick lands 30 ms after the next whole second
  // of the main clock's deadline (the first countdown when there's no clock).
  const anchor = Number((els.find((el) => el.hasAttribute("data-clock")) || els[0]).dataset.ends);
  let timer = 0;
  const loop = () => {
    if (!document.hidden) tick(calm);
    const until = anchor - Date.now();
    if (els.every((el) => Number(el.dataset.ends) < Date.now() - 1000)) return;
    timer = setTimeout(loop, (until % 1000 + 1000) % 1000 + 30);
  };
  tick(true);
  timer = setTimeout(loop, ((anchor - Date.now()) % 1000 + 1000) % 1000 + 30);
  // Back on the tab: catch up at once, without rolling every digit.
  document.addEventListener("visibilitychange", () => { if (!document.hidden) tick(true); }, { signal });
  signal?.addEventListener("abort", () => clearTimeout(timer));
})();
"#;

const STYLE: &str = r#"
/* ---- shared ---- */
.cm-main { min-width: 0; display: flex; flex-direction: column; gap: 30px; }
/* The phone layout's single column grows to its widest child; long one-line text in the rail
   mustn't widen the page. */
.cm-rail { min-width: 0; }
.cm-block { display: flex; flex-direction: column; gap: 14px; min-width: 0; }
.cm-block > h2, .cm-h { display: flex; align-items: baseline; gap: 10px; font-size: 15px; }
.cm-h .muted { font-size: 12.5px; font-weight: 400; }
.cm-pop .pop-body { width: min(560px, calc(100vw - 32px)); padding: 16px; }
.cm-pop .pop-body .card { padding: 0 14px; }
.cm-pop-foot { display: flex; justify-content: flex-end; }
.cm-actions { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; }
.cm-out .icon { width: 14px; height: 14px; }
.cm-cd { font-variant-numeric: tabular-nums; }
.cm-rail .rail-block > h2 .muted { font-size: 12px; font-weight: 400; }
.cm-rail .rail-nums b { font-size: 24px; }
.cm-edit .pop-body { width: min(500px, calc(100vw - 32px)); padding: 16px; }
.cm-edit.left .pop-body { left: 0; right: auto; transform-origin: top left; }
.cm-edit .pop-body .card { padding: 0 14px; }
.cm-review .pop-body { width: 290px; }
.cm-review .stack { gap: 8px; }
.gv-reroll .pop-body { display: flex; flex-direction: column; gap: 8px; }
.gv-reroll .pop-body > .muted { margin: 0 0 2px; }
.gv-reroll-row { display: flex; align-items: center; gap: 10px; min-width: 0; }
.gv-reroll-row .grow { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; }
.gv-reroll .pop-body > form:last-child { align-self: flex-start; margin-top: 4px; }
/* A block with an open form lifts above the blocks after it, so the form isn't painted under them. */
.gv-hero:has(.pop[open]), .gv-card:has(.pop[open]), .sg-card:has(.pop[open]) { position: relative; z-index: 5; }
/* Settings beside the post they shape: the preview keeps about Discord's own width, the form
   takes the rest. */
@container col (min-width: 1000px) {
  .compose.cm-compose { grid-template-columns: minmax(0, 1fr) min(600px, 42%); }
}
/* The post a hero is about, beside it on a wide page. */
.cm-post { min-width: 0; display: flex; flex-direction: column; gap: 10px; }
.cm-post-h { display: flex; align-items: baseline; justify-content: space-between; gap: 10px; margin: 0; font-size: 13px; font-weight: 600; color: var(--ink-2); }
.cm-post-h .muted { font-size: 12px; font-weight: 400; }
.cm-post-foot { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; }
.cm-post-foot .cm-dc-note { flex: 1; min-width: 180px; }
/* A test under a preview in the rail. */
.cm-test { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; }
.cm-test .cm-dc-note { flex: 1; min-width: 160px; }
/* A rail line standing on its own as a tile, when the rail's list moves into the main column. */
.cm-tiles > .mini, .cm-tiles > .pl-recent { padding: 12px 14px; border-radius: 14px; background: linear-gradient(180deg, rgba(243, 242, 248, 0.045), rgba(243, 242, 248, 0.015)); box-shadow: inset 0 0 0 1px var(--edge-1); }
.cm-tiles > .pl-recent + .pl-recent { padding-top: 12px; border-top: 0; }

/* A message as Discord draws it. Discord's own greys, since this is Discord. */
.cm-dc { display: flex; gap: 12px; padding: 14px 14px 16px; border-radius: 14px; background: #313338; color: #dbdee1; font: 14px/1.375 var(--sans); box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.05), 0 18px 40px -24px rgba(0, 0, 0, 0.9); animation: cm-arrive 0.8s var(--ease) both 0.35s; }
.cm-dc-av { width: 36px; height: 36px; border-radius: 50%; flex: none; object-fit: cover; }
.cm-dc-av.mark { display: grid; place-items: center; background: linear-gradient(145deg, var(--accent-hi), var(--accent-lo)); color: #fff; }
.cm-dc-av.mark .icon { width: 20px; height: 20px; }
.cm-dc-main { flex: 1; min-width: 0; display: flex; flex-direction: column; align-items: flex-start; gap: 4px; }
.cm-dc-head { display: flex; align-items: center; flex-wrap: wrap; gap: 6px; }
.cm-dc-name { color: #f2f3f5; font-weight: 500; }
.cm-dc-app { display: inline-grid; place-items: center; height: 15px; padding: 0 4px; border-radius: 3px; background: #5865f2; color: #fff; font-size: 10px; font-weight: 600; }
.cm-dc-time { color: #949ba4; font-size: 11.5px; }
.cm-dc-embed { align-self: stretch; display: flex; flex-direction: column; gap: 6px; max-width: 520px; padding: 10px 12px 12px; border-left: 4px solid var(--embed, #1e1f22); border-radius: 4px; background: #2b2d31; font-size: 13px; min-width: 0; }
.cm-dc-author { display: flex; align-items: center; gap: 8px; font-size: 12.5px; font-weight: 600; color: #f2f3f5; }
.cm-dc-author img { width: 22px; height: 22px; border-radius: 50%; object-fit: cover; }
.cm-dc-title { color: #f2f3f5; font-size: 14px; font-weight: 600; overflow-wrap: anywhere; }
.cm-dc-desc { white-space: pre-wrap; overflow-wrap: anywhere; }
.cm-dc-fields { display: flex; flex-wrap: wrap; gap: 6px 18px; }
.cm-dc-fields > div { display: flex; flex-direction: column; gap: 1px; min-width: 0; }
.cm-dc-fields b { color: #f2f3f5; font-weight: 600; font-size: 12.5px; }
.cm-dc-foot { color: #b5bac1; font-size: 11.5px; }
.cm-dc-ts { padding: 0 2px; border-radius: 3px; background: rgba(255, 255, 255, 0.06); }
.cm-dc-ping { padding: 0 2px; border-radius: 3px; background: rgba(88, 101, 242, 0.3); color: #c9cdfb; font-weight: 500; }
.cm-dc-btns { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 4px; }
.cm-dc-btn { display: inline-flex; align-items: center; height: 32px; padding: 0 14px; border-radius: 8px; background: #4e5058; color: #fff; font-size: 13.5px; font-weight: 500; white-space: nowrap; }
.cm-dc-btn.blurple { background: #5865f2; }
.cm-dc-btn.off { opacity: 0.5; }
.cm-dc-note { font-size: 12px; color: var(--ink-3); }
.cm-dc-thread { display: flex; align-items: flex-start; margin: 2px 0 0 -30px; }
.cm-dc-curve { width: 26px; height: 20px; flex: none; margin-top: -4px; border-left: 2px solid #4e5058; border-bottom: 2px solid #4e5058; border-bottom-left-radius: 8px; }
.cm-dc-tbox { white-space: nowrap; display: inline-flex; align-items: center; gap: 8px; margin: 8px 0 0 4px; padding: 7px 10px 7px 12px; border-radius: 8px; background: #2b2d31; font-size: 13px; }
.cm-dc-tbox strong { color: #f2f3f5; font-weight: 600; }
.cm-dc-tbox span { color: #00a8fc; font-weight: 500; }
.cm-dc-tbox .icon { width: 12px; height: 12px; color: #949ba4; transform: rotate(-90deg); }

/* ---- giveaways ---- */
.gv-art { position: relative; display: grid; place-items: center; flex: none; width: 56px; height: 56px; border-radius: 16px; overflow: hidden; isolation: isolate; color: color-mix(in oklab, var(--hue), white 30%);
  background: radial-gradient(85% 85% at 20% 10%, color-mix(in oklab, var(--hue) 85%, transparent), transparent 66%), radial-gradient(90% 90% at 100% 100%, color-mix(in oklab, var(--accent) 80%, transparent), transparent 64%), radial-gradient(60% 60% at 80% 15%, color-mix(in oklab, var(--data-2) 45%, transparent), transparent 70%), color-mix(in oklab, var(--hue) 22%, var(--raised));
  box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.08), inset 0 1px 0 rgba(255, 255, 255, 0.16); }
.gv-art .icon { width: 24px; height: 24px; }
.gv-emoji { font-size: 28px; line-height: 1; filter: drop-shadow(0 6px 12px rgba(0, 0, 0, 0.45)); }
.gv-art.ended { filter: saturate(0.6) brightness(0.85); }
.gv-art.big { width: 188px; height: 188px; border-radius: 30px; box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.1), inset 0 1px 0 rgba(255, 255, 255, 0.2); animation: gv-pop 0.9s var(--spring) both 0.12s; }
.gv-art.big::before { content: ""; position: absolute; inset: -45%; z-index: -1; background: conic-gradient(from 0deg, transparent 0 18%, rgba(255, 255, 255, 0.13) 24%, transparent 30% 68%, rgba(255, 255, 255, 0.09) 74%, transparent 80%); animation: gv-spin 16s linear 4; }
.gv-art.big .gv-emoji { font-size: 96px; animation: gv-float 3.2s ease-in-out 0.9s 10 alternate; }
.gv-art.big .icon { width: 72px; height: 72px; stroke-width: 1.4; }

/* The giveaway ending soonest, across the page: the prize art, the prize with its clock, and
   the post as members see it. Two columns (art, body; the post under the body) from 600 px,
   three from 1000. */
.gv-hero { position: relative; display: grid; grid-template-columns: minmax(0, 1fr); gap: 18px 26px; align-items: center; padding: 18px; border-radius: 24px; isolation: isolate;
  background: linear-gradient(180deg, rgba(243, 242, 248, 0.05), rgba(243, 242, 248, 0.02));
  box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--hue) 22%, var(--edge-1)); }
.gv-hero-body { display: flex; flex-direction: column; align-items: flex-start; gap: 14px; min-width: 0; }
.gv-prize { font-size: 27px; line-height: 1.15; letter-spacing: -0.03em; overflow-wrap: anywhere; }
.gv-meta { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 8px; font-size: 13px; color: var(--ink-3); margin-top: -6px; }
.gv-meta strong { color: var(--ink-2); font-weight: 500; }
.gv-meta .avatar { width: 20px; height: 20px; }
.gv-by { display: inline-flex; align-items: center; gap: 6px; }
.gv-role { display: inline-flex; align-items: center; height: 22px; padding: 0 8px; border-radius: 11px; background: color-mix(in oklab, var(--hue) 14%, transparent); color: var(--hue); font-size: 12px; font-weight: 500; }
.gv-clock { display: flex; gap: 8px; }
.gv-unit { display: flex; flex-direction: column; align-items: center; gap: 2px; min-width: 66px; padding: 10px 10px 8px; border-radius: 12px; background: var(--well); }
/* Each digit has its own slot; the countdown script rolls the old digit out and the new one
   in, inside the slot, so the figure never changes width and nothing around it moves. */
.gv-unit b { display: inline-flex; font: 600 28px/1.15 var(--sans); letter-spacing: -0.01em; font-variant-numeric: tabular-nums; }
.gv-dg { display: inline-grid; overflow: hidden; }
.gv-dg > span { grid-area: 1 / 1; }
.gv-unit > span { font-size: 11.5px; color: var(--ink-3); }
.gv-time { display: flex; flex-direction: column; gap: 7px; width: 100%; }
.gv-time .meter > span, .gv-card .meter > span { background: var(--data); }
@container col (min-width: 600px) {
  .gv-hero { grid-template-columns: auto minmax(0, 1fr); padding: 24px; }
  .gv-hero > .cm-post { grid-column: 2; }
}
@container col (min-width: 1000px) {
  .gv-hero { grid-template-columns: auto minmax(0, 1fr) min(560px, 38%); }
  .gv-hero > .cm-post { grid-column: 3; grid-row: 1; }
}
@container col (min-width: 1500px) {
  .gv-hero { gap: 18px 36px; padding: 28px; }
  .gv-prize { font-size: 32px; }
  .gv-unit { min-width: 80px; padding: 12px 12px 9px; }
  .gv-unit b { font-size: 34px; }
}
.gv-time-cap { display: flex; justify-content: space-between; gap: 12px; font-size: 12px; color: var(--ink-3); }
.gv-time-cap strong { color: var(--ink-2); font-weight: 500; }
.gv-people { display: flex; align-items: center; gap: 12px; font-size: 13px; color: var(--ink-3); }
.gv-people .avatars .avatar { width: 32px; height: 32px; }
.gv-people b { color: var(--ink); font-weight: 600; }

.gv-grid { --tile: 310px; }
.gv-done-grid { --tile: 340px; }
.gv-winners { --tile: 280px; }
.gv-card { display: flex; flex-direction: column; gap: 14px; padding: 16px; border-radius: 18px; background: linear-gradient(180deg, rgba(243, 242, 248, 0.045), rgba(243, 242, 248, 0.015)); box-shadow: inset 0 0 0 1px var(--edge-1); transition: box-shadow 0.25s, background 0.25s; animation: rise 0.6s var(--ease) both; animation-delay: calc(var(--i, 0) * 70ms + 250ms); }
.gv-card:hover { box-shadow: inset 0 0 0 1px var(--edge-2), var(--shadow-lift); }
.gv-card-top { display: flex; align-items: center; gap: 12px; min-width: 0; }
.gv-card-top > .grow { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.gv-card-top > .cm-out { align-self: flex-start; margin: -4px -4px 0 0; }
.gv-card-top strong { font-size: 14.5px; font-weight: 600; line-height: 1.3; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
.gv-card-top .muted { font-size: 12px; }
.gv-left { display: flex; align-items: center; justify-content: space-between; gap: 10px; font-size: 12.5px; }
.gv-left > span:first-child { display: inline-flex; align-items: center; gap: 6px; color: var(--hue); font-weight: 500; }
.gv-left .icon { width: 14px; height: 14px; }
.gv-card .meter { height: 4px; }
.gv-card-foot { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
.gv-card-foot .gv-people { min-width: 0; }
.gv-card-foot .cm-actions { flex: none; flex-wrap: nowrap; }
.gv-card-foot .gv-people { font-size: 12.5px; }
.gv-card-foot .gv-people .avatars .avatar { width: 26px; height: 26px; }

/* An ended giveaway: the same card, with who won in the middle and the totals in the foot. */
.gv-card.done { gap: 12px; }
.gv-card.done .gv-card-foot { min-height: 29px; font-size: 12.5px; color: var(--ink-3); }
.gv-won { display: flex; align-items: center; gap: 10px; min-width: 0; font-size: 13px; color: var(--ink-2); }
.gv-won .avatars { flex: none; }
.gv-won .avatars .avatar { width: 30px; height: 30px; box-shadow: 0 0 0 2px var(--page), 0 0 0 3.5px color-mix(in oklab, var(--hue) 85%, transparent); }
.gv-won .avatars .avatar + .avatar { margin-left: -6px; }
.gv-won > span:last-child { min-width: 0; overflow-wrap: anywhere; }
.gv-won strong { color: var(--ink); font-weight: 600; }
.gv-won > .icon { width: 16px; height: 16px; color: var(--hue); }
.gv-nobody { font-size: 12.5px; color: var(--ink-3); }

.cm-winner .avatar { box-shadow: 0 0 0 2px var(--page), 0 0 0 3.5px color-mix(in oklab, var(--hue) 80%, transparent); }
.cm-winner strong { font-weight: 600; }

/* ---- polls ---- */
/* The newest open poll, across the page: the results, and beside them (under them when narrow)
   the poll as members see it in Discord. */
.pl-hero { display: grid; grid-template-columns: minmax(0, 1fr); gap: 22px 36px; align-items: start; padding: 22px 24px 20px; border-radius: 24px;
  background: linear-gradient(180deg, rgba(243, 242, 248, 0.05), rgba(243, 242, 248, 0.02));
  box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--data) 18%, var(--edge-1)); }
.pl-main { display: flex; flex-direction: column; gap: 18px; min-width: 0; }
@container col (min-width: 1000px) {
  .pl-hero { grid-template-columns: minmax(0, 1fr) min(520px, 36%); padding: 24px 26px 22px; }
}
.pl-ask { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; font-size: 13px; color: var(--ink-3); }
.pl-ask .avatar { width: 34px; height: 34px; }
.pl-ask strong { color: var(--ink-2); font-weight: 600; }
.pl-ask .status { margin-left: auto; background: var(--control); backdrop-filter: none; }
.pl-q { font-size: 24px; line-height: 1.2; letter-spacing: -0.03em; overflow-wrap: anywhere; }
.pl-answers { display: flex; flex-direction: column; gap: 8px; }
.pl-ans { display: grid; grid-template-columns: minmax(0, 1fr) 70px 64px; align-items: center; gap: 12px; }
.pl-ans.bare { grid-template-columns: minmax(0, 1fr) 64px; }
.pl-bar { position: relative; display: flex; align-items: center; gap: 12px; min-height: 46px; padding: 6px 14px; border-radius: 12px; background: var(--well); box-shadow: inset 0 0 0 1px var(--edge-1); overflow: hidden; isolation: isolate; }
.pl-fill { position: absolute; inset: 0 auto 0 0; z-index: -1; background: color-mix(in oklab, var(--data) 20%, transparent); transform-origin: left; animation: fill 1.2s var(--ease) both; animation-delay: calc(var(--i, 0) * 90ms + 280ms); }
.pl-ans.lead .pl-fill { background: linear-gradient(90deg, color-mix(in oklab, var(--data-2) 62%, transparent), color-mix(in oklab, var(--data) 50%, transparent)); box-shadow: none; }
.pl-ans.lead .pl-bar { box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--data) 40%, transparent); }
.pl-label { flex: 1; min-width: 0; display: flex; align-items: center; gap: 8px; font-size: 14px; font-weight: 500; color: var(--ink-2); overflow-wrap: anywhere; }
.pl-label .icon { width: 15px; height: 15px; flex: none; }
.pl-ans.lead .pl-label { color: var(--ink); font-weight: 600; }
.pl-pct { font: 600 13.5px var(--mono); color: var(--ink-2); font-variant-numeric: tabular-nums; flex: none; }
.pl-ans.lead .pl-pct { color: var(--ink); }
.pl-voters { display: flex; justify-content: flex-end; }
.pl-voters .avatar { width: 24px; height: 24px; }
.pl-n { font: 12px var(--mono); color: var(--ink-3); text-align: right; white-space: nowrap; }
.pl-foot { display: flex; align-items: center; justify-content: space-between; gap: 12px; flex-wrap: wrap; font-size: 12.5px; color: var(--ink-3); }
.pl-foot b { color: var(--ink-2); font-weight: 600; }

.pl-grid { --tile: 400px; }
/* Ended polls differ in height (two answers or ten), so they pack into columns. */
.pl-cols { columns: 420px; column-gap: 12px; }
.pl-cols > .pl-card { break-inside: avoid; margin-bottom: 12px; }
.pl-recents { --tile: 300px; }
.pl-card { display: flex; flex-direction: column; gap: 12px; padding: 16px 18px; border-radius: 18px; background: linear-gradient(180deg, rgba(243, 242, 248, 0.045), rgba(243, 242, 248, 0.015)); box-shadow: inset 0 0 0 1px var(--edge-1); animation: rise 0.6s var(--ease) both; animation-delay: calc(var(--i, 0) * 70ms + 200ms); }
.pl-card-top { display: flex; align-items: flex-start; gap: 12px; }
.pl-card-text { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
.pl-card-top .cm-actions { flex: none; flex-wrap: nowrap; }
.pl-card-top strong { font-size: 15px; font-weight: 600; line-height: 1.35; overflow-wrap: anywhere; }
.pl-card-top .muted { font-size: 12.5px; }
.pl-card-top .avatar { width: 30px; height: 30px; }
.pl-card.ended .pl-answers { gap: 6px; }
.pl-card.ended .pl-bar { min-height: 38px; font-size: 13px; }
.pl-card.ended .pl-label { font-size: 13px; }
.pl-stack { display: flex; gap: 2px; height: 8px; border-radius: 4px; overflow: hidden; background: var(--edge-1); animation: cm-wipe 1.2s var(--ease) both 0.35s; }
.pl-stack > span { min-width: 3px; transition: filter 0.15s; }
.pl-stack > span:hover { filter: brightness(1.3); }
.pl-lead { display: flex; align-items: center; justify-content: space-between; gap: 10px; flex-wrap: wrap; font-size: 12.5px; color: var(--ink-3); }
.pl-lead strong { color: var(--ink); font-weight: 600; }
.pl-recent { display: flex; flex-direction: column; gap: 6px; padding: 2px 0; min-width: 0; }
.pl-recent + .pl-recent { padding-top: 12px; border-top: 1px solid var(--edge-1); }
.pl-recent > strong { font-size: 13px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.pl-recent .pl-stack { height: 6px; }
.pl-recent .pl-lead { font-size: 12px; }
.pl-recent .pl-lead > span:first-child { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.pl-busy { display: flex; flex-direction: column; gap: 6px; font-size: 12.5px; min-width: 0; }
.pl-busy + .pl-busy { margin-top: 6px; }
.pl-busy > div { display: flex; justify-content: space-between; gap: 10px; }
.pl-busy > div > span:first-child { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--ink-2); }
.pl-busy .meter { height: 4px; }

/* Discord's own poll. */
.cm-dc-poll { align-self: stretch; max-width: 520px; display: flex; flex-direction: column; gap: 8px; padding: 12px; border-radius: 8px; background: #2b2d31; box-shadow: inset 0 0 0 1px #3f4147; }
.cm-dc-poll > strong { color: #f2f3f5; font-size: 14.5px; font-weight: 600; overflow-wrap: anywhere; }
.cm-dc-poll > small { color: #949ba4; font-size: 12px; margin-top: -6px; }
.cm-dc-opt { display: flex; align-items: center; gap: 10px; min-height: 38px; padding: 6px 12px; border-radius: 8px; background: #383a40; font-size: 13.5px; color: #dbdee1; }
.cm-dc-opt > span:first-child { flex: 1; min-width: 0; overflow-wrap: anywhere; }
.cm-dc-radio { width: 18px; height: 18px; border-radius: 50%; flex: none; box-shadow: inset 0 0 0 2px #80848e; }
.cm-dc-radio.box { border-radius: 4px; }
.cm-dc-poll-foot { display: flex; align-items: center; justify-content: space-between; gap: 8px; font-size: 11.5px; color: #949ba4; margin-top: 2px; }
.cm-dc-poll-foot .cm-dc-btn { height: 30px; padding: 0 12px; }

/* ---- suggestions ---- */
/* The board: one list, a hairline between suggestions. */
.sg-list { display: flex; flex-direction: column; border-radius: var(--r-card); background: var(--well); box-shadow: inset 0 0 0 1px var(--edge-1); }
/* One suggestion. Narrow: score and words, the votes and reply under them, status and actions in
   a foot. From 600 px the status sits at the right. From 1000 px the votes and the reply take a
   column of their own, so a row reads across: score, suggestion, how it went, status. */
.sg-card { position: relative; display: grid; grid-template-columns: 46px minmax(0, 1fr); grid-template-areas: "score body" "score state" "side side"; gap: 10px 12px; align-items: start; padding: 16px 16px 14px; transition: background 0.2s; }
.sg-card + .sg-card { border-top: 1px solid var(--edge-1); }
.sg-card:first-child { border-radius: var(--r-card) var(--r-card) 0 0; }
.sg-card:last-child { border-radius: 0 0 var(--r-card) var(--r-card); }
.sg-card:only-child { border-radius: var(--r-card); }
.sg-card:hover { background: var(--well); }
/* The score: a small arrow and the net, on the first line of the suggestion. */
.sg-score { grid-area: score; display: flex; align-items: center; gap: 3px; height: 22px; color: var(--ink-3); white-space: nowrap; cursor: default; }
.sg-score .icon { width: 13px; height: 13px; flex: none; }
.sg-score b { font: 600 15px/1 var(--sans); color: var(--ink); font-variant-numeric: tabular-nums; }
.sg-score.up .icon { color: var(--data); }
.sg-score.down .icon { color: var(--held); }
.sg-score.flat b { color: var(--ink-2); }
.sg-body { grid-area: body; min-width: 0; display: flex; flex-direction: column; gap: 7px; }
.sg-state { grid-area: state; min-width: 0; display: flex; flex-direction: column; gap: 12px; }
.sg-text { max-width: 70ch; font-size: 15px; font-weight: 560; line-height: 1.45; text-wrap: pretty; overflow-wrap: anywhere; }
.sg-by { display: flex; align-items: center; flex-wrap: wrap; gap: 4px 8px; font-size: 12.5px; color: var(--ink-3); }
.sg-by .avatar { width: 20px; height: 20px; }
.sg-by strong { color: var(--ink-2); font-weight: 500; }
.sg-anon { text-decoration: underline dotted; text-underline-offset: 3px; cursor: default; }
.sg-tally { font-size: 12.5px; color: var(--ink-3); }
.sg-tally b { color: var(--ink-2); font-weight: 600; font-variant-numeric: tabular-nums; }
/* Staff's answer: who, what they did, and why, as plain lines beside their face. */
.sg-reply { display: flex; align-items: flex-start; gap: 10px; max-width: 64ch; }
.sg-reply .avatar { width: 22px; height: 22px; margin-top: 1px; }
.sg-reply > div { display: flex; flex-direction: column; gap: 2px; min-width: 0; font-size: 13px; line-height: 1.45; color: var(--ink-2); overflow-wrap: anywhere; }
.sg-reply-head { font-size: 12.5px; color: var(--ink-3); }
.sg-reply-head strong { color: var(--ink-2); font-weight: 600; }
.sg-side { grid-area: side; display: flex; flex-direction: row; align-items: center; justify-content: space-between; gap: 10px; }
.sg-side .cm-actions { flex-wrap: nowrap; gap: 4px; }
.sg-status { display: inline-flex; align-items: center; gap: 7px; font-size: 12.5px; color: var(--ink-2); white-space: nowrap; }
.sg-status.open .dot { background: var(--signal); }
.sg-status.warn .dot { background: var(--waiting); }
.sg-status.ok .dot { background: var(--live); }
.sg-status.bad .dot { background: var(--held); }
@container col (min-width: 600px) {
  .sg-card { grid-template-columns: 46px minmax(0, 1fr) auto; grid-template-areas: "score body side" "score state side"; gap: 10px 14px; padding: 16px 18px; }
  .sg-side { flex-direction: column; align-items: flex-end; justify-content: flex-start; gap: 8px; }
  .sg-status { height: 22px; }
}
@container col (min-width: 1000px) {
  .sg-card { grid-template-columns: 46px minmax(0, 1.15fr) minmax(0, 1fr) auto; grid-template-areas: "score body state side"; column-gap: 28px; padding: 18px 20px; }
  .sg-state { padding-top: 2px; }
}
.sg-top { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 8px; align-items: end; }
.sg-voter { display: flex; flex-direction: column; align-items: center; gap: 4px; min-width: 0; text-align: center; font-size: 12.5px; animation: rise 0.6s var(--ease) both; animation-delay: calc(var(--i, 0) * 90ms + 300ms); }
.sg-voter .avatar { width: 44px; height: 44px; margin-bottom: 4px; box-shadow: 0 0 0 2px var(--page), 0 0 0 3.5px var(--edge-3); }
.sg-voter.first .avatar { width: 54px; height: 54px; box-shadow: 0 0 0 2px var(--page), 0 0 0 3.5px var(--data); }
.sg-voter strong { max-width: 100%; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sg-voter span { font-size: 11.5px; color: var(--ink-3); }

@keyframes cm-arrive { from { opacity: 0; transform: translateY(16px) scale(0.97); filter: blur(4px); } }
@keyframes cm-wipe { from { clip-path: inset(0 100% 0 0); } to { clip-path: inset(0 0 0 0); } }
@keyframes gv-pop { from { opacity: 0; transform: scale(0.7) rotate(-6deg); } }
@keyframes gv-spin { to { transform: rotate(360deg); } }
@keyframes gv-float { to { transform: translateY(-6px) rotate(-3deg); } }

@media (max-width: 1180px) {
  .gv-art.big { width: 150px; height: 150px; }
  .gv-art.big .gv-emoji { font-size: 76px; }
  .gv-unit { min-width: 58px; }
  .gv-unit b { font-size: 24px; }
}
@media (max-width: 920px) {
  .gv-art.big { width: 112px; height: 112px; border-radius: 24px; }
  .gv-art.big .gv-emoji { font-size: 56px; }
  .gv-art.big .icon { width: 48px; height: 48px; }
  .gv-prize { font-size: 22px; }
  .gv-clock { gap: 6px; width: 100%; }
  .gv-unit { flex: 1; min-width: 0; padding: 8px 4px 7px; }
  .gv-unit b { font-size: 22px; }
  .pl-hero { padding: 18px 16px; gap: 16px; }
  .pl-q { font-size: 20px; }
  .pl-ask .status { margin-left: 0; }
  .pl-ans { grid-template-columns: minmax(0, 1fr) 58px; }
  .pl-voters { display: none; }
  .pl-card { padding: 14px; }
  .cm-edit[open] { flex-basis: 100%; order: 10; }
  .cm-edit .pop-body { position: static; width: 100%; margin-top: 8px; }
}
"#;
