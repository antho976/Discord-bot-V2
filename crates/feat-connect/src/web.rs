//! `/g/{guild}/connect`: requests waiting for an answer, the connected PCs, the approvers and the
//! recent decisions. Everything here is global; the server in the path only decides who may look.

use bot_core::axum::Router;
use bot_core::axum::body::Bytes;
use bot_core::axum::extract::{DefaultBodyLimit, Path, State};
use bot_core::axum::response::{IntoResponse, Json};
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::twilight_model::id::Id;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, serde_json, text};

use crate::store::Request;
use crate::{KEY, Via, domain, store};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/connect", get(page))
        .route("/g/{guild}/connect/requests/{id}/{answer}", post(answer))
        .route("/g/{guild}/connect/approvers", post(add_approver))
        .route("/g/{guild}/connect/approvers/{user}/remove", post(remove_approver))
        .route("/g/{guild}/connect/hosts/{id}/forget", post(forget_host))
        .route("/g/{guild}/connect/import", post(import).layer(DefaultBodyLimit::max(64 * 1024 * 1024)))
}

/// A name in this server when the member is cached, otherwise their name anywhere.
fn name_in(ctx: &Ctx, guild: u64, user: u64) -> String {
    if ctx.cache.member(Id::new(guild), Id::new(user)).is_some() { ctx.display_name(guild, user) } else { crate::name_of(ctx, user) }
}

fn minutes_left(expires_at: i64, now: i64) -> String {
    let m = ((expires_at - now) / 60_000).max(0);
    if m < 1 { "expires in under a minute".into() } else { format!("expires in {m} min") }
}

/// What the device asks for, in the words of the request card's title.
fn asks(kind: &str) -> &'static str {
    match kind {
        "pc" => "wants to connect Relay on a PC",
        "relay" => "wants to connect the Relay app",
        "avex" => "wants to sync Avex",
        _ => "wants to connect Tally",
    }
}

/// Where the code is shown: on the phone, the PC, or some other device.
fn code_place(kind: &str) -> &'static str {
    match kind {
        "relay" => "Code on the phone",
        "pc" => "Code on the PC",
        _ => "Code on the device",
    }
}

/// A small line icon for the kind of device asking.
fn device_icon(kind: &str) -> Markup {
    let body = match kind {
        "pc" => r#"<rect x="3" y="4.5" width="18" height="11.5" rx="1.5"/><path d="M9 20h6M12 16v4"/>"#,
        "relay" => r#"<rect x="7" y="2.5" width="10" height="19" rx="2.25"/><path d="M11 18.5h2"/>"#,
        _ => r#"<rect x="3.5" y="5.5" width="17" height="13" rx="2"/><path d="M8.5 18.5h7"/>"#,
    };
    html! {
        svg.icon viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" {
            (PreEscaped(body))
        }
    }
}

/// `"Antho"`, `"Antho and Mira"`, `"Antho, Mira and 2 more"`.
fn names_line(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [a, b, rest @ ..] => format!("{a}, {b} and {} more", rest.len()),
    }
}

/// "since September", or "since March 2025" when it's last year or older.
fn since_month(page: &Page, ms: i64) -> String {
    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let z = page.user.local(ms);
    let name = MONTHS[(z.month() as usize).clamp(1, 12) - 1];
    if z.year() == page.user.local(now_ms()).year() { format!("since {name}") } else { format!("since {name} {}", z.year()) }
}

