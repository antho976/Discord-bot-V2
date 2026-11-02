//! Public pages, no sign-in: the collection, giveaways and giveaway stats. Served only for
//! servers where the feature is on.

use std::collections::{HashMap, HashSet};

use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::get;
use bot_core::maud::{DOCTYPE, Markup, PreEscaped, html};
use bot_core::web::auth::MaybeUser;
use bot_core::web::ui;
use bot_core::web::{Level, WebError, WebResult};
use bot_core::{Ctx, Icon, jiff, now_ms, text};
use serde::Deserialize;

use crate::store::{self, Giveaway, Pet, Status};
use crate::{KEY, view};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/p/{guild}/pets", get(collection))
        .route("/p/{guild}/pet-giveaways", get(giveaways))
        .route("/p/{guild}/pet-stats", get(stats))
}

fn check(ctx: &Ctx, guild: u64) -> WebResult<()> {
    if !ctx.guild_ids().contains(&guild) || !ctx.enabled(guild, KEY) {
        return Err(WebError::NotFound);
    }
    store::ensure_seeded(ctx, guild)?;
    Ok(())
}

/// The public pages' frame: the server's banner (or art drawn from its icon) with its icon and
/// the page's title, then the three tabs.
const PUBLIC_CSS: &str = r#"
body.pets-pub { background: var(--page); }
/* The page is as wide as a row of a dozen pets, centred under the banner; the hero's text lines
   up with the page's edge. */
.pets-pub-hero { min-height: 310px; border-radius: 0; padding: 20px max(var(--gutter), calc((100% - 1600px) / 2)) 30px; }
.pets-pub-hero .ov-hello h1 { font-size: 42px; }
.pets-pub-page { max-width: calc(1600px + 2 * var(--gutter)); margin: 0 auto; padding: 0 var(--gutter) 72px; display: flex; flex-direction: column; gap: 34px; container: col / inline-size; }
.pets-pub-page > * { animation: rise 0.55s var(--ease) both; }
.pets-pub-page > :nth-child(2) { animation-delay: 60ms; }
.pets-pub-page > :nth-child(3) { animation-delay: 120ms; }
.pets-pub-page > :nth-child(n + 4) { animation-delay: 170ms; }
.pets-pub-foot { display: flex; align-items: center; justify-content: space-between; gap: 12px; flex-wrap: wrap; padding-top: 18px; border-top: 1px solid var(--edge-1); color: var(--ink-3); font-size: 12.5px; }
.pets-pub-foot a { color: var(--ink-2); display: inline-flex; align-items: center; gap: 6px; }
.pets-pub-foot a:hover { color: var(--ink); }
/* Tags over the grid: one per rarity, rarest first, and Pet+. One of each kind at a time. */
.pets-tags { display: flex; flex-wrap: wrap; gap: 8px; }
.pets-tag { display: inline-flex; align-items: center; gap: 8px; height: 32px; padding: 0 13px; border-radius: 16px; background: var(--control); color: var(--ink-2); font-size: 13px; font-weight: 500; white-space: nowrap; transition: background 0.18s, color 0.18s, box-shadow 0.2s, transform 0.25s var(--spring); }
.pets-tag:hover { background: var(--hover); color: var(--ink); }
.pets-tag:active { transform: scale(0.96); }
.pets-tag[class*=" r-"]::before { content: ""; width: 8px; height: 8px; border-radius: 50%; background: var(--rar); }
.pets-tag.r-common::before { background: var(--ink-3); }
.pets-tag .n { color: var(--ink-3); font-size: 12px; font-variant-numeric: tabular-nums; }
.pets-tag.on { color: var(--ink); background: color-mix(in oklab, var(--rar, var(--accent)) 16%, var(--control)); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--rar, var(--accent)) 55%, transparent); }
.pets-tag.on .n { color: var(--ink-2); }
.pets-tag.plus { --rar: var(--plus); }
.pets-tag.plus b { color: var(--plus); font-weight: 650; }
.pets-tag-gap { width: 1px; align-self: stretch; margin: 4px 4px; background: var(--edge-2); }
/* Stats (a sub-tab, so no lead): two columns that end together. Left, the chart with its
   figures and the rarity table; right, a fixed column of the people and pets, one panel each. */
