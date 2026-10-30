//! Public pages, no sign-in: the collection, giveaways and giveaway stats. Served only for
//! servers where the feature is on.

use std::collections::HashMap;

use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::get;
use bot_core::maud::{Markup, html};
use bot_core::web::auth::MaybeUser;
use bot_core::web::ui;
use bot_core::web::{Level, WebError, WebResult};
use bot_core::{Ctx, Icon, jiff, text};
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

fn nav(guild: u64, active: &str) -> Markup {
    html! {
        (view::style())
        (ui::tabs(&[
            ("Collection", format!("/p/{guild}/pets")),
            ("Giveaways", format!("/p/{guild}/pet-giveaways")),
            ("Stats", format!("/p/{guild}/pet-stats")),
        ], active))
    }
}

/// A quiet link to the dashboard for signed-in people who can open it.
fn footer(ctx: &Ctx, viewer: &MaybeUser, guild: u64, path: &str) -> Markup {
    let can = viewer.0.as_ref().is_some_and(|u| ctx.level(u, guild, KEY) >= Level::View);
    html! {
        @if can {
            p.muted.small { a href={ "/g/" (guild) "/" (path) } style="text-decoration:underline;text-underline-offset:3px" { "Open in the dashboard" } }
        }
    }
}

fn page(ctx: &Ctx, guild: u64, title: &str, body: Markup) -> WebResult {
    let subtitle = format!("{} · Idleon companions", ctx.guild_name(guild));
    Ok(ui::public_layout(title, Some(&subtitle), body).into_response())
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
    let cats = store::categories(&ctx, guild)?;
    let body = html! {
        (nav(guild, "Collection"))
        (ui::readings(&[
            ("pets in the collection", text::num(total)),
            ("companions collected", format!("{} of {}", counts.len(), catalog.len())),
            ("legendary", text::num(legendary)),
        ]))
        @if total == 0 {
            (ui::empty(Icon::Paw, "The collection is empty", "Members add pets with /pet add in Discord, and they show here once staff approve them."))
        }
        p.small.muted {
            @if only_owned {
                "Showing pets the server owns. " a href={ "/p/" (guild) "/pets" } style="color:var(--ink);text-decoration:underline;text-underline-offset:3px" { "Show every companion" }
            } @else {
                "Faded pets aren't in the collection yet. " a href={ "/p/" (guild) "/pets?owned=1" } style="color:var(--ink);text-decoration:underline;text-underline-offset:3px" { "Show only owned" }
            }
        }
        @for cat in &cats {
            @let pets: Vec<&Pet> = catalog.iter().filter(|p| &p.category == cat && (!only_owned || counts.contains_key(&p.id))).collect();
            @let all = catalog.iter().filter(|p| &p.category == cat).count();
            @let have = catalog.iter().filter(|p| &p.category == cat && counts.contains_key(&p.id)).count();
            @if !pets.is_empty() {
                (ui::section(cat, Some(&format!("{have} of {all} collected")), html! {
                    div.pgrid {
                        @for p in &pets {
                            @let n = counts.get(&p.id).copied().unwrap_or(0);
                            div.pcard.missing[n == 0] {
                                (view::pet_tile(p, true))
                                div.t {
                                    div.n {
                                        span.clip { (p.name) }
                                        @if n > 0 { b title={ (n) " in the collection" } { "×" (n) } } @else { i { "Not yet" } }
                                    }
                                    (view::rarity(&p.rarity))
                                    @if !p.description.is_empty() { p { (p.description) } }
                                    @if !p.bonus.is_empty() { p.bonus { (p.bonus) } }
                                }
                            }
                        }
                    }
                }))
            }
        }
        (footer(&ctx, &viewer, guild, "pets/catalog"))
    };
    page(&ctx, guild, "Pet collection", body)
}

fn giveaway_row(ctx: &Ctx, guild: u64, g: &Giveaway) -> Markup {
    html! {
        div.gv {
            (view::tile(g.icon(), None, false))
            div.t {
                span { b { (g.pet_name) } " " (view::rarity(&g.pet_rarity)) }
                span.who { (ctx.display_name(guild, g.giver_id)) " to " (ctx.display_name(guild, g.winner_id)) }
            }
            div.gv-side {
                @if g.status == Status::Pending {
                    @let (line, close) = view::expiry(g.expires_at);
                    (view::status_badge(g.status))
                    span.soon[close] { (line) }
                } @else {
                    (view::status_badge(g.status))
                    span { (text::ago(g.closed_at.unwrap_or(g.recorded_at))) }
                }
            }
        }
    }
}

