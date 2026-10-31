//! Pieces shared by the dashboard and the public pages: pet art lit in its rarity's colour, the
//! collection grid, giveaway cards with the giver and winner's faces, horizontal bars, and the
//! styles the core stylesheet doesn't have.

use std::collections::HashMap;

use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::{Ctx, now_ms, text};

use crate::store::{self, Giveaway, Pet, Status};
use bot_core::web::ui::{self, Tone};

pub const STYLE: &str = r#"
.rar { display: inline-flex; align-items: center; gap: 6px; height: 21px; padding: 0 9px; border-radius: 11px; font-size: 11px; font-weight: 600; white-space: nowrap; color: color-mix(in oklab, var(--rar), white 22%); background: color-mix(in oklab, var(--rar) 14%, transparent); }
.rar::before { content: ""; width: 6px; height: 6px; border-radius: 50%; background: var(--rar); box-shadow: 0 0 6px var(--rar); }
.rar.r-common { color: var(--ink-2); background: var(--control); }
.rar.r-common::before { box-shadow: none; }

/* A pet's picture on a tile lit from behind in its rarity's colour. */
.pa { --s: 40px; position: relative; display: inline-grid; place-items: center; width: var(--s); height: var(--s); flex: none; overflow: hidden; border-radius: calc(var(--s) * 0.3); font-size: calc(var(--s) * 0.56); line-height: 1; background: radial-gradient(circle at 50% 36%, color-mix(in oklab, var(--rar) 34%, transparent), color-mix(in oklab, var(--rar) 6%, transparent) 72%); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--rar) 28%, transparent); }
.pa > span { filter: drop-shadow(0 3px 6px rgba(0, 0, 0, 0.45)); }
.pa img { width: 84%; height: 84%; object-fit: contain; }
.pa.sm { --s: 30px; } .pa.md { --s: 44px; } .pa.lg { --s: 58px; } .pa.xl { --s: 88px; }
.pa.r-legendary { box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--rar) 55%, transparent), 0 10px 24px -12px var(--rar); }

/* The collection: a grid of tiles that arrives as a wave. Missing pets wait in grey. */
.pets-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(118px, 1fr)); gap: 10px; }
.pets-tile { --rar: var(--ink-2); position: relative; display: flex; flex-direction: column; align-items: center; gap: 3px; min-width: 0; padding: 18px 8px 12px; overflow: hidden; border-radius: 16px; text-align: center; background: radial-gradient(90% 70% at 50% 16%, color-mix(in oklab, var(--rar) 17%, transparent), rgba(243, 242, 248, 0.02) 72%); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--rar) 24%, var(--edge-1)); transition: transform 0.35s var(--ease), box-shadow 0.25s; animation: pets-in 0.6s var(--spring) both; animation-delay: calc(min(var(--i, 0), 48) * 16ms + 120ms); }
.pets-tile:hover { transform: translateY(-4px); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--rar) 50%, transparent), 0 18px 32px -18px var(--rar); }
.pets-art { display: grid; place-items: center; height: 54px; font-size: 40px; line-height: 1; filter: drop-shadow(0 6px 10px rgba(0, 0, 0, 0.5)); transition: transform 0.45s var(--spring), filter 0.3s; }
.pets-tile:hover .pets-art { transform: scale(1.14) rotate(-5deg); }
.pets-art img { width: 50px; height: 50px; object-fit: contain; }
.pets-name { max-width: 100%; margin-top: 6px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12.5px; font-weight: 600; }
.pets-rname { font-size: 11px; color: color-mix(in oklab, var(--rar), white 25%); }
.pets-tile.r-common .pets-rname { color: var(--ink-3); }
.pets-count-badge { position: absolute; top: 8px; right: 8px; height: 19px; padding: 0 7px; border-radius: 10px; background: color-mix(in oklab, var(--rar) 22%, rgba(7, 7, 11, 0.6)); color: var(--ink); font: 600 11px/19px var(--mono); }
.pets-tile.missing { background: none; box-shadow: inset 0 0 0 1px var(--edge-1); }
.pets-tile.missing .pets-art { filter: grayscale(1) brightness(0.7); opacity: 0.38; }
.pets-tile.missing .pets-name { color: var(--ink-3); font-weight: 500; }
.pets-tile.missing .pets-rname { opacity: 0.6; }
.pets-tile.missing:hover { box-shadow: inset 0 0 0 1px var(--edge-2); }
.pets-tile.missing:hover .pets-art { opacity: 0.7; filter: grayscale(0.4); }
/* An owned legendary catches the light once it lands. */
.pets-tile.r-legendary:not(.missing)::after { content: ""; position: absolute; inset: 0; background: linear-gradient(105deg, transparent 35%, rgba(255, 220, 140, 0.28) 50%, transparent 65%); transform: translateX(-110%); animation: pets-sheen 1.4s var(--ease) both; animation-delay: calc(min(var(--i, 0), 48) * 16ms + 700ms); pointer-events: none; }

