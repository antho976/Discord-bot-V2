//! Pieces shared by the dashboard and the public pages: pet art tinted in its rarity's colour,
//! the collection grid, giveaway cards and rows with the giver and winner's faces, horizontal
//! bars, and the styles the core stylesheet doesn't have.

use std::collections::HashMap;

use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::{Ctx, now_ms, text};

use crate::store::{self, Giveaway, Pet, Status};
use bot_core::web::ui;

pub const STYLE: &str = r#"
/* Rarity reads as its name after a dot in its colour. */
.rar { display: inline-flex; align-items: center; gap: 6px; font-size: 12.5px; font-weight: 500; color: var(--ink-2); white-space: nowrap; }
.rar::before { content: ""; width: 7px; height: 7px; border-radius: 50%; background: var(--rar); }
.rar.r-common::before { background: var(--ink-3); }

/* A pet's picture, or its emoji, on a tile tinted in its rarity's colour. */
.pa { --s: 40px; position: relative; display: inline-grid; place-items: center; width: var(--s); height: var(--s); flex: none; overflow: hidden; border-radius: var(--r-key); font-size: calc(var(--s) * 0.54); line-height: 1; background: color-mix(in oklab, var(--rar) 9%, var(--well)); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--rar) 16%, var(--edge-1)); }
.pa > span { filter: drop-shadow(0 3px 5px rgba(0, 0, 0, 0.4)); }
.pa img { width: 82%; height: 82%; object-fit: contain; }
.pa.sm { --s: 30px; } .pa.md { --s: 44px; } .pa.lg { --s: 56px; border-radius: 12px; } .pa.xl { --s: 84px; border-radius: var(--r-card); }

/* The collection: a grid of tiles that arrives as a wave. Missing pets wait in grey. */
.pets-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(124px, 1fr)); gap: 10px; }
.pets-tile { --rar: var(--ink-2); position: relative; display: flex; flex-direction: column; align-items: center; gap: 2px; min-width: 0; padding: 20px 10px 14px; border-radius: var(--r-card); text-align: center; background: var(--well); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--rar) 20%, var(--edge-1)); transition: transform 0.35s var(--ease), box-shadow 0.25s; animation: pets-in 0.6s var(--spring) both; animation-delay: calc(min(var(--i, 0), 48) * 16ms + 120ms); }
.pets-tile:hover { transform: translateY(-3px); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--rar) 40%, var(--edge-2)); }
.pets-art { display: grid; place-items: center; height: 56px; font-size: 40px; line-height: 1; filter: drop-shadow(0 6px 10px rgba(0, 0, 0, 0.45)); transition: transform 0.45s var(--spring), filter 0.3s, opacity 0.3s; }
.pets-tile:hover .pets-art { transform: scale(1.08); }
.pets-art img { width: 56px; height: 56px; object-fit: contain; }
.pets-name { max-width: 100%; margin-top: 8px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; font-weight: 600; }
.pets-rname { font-size: 12px; color: color-mix(in oklab, var(--rar), white 22%); }
.pets-tile.r-common .pets-rname { color: var(--ink-3); }
.pets-n { position: absolute; top: 9px; right: 12px; font-size: 12px; font-weight: 600; color: var(--ink-2); font-variant-numeric: tabular-nums; }
.pets-tile.missing { background: none; box-shadow: inset 0 0 0 1px var(--edge-1); }
.pets-tile.missing .pets-art { filter: grayscale(1); opacity: 0.32; }
.pets-tile.missing .pets-name { color: var(--ink-3); font-weight: 500; }
.pets-tile.missing .pets-rname { color: var(--ink-3); }
.pets-tile.missing:hover .pets-art { opacity: 0.6; }