.pets-st { display: grid; grid-template-columns: minmax(0, 1fr); gap: 20px; }
@container col (min-width: 1000px) { .pets-st { grid-template-columns: minmax(0, 1fr) 440px; } }
.pets-st-col { min-width: 0; display: flex; flex-direction: column; gap: 20px; }
.pets-st-col > .pets-panel:last-child { flex: 1; }
.pets-panel { min-width: 0; display: flex; flex-direction: column; gap: 14px; background: var(--raised); border: 1px solid var(--edge-1); border-radius: 14px; padding: 18px 20px; }
.pets-panel-head { display: flex; align-items: baseline; justify-content: space-between; gap: 4px 12px; flex-wrap: wrap; }
.pets-panel-head h2 { font-size: 14px; font-weight: 500; letter-spacing: 0; }
.pets-panel-head p { font-size: 13px; color: var(--ink-3); }
.pets-figs { display: flex; flex-wrap: wrap; gap: 10px 36px; }
.pets-figs div { display: flex; flex-direction: column; gap: 1px; }
.pets-figs b { font: 600 20px/1.2 var(--sans); letter-spacing: -0.01em; font-variant-numeric: tabular-nums; white-space: nowrap; }
.pets-figs span { font-size: 12.5px; color: var(--ink-3); white-space: nowrap; }
.pets-note-line { font-size: 12.5px; color: var(--ink-3); }
.pets-st .area svg { height: 160px; }
/* Ranked rows: a 28 px face or pet, a 14 px name, a thin bar and the count. */
.pets-rank { display: flex; flex-direction: column; }
.pets-rank-row { --rar: var(--data); display: grid; grid-template-columns: 18px 28px minmax(0, 1fr) minmax(60px, 38%) 28px; align-items: center; gap: 12px; min-height: 40px; font-size: 14px; }
.pets-rank-row + .pets-rank-row { border-top: 1px solid var(--edge-1); }
.pets-rank-row .i { font: 500 12px var(--mono); color: var(--ink-3); text-align: right; }
.pets-rank-row .avatar { width: 28px; height: 28px; }
.pets-rank-row .pa { --s: 28px; border-radius: 8px; }
.pets-rank-row .nm { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.pets-rank-row .v, .pets-rar-row .v { font: 500 13px var(--mono); color: var(--ink-2); text-align: right; }
.pets-thin { display: block; height: 4px; border-radius: 2px; background: var(--edge-1); overflow: hidden; }
.pets-thin > span { display: block; height: 100%; min-width: 2px; border-radius: 2px; background: var(--rar, var(--data)); }
/* Rarity: one row per rarity, given beside held. */
.pets-rar { display: flex; flex-direction: column; }
.pets-rar-row { display: grid; grid-template-columns: minmax(96px, 140px) minmax(0, 1fr) 28px minmax(0, 1fr) 28px; align-items: center; gap: 12px 14px; min-height: 40px; font-size: 14px; }
.pets-rar-row + .pets-rar-row { border-top: 1px solid var(--edge-1); }
.pets-rar-row.head { min-height: 24px; font-size: 12.5px; color: var(--ink-3); }
.pets-rar-row.head span:nth-child(2), .pets-rar-row.head span:nth-child(3) { grid-column: span 2; }
.pets-rar-row .rar { font-size: 14px; }
@media (max-width: 920px) {
  .pets-pub-hero { min-height: 240px; padding: 14px 18px 24px; }
  .pets-pub-hero .ov-hello h1 { font-size: 28px; }
  /* The dashboard's side padding, which the tabs' scroll edge on a phone expects. */
  .pets-pub-page { padding: 0 18px 56px; gap: 28px; }
}
"#;

fn page(ctx: &Ctx, viewer: &MaybeUser, guild: u64, title: &str, active: &str, dash: &str, body: Markup) -> WebResult {
    let name = ctx.guild_name(guild);
    let icon = ctx.guild_icon_large(guild);
    let banner = ctx.guild_banner_url(guild);
    let frame = bot_core::web::banner::frame(ctx, guild);
    let can = viewer.0.as_ref().is_some_and(|u| ctx.level(u, guild, KEY) >= Level::View);
    let initial: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    let markup = html! {
        (DOCTYPE)
        html lang="en" {
            head { (ui::head(title)) }
            body.pets-pub data-guild=(guild) data-icon=[icon.as_deref()] data-hue="pets" {
                (view::style())
                style { (PreEscaped(PUBLIC_CSS)) }
                header.ov-hero.pets-pub-hero {
                    div.ov-bg {
                        @match (&banner, &icon) {
                            (Some(b), _) => img src=(b) alt="" style=[frame.map(|f| f.style())];,
                            (None, i) => (ui::art(i.as_deref(), false)),
                        }
                    }
                    div.ov-hello {
                        @match &icon {
                            Some(i) => img.ov-icon src=(i) alt="";,
                            None => span.ov-icon.letter { (initial) },
                        }
                        div {
                            h1 { (title) }
                            p { span { (name) } span { (ui::icon(Icon::Paw)) "Idleon companions" } }
                        }
                    }
                }
                main.pets-pub-page {
                    (ui::tabs(&[
                        ("Collection", format!("/p/{guild}/pets")),
                        ("Giveaways", format!("/p/{guild}/pet-giveaways")),
                        ("Stats", format!("/p/{guild}/pet-stats")),
                    ], active))
                    (body)
                    footer.pets-pub-foot {
                        span { "Kept by the staff of " (name) " on Discord." }
                        @if can { a href={ "/g/" (guild) "/" (dash) } { "Open in the dashboard" (ui::icon(Icon::Enter)) } }
                    }
                }
            }
        }
    };
    Ok(markup.into_response())
}

#[derive(Deserialize)]
struct CollectionQuery {
    owned: Option<String>,
    /// Only pets of this rarity.
    rarity: Option<String>,
    /// Only pets with Pet+ copies.
    plus: Option<String>,
}

/// Which pets the collection page shows.
#[derive(Clone, Copy, PartialEq)]
struct Filter<'a> {
    owned: bool,
    rarity: Option<&'a str>,
    plus: bool,
}

