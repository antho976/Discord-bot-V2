use std::collections::{HashMap, HashSet};

use bot_core::axum::body::Bytes;
use bot_core::axum::extract::{DefaultBodyLimit, Path, Query, State};
use bot_core::axum::response::{IntoResponse, Json, Redirect};
use bot_core::axum::routing::{get, post};
use bot_core::axum::Router;
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::rusqlite::params;
use bot_core::twilight_model::id::Id;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, serde_json, text};
use serde::Deserialize;

use crate::{Board, KEY, Settings, curve, import};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/levels", get(members_page))
        .route("/g/{guild}/levels/settings", get(settings_page).post(save_settings))
        .route("/g/{guild}/levels/rewards", get(rewards_page).post(add_reward))
        .route("/g/{guild}/levels/rewards/{level}/delete", post(delete_reward))
        .route("/g/{guild}/levels/rewards/{level}/edit", post(edit_reward))
        .route("/g/{guild}/levels/members", get(|Path(guild): Path<u64>| async move { Redirect::to(&format!("/g/{guild}/levels")) }))
        .route("/g/{guild}/levels/members/{user}", post(set_member_xp))
        .route("/g/{guild}/levels/import", get(import_page).post(import_upload).layer(DefaultBodyLimit::max(64 * 1024 * 1024)))
        .route("/g/{guild}/levels/import-local", post(import_local))
}

fn tabs(page: &Page, active: &str) -> Markup {
    ui::tabs(
        &[
            ("Leaderboard", page.path("levels")),
            ("Rewards", page.path("levels/rewards")),
            ("Settings", page.path("levels/settings")),
            ("Import", page.path("levels/import")),
        ],
        active,
    )
}

async fn settings_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let custom = !s.custom_curve.is_empty();
    let first_levels: Vec<String> = (1..=5u32)
        .map(|l| text::num(curve::xp_to_reach(l, s.curve_base, s.curve_factor, &s.custom_curve)))
        .collect();
    // XP to reach levels 1 to 30, for the curve in the rail.
    let climb: Vec<i64> = (1..=CURVE_LEVELS).map(|l| curve::xp_to_reach(l, s.curve_base, s.curve_factor, &s.custom_curve)).collect();
    // The level-up preview celebrates whoever leads the board, at their next level.
    let leader = crate::top(&ctx, guild, 0, 1)?.into_iter().next();
    let (who, who_level) = match &leader {
        Some(m) => (ctx.display_name(guild, m.user_id), s.progress(m.xp).level + 1),
        None => (page.user.name.clone(), 5),
    };
    let channel = s
        .announce_channel
        .and_then(|c| ctx.text_channels(guild).into_iter().find(|x| x.0 == c))
        .map(|(_, n)| n)
        .unwrap_or_else(|| "general".into());
    let (bot_name, bot_avatar) = bot_face(&ctx);
    let now = page.user.local(bot_core::now_ms());
    let clock = format!("{}:{:02} {}", if now.hour() % 12 == 0 { 12 } else { now.hour() % 12 }, now.minute(), if now.hour() < 12 { "AM" } else { "PM" });

    let form = ui::settings_form(&page, &page.path("levels/settings"), html! {
            (section(Icon::Star, "Earning XP", None, ui::card(html! {
                (ui::field_number("xp_min", "XP per message, lowest", None, s.xp_min, 0, 1000, Some("XP")))
                (ui::field_number("xp_max", "XP per message, highest", Some("Each message earns a random amount between the two."), s.xp_max, 0, 1000, Some("XP")))
                (ui::field_number("cooldown_secs", "Cooldown", Some("Messages inside this window after one that earned XP earn nothing."), s.cooldown_secs, 0, 3600, Some("seconds")))
                (ui::field_number("voice_xp", "Voice XP", Some("Per minute in a voice channel with someone else. 0 turns it off."), s.voice_xp, 0, 1000, Some("XP / min")))
                (ui::field_toggle("voice_require_active", "Only unmuted voice counts", Some("Muted or deafened members earn no voice XP."), s.voice_require_active))
                (ui::field_number("reaction_xp", "Reaction XP", Some("For adding a reaction, with the same cooldown. 0 turns it off."), s.reaction_xp, 0, 1000, Some("XP")))
            })))
            (section(Icon::Chart, "Level curve", Some(&format!("Levels 1 to 5 start at {} XP.", first_levels.join(", "))), ui::card(html! {
                @if custom {
                    (ui::field_custom("Custom table", Some("This server uses a custom XP table carried over from the old bot. Switch it off to use the curve below."), html! {
                        label.switch { input type="checkbox" name="keep_custom" checked; span.track { span.thumb {} } }
                    }))
                }
                (ui::field_number("curve_base", "First level", Some("XP needed to go from level 0 to 1."), s.curve_base, 1, 100_000, Some("XP")))
                (ui::field_text("curve_factor", "Growth per level", Some("Each level costs this many times the one before. 1.2 means 20% more."), &s.curve_factor.to_string(), "1.2"))
            })))
            (section(Icon::Bell, "Level-up messages", None, ui::card(html! {
                (ui::field_toggle("announce", "Post level-ups", None, s.announce))
                (ui::field_channel(&ctx, guild, "announce_channel", "Channel", Some("None posts in the channel where they levelled up."), s.announce_channel))
                (ui::field_toggle("announce_ping", "Mention the member", None, s.announce_ping))
                (ui::field_number("announce_min_level", "Start at level", Some("Quieter: skip messages for the first few levels."), s.announce_min_level as i64, 0, 500, None))
                (ui::field_text("message", "Message", Some("{user} is the member, {level} the new level."), &s.message, crate::DEFAULT_MESSAGE))
            })))
            (section(Icon::Calendar, "Weekly board", Some("The board starts over every Monday at 00:00 UTC. The top three are kept for a week and shown on the leaderboard."), ui::card(html! {
                (ui::field_toggle("weekly_post", "Post the top three", Some("When the week ends, with how much XP each gained. Names show without a ping."), s.weekly_post))
                (ui::field_channel(&ctx, guild, "weekly_channel", "Channel", None, s.weekly_channel))
            })))
            (section(Icon::Trophy, "Prestige", None, ui::card(html! {
                (ui::field_number("prestige_level", "Prestige from level", Some("Members at this level or higher can use /prestige to start over at level 0 and rank above everyone with less prestige. 0 turns it off."), s.prestige_level as i64, 0, 500, None))
                (ui::field_number("prestige_bonus", "Bonus per prestige", Some("Extra XP from messages, voice and reactions, for each prestige."), s.prestige_bonus, 0, 100, Some("%")))
            })))
            (section(Icon::Activity, "Streaks", Some("A streak counts the days in a row, in UTC, a member earned XP. Streaks show on /rank and the leaderboard while the bonus is on."), ui::card(html! {
                (ui::field_number("streak_bonus", "Bonus per day", Some("Extra XP from messages, voice and reactions for each day in a row after the first. 0 turns the bonus off."), s.streak_bonus, 0, 100, Some("%")))
                (ui::field_number("streak_max", "Bonus cap", Some("The streak bonus stops growing here."), s.streak_max, 0, 500, Some("%")))
            })))
            (section(Icon::Close, "No XP in", None, ui::card(html! {
                (ui::field_channels(&ctx, guild, "no_xp_channels", "Text channels", Some("Threads in these channels earn nothing too."), &s.no_xp_channels))
                (ui::field_custom_wide("Voice channels", Some("Time in these earns nothing, and neither does their text chat."), chips("no_xp_voice", &voice_channels(&ctx, guild), &s.no_xp_channels)))
                (ui::field_custom_wide("Categories", Some("Every channel and thread in these earns nothing."), chips("no_xp_categories", &ctx.categories(guild), &s.no_xp_categories)))
                (ui::field_roles(&ctx, guild, "no_xp_roles", "Roles", Some("Members with any of these roles earn nothing."), &s.no_xp_roles))
                (ui::field_toggle("stack_rewards", "Keep earlier reward roles", Some("Off: members only keep the reward role for their highest level."), s.stack_rewards))
            })))
    });
    let max = climb.iter().copied().max().unwrap_or(1).max(1) as f64;
    let rail = html! {
        div.rail-block {
            h2 { "What members see" }
            div.lv-dc {
                div.lv-dc-head { (ui::icon(Icon::Hash)) span #lv-chan { (channel) } }
                div.lv-dc-msg.off[!s.announce] #lv-msg data-who=(who) data-level=(who_level) {
                    (bot_avatar)
                    div {
                        div.lv-dc-name { strong { (bot_name) } span.lv-dc-app { "APP" } span.lv-dc-time { "Today at " (clock) } }
                        div.lv-dc-text #lv-say {
                            @let template = if s.message.trim().is_empty() { crate::DEFAULT_MESSAGE } else { s.message.as_str() };
                            @for (i, part) in template.split("{user}").enumerate() {
                                @if i > 0 { @if s.announce_ping { span.lv-dc-mention { "@" (who) } } @else { (who) } }
                                (part.replace("{level}", &who_level.to_string()))
                            }
                        }
                    }
                }
            }
            p.muted.lv-dc-note #lv-note {
                @if s.announce { "Posted when a member reaches a new level" @if s.announce_min_level > 1 { ", from level " (s.announce_min_level) } "." }
                @else { "Level-ups aren't posted. Members still see their level with /rank." }
            }
        }
        div.rail-block.lv-curve {
            h2 { "The climb" }
            p.muted.small { "XP a member needs in all to reach each level." }
            div.mini-bars {
                svg #lv-curve viewBox={ "0 0 " (CURVE_LEVELS * 10 - 4) " 60" } preserveAspectRatio="none" role="img" aria-label="XP to reach levels 1 to 30" {
                    defs { linearGradient #lv-hi x1="0" y1="0" x2="0" y2="1" { stop offset="0" stop-color="var(--data)" {} stop offset="1" stop-color="var(--data-2)" {} } }
                    @for (i, xp) in climb.iter().enumerate() {
                        @let h = (*xp as f64 / max * 58.0).max(1.5);
                        rect.bar x=(i * 10) y=(60.0 - h) width="6" height=(h) rx="1.2" style={ "--i:" (i) }
                            fill=(if (i + 1) % 10 == 0 { "url(#lv-hi)" } else { "var(--data-dim)" }) data-tip={ "Level " (i + 1) ": " (text::num(*xp)) " XP" } {}
                    }
                }
            }
            div.axis { span { "Level 1" } span { "15" } span { "30" } }
            div.rail-nums {
                @for l in [10usize, 20, 30] {
                    div { b.mono #{ "lv-at-" (l) } { (compact(climb[l - 1])) } span { "Level " (l) } }
                }
            }
            @if custom { p.muted.small { "Drawn from the custom table carried over from the old bot." } }
        }
    };
    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, "Settings"))
        div.with-rail {
            div.stack { (form) }
            aside.rail { (rail) }
        }
        script { (PreEscaped(SETTINGS_JS)) }
    };
    Ok(ui::layout(&ctx, &page, "Levels", None, body).into_response())
}