/* The collection's progress: a ring that draws in and a meter per category, one colour. */
.pets-hero { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: 44px; align-items: center; padding: 22px 26px; border-radius: var(--r-card); background: var(--well); box-shadow: inset 0 0 0 1px var(--edge-1); }
.pets-ring-wrap { display: flex; align-items: center; gap: 22px; }
.pets-ring { position: relative; width: 112px; height: 112px; flex: none; }
.pets-ring svg { display: block; width: 100%; height: 100%; transform: rotate(-90deg); overflow: visible; }
.pets-ring .fill { stroke: var(--data); animation: pets-ring 1.6s var(--ease) both 0.25s; }
.pets-ring span { position: absolute; inset: 0; display: grid; place-items: center; font-size: 22px; font-weight: 650; letter-spacing: -0.03em; }
.pets-count { display: flex; flex-direction: column; gap: 4px; }
.pets-count .big-num { font-size: 32px; }
.pets-count .of { color: var(--ink-3); font-weight: 500; }
.pets-count .muted { font-size: 13px; color: var(--ink-2); }
.pets-count .up { color: var(--live); font-size: 12.5px; font-weight: 500; }
.pets-cats { display: flex; flex-direction: column; gap: 14px; min-width: 0; }
.pets-cat { display: grid; grid-template-columns: minmax(96px, 180px) minmax(0, 1fr) auto; align-items: center; gap: 16px; font-size: 13.5px; }
.pets-meter { display: block; height: 6px; border-radius: 3px; background: var(--edge-1); overflow: hidden; }
.pets-meter > span { display: block; height: 100%; border-radius: 3px; background: var(--data); transform-origin: left; animation: fill 1.1s var(--ease) both 0.25s; }
.pets-cat .n { min-width: 48px; text-align: right; white-space: nowrap; color: var(--ink-2); font-size: 12.5px; font-variant-numeric: tabular-nums; }