impl Filter<'_> {
    fn href(&self, guild: u64) -> String {
        let mut q: Vec<String> = Vec::new();
        if self.owned {
            q.push("owned=1".into());
        }
        if let Some(r) = self.rarity {
            q.push(format!("rarity={r}"));
        }
        if self.plus {
            q.push("plus=1".into());
        }
        if q.is_empty() { format!("/p/{guild}/pets") } else { format!("/p/{guild}/pets?{}", q.join("&")) }
    }
    fn keeps(&self, p: &Pet, counts: &HashMap<String, i64>, plus: &HashMap<String, i64>) -> bool {
        (!self.owned || counts.contains_key(&p.id)) && self.rarity.is_none_or(|r| store::clean_rarity(&p.rarity) == r) && (!self.plus || plus.contains_key(&p.id))
    }
}

/// The tags over the grid: every rarity, then Pet+, each with how many pets it would show.
fn tags(guild: u64, f: Filter, catalog: &[Pet], counts: &HashMap<String, i64>, plus: &HashMap<String, i64>) -> Markup {
    let n = |g: Filter| catalog.iter().filter(|p| g.keeps(p, counts, plus)).count();
    let all = Filter { rarity: None, ..f };
    let with_plus = Filter { plus: !f.plus, ..f };
    html! {
        nav.pets-tags aria-label="Filter by rarity" {
            a.pets-tag.on[f.rarity.is_none()] href=(all.href(guild)) aria-current=[f.rarity.is_none().then_some("true")] { "All rarities" span.n { (n(all)) } }
            @for r in store::RARITIES.iter().rev() {
                @let g = Filter { rarity: Some(r), ..f };
                @let on = f.rarity == Some(*r);
                @let k = n(g);
                @if k > 0 || on {
                    a class={ "pets-tag r-" (r) @if on { " on" } } href=(if on { all.href(guild) } else { g.href(guild) }) aria-current=[on.then_some("true")] {
                        (store::rarity_label(r)) span.n { (k) }
                    }
                }
            }
            @if !plus.is_empty() || f.plus {
                span.pets-tag-gap aria-hidden="true" {}
                a.pets-tag.plus.on[f.plus] href=(with_plus.href(guild)) aria-current=[f.plus.then_some("true")] data-tip="Pets the server has the Pet+ of, the upgraded version" {
                    b { "Pet+" } span.n { (n(Filter { plus: true, ..f })) }
                }
            }
        }
    }
}

