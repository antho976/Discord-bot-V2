//! Dashboard pages: overview and settings, approvals, catalog, giveaways and the giver ban list.

use std::collections::HashMap;

use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::twilight_model::id::Id;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};
use serde::Deserialize;

use crate::store::{self, Giveaway, Pet, Status};
use crate::view;
use crate::{KEY, Settings};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/pets", get(overview).post(save_settings))
        .route("/g/{guild}/pets/approvals", get(approvals))
        .route("/g/{guild}/pets/approvals/approve", post(approve))
        .route("/g/{guild}/pets/approvals/reject", post(reject))
        .route("/g/{guild}/pets/catalog", get(catalog).post(save_pet))
        .route("/g/{guild}/pets/catalog/{id}/delete", post(delete_pet))
        .route("/g/{guild}/pets/catalog/{id}/owned", post(change_owned))
        .route("/g/{guild}/pets/categories", post(add_category))
        .route("/g/{guild}/pets/categories/delete", post(delete_category))
        .route("/g/{guild}/pets/giveaways", get(giveaways).post(record_giveaway))
        .route("/g/{guild}/pets/giveaways/{id}/confirm", post(confirm_giveaway))
        .route("/g/{guild}/pets/giveaways/{id}/cancel", post(cancel_giveaway))
        .route("/g/{guild}/pets/giveaways/{id}/delete", post(delete_giveaway))
        .route("/g/{guild}/pets/giveaways/{id}/comments", post(add_comment))
        .route("/g/{guild}/pets/giveaways/{id}/comments/{comment}/delete", post(delete_comment))
        .route("/g/{guild}/pets/bans", get(bans).post(add_ban))
        .route("/g/{guild}/pets/bans/{user}/delete", post(remove_ban))
}

fn open(ctx: &Ctx, user: &User, guild: u64, need: Level) -> WebResult<Page> {
    let page = ctx.page(user, guild, KEY, need)?;
    store::ensure_seeded(ctx, guild)?;
    Ok(page)
}

fn tabs(page: &Page, active: &str, waiting: i64) -> Markup {
    let approvals = if waiting > 0 { format!("Approvals ({waiting})") } else { "Approvals".to_owned() };
    let items = [
        ("Collection", page.path("pets")),
        (approvals.as_str(), page.path("pets/approvals")),
        ("Giveaways", page.path("pets/giveaways")),
        ("Catalog", page.path("pets/catalog")),
        ("Bans", page.path("pets/bans")),
    ];
    let active = if active == "Approvals" { approvals.as_str() } else { active };
    html! { (view::style()) (ui::tabs(&items, active)) }
}

fn shell(ctx: &Ctx, page: &Page, body: Markup) -> WebResult {
    Ok(ui::layout(ctx, page, "Pets", None, body).into_response())
}

/// A member from a name as shown in the server, a username, a mention or an ID.
fn resolve_member(ctx: &Ctx, guild: u64, input: &str) -> Option<u64> {
    let raw = input.trim().trim_start_matches("<@").trim_start_matches('!').trim_end_matches('>').trim_start_matches('@');
    if let Ok(id) = raw.parse::<u64>() {
        return (id > 0).then_some(id);
    }
    let needle = raw.to_lowercase();
    if needle.is_empty() {
        return None;
    }
    let members = ctx.cache.guild_members(Id::new(guild))?;
    let ids: Vec<u64> = members.iter().map(|u| u.get()).collect();
    ids.iter().copied().find(|u| ctx.display_name(guild, *u).to_lowercase() == needle).or_else(|| {
        ids.into_iter().find(|u| ctx.cache.user(Id::new(*u)).is_some_and(|x| x.name.to_lowercase() == needle))
    })
}