/* Lists: one card, rows split by hairlines, as many columns as the width holds. */
.pets-list { isolation: isolate; display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 440px), 1fr)); column-gap: 36px; padding: 0 18px; border-radius: var(--r-card); background: var(--well); box-shadow: inset 0 0 0 1px var(--edge-1); overflow: hidden; }
.pets-li { --rar: var(--ink-2); position: relative; display: flex; align-items: center; flex-wrap: wrap; gap: 10px 14px; min-width: 0; min-height: 68px; padding: 12px 0; margin-top: -1px; border-top: 1px solid var(--edge-1); font-size: 13.5px; transition: opacity 0.28s var(--ease), transform 0.32s var(--ease); }
.pets-li::before { content: ""; position: absolute; z-index: -1; inset: 0 -16px; background: var(--accent-wash); opacity: 0; transition: opacity 0.2s; pointer-events: none; }
.pets-li:has(> input:checked)::before { opacity: 0.55; }
.pets-li > input[type=checkbox] { flex: none; width: 16px; height: 16px; margin: 0; accent-color: var(--accent); cursor: pointer; }
.pets-li > .avatar { width: 36px; height: 36px; }
.pets-li-main { display: flex; flex-direction: column; gap: 4px; flex: 1 1 0; min-width: 0; }
.pets-li-title { display: flex; align-items: center; gap: 4px 10px; flex-wrap: wrap; min-width: 0; }
.pets-li-title strong { font-size: 14px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.pets-li-title .qty { color: var(--ink-2); font-weight: 600; font-variant-numeric: tabular-nums; }
.pets-li-sub { font-size: 12.5px; color: var(--ink-2); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.pets-li-sub.faint { color: var(--ink-3); }
.pets-li-side { flex: none; display: flex; flex-direction: column; align-items: flex-end; gap: 3px; font-size: 12.5px; color: var(--ink-3); text-align: right; }
.pets-li-acts { flex: none; display: flex; align-items: center; gap: 6px; margin-left: auto; }
/* Rows with two buttons give the text room first; on a phone the buttons go under it. */
.pets-li.wide-text .pets-li-main { flex-basis: 190px; }
.pets-li-extra { flex: 1 0 100%; min-width: 0; padding: 0 0 2px calc(var(--lead, 44px) + 14px); }
.pets-li.leaving { opacity: 0; transform: translateX(22px); pointer-events: none; }
.pets-li.leaving.no { transform: translateX(-22px); }
.pets-by { display: inline-flex; align-items: center; gap: 7px; min-width: 0; font-size: 12.5px; color: var(--ink-3); }
.pets-by .avatar { width: 20px; height: 20px; }
.pets-by b { color: var(--ink-2); font-weight: 500; }
.pets-by b + span { white-space: nowrap; }
.pets-by b + span::before { content: "·"; margin-right: 6px; }

/* A giveaway still waiting: a card with the pet, who gives it to whom, and the clock. */
.pets-gvs { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 460px), 1fr)); gap: 12px; }
.pets-gv { --rar: var(--ink-2); display: grid; grid-template-columns: auto minmax(0, 1fr) auto; gap: 10px 16px; align-items: center; min-width: 0; padding: 16px 18px; border-radius: var(--r-card); background: var(--well); box-shadow: inset 0 0 0 1px var(--edge-1); }
.pets-gv-main { display: flex; flex-direction: column; gap: 7px; min-width: 0; }
.pets-gv-title { display: flex; align-items: center; gap: 4px 10px; flex-wrap: wrap; }
.pets-gv-title b { font-size: 15px; font-weight: 600; }
.pets-gv-side { display: flex; flex-direction: column; align-items: flex-end; gap: 10px; text-align: right; font-size: 12.5px; color: var(--ink-3); }
.pets-gv-acts { display: flex; gap: 6px; }
.pets-gv-extra { grid-column: 2 / -1; display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.pets-note { font-size: 13px; color: var(--ink-2); }

/* The giver's face, an arrow, the winner's face. */
.pets-flow { display: inline-flex; align-items: center; gap: 7px; min-width: 0; flex-wrap: wrap; font-size: 13px; color: var(--ink-2); }
.pets-flow .avatar { width: 22px; height: 22px; }
.pets-flow b { color: var(--ink); font-weight: 500; }
.pets-arrow { flex: none; color: var(--ink-3); }
.pets-flow.live .pets-arrow { color: var(--waiting); }
.pets-flow.live .pets-arrow path:first-child { stroke-dasharray: 2.4 2.2; animation: pets-dash 0.9s linear infinite; }

/* Time left reads as text; it turns into an amber pill only when the hour is nearly up. */
.pets-clock { display: inline-flex; align-items: center; gap: 6px; font-size: 12.5px; color: var(--ink-2); white-space: nowrap; }
.pets-clock .icon { width: 14px; height: 14px; color: var(--ink-3); }
.pets-clock.soon { height: 24px; padding: 0 10px; border-radius: 12px; color: var(--waiting); background: color-mix(in oklab, var(--waiting) 14%, transparent); font-weight: 500; }
.pets-out { display: inline-flex; align-items: center; gap: 6px; font-size: 12.5px; color: var(--ink-2); white-space: nowrap; }
.pets-out::before { content: ""; width: 7px; height: 7px; border-radius: 50%; background: var(--ink-3); }
.pets-out.ok::before { background: var(--live); }
.pets-out.bad::before { background: var(--held); }
.pets-more summary { cursor: pointer; width: max-content; max-width: 100%; color: var(--ink-3); font-size: 12.5px; list-style: none; }
.pets-more summary::-webkit-details-marker { display: none; }
.pets-more summary:hover { color: var(--ink); }
.pets-cmts { display: flex; flex-direction: column; gap: 10px; padding: 10px 0 2px; }
.pets-cmts > .meta { font-size: 12px; color: var(--ink-3); }
.pets-cmt { display: flex; gap: 10px; align-items: flex-start; font-size: 13px; }
.pets-cmt .grow span { color: var(--ink-3); font-size: 12px; }
.pets-cmt .ini { display: grid; place-items: center; width: 24px; height: 24px; border-radius: 50%; flex: none; background: var(--control); color: var(--ink-2); font-size: 11px; font-weight: 700; }

/* Horizontal bars for the stats: one colour each. */
.hbars { display: flex; flex-direction: column; gap: 12px; }
.hbar { --rar: var(--data); display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 7px 12px; align-items: center; font-size: 13.5px; }
.hbar .v { font-size: 12.5px; font-weight: 600; color: var(--ink-2); font-variant-numeric: tabular-nums; }
.hbar .track { display: block; grid-column: 1 / -1; height: 6px; border-radius: 3px; background: var(--edge-1); overflow: hidden; }
.hbar .fill { display: block; height: 100%; min-width: 3px; border-radius: 3px; background: var(--rar); transform-origin: left; animation: fill 1s var(--ease) both; animation-delay: calc(var(--i, 0) * 70ms + 200ms); }
.pc { display: inline-flex; align-items: center; gap: 10px; min-width: 0; }
.pc .avatar { width: 26px; height: 26px; }

/* Section heads: the title and its one line, with an action at the right when there is one. */
.pets-sec-head { display: flex; align-items: flex-end; gap: 10px 16px; flex-wrap: wrap; }
.pets-sec-head > .block-head { flex: 1 1 320px; min-width: 0; }
.pets-main { min-width: 0; gap: 32px; }
.pets-pop > summary .pets-pop-label { white-space: nowrap; }
.pets-pop .pop-body { width: min(560px, calc(100vw - 32px)); padding: 18px; }

/* The rail. */
.pets-req { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: 12px; align-items: start; font-size: 13.5px; transition: opacity 0.28s var(--ease), transform 0.32s var(--ease); }
.pets-req.leaving { opacity: 0; transform: translateX(22px); pointer-events: none; }
.pets-req.leaving.no { transform: translateX(-22px); }
.pets-req strong { font-weight: 600; }
.pets-req .qty { color: var(--ink-2); font-weight: 600; }
.pets-req-acts { display: flex; gap: 6px; margin-top: 9px; flex-wrap: wrap; }
.pets-req-acts form { display: inline; }
.pets-mini-gv { display: grid; grid-template-columns: auto auto minmax(0, 1fr) auto; gap: 10px; align-items: center; font-size: 13.5px; }
.pets-mini-gv .avatars .avatar { width: 26px; height: 26px; }
.pets-mini-gv .grow { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.pets-state { font-size: 12.5px; white-space: nowrap; color: var(--ink-3); }
.pets-state.soon { color: var(--waiting); }
.pets-state.ok { color: var(--live); }
.pets-state.bad { color: var(--held); }
.pets-ok { color: var(--live); }
.pets-bad { color: var(--held); }
.links { display: flex; flex-direction: column; }
.links a { display: flex; align-items: center; gap: 12px; padding: 9px 0; font-size: 13.5px; }
.links a + a { border-top: 1px solid var(--edge-1); }
.links a:hover { color: var(--hue); }
.links .icon { color: var(--ink-3); }

/* The public pages' spotlight. */
.podium .step.p1 { grid-column: 2; grid-row: 1; } .podium .step.p2 { grid-column: 1; grid-row: 1; } .podium .step.p3 { grid-column: 3; grid-row: 1; }
.pets-spot { display: grid; grid-template-columns: repeat(auto-fill, minmax(max(220px, calc((100% - 36px) / 4)), 1fr)); gap: 12px; }
.pets-star { --rar: var(--ink-2); position: relative; display: flex; align-items: center; gap: 16px; padding: 16px 18px; overflow: hidden; border-radius: var(--r-card); background: var(--well); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--rar) 26%, var(--edge-1)); animation: pets-in 0.7s var(--spring) both; animation-delay: calc(var(--i, 0) * 90ms + 200ms); transition: transform 0.35s var(--ease); }
.pets-star:hover { transform: translateY(-3px); }
.pets-star .t { display: flex; flex-direction: column; align-items: flex-start; gap: 5px; min-width: 0; }
.pets-star strong { font-size: 16px; font-weight: 600; letter-spacing: -0.01em; }
.pets-star p { font-size: 13px; color: var(--ink-2); }
.pets-star .n { position: absolute; top: 12px; right: 14px; font-size: 12.5px; font-weight: 600; color: var(--ink-2); font-variant-numeric: tabular-nums; }
/* Each rarity has its own colour: common takes the ink, the rest their game colours. Declared
   after the components so it wins over their defaults. */