async fn collection(State(ctx): State<Ctx>, viewer: MaybeUser, Path(guild): Path<u64>, Query(q): Query<CollectionQuery>) -> WebResult {
    check(&ctx, guild)?;
    let catalog = store::catalog(&ctx, guild)?;
    let counts = store::owned_counts(&ctx, guild)?;
    let plus = store::plus_counts(&ctx, guild)?;
    let total: i64 = counts.values().sum();
    let only_owned = q.owned.is_some();
    let f = Filter {
        owned: only_owned,
        rarity: q.rarity.as_deref().and_then(|r| store::RARITIES.iter().copied().find(|x| *x == r)),
        plus: q.plus.is_some(),
    };
    let filtered = f.rarity.is_some() || f.plus;
    let legendary: i64 = catalog.iter().filter(|p| p.rarity == "legendary").map(|p| counts.get(&p.id).copied().unwrap_or(0)).sum();
    let week = store::first_added_since(&ctx, guild, now_ms() - 7 * 86_400_000)?;
    let groups = view::groups(&catalog);
    // The rarest pets the server has, for the spotlight.
    let mut stars: Vec<&Pet> = catalog.iter().filter(|p| counts.contains_key(&p.id) && store::rarity_rank(&p.rarity) >= 2).collect();
    stars.sort_by_key(|p| (std::cmp::Reverse(store::rarity_rank(&p.rarity)), std::cmp::Reverse(counts.get(&p.id).copied().unwrap_or(0))));
    stars.truncate(4);
    let shown: Vec<(&String, Vec<&&Pet>, usize)> = groups
        .iter()
        .map(|(cat, list)| (cat, list.iter().filter(|p| f.keeps(p, &counts, &plus)).collect::<Vec<_>>(), list.iter().filter(|p| counts.contains_key(&p.id)).count()))
        .collect();
    let starts: Vec<usize> = shown.iter().scan(0, |n, (_, l, _)| { let at = *n; *n += l.len(); Some(at) }).collect();
    let body = html! {
        (view::collection_hero(&groups, &counts, week))
        @if !stars.is_empty() && !filtered {
            (view::section("Rarest in the collection", None, None, html! {
                div.pets-spot {
                    @for (i, p) in stars.iter().enumerate() {
                        @let r = store::clean_rarity(&p.rarity);
                        div class={ "pets-star r-" (r) } style={ "--i:" (i) } {
                            span.n { "×" (counts.get(&p.id).copied().unwrap_or(0)) }
                            (view::pet_art(p, "xl"))
                            div.t {
                                strong { (p.name) }
                                @if let Some(k) = plus.get(&p.id) { span.pets-plus { "Pet+" @if *k > 1 { " ×" (k) } } }
                                (view::rarity(r))
                                @if !p.bonus.is_empty() { p { (p.bonus) } } @else if !p.description.is_empty() { p { (p.description) } }
                            }
                        }
                    }
                }
            }))
        }
        (view::section("Every companion", Some(&if total > 0 {
            let kinds = counts.len() as i64;
            format!(
                "The server has {} {} of {} {}{}.",
                text::num(total), if total == 1 { "copy" } else { "copies" },
                text::num(kinds), if kinds == 1 { "pet" } else { "pets" },
                if legendary > 0 { format!(", {} of them legendary", text::num(legendary)) } else { String::new() }
            )
        } else {
            "Faded pets aren't in the collection yet.".to_owned()
        }), Some(html! {
            nav.seg aria-label="Which pets" {
                a.on[!only_owned] href=(Filter { owned: false, ..f }.href(guild)) aria-current=[(!only_owned).then_some("true")] { "Every companion" }
                a.on[only_owned] href=(Filter { owned: true, ..f }.href(guild)) aria-current=[only_owned.then_some("true")] { "Only owned" }
            }
        }), html! {
            (tags(guild, f, &catalog, &counts, &plus))
            @if total == 0 && !only_owned && !filtered {
                (ui::empty(Icon::Paw, "The collection is empty", "Members add pets with /pet add in Discord, and they show here once staff approve them."))
            } @else if shown.iter().all(|(_, l, _)| l.is_empty()) {
                (ui::empty(Icon::Search, "No pets match", &match (f.rarity, f.plus, only_owned) {
                    (_, true, _) => "The server has no Pet+ of these yet. Pick another tag, or All rarities.".to_owned(),
                    (Some(r), _, true) => format!("The server has no {} pets yet. Show every companion to see them all.", store::rarity_label(r).to_lowercase()),
                    _ => "Pick another tag, or All rarities.".to_owned(),
                }))
            }
        }))
        @for (k, (cat, pets, have)) in shown.iter().enumerate() {
            @if !pets.is_empty() {
                (ui::section(cat, Some(&format!("{have} of {} collected", groups[k].1.len())), html! {
                    div.pets-grid {
                        @for (n, p) in pets.iter().enumerate() { (view::collection_tile(p, counts.get(&p.id).copied().unwrap_or(0), plus.get(&p.id).copied().unwrap_or(0), starts[k] + n, None)) }
                    }
                }))
            }
        }
    };
    page(&ctx, &viewer, guild, "Pet collection", "Collection", "pets", body)
}

