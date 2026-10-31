//! Pieces shared by the dashboard and the public pages: pet tiles, rarity tags, a horizontal bar
//! list and the few styles the core stylesheet doesn't have.

use std::collections::HashMap;

use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::{Ctx, now_ms, text};

use crate::store::{self, Pet, Status};
use bot_core::web::ui::{self, Tone};

pub const STYLE: &str = r#"
.pt { display: inline-grid; place-items: center; width: 34px; height: 34px; border-radius: 10px; background: var(--control); font-size: 18px; line-height: 1; flex: none; overflow: hidden; }
.pt img { width: 100%; height: 100%; object-fit: contain; }
.pt.lg { width: 44px; height: 44px; font-size: 24px; border-radius: 12px; }
.rar { display: inline-flex; align-items: center; gap: 6px; font-size: 12px; color: var(--ink-2); white-space: nowrap; }
.rar::before { content: ""; width: 7px; height: 7px; border-radius: 50%; background: var(--rar); flex: none; }
.rar.common { --rar: rgba(237, 237, 237, 0.4); }
.rar.uncommon { --rar: #7cc49a; }
.rar.rare { --rar: #7f9cf5; }
.rar.legendary { --rar: #e8b765; }
.pc { display: flex; align-items: center; gap: 10px; min-width: 0; }
.pc .t { display: flex; flex-direction: column; min-width: 0; }
.pc .t > span { color: var(--ink-3); font-size: 12px; }
.pgrid { display: grid; grid-template-columns: repeat(auto-fill, minmax(260px, 1fr)); column-gap: 28px; }
.pcard { display: flex; gap: 12px; align-items: flex-start; padding: 12px 0; border-top: 1px solid var(--edge-1); }
.pcard.missing { opacity: 0.4; }
.pcard .t { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
.pcard .n { display: flex; gap: 8px; align-items: baseline; justify-content: space-between; font-weight: 500; }
.pcard .n b { font: 500 12px var(--mono); color: var(--ink); flex: none; }
.pcard .n i { font: 12px var(--sans); font-style: normal; color: var(--ink-3); flex: none; }
.pcard p { font-size: 12.5px; color: var(--ink-3); }
.pcard p.bonus { color: var(--ink-2); }
.hbars { display: flex; flex-direction: column; gap: 12px; }
.hbar { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 6px 12px; align-items: center; font-size: 13px; }
.hbar .v { font: 12px var(--mono); color: var(--ink-2); }
.hbar .track { display: block; grid-column: 1 / -1; height: 5px; border-radius: 3px; background: var(--control); }
.hbar .fill { display: block; height: 100%; border-radius: 3px; background: var(--data); min-width: 3px; }
.gv { display: flex; align-items: center; gap: 12px; padding: 12px 0; }
.gv + .gv, .gv-wrap + .gv-wrap { border-top: 1px solid var(--edge-1); }
.gv .t { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.gv .t b { font-weight: 500; }
.gv .who { color: var(--ink-2); font-size: 12.5px; }
.gv .gv-side { display: flex; flex-direction: column; align-items: flex-end; gap: 4px; flex: none; font-size: 12px; color: var(--ink-3); text-align: right; }
.gv .gv-side .soon { color: var(--waiting); }
.gv-acts { display: flex; gap: 6px; align-items: center; flex-wrap: wrap; justify-content: flex-end; }
.gv-wrap { padding-bottom: 4px; }
.gv-wrap .gv { padding-bottom: 6px; }
.gv-note { font-size: 12.5px; color: var(--ink-2); margin: 0 0 6px 46px; }
.gv-more { margin: 0 0 10px 46px; font-size: 12.5px; }
.gv-more summary { cursor: pointer; color: var(--ink-3); width: max-content; }
.gv-more summary:hover { color: var(--ink); }
.cmts { display: flex; flex-direction: column; gap: 8px; padding: 8px 0 4px; }
.cmt { display: flex; gap: 8px; align-items: baseline; }
.cmt .grow span { color: var(--ink-3); font-size: 12px; }
.links { display: flex; flex-direction: column; }
.links a { display: flex; align-items: center; gap: 12px; padding: 10px 0; }
.links a + a { border-top: 1px solid var(--edge-1); }
.links a:hover strong { text-decoration: underline; text-underline-offset: 3px; }
.links .grow span { display: block; color: var(--ink-3); font-size: 12px; }
.links .icon { color: var(--ink-3); }
.bulk { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; padding-top: 12px; }
.bulk .input { flex: 1; min-width: 200px; max-width: 380px; }
.pform { display: flex; flex-direction: column; gap: 12px; }
.pform .acts { display: flex; gap: 8px; justify-content: flex-end; }
.cat-list { display: flex; flex-wrap: wrap; gap: 6px; }
.cat-list .chip { cursor: default; gap: 8px; }
.cat-list .chip form button { background: none; border: 0; color: var(--ink-3); cursor: pointer; padding: 0; font: inherit; }
.cat-list .chip form button:hover, .cat-list .chip form button.armed { color: var(--held); }
.pets-hero { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: 36px; align-items: center; }
.pets-ring-wrap { display: flex; align-items: center; gap: 22px; }
.pets-ring { position: relative; width: 88px; height: 88px; flex: none; }
.pets-ring svg { width: 100%; height: 100%; transform: rotate(-90deg); display: block; }
.pets-ring span { position: absolute; inset: 0; display: grid; place-items: center; font-size: 17px; font-weight: 500; letter-spacing: -0.01em; }
.pets-count { display: flex; flex-direction: column; gap: 3px; }
.pets-count .muted { font-size: 13px; }
.pets-count .small { color: var(--live); }
.pets-cats { display: flex; flex-direction: column; gap: 12px; min-width: 0; }
.pets-cat { display: grid; grid-template-columns: minmax(96px, 150px) minmax(0, 1fr) minmax(40px, auto); align-items: center; gap: 16px; font-size: 13px; }
.pets-cat .clip { min-width: 0; }
.pets-cat .meter { height: 5px; }
.pets-cat .mono { text-align: right; color: var(--ink-2); }
.pets-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(112px, 1fr)); gap: 10px; }
.pets-tile { position: relative; display: flex; flex-direction: column; align-items: center; gap: 6px; padding: 16px 8px 12px; min-width: 0; border-radius: 12px; background: var(--veil); border: 1px solid var(--edge-1); text-align: center; }
.pets-tile.missing { opacity: .4; }
.pets-art { font-size: 30px; line-height: 1; height: 34px; display: grid; place-items: center; }
.pets-art img { width: 34px; height: 34px; object-fit: contain; }
.pets-name { font-size: 12.5px; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.pets-count-badge { position: absolute; top: 6px; right: 6px; padding: 0 6px; height: 18px; line-height: 18px; border-radius: 9px; background: var(--control); color: var(--ink-2); font: 500 11px var(--mono); }
.pets-rar-dot { position: absolute; top: 9px; left: 9px; width: 6px; height: 6px; border-radius: 50%; background: var(--rar); }
.pets-tile.common .pets-rar-dot { display: none; }
.pets-tile.uncommon { --rar: #7cc49a; }
.pets-tile.rare { --rar: #7f9cf5; }
.pets-tile.legendary { --rar: #e8b765; }
.pets-tab-count { margin-left: 6px; font: 12px var(--mono); color: var(--ink-3); }
.pets-pop > summary .pets-pop-label { white-space: nowrap; }
.pets-pop .pop-body { width: min(520px, calc(100vw - 32px)); padding: 16px; }
.pets-pop .pop-body .card { padding: 0 14px; }
.pets-main { min-width: 0; }
.pets-list { display: flex; flex-direction: column; }
.pets-list-head { display: flex; align-items: center; gap: 16px; flex-wrap: wrap; padding-bottom: 10px; }
.pets-check { display: inline-flex; align-items: center; gap: 8px; font-size: 13px; color: var(--ink-2); cursor: pointer; }
.pets-row { display: flex; align-items: center; gap: 12px; padding: 12px 0; border-top: 1px solid var(--edge-1); min-width: 0; font-size: 13px; }
.pets-row:last-child { border-bottom: 1px solid var(--edge-1); }
.pets-row > input[type=checkbox] { flex: none; }
.pets-row-main { display: flex; flex-direction: column; gap: 3px; flex: 1; min-width: 0; }
.pets-row-main strong { font-weight: 500; }
.pets-row-main .rar { margin-left: 4px; }
.pets-by { display: inline-flex; align-items: center; gap: 6px; font-size: 12.5px; color: var(--ink-3); min-width: 0; }
.pets-by .avatar { width: 18px; height: 18px; }
.pets-reason { font-size: 12.5px; color: var(--ink-2); overflow-wrap: anywhere; }
.pets-row-side { display: flex; flex-direction: column; align-items: flex-end; gap: 2px; flex: none; font-size: 12px; color: var(--ink-3); text-align: right; }
.pets-row-acts { display: flex; align-items: center; gap: 6px; flex: none; }
.pets-ok { color: var(--live); }
.pets-who { display: inline-flex; align-items: center; gap: 6px; min-width: 0; max-width: 100%; }
.pets-who .avatar { width: 18px; height: 18px; }
.pets-who .pets-arrow { flex: none; color: var(--ink-3); }
.pets-nums { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 8px; }
.pets-nums > div { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.pets-nums b { font-size: 24px; font-weight: 500; letter-spacing: -0.02em; line-height: 1.15; }
.pets-nums span { font-size: 12px; color: var(--ink-3); }
.pets-edit { display: block; color: inherit; border-radius: var(--r-key); }
.pets-edit:hover strong { text-decoration: underline; text-underline-offset: 3px; }
.pets-stepper { display: inline-flex; align-items: center; justify-content: flex-end; gap: 6px; }
.pets-stepper > span { min-width: 2ch; text-align: center; }
.rail .links a { padding: 8px 0; }
.pets-bad { color: var(--held); }
@media (max-width: 820px) {
  .pets-hero { grid-template-columns: 1fr; gap: 24px; }
  .pets-row { flex-wrap: wrap; row-gap: 8px; }
  .pets-row-main { flex-basis: calc(100% - 96px); }
  .pets-by { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .pets-row-side { flex: 1; flex-direction: column; align-items: flex-start; padding-left: 46px; text-align: left; }
  .pets-row-acts { margin-left: auto; }
  .pets-rar-col { display: none; }
}
@media (max-width: 560px) {
  .pets-grid { grid-template-columns: repeat(auto-fill, minmax(96px, 1fr)); gap: 8px; }
  .pets-tabs { gap: 14px; }
  .pets-pop > summary .pets-pop-label { display: none; }
  .pets-pop > summary { padding: 0 10px; }
  .gv { flex-wrap: wrap; }
  .gv .gv-side { flex-basis: 100%; flex-direction: row; justify-content: space-between; align-items: center; padding-left: 46px; text-align: left; }
  .gv-note, .gv-more { margin-left: 0; }
  .pc .t > span.desc { display: none; }
  .links .mono { display: none; }
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
                        circle cx="18" cy="18" r="15.915" pathLength="100" fill="none" style="stroke: var(--control)" stroke-width="3" {}
                        circle cx="18" cy="18" r="15.915" pathLength="100" fill="none" style="stroke: var(--data)" stroke-width="3" stroke-linecap="round"
                            stroke-dasharray=(format!("{pct} 100")) {}
                    }
                    span { (pct) "%" }
                }
                div.pets-count {
                    span.big-num { (collected) " of " (total) }
                    span.muted { "companions collected" }
                    @if week > 0 { span.small { "+" (week) " this week" } }
                }
            }
            div.pets-cats {
                @for (name, list) in groups {
                    @let got = list.iter().filter(|p| counts.contains_key(&p.id)).count();
                    div.pets-cat {
                        span.clip { (name) }
                        span.meter { span style={ "width:" (if list.is_empty() { 0 } else { got * 100 / list.len() }) "%" } {} }
                        span.mono { (got) "/" (list.len()) }
                    }
                }
            }
        }
    }
}

/// A pet on the collection grid: faded when the server has none, with how many it has, a dot
/// for rarer pets, and the rarity and bonus on hover.
pub fn collection_tile(p: &Pet, count: i64) -> Markup {
    let rarity = store::clean_rarity(&p.rarity);
    let mut title = format!("{} · {}", p.name, store::rarity_label(rarity));
    if !p.bonus.is_empty() {
        title.push_str(&format!("\n{}", p.bonus));
    }
    html! {
        div class={ "pets-tile " (rarity) @if count == 0 { " missing" } } title=(title) {
            span.pets-rar-dot {}
            @if count > 0 { span.pets-count-badge { "×" (count) } }
            span.pets-art aria-hidden="true" {
                @match p.image_url() {
                    Some(src) => img src=(src) alt="" loading="lazy";,
                    None => (p.icon()),
                }
            }
            span.pets-name { (p.name) }
        }
    }
}

/// The pet's picture, or its emoji on a tile.
pub fn tile(icon: &str, image: Option<&str>, large: bool) -> Markup {
    html! {
        span.pt.lg[large] aria-hidden="true" {
            @match image {
                Some(src) => img src=(src) alt="" loading="lazy";,
                None => (icon),
            }
        }
    }
}

pub fn pet_tile(p: &Pet, large: bool) -> Markup {
    tile(p.icon(), p.image_url(), large)
}

pub fn rarity(r: &str) -> Markup {
    let r = store::clean_rarity(r);
    html! { span class={ "rar " (r) } { (store::rarity_label(r)) } }
}

/// A pet with its name and a muted second line.
pub fn pet_cell(p: &Pet, detail: Option<&str>) -> Markup {
    html! {
        span.pc {
            (pet_tile(p, false))
            span.t {
                strong { (p.name) }
                @if let Some(d) = detail { span.desc { (d) } }
            }
        }
    }
}

/// Horizontal bars: `(label, value)`, scaled to the largest.
pub fn hbars(items: &[(Markup, i64)]) -> Markup {
    let max = items.iter().map(|(_, v)| *v).max().unwrap_or(0).max(1);
    html! {
        div.hbars {
            @for (label, v) in items {
                div.hbar {
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

/// A small arrow from giver to winner, read as "to".
pub fn arrow() -> Markup {
    html! {
        svg.pets-arrow viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" role="img" aria-label="to" {
            path d="M3 8h9M8.5 4.5 12 8l-3.5 3.5" {}
        }
    }
}

/// The giver and the winner of a pet giveaway, with their faces.
pub fn pair(ctx: &Ctx, guild: u64, giver: u64, winner: u64) -> Markup {
    html! {
        span.who.pets-who {
            img.avatar src=(ctx.member_avatar(guild, giver)) alt="" width="18" height="18" loading="lazy";
            span.clip { (ctx.display_name(guild, giver)) }
            (arrow())
            img.avatar src=(ctx.member_avatar(guild, winner)) alt="" width="18" height="18" loading="lazy";
            span.clip { (ctx.display_name(guild, winner)) }
        }
    }
}

pub fn person(ctx: &Ctx, guild: u64, user: u64) -> Markup {
    ui::person(&ctx.display_name(guild, user), &ctx.member_avatar(guild, user), None)
}

#[cfg(test)]
mod tests {
    #[test]
    fn until_reads_well() {
        let now = bot_core::now_ms();
        assert_eq!(super::until(now + 10 * 60_000 + 500), "in 10 min");
        assert_eq!(super::until(now + 3 * 3_600_000 + 500), "in 3 h");
        assert_eq!(super::until(now + 5 * 86_400_000), "in 5 days");
        assert_eq!(super::until(now - 1000), "any moment");
    }
}