/// Levels the settings rail draws the curve for.
const CURVE_LEVELS: u32 = 30;

/// The settings rail follows the form: the level-up message as it's typed, the curve as its
/// numbers change (not while a custom table is kept, which only the server can read).
const SETTINGS_JS: &str = r##"
(() => {
  const $ = (id) => document.getElementById(id);
  const msg = $("lv-msg"), say = $("lv-say");
  if (!msg) return;
  const text = $("message"), ping = $("announce_ping"), on = $("announce"), chan = $("announce_channel"), min = $("announce_min_level");
  const who = msg.dataset.who, level = msg.dataset.level;
  const drawMsg = () => {
    say.textContent = "";
    const tpl = text.value.trim() || text.placeholder;
    tpl.split("{user}").forEach((part, i) => {
      if (i > 0) {
        if (ping.checked) { const m = document.createElement("span"); m.className = "lv-dc-mention"; m.textContent = "@" + who; say.append(m); }
        else say.append(who);
      }
      say.append(part.split("{level}").join(level));
    });
    msg.classList.toggle("off", !on.checked);
    const c = chan.selectedOptions[0];
    $("lv-chan").textContent = c && c.value ? c.text.replace(/^#/, "") : "general";
    const from = Number(min.value) || 0;
    $("lv-note").textContent = on.checked ? "Posted when a member reaches a new level" + (from > 1 ? ", from level " + from : "") + "." : "Level-ups aren't posted. Members still see their level with /rank.";
  };
  [text, ping, on, chan, min].forEach((el) => { el.addEventListener("input", drawMsg); el.addEventListener("change", drawMsg); });
  const base = $("curve_base"), factor = $("curve_factor"), keep = document.querySelector('input[name="keep_custom"]');
  const bars = [...document.querySelectorAll("#lv-curve rect")];
  const fmt = new Intl.NumberFormat("en-US");
  const short = (n) => n < 1e4 ? fmt.format(n) : n < 1e5 ? (n / 1e3).toFixed(1) + "k" : n < 1e6 ? Math.floor(n / 1e3) + "k" : (n / 1e6).toFixed(1) + "M";
  const drawCurve = () => {
    if (keep && keep.checked) return;
    const b = Number(base.value), f = Number(factor.value);
    if (!(b >= 1) || !(f >= 1 && f <= 5)) return;
    let sum = 0;
    const xs = bars.map((_, l) => (sum += Math.max(1, Math.round(b * Math.pow(f, l)))));
    const max = Math.max(1, ...xs);
    bars.forEach((r, i) => {
      const h = Math.max(1.5, xs[i] / max * 58);
      r.setAttribute("y", 60 - h); r.setAttribute("height", h);
      r.dataset.tip = "Level " + (i + 1) + ": " + fmt.format(xs[i]) + " XP";
    });
    [10, 20, 30].forEach((l) => { const el = $("lv-at-" + l); if (el) el.textContent = short(xs[l - 1]); });
  };
  [base, factor].forEach((el) => el.addEventListener("input", drawCurve));
  keep?.addEventListener("change", drawCurve);
})();
"##;

/// The levels pages' own styles. Each place keeps its spot on the podium even when fewer than
/// three members have XP.
const CSS: &str = r#"
.podium .step.p1 { grid-column: 2; grid-row: 1; } .podium .step.p2 { grid-column: 1; grid-row: 1; } .podium .step.p3 { grid-column: 3; grid-row: 1; }
.lv-board { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; }
.lv-prestige { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
.big-num b { font: inherit; }
/* Room for "Level 17 · Prestige 1 · 27-day streak" on one line. */
@media (min-width: 821px) { .lb .rank { grid-template-columns: 28px 28px minmax(100px, 250px) minmax(0, 1fr) auto auto; } }

/* Section heads carry the group's icon tile. */
.lv-sec { gap: 14px; }
.lv-sec-head { display: flex; align-items: flex-start; gap: 12px; }
.lv-sec-ic { display: grid; place-items: center; width: 32px; height: 32px; border-radius: 10px; flex: none; color: var(--hue); background: color-mix(in oklab, var(--hue) 14%, transparent); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--hue) 24%, transparent); }
.lv-sec-ic .icon { width: 17px; height: 17px; }
.lv-sec-head h2 { padding-top: 6px; }
.lv-sec-head p { font-size: 13px; margin-top: 3px; max-width: 68ch; }

/* Rewards: the climb. A track runs down through each reward's level, drawn in on arrival. */
.lv-ladder { position: relative; display: flex; flex-direction: column; gap: 12px; }
.lv-ladder::before { content: ""; position: absolute; left: 27px; top: 34px; bottom: 34px; width: 2px; border-radius: 2px; background: linear-gradient(180deg, var(--hue), var(--data-2)); opacity: 0.6; transform-origin: top; animation: lv-draw 1.1s var(--ease) both 0.15s; }
.lv-rung { --role: var(--hue); position: relative; display: grid; grid-template-columns: 56px minmax(0, 1fr); gap: 16px; align-items: center; }
.lv-node { position: relative; z-index: 1; display: grid; place-content: center; justify-items: center; width: 56px; height: 56px; border-radius: 50%; background: radial-gradient(circle at 50% 28%, color-mix(in oklab, var(--role) 34%, var(--page)), var(--page) 78%); box-shadow: 0 0 0 2px color-mix(in oklab, var(--role) 75%, transparent), 0 0 0 6px var(--page), 0 12px 28px -10px var(--role); animation: pop-in 0.55s var(--spring) both; animation-delay: calc(0.3s + var(--i, 0) * 110ms); }
.lv-node b { font: 650 19px/1 var(--sans); letter-spacing: -0.03em; font-variant-numeric: tabular-nums; }
.lv-node small { font-size: 10px; color: var(--ink-2); margin-top: 3px; }
.lv-card { display: flex; flex-direction: column; gap: 10px; min-width: 0; padding: 14px 16px 15px; border-radius: 14px; background: linear-gradient(90deg, color-mix(in oklab, var(--role) 9%, transparent), rgba(243, 242, 248, 0.015) 70%); box-shadow: inset 0 0 0 1px var(--edge-1); transition: transform 0.35s var(--ease), box-shadow 0.25s; animation: rise 0.5s var(--ease) both; animation-delay: calc(0.36s + var(--i, 0) * 110ms); }
.lv-rung:hover .lv-card { transform: translateX(3px); box-shadow: inset 0 0 0 1px var(--edge-2), var(--shadow-lift); }
.lv-card-top { display: flex; align-items: center; gap: 8px 10px; flex-wrap: wrap; }
.lv-pill { display: inline-flex; align-items: center; height: 26px; padding: 0 10px; border-radius: 7px; font-weight: 600; font-size: 14px; color: color-mix(in oklab, var(--role), white 40%); background: color-mix(in oklab, var(--role) 17%, transparent); white-space: nowrap; }
.lv-card-top .muted { font-size: 12.5px; }
.lv-acts { display: flex; gap: 6px; margin-left: auto; }
.lv-acts .pop-body { width: 250px; }
.lv-acts .pop-body .stack { gap: 10px; }
.lv-who { display: flex; align-items: center; gap: 10px; font-size: 13px; color: var(--ink-2); flex-wrap: wrap; }
.lv-who .avatar { width: 26px; height: 26px; }
.lv-who b { color: var(--ink); font-weight: 600; }
.lv-card .meter { height: 5px; }
.lv-card .meter > span { background: linear-gradient(90deg, color-mix(in oklab, var(--role) 60%, var(--data-2)), color-mix(in oklab, var(--role), white 20%)); box-shadow: 0 0 10px color-mix(in oklab, var(--role) 50%, transparent); }
.lv-add .lv-node { background: var(--page); box-shadow: 0 0 0 6px var(--page); border: 1.5px dashed var(--edge-3); color: var(--ink-3); }
.lv-add .lv-node .icon { width: 18px; height: 18px; }
.lv-add .lv-card { background: none; box-shadow: inset 0 0 0 1px var(--edge-1); flex-direction: row; align-items: center; flex-wrap: wrap; }
.lv-add .lv-card .input.num { width: 96px; }
.lv-add .lv-card .select-wrap { flex: 1; min-width: 160px; max-width: 280px; }
.lv-warn { font-size: 12.5px; color: var(--ink-2); }
.lv-near .mini { padding: 4px 0; }
.lv-near .mini .mono { color: var(--data); }

