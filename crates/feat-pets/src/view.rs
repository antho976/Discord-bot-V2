//! Pieces shared by the dashboard and the public pages: pet tiles, rarity tags, a horizontal bar
//! list and the few styles the core stylesheet doesn't have.

use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::twilight_model::id::Id;
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
@media (max-width: 560px) {
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

pub fn avatar(ctx: &Ctx, user: u64) -> String {
    let hash = ctx.cache.user(Id::new(user)).and_then(|u| u.avatar.map(|a| a.to_string()));
    bot_core::discord::avatar_url(user, hash.as_deref())
}

pub fn person(ctx: &Ctx, guild: u64, user: u64) -> Markup {
    ui::person(&ctx.display_name(guild, user), &avatar(ctx, user), None)
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