fn members_datalist(ctx: &Ctx, guild: u64) -> Markup {
    let mut names: Vec<String> = ctx
        .cache
        .guild_members(Id::new(guild))
        .map(|set| set.iter().take(2000).map(|u| ctx.display_name(guild, u.get())).collect())
        .unwrap_or_default();
    names.sort_by_key(|n| n.to_lowercase());
    html! { datalist #pets-members { @for n in &names { option value=(n) {} } } }
}

// ---- collection (the first tab) ----

const DAY_MS: i64 = 86_400_000;

const COLLECTION_CSS: &str = "
.pets-hero { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: 36px; align-items: center; }
.pets-ring-wrap { display: flex; align-items: center; gap: 22px; }
.pets-ring { position: relative; width: 88px; height: 88px; flex: none; }
.pets-ring svg { width: 100%; height: 100%; transform: rotate(-90deg); display: block; }
.pets-ring span { position: absolute; inset: 0; display: grid; place-items: center; font-size: 17px; font-weight: 500; letter-spacing: -0.01em; }
.pets-count { display: flex; flex-direction: column; gap: 3px; }
.pets-count .muted { font-size: 13px; }
.pets-count .small { color: var(--live); }
.pets-cats { display: flex; flex-direction: column; gap: 12px; min-width: 0; }
.pets-cat { display: grid; grid-template-columns: minmax(96px, 150px) minmax(0, 1fr) 40px; align-items: center; gap: 16px; font-size: 13px; }
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
.pets-req { display: grid; grid-template-columns: 32px minmax(0, 1fr); gap: 10px; align-items: start; font-size: 13px; }
.pets-req strong { font-weight: 500; }
.pets-req-acts { display: flex; gap: 8px; align-items: center; margin-top: 8px; flex-wrap: wrap; }
.pets-req-acts form { display: inline; }
.pets-gv { display: grid; grid-template-columns: auto minmax(0, 1fr) auto; gap: 12px; align-items: center; font-size: 13px; }
.pets-pair { display: inline-flex; align-items: center; gap: 4px; color: var(--ink-3); }
.pets-pair .avatar { width: 26px; height: 26px; }
.pets-arrow { flex: none; margin: 0 1px; }
.pets-gv .grow { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.pets-gv-state { font-size: 12px; white-space: nowrap; color: var(--ink-3); }
.pets-gv-state.soon { color: var(--waiting); }
.pets-gv-state.ok { color: var(--live); }
.pets-gv-state.bad { color: var(--held); }
.pets-foot { font-size: 12px; }
@media (max-width: 820px) {
  .pets-hero { grid-template-columns: 1fr; gap: 24px; }
}
";

fn collection_tile(p: &Pet, count: i64) -> Markup {
    html! {
        div.pets-tile.missing[count == 0] title=(p.name) {
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

/// `"in 2 h"` style for a giveaway's expiry, read as "2 h left".
fn left_text(expires_at: Option<i64>) -> String {
    match expires_at {
        Some(t) => {
            let secs = (t - now_ms()) / 1000;
            if secs < 60 { "under a minute left".into() } else { format!("{} left", text::duration(secs)) }
        }
        None => "no expiry".into(),
    }
}

async fn overview(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = open(&ctx, &user, guild, Level::View)?;
    let catalog = store::catalog(&ctx, guild)?;
    let counts = store::owned_counts(&ctx, guild)?;
    let pending = store::pending(&ctx, guild)?;
    let waiting: i64 = pending.iter().map(|p| p.quantity).sum();
    let active = store::active_giveaways(&ctx, guild)?;
    let past = store::past_giveaways(&ctx, guild, 3)?;
    let gstats = store::stats(&store::all_giveaways(&ctx, guild)?);
    let week = store::first_added_since(&ctx, guild, now_ms() - 7 * DAY_MS)?;
    let find = |id: &str| catalog.iter().find(|p| p.id == id).cloned().unwrap_or_else(|| missing_pet(id));

    let total = catalog.len() as i64;
    let collected = counts.len() as i64;
    let pct = if total > 0 { collected * 100 / total } else { 0 };
    // Categories in the order the catalog lists them, each with its pets.
    let mut groups: Vec<(String, Vec<&Pet>)> = Vec::new();
    for p in &catalog {
        match groups.iter_mut().find(|(c, _)| *c == p.category) {
            Some((_, list)) => list.push(p),
            None => groups.push((p.category.clone(), vec![p])),
        }
    }
    let actions = html! {
        a.btn.small href={ "/p/" (guild) "/pets" } target="_blank" rel="noopener" { "Public page" (ui::icon(Icon::External)) }
    };

    let body = html! {
        style { (PreEscaped(COLLECTION_CSS)) }
        (tabs(&page, "Collection", waiting))
        div.with-rail {
            div.stack.pets-main {
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
                        @for (name, list) in &groups {
                            @let got = list.iter().filter(|p| counts.contains_key(&p.id)).count();
                            div.pets-cat {
                                span.clip { (name) }
                                span.meter { span style={ "width:" (if list.is_empty() { 0 } else { got * 100 / list.len() }) "%" } {} }
                                span.mono { (got) "/" (list.len()) }
                            }
                        }
                    }
                }

                @for (i, (name, list)) in groups.iter().enumerate() {
                    (ui::section(name, if i == 0 { Some("Faded ones aren't in the collection yet") } else { None }, html! {
                        div.pets-grid {
                            @for p in list {
                                (collection_tile(p, counts.get(&p.id).copied().unwrap_or(0)))
                            }
                        }
                    }))
                }
                @if groups.is_empty() {
                    (ui::empty(Icon::Paw, "No companions yet", "The catalog fills from the server's list. Add pets in the Catalog tab."))
                }
            }

            aside.rail {
                div.rail-block {
                    h2 { "Waiting for you" @if !pending.is_empty() { span.count-pill { (pending.len()) } } }
                    @if pending.is_empty() {
                        p.muted.small { "Submissions from /pet add land here. Approve or reject them from here or the Approvals tab." }
                    }
                    @for p in pending.iter().take(3) {
                        @let pet = find(&p.pet_id);
                        div.pets-req {
                            img.avatar src=(ctx.member_avatar(guild, p.user_id)) alt="" width="32" height="32" loading="lazy";
                            div {
                                div { strong { (ctx.display_name(guild, p.user_id)) } " added " (pet.icon()) " " (pet.name) @if p.quantity > 1 { " ×" (p.quantity) } }
                                div.muted.small { (text::ago(p.requested_at)) }
                                @if page.can_edit() {
                                    div.pets-req-acts {
                                        form method="post" action=(page.path("pets/approvals/approve")) {
                                            input type="hidden" name="only" value=(p.id);
                                            input type="hidden" name="back" value="collection";
                                            button.btn.small.outline type="submit" { "Approve" }
                                        }
                                        form method="post" action=(page.path("pets/approvals/reject")) {
                                            input type="hidden" name="only" value=(p.id);
                                            input type="hidden" name="back" value="collection";
                                            button.btn.small type="submit" { "Reject" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                div.rail-block {
                    h2 { "Pet giveaways" }
                    @if active.is_empty() && past.is_empty() {
                        p.muted.small { "Giveaways you record show up here until they're confirmed or expire." }
                    }
                    @for g in active.iter().take(3).chain(past.iter().take(3)) {
                        div.pets-gv {
                            span.pets-pair {
                                img.avatar src=(ctx.member_avatar(guild, g.giver_id)) alt="" loading="lazy";
                                svg.pets-arrow viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" {
                                    path d="M3 8h9M8.5 4.5 12 8l-3.5 3.5" {}
                                }
                                img.avatar src=(ctx.member_avatar(guild, g.winner_id)) alt="" loading="lazy";
                            }
                            span.grow { (g.pet_emoji) " " (g.pet_name) }
                            @match g.status {
                                Status::Pending => span.pets-gv-state.soon { (left_text(g.expires_at)) },
                                Status::Confirmed => span.pets-gv-state.ok { "Confirmed" },
                                Status::Expired => span.pets-gv-state.bad { "Expired" },
                                Status::Cancelled => span.pets-gv-state { "Cancelled" },
                            }
                        }
                    }
                    @if gstats.confirmed > 0 {
                        p.muted.small.pets-foot {
                            (gstats.confirmed) " confirmed"
                            @if let Some(rate) = gstats.confirmation_rate() { " · " (rate) "% of giveaways go through" }
                        }
                    }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Pets", Some(actions), body).into_response())
}

async fn save_settings(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = open(&ctx, &user, guild, Level::Edit)?;
    let s = Settings {
        announce_channel: form.channel(&ctx, guild, "announce_channel"),
        expiry_hours: form.int_in("expiry_hours", 0, 720, 24),
        dm_reviews: form.bool("dm_reviews"),
    };
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed pet settings");
    Ok(saved(&page.path("pets/giveaways")))
}

// ---- approvals ----

async fn approvals(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = open(&ctx, &user, guild, Level::View)?;
    let catalog = store::catalog(&ctx, guild)?;
    let find = |id: &str| catalog.iter().find(|p| p.id == id).cloned().unwrap_or_else(|| missing_pet(id));
    let pending = store::pending(&ctx, guild)?;
    let waiting: i64 = pending.iter().map(|p| p.quantity).sum();
    let done = store::reviewed(&ctx, guild, 20)?;
    let edit = page.can_edit();
    let approve = page.path("pets/approvals/approve");
    let reject = page.path("pets/approvals/reject");
    let body = html! {
        (tabs(&page, "Approvals", waiting))
        (ui::section("Waiting for approval", Some("Approved pets join the collection, credited to the member who submitted them."), html! {
            @if pending.is_empty() {
                (ui::empty(Icon::Paw, "Nothing to review", "Submissions from /pet add land here."))
            } @else {
                form method="post" action=(approve) {
                    // Enter in the reason box must not approve anything.
                    button type="submit" disabled hidden {}
                    div.table-wrap { table.table {
                        thead { tr {
                            @if edit { th style="width:28px" { input #sel-all type="checkbox" aria-label="Select all"; } }
                            th { "Pet" } th.num { "Qty" } th { "Submitted by" } th { "When" }
                            @if edit { th {} }
                        } }
                        tbody { @for p in &pending {
                            @let pet = find(&p.pet_id);
                            tr {
                                @if edit { td { input type="checkbox" name="sel" value=(p.id) aria-label={ "Select " (pet.name) }; } }
                                td { (view::pet_cell(&pet, Some(store::rarity_label(&pet.rarity)))) }
                                td.num { (p.quantity) }
                                td { (view::person(&ctx, guild, p.user_id)) }
                                td.muted { (text::ago(p.requested_at)) }
                                @if edit {
                                    td.actions {
                                        button.btn.small type="submit" formaction=(approve) name="only" value=(p.id) { "Approve" }
                                        button.btn.small type="submit" formaction=(reject) name="only" value=(p.id) { "Reject" }
                                    }
                                }
                            }
                        } }
                    } }
                    @if edit {
                        div.bulk {
                            input.input type="text" name="reason" maxlength="300" placeholder="Reason for rejecting, sent to them (optional)" autocomplete="off";
                            button.btn.small type="submit" formaction=(approve) { "Approve selected" }
                            button.btn.small type="submit" formaction=(reject) { "Reject selected" }
                            button.btn.small type="submit" formaction=(approve) name="all" value="1" { "Approve all" }
                        }
                    }
                }
                script { (PreEscaped(r#"document.getElementById("sel-all")?.addEventListener("change",e=>document.querySelectorAll('input[name="sel"]').forEach(c=>c.checked=e.target.checked));"#)) }
            }
        }))
        @if !done.is_empty() {
            (ui::section("Recently reviewed", None, html! {
                div.table-wrap { table.table {
                    thead { tr { th { "Pet" } th { "Member" } th { "Result" } th { "By" } th { "When" } } }
                    tbody { @for p in &done {
                        @let pet = find(&p.pet_id);
                        tr {
                            td { (view::pet_cell(&pet, (!p.reason.is_empty()).then_some(p.reason.as_str()))) }
                            td { (view::person(&ctx, guild, p.user_id)) }
                            td { @if p.status == "approved" { (ui::badge(&format!("Approved{}", if p.quantity > 1 { format!(" ×{}", p.quantity) } else { String::new() }), Tone::Ok)) } @else { (ui::badge("Rejected", Tone::Bad)) } }
                            td.muted { (p.reviewed_by.as_deref().unwrap_or("")) }
                            td.muted { (p.reviewed_at.map(text::ago).unwrap_or_default()) }
                        }
                    } }
                } }
            }))
        }
    };
    shell(&ctx, &page, body)
}

fn missing_pet(id: &str) -> Pet {
    Pet { id: id.into(), name: id.into(), emoji: String::new(), category: String::new(), rarity: "common".into(), description: String::new(), bonus: String::new(), image: String::new() }
}

fn picked(ctx: &Ctx, guild: u64, form: &FormData) -> WebResult<Vec<i64>> {
    let ids: Vec<i64> = if form.bool("all") {
        store::pending(ctx, guild)?.into_iter().map(|p| p.id).collect()
    } else if let Some(one) = form.i64("only") {
        vec![one]
    } else {
        form.all("sel").into_iter().filter_map(|v| v.parse().ok()).collect()
    };
    if ids.is_empty() {
        return Err(WebError::Bad("Tick at least one submission first.".into()));
    }
    Ok(ids)
}

async fn approve(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    review(ctx, user, guild, form, true).await
}

async fn reject(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    review(ctx, user, guild, form, false).await
}

async fn review(ctx: Ctx, user: User, guild: u64, form: FormData, approve: bool) -> WebResult {
    let page = open(&ctx, &user, guild, Level::Edit)?;
    let ids = picked(&ctx, guild, &form)?;
    let reason = if approve { String::new() } else { text::clip(&form.string("reason"), 300) };
    let changed = store::review(&ctx, guild, &ids, approve, &page.user.name, &reason)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let catalog = store::catalog(&ctx, guild)?;
    let server = ctx.guild_name(guild);
    for p in &changed {
        let pet = catalog.iter().find(|c| c.id == p.pet_id).map(|c| format!("{} {}", c.icon(), c.name)).unwrap_or_else(|| p.pet_id.clone());
        let qty = crate::qty_label(p.quantity);
        if s.dm_reviews {
            let msg = if approve {
                format!("Your {qty}{pet} submission in {server} was approved and is in the collection.")
            } else if reason.is_empty() {
                format!("Your {qty}{pet} submission in {server} was rejected.")
            } else {
                format!("Your {qty}{pet} submission in {server} was rejected: {reason}")
            };
            crate::dm_later(&ctx, p.user_id, msg);
        }
    }
    let verb = if approve { "Approved" } else { "Rejected" };
    // The Collection tab's rail posts here too, and expects to be sent back to it.
    let back = if form.string("back") == "collection" { "pets" } else { "pets/approvals" };
    match changed.as_slice() {
        [] => {}
        [one] => {
            let pet = catalog.iter().find(|c| c.id == one.pet_id).map(|c| c.name.clone()).unwrap_or_default();
            ctx.audit(&page.user, Some(guild), KEY, format!("{verb} {}{pet} from {}", crate::qty_label(one.quantity), ctx.display_name(guild, one.user_id)));
        }
        many => ctx.audit(&page.user, Some(guild), KEY, format!("{verb} {} pet submissions", many.len())),
    }
    Ok(saved(&page.path(back)))
}

// ---- catalog ----

#[derive(Deserialize)]
struct CatalogQuery {
    edit: Option<String>,
}

async fn catalog(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<CatalogQuery>) -> WebResult {
    let page = open(&ctx, &user, guild, Level::View)?;
    let catalog = store::catalog(&ctx, guild)?;
    let cats = store::categories(&ctx, guild)?;
    let counts = store::owned_counts(&ctx, guild)?;
    let waiting = store::pending_units(&ctx, guild)?;
    let editing = q.edit.as_deref().and_then(|id| catalog.iter().find(|p| p.id == id));
    let edit = page.can_edit();
    let body = html! {
        (tabs(&page, "Catalog", waiting))
        (ui::readings(&[
            ("companions", text::num(catalog.len() as i64)),
            ("categories", text::num(cats.len() as i64)),
            ("collected", format!("{} of {}", counts.len(), catalog.len())),
        ]))
        @if let (Some(p), true) = (editing, edit) {
            div #edit { (ui::section(&format!("Edit {}", p.name), Some("Removing a pet also takes every copy out of the collection."), html! {
                (pet_form(&page, &cats, Some(p)))
                div { (ui::action_button(&page.path(&format!("pets/catalog/{}/delete", p.id)), "Remove pet", Tone::Bad, true)) }
            })) }
        }
        @for cat in &cats {
            @let pets: Vec<&Pet> = catalog.iter().filter(|p| &p.category == cat).collect();
            @let have = pets.iter().filter(|p| counts.contains_key(&p.id)).count();
            (ui::section(cat, Some(&format!("{} pets, {} collected", pets.len(), have)), html! {
                @if pets.is_empty() {
                    (ui::empty(Icon::Paw, "No pets here", "Add one below and pick this category, or remove the category."))
                } @else {
                    div.table-wrap { table.table {
                        thead { tr { th { "Pet" } th { "Rarity" } th.num { "Owned" } @if edit { th {} } } }
                        tbody { @for p in &pets {
                            @let n = counts.get(&p.id).copied().unwrap_or(0);
                            @let detail = if p.bonus.is_empty() { p.description.clone() } else { p.bonus.clone() };
                            tr id={ "pet-" (p.id) } {
                                td { (view::pet_cell(p, (!detail.is_empty()).then_some(detail.as_str()))) }
                                td { (view::rarity(&p.rarity)) }
                                td.num {
                                    @if edit {
                                        form.inline method="post" action=(page.path(&format!("pets/catalog/{}/owned", p.id))) {
                                            span.inline-form style="justify-content:flex-end;flex-wrap:nowrap;gap:6px" {
                                                button.btn.small type="submit" name="delta" value="-1" disabled[n == 0] aria-label={ "Remove one " (p.name) } { "−" }
                                                span style="min-width:2ch;text-align:center" { (n) }
                                                button.btn.small type="submit" name="delta" value="1" aria-label={ "Add one " (p.name) } { "+" }
                                            }
                                        }
                                    } @else { (n) }
                                }
                                @if edit {
                                    td.actions {
                                        a.btn.small href={ (page.path("pets/catalog")) "?edit=" (p.id) "#edit" } { "Edit" }
                                    }
                                }
                            }
                        } }
                    } }
                }
            }))
        }
        @if edit {
            (ui::section("Add a pet", None, pet_form(&page, &cats, None)))
            (ui::section("Categories", Some("A category can be removed once no pet uses it."), html! {
                div.cat-list {
                    @for cat in &cats {
                        @let used = catalog.iter().any(|p| &p.category == cat);
                        span.chip {
                            span { (cat) }
                            @if !used {
                                form.inline method="post" action=(page.path("pets/categories/delete")) {
                                    input type="hidden" name="name" value=(cat);
                                    button type="submit" data-confirm="Remove?" aria-label={ "Remove " (cat) } { "Remove" }
                                }
                            }
                        }
                    }
                }
                form.inline-form method="post" action=(page.path("pets/categories")) {
                    input.input type="text" name="name" maxlength="60" placeholder="New category" required autocomplete="off";
                    button.btn type="submit" { "Add category" }
                }
            }))
        }
    };
    shell(&ctx, &page, body)
}

fn pet_form(page: &Page, cats: &[String], p: Option<&Pet>) -> Markup {
    let cat_opts: Vec<(String, String)> = cats.iter().map(|c| (c.clone(), c.clone())).collect();
    let rar_opts: Vec<(String, String)> = store::RARITIES.iter().map(|r| (r.to_string(), store::rarity_label(r).to_string())).collect();
    let v = |f: fn(&Pet) -> &str| p.map(f).unwrap_or_default().to_owned();
    html! {
        form.pform method="post" action=(page.path("pets/catalog")) {
            @if let Some(p) = p { input type="hidden" name="original" value=(p.id); }
            (ui::card(html! {
                (ui::field_text("name", "Name", None, &v(|p| &p.name), "King Doot"))
                (ui::field_text("emoji", "Emoji", Some("Shown when there's no image."), &v(|p| &p.emoji), "🐾"))
                (ui::field_select("category", "Category", None, &cat_opts, &v(|p| &p.category)))
                (ui::field_select("rarity", "Rarity", None, &rar_opts, p.map(|p| store::clean_rarity(&p.rarity)).unwrap_or("common")))
                (ui::field_text("image", "Image URL", Some("An https link to a picture of the pet. Leave empty to use the emoji."), &v(|p| &p.image), "https://"))
                (ui::field_text("bonus", "Bonus", Some("What it does in game, if anything."), &v(|p| &p.bonus), "2x Construction Build Speed"))
                (ui::field_textarea("description", "Description", None, &v(|p| &p.description), "One line about the pet."))
            }))
            div.acts {
                @if p.is_some() { a.btn href=(page.path("pets/catalog")) { "Cancel" } }
                button.btn.primary type="submit" { @if p.is_some() { "Save pet" } @else { "Add pet" } }
            }
        }
    }
}

async fn save_pet(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = open(&ctx, &user, guild, Level::Edit)?;
    let name = text::clip(&form.string("name"), 60);
    if name.is_empty() {
        return Err(WebError::Bad("Give the pet a name.".into()));
    }
    let category = text::clip(&form.string("category"), 60);
    if category.is_empty() {
        return Err(WebError::Bad("Pick a category. Add one first if the list is empty.".into()));
    }
    let image = form.string("image");
    if !image.is_empty() && !image.starts_with("https://") {
        return Err(WebError::Bad("The image URL must start with https://.".into()));
    }
    let original = form.get("original").map(str::to_owned);
    let id = match &original {
        Some(id) if store::pet(&ctx, guild, id)?.is_some() => id.clone(),
        _ => store::new_pet_id(&ctx, guild, &name)?,
    };
    let pet = Pet {
        id,
        name: name.clone(),
        emoji: text::clip(&form.string("emoji"), 16),
        category,
        rarity: store::clean_rarity(&form.string("rarity")).to_owned(),
        description: text::clip(&form.text("description"), 300),
        bonus: text::clip(&form.string("bonus"), 200),
        image: text::clip(&image, 500),
    };
    store::save_pet(&ctx, guild, &pet)?;
    ctx.audit(&page.user, Some(guild), KEY, if original.is_some() { format!("Edited the pet {name}") } else { format!("Added the pet {name} to the catalog") });
    Ok(saved(&page.path("pets/catalog")))
}

async fn delete_pet(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, String)>) -> WebResult {
    let page = open(&ctx, &user, guild, Level::Edit)?;
    let Some(pet) = store::pet(&ctx, guild, &id)? else { return Err(WebError::NotFound) };
    let copies = store::delete_pet(&ctx, guild, &id)?;
    ctx.audit(
        &page.user,
        Some(guild),
        KEY,
        if copies > 0 { format!("Removed the pet {} and {copies} owned", pet.name) } else { format!("Removed the pet {}", pet.name) },
    );
    Ok(saved(&page.path("pets/catalog")))
}

async fn change_owned(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, String)>, form: FormData) -> WebResult {
    let page = open(&ctx, &user, guild, Level::Edit)?;
    let Some(pet) = store::pet(&ctx, guild, &id)? else { return Err(WebError::NotFound) };
    if form.i64("delta").unwrap_or(0) > 0 {
        store::add_owned(&ctx, guild, &pet.id, page.user.id, page.user.id, 1, &page.user.name)?;
        ctx.audit(&page.user, Some(guild), KEY, format!("Added one {} to the collection", pet.name));
    } else if store::remove_owned(&ctx, guild, &pet.id, 1, None)? > 0 {
        ctx.audit(&page.user, Some(guild), KEY, format!("Removed one {} from the collection", pet.name));
    }
    // Back to the same row, so a run of +/- clicks doesn't lose the scroll position.
    Ok(bot_core::axum::response::Redirect::to(&format!("{}?saved=1#pet-{}", page.path("pets/catalog"), pet.id)).into_response())
}

async fn add_category(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = open(&ctx, &user, guild, Level::Edit)?;
    let name = text::clip(&form.string("name"), 60);
    if name.is_empty() {
        return Err(WebError::Bad("Name the category.".into()));
    }
    if store::add_category(&ctx, guild, &name)? {
        ctx.audit(&page.user, Some(guild), KEY, format!("Added the pet category {name}"));
    }
    Ok(saved(&page.path("pets/catalog")))
}

async fn delete_category(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = open(&ctx, &user, guild, Level::Edit)?;
    let name = form.string("name");
    if !store::delete_category(&ctx, guild, &name)? {
        return Err(WebError::Bad("Pets still use that category. Move or remove them first.".into()));
    }
    ctx.audit(&page.user, Some(guild), KEY, format!("Removed the pet category {name}"));
    Ok(saved(&page.path("pets/catalog")))
}

// ---- giveaways ----

async fn giveaways(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = open(&ctx, &user, guild, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let catalog = store::catalog(&ctx, guild)?;
    let counts = store::owned_counts(&ctx, guild)?;
    let active = store::active_giveaways(&ctx, guild)?;
    let past = store::past_giveaways(&ctx, guild, 50)?;
    let comments = store::comments(&ctx, guild)?;
    let stats = store::stats(&store::all_giveaways(&ctx, guild)?);
    let waiting = store::pending_units(&ctx, guild)?;
    let mut options: Vec<(String, String)> = vec![(String::new(), "Pick a pet".into())];
    let mut sorted: Vec<&Pet> = catalog.iter().collect();
    sorted.sort_by_key(|p| (!counts.contains_key(&p.id), p.name.to_lowercase()));
    for p in sorted {
        let label = match counts.get(&p.id) {
            Some(n) => format!("{} {} (×{n} owned)", p.icon(), p.name),
            None => format!("{} {}", p.icon(), p.name),
        };
        options.push((p.id.clone(), label));
    }
    let default_expiry = if s.expiry_hours > 0 { format!("{}h", s.expiry_hours) } else { String::new() };
    let body = html! {
        (tabs(&page, "Giveaways", waiting))
        (ui::readings(&[
            ("waiting for a hand-over", text::num(stats.active)),
            ("confirmed", text::num(stats.confirmed)),
            ("confirmation rate", stats.confirmation_rate().map(|r| format!("{r}%")).unwrap_or_else(|| "–".into())),
        ]))
        @if page.can_edit() {
            (ui::section("Record a giveaway", Some("Both members get a DM, a warning 15 minutes before it expires, and it's cancelled if nobody confirms it in time."), html! {
                form.pform method="post" action=(page.path("pets/giveaways")) {
                    (members_datalist(&ctx, guild))
                    (ui::card(html! {
                        (ui::field_select("pet", "Pet", None, &options, ""))
                        (ui::field_custom("Giver", Some("Name as shown in the server, or their ID."), html! {
                            input.input type="text" name="giver" list="pets-members" required autocomplete="off" placeholder="Who gives the pet";
                        }))
                        (ui::field_custom("Winner", None, html! {
                            input.input type="text" name="winner" list="pets-members" required autocomplete="off" placeholder="Who gets it";
                        }))
                        (ui::field_text("expires", "Expires in", Some("Like 2h, 1d or 1d 12h. Leave empty for no expiry."), &default_expiry, "24h"))
                        (ui::field_text("notes", "Notes", None, "", "Optional"))
                    }))
                    div.acts { button.btn.primary type="submit" { "Record giveaway" } }
                }
            }))
        }
        (ui::section("Waiting", Some("Confirm once the pet has changed hands. Confirming takes one copy out of the collection."), html! {
            @if active.is_empty() {
                (ui::empty(Icon::Gift, "No giveaways waiting", "Record one above and it shows here until it's confirmed or expires."))
            } @else {
                div { @for g in &active { (giveaway_item(&ctx, &page, g, comments.get(&g.id).map(Vec::as_slice).unwrap_or(&[]))) } }
            }
        }))
        (ui::section("History", None, html! {
            @if past.is_empty() {
                (ui::empty(Icon::Log, "No history yet", "Confirmed, expired and cancelled giveaways are kept here."))
            } @else {
                div { @for g in &past { (giveaway_item(&ctx, &page, g, comments.get(&g.id).map(Vec::as_slice).unwrap_or(&[]))) } }
            }
        }))
        (ui::settings_form(&page, &page.path("pets"), html! {
            (ui::section("Giveaway settings", None, ui::card(html! {
                (ui::field_channel(&ctx, guild, "announce_channel", "Confirmed giveaways", Some("Posted here when a giveaway is confirmed, mentioning the giver and the winner. None posts nothing."), s.announce_channel))
                (ui::field_number("expiry_hours", "Giveaways expire after", Some("The default when you record one. 0 means no expiry."), s.expiry_hours, 0, 720, Some("hours")))
                (ui::field_toggle("dm_reviews", "DM review results", Some("Members get a DM when their submission is approved or rejected."), s.dm_reviews))
            })))
        }))
        (ui::section("Public pages", Some("Anyone with the link can open these, without signing in."), html! {
            div.links {
                @for (label, path, hint) in [
                    ("Collection", "pets", "Every companion by category, with what the server owns."),
                    ("Giveaways", "pet-giveaways", "Giveaways waiting for a hand-over and recent results."),
                    ("Stats", "pet-stats", "Top givers, most given pets, rarity and confirmation rate."),
                ] {
                    a href={ "/p/" (guild) "/" (path) } target="_blank" rel="noopener" {
                        span.grow { strong { (label) } span { (hint) } }
                        span.mono.muted.clip style="max-width:40%" { (ctx.cfg.public_url.trim_end_matches('/')) "/p/" (guild) "/" (path) }
                        (ui::icon(Icon::External))
                    }
                }
            }
        }))
    };
    shell(&ctx, &page, body)
}

fn giveaway_item(ctx: &Ctx, page: &Page, g: &Giveaway, comments: &[store::Comment]) -> Markup {
    let guild = page.guild_id;
    let base = page.path(&format!("pets/giveaways/{}", g.id));
    let (expiry, close) = view::expiry(g.expires_at);
    html! {
        div.gv-wrap {
            div.gv {
                (view::tile(g.icon(), None, false))
                div.t {
                    span { b { (g.pet_name) } " " (view::rarity(&g.pet_rarity)) }
                    span.who { (ctx.display_name(guild, g.giver_id)) " to " (ctx.display_name(guild, g.winner_id)) }
                }
                div.gv-side {
                    @if g.status == Status::Pending {
                        span.soon[close] { (expiry) }
                        @if page.can_edit() {
                            div.gv-acts {
                                (ui::action_button(&format!("{base}/confirm"), "Confirm", Tone::Muted, false))
                                (ui::action_button(&format!("{base}/cancel"), "Cancel", Tone::Bad, true))
                            }
                        } @else {
                            span { "Recorded by " (g.recorded_by) " " (text::ago(g.recorded_at)) }
                        }
                    } @else {
                        (view::status_badge(g.status))
                        span { (g.closed_at.map(text::ago).unwrap_or_else(|| text::ago(g.recorded_at))) @if let Some(by) = g.closed_by.as_deref().filter(|b| *b != "Expired") { " by " (by) } }
                    }
                }
            }
            @if !g.notes.is_empty() { p.gv-note { (g.notes) } }
            details.gv-more {
                summary { @if comments.is_empty() { "Comments" } @else { "Comments (" (comments.len()) ")" } " · recorded by " (g.recorded_by) " " (text::ago(g.recorded_at)) }
                div.cmts {
                    @for c in comments {
                        div.cmt {
                            div.grow { strong { (c.author) } " " span { (text::ago(c.at)) } div { (c.text) } }
                            @if page.can_edit() { (ui::action_button(&format!("{base}/comments/{}/delete", c.id), "Delete", Tone::Bad, true)) }
                        }
                    }
                    @if page.can_edit() {
                        form.inline-form method="post" action={ (base) "/comments" } {
                            input.input type="text" name="text" maxlength="500" required placeholder="Add a comment" autocomplete="off";
                            button.btn.small type="submit" { "Comment" }
                        }
                        @if g.status != Status::Pending { div { (ui::action_button(&format!("{base}/delete"), "Delete giveaway", Tone::Bad, true)) } }
                    }
                }
            }
        }
    }
}

async fn record_giveaway(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = open(&ctx, &user, guild, Level::Edit)?;
    let pet = store::pet(&ctx, guild, &form.string("pet"))?.ok_or_else(|| WebError::Bad("Pick a pet.".into()))?;
    let giver = resolve_member(&ctx, guild, &form.string("giver"))
        .ok_or_else(|| WebError::Bad("Couldn't find the giver. Use their name as shown in the server, or their ID.".into()))?;
    let winner = resolve_member(&ctx, guild, &form.string("winner"))
        .ok_or_else(|| WebError::Bad("Couldn't find the winner. Use their name as shown in the server, or their ID.".into()))?;
    if giver == winner {
        return Err(WebError::Bad("The giver and the winner are the same person.".into()));
    }
    if let Some(b) = store::ban(&ctx, guild, giver)? {
        let why = if b.reason.is_empty() { String::new() } else { format!(" ({})", b.reason) };
        return Err(WebError::Bad(format!("{} is banned from giving pets{why}. Unban them on the Bans tab first.", ctx.display_name(guild, giver))));
    }
    let expires = form.string("expires");
    let expires_at = if expires.is_empty() {
        None
    } else {
        let secs = text::parse_duration(&expires).filter(|s| (60..=30 * 86_400).contains(s)).ok_or_else(|| WebError::Bad("Expiry should look like 30m, 2h or 1d, up to 30 days.".into()))?;
        Some(now_ms() + secs * 1000)
    };
    let notes = text::clip(&form.string("notes"), 300);
    let id = store::record_giveaway(&ctx, guild, &store::NewGiveaway { pet: &pet, giver, winner, notes: &notes, recorded_by: &page.user.name, expires_at })?;
    if let Some(g) = store::giveaway(&ctx, guild, id)? {
        crate::announce_recorded(&ctx, guild, &g);
    }
    ctx.audit(&page.user, Some(guild), KEY, format!("Recorded a {} giveaway from {} to {}", pet.name, ctx.display_name(guild, giver), ctx.display_name(guild, winner)));
    Ok(saved(&page.path("pets/giveaways")))
}

async fn confirm_giveaway(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    let page = open(&ctx, &user, guild, Level::Edit)?;
    let g = store::giveaway(&ctx, guild, id)?.ok_or(WebError::NotFound)?;
    if store::close_giveaway(&ctx, guild, id, Status::Confirmed, &page.user.name)? {
        // As before: the pet left the collection, preferably a copy the giver had given.
        store::remove_owned(&ctx, guild, &g.pet_id, 1, Some(g.giver_id))?;
        let c2 = ctx.clone();
        bot_core::tokio::spawn(async move { crate::announce_confirmed(&c2, guild, &g).await });
        ctx.audit(&page.user, Some(guild), KEY, format!("Confirmed giveaway #{id}"));
    }
    Ok(saved(&page.path("pets/giveaways")))
}

async fn cancel_giveaway(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    let page = open(&ctx, &user, guild, Level::Edit)?;
    if store::close_giveaway(&ctx, guild, id, Status::Cancelled, &page.user.name)? {
        ctx.audit(&page.user, Some(guild), KEY, format!("Cancelled giveaway #{id}"));
    }
    Ok(saved(&page.path("pets/giveaways")))
}

async fn delete_giveaway(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    let page = open(&ctx, &user, guild, Level::Edit)?;
    store::delete_giveaway(&ctx, guild, id)?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Deleted giveaway #{id}"));
    Ok(saved(&page.path("pets/giveaways")))
}

async fn add_comment(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>, form: FormData) -> WebResult {
    let page = open(&ctx, &user, guild, Level::Edit)?;
    store::giveaway(&ctx, guild, id)?.ok_or(WebError::NotFound)?;
    let body = text::clip(&form.text("text"), 500);
    if body.is_empty() {
        return Err(WebError::Bad("Write something first.".into()));
    }
    store::add_comment(&ctx, id, &page.user.name, &body)?;
    Ok(saved(&page.path("pets/giveaways")))
}

async fn delete_comment(State(ctx): State<Ctx>, user: User, Path((guild, id, comment)): Path<(u64, i64, i64)>) -> WebResult {
    let page = open(&ctx, &user, guild, Level::Edit)?;
    store::giveaway(&ctx, guild, id)?.ok_or(WebError::NotFound)?;
    store::delete_comment(&ctx, id, comment)?;
    Ok(saved(&page.path("pets/giveaways")))
}

// ---- bans ----

async fn bans(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = open(&ctx, &user, guild, Level::View)?;
    let list = store::bans(&ctx, guild)?;
    let waiting = store::pending_units(&ctx, guild)?;
    let given: HashMap<u64, i64> = store::stats(&store::all_giveaways(&ctx, guild)?).top_givers.into_iter().collect();
    let body = html! {
        (tabs(&page, "Bans", waiting))
        (ui::section("Banned givers", Some("Members on this list can't be recorded as the giver of a pet giveaway. They can still win."), html! {
            @if page.can_edit() {
                form.inline-form method="post" action=(page.path("pets/bans")) {
                    (members_datalist(&ctx, guild))
                    input.input type="text" name="member" list="pets-members" required placeholder="Name or ID" autocomplete="off" style="max-width:220px";
                    input.input type="text" name="reason" maxlength="200" placeholder="Reason" autocomplete="off";
                    button.btn type="submit" { "Ban giver" }
                }
            }
            @if list.is_empty() {
                (ui::empty(Icon::Shield, "Nobody is banned", "Ban a member here and they can't be recorded as a giver."))
            } @else {
                div.table-wrap { table.table {
                    thead { tr { th { "Member" } th { "Reason" } th.num { "Given" } th { "Banned" } @if page.can_edit() { th {} } } }
                    tbody { @for b in &list {
                        tr {
                            td { (view::person(&ctx, guild, b.user_id)) }
                            td { @if b.reason.is_empty() { span.muted { "No reason given" } } @else { (b.reason) } }
                            td.num { (given.get(&b.user_id).copied().unwrap_or(0)) }
                            td.muted { (text::ago(b.banned_at)) " by " (b.banned_by) }
                            @if page.can_edit() { td.actions { (ui::action_button(&page.path(&format!("pets/bans/{}/delete", b.user_id)), "Unban", Tone::Muted, false)) } }
                        }
                    } }
                } }
            }
        }))
    };
    shell(&ctx, &page, body)
}

async fn add_ban(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = open(&ctx, &user, guild, Level::Edit)?;
    let member = resolve_member(&ctx, guild, &form.string("member"))
        .ok_or_else(|| WebError::Bad("Couldn't find that member. Use their name as shown in the server, or their ID.".into()))?;
    let reason = text::clip(&form.string("reason"), 200);
    if store::add_ban(&ctx, guild, member, &reason, &page.user.name)? {
        ctx.audit(&page.user, Some(guild), KEY, format!("Banned {} from giving pets", ctx.display_name(guild, member)));
    }
    Ok(saved(&page.path("pets/bans")))
}

async fn remove_ban(State(ctx): State<Ctx>, user: User, Path((guild, member)): Path<(u64, u64)>) -> WebResult {
    let page = open(&ctx, &user, guild, Level::Edit)?;
    store::remove_ban(&ctx, guild, member)?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Unbanned {} from giving pets", ctx.display_name(guild, member)));
    Ok(saved(&page.path("pets/bans")))
}