fn waiting_card(ctx: &Ctx, guild: u64, g: &Giveaway, image: Option<&str>) -> Markup {
    view::giveaway_card(ctx, guild, g, image, view::clock(g.expires_at), None)
}

async fn giveaways(State(ctx): State<Ctx>, viewer: MaybeUser, Path(guild): Path<u64>) -> WebResult {
    check(&ctx, guild)?;
    let active = store::active_giveaways(&ctx, guild)?;
    let past = store::past_giveaways(&ctx, guild, 30)?;
    let catalog = store::catalog(&ctx, guild)?;
    let image = |id: &str| catalog.iter().find(|p| p.id == id).and_then(Pet::image_url);
    let s = store::stats(&store::all_giveaways(&ctx, guild)?);
    let mut readings = vec![("waiting for a hand-over", text::num(s.active)), ("confirmed", text::num(s.confirmed))];
    if let Some(r) = s.confirmation_rate() {
        readings.push(("confirmation rate", format!("{r}%")));
    }
    let body = html! {
        (view::readings(&readings))
        (view::section("Waiting for a hand-over", Some("The giver passes the pet in game, then staff confirm it. Unconfirmed giveaways are cancelled when they expire."), None, html! {
            @if active.is_empty() {
                (ui::empty(Icon::Gift, "Nothing waiting", "Giveaways show here from the moment staff record them until they're confirmed."))
            } @else {
                div.pets-gvs { @for g in &active { (waiting_card(&ctx, guild, g, image(&g.pet_id).as_deref())) } }
            }
        }))
        (view::section("Recent", None, None, html! {
            @if past.is_empty() {
                (ui::empty(Icon::Log, "No finished giveaways yet", "Confirmed, expired and cancelled giveaways are listed here."))
            } @else {
                div.pets-list { @for g in &past { (view::giveaway_row(&ctx, guild, g, image(&g.pet_id).as_deref(), None)) } }
            }
        }))
    };
    page(&ctx, &viewer, guild, "Pet giveaways", "Giveaways", "pets/giveaways", body)
}

