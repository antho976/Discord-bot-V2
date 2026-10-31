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
    let cloud = ctx.cfg.relay_public_url.is_some();

    let body = html! {
        style { (PreEscaped(CSS)) }
        @if !cloud {
            p.muted.small { "Tally, Avex and PCs can't connect until RELAY_PUBLIC_URL is set. The Relay app still pairs through a connected PC." }
        }
        div.with-rail {
            div.stack.cn-main {
                div.block {
                    div.block-head.cn-head {
                        h2 { "Waiting for an answer" }
                        @if !pending.is_empty() { span.count-pill { (pending.len()) } }
                    }
                    p.muted.small { "Requests from Relay, Tally, Avex and your PCs land here and in the approvers' DMs. Allow one only if the app shows the same code." }
                    @if pending.is_empty() {
                        (ui::empty(Icon::Plug, "Nothing waiting", "When an app asks to connect, it shows up here for 15 minutes."))
                    } @else {
                        div.cn-reqs {
                            @for (i, r) in pending.iter().enumerate() {
                                (request_card(&ctx, &page, guild, r, now, i == 0))
                            }
                        }
                    }
                }

                div.block {
                    div.block-head {
                        h2 { "Connected PCs" }
                        p.muted.small { "PCs that ran relay cloud connect. Each one opens pairing windows for the Relay phones you allow. Forgetting a PC stops its token working here." }
                    }
                    @if hosts.is_empty() {
                        (ui::empty(Icon::Plug, "No PCs yet", "A PC shows up here after it runs relay cloud connect and an approver allows it."))
                        @if page.can_edit() { (import_form(&page)) }
                    } @else {
                        div.cn-pcs {
                            @for h in &hosts {
                                @let live = h.last_seen_at.is_some_and(|t| now - t < 10 * 60_000);
                                div.cn-pc {
                                    span.cn-dot.live[live] aria-hidden="true" {}
                                    div.grow {
                                        strong { (h.name) }
                                        div.muted.small { (match h.last_seen_at {
                                            Some(t) => format!("Checked in {}", text::ago(t)),
                                            None => "Not checked in yet".into(),
                                        }) " · connected " (since_month(&page, h.created_at)) }
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
                    h2 { "Approvers" }
                    p.muted.small { "Each gets the DM. The first answer counts." }
                    div.cn-aps {
                        @for id in &approvers {
                            div.cn-ap {
                                img.avatar src=(ctx.member_avatar(guild, *id)) alt="" width="28" height="28" loading="lazy";
                                span.grow.clip { (name_in(&ctx, guild, *id)) }
                                @if *id == ctx.cfg.owner_id {
                                    span.muted.small { "bot owner" }
                                } @else if page.can_edit() && approvers.len() > 1 {
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

/// One request waiting for an answer. The first one in the list is outlined: it's the next to answer.
fn request_card(ctx: &Ctx, page: &Page, guild: u64, r: &Request, now: i64, first: bool) -> Markup {
    let dmed: Vec<String> = if r.notified {
        store::dms(ctx, &r.id).unwrap_or_default().into_iter().map(|(user, _, _)| name_in(ctx, guild, user)).collect()
    } else {
        Vec::new()
    };
    html! {
        div.cn-req.first[first] {
            span.cn-tile { (device_icon(&r.kind)) }
            div.cn-title {
                strong { (r.device) " " (asks(&r.kind)) }
                div.cn-meta {
                    "Asked " (text::ago(r.created_at)) " · " (minutes_left(r.expires_at, now))
                    " · "
                    @if !r.notified { "nobody could be DMed" } @else if dmed.is_empty() { "approvers were DMed" } @else { "DMed to " (names_line(&dmed)) }
                }
            }
            div.cn-code {
                span.lbl { (code_place(&r.kind)) }
                b.mono { (r.code) }
            }
            @if page.can_edit() {
                div.cn-acts {
                    (ui::action_button(&page.path(&format!("connect/requests/{}/allow", r.id)), "Allow", Tone::Accent, false))
                    (ui::action_button(&page.path(&format!("connect/requests/{}/deny", r.id)), "Deny", Tone::Muted, false))
                }
            }
        }
    }
}

/// One answered or run-out request, in the recent decisions rail.
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
    html! {
        div.cn-dec {
            @match r.decided_by {
                Some(by) => img.avatar src=(ctx.member_avatar(guild, by)) alt="" width="28" height="28" loading="lazy";,
                None => span.cn-dec-none aria-hidden="true" {},
            }
            div.grow {
                p {
                    @if let Some(by) = r.decided_by { strong { (name_in(ctx, guild, by)) } " " }
                    @match verb {
                        Some(v) => { (v) " " (r.device) },
                        None => { (r.device) " expired unanswered" },
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
        form.cn-add method="post" action=(page.path("connect/approvers")) {
            (ui::select("member", &members, ""))
            input.input.mono type="text" name="user_id" inputmode="numeric" placeholder="or paste a user ID" autocomplete="off";
            button.btn.small type="submit" { "Add an approver" }
        }
    }
}

fn import_form(page: &Page) -> Markup {
    html! {
        div.inline-form style="margin-top:12px" {
            span.muted.small { "Moving from antho-s-bot? Upload its database to keep the PCs already connected there." }
            input.input id="connect-import-file" type="file" accept=".db,.sqlite" style="height:auto;padding:6px";
            button.btn.small id="connect-import-go" type="button" { "Bring PCs over" }
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
    let mut list = crate::approvers(&ctx);
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

const CSS: &str = "
.cn-main { gap: 34px; }
.cn-head { display: flex; flex-direction: row; align-items: center; gap: 8px; }
.cn-reqs { display: flex; flex-direction: column; gap: 10px; margin-top: 4px; }
.cn-req { display: grid; grid-template-columns: 40px minmax(0, 1fr) auto auto; align-items: center; gap: 16px; padding: 16px 18px; border-radius: var(--r-card); background: var(--veil); border: 1px solid var(--edge-1); }
.cn-req.first { border-color: rgba(240, 128, 106, 0.42); }
.cn-tile { display: grid; place-items: center; width: 40px; height: 40px; border-radius: 12px; background: var(--control); border: 1px solid var(--edge-1); color: var(--ink-2); }
.cn-title { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
.cn-title strong { font-weight: 500; font-size: 14px; overflow-wrap: anywhere; }
.cn-meta { font-size: 12px; color: var(--ink-3); }
.cn-code { display: flex; flex-direction: column; align-items: flex-end; gap: 4px; }
.cn-code .lbl { font-size: 12px; color: var(--ink-3); white-space: nowrap; }
.cn-code b { font-size: 22px; font-weight: 500; letter-spacing: 0.14em; line-height: 1; }
.cn-acts { display: flex; gap: 8px; }
.cn-pcs { display: flex; flex-direction: column; }
.cn-pc { display: grid; grid-template-columns: 8px minmax(0, 1fr) auto; align-items: center; gap: 14px; padding: 14px 0; border-top: 1px solid var(--edge-1); }
.cn-pc:last-child { border-bottom: 1px solid var(--edge-1); }
.cn-pc strong { font-weight: 500; font-size: 14px; }
.cn-pc .grow { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.cn-dot { width: 8px; height: 8px; border-radius: 50%; background: var(--ink-3); }
.cn-dot.live { background: var(--live); }
.cn-aps { display: flex; flex-direction: column; gap: 12px; }
.cn-ap { display: flex; align-items: center; gap: 10px; font-size: 13px; min-width: 0; }
.cn-ap .grow { min-width: 0; }
.cn-add { display: flex; flex-direction: column; gap: 8px; margin-top: 4px; }
.cn-add .input { width: 100%; }
.cn-add .select-wrap { width: 100%; }
.cn-decs { display: flex; flex-direction: column; gap: 14px; }
.cn-dec { display: flex; align-items: flex-start; gap: 10px; font-size: 13px; min-width: 0; }
.cn-dec .avatar { margin-top: 1px; }
.cn-dec-none { width: 28px; height: 28px; border-radius: 50%; background: var(--control); flex: none; }
.cn-dec p { color: var(--ink-2); overflow-wrap: anywhere; }
.cn-dec p strong { color: var(--ink); font-weight: 500; }
.cn-dec .small { color: var(--ink-3); }
.cn-dec .badge { align-self: center; }
@media (max-width: 560px) {
  .cn-req { grid-template-columns: 40px minmax(0, 1fr); }
  .cn-code { grid-column: 2; flex-direction: row; align-items: baseline; justify-content: space-between; }
  .cn-acts { grid-column: 2; }
}
";