/* The collection's progress: a ring that draws in, numbers that count, a meter per category. */
.pets-hero { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: 40px; align-items: center; padding: 22px 26px; border-radius: 20px; background: radial-gradient(70% 140% at 0% 50%, color-mix(in oklab, var(--hue) 13%, transparent), transparent 70%), linear-gradient(180deg, rgba(243, 242, 248, 0.04), rgba(243, 242, 248, 0.012)); box-shadow: inset 0 0 0 1px var(--edge-1); }
.pets-ring-wrap { display: flex; align-items: center; gap: 22px; }
.pets-ring { position: relative; width: 118px; height: 118px; flex: none; }
.pets-ring svg { display: block; width: 100%; height: 100%; transform: rotate(-90deg); overflow: visible; }
.pets-ring .fill { filter: drop-shadow(0 0 8px color-mix(in oklab, var(--hue) 60%, transparent)); animation: pets-ring 1.6s var(--ease) both 0.25s; }
.pets-ring span { position: absolute; inset: 0; display: grid; place-items: center; font-size: 24px; font-weight: 650; letter-spacing: -0.03em; }
.pets-count { display: flex; flex-direction: column; gap: 3px; }
.pets-count .big-num { font-size: 34px; }
.pets-count .muted { font-size: 13px; }
.pets-count .up { color: var(--live); font-size: 12.5px; font-weight: 500; }
.pets-cats { display: flex; flex-direction: column; gap: 13px; min-width: 0; }
.pets-cat { display: grid; grid-template-columns: minmax(96px, 170px) minmax(0, 1fr) minmax(44px, auto); align-items: center; gap: 16px; font-size: 13px; }
.pets-cat .meter { height: 6px; }
.pets-cat .meter > span { background: linear-gradient(90deg, color-mix(in oklab, var(--hue) 70%, var(--data-2)), var(--hue)); box-shadow: 0 0 10px color-mix(in oklab, var(--hue) 50%, transparent); }
.pets-cat .mono { text-align: right; color: var(--ink-2); }

