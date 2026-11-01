//! Dashboard pages: the collection, approvals, giveaways with their settings, the catalog
//! (editing pets and their pictures, search, bulk removal) and the giver ban list.

use std::collections::{HashMap, HashSet};

use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::{IntoResponse, Redirect};
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::twilight_model::channel::message::AllowedMentions;
use bot_core::twilight_model::id::Id;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};
use serde::Deserialize;

use crate::store::{self, Giveaway, Pet, Status};
use crate::{KEY, Settings, idleon, picture, view};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/pets", get(overview).post(save_settings))
        .route("/g/{guild}/pets/approvals", get(approvals))
        .route("/g/{guild}/pets/approvals/approve", post(approve))
        .route("/g/{guild}/pets/approvals/reject", post(reject))
        .route("/g/{guild}/pets/approvals/test", post(test_review_dms))
        .route("/g/{guild}/pets/catalog", get(catalog).post(save_pet))
        .route("/g/{guild}/pets/catalog/remove", post(remove_pets))
        .route("/g/{guild}/pets/catalog/{id}/delete", post(delete_pet))
        .route("/g/{guild}/pets/catalog/{id}/owned", post(change_owned))
        .route("/g/{guild}/pets/categories", post(add_category))
        .route("/g/{guild}/pets/categories/delete", post(delete_category))
        .route("/g/{guild}/pets/giveaways", get(giveaways).post(record_giveaway))
        .route("/g/{guild}/pets/giveaways/test-post", post(test_post))
        .route("/g/{guild}/pets/giveaways/test-dms", post(test_giveaway_dms))
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