/* Settings: the rail shows what members will see and how the curve climbs. */
.lv-dc { border-radius: 12px; overflow: hidden; background: #313338; color: #dbdee1; font-size: 14px; box-shadow: 0 18px 40px -20px rgba(0, 0, 0, 0.9), inset 0 0 0 1px rgba(255, 255, 255, 0.06); }
.lv-dc-head { display: flex; align-items: center; gap: 6px; height: 38px; padding: 0 12px; border-bottom: 1px solid #26272b; color: #f2f3f5; font-weight: 600; font-size: 13.5px; }
.lv-dc-head .icon { color: #80848e; width: 16px; height: 16px; }
.lv-dc-msg { display: grid; grid-template-columns: 40px minmax(0, 1fr); gap: 12px; padding: 12px 14px 16px; transition: opacity 0.3s, filter 0.3s; }
.lv-dc-msg.off { opacity: 0.4; filter: saturate(0.4); }
.lv-dc-av { width: 40px; height: 40px; border-radius: 50%; object-fit: cover; }
span.lv-dc-av { display: grid; place-items: center; background: linear-gradient(145deg, var(--accent-hi), var(--accent-lo)); }
span.lv-dc-av img { width: 22px; height: 22px; }
.lv-dc-name { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; line-height: 1.3; }
.lv-dc-name strong { color: #f2f3f5; font-weight: 600; font-size: 14.5px; }
.lv-dc-app { display: inline-flex; align-items: center; height: 15px; padding: 0 4px; border-radius: 3px; background: #5865f2; color: #fff; font: 600 10px/1 var(--sans); }
.lv-dc-time { color: #949ba4; font-size: 11.5px; }
.lv-dc-text { margin-top: 3px; line-height: 1.4; overflow-wrap: anywhere; }
.lv-dc-mention { background: rgba(88, 101, 242, 0.3); color: #c9cdfb; border-radius: 3px; padding: 0 2px; font-weight: 500; }
.lv-dc-note { font-size: 12px; }
.lv-curve .mini-bars svg { height: 92px; }
.lv-curve .axis { display: flex; justify-content: space-between; color: var(--ink-3); font: 11px var(--mono); margin-top: 6px; }
.lv-curve .rail-nums b { font-size: 21px; }

/* Import: a drop zone that takes the whole card. */
.lv-drop { position: relative; display: flex; flex-direction: column; align-items: center; gap: 8px; padding: 36px 20px 30px; border-radius: 16px; text-align: center; border: 1.5px dashed var(--edge-3); background: radial-gradient(70% 120% at 50% 0%, color-mix(in oklab, var(--hue) 11%, transparent), transparent 70%); transition: border-color 0.2s, background 0.3s, transform 0.3s var(--ease); }
.lv-drop:hover, .lv-drop.over { border-color: var(--hue); background: radial-gradient(80% 130% at 50% 0%, color-mix(in oklab, var(--hue) 20%, transparent), transparent 75%); }
.lv-drop.over { transform: scale(1.01); }
.lv-drop:has(input:focus-visible) { outline: 2px solid var(--focus); outline-offset: 3px; }
.lv-drop input { position: absolute; inset: 0; width: 100%; height: 100%; opacity: 0; cursor: pointer; }
.lv-drop-ic { display: grid; place-items: center; width: 54px; height: 54px; margin-bottom: 6px; border-radius: 17px; color: #0b0b10; background: linear-gradient(150deg, color-mix(in oklab, var(--hue), white 25%), var(--hue) 55%, color-mix(in oklab, var(--hue), black 25%)); box-shadow: 0 14px 30px -12px var(--hue), inset 0 1px 0 rgba(255, 255, 255, 0.35); animation: float 5s ease-in-out infinite; }
.lv-drop-ic .icon { width: 26px; height: 26px; }
.lv-drop strong { font-size: 15px; font-weight: 600; }
.lv-drop .muted { font-size: 12.5px; }
.lv-drop .lv-file { display: none; align-items: center; gap: 8px; margin-top: 6px; padding: 6px 12px; border-radius: 10px; background: var(--control); font-size: 12.5px; color: var(--ink); }
.lv-drop.picked .lv-file { display: inline-flex; animation: pop-in 0.4s var(--spring) both; }
.lv-go { display: flex; align-items: center; justify-content: space-between; gap: 12px; flex-wrap: wrap; margin-top: 14px; }
.lv-result { display: flex; align-items: flex-start; gap: 12px; padding: 14px 16px; border-radius: 14px; font-size: 13.5px; background: var(--well); box-shadow: inset 0 0 0 1px var(--edge-1); animation: rise 0.45s var(--ease) both; }
.lv-result[hidden] { display: none; }
.lv-result .lv-res-ic { display: grid; place-items: center; width: 26px; height: 26px; border-radius: 50%; flex: none; background: var(--control); color: var(--ink-2); }
.lv-result.ok { background: color-mix(in oklab, var(--live) 9%, transparent); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--live) 28%, transparent); }
.lv-result.ok .lv-res-ic { background: var(--live); color: #062014; }
.lv-result.bad { background: color-mix(in oklab, var(--held) 9%, transparent); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--held) 28%, transparent); }
.lv-result.bad .lv-res-ic { background: var(--held); color: #1a0507; }
.lv-local { display: flex; align-items: center; gap: 16px; flex-wrap: wrap; padding: 16px 18px; }
.lv-local .grow p { font-size: 12.5px; margin-top: 2px; overflow-wrap: anywhere; }
.lv-local code { color: var(--ink-2); }
.lv-main { gap: 30px; }
.lv-steps { display: flex; flex-direction: column; gap: 12px; }
.lv-steps .mini { align-items: flex-start; }
.lv-steps .mini .lvl { flex: none; }
@keyframes lv-draw { from { transform: scaleY(0); } }
@media (max-width: 920px) {
  .lv-ladder::before { left: 21px; }
  .lv-rung { grid-template-columns: 44px minmax(0, 1fr); gap: 12px; }
  .lv-node { width: 44px; height: 44px; }
  .lv-node b { font-size: 16px; }
  .lv-node small { display: none; }
  .lv-acts { margin-left: 0; }
}
"#;

/// A section headed by an icon tile in the group's hue.
fn section(ic: Icon, title: &str, hint: Option<&str>, body: Markup) -> Markup {
    html! {
        section.block.lv-sec {
            div.lv-sec-head {
                span.lv-sec-ic { (ui::icon(ic)) }
                div { h2 { (title) } @if let Some(h) = hint { p.muted { (h) } } }
            }
            (body)
        }
    }
}

/// Reading cards whose figures count up as they scroll into view.
fn readings(items: &[(&str, String)]) -> Markup {
    html! { div.readings { @for (label, value) in items { div.reading { b data-count { (value) } span { (label) } } } } }
}

/// "740", "25.9k", "1.2M": XP short enough for a rail figure.
fn compact(n: i64) -> String {
    match n {
        ..10_000 => text::num(n),
        10_000..100_000 => format!("{:.1}k", n as f64 / 1000.0),
        100_000..1_000_000 => format!("{}k", n / 1000),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

/// The bot as members see it in a channel: its name, and its avatar or the dashboard's mark
/// when the bot isn't connected.
fn bot_face(ctx: &Ctx) -> (String, Markup) {
    let user = ctx.bot_id().and_then(|b| ctx.cache.user(Id::new(b)).map(|u| (b, u)));
    match user {
        Some((id, u)) => (
            u.global_name.clone().unwrap_or_else(|| u.name.clone()),
            html! { img.lv-dc-av src=(bot_core::discord::avatar_url(id, u.avatar.map(|a| a.to_string()).as_deref())) alt=""; },
        ),
        None => ("Bot".into(), html! { span.lv-dc-av { img src="/static/icon.svg" alt=""; } }),
    }
}

/// An upload arrow over a tray, drawn on the dashboard's 24-unit grid.
fn upload_icon() -> Markup {
    html! {
        svg.icon viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" {
            path d="M12 15V4M7.5 8.5 12 4l4.5 4.5M4.5 14.5v3A2.5 2.5 0 0 0 7 20h10a2.5 2.5 0 0 0 2.5-2.5v-3" {}
        }
    }
}

/// The server's voice and stage channels, in Discord's order.
fn voice_channels(ctx: &Ctx, guild: u64) -> Vec<(u64, String)> {
    use bot_core::twilight_model::channel::ChannelType;
    let Some(ids) = ctx.cache.guild_channels(Id::new(guild)) else { return Vec::new() };
    let mut out: Vec<(i32, u64, String)> = ids
        .iter()
        .filter_map(|id| ctx.cache.channel(*id))
        .filter(|c| matches!(c.kind, ChannelType::GuildVoice | ChannelType::GuildStageVoice))
        .map(|c| (c.position.unwrap_or_default(), c.id.get(), c.name.clone().unwrap_or_default()))
        .collect();
    out.sort();
    out.into_iter().map(|(_, id, name)| (id, name)).collect()
}

/// Switch-on chips for picking any number of channels or categories, like `ui::field_channels`.
fn chips(name: &str, items: &[(u64, String)], selected: &[u64]) -> Markup {
    html! {
        div.chips id=(name) {
            @if items.is_empty() { span.muted.small { "Nothing to pick from yet." } }
            @for (id, label) in items {
                label.chip {
                    input type="checkbox" name=(name) value=(id) checked[selected.contains(id)];
                    span { (label) }
                }
            }
        }
    }
}

fn role_name(ctx: &Ctx, guild: u64, role: u64) -> String {
    ctx.roles(guild).into_iter().find(|(id, _, _)| *id == role).map(|(_, n, _)| format!("@{n}")).unwrap_or_else(|| "Deleted role".into())
}

async fn save_settings(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let old = ctx.settings::<Settings>(guild, KEY)?;
    let xp_min = form.int_in("xp_min", 0, 1000, 15);
    let s = Settings {
        xp_min,
        xp_max: form.int_in("xp_max", 0, 1000, 25).max(xp_min),
        cooldown_secs: form.int_in("cooldown_secs", 0, 3600, 60),
        voice_xp: form.int_in("voice_xp", 0, 1000, 5),
        voice_require_active: form.bool("voice_require_active"),
        reaction_xp: form.int_in("reaction_xp", 0, 1000, 0),
        curve_base: form.int_in("curve_base", 1, 100_000, 100),
        curve_factor: form.f64("curve_factor").filter(|f| (1.0..=5.0).contains(f)).unwrap_or(1.2),
        custom_curve: if form.bool("keep_custom") { old.custom_curve.clone() } else { Default::default() },
        announce: form.bool("announce"),
        announce_channel: form.channel(&ctx, guild, "announce_channel"),
        announce_ping: form.bool("announce_ping"),
        announce_min_level: form.int_in("announce_min_level", 0, 500, 0) as u32,
        message: { let m = form.string("message"); if m.is_empty() { crate::DEFAULT_MESSAGE.into() } else { text::clip(&m, 500) } },
        stack_rewards: form.bool("stack_rewards"),
        no_xp_channels: form.channels(&ctx, guild, "no_xp_channels").into_iter().chain(form.channels(&ctx, guild, "no_xp_voice")).collect(),
        no_xp_categories: form.channels(&ctx, guild, "no_xp_categories"),
        no_xp_roles: form.roles(&ctx, guild, "no_xp_roles"),
        weekly_post: form.bool("weekly_post"),
        weekly_channel: form.channel(&ctx, guild, "weekly_channel"),
        prestige_level: form.int_in("prestige_level", 0, 500, 0) as u32,
        prestige_bonus: form.int_in("prestige_bonus", 0, 100, 5),
        streak_bonus: form.int_in("streak_bonus", 0, 100, 0),
        streak_max: form.int_in("streak_max", 0, 500, 20),
    };
    let curve_changed = s.curve_base != old.curve_base || s.curve_factor != old.curve_factor || s.custom_curve != old.custom_curve;
    ctx.save_settings(guild, KEY, &s)?;
    if curve_changed {
        recompute_levels(&ctx, guild, &s)?;
    }
    // /prestige is registered only where prestige is on.
    // Saving again while prestige is on retries a registration that failed; names Discord
    // already has don't count toward its daily limit.
    if (s.prestige_on() || old.prestige_on()) && ctx.enabled(guild, KEY) && !ctx.cfg.preview {
        let ctx = ctx.clone();
        bot_core::tokio::spawn(async move {
            if let Err(e) = bot_core::discord::register_commands(&ctx, guild).await {
                bot_core::tracing::warn!(guild, error = %e, "couldn't update commands");
                let why = bot_core::discord::why_not_registered(&e);
                ctx.notify_once(guild, KEY, "register", format!("/prestige didn't reach Discord. {why} Save the settings again to retry."), Some(format!("/g/{guild}/levels/settings")), std::time::Duration::from_secs(3600));
            }
        });
    }
    ctx.audit(&page.user, Some(guild), KEY, "Changed XP settings");
    Ok(saved(&page.path("levels/settings")))
}

/// After a curve change, every stored level is recomputed from XP. Roles catch up on the
/// member's next XP gain.
fn recompute_levels(ctx: &Ctx, guild: u64, s: &Settings) -> bot_core::anyhow::Result<()> {
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    let rows: Vec<(i64, i64)> = {
        let mut st = tx.prepare("SELECT user_id, xp FROM levels WHERE guild_id = ?1")?;
        st.query_map([guild as i64], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
    };
    for (user, xp) in rows {
        tx.execute("UPDATE levels SET level = ?3 WHERE guild_id = ?1 AND user_id = ?2", params![guild as i64, user, s.progress(xp).level as i64])?;
    }
    tx.commit()?;
    Ok(())
}

/// Why the bot can't give a reward role, or None when it can.
fn cant_give(ctx: &Ctx, guild: u64, role: u64) -> Option<&'static str> {
    let Some(r) = ctx.cache.role(Id::new(role)) else { return Some("Deleted role") };
    if r.managed {
        return Some("Managed by an integration");
    }
    let me = ctx.bot_id().and_then(|bot| ctx.cache.member(Id::new(guild), Id::new(bot)))?;
    let top = me.roles().iter().filter_map(|id| ctx.cache.role(*id).map(|r| r.position)).max().unwrap_or(0);
    (r.position >= top).then_some("Above the bot's role")
}

/// A role's colour for its dot; roles without one get the caption grey.
fn role_color(color: u32) -> String {
    if color == 0 { "var(--ink-3)".to_owned() } else { format!("#{color:06x}") }
}

fn role_options(ctx: &Ctx, guild: u64) -> Vec<(String, String)> {
    ctx.roles(guild).into_iter().map(|(id, n, _)| (id.to_string(), format!("@{n}"))).collect()
}

/// Change a reward's level or role where it stands.
async fn edit_reward(State(ctx): State<Ctx>, user: User, Path((guild, level)): Path<(u64, i64)>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let new_level = form.i64("level").filter(|l| (1..=500).contains(l)).ok_or_else(|| WebError::Bad("Pick a level from 1 to 500.".into()))?;
    // Each row's select has its own name, so the page has no duplicate ids.
    let role = form.role(&ctx, guild, &format!("role-{level}")).ok_or_else(|| WebError::Bad("Pick a role.".into()))?;
    let old_role = crate::rewards(&ctx, guild)?.into_iter().find(|(l, _)| *l as i64 == level).map(|(_, r)| r).ok_or(WebError::NotFound)?;
    if role != old_role && !page.user.admin && !ctx.safe_role(guild, role) {
        return Err(WebError::Bad("That role has moderation or admin permissions, or sits above the bot's role. Only a dashboard admin can make it a reward.".into()));
    }
    let moved = {
        let mut conn = ctx.db.lock();
        let tx = conn.transaction().map_err(|e| WebError::Internal(e.into()))?;
        let taken = new_level != level
            && tx
                .query_row("SELECT 1 FROM level_rewards WHERE guild_id = ?1 AND level = ?2", params![guild as i64, new_level], |_| Ok(()))
                .is_ok();
        if !taken {
            tx.execute(
                "UPDATE level_rewards SET level = ?3, role_id = ?4 WHERE guild_id = ?1 AND level = ?2",
                params![guild as i64, level, new_level, role as i64],
            )
            .and_then(|_| tx.commit())
            .map_err(|e| WebError::Internal(e.into()))?;
        }
        !taken
    };
    if !moved {
        return Err(WebError::Bad(format!("Level {new_level} already has a reward. Change or remove that one first.")));
    }
    crate::resync_rewards(&ctx, guild, level.min(new_level) as u32, (role != old_role).then_some(old_role));
    ctx.audit(&page.user, Some(guild), KEY, format!("Changed the level {level} reward to {} at level {new_level}", role_name(&ctx, guild, role)));
    Ok(saved(&page.path("levels/rewards")))
}

async fn add_reward(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let level = form.i64("level").filter(|l| (1..=500).contains(l)).ok_or_else(|| WebError::Bad("Pick a level from 1 to 500.".into()))?;
    let role = form.role(&ctx, guild, "role").ok_or_else(|| WebError::Bad("Pick a role.".into()))?;
    if !page.user.admin && !ctx.safe_role(guild, role) {
        return Err(WebError::Bad("That role has moderation or admin permissions, or sits above the bot's role. Only a dashboard admin can make it a reward.".into()));
    }
    // Adding at a level that has a reward replaces its role.
    let replaced = crate::rewards(&ctx, guild)?.into_iter().find(|(l, r)| *l as i64 == level && *r != role).map(|(_, r)| r);
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO level_rewards (guild_id, level, role_id) VALUES (?1, ?2, ?3)
             ON CONFLICT (guild_id, level) DO UPDATE SET role_id = excluded.role_id",
            params![guild as i64, level, role as i64],
        )
    })?;
    crate::resync_rewards(&ctx, guild, level as u32, replaced);
    ctx.audit(&page.user, Some(guild), KEY, format!("Set the level {level} reward to {}", role_name(&ctx, guild, role)));
    Ok(saved(&page.path("levels/rewards")))
}

async fn delete_reward(State(ctx): State<Ctx>, user: User, Path((guild, level)): Path<(u64, i64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    ctx.db.with(|c| c.execute("DELETE FROM level_rewards WHERE guild_id = ?1 AND level = ?2", params![guild as i64, level]))?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Removed the level {level} reward"));
    Ok(saved(&page.path("levels/rewards")))
}

#[derive(Deserialize)]
struct MembersQuery {
    q: Option<String>,
    page: Option<i64>,
    board: Option<String>,
}

/// Name and avatar of members Discord found for a search, who the bot's cache doesn't hold.
type Found = HashMap<u64, (String, String)>;

/// Members whose name contains `needle`, or whose id it is. The cache only holds members the bot
/// has seen lately, so on a big server most are missing: Discord's member search fills the gap
/// (it matches the start of a username or nickname) and brings their name and avatar along.
async fn find_members(ctx: &Ctx, guild: u64, needle: &str) -> (HashSet<u64>, Found) {
    let lower = needle.to_lowercase();
    let mut ids: HashSet<u64> = ctx
        .cache
        .guild_members(Id::new(guild))
        .map(|set| set.iter().map(|u| u.get()).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|u| ctx.display_name(guild, *u).to_lowercase().contains(&lower))
        .collect();
    let mut found = Found::new();
    if let Ok(id) = needle.parse::<u64>() {
        ids.insert(id);
        return (ids, found);
    }
    let members = match ctx.http.search_guild_members(Id::new(guild), needle).limit(1000).await {
        Ok(res) => res.models().await.unwrap_or_default(),
        Err(e) => {
            bot_core::tracing::debug!(guild, error = %e, "member search failed");
            Vec::new()
        }
    };
    for m in members {
        let id = m.user.id.get();
        if ids.insert(id) {
            let name = m.nick.clone().or_else(|| m.user.global_name.clone()).unwrap_or_else(|| m.user.name.clone());
            let avatar = match m.avatar {
                Some(a) => format!("https://cdn.discordapp.com/guilds/{guild}/users/{id}/avatars/{a}.png?size=128"),
                None => bot_core::discord::avatar_url(id, m.user.avatar.map(|a| a.to_string()).as_deref()),
            };
            found.insert(id, (name, avatar));
        }
    }
    (ids, found)
}

async fn members_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<MembersQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let per = 25;
    let board = if q.board.as_deref() == Some("week") { Board::Week } else { Board::All };
    // Search is for finding someone's XP; the weekly board is short enough to read.
    let search = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty() && board == Board::All);
    let week = crate::week_totals(&ctx, guild)?;
    let total = match board {
        Board::All => crate::count(&ctx, guild)?,
        Board::Week => week.0,
    };
    // The podium takes the first three; the pages of the list start after it.
    let pages = ((total - 3).max(0) + per - 1) / per;
    let n = q.page.unwrap_or(0).clamp(0, (pages - 1).max(0));
    let start = 3 + n * per;
    // Each row: place, member, and XP gained this week on the weekly board.
    type Row = (i64, crate::Member, Option<i64>);
    let ranked = |offset: i64, limit: i64| -> bot_core::anyhow::Result<Vec<Row>> {
        Ok(match board {
            Board::All => crate::top(&ctx, guild, offset, limit)?.into_iter().enumerate().map(|(i, m)| (offset + i as i64 + 1, m, None)).collect(),
            Board::Week => crate::week_top(&ctx, guild, offset, limit)?.into_iter().enumerate().map(|(i, (m, g))| (offset + i as i64 + 1, m, Some(g))).collect(),
        })
    };
    let podium = if search.is_none() && n == 0 { ranked(0, 3)? } else { Vec::new() };
    let (rows, found, matched): (Vec<Row>, Found, i64) = match search {
        Some(needle) => {
            let (ids, found) = find_members(&ctx, guild, needle).await;
            let (rows, matched) = crate::find(&ctx, guild, &ids, per as usize)?;
            (rows.into_iter().map(|(p, m)| (p, m, None)).collect(), found, matched)
        }
        None => (ranked(start, per)?, Found::new(), 0),
    };
    // The weekly meters measure against first place.
    let leader = match board {
        Board::Week if n == 0 => podium.first().and_then(|r| r.2),
        Board::Week => crate::week_top(&ctx, guild, 0, 1)?.first().map(|r| r.1),
        Board::All => None,
    }
    .unwrap_or(1)
    .max(1);
    let rewards = crate::rewards(&ctx, guild)?;
    let roles = ctx.roles(guild);
    let at_least = |level: u32| -> i64 {
        ctx.db
            .with(|c| c.query_row("SELECT COUNT(*) FROM levels WHERE guild_id = ?1 AND level >= ?2", params![guild as i64, level as i64], |r| r.get(0)))
            .unwrap_or(0)
    };
    let name = |u: u64| found.get(&u).map(|f| f.0.clone()).unwrap_or_else(|| ctx.display_name(guild, u));
    let avatar = |u: u64| found.get(&u).map(|f| f.1.clone()).unwrap_or_else(|| ctx.member_avatar(guild, u));
    let line = |m: &crate::Member, gain: Option<i64>| match gain {
        Some(g) => format!("+{} XP this week", text::num(g)),
        None => format!("Level {} · {} XP", s.progress(m.xp).level, text::num(m.xp)),
    };
    // Prestige and streak, for the caption under a name.
    let extras = |m: &crate::Member| -> String {
        let mut parts = Vec::new();
        if s.prestige_on() && m.prestige > 0 {
            parts.push(format!("Prestige {}", m.prestige));
        }
        if s.streaks_on() && m.streak > 1 {
            parts.push(format!("{}-day streak", m.streak));
        }
        parts.join(" · ")
    };
    let earning = {
        let mut parts = vec![format!("{} to {} per message, once every {}.", s.xp_min, s.xp_max, text::duration(s.cooldown_secs))];
        if s.voice_xp > 0 {
            parts.push(format!("{} per minute in voice with someone else{}.", s.voice_xp, if s.voice_require_active { ", unmuted" } else { "" }));
        }
        if s.reaction_xp > 0 {
            parts.push(format!("{} for a reaction.", s.reaction_xp));
        }
        if s.streaks_on() {
            parts.push(format!("Each day in a row with XP adds {}% more, up to {}%.", s.streak_bonus, s.streak_max));
        }
        if s.prestige_on() {
            parts.push(if s.prestige_bonus > 0 {
                format!("From level {} members can prestige: they start over at level 0 and earn {}% more for each prestige.", s.prestige_level, s.prestige_bonus)
            } else {
                format!("From level {} members can prestige and start over at level 0.", s.prestige_level)
            });
        }
        parts.join(" ")
    };
    let board_q = if board == Board::Week { "board=week&" } else { "" };

    let main = html! {
        style { (PreEscaped(CSS)) }
        div.lv-board {
            nav.seg aria-label="Board" {
                a.on[board == Board::All] href=(page.path("levels")) aria-current=[(board == Board::All).then_some("true")] { "All time" }
                a.on[board == Board::Week] href={ (page.path("levels")) "?board=week" } aria-current=[(board == Board::Week).then_some("true")] { "This week" }
            }
            @if board == Board::Week { span.muted.small { "XP gained since Monday 00:00 UTC" } }
        }
        @if !podium.is_empty() {
            div.podium {
                @for (place, i) in [(2usize, 1usize), (1, 0), (3, 2)] {
                    @if let Some((_, m, gain)) = podium.get(i) {
                        div class={ "step p" (place) } {
                            img.avatar src=(ctx.member_avatar(guild, m.user_id)) alt="" loading="lazy";
                            strong { (name(m.user_id)) }
                            @let more = extras(m);
                            @if !more.is_empty() { span.muted.small { (more) } }
                            span.muted.small { (line(m, *gain)) }
                            div.block { (place) }
                        }
                    }
                }
            }
        }
        div.list-head {
            h2 { @if podium.is_empty() { "Members" } @else { "Everyone else" } }
            @if board == Board::All {
                form method="get" action=(page.path("levels")) {
                    input.input.search type="search" name="q" value=(search.unwrap_or_default()) placeholder="Search members";
                }
            }
        }
        @if rows.is_empty() && podium.is_empty() {
            @if search.is_some() {
                (ui::empty(Icon::Users, "No match", "Nobody with XP matches that name or ID."))
            } @else if board == Board::Week {
                (ui::empty(Icon::Trophy, "Nobody has earned XP this week", "The board starts over every Monday at 00:00 UTC and fills as members chat."))
            } @else {
                (ui::empty(Icon::Trophy, "Nobody has XP yet", "Members show up here once they chat, or after you import XP from the old bot."))
            }
        } @else if rows.is_empty() && search.is_none() {
            p.muted.small { @if board == Board::Week { "Everyone who earned XP this week is on the podium so far." } @else { "Everyone with XP is on the podium so far." } }
        } @else {
            div.ranks {
                @for (place, m, gain) in &rows {
                    @let p = s.progress(m.xp);
                    div.rank {
                        span.mono.muted.pos { (place) }
                        img.avatar src=(avatar(m.user_id)) alt="" width="28" height="28" loading="lazy";
                        @let more = extras(m);
                        div.who { strong { (name(m.user_id)) } span.muted.small { "Level " (p.level) @if !more.is_empty() { " · " (more) } } }
                        @if let Some(g) = gain {
                            @let pct = (g * 100 / leader).clamp(0, 100);
                            div.progress { span.meter { span style={ "width:" (pct) "%" } {} } span.muted.small { (pct) "% of first place" } }
                            span.mono.xp { "+" (text::num(*g)) " XP" }
                        } @else {
                            @let pct = if p.needed > 0 { (p.into * 100 / p.needed).clamp(0, 100) } else { 0 };
                            div.progress { span.meter { span style={ "width:" (pct) "%" } {} } span.muted.small { (pct) "% to level " (p.level + 1) } }
                            span.mono.xp { (text::num(m.xp)) " XP" }
                            @if page.can_edit() {
                                details.pop.set-xp {
                                    summary.btn.small.outline { "Set XP" }
                                    div.pop-body {
                                        form.stack method="post" action=(page.path(&format!("levels/members/{}", m.user_id))) {
                                            div.inline-form {
                                                input.input.num type="number" name="xp" value=(m.xp) min="0" aria-label="XP";
                                                button.btn.primary type="submit" { "Set" }
                                            }
                                            @if s.prestige_on() {
                                                label.lv-prestige { span.muted.small { "Prestige" } input.input.num type="number" name="prestige" value=(m.prestige) min="0" max="1000"; }
                                            }
                                        }
                                        p.muted.small { "Their level and reward roles follow the new XP." }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            @if matched > rows.len() as i64 {
                p.muted.small { "Showing the top " (rows.len()) " of " (text::num(matched)) " matches. Type more of the name to narrow it." }
            }
            @if search.is_none() && pages > 1 {
                div.inline-form {
                    @if n > 0 { a.btn.small.outline href={ (page.path("levels")) "?" (board_q) "page=" (n - 1) } { "Previous" } }
                    span.muted.small { "Page " (n + 1) " of " (pages) }
                    @if n + 1 < pages { a.btn.small.outline href={ (page.path("levels")) "?" (board_q) "page=" (n + 1) } { "Next" } }
                }
            }
        }
    };
    let week_block = html! {
        div.rail-block {
            h2 { "XP earned this week" }
            div.big-num { b data-count { (text::num(week.1)) } }
            span.muted.small { "by " (text::num(week.0)) @if week.0 == 1 { " member" } @else { " members" } " since Monday" }
        }
    };
    let rail = match board {
        Board::All => html! {
            div.rail-block {
                h2 { "Reward roles" }
                @if rewards.is_empty() {
                    p.muted.small { "None yet. " a href=(page.path("levels/rewards")) { "Add one" } " and members get it at that level." }
                }
                @for (level, role) in &rewards {
                    @let r = roles.iter().find(|x| x.0 == *role);
                    div.mini {
                        span.lvl { (level) }
                        span.dot style={ "background:" (r.map(|r| role_color(r.2)).unwrap_or_else(|| "var(--ink-3)".into())) } {}
                        div.grow { span { (r.map(|r| r.1.clone()).unwrap_or_else(|| "Deleted role".into())) } }
                        span.mono { (text::num(at_least(*level))) }
                    }
                }
            }
            (week_block)
            div.rail-block {
                h2 { "How XP is earned" }
                p.muted.small { (earning) }
            }
        },
        Board::Week => {
            let last = crate::last_week(&ctx, guild)?;
            html! {
                (week_block)
                div.rail-block {
                    h2 { "Last week" }
                    @match &last {
                        Some((_, winners)) => {
                            @for (i, (u, gain)) in winners.iter().enumerate() {
                                div.mini {
                                    span.lvl { (i + 1) }
                                    img.avatar src=(ctx.member_avatar(guild, *u)) alt="" width="24" height="24" loading="lazy";
                                    div.grow { span { (ctx.display_name(guild, *u)) } }
                                    span.mono { "+" (text::num(*gain)) }
                                }
                            }
                        },
                        None => { p.muted.small { "The top three show here once this week ends." } },
                    }
                    p.muted.small {
                        @match s.weekly_channel.filter(|_| s.weekly_post).and_then(|c| ctx.text_channels(guild).into_iter().find(|x| x.0 == c)) {
                            Some((_, ch)) => { "Posted to #" (ch) " when each week ends." },
                            None => { "Not posted anywhere. " @if page.can_edit() { a href=(page.path("levels/settings")) { "Pick a channel" } " to post them when each week ends." } },
                        }
                    }
                }
            }
        }
    };
    let body = html! {
        (tabs(&page, "Leaderboard"))
        div.with-rail {
            div.stack.lb { (main) }
            aside.rail { (rail) }
        }
    };
    Ok(ui::layout(&ctx, &page, "Levels", None, body).into_response())
}

async fn rewards_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let rewards = crate::rewards(&ctx, guild)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let roles = ctx.roles(guild);
    let options = role_options(&ctx, guild);
    let ranked = crate::count(&ctx, guild)?;
    // Everyone on the board, best first: (member, level).
    let board: Vec<(u64, u32)> = crate::top(&ctx, guild, 0, 2000)?.into_iter().map(|m| (m.user_id, s.progress(m.xp).level)).collect();
    // Members at or above a level, counted in the database, and the faces of the first five.
    let reached = |level: u32| -> (i64, Vec<String>) {
        let n = ctx
            .db
            .with(|c| c.query_row("SELECT COUNT(*) FROM levels WHERE guild_id = ?1 AND level >= ?2", params![guild as i64, level as i64], |r| r.get::<_, i64>(0)))
            .unwrap_or(0);
        (n, board.iter().filter(|(_, l)| *l >= level).take(5).map(|(u, _)| ctx.member_avatar(guild, *u)).collect())
    };
    let holding = rewards.first().map(|(l, _)| reached(*l).0).unwrap_or(0);
    // Close to their next reward: within three levels of it, nearest first.
    let mut near: Vec<(u64, u32, u32, u64)> = board
        .iter()
        .filter_map(|(u, l)| rewards.iter().find(|(rl, _)| rl > l).map(|(rl, r)| (*u, *l, *rl, *r)))
        .filter(|(_, l, rl, _)| rl - l <= 3)
        .collect();
    near.sort_by_key(|(_, l, rl, _)| rl - l);
    near.truncate(4);
    // Members per band of levels, for the rail's chart.
    let top_level = board.iter().map(|(_, l)| *l).max().unwrap_or(0);
    let band = (top_level / 10 + 1).max(1);
    let spread: Vec<(String, Option<i64>)> = (0..=top_level / band)
        .map(|b| {
            let (lo, hi) = (b * band, b * band + band - 1);
            let label = if band == 1 { format!("Level {lo}") } else { format!("Levels {lo}–{hi}") };
            (label, Some(board.iter().filter(|(_, l)| (lo..=hi).contains(l)).count() as i64))
        })
        .collect();
    let color = |role: u64| roles.iter().find(|r| r.0 == role).map(|r| role_color(r.2)).unwrap_or_else(|| "var(--ink-3)".into());

    let main = html! {
        (readings(&[
            ("reward roles", text::num(rewards.len() as i64)),
            ("members holding one", text::num(holding)),
            ("highest reward", rewards.last().map(|(l, _)| format!("Level {l}")).unwrap_or_else(|| "None".into())),
        ]))
        (section(Icon::Trophy, "The climb", Some(if s.stack_rewards {
            "Given when a member reaches the level, and to members already past it. Members keep earlier rewards. Removing a reward leaves the role on whoever has it."
        } else {
            "Given when a member reaches the level, and to members already past it. Members keep only the reward for their highest level. Removing a reward leaves the role on whoever has it."
        }), html! {
            @if rewards.is_empty() && !page.can_edit() {
                (ui::empty(Icon::Trophy, "No reward roles", "Roles staff add here are given to members when they reach the level."))
            } @else {
                div.lv-ladder {
                    @for (i, (level, role)) in rewards.iter().enumerate() {
                        @let known = roles.iter().any(|r| r.0 == *role);
                        @let problem = if known { cant_give(&ctx, guild, *role) } else { Some("Deleted role") };
                        @let (n, faces) = reached(*level);
                        @let pct = if ranked > 0 { (n * 100 / ranked).clamp(0, 100) } else { 0 };
                        div.lv-rung style={ "--i:" (i) ";--role:" (color(*role)) } {
                            span.lv-node data-tip={ "Level " (level) } { b { (level) } small { "level" } }
                            div.lv-card {
                                div.lv-card-top {
                                    span.lv-pill { (role_name(&ctx, guild, *role)) }
                                    @if let Some(why) = problem { (ui::badge("Can't give", Tone::Warn)) span.lv-warn { (why) } }
                                    @if page.can_edit() {
                                        div.lv-acts {
                                            details.pop {
                                                summary.btn.small.outline { "Change" }
                                                div.pop-body {
                                                    form.stack method="post" action=(page.path(&format!("levels/rewards/{level}/edit"))) {
                                                        input.input.num type="number" name="level" min="1" max="500" value=(level) aria-label="Level" required;
                                                        (ui::select(&format!("role-{level}"), &options, &role.to_string()))
                                                        button.btn.primary type="submit" { "Save" }
                                                    }
                                                }
                                            }
                                            (ui::action_button(&page.path(&format!("levels/rewards/{level}/delete")), "Remove", Tone::Bad, true))
                                        }
                                    }
                                }
                                div.lv-who {
                                    @if !faces.is_empty() { (ui::avatars(&faces, 5)) }
                                    span {
                                        @if n == 0 { "Nobody has reached it yet" }
                                        @else { b { (text::num(n)) } (if n == 1 { " member reached it" } else { " members reached it" }) span.muted { " · " (pct) "% of the board" } }
                                    }
                                }
                                span.meter { span style={ "width:" (pct) "%" } {} }
                            }
                        }
                    }
                    @if page.can_edit() {
                        div.lv-rung.lv-add style={ "--i:" (rewards.len()) } {
                            span.lv-node { (ui::icon(Icon::Plus)) }
                            form.lv-card method="post" action=(page.path("levels/rewards")) {
                                input.input.num type="number" name="level" min="1" max="500" placeholder="Level" required aria-label="Level";
                                (ui::select("role", &std::iter::once((String::new(), "Pick a role".to_owned())).chain(options.iter().cloned()).collect::<Vec<_>>(), ""))
                                button.btn type="submit" { "Add reward" }
                                @if rewards.is_empty() { span.muted.small { "Members get the role when they reach the level." } }
                            }
                        }
                    }
                }
                @if rewards.iter().any(|(_, r)| cant_give(&ctx, guild, *r).is_some()) {
                    p.muted.small { "Members reaching a level marked Can't give get nothing. Pick another role, or move the bot's role above it in Discord." }
                }
            }
        }))
    };
    let rail = html! {
        div.rail-block {
            h2 { "Members by level" }
            @if board.is_empty() {
                p.muted.small { "Nobody has XP yet." }
            } @else {
                (ui::column_bars(&spread, "members", "Members in each band of levels"))
            }
        }
        div.rail-block.lv-near {
            h2 { "Close to a reward" }
            @if near.is_empty() {
                p.muted.small { "Nobody is within three levels of their next reward." }
            }
            @for (u, l, rl, r) in &near {
                div.mini {
                    img.avatar src=(ctx.member_avatar(guild, *u)) alt="" loading="lazy";
                    div.grow {
                        span { (ctx.display_name(guild, *u)) }
                        span.muted { "Level " (l) " · " (role_name(&ctx, guild, *r)) " at " (rl) }
                    }
                    span.mono { (rl - l) (if rl - l == 1 { " level" } else { " levels" }) }
                }
            }
        }
    };
    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, "Rewards"))
        div.with-rail {
            div.stack.lv-main { (main) }
            aside.rail { (rail) }
        }
    };
    Ok(ui::layout(&ctx, &page, "Levels", None, body).into_response())
}

async fn set_member_xp(State(ctx): State<Ctx>, user: User, Path((guild, member)): Path<(u64, u64)>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let xp = form
        .i64("xp")
        .filter(|x| (0..=curve::MAX_XP).contains(x))
        .ok_or_else(|| WebError::Bad(format!("XP must be from 0 to {}.", text::num(curve::MAX_XP))))?;
    let had = crate::member(&ctx, guild, member)?.map(|m| m.prestige).unwrap_or(0);
    crate::set_xp(&ctx, guild, member, xp).await?;
    let who = ctx.display_name(guild, member);
    let prestige = form.i64("prestige").map(|p| p.clamp(0, 1000)).filter(|p| *p != had);
    match prestige.filter(|_| ctx.settings::<Settings>(guild, KEY).is_ok_and(|s| s.prestige_on())) {
        Some(p) => {
            crate::set_prestige(&ctx, guild, member, p)?;
            ctx.audit(&page.user, Some(guild), KEY, format!("Set {who}'s XP to {} and prestige to {p}", text::num(xp)));
        }
        None => ctx.audit(&page.user, Some(guild), KEY, format!("Set {who}'s XP to {}", text::num(xp))),
    }
    Ok(saved(&page.path("levels")))
}

async fn import_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let local = legacy_db_path().filter(|p| std::path::Path::new(p).exists());
    let ranked = crate::count(&ctx, guild)?;
    let rewards = crate::rewards(&ctx, guild)?.len() as i64;
    let top_level: i64 = ctx
        .db
        .with(|c| c.query_row("SELECT COALESCE(MAX(level), 0) FROM levels WHERE guild_id = ?1", [guild as i64], |r| r.get(0)))
        .unwrap_or(0);
    // Replace or keep the higher, as a segmented choice; `name` is what the form or script reads.
    let modes = |name: &str, theirs: &str| {
        html! {
            div.seg role="radiogroup" aria-label="When a member already has XP" {
                label { input type="radio" name=(name) value="replace" checked; { "Use " (theirs) } }
                label { input type="radio" name=(name) value="higher"; "Keep the higher" }
            }
        }
    };
    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, "Import"))
        (readings(&[
            ("members with XP now", text::num(ranked)),
            ("reward roles", text::num(rewards)),
            ("highest level", text::num(top_level)),
        ]))
        @if let Some(path) = &local {
            (section(Icon::Plug, "The old bot's database is on this server", None, html! {
                div.card.lv-local {
                    div.grow {
                        strong { "Ready to read" }
                        p.muted { "Found at " code { (path) } ". Importing reads it and leaves it as it is." }
                    }
                    @if page.can_edit() {
                        form.inline-form method="post" action=(page.path("levels/import-local")) {
                            (modes("mode", "the old bot's"))
                            button.btn.primary type="submit" { "Import from the old bot" }
                        }
                    }
                }
            }))
        }
        (section(Icon::Log, if local.is_some() { "Or upload a file" } else { "Bring XP over from antho-s-bot" }, Some("Use a backup of its database, or the JSON the Render Shell command prints. Only members of this server are imported. Reward roles and XP settings come too."), html! {
            div.card style="padding:18px" {
                label.lv-drop #import-drop {
                    input #import-file type="file" accept=".db,.sqlite,.json,application/json" aria-label="The old bot's database or JSON";
                    span.lv-drop-ic { (upload_icon()) }
                    strong { "Drop the database or JSON here" }
                    span.muted { "or click to pick it. A .db, .sqlite or .json file up to 64 MB." }
                    span.lv-file { (ui::icon(Icon::Log)) span #import-name {} }
                }
                div.lv-go {
                    (modes("import-mode", "the file's"))
                    button.btn.primary #import-go type="button" { (upload_icon()) "Import" }
                }
            }
            div.lv-result #import-result hidden role="status" {
                span.lv-res-ic #import-icon {}
                div.grow { strong #import-title {} p.muted.small #import-text {} }
            }
        }))
        (section(Icon::Bulb, "What happens", None, html! {
            div.lv-steps {
                div.mini { span.lvl { "1" } div.grow { span { "Only this server's members are read" } span.muted { "Other servers in the file are left alone." } } }
                div.mini { span.lvl { "2" } div.grow { span { "XP is replaced, or the higher of the two is kept" } span.muted { "Levels are worked out again from the XP, on the curve the file brings." } } }
                div.mini { span.lvl { "3" } div.grow { span { "Reward roles and XP settings come along" } span.muted { "Roles with moderation or admin power come only when an admin imports." } } }
            }
        }))
        script { (PreEscaped(format!(r#"
(() => {{
  const $ = (id) => document.getElementById(id);
  const input = $("import-file"), drop = $("import-drop"), out = $("import-result");
  const size = (n) => n < 1048576 ? Math.max(1, Math.round(n / 1024)) + " KB" : (n / 1048576).toFixed(1) + " MB";
  const show = () => {{
    const f = input.files[0];
    drop.classList.toggle("picked", !!f);
    $("import-name").textContent = f ? f.name + " · " + size(f.size) : "";
  }};
  input.addEventListener("change", show);
  ["dragenter", "dragover"].forEach((t) => drop.addEventListener(t, () => drop.classList.add("over")));
  ["dragleave", "drop"].forEach((t) => drop.addEventListener(t, () => drop.classList.remove("over")));
  const say = (kind, title, text) => {{
    out.hidden = false;
    out.className = "lv-result " + kind;
    $("import-icon").innerHTML = kind === "ok" ? '{ok}' : kind === "bad" ? '{bad}' : "";
    $("import-title").textContent = title;
    $("import-text").textContent = text;
  }};
  $("import-go").addEventListener("click", async (e) => {{
    const file = input.files[0];
    if (!file) {{ say("bad", "Pick a file first", "Drop the old bot's database or JSON on the box above."); return; }}
    const btn = e.currentTarget;
    btn.classList.add("busy");
    say("", "Importing " + file.name, "Large databases take a minute.");
    const mode = document.querySelector('input[name="import-mode"]:checked').value;
    try {{
      const res = await fetch("{path}?mode=" + mode, {{ method: "POST", body: file }});
      const data = await res.json();
      say(data.ok ? "ok" : "bad", data.ok ? "Imported" : "Nothing imported", data.message);
      if (data.ok) window.botToast("Imported");
    }} catch (_) {{ say("bad", "The upload failed", "Check the connection and try again."); }}
    btn.classList.remove("busy");
  }});
}})();
"#, path = page.path("levels/import"), ok = ui::icon(Icon::Check).into_string(), bad = ui::icon(Icon::Close).into_string()))) }
    };
    Ok(ui::layout(&ctx, &page, "Levels", None, body).into_response())
}

/// Where antho-s-bot kept its database on the shared Render disk.
fn legacy_db_path() -> Option<String> {
    let p = std::env::var("LEGACY_DB_PATH").unwrap_or_else(|_| "/data/bot.db".into());
    (!p.trim().is_empty() && p != "off").then_some(p)
}

async fn import_local(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let path = legacy_db_path().filter(|p| std::path::Path::new(p).exists()).ok_or(WebError::NotFound)?;
    let mode = if form.get("mode") == Some("higher") { import::Mode::KeepHigher } else { import::Mode::Replace };
    let ctx2 = ctx.clone();
    let page_admin = page.user.admin;
    let report = bot_core::tokio::task::spawn_blocking(move || import::import_db(&ctx2, std::path::Path::new(&path), Some(guild), mode, page_admin))
        .await
        .map_err(|e| WebError::Internal(e.into()))?
        .map_err(|e| WebError::Bad(format!("Couldn't import: {e}")))?;
    let msg = report.summary(&ctx);
    ctx.audit(&page.user, Some(guild), KEY, msg.clone());
    ctx.notify(Some(guild), KEY, msg, Some(page.path("levels/members")));
    Ok(saved(&page.path("levels/members")))
}

#[derive(Deserialize)]
struct ImportQuery {
    mode: Option<String>,
}

async fn import_upload(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<ImportQuery>, body: Bytes) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let mode = if q.mode.as_deref() == Some("higher") { import::Mode::KeepHigher } else { import::Mode::Replace };
    let res = tokio_blocking(ctx.clone(), body, guild, mode, page.user.admin).await;
    let (ok, message) = match res {
        Ok(report) => {
            let m = report.summary(&ctx);
            ctx.audit(&page.user, Some(guild), KEY, m.clone());
            (true, m)
        }
        Err(e) => (false, format!("Couldn't import: {e}")),
    };
    Ok(Json(serde_json::json!({ "ok": ok, "message": message })).into_response())
}

async fn tokio_blocking(ctx: Ctx, body: Bytes, guild: u64, mode: import::Mode, admin: bool) -> bot_core::anyhow::Result<import::Report> {
    // SQLite work on a large file shouldn't hold up the async workers.
    bot_core::tokio::task::spawn_blocking(move || import::import_bytes(&ctx, &body, Some(guild), mode, admin)).await?
}

/// The overview's "Most XP this week" block and recent level-ups.
pub(crate) fn overview(ctx: &Ctx, page: &Page) -> bot_core::Overview {
    let guild = page.guild_id;
    let week: Vec<(u64, i64)> = ctx
        .db
        .with(|c| {
            let mut s = c.prepare(
                "SELECT l.user_id, l.xp - COALESCE(w.xp, 0) AS gain FROM levels l
                 LEFT JOIN level_week w ON w.guild_id = l.guild_id AND w.user_id = l.user_id
                 WHERE l.guild_id = ?1 AND gain > 0 ORDER BY gain DESC LIMIT 4",
            )?;
            let rows = s.query_map([guild as i64], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?)))?.collect::<bot_core::rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap_or_default();
    let ups: Vec<(u64, i64, Option<i64>, i64)> = ctx
        .db
        .with(|c| {
            let mut s = c.prepare("SELECT user_id, level, role_id, at FROM level_ups WHERE guild_id = ?1 AND at > ?2 ORDER BY at DESC LIMIT 5")?;
            let rows = s
                .query_map(params![guild as i64, bot_core::now_ms() - 2 * 86_400_000], |r| {
                    Ok((r.get::<_, i64>(0)? as u64, r.get(1)?, r.get(2)?, r.get(3)?))
                })?
                .collect::<bot_core::rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap_or_default();
    let roles = ctx.roles(guild);
    let mut o = bot_core::Overview::default();
    if !week.is_empty() {
        o.panel.push((20, html! {
            h2 { "Most XP this week" }
            @for (user, gain) in &week {
                a.mini href=(page.path("levels")) {
                    img.avatar src=(ctx.member_avatar(guild, *user)) alt="" width="24" height="24" loading="lazy";
                    div.grow { span { (ctx.display_name(guild, *user)) } }
                    span.mono { "+" (text::num(*gain)) }
                }
            }
        }));
    }
    for (user, level, role, at) in ups {
        let role = role.and_then(|r| roles.iter().find(|x| x.0 as i64 == r)).map(|r| format!(" and got @{}", r.1)).unwrap_or_default();
        o.feed.push(bot_core::Happening {
            at,
            avatar: ctx.member_avatar(guild, user),
            who: ctx.display_name(guild, user),
            text: format!("reached level {level}{role}"),
        });
    }
    o
}
