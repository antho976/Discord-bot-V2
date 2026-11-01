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

/// The five tabs, with the number of submissions waiting on Approvals.
fn tabs(page: &Page, active: &str, waiting: i64) -> Markup {
    html! {
        (view::style())
        style { (PreEscaped(DASH_CSS)) }
        (ui::tabs_counted(&[
            ("Collection", page.path("pets"), None),
            ("Approvals", page.path("pets/approvals"), (waiting > 0).then_some(waiting)),
            ("Giveaways", page.path("pets/giveaways"), None),
            ("Catalog", page.path("pets/catalog"), None),
            ("Bans", page.path("pets/bans"), None),
        ], active))
    }
}

/// Styles only the dashboard's pet pages use: Discord's look for the announcement preview.
const DASH_CSS: &str = r#"
.pets-dc { border-radius: 12px; overflow: hidden; background: #313338; color: #dbdee1; font-size: 14.5px; box-shadow: 0 22px 44px -24px rgba(0, 0, 0, 0.9), inset 0 0 0 1px rgba(255, 255, 255, 0.06); }
.pets-dc-head { display: flex; align-items: center; gap: 6px; height: 40px; padding: 0 14px; color: #f2f3f5; font-weight: 600; font-size: 14px; box-shadow: 0 1px 0 rgba(4, 4, 5, 0.25); }
.pets-dc-head .icon { color: #80848e; width: 18px; height: 18px; }
.pets-dc-msg { display: grid; grid-template-columns: 40px minmax(0, 1fr); gap: 0 14px; padding: 12px 14px 16px; }
.pets-dc-av { width: 40px; height: 40px; border-radius: 50%; object-fit: cover; }
span.pets-dc-av { display: grid; place-items: center; background: linear-gradient(145deg, var(--accent-hi), var(--accent-lo)); }
span.pets-dc-av img { width: 22px; height: 22px; }
.pets-dc-name { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; line-height: 1.375; }
.pets-dc-name strong { color: #f2f3f5; font-weight: 600; }
.pets-dc-app { display: inline-flex; align-items: center; height: 15px; padding: 0 4px; border-radius: 3px; background: #5865f2; color: #fff; font: 600 10px/1 var(--sans); }
.pets-dc-time { color: #949ba4; font-size: 12px; }
.pets-dc-text { line-height: 1.375; overflow-wrap: anywhere; }
.pets-dc-text b { color: #f2f3f5; }
.pets-dc-mention { background: rgba(88, 101, 242, 0.3); color: #c9cdfb; border-radius: 3px; padding: 0 2px; font-weight: 500; }
.pets-split { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 340px); gap: 20px; align-items: start; }
.pets-split .pets-cap { font-size: 12.5px; color: var(--ink-3); margin-bottom: 8px; }
@media (max-width: 1180px) { .pets-split { grid-template-columns: 1fr; } }
"#;

/// The bot as members see it: its name, and its avatar or the dashboard's mark while it isn't
/// connected.
fn bot_face(ctx: &Ctx) -> (String, Markup) {
    let user = ctx.bot_id().and_then(|b| ctx.cache.user(Id::new(b)).map(|u| (b, u)));
    match user {
        Some((id, u)) => (
            u.global_name.clone().unwrap_or_else(|| u.name.clone()),
            html! { img.pets-dc-av src=(bot_core::discord::avatar_url(id, u.avatar.map(|a| a.to_string()).as_deref())) alt=""; },
        ),
        None => ("Bot".into(), html! { span.pets-dc-av { img src="/static/icon.svg" alt=""; } }),
    }
}

fn shell(ctx: &Ctx, page: &Page, actions: Option<Markup>, body: Markup) -> WebResult {
    Ok(ui::layout(ctx, page, "Pets", actions, body).into_response())
}

/// A header button that opens a form in a popover, for editors only.
fn header_pop(label: &str, body: Markup) -> Markup {
    html! {
        details.pop.pets-pop {
            summary.btn.primary aria-label=(label) { (ui::icon(Icon::Plus)) span.pets-pop-label { (label) } }
            div.pop-body { (body) }
        }
    }
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
    let waiting = pending.len() as i64;
    let active = store::active_giveaways(&ctx, guild)?;
    let past = store::past_giveaways(&ctx, guild, 3)?;
    let gstats = store::stats(&store::all_giveaways(&ctx, guild)?);
    let week = store::first_added_since(&ctx, guild, now_ms() - 7 * DAY_MS)?;
    let find = |id: &str| catalog.iter().find(|p| p.id == id).cloned().unwrap_or_else(|| missing_pet(id));

    let groups = view::groups(&catalog);
    // Where each category's tiles start in the page's one wave of arrivals.
    let starts: Vec<usize> = groups.iter().scan(0, |n, (_, l)| { let at = *n; *n += l.len(); Some(at) }).collect();
    let actions = html! {
        a.btn.outline href={ "/p/" (guild) "/pets" } target="_blank" rel="noopener" { (ui::icon(Icon::External)) span.pop-label { "Public page" } }
    };

    let body = html! {
        (tabs(&page, "Collection", waiting))
        div.with-rail {
            div.stack.pets-main {
                (view::collection_hero(&groups, &counts, week))
                @for (g, (name, list)) in groups.iter().enumerate() {
                    @let have = list.iter().filter(|p| counts.contains_key(&p.id)).count();
                    (ui::section(name, Some(&format!("{have} of {} collected{}", list.len(), if g == 0 { ". Faded ones aren't in the collection yet." } else { "" })), html! {
                        div.pets-grid {
                            @for (k, p) in list.iter().enumerate() {
                                (view::collection_tile(p, counts.get(&p.id).copied().unwrap_or(0), starts[g] + k))
                            }
                        }
                    }))
                }
                @if groups.is_empty() {
                    (ui::empty(Icon::Paw, "No companions yet", if page.can_edit() {
                        "The catalog fills from the server's list. Add pets in the Catalog tab."
                    } else {
                        "The catalog fills from the server's list."
                    }))
                }
            }

            aside.rail {
                div.rail-block {
                    h2 { (if page.can_edit() { "Waiting for you" } else { "Waiting for review" }) @if !pending.is_empty() { span.count-pill { (pending.len()) } } }
                    @if pending.is_empty() {
                        @if page.can_edit() {
                            p.muted.small { "Submissions from /pet add land here. Approve or reject them from here or the Approvals tab." }
                        } @else {
                            p.muted.small { "Submissions from /pet add wait here until staff review them." }
                        }
                    }
                    @for p in pending.iter().take(3) {
                        @let pet = find(&p.pet_id);
                        div.pets-req {
                            (view::pet_art(&pet, "md"))
                            div {
                                div { strong { (pet.name) } @if p.quantity > 1 { span.mono { " ×" (p.quantity) } } }
                                div.who.muted.small {
                                    img.avatar src=(ctx.member_avatar(guild, p.user_id)) alt="" loading="lazy";
                                    (ctx.display_name(guild, p.user_id)) " · " (text::ago(p.requested_at))
                                }
                                @if page.can_edit() {
                                    div.pets-req-acts {
                                        form method="post" action=(page.path("pets/approvals/approve")) {
                                            input type="hidden" name="only" value=(p.id);
                                            input type="hidden" name="back" value="collection";
                                            button.btn.small.outline type="submit" { (ui::icon(Icon::Check)) "Approve" }
                                        }
                                        form method="post" action=(page.path("pets/approvals/reject")) {
                                            input type="hidden" name="only" value=(p.id);
                                            input type="hidden" name="back" value="collection";
                                            button.btn.small.ghost type="submit" { "Reject" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    @if pending.len() > 3 {
                        a.btn.small.ghost href=(page.path("pets/approvals")) { (pending.len() - 3) " more on Approvals" (ui::icon(Icon::Enter)) }
                    }
                }
                div.rail-block {
                    h2 { "Pet giveaways" }
                    @if active.is_empty() && past.is_empty() {
                        p.muted.small { "Giveaways you record show up here until they're confirmed or expire." }
                    }
                    @for g in active.iter().take(3).chain(past.iter().take(3)) {
                        div.pets-mini-gv {
                            (ui::avatars(&[ctx.member_avatar(guild, g.giver_id), ctx.member_avatar(guild, g.winner_id)], 2))
                            (view::art(g.icon(), None, &g.pet_rarity, "sm"))
                            span.grow data-tip={ (ctx.display_name(guild, g.giver_id)) " to " (ctx.display_name(guild, g.winner_id)) } { (g.pet_name) }
                            @match g.status {
                                Status::Pending => span.pets-state.soon { (left_text(g.expires_at)) },
                                Status::Confirmed => span.pets-state.ok { "Confirmed" },
                                Status::Expired => span.pets-state.bad { "Expired" },
                                Status::Cancelled => span.pets-state { "Cancelled" },
                            }
                        }
                    }
                    @if gstats.confirmed > 0 {
                        p.muted.small {
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
    let done = store::reviewed(&ctx, guild, 8)?;
    let edit = page.can_edit();
    let approve = page.path("pets/approvals/approve");
    let reject = page.path("pets/approvals/reject");
    let copies: i64 = pending.iter().map(|p| p.quantity).sum();
    let submitters = pending.iter().map(|p| p.user_id).collect::<std::collections::HashSet<_>>().len() as i64;
    let body = html! {
        (tabs(&page, "Approvals", pending.len() as i64))
        div.with-rail {
            div.stack.pets-main {
                (view::readings(&[
                    ("waiting for review", text::num(pending.len() as i64)),
                    ("copies in them", text::num(copies)),
                    ("from members", text::num(submitters)),
                ]))
                @if pending.is_empty() {
                    (ui::empty(Icon::Paw, "Nothing to review", if edit {
                        "Submissions from /pet add wait here until you approve or reject them."
                    } else {
                        "Submissions from /pet add wait here until staff review them."
                    }))
                } @else {
                    form method="post" action=(approve) {
                        // Enter in the reason box must not approve anything.
                        button type="submit" disabled hidden {}
                        div.pets-list-head {
                            @if edit { label.pets-check { input #sel-all type="checkbox"; "Select all" } }
                            span.muted.small { "Approved pets join the collection, credited to whoever submitted them." }
                        }
                        div.pets-rows.pets-tiles.wide {
                            @for p in &pending {
                                @let pet = find(&p.pet_id);
                                div class={ "pets-row r-" (store::clean_rarity(&pet.rarity)) } {
                                    @if edit { input type="checkbox" name="sel" value=(p.id) aria-label={ "Select " (pet.name) " from " (ctx.display_name(guild, p.user_id)) }; }
                                    (view::pet_art(&pet, "lg"))
                                    div.pets-row-main {
                                        span { strong { (pet.name) } @if p.quantity > 1 { span.mono { "×" (p.quantity) } } (view::rarity(&pet.rarity)) }
                                        span.pets-by {
                                            img.avatar src=(ctx.member_avatar(guild, p.user_id)) alt="" loading="lazy";
                                            b { (ctx.display_name(guild, p.user_id)) } " · " (text::ago(p.requested_at))
                                        }
                                    }
                                    @if edit {
                                        div.pets-row-acts {
                                            button.btn.small.outline type="submit" formaction=(approve) name="only" value=(p.id) { (ui::icon(Icon::Check)) "Approve" }
                                            button.btn.small.ghost type="submit" formaction=(reject) name="only" value=(p.id) { "Reject" }
                                        }
                                    }
                                }
                            }
                        }
                        @if edit {
                            div.bulk {
                                input.input type="text" name="reason" maxlength="300" placeholder="Reason for rejecting, sent to them (optional)" autocomplete="off" aria-label="Reason for rejecting";
                                button.btn.small type="submit" formaction=(reject) { "Reject selected" }
                                button.btn.small type="submit" formaction=(approve) { "Approve selected" }
                                button.btn.small.primary type="submit" formaction=(approve) name="all" value="1" { "Approve all" }
                            }
                        }
                    }
                    script { (PreEscaped(r#"document.getElementById("sel-all")?.addEventListener("change",e=>document.querySelectorAll('input[name="sel"]').forEach(c=>c.checked=e.target.checked));"#)) }
                }
            }
            aside.rail {
                div.rail-block {
                    h2 { "Recently reviewed" }
                    @if done.is_empty() {
                        p.muted.small { "Approved and rejected submissions show here." }
                    }
                    @for p in &done {
                        @let pet = find(&p.pet_id);
                        @let approved = p.status == "approved";
                        div.mini {
                            img.avatar src=(ctx.member_avatar(guild, p.user_id)) alt="" loading="lazy";
                            div.grow {
                                span { strong { (ctx.display_name(guild, p.user_id)) } " · " (pet.name) @if p.quantity > 1 { " ×" (p.quantity) } }
                                span.muted {
                                    span class=(if approved { "pets-ok" } else { "pets-bad" }) { (if approved { "Approved" } else { "Rejected" }) }
                                    @if let Some(by) = p.reviewed_by.as_deref().filter(|b| !b.is_empty()) { " by " (by) }
                                    @if let Some(at) = p.reviewed_at { " · " (text::ago(at)) }
                                }
                                @if !p.reason.is_empty() { span.muted title=(p.reason) { "“" (p.reason) "”" } }
                            }
                            (view::pet_art(&pet, "sm"))
                        }
                    }
                }
            }
        }
    };
    shell(&ctx, &page, None, body)
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
    let waiting = store::pending_count(&ctx, guild)?;
    let editing = q.edit.as_deref().and_then(|id| catalog.iter().find(|p| p.id == id));
    let edit = page.can_edit();
    let actions = edit.then(|| header_pop("Add a pet", pet_form(&page, &cats, None)));
    let copies: i64 = counts.values().sum();
    let body = html! {
        (tabs(&page, "Catalog", waiting))
        div.with-rail {
            div.stack.pets-main {
                @if let (Some(p), true) = (editing, edit) {
                    div #edit { (view::section(Icon::Brush, &format!("Edit {}", p.name), Some("Removing a pet also takes every copy out of the collection."), Some(view::pet_art(p, "lg")), html! {
                        (pet_form(&page, &cats, Some(p)))
                        div { (ui::action_button(&page.path(&format!("pets/catalog/{}/delete", p.id)), "Remove pet", Tone::Bad, true)) }
                    })) }
                }
                @for cat in &cats {
                    @let pets: Vec<&Pet> = catalog.iter().filter(|p| &p.category == cat).collect();
                    @let have = pets.iter().filter(|p| counts.contains_key(&p.id)).count();
                    (ui::section(cat, Some(&format!("{} pets, {} collected", pets.len(), have)), html! {
                        @if pets.is_empty() {
                            @if edit {
                                (ui::empty(Icon::Paw, "No pets here", "Add one with the button above and pick this category, or remove the category."))
                            } @else {
                                (ui::empty(Icon::Paw, "No pets here", "Pets staff add to this category show here."))
                            }
                        } @else {
                            div.pets-entries {
                                @for p in &pets {
                                    @let n = counts.get(&p.id).copied().unwrap_or(0);
                                    @let rarity = store::clean_rarity(&p.rarity);
                                    div class={ "pets-entry r-" (rarity) @if n == 0 { " missing" } } id={ "pet-" (p.id) } {
                                        (view::pet_art(p, "md"))
                                        div.t {
                                            span {
                                                @if edit {
                                                    a.pets-edit href={ (page.path("pets/catalog")) "?edit=" (p.id) "#edit" } title={ "Edit " (p.name) } { strong { (p.name) } }
                                                } @else { strong { (p.name) } }
                                                (view::rarity(rarity))
                                            }
                                            @if !p.bonus.is_empty() { p.bonus title=(p.bonus) { (p.bonus) } }
                                            @else if !p.description.is_empty() { p title=(p.description) { (p.description) } }
                                        }
                                        @if edit {
                                            form.inline method="post" action=(page.path(&format!("pets/catalog/{}/owned", p.id))) {
                                                span.pets-stepper {
                                                    button.btn.ghost type="submit" name="delta" value="-1" disabled[n == 0] aria-label={ "Remove one " (p.name) } { "−" }
                                                    span aria-label={ (n) " owned" } { (n) }
                                                    button.btn.ghost type="submit" name="delta" value="1" aria-label={ "Add one " (p.name) } { "+" }
                                                }
                                            }
                                        } @else {
                                            span.pets-owned { "×" (n) }
                                        }
                                    }
                                }
                            }
                        }
                    }))
                }
            }
            aside.rail {
                div.rail-block {
                    h2 { "Catalog" }
                    div.rail-nums {
                        div { b data-count { (text::num(catalog.len() as i64)) } span { "companions" } }
                        div { b data-count { (text::num(counts.len() as i64)) } span { "collected" } }
                        div { b data-count { (text::num(copies)) } span { "copies" } }
                    }
                    @if edit { p.muted.small { "Click a pet's name to edit it." } }
                }
                div.rail-block {
                    h2 { "Categories" }
                    div.cat-list {
                        @for cat in &cats {
                            @let used = catalog.iter().any(|p| &p.category == cat);
                            span.chip {
                                span { (cat) }
                                @if edit && !used {
                                    form.inline method="post" action=(page.path("pets/categories/delete")) {
                                        input type="hidden" name="name" value=(cat);
                                        button type="submit" data-confirm="Remove?" aria-label={ "Remove " (cat) } { "Remove" }
                                    }
                                }
                            }
                        }
                    }
                    @if edit {
                        form.inline-form method="post" action=(page.path("pets/categories")) {
                            input.input type="text" name="name" maxlength="60" placeholder="New category" required autocomplete="off" aria-label="New category";
                            button.btn.small type="submit" { "Add" }
                        }
                        p.muted.small { "A category can be removed once no pet uses it." }
                    }
                }
            }
        }
    };
    shell(&ctx, &page, actions, body)
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
    let waiting = store::pending_count(&ctx, guild)?;
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
    let image = |id: &str| catalog.iter().find(|p| p.id == id).and_then(|p| p.image_url().map(str::to_owned));
    let actions = page.can_edit().then(|| {
        header_pop("Record a giveaway", html! {
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
                    (ui::field_text("expires", "Expires in", Some("Like 2h, 1d or 1d 12h. Empty for no expiry."), &default_expiry, "24h"))
                    (ui::field_text("notes", "Notes", None, "", "Optional"))
                }))
                p.muted.small { "Both members get a DM, and a warning 15 minutes before it expires. Unconfirmed giveaways are cancelled when they expire." }
                div.acts { button.btn.primary type="submit" { "Record giveaway" } }
            }
        })
    });
    // The announcement preview shows the latest confirmed giveaway, or a waiting one.
    let example = past.iter().find(|g| g.status == Status::Confirmed).or(active.first());
    let bot = bot_face(&ctx);
    let channel = s.announce_channel.and_then(|c| ctx.text_channels(guild).into_iter().find(|x| x.0 == c)).map(|(_, n)| n);
    let body = html! {
        (tabs(&page, "Giveaways", waiting))
        div.with-rail {
            div.stack.pets-main {
                (view::readings(&[
                    ("waiting for a hand-over", text::num(stats.active)),
                    ("confirmed", text::num(stats.confirmed)),
                    ("go through", stats.confirmation_rate().map(|r| format!("{r}%")).unwrap_or_else(|| "None yet".into())),
                ]))
                (view::section(Icon::Gift, "Waiting", Some("Confirm once the pet has changed hands. Confirming takes one copy out of the collection."), None, html! {
                    @if active.is_empty() {
                        @if page.can_edit() {
                            (ui::empty(Icon::Gift, "No giveaways waiting", "Record one with the button above and it waits here until it's confirmed or expires."))
                        } @else {
                            (ui::empty(Icon::Gift, "No giveaways waiting", "Giveaways staff record wait here until they're confirmed or expire."))
                        }
                    } @else {
                        div.pets-gvs.pets-tiles.wide { @for g in &active { (giveaway_item(&ctx, &page, g, image(&g.pet_id).as_deref(), comments.get(&g.id).map(Vec::as_slice).unwrap_or(&[]))) } }
                    }
                }))
                (view::section(Icon::Log, "History", None, None, html! {
                    @if past.is_empty() {
                        (ui::empty(Icon::Log, "No history yet", "Confirmed, expired and cancelled giveaways are kept here."))
                    } @else {
                        div.pets-gvs.pets-tiles.wide { @for g in &past { (giveaway_item(&ctx, &page, g, image(&g.pet_id).as_deref(), comments.get(&g.id).map(Vec::as_slice).unwrap_or(&[]))) } }
                    }
                }))
                (ui::settings_form(&page, &page.path("pets"), html! {
                    (view::section(Icon::Gear, "Giveaway settings", None, None, html! {
                        div.pets-split {
                            (ui::card(html! {
                                (ui::field_channel(&ctx, guild, "announce_channel", "Confirmed giveaways", Some("Posted here when a giveaway is confirmed, mentioning the giver and the winner. None posts nothing."), s.announce_channel))
                                (ui::field_number("expiry_hours", "Giveaways expire after", Some("The default when you record one. 0 means no expiry."), s.expiry_hours, 0, 720, Some("hours")))
                                (ui::field_toggle("dm_reviews", "DM review results", Some("Members get a DM when their submission is approved or rejected."), s.dm_reviews))
                            }))
                            @if let Some(g) = example {
                                div {
                                    p.pets-cap { @if channel.is_some() { "Posted when you confirm one" } @else { "Posted when you confirm one, once a channel is picked" } }
                                    div.pets-dc {
                                        div.pets-dc-head { (ui::icon(Icon::Hash)) span { (channel.as_deref().unwrap_or("pick a channel")) } }
                                        div.pets-dc-msg {
                                            (bot.1)
                                            div {
                                                div.pets-dc-name { strong { (bot.0) } span.pets-dc-app { "APP" } span.pets-dc-time { "Today" } }
                                                div.pets-dc-text {
                                                    span.pets-dc-mention { "@" (ctx.display_name(guild, g.giver_id)) } " gave "
                                                    span.pets-dc-mention { "@" (ctx.display_name(guild, g.winner_id)) } " " (g.icon()) " " b { (g.pet_name) } "."
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }))
                }))
            }
            aside.rail {
                div.rail-block {
                    h2 { "Top givers" }
                    @if stats.top_givers.is_empty() {
                        p.muted.small { "Givers show here once a giveaway is confirmed." }
                    }
                    @let most = stats.top_givers.first().map(|t| t.1).unwrap_or(1).max(1);
                    @for (u, n) in stats.top_givers.iter().take(5) {
                        div.mini {
                            img.avatar src=(ctx.member_avatar(guild, *u)) alt="" loading="lazy";
                            div.grow {
                                span { (ctx.display_name(guild, *u)) }
                                span.meter style="margin-top:6px" { span style={ "width:" (n * 100 / most) "%" } {} }
                            }
                            span.mono { (text::num(*n)) }
                        }
                    }
                }
                div.rail-block {
                    h2 { "Public pages" }
                    p.muted.small { "Anyone with the link can open these, without signing in." }
                    div.links {
                        @for (label, path) in [("Collection", "pets"), ("Giveaways", "pet-giveaways"), ("Stats", "pet-stats")] {
                            a href={ "/p/" (guild) "/" (path) } target="_blank" rel="noopener" {
                                span.grow { (label) }
                                (ui::icon(Icon::External))
                            }
                        }
                    }
                }
            }
        }
    };
    shell(&ctx, &page, actions, body)
}

fn giveaway_item(ctx: &Ctx, page: &Page, g: &Giveaway, image: Option<&str>, comments: &[store::Comment]) -> Markup {
    let base = page.path(&format!("pets/giveaways/{}", g.id));
    let side = html! {
        @if g.status == Status::Pending {
            (view::clock(g.expires_at))
            @if page.can_edit() {
                div.pets-gv-acts {
                    (ui::action_button(&format!("{base}/confirm"), "Confirm", Tone::Muted, false))
                    (ui::action_button(&format!("{base}/cancel"), "Cancel", Tone::Bad, true))
                }
            }
        } @else {
            (view::status_badge(g.status))
            span { (g.closed_at.map(text::ago).unwrap_or_else(|| text::ago(g.recorded_at))) @if let Some(by) = g.closed_by.as_deref().filter(|b| *b != "Expired") { " by " (by) } }
        }
    };
    let extra = html! {
        @if !g.notes.is_empty() { p.pets-note { (g.notes) } }
        details.pets-more {
            summary { @if comments.is_empty() { "Comments" } @else { "Comments (" (comments.len()) ")" } " · recorded by " (g.recorded_by) " " (text::ago(g.recorded_at)) }
            div.pets-cmts {
                @for c in comments {
                    div.pets-cmt {
                        span.ini { (c.author.chars().next().map(|ch| ch.to_uppercase().to_string()).unwrap_or_default()) }
                        div.grow { strong { (c.author) } " " span { (text::ago(c.at)) } div { (c.text) } }
                        @if page.can_edit() { (ui::action_button(&format!("{base}/comments/{}/delete", c.id), "Delete", Tone::Bad, true)) }
                    }
                }
                @if page.can_edit() {
                    form.inline-form method="post" action={ (base) "/comments" } {
                        input.input type="text" name="text" maxlength="500" required placeholder="Add a comment" autocomplete="off" aria-label="Comment";
                        button.btn.small type="submit" { "Comment" }
                    }
                    @if g.status != Status::Pending { div { (ui::action_button(&format!("{base}/delete"), "Delete giveaway", Tone::Bad, true)) } }
                }
            }
        }
    };
    view::giveaway_card(ctx, page.guild_id, g, image, side, Some(extra))
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
    let waiting = store::pending_count(&ctx, guild)?;
    let all = store::all_giveaways(&ctx, guild)?;
    // Confirmed giveaways per giver, for everyone: the stats' top list is cut at ten.
    let mut given: HashMap<u64, i64> = HashMap::new();
    for g in all.iter().filter(|g| g.status == Status::Confirmed) {
        *given.entry(g.giver_id).or_default() += 1;
    }
    let top = store::stats(&all).top_givers;
    let edit = page.can_edit();
    let actions = edit.then(|| {
        header_pop("Ban a giver", html! {
            form.pform method="post" action=(page.path("pets/bans")) {
                (members_datalist(&ctx, guild))
                (ui::card(html! {
                    (ui::field_custom("Member", Some("Name as shown in the server, or their ID."), html! {
                        input.input type="text" name="member" list="pets-members" required placeholder="Name or ID" autocomplete="off";
                    }))
                    (ui::field_text("reason", "Reason", None, "", "Took a pet back after the giveaway"))
                }))
                div.acts { button.btn.primary type="submit" { "Ban giver" } }
            }
        })
    });
    let body = html! {
        (tabs(&page, "Bans", waiting))
        div.with-rail {
            div.stack.pets-main {
                (view::section(Icon::Shield, "Banned givers", Some("They can't be recorded as the giver of a pet giveaway. They can still win."), None, html! {
                    @if list.is_empty() {
                        @if edit {
                            (ui::empty(Icon::Shield, "Nobody is banned", "Ban a member with the button above and they can't be recorded as a giver."))
                        } @else {
                            (ui::empty(Icon::Shield, "Nobody is banned", "Members staff ban from giving pets show here."))
                        }
                    } @else {
                        div.pets-rows.pets-tiles.wide {
                            @for b in &list {
                                @let n = given.get(&b.user_id).copied().unwrap_or(0);
                                div.pets-row {
                                    img.avatar src=(ctx.member_avatar(guild, b.user_id)) alt="" loading="lazy";
                                    div.pets-row-main {
                                        span { strong { (ctx.display_name(guild, b.user_id)) } (ui::badge("Can't give", Tone::Bad)) }
                                        @if b.reason.is_empty() { span.muted.small { "No reason given" } } @else { span.pets-reason { (b.reason) } }
                                    }
                                    div.pets-row-side {
                                        span { "Banned " (text::ago(b.banned_at)) " by " (b.banned_by) }
                                        span { (text::num(n)) " " (if n == 1 { "pet given before" } else { "pets given before" }) }
                                    }
                                    @if edit {
                                        div.pets-row-acts { (ui::action_button(&page.path(&format!("pets/bans/{}/delete", b.user_id)), "Unban", Tone::Muted, false)) }
                                    }
                                }
                            }
                        }
                    }
                }))
            }
            aside.rail {
                div.rail-block {
                    h2 { "Top givers" }
                    @if top.is_empty() {
                        p.muted.small { "Givers show here once a pet giveaway is confirmed." }
                    }
                    @for (u, n) in top.iter().take(6) {
                        div.mini {
                            img.avatar src=(ctx.member_avatar(guild, *u)) alt="" loading="lazy";
                            div.grow {
                                span { (ctx.display_name(guild, *u)) }
                                span.muted {
                                    (text::num(*n)) " confirmed"
                                    @if list.iter().any(|b| b.user_id == *u) { " · " span.pets-bad { "banned" } }
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    shell(&ctx, &page, actions, body)
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