async fn page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let now = now_ms();
    let pending = store::pending(&ctx, now)?;
    let history = store::history(&ctx, now, 8)?;
    let hosts = store::hosts(&ctx)?;
    let approvers = crate::approvers(&ctx);
    let cloud = crate::api::cloud(&ctx);

    let body = html! {
        style { (PreEscaped(CSS)) }
        @if !cloud {
            div.cn-note role="note" {
                span.cn-note-ic { (cloud_off_icon()) }
                p { "Tally, Avex and PCs can't connect until RELAY_PUBLIC_URL is set. The Relay app still pairs through a connected PC." }
            }
        }
        div.with-rail {
            div.cn-main {
                section.cn-block {
                    div.cn-head {
                        h2 { "Waiting for an answer" @if !pending.is_empty() { span.count-pill { (pending.len()) } } }
                        p.muted { "Requests from Relay, Tally, Avex and your PCs land here and in the approvers' DMs. Allow one only if the app shows the same code." }
                    }
                    @if pending.is_empty() {
                        (ui::empty(Icon::Plug, "Nothing waiting", "When an app asks to connect, it shows up here for 15 minutes."))
                    } @else {
                        div.cn-reqs {
                            @for (i, r) in pending.iter().enumerate() {
                                (request_card(&ctx, &page, guild, r, now, i, stuck(r, cloud, !hosts.is_empty())))
                            }
                        }
                    }
                }

                section.cn-block {
                    div.cn-head {
                        h2 { "Connected PCs" @if !hosts.is_empty() { span.tab-count { (hosts.len()) } } }
                        p.muted { "PCs that ran relay cloud connect. Each one opens pairing windows for the Relay phones you allow. Forgetting a PC stops its token working here." }
                    }
                    @if hosts.is_empty() {
                        (ui::empty(Icon::Plug, "No PCs yet", "A PC shows up here after it runs relay cloud connect and an approver allows it."))
                        @if page.can_edit() { (import_form(&page)) }
                    } @else {
                        div.cn-pcs {
                            @for h in &hosts {
                                @let live = h.last_seen_at.is_some_and(|t| now - t < 10 * 60_000);
                                div.cn-pc.live[live] {
                                    span.cn-pc-tile {
                                        (device_icon("pc"))
                                        span.dot.live[live] role="img" aria-label=(if live { "Online" } else { "Not seen lately" }) {}
                                    }
                                    div.grow {
                                        strong.clip { (h.name) }
                                        span.cn-seen { (match h.last_seen_at {
                                            Some(t) => format!("Checked in {}", text::ago(t)),
                                            None => "Not checked in yet".into(),
                                        }) }
                                        span.muted.small { "Connected " (since_month(&page, h.created_at)) }
                                    }
                                    @if page.can_edit() {
                                        (ui::action_button(&page.path(&format!("connect/hosts/{}/forget", h.id)), "Forget", Tone::Bad, true))
                                    }
                                }
                            }
                        }
                    }
                }
            }

            aside.rail {
                div.rail-block {
                    h2 { "Approvers" span.tab-count { (approvers.len()) } }
                    p.muted.small { "Each gets the DM. The first answer counts." }
                    div.cn-aps {
                        @for id in &approvers {
                            div.mini.cn-ap {
                                img.avatar src=(ctx.member_avatar(guild, *id)) alt="" width="30" height="30" loading="lazy";
                                div.grow {
                                    strong { (name_in(&ctx, guild, *id)) }
                                    span.muted { @if *id == ctx.cfg.owner_id { "Bot owner" } @else { "Approver" } }
                                }
                                @if *id != ctx.cfg.owner_id && page.can_edit() && approvers.len() > 1 {
                                    (ui::action_button(&page.path(&format!("connect/approvers/{id}/remove")), "Remove", Tone::Bad, true))
                                }
                            }
                        }
                    }
                    @if page.can_edit() {
                        (approver_form(&ctx, &page, guild, &approvers))
                    }
                }
                div.rail-block {
                    h2 { "Recent decisions" }
                    @if history.is_empty() {
                        p.muted.small { "Every answer, from Discord or from here, is listed with who gave it." }
                    }
                    div.cn-decs {
                        @for r in &history { (decision(&ctx, guild, r, now)) }
                    }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "App approvals", Some(cloud_pill(cloud)), body).into_response())
}

fn cloud_pill(on: bool) -> Markup {
    html! {
        span.status {
            @if on { span.dot.live {} "Cloud copy on" } @else { span.dot.waiting {} "Cloud copy off" }
        }
    }
}

/// A cloud with a line through it, drawn like the dashboard's own icons.
fn cloud_off_icon() -> Markup {
    html! {
        svg.icon viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" {
            (PreEscaped(r#"<path d="M7.5 18.5h9.75a3.75 3.75 0 0 0 .9-7.4A5.75 5.75 0 0 0 7.6 8.9 4.75 4.75 0 0 0 7.5 18.5ZM4 4l16 16"/>"#))
        }
    }
}

/// Why an allowed request still couldn't connect, as things stand.
fn stuck(r: &Request, cloud: bool, has_pc: bool) -> Option<&'static str> {
    if domain::pairs_with_cloud(&r.kind) && !cloud {
        Some("It can't connect while the cloud copy is off, even if you allow it.")
    } else if r.kind == "relay" && !has_pc {
        Some("No PC is connected to open its pairing window, so allowing it won't be enough.")
    } else {
        None
    }
}

/// One request waiting for an answer: the device, the code it shows, who was DMed and how long
/// is left. The first one in the list is lit: it's the next to answer.
fn request_card(ctx: &Ctx, page: &Page, guild: u64, r: &Request, now: i64, i: usize, stuck: Option<&str>) -> Markup {
    let dmed: Vec<u64> = if r.notified { store::dms(ctx, &r.id).unwrap_or_default().into_iter().map(|(user, _, _)| user).collect() } else { Vec::new() };
    let names: Vec<String> = dmed.iter().map(|u| name_in(ctx, guild, *u)).collect();
    let faces: Vec<String> = dmed.iter().map(|u| ctx.member_avatar(guild, *u)).collect();
    let left_ms = (r.expires_at - now).max(0);
    let left = (left_ms as f64 / domain::TTL_MS as f64).clamp(0.0, 1.0);
    html! {
        article.cn-req.first[i == 0] style={ "--i:" (i) } {
            div.cn-req-top {
                span.cn-tile { (device_icon(&r.kind)) }
                div.cn-who {
                    span.cn-kind { (domain::kind_name(&r.kind)) }
                    strong.cn-name { (r.device) }
                    span.cn-ask { (asks(&r.kind)) }
                }
                div.cn-codebox {
                    span.cn-lbl { (code_place(&r.kind)) }
                    b.cn-code aria-label={ "Code " (r.code) } {
                        @for (j, ch) in r.code.chars().enumerate() {
                            span.cn-key style={ "--k:" (j) } aria-hidden="true" { (ch) }
                        }
                    }
                }
            }
            @if let Some(why) = stuck {
                p.cn-stuck { (ui::icon(Icon::Bell)) span { (why) } }
            }
            div.cn-req-foot {
                div.cn-dm {
                    @if !r.notified {
                        span.cn-nodm { (ui::icon(Icon::Mail)) }
                        span { "Nobody could be DMed, so answer it here" }
                    } @else if names.is_empty() {
                        span.cn-nodm { (ui::icon(Icon::Mail)) }
                        span { "Approvers were DMed" }
                    } @else {
                        (ui::avatars(&faces, 4))
                        span { "DMed to " (names_line(&names)) }
                    }
                }
                div.cn-ttl {
                    span.cn-ttl-bar aria-hidden="true" { span style={ "--left:" (format!("{left:.3}")) ";--secs:" (left_ms / 1000) "s" } {} }
                    span.small { "Asked " (text::ago(r.created_at)) " · " (minutes_left(r.expires_at, now)) }
                }
                @if page.can_edit() {
                    div.cn-acts {
                        form.inline method="post" action=(page.path(&format!("connect/requests/{}/allow", r.id))) {
                            button.btn.primary type="submit" { (ui::icon(Icon::Check)) "Allow" }
                        }
                        form.inline method="post" action=(page.path(&format!("connect/requests/{}/deny", r.id))) {
                            button.btn.outline type="submit" { (ui::icon(Icon::Close)) "Deny" }
                        }
                    }
                }
            }
        }
    }
}

/// One answered or run-out request, in the recent decisions rail: who answered, with a mark for
/// how it went on their face.
fn decision(ctx: &Ctx, guild: u64, r: &Request, now: i64) -> Markup {
    let when = r.decided_at.unwrap_or(r.expires_at);
    // A request someone allowed that nobody collected shows as allowed, and says it expired.
    let (verb, uncollected) = match r.shown_state(now) {
        "allowed" => (Some("allowed"), false),
        "claimed" => (Some("connected"), false),
        "denied" => (Some("denied"), false),
        _ if r.decided_by.is_some() => (Some("allowed"), true),
        _ => (None, false),
    };
    let (mark, ic) = match verb {
        Some("denied") => ("no", Icon::Close),
        Some(_) => ("ok", Icon::Check),
        None => ("gone", Icon::Clock),
    };
    html! {
        div.cn-dec {
            span.cn-face {
                @match r.decided_by {
                    Some(by) => img.avatar src=(ctx.member_avatar(guild, by)) alt="" width="30" height="30" loading="lazy";,
                    None => span.cn-dec-none aria-hidden="true" { (device_icon(&r.kind)) },
                }
                span class={ "cn-mark " (mark) } aria-hidden="true" { (ui::icon(ic)) }
            }
            div.grow {
                p {
                    @if let Some(by) = r.decided_by { strong { (name_in(ctx, guild, by)) } " " }
                    @match verb {
                        Some(v) => { (v) " " strong.cn-dev { (r.device) } },
                        None => { strong.cn-dev { (r.device) } " expired unanswered" },
                    }
                }
                span.muted.small {
                    (domain::kind_name(&r.kind)) " · "
                    @if r.decided_by.is_some() {
                        @if r.decided_on.as_deref() == Some("dashboard") { "on the dashboard · " } @else { "on Discord · " }
                    }
                    @if uncollected { "never collected · " }
                    (text::ago(when))
                }
            }
        }
    }
}

fn approver_form(ctx: &Ctx, page: &Page, guild: u64, approvers: &[u64]) -> Markup {
    let mut members: Vec<(String, String)> = ctx
        .cache
        .guild_members(Id::new(guild))
        .map(|set| set.iter().map(|u| u.get()).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|u| !approvers.contains(u) && !ctx.cache.user(Id::new(*u)).is_some_and(|x| x.bot))
        .map(|u| (u.to_string(), ctx.display_name(guild, u)))
        .collect();
    members.sort_by_key(|(_, n)| n.to_lowercase());
    members.insert(0, (String::new(), "Pick a member".into()));
    html! {
        details.pop.cn-addpop {
            summary.btn.small.outline { (ui::icon(Icon::Plus)) "Add an approver" }
            div.pop-body {
                form.cn-add method="post" action=(page.path("connect/approvers")) {
                    label.cn-add-lbl for="member" { "A member of this server" }
                    (ui::select("member", &members, ""))
                    label.cn-add-lbl for="cn-user-id" { "Or anyone, by user ID" }
                    input.input.mono #cn-user-id type="text" name="user_id" inputmode="numeric" placeholder="123456789012345678" autocomplete="off";
                    p.help { "They get a DM with Allow and Deny for every request from now on." }
                    button.btn.primary type="submit" { "Add an approver" }
                }
            }
        }
    }
}

fn import_form(page: &Page) -> Markup {
    html! {
        div.cn-import {
            p.muted.small { "Moving from antho-s-bot? Upload its database to keep the PCs already connected there." }
            div.cn-import-row {
                input.input id="connect-import-file" type="file" accept=".db,.sqlite";
                button.btn.small id="connect-import-go" type="button" { "Bring PCs over" }
            }
            span.muted.small id="connect-import-result" {}
        }
        script { (PreEscaped(format!(r#"
document.getElementById("connect-import-go").addEventListener("click", async () => {{
  const file = document.getElementById("connect-import-file").files[0];
  const out = document.getElementById("connect-import-result");
  if (!file) {{ out.textContent = "Pick the database file first."; return; }}
  out.textContent = "Importing…";
  try {{
    const res = await fetch("{path}", {{ method: "POST", body: file }});
    const data = await res.json();
    out.textContent = data.message;
    if (data.ok) setTimeout(() => location.reload(), 900);
  }} catch (e) {{ out.textContent = "The upload failed. Try again."; }}
}});
"#, path = page.path("connect/import")))) }
    }
}

async fn answer(State(ctx): State<Ctx>, user: User, Path((guild, id, answer)): Path<(u64, String, String)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let allow = match answer.as_str() {
        "allow" => true,
        "deny" => false,
        _ => return Err(WebError::NotFound),
    };
    if !domain::is_hex(&id, 32) {
        return Err(WebError::NotFound);
    }
    let Some(req) = crate::decide(&ctx, &id, allow, page.user.id, Via::Dashboard(page.user.name.clone())).await? else {
        return Err(WebError::Bad("This request was already answered or has expired.".into()));
    };
    ctx.audit(&page.user, Some(guild), KEY, format!("{} {}", if allow { "Allowed" } else { "Denied" }, req.label()));
    Ok(saved(&page.path(KEY)))
}

async fn add_approver(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let pasted = form.get("user_id").map(|s| s.trim_start_matches("<@").trim_end_matches('>').trim_start_matches('!'));
    let id = match (form.id("member"), pasted) {
        (_, Some(raw)) => raw.parse::<u64>().ok().filter(|n| *n > 1 << 22).ok_or_else(|| WebError::Bad("That isn't a Discord user ID. It's a long number, like 123456789012345678.".into()))?,
        (Some(id), None) => id,
        (None, None) => return Err(WebError::Bad("Pick a member or paste a user ID.".into())),
    };
    let mut list = crate::approvers(&ctx);
    if !list.contains(&id) {
        list.push(id);
        crate::save_approvers(&ctx, list)?;
        ctx.audit(&page.user, Some(guild), KEY, format!("Added {} as an app approver", name_in(&ctx, guild, id)));
    }
    Ok(saved(&page.path(KEY)))
}

async fn remove_approver(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, u64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    if id == ctx.cfg.owner_id {
        return Err(WebError::Bad("The bot's owner can't be removed.".into()));
    }
    let mut list = crate::approvers(&ctx);
    if !list.contains(&id) {
        return Err(WebError::NotFound);
    }
    list.retain(|u| *u != id);
    if list.is_empty() {
        return Err(WebError::Bad("Keep at least one approver, or nobody can let an app in.".into()));
    }
    crate::save_approvers(&ctx, list)?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Removed {} as an app approver", name_in(&ctx, guild, id)));
    Ok(saved(&page.path(KEY)))
}

async fn forget_host(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, String)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let Some(name) = store::revoke_host(&ctx, &id, now_ms())? else { return Err(WebError::NotFound) };
    ctx.audit(&page.user, Some(guild), KEY, format!("Forgot the PC {name}"));
    Ok(saved(&page.path(KEY)))
}

async fn import(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, body: Bytes) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let c = ctx.clone();
    let res = bot_core::tokio::task::spawn_blocking(move || crate::import::import_bytes(&c, &body)).await?;
    let (ok, message) = match res {
        Ok(0) => (true, "No new PCs in that database.".to_owned()),
        Ok(n) => {
            let m = format!("Brought {n} PC{} over from antho-s-bot.", if n == 1 { "" } else { "s" });
            ctx.audit(&page.user, Some(guild), KEY, m.clone());
            (true, m)
        }
        Err(e) => (false, format!("Couldn't import: {e}")),
    };
    Ok(Json(serde_json::json!({ "ok": ok, "message": message })).into_response())
}

const CSS: &str = r#"
.cn-note { display: flex; align-items: center; gap: 12px; padding: 11px 16px 11px 12px; border-radius: 14px; background: linear-gradient(90deg, color-mix(in oklab, var(--waiting) 11%, transparent), color-mix(in oklab, var(--waiting) 2%, transparent) 75%); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--waiting) 22%, transparent); font-size: 13px; color: var(--ink-2); }
.cn-note-ic { display: grid; place-items: center; width: 34px; height: 34px; border-radius: 11px; flex: none; color: var(--waiting); background: color-mix(in oklab, var(--waiting) 16%, transparent); }
.cn-main { display: flex; flex-direction: column; gap: 38px; min-width: 0; }
.cn-block { display: flex; flex-direction: column; gap: 14px; min-width: 0; }
.cn-head { display: flex; flex-direction: column; gap: 4px; }
.cn-head h2 { display: flex; align-items: center; gap: 9px; font-size: 16px; }
.cn-head h2 .tab-count { margin-left: 0; }
.cn-head p { font-size: 13px; max-width: 72ch; }
.rail-block h2 .tab-count { margin-left: 0; margin-right: auto; }
.rail-block h2:has(.tab-count) { justify-content: flex-start; gap: 8px; }

/* A request: the device on the left, the code it shows on the right, as keys that drop in one
   after another. The first is lit in the page's hue: it's the next to answer. */
.cn-reqs { display: flex; flex-direction: column; gap: 12px; }
.cn-req { position: relative; display: flex; flex-direction: column; border-radius: 18px; background: linear-gradient(180deg, rgba(243, 242, 248, 0.05), rgba(243, 242, 248, 0.018)); box-shadow: inset 0 0 0 1px var(--edge-1); overflow: hidden; isolation: isolate; }
.cn-req.first { background: radial-gradient(70% 160% at 100% 0%, color-mix(in oklab, var(--hue) 17%, transparent), transparent 62%), linear-gradient(180deg, rgba(243, 242, 248, 0.055), rgba(243, 242, 248, 0.02)); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--hue) 40%, transparent), 0 22px 50px -30px var(--hue); }
.cn-req-top { display: grid; grid-template-columns: auto minmax(0, 1fr) auto; align-items: center; gap: 18px; padding: 20px 22px 18px; }
.cn-tile { display: grid; place-items: center; width: 56px; height: 56px; border-radius: 17px; color: var(--hue); background: color-mix(in oklab, var(--hue) 14%, transparent); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--hue) 24%, transparent); }
.cn-req.first .cn-tile { color: #0b0b10; background: linear-gradient(150deg, color-mix(in oklab, var(--hue), white 25%), var(--hue) 55%, color-mix(in oklab, var(--hue), black 22%)); box-shadow: 0 12px 26px -12px var(--hue), inset 0 1px 0 rgba(255, 255, 255, 0.35); animation: pop-in 0.6s var(--spring) both 0.15s; }
.cn-tile .icon { width: 28px; height: 28px; }
.cn-who { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.cn-kind { align-self: flex-start; display: inline-flex; align-items: center; height: 20px; padding: 0 8px; margin-bottom: 4px; border-radius: 10px; font-size: 11px; font-weight: 600; color: var(--hue); background: color-mix(in oklab, var(--hue) 14%, transparent); }
.cn-name { font-size: 19px; font-weight: 650; letter-spacing: -0.02em; line-height: 1.2; overflow-wrap: anywhere; }
.cn-ask { color: var(--ink-2); font-size: 13px; }
.cn-codebox { display: flex; flex-direction: column; align-items: flex-end; gap: 8px; }
.cn-lbl { font-size: 12px; color: var(--ink-3); white-space: nowrap; }
.cn-code { display: flex; gap: 6px; perspective: 420px; }
.cn-key { display: grid; place-items: center; width: 44px; height: 54px; border-radius: 12px; font: 600 27px/1 var(--mono); color: var(--ink); background: linear-gradient(180deg, rgba(243, 242, 248, 0.1), rgba(243, 242, 248, 0.035)); box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.08), inset 0 -2px 0 rgba(0, 0, 0, 0.35), inset 0 0 0 1px var(--edge-2), 0 8px 18px -10px rgba(0, 0, 0, 0.8); transform-origin: 50% 0; animation: cn-key 0.75s var(--spring) both; animation-delay: calc(var(--i, 0) * 150ms + var(--k) * 80ms + 300ms); }
.cn-req.first .cn-key { box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.1), inset 0 -2px 0 rgba(0, 0, 0, 0.35), inset 0 0 0 1px color-mix(in oklab, var(--hue) 50%, transparent), 0 10px 24px -12px var(--hue); }
@keyframes cn-key { from { opacity: 0; transform: translateY(-12px) rotateX(80deg); filter: blur(3px); } }
.cn-stuck { display: flex; align-items: center; gap: 9px; margin: -4px 22px 16px; padding: 9px 12px; border-radius: 11px; font-size: 12.5px; color: var(--waiting); background: color-mix(in oklab, var(--waiting) 10%, transparent); }
.cn-stuck .icon { width: 15px; height: 15px; }
.cn-req-foot { display: grid; grid-template-columns: minmax(0, 1fr) minmax(150px, 215px) auto; align-items: center; gap: 12px 22px; padding: 12px 22px; border-top: 1px solid var(--edge-1); background: rgba(7, 7, 11, 0.24); }
.cn-dm { display: flex; align-items: center; gap: 10px; min-width: 0; font-size: 12.5px; color: var(--ink-2); }
.cn-dm .avatars .avatar { width: 24px; height: 24px; }
.cn-nodm { display: grid; place-items: center; width: 24px; height: 24px; flex: none; border-radius: 8px; color: var(--waiting); background: color-mix(in oklab, var(--waiting) 15%, transparent); }
.cn-nodm .icon { width: 14px; height: 14px; }
.cn-ttl { display: flex; flex-direction: column; gap: 5px; min-width: 0; color: var(--ink-3); }
.cn-ttl-bar { display: block; height: 4px; border-radius: 2px; background: var(--edge-1); overflow: hidden; }
/* Fills to the time left, then drains for real as the request runs out. */
.cn-ttl-bar > span { display: block; height: 100%; border-radius: 2px; background: linear-gradient(90deg, var(--hue), color-mix(in oklab, var(--hue), white 35%)); transform-origin: left; transform: scaleX(var(--left)); animation: cn-fill 0.9s var(--ease) both 0.4s, cn-drain var(--secs) linear 1.3s forwards; }
@keyframes cn-fill { from { transform: scaleX(0); } to { transform: scaleX(var(--left)); } }
@keyframes cn-drain { from { transform: scaleX(var(--left)); } to { transform: scaleX(0); } }
.cn-acts { display: flex; gap: 8px; }
.cn-acts .btn { height: 36px; padding: 0 16px; }