/// Styles only the dashboard's pet pages use.
const DASH_CSS: &str = r#"
/* Discord's look for the announcement preview. */
.pets-dc { border-radius: 12px; overflow: hidden; background: #313338; color: #dbdee1; font-size: 14.5px; box-shadow: 0 22px 44px -24px rgba(0, 0, 0, 0.9), inset 0 0 0 1px rgba(255, 255, 255, 0.06); }
.pets-dc-head { display: flex; align-items: center; gap: 6px; height: 40px; padding: 0 14px; color: #f2f3f5; font-weight: 600; font-size: 14px; box-shadow: 0 1px 0 rgba(4, 4, 5, 0.25); }
.pets-dc-head .icon { color: #80848e; width: 18px; height: 18px; }
.pets-dc-msg { display: grid; grid-template-columns: 40px minmax(0, 1fr); gap: 0 14px; padding: 12px 14px 16px; }
.pets-dc-av { width: 40px; height: 40px; border-radius: 50%; object-fit: cover; }
span.pets-dc-av { display: grid; place-items: center; background: var(--accent-lo); }
span.pets-dc-av img { width: 22px; height: 22px; }
.pets-dc-name { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; line-height: 1.375; }
.pets-dc-name strong { color: #f2f3f5; font-weight: 600; }
.pets-dc-app { display: inline-flex; align-items: center; height: 15px; padding: 0 4px; border-radius: 3px; background: #5865f2; color: #fff; font: 600 10px/1 var(--sans); }
.pets-dc-time { color: #949ba4; font-size: 12px; }
.pets-dc-text { line-height: 1.375; overflow-wrap: anywhere; }
.pets-dc-text b { color: #f2f3f5; }
.pets-dc-mention { background: rgba(88, 101, 242, 0.3); color: #c9cdfb; border-radius: 3px; padding: 0 2px; font-weight: 500; }
.pets-split { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 360px); gap: 24px; align-items: start; }
.pets-preview { display: flex; flex-direction: column; gap: 10px; }
.pets-cap { font-size: 12.5px; color: var(--ink-2); }
.pets-preview .btn { align-self: flex-start; }
@container col (max-width: 940px) { .pets-split { grid-template-columns: 1fr; } .pets-preview { max-width: 480px; } }

/* Approvals: the list's head, and the bulk actions under the rows. */
.pets-list-head { display: flex; align-items: center; gap: 8px 18px; flex-wrap: wrap; padding: 0 2px 12px; font-size: 13px; color: var(--ink-2); }
.pets-check { display: inline-flex; align-items: center; gap: 8px; font-size: 13px; color: var(--ink-2); cursor: pointer; white-space: nowrap; }
.pets-check input { width: 15px; height: 15px; margin: 0; accent-color: var(--accent); }
.pets-bulk { grid-column: 1 / -1; display: flex; gap: 8px; align-items: center; flex-wrap: wrap; margin-top: -1px; padding: 12px 0 14px; border-top: 1px solid var(--edge-1); }
.pets-bulk .input { flex: 1 1 220px; height: 32px; }

/* Catalog: search and bulk removal above the lists. */
.pets-tools { display: flex; align-items: center; gap: 10px 16px; flex-wrap: wrap; }
.pets-search { position: relative; flex: 1 1 260px; max-width: 440px; }
.pets-search .icon { position: absolute; left: 12px; top: 50%; width: 15px; height: 15px; transform: translateY(-50%); color: var(--ink-3); pointer-events: none; }
.pets-search .input { padding-left: 36px; }
.pets-search .input::-webkit-search-cancel-button { filter: grayscale(1); }
.pets-bulk-form { display: flex; align-items: center; gap: 12px; margin-left: auto; }
.pets-bulk-form .btn:disabled { opacity: 0.45; pointer-events: none; }
[data-pet][hidden], [data-cat][hidden], #pets-none[hidden] { display: none !important; }
.pets-entry.missing .pa { filter: grayscale(0.7); opacity: 0.62; }
.pets-entry .pets-li-title a { color: inherit; min-width: 0; overflow: hidden; }
.pets-entry .pets-li-title a:hover strong { text-decoration: underline; text-underline-offset: 3px; text-decoration-color: var(--edge-3); }
.pets-list.cat { grid-template-columns: repeat(auto-fill, minmax(min(100%, 320px), 1fr)); column-gap: 28px; }
.pets-stepper { display: inline-flex; align-items: center; gap: 2px; padding: 3px; border-radius: var(--r-key); background: var(--control); }
.pets-stepper .btn { width: 26px; height: 26px; padding: 0; border-radius: 7px; font-size: 15px; }
.pets-stepper > span { min-width: 3ch; text-align: center; font-size: 13px; font-weight: 600; font-variant-numeric: tabular-nums; }
.pets-owned { font-size: 13px; font-weight: 600; color: var(--ink-2); font-variant-numeric: tabular-nums; }
.pets-li.editing { display: block; grid-column: 1 / -1; padding: 18px 0 20px; scroll-margin-top: 90px; }
.pets-li.editing::before { opacity: 0.35; background: var(--hover); }
.pets-edit-head { display: flex; align-items: center; gap: 14px; margin-bottom: 16px; }
.pets-edit-head h3 { font-size: 15px; font-weight: 600; }
.pets-edit-head p { font-size: 12.5px; color: var(--ink-2); margin-top: 2px; }
.pets-entry { scroll-margin-top: 90px; }
.cat-list { display: flex; flex-wrap: wrap; gap: 6px; }
.cat-list .chip { cursor: default; gap: 8px; }
.cat-list .chip form button { background: none; border: 0; color: var(--ink-3); cursor: pointer; padding: 0; font: inherit; }
.cat-list .chip form button:hover, .cat-list .chip form button.armed { color: var(--held); }

/* The pet form: the picture's drop zone beside the fields. */
.pform { display: flex; flex-direction: column; gap: 18px; container: pform / inline-size; }
.pform-grid { display: grid; grid-template-columns: 200px minmax(0, 1fr); gap: 20px; align-items: start; }
.pform-fields { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 14px 16px; }
.pf { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.pf > span { font-size: 12.5px; font-weight: 500; color: var(--ink-2); }
.pf > small { font-size: 12px; color: var(--ink-3); }
.pf.wide { grid-column: 1 / -1; }
.pf textarea.input { height: auto; min-height: 64px; padding: 9px 12px; resize: vertical; line-height: 1.45; }
.pf .seg { align-self: flex-start; }
.pf .seg label { display: inline-flex; align-items: center; gap: 6px; }
.pf .seg .rar { font-size: 12.5px; color: inherit; }
.pform-acts { display: flex; gap: 8px; align-items: center; justify-content: flex-end; flex-wrap: wrap; }
.pform-acts .btn.danger { order: -1; margin-right: auto; }
.pets-drop { display: flex; flex-direction: column; gap: 10px; }
.pets-drop-hit { position: relative; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 10px; min-height: 184px; padding: 16px 14px; border-radius: 14px; border: 1.5px dashed var(--edge-3); background: var(--well); text-align: center; cursor: pointer; transition: border-color 0.2s, background 0.2s; }
.pets-drop-hit:hover, .pets-drop.over .pets-drop-hit { border-color: color-mix(in oklab, var(--accent) 70%, transparent); background: var(--accent-wash); }
.pets-drop:has(input[type=file]:focus-visible) .pets-drop-hit { outline: 2px solid var(--focus); outline-offset: 2px; }
.pets-drop input[type=file] { position: absolute; width: 1px; height: 1px; opacity: 0; pointer-events: none; }
.pets-drop-art { display: grid; place-items: center; width: 88px; height: 88px; font-size: 46px; line-height: 1; transition: transform 0.4s var(--spring), opacity 0.2s; }
.pets-drop-art img { max-width: 88px; max-height: 88px; object-fit: contain; }
.pets-drop:not(.has) .pets-drop-art { opacity: 0.4; filter: grayscale(1); }
.pets-drop.fresh .pets-drop-art { animation: pets-in 0.5s var(--spring) both; }
.pets-drop.busy .pets-drop-art { opacity: 0.3; }
.pets-drop-text { display: flex; flex-direction: column; gap: 3px; }
.pets-drop-text strong { font-size: 13px; font-weight: 600; }
.pets-drop-text span { font-size: 12px; color: var(--ink-3); }
.pets-drop-text u { color: var(--ink); text-decoration-color: var(--edge-3); text-underline-offset: 3px; }
.pets-pop .pform-grid { grid-template-columns: 1fr; }
.pets-pop .pets-drop-hit { flex-direction: row; justify-content: flex-start; min-height: 0; padding: 12px 14px; text-align: left; }
.pets-pop .pets-drop-art { width: 56px; height: 56px; font-size: 32px; }
.pets-pop .pets-drop-art img { max-width: 56px; max-height: 56px; }
@container pform (max-width: 560px) {
  .pform-grid { grid-template-columns: 1fr; }
  .pets-drop-hit { flex-direction: row; justify-content: flex-start; min-height: 0; text-align: left; }
  .pets-drop-art { width: 56px; height: 56px; font-size: 32px; }
  .pets-drop-art img { max-width: 56px; max-height: 56px; }
}
@container pform (max-width: 420px) { .pform-fields { grid-template-columns: 1fr; } }

/* What idleontoolbox.com has that the catalog doesn't. */
.pets-io { display: flex; flex-direction: column; gap: 16px; padding: 18px 20px 20px; border-radius: var(--r-card); background: var(--well); box-shadow: inset 0 0 0 1px var(--edge-2); scroll-margin-top: 90px; }
.pets-io.bad { box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--held) 45%, transparent); }
.pets-io-head { display: flex; align-items: flex-start; gap: 10px; flex-wrap: wrap; }
.pets-io-head h2 { font-size: 15px; }
.pets-io-head p { font-size: 13px; color: var(--ink-2); margin-top: 4px; max-width: 80ch; }
.pets-io-form { display: flex; flex-direction: column; gap: 12px; }
.pets-io-sub { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding-top: 4px; }
.pets-io-sub h3 { font-size: 13.5px; font-weight: 600; }
.pets-io-sub:first-child { justify-content: flex-end; padding-top: 0; }
.pets-picks { display: grid; grid-template-columns: repeat(auto-fill, minmax(108px, 1fr)); gap: 4px; }
.pets-pick { position: relative; display: flex; flex-direction: column; align-items: center; gap: 6px; min-width: 0; padding: 12px 6px 10px; border-radius: 12px; cursor: pointer; text-align: center; transition: background 0.2s, opacity 0.2s; }
.pets-pick:hover { background: var(--hover); }
.pets-pick input { position: absolute; top: 8px; left: 8px; width: 15px; height: 15px; margin: 0; accent-color: var(--accent); }
.pets-pick:not(:has(input:checked)) { opacity: 0.45; }
.pets-pick:not(:has(input:checked)) img { filter: grayscale(1); }
.pets-pick-art { display: grid; place-items: center; height: 52px; }
.pets-pick-art img { max-width: 56px; max-height: 52px; object-fit: contain; }
.pets-pick-name { max-width: 100%; font-size: 12.5px; font-weight: 500; line-height: 1.3; overflow-wrap: anywhere; }
.pets-io-foot { display: flex; justify-content: flex-end; padding-top: 4px; }

@media (max-width: 920px) {
  .pets-bulk-form { margin-left: 0; }
  .pets-search { max-width: none; }
}
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

/// A toast after a test was sent (`?sent=...` in the address).
fn toast_if(show: bool, msg: &str) -> Markup {
    html! {
        @if show {
            script { (PreEscaped(format!(
                "(()=>{{const go=()=>{{window.botToast&&window.botToast({msg:?});history.replaceState(history.state,'',location.pathname)}};document.readyState==='loading'?addEventListener('DOMContentLoaded',go):setTimeout(go)}})()"
            ))) }
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

fn missing_pet(id: &str) -> Pet {
    Pet {
        id: id.into(),
        name: id.into(),
        emoji: String::new(),
        category: String::new(),
        rarity: "common".into(),
        description: String::new(),
        bonus: String::new(),
        image: String::new(),
        guild: 0,
        picture: None,
    }
}

/// Approve and reject feel instant: the row slides away while the page's answer is on its way.
/// The answer then puts the list in its new state. Rows carry `data-row`, forms
/// `data-review-form`, and the buttons name the submission (`only`), all of them (`all`) or
/// the ticked ones.
const REVIEW_JS: &str = r##"(() => {
  const main = document.querySelector("main.page");
  if (!main) return;
  main.querySelectorAll("form[data-review-form]").forEach((f) => f.addEventListener("submit", (e) => {
    const b = e.submitter;
    if (!b) return;
    const no = (b.getAttribute("formaction") || f.getAttribute("action") || "").endsWith("/reject");
    let rows = [...main.querySelectorAll("[data-row]")];
    if (b.name === "only") rows = rows.filter((r) => r.dataset.row === b.value);
    else if (b.name !== "all") rows = rows.filter((r) => r.querySelector('input[name="sel"]:checked'));
    rows.forEach((r) => r.classList.add("leaving", ...(no ? ["no"] : [])));
    // Still here after a while: the answer was a refusal, so bring them back.
    setTimeout(() => rows.forEach((r) => r.classList.remove("leaving", "no")), 4000);
  }));
  const all = main.querySelector("#sel-all");
  all?.addEventListener("change", () => main.querySelectorAll('input[name="sel"]').forEach((c) => { c.checked = all.checked; }));
})();"##;

// ---- collection (the first tab) ----

const DAY_MS: i64 = 86_400_000;

/// `"2 h left"` for a giveaway's expiry.
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
                        div.pets-req data-row=(p.id) {
                            (view::pet_art(&pet, "md"))
                            div {
                                div { strong { (pet.name) } @if p.quantity > 1 { span.qty { " ×" (p.quantity) } } }
                                div.pets-by {
                                    img.avatar src=(ctx.member_avatar(guild, p.user_id)) alt="" loading="lazy";
                                    b { (ctx.display_name(guild, p.user_id)) } span { (text::ago(p.requested_at)) }
                                }
                                @if page.can_edit() {
                                    form.pets-req-acts method="post" action=(page.path("pets/approvals/approve")) data-review-form {
                                        input type="hidden" name="back" value="collection";
                                        button.btn.small.outline type="submit" name="only" value=(p.id) { (ui::icon(Icon::Check)) "Approve" }
                                        button.btn.small.ghost type="submit" name="only" value=(p.id) formaction=(page.path("pets/approvals/reject")) { "Reject" }
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
                                Status::Pending => span.pets-state.soon[g.expires_at.is_some()] { (left_text(g.expires_at)) },
                                Status::Confirmed => span.pets-state.ok { "Confirmed" },
                                Status::Expired => span.pets-state.bad { "Expired" },
                                Status::Cancelled => span.pets-state { "Cancelled" },
                            }
                        }
                    }
                    @if let Some(rate) = gstats.confirmation_rate() {
                        p.muted.small { (rate) "% of giveaways go through, " (gstats.confirmed) " confirmed so far." }
                    }
                }
            }
        }
        @if page.can_edit() && !pending.is_empty() { script { (PreEscaped(REVIEW_JS)) } }
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

#[derive(Deserialize, Default)]
struct SentQuery {
    sent: Option<String>,
}

async fn approvals(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<SentQuery>) -> WebResult {
    let page = open(&ctx, &user, guild, Level::View)?;
    let catalog = store::catalog(&ctx, guild)?;
    let find = |id: &str| catalog.iter().find(|p| p.id == id).cloned().unwrap_or_else(|| missing_pet(id));
    let pending = store::pending(&ctx, guild)?;
    let done = store::reviewed(&ctx, guild, 8)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let edit = page.can_edit();
    let approve = page.path("pets/approvals/approve");
    let reject = page.path("pets/approvals/reject");
    let copies: i64 = pending.iter().map(|p| p.quantity).sum();
    let submitters = pending.iter().map(|p| p.user_id).collect::<HashSet<_>>().len() as i64;
    let body = html! {
        (tabs(&page, "Approvals", pending.len() as i64))
        (toast_if(q.sent.is_some(), "Test DMs sent. Check your DMs."))
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
                    form method="post" action=(approve) data-review-form {
                        // Enter in the reason box must not approve anything.
                        button type="submit" disabled hidden {}
                        div.pets-list-head {
                            @if edit { label.pets-check { input #sel-all type="checkbox"; "Select all" } }
                            span { "Approved pets join the collection, credited to whoever submitted them." }
                        }
                        div.pets-list {
                            @for p in &pending {
                                @let pet = find(&p.pet_id);
                                div class={ "pets-li wide-text r-" (store::clean_rarity(&pet.rarity)) } data-row=(p.id) {
                                    @if edit { input type="checkbox" name="sel" value=(p.id) aria-label={ "Select " (pet.name) " from " (ctx.display_name(guild, p.user_id)) }; }
                                    (view::pet_art(&pet, "md"))
                                    div.pets-li-main {
                                        span.pets-li-title {
                                            strong { (pet.name) }
                                            @if p.quantity > 1 { span.qty { "×" (p.quantity) } }
                                            (view::rarity(&pet.rarity))
                                        }
                                        span.pets-by data-tip={ "Submitted: " (page.user.local(p.requested_at).strftime("%b %-d, %H:%M")) } {
                                            img.avatar src=(ctx.member_avatar(guild, p.user_id)) alt="" loading="lazy";
                                            b { (ctx.display_name(guild, p.user_id)) } span { (text::ago(p.requested_at)) }
                                        }
                                    }
                                    @if edit {
                                        div.pets-li-acts {
                                            button.btn.small.outline type="submit" formaction=(approve) name="only" value=(p.id) { (ui::icon(Icon::Check)) "Approve" }
                                            button.btn.small.ghost type="submit" formaction=(reject) name="only" value=(p.id) { "Reject" }
                                        }
                                    }
                                }
                            }
                            @if edit {
                                div.pets-bulk {
                                    input.input type="text" name="reason" maxlength="300" placeholder="Reason for rejecting, sent to them (optional)" autocomplete="off" aria-label="Reason for rejecting";
                                    button.btn.small type="submit" formaction=(reject) { "Reject selected" }
                                    button.btn.small type="submit" formaction=(approve) { "Approve selected" }
                                    button.btn.small.primary type="submit" formaction=(approve) name="all" value="1" { "Approve all" }
                                }
                            }
                        }
                    }
                    @if edit { script { (PreEscaped(REVIEW_JS)) } }
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
                        @let by = p.reviewed_by.as_deref().filter(|b| !b.is_empty());
                        div.mini {
                            img.avatar src=(ctx.member_avatar(guild, p.user_id)) alt="" loading="lazy";
                            div.grow {
                                span { strong { (pet.name) } @if p.quantity > 1 { " ×" (p.quantity) } span.muted { " from " (ctx.display_name(guild, p.user_id)) } }
                                span.muted data-tip=[by.map(|b| format!("Reviewed by: {b}"))] {
                                    span class=(if approved { "pets-ok" } else { "pets-bad" }) { (if approved { "Approved" } else { "Rejected" }) }
                                    @if let Some(at) = p.reviewed_at { " " (text::ago(at)) }
                                }
                                @if !p.reason.is_empty() { span.muted title=(p.reason) { "“" (p.reason) "”" } }
                            }
                            (view::pet_art(&pet, "sm"))
                        }
                    }
                }
                div.rail-block {
                    h2 { "Review DMs" }
                    @if s.dm_reviews {
                        p.muted.small { "Members get a DM when you approve or reject their submission, with your reason if you gave one." }
                    } @else {
                        p.muted.small { "Off. Members aren't told when you review their submission. Turn it on under Giveaway settings." }
                    }
                    @if edit {
                        form method="post" action=(page.path("pets/approvals/test")) {
                            button.btn.small.outline type="submit" { "Send me a test" }
                        }
                    }
                }
            }
        }
    };
    shell(&ctx, &page, None, body)
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
        if s.dm_reviews {
            crate::dm_later(&ctx, p.user_id, crate::review_dm(p.quantity, &pet, &server, approve, &reason));
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

/// DM someone, or say why it didn't go.
async fn dm_or_why(ctx: &Ctx, user: u64, content: &str) -> WebResult<()> {
    match ctx.dm(user, content, &[], &[]).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(WebError::Bad("Couldn't DM you. Allow direct messages from this server's members, then try again.".into())),
        Err(e) => Err(WebError::Bad(bot_core::discord::why_refused_any(&e))),
    }
}

/// A pet to show in a test message: the first one waiting, or the first in the catalog.
fn sample_pet(ctx: &Ctx, guild: u64) -> WebResult<(String, String)> {
    let catalog = store::catalog(ctx, guild)?;
    let waiting = store::pending(ctx, guild)?;
    let pet = waiting.first().and_then(|w| catalog.iter().find(|p| p.id == w.pet_id)).or(catalog.first());
    Ok(pet.map(|p| (p.icon().to_owned(), p.name.clone())).unwrap_or_else(|| ("🐾".into(), "King Doot".into())))
}

/// The two review DMs a member can get, sent to the person clicking, marked as a test.
async fn test_review_dms(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = open(&ctx, &user, guild, Level::Edit)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    if !s.dm_reviews {
        return Err(WebError::Bad("Review DMs are off. Turn on DM review results under Giveaway settings and save first.".into()));
    }
    let (icon, name) = sample_pet(&ctx, guild)?;
    let server = ctx.guild_name(guild);
    let pet = format!("{icon} {name}");
    dm_or_why(&ctx, page.user.id, &format!("Test from the dashboard. When you approve one:\n{}", crate::review_dm(1, &pet, &server, true, ""))).await?;
    dm_or_why(&ctx, page.user.id, &format!("Test from the dashboard. When you reject one with a reason:\n{}", crate::review_dm(1, &pet, &server, false, "Already counted last week."))).await?;
    ctx.audit(&page.user, Some(guild), KEY, "Sent themselves the pet review DMs as a test");
    Ok(Redirect::to(&page.path("pets/approvals?sent=1")).into_response())
}

// ---- catalog ----

#[derive(Deserialize)]
struct CatalogQuery {
    edit: Option<String>,
    idleon: Option<String>,
}

/// What the search box matches on for a pet.
fn haystack(p: &Pet) -> String {
    format!("{} {} {} {} {}", p.name, store::rarity_label(&p.rarity), p.category, p.bonus, p.description).to_lowercase()
}

async fn catalog(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<CatalogQuery>) -> WebResult {
    let page = open(&ctx, &user, guild, Level::View)?;
    let catalog = store::catalog(&ctx, guild)?;
    let cats = store::categories(&ctx, guild)?;
    let counts = store::owned_counts(&ctx, guild)?;
    let waiting = store::pending_count(&ctx, guild)?;
    let edit = page.can_edit();
    let editing = q.edit.as_deref().filter(|_| edit);
    let actions = edit.then(|| header_pop("Add a pet", pet_form(&page, &cats, None)));
    let copies: i64 = counts.values().sum();
    let base = page.path("pets/catalog");
    let body = html! {
        (tabs(&page, "Catalog", waiting))
        div.with-rail {
            div.stack.pets-main {
                @if edit && q.idleon.is_some() { (idleon::panel(&page, &catalog)) }
                div.pets-tools {
                    label.pets-search {
                        (ui::icon(Icon::Search))
                        input.input #pets-q type="search" placeholder="Search by name, rarity or bonus" autocomplete="off" aria-label="Search pets";
                    }
                    @if edit {
                        form #pets-bulk.pets-bulk-form method="post" action=(page.path("pets/catalog/remove")) {
                            label.pets-check { input #pets-sel-shown type="checkbox"; "Select shown" }
                            button.btn.small.danger type="submit" disabled data-confirm="Click again to remove" { "Remove" }
                        }
                    }
                }
                div.empty #pets-none hidden {
                    span.empty-icon { (ui::icon(Icon::Search)) }
                    div { strong { "No pets match “" span data-q {} "”" } p.muted { "Try part of a name, a rarity like legendary, or a bonus like drop rate." } }
                }
                @for cat in &cats {
                    @let pets: Vec<&Pet> = catalog.iter().filter(|p| &p.category == cat).collect();
                    @let have = pets.iter().filter(|p| counts.contains_key(&p.id)).count();
                    div data-cat {
                        (ui::section(cat, Some(&format!("{} pets, {} collected", pets.len(), have)), html! {
                            @if pets.is_empty() {
                                @if edit {
                                    (ui::empty(Icon::Paw, "No pets here", "Add one with the button above and pick this category, or remove the category."))
                                } @else {
                                    (ui::empty(Icon::Paw, "No pets here", "Pets staff add to this category show here."))
                                }
                            } @else {
                                div.pets-list.cat {
                                    @for p in &pets {
                                        @if editing == Some(p.id.as_str()) {
                                            (edit_row(&page, &cats, p))
                                        } @else {
                                            (catalog_row(&page, &base, p, counts.get(&p.id).copied().unwrap_or(0)))
                                        }
                                    }
                                }
                            }
                        }))
                    }
                }
            }
            aside.rail {
                div.rail-block {
                    h2 { "Catalog" }
                    (ui::rail_numbers(&[
                        ("companions", text::num(catalog.len() as i64)),
                        ("collected", text::num(counts.len() as i64)),
                        ("copies", text::num(copies)),
                    ]))
                    @if edit { p.muted.small { "Click a pet's name to change its name, rarity or picture." } }
                }
                @if edit { (idleon::rail_block(&page)) }
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
        script { (PreEscaped(CATALOG_JS)) }
    };
    shell(&ctx, &page, actions, body)
}

/// A pet in the catalog list: tick box, picture, name and rarity, its bonus, the stepper.
fn catalog_row(page: &Page, base: &str, p: &Pet, n: i64) -> Markup {
    let rarity = store::clean_rarity(&p.rarity);
    let edit = page.can_edit();
    let href = format!("{base}?edit={}#pet-{}", p.id, p.id);
    html! {
        div class={ "pets-li pets-entry r-" (rarity) @if n == 0 { " missing" } } id={ "pet-" (p.id) } data-pet=(haystack(p)) {
            @if edit { input type="checkbox" name="sel" value=(p.id) form="pets-bulk" aria-label={ "Select " (p.name) }; }
            (view::pet_art(p, "md"))
            div.pets-li-main {
                span.pets-li-title {
                    @if edit { a href=(href) { strong { (p.name) } } } @else { strong { (p.name) } }
                    (view::rarity(rarity))
                }
                @if !p.bonus.is_empty() { span.pets-li-sub title=(p.bonus) { (p.bonus) } }
                @else if !p.description.is_empty() { span.pets-li-sub.faint title=(p.description) { (p.description) } }
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
                span.pets-owned aria-label={ (n) " owned" } { "×" (n) }
            }
        }
    }
}

/// The pet being edited, in its own place in the list.
fn edit_row(page: &Page, cats: &[String], p: &Pet) -> Markup {
    html! {
        div class={ "pets-li editing r-" (store::clean_rarity(&p.rarity)) } id={ "pet-" (p.id) } data-pet=(haystack(p)) {
            div.pets-edit-head {
                (view::pet_art(p, "md"))
                div {
                    h3 { "Edit " (p.name) }
                    p { "Its copies, submissions and giveaways stay with it when you rename it." }
                }
            }
            (pet_form(page, cats, Some(p)))
        }
    }
}

/// Add or edit a pet: the picture's drop zone beside name, rarity, category and the rest.
fn pet_form(page: &Page, cats: &[String], p: Option<&Pet>) -> Markup {
    let v = |f: fn(&Pet) -> &str| p.map(f).unwrap_or_default().to_owned();
    let rarity = p.map(|p| store::clean_rarity(&p.rarity)).unwrap_or("common");
    let category = p.map(|p| p.category.clone()).or_else(|| cats.first().cloned()).unwrap_or_default();
    let image = p.and_then(Pet::image_url);
    let back = page.path("pets/catalog");
    html! {
        form.pform method="post" action=(back) {
            @if let Some(p) = p { input type="hidden" name="original" value=(p.id); }
            div.pform-grid {
                div.pets-drop.has[image.is_some()] data-drop {
                    label.pets-drop-hit {
                        span.pets-drop-art {
                            @match &image {
                                Some(src) => img src=(src) alt="";,
                                None => span { (p.map(Pet::icon).unwrap_or("🐾")) },
                            }
                        }
                        span.pets-drop-text {
                            strong data-drop-title { @if image.is_some() { "Drop a new picture to replace it" } @else { "Drop a picture here" } }
                            span { "or " u { "choose a file" } ". PNG, JPEG, WebP or GIF, shrunk to 256 px." }
                        }
                        input type="file" accept="image/png,image/jpeg,image/webp,image/gif";
                    }
                    input type="hidden" name="picture";
                    @if image.is_some() {
                        label.pets-check { input type="checkbox" name="remove_picture"; "Remove the picture" }
                    }
                }
                div.pform-fields {
                    label.pf {
                        span { "Name" }
                        input.input type="text" name="name" value=(v(|p| &p.name)) maxlength="60" required placeholder="King Doot" autocomplete="off";
                    }
                    label.pf {
                        span { "Category" }
                        div.select-wrap {
                            select.input name="category" {
                                @for c in cats { option value=(c) selected[*c == category] { (c) } }
                            }
                            (ui::icon(Icon::Chevron))
                        }
                    }
                    div.pf.wide role="radiogroup" aria-label="Rarity" {
                        span { "Rarity" }
                        div.seg {
                            @for r in store::RARITIES {
                                label { input type="radio" name="rarity" value=(r) checked[r == rarity]; span class={ "rar r-" (r) } { (store::rarity_label(r)) } }
                            }
                        }
                    }
                    label.pf {
                        span { "Bonus" }
                        input.input type="text" name="bonus" value=(v(|p| &p.bonus)) maxlength="200" placeholder="2x Construction Build Speed" autocomplete="off";
                    }
                    label.pf {
                        span { "Emoji" }
                        input.input type="text" name="emoji" value=(v(|p| &p.emoji)) maxlength="16" placeholder="🐾" autocomplete="off";
                        small { "Shown when there's no picture." }
                    }
                    label.pf.wide {
                        span { "Description" }
                        textarea.input name="description" rows="2" maxlength="300" placeholder="One line about the pet." { (v(|p| &p.description)) }
                    }
                }
            }
            div.pform-acts {
                button.btn.primary type="submit" { @if p.is_some() { "Save pet" } @else { "Add pet" } }
                @if let Some(p) = p {
                    a.btn href={ (back) "#pet-" (p.id) } { "Cancel" }
                    button.btn.danger type="submit" formaction=(page.path(&format!("pets/catalog/{}/delete", p.id))) formnovalidate data-confirm="Click again to remove it" { "Remove pet" }
                }
            }
        }
    }
}

/// Search, bulk removal and the picture drop zones on the catalog. Runs on every load and swap.
const CATALOG_JS: &str = r##"(() => {
  const main = document.querySelector("main.page");
  if (!main) return;
  const signal = window.botPage?.signal;
  const KEY = "pets-catalog-q";

  // ---- search: filters as you type; a save or a +/- on the same page keeps it
  const q = main.querySelector("#pets-q");
  const rows = [...main.querySelectorAll("[data-pet]")];
  const none = main.querySelector("#pets-none");
  const bulk = main.querySelector("#pets-bulk");
  const remove = bulk?.querySelector('button[type="submit"]');
  const shownBox = main.querySelector("#pets-sel-shown");
  const boxes = rows.map((r) => r.querySelector('input[name="sel"]')).filter(Boolean);
  const visible = (b) => !b.closest("[data-pet]").hidden;
  const sync = () => {
    if (!bulk) return;
    const on = boxes.filter((b) => b.checked && visible(b)).length;
    const shown = boxes.filter(visible);
    remove.disabled = on === 0;
    if (!remove.classList.contains("armed")) remove.textContent = on ? `Remove ${on} ${on === 1 ? "pet" : "pets"}` : "Remove";
    remove.dataset.confirm = `Click again to remove ${on}`;
    shownBox.checked = shown.length > 0 && shown.every((b) => b.checked);
    shownBox.indeterminate = !shownBox.checked && shown.some((b) => b.checked);
  };
  const filter = () => {
    const words = (q?.value || "").trim().toLowerCase().split(/\s+/).filter(Boolean);
    let hits = 0;
    rows.forEach((r) => {
      const hit = r.classList.contains("editing") || words.every((w) => r.dataset.pet.includes(w));
      r.hidden = !hit;
      if (hit) hits++;
    });
    main.querySelectorAll("[data-cat]").forEach((c) => { c.hidden = words.length > 0 && !c.querySelector("[data-pet]:not([hidden])"); });
    if (none) {
      none.hidden = !words.length || hits > 0;
      none.querySelector("[data-q]").textContent = (q?.value || "").trim();
    }
    try { sessionStorage.setItem(KEY, q?.value || ""); } catch (_) {}
    sync();
  };
  if (q) {
    let kept = "";
    try { kept = sessionStorage.getItem(KEY) || ""; } catch (_) {}
    if (document.documentElement.classList.contains("same") && kept) q.value = kept;
    q.addEventListener("input", filter);
    q.addEventListener("keydown", (e) => { if (e.key === "Escape" && q.value) { e.preventDefault(); e.stopPropagation(); q.value = ""; filter(); } });
  }

  // ---- bulk removal: tick rows, then one Remove that asks for a second click
  main.addEventListener("change", (e) => { if (e.target.name === "sel") sync(); });
  shownBox?.addEventListener("change", () => { boxes.forEach((b) => { if (visible(b)) b.checked = shownBox.checked; }); sync(); });
  bulk?.addEventListener("submit", () => {
    // Only rows on screen go: a ticked row the search hides stays.
    boxes.forEach((b) => { b.disabled = !visible(b); });
    const going = boxes.filter((b) => b.checked && !b.disabled).map((b) => b.closest("[data-pet]"));
    going.forEach((r) => r.classList.add("leaving", "no"));
    setTimeout(() => { boxes.forEach((b) => { b.disabled = false; }); going.forEach((r) => r.classList.remove("leaving", "no")); }, 4000);
  });
  filter();

  // ---- pictures: drop, pick or paste; shrunk here before they're sent
  const MAX = 1024 * 1024;
  const read = (file) => new Promise((ok, no) => { const r = new FileReader(); r.onload = () => ok(r.result); r.onerror = () => no(new Error("Couldn't read that file")); r.readAsDataURL(file); });
  const shrink = async (file) => {
    if (!/^image\/(png|jpeg|webp|gif)$/.test(file.type)) throw new Error("Pick a PNG, JPEG, WebP or GIF image");
    if (file.type === "image/gif") {
      if (file.size > MAX) throw new Error("That GIF is over 1 MB");
      return read(file);
    }
    const bmp = await createImageBitmap(file);
    const k = Math.min(1, 256 / Math.max(bmp.width, bmp.height));
    const c = document.createElement("canvas");
    c.width = Math.max(1, Math.round(bmp.width * k));
    c.height = Math.max(1, Math.round(bmp.height * k));
    const g = c.getContext("2d");
    g.imageSmoothingQuality = "high";
    g.drawImage(bmp, 0, 0, c.width, c.height);
    let url = c.toDataURL("image/webp", 0.92);
    if (!url.startsWith("data:image/webp")) url = c.toDataURL("image/png");
    return url;
  };
  main.querySelectorAll("[data-drop]").forEach((zone) => {
    const file = zone.querySelector('input[type="file"]');
    const field = zone.querySelector('input[name="picture"]');
    const art = zone.querySelector(".pets-drop-art");
    const title = zone.querySelector("[data-drop-title]");
    const take = async (f) => {
      if (!f) return;
      zone.classList.add("busy");
      try {
        const url = await shrink(f);
        field.value = url;
        const img = new Image();
        img.alt = "";
        img.src = url;
        art.replaceChildren(img);
        zone.classList.remove("fresh");
        void zone.offsetWidth;
        zone.classList.add("has", "fresh");
        const rm = zone.querySelector('input[name="remove_picture"]');
        if (rm) rm.checked = false;
        title.textContent = `${f.name || "Pasted picture"}, ready to save`;
      } catch (err) {
        window.botToast?.(err.message || "Couldn't read that picture", true);
      } finally {
        zone.classList.remove("busy");
        file.value = "";
      }
    };
    const files = (e) => [...(e.dataTransfer?.types || [])].includes("Files");
    ["dragenter", "dragover"].forEach((t) => zone.addEventListener(t, (e) => { if (files(e)) { e.preventDefault(); zone.classList.add("over"); } }));
    zone.addEventListener("dragleave", (e) => { if (!zone.contains(e.relatedTarget)) zone.classList.remove("over"); });
    zone.addEventListener("drop", (e) => { e.preventDefault(); zone.classList.remove("over"); take(e.dataTransfer.files[0]); });
    file.addEventListener("change", () => take(file.files[0]));
    zone.closest("form")?.addEventListener("paste", (e) => {
      const f = [...(e.clipboardData?.files || [])].find((x) => x.type.startsWith("image/"));
      if (f) { e.preventDefault(); take(f); }
    });
  });
  // A picture dropped beside the zone shouldn't open in the tab.
  const stray = (e) => { if ([...(e.dataTransfer?.types || [])].includes("Files")) e.preventDefault(); };
  addEventListener("dragover", stray, { signal });
  addEventListener("drop", stray, { signal });
  main.querySelector(".pets-li.editing input[name=name]")?.focus({ preventScroll: true });
})();"##;

/// What changed in an edit, for the audit log: "renamed it to X, made it Rare, a new picture".
fn changes(old: &Pet, new: &Pet, picture: Option<bool>) -> Vec<String> {
    let mut out = Vec::new();
    if old.name != new.name {
        out.push(format!("renamed it to {}", new.name));
    }
    if store::clean_rarity(&old.rarity) != store::clean_rarity(&new.rarity) {
        out.push(format!("made it {}", store::rarity_label(&new.rarity)));
    }
    if old.category != new.category {
        out.push(format!("moved it to {}", new.category));
    }
    match picture {
        Some(true) => out.push("gave it a new picture".into()),
        Some(false) => out.push("removed its picture".into()),
        None => {}
    }
    if old.emoji != new.emoji || old.bonus != new.bonus || old.description != new.description {
        out.push("changed its details".into());
    }
    out
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
    let old = match form.get("original") {
        Some(id) => Some(store::pet(&ctx, guild, id)?.ok_or_else(|| WebError::Bad("That pet was removed in the meantime.".into()))?),
        None => None,
    };
    if let Some(other) = store::name_taken(&ctx, guild, &name, old.as_ref().map(|p| p.id.as_str()))? {
        return Err(WebError::Bad(format!("There's already a pet called {} in {}.", other.name, other.category)));
    }
    // A new picture, checked before anything is saved.
    let upload = match form.get("picture") {
        Some(url) => Some(picture::from_data_url(url).map_err(WebError::Bad)?),
        None => None,
    };
    let remove = form.bool("remove_picture") && upload.is_none();
    let id = match &old {
        Some(p) => p.id.clone(),
        None => store::new_pet_id(&ctx, guild, &name)?,
    };
    let pet = Pet {
        id,
        name: name.clone(),
        emoji: text::clip(&form.string("emoji"), 16),
        category,
        rarity: store::clean_rarity(&form.string("rarity")).to_owned(),
        description: text::clip(&form.text("description"), 300),
        bonus: text::clip(&form.string("bonus"), 200),
        // A link from before pictures were uploaded stays until the picture is replaced or removed.
        image: match &old {
            Some(p) if upload.is_none() && !remove => p.image.clone(),
            _ => String::new(),
        },
        guild,
        picture: None,
    };
    store::save_pet(&ctx, guild, &pet)?;
    let had_picture = old.as_ref().is_some_and(Pet::has_picture);
    let picture_change = if let Some((mime, bytes)) = &upload {
        store::set_picture(&ctx, guild, &pet.id, mime, bytes)?;
        Some(true)
    } else if remove {
        store::remove_picture(&ctx, guild, &pet.id)?;
        had_picture.then_some(false)
    } else {
        None
    };
    match &old {
        Some(o) => {
            store::sync_giveaways(&ctx, guild, &pet)?;
            let what = changes(o, &pet, picture_change);
            if !what.is_empty() {
                ctx.audit(&page.user, Some(guild), KEY, format!("Edited the pet {}: {}", o.name, what.join(", ")));
            }
        }
        None => ctx.audit(
            &page.user,
            Some(guild),
            KEY,
            format!("Added the pet {name} to the catalog{}", if upload.is_some() { " with a picture" } else { "" }),
        ),
    }
    Ok(Redirect::to(&format!("{}?saved=1#pet-{}", page.path("pets/catalog"), pet.id)).into_response())
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

/// Remove the ticked pets in one go, each as a single removal does.
async fn remove_pets(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = open(&ctx, &user, guild, Level::Edit)?;
    let ids: Vec<&str> = form.all("sel");
    if ids.is_empty() {
        return Err(WebError::Bad("Tick at least one pet first.".into()));
    }
    let (names, copies) = store::delete_pets(&ctx, guild, &ids)?;
    let owned = if copies > 0 { format!(" and {copies} owned") } else { String::new() };
    match names.as_slice() {
        [] => {}
        [one] => ctx.audit(&page.user, Some(guild), KEY, format!("Removed the pet {one}{owned}")),
        many => ctx.audit(&page.user, Some(guild), KEY, format!("Removed {} pets{owned}: {}", many.len(), text::clip(&many.join(", "), 300))),
    }
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
    Ok(Redirect::to(&format!("{}?saved=1#pet-{}", page.path("pets/catalog"), pet.id)).into_response())
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

async fn giveaways(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<SentQuery>) -> WebResult {
    let page = open(&ctx, &user, guild, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let catalog = store::catalog(&ctx, guild)?;
    let counts = store::owned_counts(&ctx, guild)?;
    let active = store::active_giveaways(&ctx, guild)?;
    let past = store::past_giveaways(&ctx, guild, 50)?;
    let comments = store::comments(&ctx, guild)?;
    let stats = store::stats(&store::all_giveaways(&ctx, guild)?);
    let waiting = store::pending_count(&ctx, guild)?;
    let edit = page.can_edit();
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
    let image = |id: &str| catalog.iter().find(|p| p.id == id).and_then(Pet::image_url);
    let actions = edit.then(|| {
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
                div.pform-acts { button.btn.primary type="submit" { "Record giveaway" } }
            }
        })
    });
    // The announcement preview shows the latest confirmed giveaway, or a waiting one.
    let example = past.iter().find(|g| g.status == Status::Confirmed).or(active.first());
    let bot = bot_face(&ctx);
    let channel = s.announce_channel.and_then(|c| ctx.text_channels(guild).into_iter().find(|x| x.0 == c)).map(|(_, n)| n);
    let sent = match q.sent.as_deref() {
        Some("post") => Some("Test post sent"),
        Some("dms") => Some("Test DMs sent. Check your DMs."),
        _ => None,
    };
    let body = html! {
        (tabs(&page, "Giveaways", waiting))
        (toast_if(sent.is_some(), sent.unwrap_or_default()))
        div.with-rail {
            div.stack.pets-main {
                (view::readings(&[
                    ("waiting for a hand-over", text::num(stats.active)),
                    ("confirmed", text::num(stats.confirmed)),
                    ("go through", stats.confirmation_rate().map(|r| format!("{r}%")).unwrap_or_else(|| "None yet".into())),
                ]))
                (view::section("Waiting", Some("Confirm once the pet has changed hands. Confirming takes one copy out of the collection. Both members get a DM when you record one, 15 minutes before it expires, and if it expires."), edit.then(|| html! {
                    form method="post" action=(page.path("pets/giveaways/test-dms")) {
                        button.btn.small.ghost type="submit" data-tip="The three DMs, sent to you" { "Send me the DMs" }
                    }
                }), html! {
                    @if active.is_empty() {
                        @if edit {
                            (ui::empty(Icon::Gift, "No giveaways waiting", "Record one with the button above and it waits here until it's confirmed or expires."))
                        } @else {
                            (ui::empty(Icon::Gift, "No giveaways waiting", "Giveaways staff record wait here until they're confirmed or expire."))
                        }
                    } @else {
                        div.pets-gvs { @for g in &active { (waiting_card(&ctx, &page, g, image(&g.pet_id).as_deref(), comments.get(&g.id).map(Vec::as_slice).unwrap_or(&[]))) } }
                    }
                }))
                (view::section("History", None, None, html! {
                    @if past.is_empty() {
                        (ui::empty(Icon::Log, "No history yet", "Confirmed, expired and cancelled giveaways are kept here."))
                    } @else {
                        div.pets-list { @for g in &past { (history_row(&ctx, &page, g, image(&g.pet_id).as_deref(), comments.get(&g.id).map(Vec::as_slice).unwrap_or(&[]))) } }
                    }
                }))
                (ui::settings_form(&page, &page.path("pets"), html! {
                    (view::section("Giveaway settings", None, None, html! {
                        div.pets-split {
                            (ui::card(html! {
                                (ui::field_channel(&ctx, guild, "announce_channel", "Confirmed giveaways", Some("Posted here when a giveaway is confirmed, mentioning the giver and the winner. None posts nothing."), s.announce_channel))
                                (ui::field_number("expiry_hours", "Giveaways expire after", Some("The default when you record one. 0 means no expiry."), s.expiry_hours, 0, 720, Some("hours")))
                                (ui::field_toggle("dm_reviews", "DM review results", Some("Members get a DM when their submission is approved or rejected."), s.dm_reviews))
                            }))
                            div.pets-preview {
                                p.pets-cap { @if channel.is_some() { "Posted when you confirm one" } @else { "Posted when you confirm one, once a channel is picked" } }
                                div.pets-dc {
                                    div.pets-dc-head { (ui::icon(Icon::Hash)) span { (channel.as_deref().unwrap_or("pick a channel")) } }
                                    div.pets-dc-msg {
                                        (bot.1)
                                        div {
                                            div.pets-dc-name { strong { (bot.0) } span.pets-dc-app { "APP" } span.pets-dc-time { "Today" } }
                                            div.pets-dc-text {
                                                @match example {
                                                    Some(g) => {
                                                        span.pets-dc-mention { "@" (ctx.display_name(guild, g.giver_id)) } " gave "
                                                        span.pets-dc-mention { "@" (ctx.display_name(guild, g.winner_id)) } " " (g.icon()) " " b { (g.pet_name) } "."
                                                    }
                                                    None => {
                                                        span.pets-dc-mention { "@giver" } " gave " span.pets-dc-mention { "@winner" } " 🐾 " b { "a pet" } "."
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                // The button belongs to the form after this one: a test isn't a save.
                                @if edit { button.btn.small.outline type="submit" form="pets-test-post" { "Send a test post" } }
                            }
                        }
                    }))
                }))
                @if edit { form #pets-test-post method="post" action=(page.path("pets/giveaways/test-post")) hidden {} }
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
                                span.pets-meter style="margin-top:6px" { span style={ "width:" (n * 100 / most) "%" } {} }
                            }
                            span.pets-owned { (text::num(*n)) }
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

/// Notes and comments under a giveaway, with who recorded it.
fn giveaway_extra(page: &Page, g: &Giveaway, comments: &[store::Comment]) -> Markup {
    let base = page.path(&format!("pets/giveaways/{}", g.id));
    html! {
        @if !g.notes.is_empty() { p.pets-note { (g.notes) } }
        details.pets-more {
            summary { @if comments.is_empty() { "Comments" } @else { "Comments (" (comments.len()) ")" } }
            div.pets-cmts {
                span.meta { "Recorded by " (g.recorded_by) " " (text::ago(g.recorded_at)) }
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
    }
}

fn waiting_card(ctx: &Ctx, page: &Page, g: &Giveaway, image: Option<&str>, comments: &[store::Comment]) -> Markup {
    let base = page.path(&format!("pets/giveaways/{}", g.id));
    let side = html! {
        (view::clock(g.expires_at))
        @if page.can_edit() {
            div.pets-gv-acts {
                (ui::action_button(&format!("{base}/confirm"), "Confirm", Tone::Muted, false))
                (ui::action_button(&format!("{base}/cancel"), "Cancel", Tone::Bad, true))
            }
        }
    };
    view::giveaway_card(ctx, page.guild_id, g, image, side, Some(giveaway_extra(page, g, comments)))
}

fn history_row(ctx: &Ctx, page: &Page, g: &Giveaway, image: Option<&str>, comments: &[store::Comment]) -> Markup {
    view::giveaway_row(ctx, page.guild_id, g, image, Some(giveaway_extra(page, g, comments)))
}

/// A sample announcement in the announce channel, marked as a test, pinging nobody.
async fn test_post(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = open(&ctx, &user, guild, Level::Edit)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let Some(channel) = s.announce_channel else {
        return Err(WebError::Bad("Pick a channel for confirmed giveaways and save first.".into()));
    };
    let (icon, name) = sample_pet(&ctx, guild)?;
    let content = format!("Test from the dashboard, nothing was given: {}", crate::announcement(page.user.id, page.user.id, &icon, &name));
    let nobody = AllowedMentions::default();
    if let Err(e) = ctx.http.create_message(Id::new(channel)).content(&content).allowed_mentions(Some(&nobody)).await {
        bot_core::tracing::warn!(guild, channel, error = %e, "test pet announcement failed");
        return Err(WebError::Bad(bot_core::discord::why_refused(&e)));
    }
    let where_ = ctx.text_channels(guild).into_iter().find(|x| x.0 == channel).map(|(_, n)| format!(" in #{n}")).unwrap_or_default();
    ctx.audit(&page.user, Some(guild), KEY, format!("Sent a test pet giveaway post{where_}"));
    Ok(Redirect::to(&page.path("pets/giveaways?sent=post")).into_response())
}

/// The DMs giver and winner get (recorded, 15 minutes left, expired), sent to the person
/// clicking, marked as a test.
async fn test_giveaway_dms(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = open(&ctx, &user, guild, Level::Edit)?;
    let (icon, name) = sample_pet(&ctx, guild)?;
    let server = ctx.guild_name(guild);
    let me = ctx.display_name(guild, page.user.id);
    let soon = now_ms() + 15 * 60_000;
    let (to_winner, _) = crate::recorded_dms(&icon, &name, &me, &me, &server, Some(now_ms() + 24 * 3_600_000));
    let lines = [
        ("When you record one, the winner gets", to_winner),
        ("15 minutes before it expires, both get", crate::warning_dm(&name, &me, &me, &server, soon)),
        ("If it expires unconfirmed, both get", crate::expired_dm(&name, &me, &me, &server)),
    ];
    for (when, line) in lines {
        dm_or_why(&ctx, page.user.id, &format!("Test from the dashboard. {when}:\n{line}")).await?;
    }
    ctx.audit(&page.user, Some(guild), KEY, "Sent themselves the pet giveaway DMs as a test");
    Ok(Redirect::to(&page.path("pets/giveaways?sent=dms")).into_response())
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
                div.pform-acts { button.btn.primary type="submit" { "Ban giver" } }
            }
        })
    });
    let body = html! {
        (tabs(&page, "Bans", waiting))
        div.with-rail {
            div.stack.pets-main {
                (view::section("Banned givers", Some("They can't be recorded as the giver of a pet giveaway. They can still win."), None, html! {
                    @if list.is_empty() {
                        @if edit {
                            (ui::empty(Icon::Shield, "Nobody is banned", "Ban a member with the button above and they can't be recorded as a giver."))
                        } @else {
                            (ui::empty(Icon::Shield, "Nobody is banned", "Members staff ban from giving pets show here."))
                        }
                    } @else {
                        div.pets-list {
                            @for b in &list {
                                @let n = given.get(&b.user_id).copied().unwrap_or(0);
                                div.pets-li.wide-text style="--lead: 36px" {
                                    img.avatar src=(ctx.member_avatar(guild, b.user_id)) alt="" loading="lazy";
                                    div.pets-li-main {
                                        span.pets-li-title { strong { (ctx.display_name(guild, b.user_id)) } }
                                        @if b.reason.is_empty() { span.pets-li-sub.faint { "No reason given" } } @else { span.pets-li-sub title=(b.reason) { (b.reason) } }
                                    }
                                    div.pets-li-side {
                                        span { "Banned " (text::ago(b.banned_at)) " by " (b.banned_by) }
                                        @if n > 0 { span { (text::num(n)) " " (if n == 1 { "pet given before" } else { "pets given before" }) } }
                                    }
                                    @if edit {
                                        div.pets-li-acts { (ui::action_button(&page.path(&format!("pets/bans/{}/delete", b.user_id)), "Unban", Tone::Muted, false)) }
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
                        @let banned = list.iter().any(|b| b.user_id == *u);
                        div.mini {
                            img.avatar src=(ctx.member_avatar(guild, *u)) alt="" loading="lazy";
                            div.grow {
                                span { (ctx.display_name(guild, *u)) }
                                span.muted { (text::num(*n)) " confirmed" @if banned { ", " span.pets-bad { "banned" } } }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pet(name: &str, rarity: &str) -> Pet {
        Pet { name: name.into(), rarity: rarity.into(), category: "Legacy".into(), ..missing_pet("x") }
    }

    #[test]
    fn edits_read_in_the_log() {
        let old = pet("Deckhand Pirate", "common");
        let new = pet("Pirate Deckhand", "rare");
        assert_eq!(changes(&old, &new, Some(true)), ["renamed it to Pirate Deckhand", "made it Rare", "gave it a new picture"]);
        assert!(changes(&old, &old, None).is_empty());
        assert!(haystack(&new).contains("pirate deckhand rare"));
    }
}