.r-common { --rar: var(--ink-2); }
.r-uncommon { --rar: #4fd08a; }
.r-rare { --rar: #5aa9ff; }
.r-legendary { --rar: #ffc24d; }
@keyframes pets-in { from { opacity: 0; transform: translateY(14px) scale(0.9); filter: blur(3px); } }
@keyframes pets-ring { from { stroke-dashoffset: 100; } }
@keyframes pets-dash { to { stroke-dashoffset: -4.6; } }
@media (max-width: 920px) {
  .pets-hero { grid-template-columns: 1fr; gap: 22px; padding: 18px; }
  .pets-ring { width: 92px; height: 92px; }
  .pets-cat { grid-template-columns: minmax(80px, 130px) minmax(0, 1fr) auto; gap: 12px; }
  .pets-list { padding: 0 14px; }
  .pets-li::before { inset: 0 -14px; }
  .pets-li-side { order: 3; flex: 1 1 100%; flex-direction: row; justify-content: flex-start; gap: 10px; text-align: left; }
  .pets-gv { grid-template-columns: auto minmax(0, 1fr); padding: 14px; }
  .pets-gv-side { grid-column: 1 / -1; flex-direction: row; justify-content: space-between; align-items: center; text-align: left; }
  .pets-gv-extra { grid-column: 1 / -1; }
  .pets-li-extra { order: 4; padding-left: 0; }
}
@media (max-width: 560px) {
  .pets-grid { grid-template-columns: repeat(auto-fill, minmax(100px, 1fr)); gap: 8px; }
  .pets-pop > summary .pets-pop-label { display: none; }
  .pets-pop > summary { padding: 0 10px; }
}
"#;

pub fn style() -> Markup {
    html! { style { (PreEscaped(STYLE)) } }
}

/// Categories in the order the catalog lists them, each with its pets.
pub fn groups(catalog: &[Pet]) -> Vec<(String, Vec<&Pet>)> {
    let mut groups: Vec<(String, Vec<&Pet>)> = Vec::new();
    for p in catalog {
        match groups.iter_mut().find(|(c, _)| *c == p.category) {
            Some((_, list)) => list.push(p),
            None => groups.push((p.category.clone(), vec![p])),
        }
    }
    groups
}

/// Reading cards whose figures count up as they come into view.
pub fn readings(items: &[(&str, String)]) -> Markup {
    ui::readings(items)
}

/// A titled block: the title and its one line, `side` at the right. No icon, no box.
pub fn section(title: &str, hint: Option<&str>, side: Option<Markup>, body: Markup) -> Markup {
    html! {
        section.block {
            div.pets-sec-head {
                div.block-head { h2 { (title) } @if let Some(h) = hint { p.muted { (h) } } }
                @if let Some(s) = side { (s) }
            }
            (body)
        }
    }
}

/// The ring of companions collected, with a meter per category. `week` is how many joined in
/// the last seven days; 0 leaves the line out.
pub fn collection_hero(groups: &[(String, Vec<&Pet>)], counts: &HashMap<String, i64>, week: i64) -> Markup {
    let total: usize = groups.iter().map(|(_, l)| l.len()).sum();
    let collected = groups.iter().flat_map(|(_, l)| l.iter()).filter(|p| counts.contains_key(&p.id)).count();
    let pct = (collected * 100).checked_div(total).unwrap_or(0);
    html! {
        div.pets-hero {
            div.pets-ring-wrap {
                div.pets-ring {
                    svg viewBox="0 0 36 36" aria-hidden="true" {
                        circle cx="18" cy="18" r="15.915" pathLength="100" fill="none" style="stroke: var(--edge-1)" stroke-width="2.6" {}
                        circle.fill cx="18" cy="18" r="15.915" pathLength="100" fill="none" stroke-width="2.6" stroke-linecap="round"
                            stroke-dasharray="100 100" style={ "stroke-dashoffset:" (100 - pct) } {}
                    }
                    span { b data-count { (pct) "%" } }
                }
                div.pets-count {
                    span.big-num { b data-count { (collected) } span.of { " of " (total) } }
                    span.muted { "companions collected" }
                    @if week > 0 { span.up { "+" (week) " this week" } }
                }
            }
            div.pets-cats {
                @for (i, (name, list)) in groups.iter().enumerate() {
                    @let got = list.iter().filter(|p| counts.contains_key(&p.id)).count();
                    div.pets-cat {
                        span.clip { (name) }
                        span.pets-meter { span style={ "width:" (if list.is_empty() { 0 } else { got * 100 / list.len() }) "%;animation-delay:" (300 + i * 90) "ms" } {} }
                        span.n { (got) " of " (list.len()) }
                    }
                }
            }
        }
    }
}

/// A pet on the collection grid: faded when the server has none, with how many it has and its
/// rarity. `i` is its place in the page's wave of arrivals.
pub fn collection_tile(p: &Pet, count: i64, i: usize) -> Markup {
    let rarity = store::clean_rarity(&p.rarity);
    let detail = if !p.bonus.is_empty() { p.bonus.as_str() } else if !p.description.is_empty() { p.description.as_str() } else { p.name.as_str() };
    let tip = format!("{}: {}", store::rarity_label(rarity), detail);
    html! {
        div class={ "pets-tile r-" (rarity) @if count == 0 { " missing" } } style={ "--i:" (i) } data-tip=(tip) {
            @if count > 0 { span.pets-n aria-label={ (count) " in the collection" } { "×" (count) } }
            span.pets-art aria-hidden="true" {
                @match p.image_url() {
                    Some(src) => img src=(src) alt="" loading="lazy";,
                    None => (p.icon()),
                }
            }
            span.pets-name { (p.name) }
            span.pets-rname { (store::rarity_label(rarity)) @if count == 0 { span.sr { ", not collected yet" } } }
        }
    }
}

/// A pet's picture, or its emoji, on a tile tinted in its rarity's colour. `size` is "sm",
/// "md", "lg" or "xl".
pub fn art(icon: &str, image: Option<&str>, rarity: &str, size: &str) -> Markup {
    html! {
        span class={ "pa " (size) " r-" (store::clean_rarity(rarity)) } aria-hidden="true" {
            @match image {
                Some(src) => img src=(src) alt="" loading="lazy";,
                None => span { (icon) },
            }
        }
    }
}

pub fn pet_art(p: &Pet, size: &str) -> Markup {
    art(p.icon(), p.image_url().as_deref(), &p.rarity, size)
}

pub fn rarity(r: &str) -> Markup {
    let r = store::clean_rarity(r);
    html! { span class={ "rar r-" (r) } { (store::rarity_label(r)) } }
}

/// Horizontal bars: `(label, value, rarity or "")`, scaled to the largest. A rarity colours
/// its bar; "" keeps the data colour.
pub fn hbars(items: &[(Markup, i64, &str)]) -> Markup {
    let max = items.iter().map(|(_, v, _)| *v).max().unwrap_or(0).max(1);
    html! {
        div.hbars {
            @for (i, (label, v, r)) in items.iter().enumerate() {
                div class={ "hbar" @if !r.is_empty() { " r-" (store::clean_rarity(r)) } } style={ "--i:" (i) } {
                    span.clip { (label) }
                    span.v { (text::num(*v)) }
                    span.track { span.fill style={ "width:" (format!("{:.1}", *v as f64 * 100.0 / max as f64)) "%" } {} }
                }
            }
        }
    }
}

/// How a finished giveaway ended, as a word after a dot in its colour.
pub fn outcome(s: Status) -> Markup {
    let class = match s {
        Status::Confirmed => "pets-out ok",
        Status::Expired => "pets-out bad",
        _ => "pets-out",
    };
    html! { span class=(class) { (s.label()) } }
}

/// "in 3 h", "in 12 min", "in 4 days" for a future time in epoch milliseconds.
pub fn until(ms: i64) -> String {
    let secs = (ms - now_ms()) / 1000;
    match secs {
        ..=59 => "any moment".into(),
        60..10_800 => format!("in {}", text::duration(secs / 60 * 60)),
        10_800..172_800 => format!("in {} h", (secs + 1800) / 3600),
        _ => format!("in {} days", secs / 86_400),
    }
}

/// The expiry line for a waiting giveaway, and whether it's close.
pub fn expiry(at: Option<i64>) -> (String, bool) {
    match at {
        Some(t) => (format!("Expires {}", until(t)), t - now_ms() < 3_600_000),
        None => ("No expiry".into(), false),
    }
}

/// A waiting giveaway's expiry: text with a clock, or an amber pill with a live dot when the
/// last hour has started.
pub fn clock(at: Option<i64>) -> Markup {
    let (line, close) = expiry(at);
    html! { span.pets-clock.soon[close] { @if close { span.dot.waiting {} } @else { (ui::icon(ui::Icon::Clock)) } (line) } }
}

/// A small arrow from giver to winner, read as "to".
pub fn arrow() -> Markup {
    html! {
        svg.pets-arrow viewBox="0 0 20 12" width="18" height="11" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" role="img" aria-label="to" {
            path d="M2 6h14" {}
            path d="M12.5 2.5 16.5 6l-4 3.5" {}
        }
    }
}

/// The giver and the winner of a pet giveaway, with their faces. `live` animates the arrow,
/// for a giveaway still waiting on its hand-over.
pub fn flow(ctx: &Ctx, guild: u64, giver: u64, winner: u64, live: bool) -> Markup {
    html! {
        span.pets-flow.live[live] {
            img.avatar src=(ctx.member_avatar(guild, giver)) alt="" loading="lazy";
            b { (ctx.display_name(guild, giver)) }
            (arrow())
            img.avatar src=(ctx.member_avatar(guild, winner)) alt="" loading="lazy";
            b { (ctx.display_name(guild, winner)) }
        }
    }
}

/// A waiting giveaway as a card: the pet's art, its name and rarity, who gives it to whom, then
/// `side` (expiry and actions) and `extra` (notes and comments) under it.
pub fn giveaway_card(ctx: &Ctx, guild: u64, g: &Giveaway, image: Option<&str>, side: Markup, extra: Option<Markup>) -> Markup {
    let rar = store::clean_rarity(&g.pet_rarity);
    html! {
        div class={ "pets-gv r-" (rar) } {
            (art(g.icon(), image, rar, "lg"))
            div.pets-gv-main {
                div.pets-gv-title { b { (g.pet_name) } (rarity(rar)) }
                (flow(ctx, guild, g.giver_id, g.winner_id, true))
            }
            div.pets-gv-side { (side) }
            @if let Some(x) = extra { div.pets-gv-extra { (x) } }
        }
    }
}

/// A finished giveaway as a row in a `.pets-list`: art, pet, giver to winner, how it ended.
pub fn giveaway_row(ctx: &Ctx, guild: u64, g: &Giveaway, image: Option<&str>, extra: Option<Markup>) -> Markup {
    let rar = store::clean_rarity(&g.pet_rarity);
    let when = g.closed_at.unwrap_or(g.recorded_at);
    let by = g.closed_by.as_deref().filter(|b| !b.is_empty() && *b != "Expired");
    let tip = match by {
        Some(b) => format!("{} by {b}", g.status.label()),
        None => g.status.label().to_owned(),
    };
    html! {
        div class={ "pets-li r-" (rar) } style="--lead: 44px" {
            (art(g.icon(), image, rar, "md"))
            div.pets-li-main {
                span.pets-li-title { strong { (g.pet_name) } (rarity(rar)) }
                (flow(ctx, guild, g.giver_id, g.winner_id, false))
            }
            div.pets-li-side data-tip=(tip) {
                (outcome(g.status))
                span { (text::ago(when)) }
            }
            @if let Some(x) = extra { div.pets-li-extra { (x) } }
        }
    }
}

/// A member: face and name.
pub fn person(ctx: &Ctx, guild: u64, user: u64) -> Markup {
    html! {
        span.pc {
            img.avatar src=(ctx.member_avatar(guild, user)) alt="" loading="lazy";
            span.clip { (ctx.display_name(guild, user)) }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn until_reads_well() {
        let now = bot_core::now_ms();
        assert_eq!(super::until(now + 10 * 60_000 + 500), "in 10 min");
        assert_eq!(super::until(now + 3 * 3_600_000 + 500), "in 3 h");
        assert_eq!(super::until(now + 5 * 86_400_000 + 500), "in 5 days");
        assert_eq!(super::until(now - 1000), "any moment");
    }
}