/* Connected PCs. */
.cn-pcs { display: grid; grid-template-columns: repeat(auto-fill, minmax(290px, 1fr)); gap: 12px; }
.cn-pc { display: flex; align-items: center; gap: 14px; min-width: 0; padding: 14px 16px; border-radius: 16px; background: linear-gradient(180deg, rgba(243, 242, 248, 0.045), rgba(243, 242, 248, 0.015)); box-shadow: inset 0 0 0 1px var(--edge-1); transition: transform 0.35s var(--ease), box-shadow 0.25s; }
.cn-pc:hover { transform: translateY(-2px); box-shadow: inset 0 0 0 1px var(--edge-2), var(--shadow-lift); }
.cn-pc-tile { position: relative; display: grid; place-items: center; width: 46px; height: 46px; flex: none; border-radius: 14px; color: var(--hue); background: color-mix(in oklab, var(--hue) 14%, transparent); }
.cn-pc-tile .icon { width: 24px; height: 24px; }
.cn-pc-tile .dot { position: absolute; right: -3px; bottom: -3px; width: 12px; height: 12px; box-shadow: 0 0 0 3px var(--page); }
.cn-pc-tile .dot.live { box-shadow: 0 0 0 3px var(--page), 0 0 10px var(--live); }
.cn-pc .grow { display: flex; flex-direction: column; gap: 1px; min-width: 0; }
.cn-pc strong { font-size: 14px; font-weight: 600; }
.cn-seen { font-size: 12.5px; color: var(--ink-2); }
.cn-pc.live .cn-seen { color: var(--live); }
.cn-import { display: flex; flex-direction: column; gap: 10px; padding: 16px; border-radius: 14px; border: 1px dashed var(--edge-2); }
.cn-import-row { display: flex; gap: 8px; flex-wrap: wrap; align-items: center; }
.cn-import input[type="file"] { width: auto; flex: 1; min-width: 180px; max-width: 360px; height: auto; padding: 6px; }