async fn stats(State(ctx): State<Ctx>, viewer: MaybeUser, Path(guild): Path<u64>) -> WebResult {
    check(&ctx, guild)?;
    let all = store::all_giveaways(&ctx, guild)?;
    let s = store::stats(&all);
    let catalog = store::catalog(&ctx, guild)?;
    let counts = store::owned_counts(&ctx, guild)?;
    let confirmed: Vec<&Giveaway> = all.iter().filter(|g| g.status == Status::Confirmed).collect();
    let distinct_given = confirmed.iter().map(|g| g.pet_id.as_str()).collect::<HashSet<_>>().len();
    // Every giver, not the stats' top ten.
    let givers = confirmed.iter().map(|g| g.giver_id).collect::<HashSet<_>>().len();
    let weeks = weekly(&all, 12);
    let held = |r: &str| catalog.iter().filter(|p| p.rarity == r).map(|p| counts.get(&p.id).copied().unwrap_or(0)).sum::<i64>();
    let given = |r: &str| s.rarities.iter().find(|(x, _)| x == r).map(|(_, n)| *n).unwrap_or(0);
    let rarities: Vec<(&str, i64, i64)> = store::RARITIES.iter().rev().map(|r| (*r, given(r), held(r))).collect();
    let max_given = rarities.iter().map(|r| r.1).max().unwrap_or(0).max(1);
    let max_held = rarities.iter().map(|r| r.2).max().unwrap_or(0).max(1);
    let pet = |name: &str| catalog.iter().find(|p| p.name == name);
    let mut figs = vec![("giveaways confirmed", text::num(s.confirmed))];
    if let Some(r) = s.confirmation_rate() {
        figs.push(("confirmation rate", format!("{r}%")));
    }
    figs.push(("givers", text::num(givers as i64)));
    figs.push(("different pets given", text::num(distinct_given as i64)));
    let pct = |v: i64, max: i64| if v > 0 { format!("width:{:.1}%", v as f64 * 100.0 / max as f64) } else { "width:0;min-width:0".to_owned() };
    let top_giver = s.top_givers.first().map(|t| t.1).unwrap_or(1).max(1);
    let top_pet = s.top_pets.first().map(|t| t.2).unwrap_or(1).max(1);
    let body = html! {
        div.pets-st {
            div.pets-st-col {
                section.pets-panel {
                    div.pets-panel-head { h2 { "Confirmed giveaways" } p { "Each week of the last 12, in UTC." } }
                    div.pets-figs { @for (label, v) in &figs { div { b { (v) } span { (label) } } } }
                    @if s.confirmed == 0 {
                        (ui::empty(Icon::Chart, "No confirmed giveaways yet", "The chart fills in as staff confirm pet giveaways."))
                    } @else {
                        (ui::area_chart_series(&weeks, "giveaways"))
                    }
                    @if s.expired + s.cancelled > 0 {
                        p.pets-note-line { (text::num(s.expired)) " expired and " (text::num(s.cancelled)) " cancelled. The rate counts finished giveaways only." }
                    }
                }
                section.pets-panel {
                    div.pets-panel-head { h2 { "By rarity" } p { "Pets given in confirmed giveaways, and copies the server holds." } }
                    div.pets-rar {
                        div.pets-rar-row.head aria-hidden="true" { span {} span { "Given" } span { "In the collection" } }
                        @for (r, g, h) in &rarities {
                            div class={ "pets-rar-row r-" (r) } {
                                (view::rarity(r))
                                span.pets-thin { span style=(pct(*g, max_given)) {} }
                                span.v aria-label={ (g) " given" } { (text::num(*g)) }
                                span.pets-thin { span style=(pct(*h, max_held)) {} }
                                span.v aria-label={ (h) " held" } { (text::num(*h)) }
                            }
                        }
                    }
                }
            }
            div.pets-st-col {
                section.pets-panel {
                    div.pets-panel-head { h2 { "Top givers" } p { "Confirmed giveaways each." } }
                    @if s.top_givers.is_empty() {
                        p.pets-note-line { "Members show here once one of their giveaways is confirmed." }
                    } @else {
                        div.pets-rank {
                            @for (i, (u, n)) in s.top_givers.iter().take(8).enumerate() {
                                div.pets-rank-row {
                                    span.i { (i + 1) }
                                    img.avatar src=(ctx.member_avatar(guild, *u)) alt="" loading="lazy";
                                    span.nm { (ctx.display_name(guild, *u)) }
                                    span.pets-thin { span style=(pct(*n, top_giver)) {} }
                                    span.v { (text::num(*n)) }
                                }
                            }
                        }
                    }
                }
                section.pets-panel {
                    div.pets-panel-head { h2 { "Most given" } p { "Pets that changed hands most." } }
                    @if s.top_pets.is_empty() {
                        p.pets-note-line { "Pets show here once a giveaway of them is confirmed." }
                    } @else {
                        div.pets-rank {
                            @for (i, (name, emoji, n)) in s.top_pets.iter().take(8).enumerate() {
                                @let p = pet(name);
                                @let r = p.map(|p| p.rarity.as_str()).unwrap_or("common");
                                div.pets-rank-row {
                                    span.i { (i + 1) }
                                    (view::art(emoji, p.and_then(Pet::image_url).as_deref(), r, "sm"))
                                    span.nm { (name) }
                                    span.pets-thin { span style=(pct(*n, top_pet)) {} }
                                    span.v { (text::num(*n)) }
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    page(&ctx, &viewer, guild, "Pet giveaway stats", "Stats", "pets/giveaways", body)
}

/// Confirmed giveaways per week for the last `weeks` weeks (Monday to Sunday, UTC), oldest
/// first, each labelled by its Monday ("Oct 3"); the current week is "This week".
fn weekly(all: &[Giveaway], weeks: i64) -> Vec<(String, i64)> {
    let tz = jiff::tz::TimeZone::UTC;
    let today = jiff::Timestamp::now().to_zoned(tz.clone()).date();
    let monday = today.checked_sub(jiff::Span::new().days(i64::from(today.weekday().to_monday_zero_offset()))).unwrap_or(today);
    let mut per: HashMap<jiff::civil::Date, i64> = HashMap::new();
    for g in all.iter().filter(|g| g.status == Status::Confirmed) {
        let at = g.closed_at.unwrap_or(g.recorded_at);
        if let Ok(ts) = jiff::Timestamp::from_millisecond(at) {
            let d = ts.to_zoned(tz.clone()).date();
            let start = d.checked_sub(jiff::Span::new().days(i64::from(d.weekday().to_monday_zero_offset()))).unwrap_or(d);
            *per.entry(start).or_default() += 1;
        }
    }
    (0..weeks)
        .rev()
        .filter_map(|ago| monday.checked_sub(jiff::Span::new().weeks(ago)).ok().map(|d| (ago, d)))
        .map(|(ago, d)| (if ago == 0 { "This week".to_owned() } else { format!("{} {}", month(d.month()), d.day()) }, per.get(&d).copied().unwrap_or(0)))
        .collect()
}

fn month(m: i8) -> &'static str {
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"].get((m - 1).max(0) as usize).copied().unwrap_or("")
}