async fn giveaways(State(ctx): State<Ctx>, viewer: MaybeUser, Path(guild): Path<u64>) -> WebResult {
    check(&ctx, guild)?;
    let active = store::active_giveaways(&ctx, guild)?;
    let past = store::past_giveaways(&ctx, guild, 30)?;
    let s = store::stats(&store::all_giveaways(&ctx, guild)?);
    let body = html! {
        (nav(guild, "Giveaways"))
        (ui::readings(&[
            ("waiting for a hand-over", text::num(s.active)),
            ("confirmed", text::num(s.confirmed)),
            ("confirmation rate", s.confirmation_rate().map(|r| format!("{r}%")).unwrap_or_else(|| "–".into())),
        ]))
        (ui::section("Waiting for a hand-over", Some("The giver passes the pet in game, then staff confirm it. Unconfirmed giveaways are cancelled when they expire."), html! {
            @if active.is_empty() {
                (ui::empty(Icon::Gift, "Nothing waiting", "Giveaways show here from the moment staff record them until they're confirmed."))
            } @else {
                div { @for g in &active { (giveaway_row(&ctx, guild, g)) } }
            }
        }))
        (ui::section("Recent", None, html! {
            @if past.is_empty() {
                (ui::empty(Icon::Log, "No finished giveaways yet", "Confirmed, expired and cancelled giveaways are listed here."))
            } @else {
                div { @for g in &past { (giveaway_row(&ctx, guild, g)) } }
            }
        }))
        (footer(&ctx, &viewer, guild, "pets/giveaways"))
    };
    page(&ctx, guild, "Pet giveaways", body)
}

async fn stats(State(ctx): State<Ctx>, viewer: MaybeUser, Path(guild): Path<u64>) -> WebResult {
    check(&ctx, guild)?;
    let all = store::all_giveaways(&ctx, guild)?;
    let s = store::stats(&all);
    let catalog = store::catalog(&ctx, guild)?;
    let counts = store::owned_counts(&ctx, guild)?;
    let distinct_given = {
        let mut ids: Vec<&str> = all.iter().filter(|g| g.status == Status::Confirmed).map(|g| g.pet_id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        ids.len()
    };
    let series = daily(&all, 30);
    let collection_rarity: Vec<(Markup, i64)> = store::RARITIES
        .iter()
        .rev()
        .map(|r| (view::rarity(r), catalog.iter().filter(|p| p.rarity == *r).map(|p| counts.get(&p.id).copied().unwrap_or(0)).sum::<i64>()))
        .collect();
    let body = html! {
        (nav(guild, "Stats"))
        (ui::readings(&[
            ("giveaways confirmed", text::num(s.confirmed)),
            ("confirmation rate", s.confirmation_rate().map(|r| format!("{r}%")).unwrap_or_else(|| "–".into())),
            ("givers", text::num(s.top_givers.len() as i64)),
            ("different pets given", text::num(distinct_given as i64)),
        ]))
        @if s.confirmed == 0 {
            (ui::empty(Icon::Chart, "No confirmed giveaways yet", "Stats fill in as staff confirm pet giveaways."))
        } @else {
            (ui::section("Confirmed giveaways", Some("Last 30 days."), ui::bar_chart(&series, "var(--data)", "giveaways")))
            div.split {
                (ui::section("Top givers", None, view::hbars(&s.top_givers.iter().map(|(u, n)| (view::person(&ctx, guild, *u), *n)).collect::<Vec<_>>())))
                (ui::section("Most given", None, view::hbars(&s.top_pets.iter().map(|(name, emoji, n)| (html! { span.pc { (view::tile(emoji, None, false)) span { (name) } } }, *n)).collect::<Vec<_>>())))
            }
        }
        div.split {
            @if !s.rarities.is_empty() {
                (ui::section("Given, by rarity", None, view::hbars(&s.rarities.iter().map(|(r, n)| (view::rarity(r), *n)).collect::<Vec<_>>())))
            }
            (ui::section("Collection, by rarity", None, view::hbars(&collection_rarity)))
        }
        @if s.expired + s.cancelled > 0 {
            p.muted.small { (text::num(s.expired)) " expired and " (text::num(s.cancelled)) " cancelled. The rate counts finished giveaways only." }
        }
        (footer(&ctx, &viewer, guild, "pets/giveaways"))
    };
    page(&ctx, guild, "Pet giveaway stats", body)
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