/* Giveaways: the pet passes from the giver's face to the winner's. */
.pets-flow { display: inline-flex; align-items: center; gap: 7px; min-width: 0; flex-wrap: wrap; font-size: 12.5px; color: var(--ink-2); }
.pets-flow .avatar { width: 22px; height: 22px; }
.pets-flow b { color: var(--ink); font-weight: 600; }
.pets-arrow { flex: none; color: var(--ink-3); }
.pets-flow.live .pets-arrow { color: var(--waiting); }
.pets-flow.live .pets-arrow path:first-child { stroke-dasharray: 2.4 2.2; animation: pets-dash 0.9s linear infinite; }
.pets-gvs { display: flex; flex-direction: column; gap: 10px; }
.pets-gv { --rar: var(--ink-2); display: grid; grid-template-columns: auto minmax(0, 1fr) auto; gap: 8px 16px; align-items: center; padding: 14px 16px; border-radius: 16px; background: linear-gradient(90deg, color-mix(in oklab, var(--rar) 10%, transparent), rgba(243, 242, 248, 0.014) 60%); box-shadow: inset 0 0 0 1px var(--edge-1); transition: transform 0.35s var(--ease), box-shadow 0.25s; min-width: 0; }
.pets-gv:hover { transform: translateX(3px); box-shadow: inset 0 0 0 1px var(--edge-2), var(--shadow-lift); }
.pets-gv.past { background: linear-gradient(90deg, color-mix(in oklab, var(--rar) 5%, transparent), transparent 50%); padding: 11px 16px; }
.pets-gv-main { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.pets-gv-title { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.pets-gv-title b { font-size: 14.5px; font-weight: 600; }
.pets-gv-side { display: flex; flex-direction: column; align-items: flex-end; gap: 8px; text-align: right; font-size: 12px; color: var(--ink-3); }
.pets-gv-acts { display: flex; gap: 6px; }
.pets-gv-extra { grid-column: 2 / -1; display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.pets-note { font-size: 12.5px; color: var(--ink-2); }
.pets-clock { display: inline-flex; align-items: center; gap: 7px; height: 24px; padding: 0 10px; border-radius: 12px; background: var(--control); color: var(--ink-2); font-size: 12px; font-weight: 500; white-space: nowrap; }
.pets-clock.soon { color: var(--waiting); background: color-mix(in oklab, var(--waiting) 14%, transparent); }
.pets-more summary { cursor: pointer; width: max-content; max-width: 100%; color: var(--ink-3); font-size: 12px; list-style: none; }
.pets-more summary::-webkit-details-marker { display: none; }
.pets-more summary:hover { color: var(--ink); }
.pets-cmts { display: flex; flex-direction: column; gap: 8px; padding: 10px 0 2px; }
.pets-cmt { display: flex; gap: 10px; align-items: flex-start; font-size: 13px; }
.pets-cmt .grow span { color: var(--ink-3); font-size: 12px; }
.pets-cmt .ini { display: grid; place-items: center; width: 24px; height: 24px; border-radius: 50%; flex: none; background: color-mix(in oklab, var(--hue) 16%, transparent); color: var(--hue); font-size: 11px; font-weight: 700; }

/* Horizontal bars for the stats. */
.hbars { display: flex; flex-direction: column; gap: 12px; }
.hbar { --rar: var(--data); display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 7px 12px; align-items: center; font-size: 13px; }
.hbar .v { font: 600 12px var(--mono); color: var(--ink-2); }
.hbar .track { display: block; grid-column: 1 / -1; height: 6px; border-radius: 3px; background: var(--edge-1); overflow: hidden; }
.hbar .fill { display: block; height: 100%; min-width: 3px; border-radius: 3px; background: linear-gradient(90deg, color-mix(in oklab, var(--rar) 55%, var(--data-2)), var(--rar)); box-shadow: 0 0 10px color-mix(in oklab, var(--rar) 50%, transparent); transform-origin: left; animation: fill 1s var(--ease) both; animation-delay: calc(var(--i, 0) * 70ms + 200ms); }
.pc { display: inline-flex; align-items: center; gap: 10px; min-width: 0; }
.pc .avatar { width: 26px; height: 26px; }

/* Rows: submissions, bans, catalog entries. */
.pets-rows { display: flex; flex-direction: column; gap: 8px; }
.pets-row { --rar: var(--ink-2); display: flex; align-items: center; gap: 14px; padding: 12px 16px; min-width: 0; border-radius: 16px; font-size: 13px; background: linear-gradient(90deg, color-mix(in oklab, var(--rar) 9%, transparent), rgba(243, 242, 248, 0.014) 60%); box-shadow: inset 0 0 0 1px var(--edge-1); transition: transform 0.35s var(--ease), box-shadow 0.25s, background 0.25s; }
.pets-row:hover { transform: translateX(3px); box-shadow: inset 0 0 0 1px var(--edge-2), var(--shadow-lift); }
.pets-row:has(> input:checked) { background: linear-gradient(90deg, var(--accent-wash), rgba(243, 242, 248, 0.014) 70%); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--accent) 50%, transparent); }
.pets-row > input[type=checkbox] { flex: none; width: 16px; height: 16px; }
.pets-row > .avatar { width: 40px; height: 40px; }
.pets-row-main { display: flex; flex-direction: column; gap: 5px; flex: 1; min-width: 0; }
.pets-row-main > span:first-child { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.pets-row-main strong { font-weight: 600; font-size: 14px; }
.pets-row-side { display: flex; flex-direction: column; align-items: flex-end; gap: 2px; flex: none; font-size: 12px; color: var(--ink-3); text-align: right; }
.pets-row-acts { display: flex; align-items: center; gap: 6px; flex: none; }
.pets-by { display: inline-flex; align-items: center; gap: 7px; min-width: 0; font-size: 12.5px; color: var(--ink-3); }
.pets-by .avatar { width: 20px; height: 20px; }
.pets-by b { color: var(--ink-2); font-weight: 500; }
.pets-reason { font-size: 12.5px; color: var(--ink-2); overflow-wrap: anywhere; }
.pets-ok { color: var(--live); }
.pets-bad { color: var(--held); }

/* The catalog: a card per pet, with a stepper for how many the server has. */
.pets-entries { display: grid; grid-template-columns: repeat(auto-fill, minmax(320px, 1fr)); gap: 10px; }
.pets-entry { --rar: var(--ink-2); display: grid; grid-template-columns: auto minmax(0, 1fr) auto; gap: 12px; align-items: center; padding: 12px 12px 12px 14px; border-radius: 16px; background: linear-gradient(90deg, color-mix(in oklab, var(--rar) 9%, transparent), rgba(243, 242, 248, 0.012) 70%); box-shadow: inset 0 0 0 1px var(--edge-1); transition: box-shadow 0.25s; scroll-margin-top: 90px; }
.pets-entry:hover { box-shadow: inset 0 0 0 1px var(--edge-2); }
.pets-entry:target { box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--accent) 60%, transparent), 0 0 0 4px var(--accent-wash); }
.pets-entry.missing .pa { filter: grayscale(0.85); opacity: 0.55; }
.pets-entry .t { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.pets-entry .t > span:first-child { display: flex; align-items: center; flex-wrap: wrap; gap: 2px 8px; min-width: 0; }
.pets-entry .t strong { font-weight: 600; font-size: 13.5px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.pets-entry .t p { font-size: 12px; color: var(--ink-3); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.pets-entry .t p.bonus { color: var(--ink-2); }
a.pets-edit { color: inherit; }
a.pets-edit:hover strong { text-decoration: underline; text-underline-offset: 3px; text-decoration-color: var(--edge-3); }
.pets-stepper { display: inline-flex; align-items: center; gap: 2px; padding: 3px; border-radius: 11px; background: var(--well); box-shadow: inset 0 0 0 1px var(--edge-1); }
.pets-stepper .btn { width: 26px; height: 26px; padding: 0; border-radius: 8px; font-size: 15px; }
.pets-stepper > span { min-width: 3ch; text-align: center; font: 600 12.5px var(--mono); }
.pets-owned { font: 600 12.5px var(--mono); color: var(--ink-2); }

.pets-main { min-width: 0; gap: 30px; }
.pets-pop > summary .pets-pop-label { white-space: nowrap; }
.pets-pop .pop-body { width: min(520px, calc(100vw - 32px)); padding: 16px; }
.pets-pop .pop-body .card { padding: 0 14px; }
.pform { display: flex; flex-direction: column; gap: 12px; }
.pform .acts { display: flex; gap: 8px; justify-content: flex-end; }
.bulk { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; padding: 12px 14px; margin-top: 12px; border-radius: 14px; background: var(--well); box-shadow: inset 0 0 0 1px var(--edge-1); }
.bulk .input { flex: 1; min-width: 200px; }
.pets-list-head { display: flex; align-items: center; gap: 16px; flex-wrap: wrap; padding: 0 2px 12px; }
.pets-check { display: inline-flex; align-items: center; gap: 8px; font-size: 13px; color: var(--ink-2); cursor: pointer; }
.cat-list { display: flex; flex-wrap: wrap; gap: 6px; }
.cat-list .chip { cursor: default; gap: 8px; }
.cat-list .chip form button { background: none; border: 0; color: var(--ink-3); cursor: pointer; padding: 0; font: inherit; }
.cat-list .chip form button:hover, .cat-list .chip form button.armed { color: var(--held); }
.links { display: flex; flex-direction: column; }
.links a { display: flex; align-items: center; gap: 12px; padding: 9px 0; font-size: 13.5px; }
.links a + a { border-top: 1px solid var(--edge-1); }
.links a:hover { color: var(--hue); }
.links .icon { color: var(--ink-3); }
.pets-sec { gap: 14px; }
.pets-sec-head { display: flex; align-items: flex-start; gap: 12px; }
.pets-sec-ic { display: grid; place-items: center; width: 32px; height: 32px; border-radius: 10px; flex: none; color: var(--hue); background: color-mix(in oklab, var(--hue) 14%, transparent); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--hue) 24%, transparent); }
.pets-sec-ic .icon { width: 17px; height: 17px; }
.pets-sec-head h2 { padding-top: 6px; }
.pets-sec-head p { font-size: 13px; margin-top: 3px; max-width: 68ch; }
.pets-sec-head .grow + * { flex: none; }
.pets-mini-gv { display: grid; grid-template-columns: auto auto minmax(0, 1fr) auto; gap: 10px; align-items: center; font-size: 13px; }
.pets-mini-gv .avatars .avatar { width: 26px; height: 26px; }
.pets-mini-gv .grow { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.pets-state { font-size: 12px; white-space: nowrap; color: var(--ink-3); }
.pets-state.soon { color: var(--waiting); }
.pets-state.ok { color: var(--live); }
.pets-state.bad { color: var(--held); }
.pets-req { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: 11px; align-items: start; font-size: 13px; }
.pets-req .who { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; }
.pets-req .who .avatar { width: 20px; height: 20px; }
.pets-req strong { font-weight: 600; }
.pets-req-acts { display: flex; gap: 6px; margin-top: 8px; flex-wrap: wrap; }
.pets-req-acts form { display: inline; }

/* The public pages open on the server's banner. */
.podium .step.p1 { grid-column: 2; grid-row: 1; } .podium .step.p2 { grid-column: 1; grid-row: 1; } .podium .step.p3 { grid-column: 3; grid-row: 1; }
.pets-spot { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 12px; }
.pets-star { --rar: var(--ink-2); position: relative; display: flex; align-items: center; gap: 16px; padding: 16px 18px; overflow: hidden; border-radius: 18px; background: radial-gradient(90% 120% at 0% 50%, color-mix(in oklab, var(--rar) 22%, transparent), rgba(243, 242, 248, 0.02) 70%); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--rar) 32%, transparent), 0 18px 40px -26px var(--rar); animation: pets-in 0.7s var(--spring) both; animation-delay: calc(var(--i, 0) * 90ms + 200ms); transition: transform 0.35s var(--ease); }
.pets-star:hover { transform: translateY(-3px); }
.pets-star .pa { font-size: 46px; }
.pets-star .t { display: flex; flex-direction: column; align-items: flex-start; gap: 4px; min-width: 0; }
.pets-star strong { font-size: 16px; font-weight: 650; letter-spacing: -0.01em; }
.pets-star p { font-size: 12.5px; color: var(--ink-2); }
.pets-star .n { position: absolute; top: 10px; right: 12px; font: 600 12px var(--mono); color: color-mix(in oklab, var(--rar), white 30%); }
/* Each rarity has its own light: common takes the ink, the rest their game colours. Declared
   after the components so it wins over their defaults. */
