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
.pets-pub .chart-card { padding: 20px 22px 16px; }
/* Stats: the chart beside the podium, then the breakdowns three across, each wrapping under the
   other when the page is narrow. */
.pets-duo, .pets-trio { display: flex; flex-wrap: wrap; align-items: flex-start; gap: 34px 40px; }
.pets-duo > * { flex: 1 1 440px; min-width: 0; }
.pets-duo > :first-child { flex-grow: 1.4; }
.pets-trio > * { flex: 1 1 300px; min-width: 0; }
@container col (min-width: 1000px) { .pets-duo .chart svg { height: 200px; } }
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
                            (Some(b), _) => img src=(b) alt="";,
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
}

async fn collection(State(ctx): State<Ctx>, viewer: MaybeUser, Path(guild): Path<u64>, Query(q): Query<CollectionQuery>) -> WebResult {
    check(&ctx, guild)?;
    let catalog = store::catalog(&ctx, guild)?;
    let counts = store::owned_counts(&ctx, guild)?;
    let total: i64 = counts.values().sum();
    let only_owned = q.owned.is_some();
    let legendary: i64 = catalog.iter().filter(|p| p.rarity == "legendary").map(|p| counts.get(&p.id).copied().unwrap_or(0)).sum();
    let week = store::first_added_since(&ctx, guild, now_ms() - 7 * 86_400_000)?;
    let groups = view::groups(&catalog);
    // The rarest pets the server has, for the spotlight.
    let mut stars: Vec<&Pet> = catalog.iter().filter(|p| counts.contains_key(&p.id) && store::rarity_rank(&p.rarity) >= 2).collect();
    stars.sort_by_key(|p| (std::cmp::Reverse(store::rarity_rank(&p.rarity)), std::cmp::Reverse(counts.get(&p.id).copied().unwrap_or(0))));
    stars.truncate(4);
    let shown: Vec<(&String, Vec<&&Pet>, usize)> = groups
        .iter()
        .map(|(cat, list)| (cat, list.iter().filter(|p| !only_owned || counts.contains_key(&p.id)).collect::<Vec<_>>(), list.iter().filter(|p| counts.contains_key(&p.id)).count()))
        .collect();
    let starts: Vec<usize> = shown.iter().scan(0, |n, (_, l, _)| { let at = *n; *n += l.len(); Some(at) }).collect();
    let body = html! {
        (view::collection_hero(&groups, &counts, week))
        @if !stars.is_empty() {
            (view::section("Rarest in the collection", None, None, html! {
                div.pets-spot {
                    @for (i, p) in stars.iter().enumerate() {
                        @let r = store::clean_rarity(&p.rarity);
                        div class={ "pets-star r-" (r) } style={ "--i:" (i) } {
                            span.n { "×" (counts.get(&p.id).copied().unwrap_or(0)) }
                            (view::pet_art(p, "xl"))
                            div.t {
                                strong { (p.name) }
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
                a.on[!only_owned] href={ "/p/" (guild) "/pets" } aria-current=[(!only_owned).then_some("true")] { "Every companion" }
                a.on[only_owned] href={ "/p/" (guild) "/pets?owned=1" } aria-current=[only_owned.then_some("true")] { "Only owned" }
            }
        }), html! {
            @if total == 0 {
                (ui::empty(Icon::Paw, "The collection is empty", "Members add pets with /pet add in Discord, and they show here once staff approve them."))
            }
        }))
        @for (k, (cat, pets, have)) in shown.iter().enumerate() {
            @if !pets.is_empty() {
                (ui::section(cat, Some(&format!("{have} of {} collected", groups[k].1.len())), html! {
                    div.pets-grid {
                        @for (n, p) in pets.iter().enumerate() { (view::collection_tile(p, counts.get(&p.id).copied().unwrap_or(0), starts[k] + n)) }
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
    let series = daily(&all, 30);
    let collection_rarity: Vec<(Markup, i64, &str)> = store::RARITIES
        .iter()
        .rev()
        .map(|r| (view::rarity(r), catalog.iter().filter(|p| p.rarity == *r).map(|p| counts.get(&p.id).copied().unwrap_or(0)).sum::<i64>(), *r))
        .collect();
    let mut readings = vec![("giveaways confirmed", text::num(s.confirmed))];
    if let Some(r) = s.confirmation_rate() {
        readings.push(("confirmation rate", format!("{r}%")));
    }
    readings.push(("givers", text::num(givers as i64)));
    readings.push(("different pets given", text::num(distinct_given as i64)));
    let podium: Vec<(usize, &(u64, i64))> = [(2usize, 1usize), (1, 0), (3, 2)].into_iter().filter_map(|(place, i)| s.top_givers.get(i).map(|t| (place, t))).collect();
    let pet_rarity = |name: &str| catalog.iter().find(|p| p.name == name).map(|p| p.rarity.clone()).unwrap_or_else(|| "common".into());
    let body = html! {
        (view::readings(&readings))
        @let by_rarity = view::section("The collection, by rarity", Some("Copies the server holds of each rarity."), None, view::hbars(&collection_rarity));
        @if s.confirmed == 0 {
            (ui::empty(Icon::Chart, "No confirmed giveaways yet", "Stats fill in as staff confirm pet giveaways."))
            (by_rarity)
        } @else {
            div.pets-duo {
                (view::section("Confirmed giveaways", Some("Each day of the last 30, in UTC."), None, html! {
                    div.card.chart-card { (ui::bar_chart(&series, "var(--data)", "giveaways")) }
                }))
                (view::section("Top givers", Some("Members whose giveaways went through."), None, html! {
                    div.podium {
                        @for (place, (u, n)) in &podium {
                            div class={ "step p" (place) } {
                                img.avatar src=(ctx.member_avatar(guild, *u)) alt="" loading="lazy";
                                strong { (ctx.display_name(guild, *u)) }
                                span.muted.small { (text::num(*n)) (if *n == 1 { " pet given" } else { " pets given" }) }
                                div.block { (place) }
                            }
                        }
                    }
                    @if s.top_givers.len() > 3 {
                        (view::hbars(&s.top_givers.iter().skip(3).map(|(u, n)| (view::person(&ctx, guild, *u), *n, "")).collect::<Vec<_>>()))
                    }
                }))
            }
            div.pets-trio {
                (view::section("Most given", None, None, view::hbars(&s.top_pets.iter().map(|(name, emoji, n)| {
                    let r = pet_rarity(name);
                    (html! { span.pc { (view::art(emoji, None, &r, "sm")) span { (name) } } }, *n, "")
                }).collect::<Vec<_>>())))
                @if !s.rarities.is_empty() {
                    (view::section("Given, by rarity", None, None, view::hbars(&s.rarities.iter().map(|(r, n)| (view::rarity(r), *n, r.as_str())).collect::<Vec<_>>())))
                }
                (by_rarity)
            }
        }
        @if s.expired + s.cancelled > 0 {
            p.muted.small { (text::num(s.expired)) " expired and " (text::num(s.cancelled)) " cancelled. The rate counts finished giveaways only." }
        }
    };
    page(&ctx, &viewer, guild, "Pet giveaway stats", "Stats", "pets/giveaways", body)
}

/// Confirmed giveaways per day for the last `days` days, oldest first, labelled "Oct 3".
fn daily(all: &[Giveaway], days: i64) -> Vec<(String, i64)> {
    let tz = jiff::tz::TimeZone::UTC;
    let today = jiff::Timestamp::now().to_zoned(tz.clone()).date();
    let mut per: HashMap<jiff::civil::Date, i64> = HashMap::new();
    for g in all.iter().filter(|g| g.status == Status::Confirmed) {
        let at = g.closed_at.unwrap_or(g.recorded_at);
        if let Ok(ts) = jiff::Timestamp::from_millisecond(at) {
            *per.entry(ts.to_zoned(tz.clone()).date()).or_default() += 1;
        }
    }
    (0..days)
        .rev()
        .filter_map(|back| today.checked_sub(jiff::Span::new().days(back)).ok())
        .map(|d| (format!("{} {}", month(d.month()), d.day()), per.get(&d).copied().unwrap_or(0)))
        .collect()
}

fn month(m: i8) -> &'static str {
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"].get((m - 1).max(0) as usize).copied().unwrap_or("")
}