/* Approvers and decisions, in the rail. */
.cn-aps { display: flex; flex-direction: column; gap: 4px; }
.cn-ap { padding: 4px 0; }
.cn-ap .grow strong { font-size: 13.5px; font-weight: 600; }
.cn-ap .btn { opacity: 0; transition: opacity 0.15s; }
.cn-ap:hover .btn, .cn-ap .btn:focus-visible, .cn-ap .btn.armed { opacity: 1; }
@media (hover: none) { .cn-ap .btn { opacity: 1; } }
.cn-addpop > summary .icon { width: 13px; height: 13px; }
.cn-addpop .pop-body { left: 0; right: auto; width: min(320px, calc(100vw - 48px)); transform-origin: top left; }
.cn-add { display: flex; flex-direction: column; gap: 8px; }
.cn-add-lbl { font-size: 12px; font-weight: 500; color: var(--ink-2); }
.cn-add .select-wrap, .cn-add .input { width: 100%; }
.cn-add .help { margin: 0; }
.cn-add .btn { align-self: flex-end; margin-top: 4px; }
.cn-decs { display: flex; flex-direction: column; gap: 2px; }
.cn-dec { display: flex; align-items: center; gap: 12px; min-width: 0; padding: 7px 8px; margin: 0 -8px; border-radius: 12px; font-size: 13px; transition: background 0.18s; }
.cn-dec:hover { background: var(--well); }
.cn-face { position: relative; flex: none; width: 30px; height: 30px; }
.cn-face .avatar { display: block; width: 30px; height: 30px; }
.cn-dec-none { display: grid; place-items: center; width: 30px; height: 30px; border-radius: 50%; color: var(--ink-3); background: var(--control); }
.cn-dec-none .icon { width: 16px; height: 16px; }
.cn-mark { position: absolute; right: -4px; bottom: -4px; display: grid; place-items: center; width: 16px; height: 16px; border-radius: 50%; box-shadow: 0 0 0 2.5px var(--raised); }
.cn-mark .icon { width: 10px; height: 10px; stroke-width: 2.4; }
.cn-mark.ok { background: var(--live); color: #062014; }
.cn-mark.no { background: var(--held); color: #1a0507; }
.cn-mark.gone { background: var(--solid); color: var(--ink-2); box-shadow: 0 0 0 2.5px var(--raised), inset 0 0 0 1px var(--edge-2); }
.cn-dec .grow { display: flex; flex-direction: column; gap: 1px; min-width: 0; }
.cn-dec p { color: var(--ink-2); line-height: 1.4; overflow-wrap: anywhere; }
.cn-dec p strong { color: var(--ink); font-weight: 600; }
.cn-dec p strong.cn-dev { font-weight: 500; }
@media (max-width: 640px) {
  .cn-req-top { grid-template-columns: auto minmax(0, 1fr); gap: 14px; padding: 16px; }
  .cn-tile { width: 48px; height: 48px; border-radius: 15px; }
  .cn-tile .icon { width: 24px; height: 24px; }
  .cn-name { font-size: 17px; }
  .cn-codebox { grid-column: 1 / -1; flex-direction: row; align-items: center; justify-content: space-between; padding: 10px 12px 10px 14px; border-radius: 14px; background: rgba(7, 7, 11, 0.3); box-shadow: inset 0 0 0 1px var(--edge-1); }
  .cn-key { width: 40px; height: 48px; font-size: 24px; }
  .cn-stuck { margin: -2px 16px 14px; }
  .cn-req-foot { grid-template-columns: minmax(0, 1fr); padding: 12px 16px 14px; }
  .cn-acts .btn { flex: 1; }
  .cn-acts form.inline { display: flex; flex: 1; }
}
@media (prefers-reduced-motion: reduce) { .cn-ttl-bar > span { animation: none; } }
"#;