.r-common { --rar: var(--ink-2); }
.r-uncommon { --rar: #4fd08a; }
.r-rare { --rar: #5aa9ff; }
.r-legendary { --rar: #ffc24d; }
@keyframes pets-in { from { opacity: 0; transform: translateY(14px) scale(0.86); filter: blur(4px); } }
@keyframes pets-sheen { to { transform: translateX(110%); } }
@keyframes pets-ring { from { stroke-dashoffset: 100; } }
@keyframes pets-dash { to { stroke-dashoffset: -4.6; } }
@media (max-width: 920px) {
  .pets-hero { grid-template-columns: 1fr; gap: 22px; padding: 18px; }
  .pets-ring { width: 96px; height: 96px; }
  .pets-cat { grid-template-columns: minmax(80px, 120px) minmax(0, 1fr) auto; gap: 12px; }
  .pets-row { flex-wrap: wrap; row-gap: 10px; }
  .pets-row .pa.lg { --s: 44px; }
  .pets-row-main { flex: 1 1 150px; }
  .pets-row-side { order: 3; flex: 1; align-items: flex-start; text-align: left; }
  .pets-row-acts { order: 4; margin-left: auto; }
  .pets-gv { grid-template-columns: auto minmax(0, 1fr); }
  .pets-gv-side { grid-column: 1 / -1; flex-direction: row; justify-content: space-between; align-items: center; text-align: left; }
  .pets-gv-extra { grid-column: 1 / -1; }
  .pets-entries { grid-template-columns: 1fr; }
}
@media (max-width: 560px) {
  .pets-grid { grid-template-columns: repeat(auto-fill, minmax(98px, 1fr)); gap: 8px; }
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
    html! { div.readings { @for (label, value) in items { div.reading { b data-count { (value) } span { (label) } } } } }
}

/// A section headed by an icon tile in the group's hue, with `side` at the right of the title.
pub fn section(ic: ui::Icon, title: &str, hint: Option<&str>, side: Option<Markup>, body: Markup) -> Markup {
    html! {
        section.block.pets-sec {
            div.pets-sec-head {
                span.pets-sec-ic { (ui::icon(ic)) }
                div.grow { h2 { (title) } @if let Some(h) = hint { p.muted { (h) } } }
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
                        defs {
                            linearGradient #pets-ring-g x1="0" y1="0" x2="1" y2="1" {
                                stop offset="0" stop-color="var(--hue)" {}
                                stop offset="1" stop-color="var(--data)" {}
                            }
                        }
                        circle cx="18" cy="18" r="15.915" pathLength="100" fill="none" style="stroke: var(--edge-1)" stroke-width="2.6" {}
                        circle.fill cx="18" cy="18" r="15.915" pathLength="100" fill="none" stroke="url(#pets-ring-g)" stroke-width="2.6" stroke-linecap="round"
                            stroke-dasharray="100 100" style={ "stroke-dashoffset:" (100 - pct) } {}
                    }
                    span { b data-count { (pct) "%" } }
                }
                div.pets-count {
                    span.big-num { b data-count { (collected) } " of " (total) }
                    span.muted { "companions collected" }
                    @if week > 0 { span.up { "+" (week) " this week" } }
                }
            }
            div.pets-cats {
                @for (i, (name, list)) in groups.iter().enumerate() {
                    @let got = list.iter().filter(|p| counts.contains_key(&p.id)).count();
                    div.pets-cat {
                        span.clip { (name) }
                        span.meter { span style={ "width:" (if list.is_empty() { 0 } else { got * 100 / list.len() }) "%;animation-delay:" (300 + i * 90) "ms" } {} }
                        span.mono { (got) "/" (list.len()) }
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
            @if count > 0 { span.pets-count-badge { "×" (count) } }
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

/// A pet's picture, or its emoji, on a tile lit in its rarity's colour. `size` is "sm", "md",
/// "lg" or "xl".
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
    art(p.icon(), p.image_url(), &p.rarity, size)
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

pub fn status_badge(s: Status) -> Markup {
    let tone = match s {
        Status::Pending => Tone::Warn,
        Status::Confirmed => Tone::Ok,
        Status::Expired => Tone::Bad,
        Status::Cancelled => Tone::Muted,
    };
    ui::badge(s.label(), tone)
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

/// A waiting giveaway's expiry as a pill, amber with a live dot when it's close.
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

/// A giveaway as a card: the pet's art, its name and rarity, who gives it to whom, then `side`
/// (expiry and actions, or the outcome) and `extra` (notes and comments) under it.
pub fn giveaway_card(ctx: &Ctx, guild: u64, g: &Giveaway, image: Option<&str>, side: Markup, extra: Option<Markup>) -> Markup {
    let rar = store::clean_rarity(&g.pet_rarity);
    let pending = g.status == Status::Pending;
    html! {
        div class={ "pets-gv r-" (rar) @if !pending { " past" } } {
            (art(g.icon(), image, rar, if pending { "lg" } else { "md" }))
            div.pets-gv-main {
                div.pets-gv-title { b { (g.pet_name) } (rarity(rar)) }
                (flow(ctx, guild, g.giver_id, g.winner_id, pending))
            }
            div.pets-gv-side { (side) }
            @if let Some(x) = extra { div.pets-gv-extra { (x) } }
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
