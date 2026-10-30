//! `/g/{guild}/connect`: requests waiting for an answer, the approvers, recent decisions and the
//! connected PCs. Everything here is global; the server in the path only decides who may look.

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

fn avatar(ctx: &Ctx, user: u64) -> String {
    let hash = ctx.cache.user(Id::new(user)).and_then(|u| u.avatar.map(|a| a.to_string()));
    bot_core::discord::avatar_url(user, hash.as_deref())
}

/// A name in this server when the member is cached, otherwise their name anywhere.
fn name_in(ctx: &Ctx, guild: u64, user: u64) -> String {
    if ctx.cache.member(Id::new(guild), Id::new(user)).is_some() { ctx.display_name(guild, user) } else { crate::name_of(ctx, user) }
}

fn minutes_left(expires_at: i64, now: i64) -> String {
    let m = ((expires_at - now) / 60_000).max(0);
    if m < 1 { "expires in under a minute".into() } else { format!("expires in {m} min") }
}

async fn page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let now = now_ms();
    let pending = store::pending(&ctx, now)?;
    let history = store::history(&ctx, now, 20)?;
    let hosts = store::hosts(&ctx)?;
    let approvers = crate::approvers(&ctx);
    let cloud = ctx.cfg.relay_public_url.is_some();

    let body = html! {
        (ui::readings(&[
            ("waiting for an answer", pending.len().to_string()),
            ("approvers", approvers.len().to_string()),
            ("PCs connected", hosts.len().to_string()),
            ("cloud copy", if cloud { "on".into() } else { "off".into() }),
        ]))
        @if !cloud {
            p.muted.small { "Tally, Avex and PCs can't connect until RELAY_PUBLIC_URL is set. The Relay app still pairs through a connected PC." }
        }
        (ui::section("Waiting for an answer", Some("Requests from Relay, Tally, Avex and your PCs land here and in the approvers' DMs. Allow one only if the app shows the same code."), html! {
            @if pending.is_empty() {
                (ui::empty(Icon::Plug, "Nothing waiting", "When an app asks to connect, it shows up here for 15 minutes."))
            } @else {
                div.table-wrap { table.table {
                    thead { tr { th { "Device" } th { "Code" } @if page.can_edit() { th {} } } }
                    tbody { @for r in &pending {
                        tr {
                            td {
                                (r.device) @if !r.notified { " " (ui::badge("Not DMed", Tone::Warn)) }
                                br; span.muted.small { (domain::kind_name(&r.kind)) " · asked " (text::ago(r.created_at)) " · " (minutes_left(r.expires_at, now)) }
                            }
                            td { b.mono { (r.code) } }
                            @if page.can_edit() {
                                td.actions {
                                    (ui::action_button(&page.path(&format!("connect/requests/{}/allow", r.id)), "Allow", Tone::Accent, false))
                                    (ui::action_button(&page.path(&format!("connect/requests/{}/deny", r.id)), "Deny", Tone::Bad, false))
                                }
                            }
                        }
                    } }
                } }
            }
        }))
        (approvers_section(&ctx, &page, &approvers))
        (ui::section("Recent decisions", None, html! {
            @if history.is_empty() {
                (ui::empty(Icon::Log, "No decisions yet", "Every answer, from Discord or from here, is listed with who gave it."))
            } @else {
                div.table-wrap { table.table {
                    thead { tr { th { "Device" } th { "Outcome" } th { "Answered by" } } }
                    tbody { @for r in &history {
                        tr {
                            td { (r.device) br; span.muted.small { (domain::kind_name(&r.kind)) } }
                            td { (outcome(r.shown_state(now))) }
                            td {
                                @match r.decided_by {
                                    Some(by) => (name_in(&ctx, guild, by)),
                                    None => span.muted { "Nobody" },
                                }
                                br;
                                span.muted.small {
                                    @if r.decided_by.is_some() { @if r.decided_on.as_deref() == Some("dashboard") { "on the dashboard, " } @else { "on Discord, " } }
                                    (text::ago(r.decided_at.unwrap_or(r.expires_at)))
                                }
                            }
                        }
                    } }
                } }
            }
        }))
        (ui::section("Connected PCs", Some("PCs that ran relay cloud connect. Each one opens pairing windows for the Relay phones you allow. Forgetting a PC stops its token working here."), html! {
            @if hosts.is_empty() {
                (ui::empty(Icon::Plug, "No PCs yet", "A PC shows up here after it runs relay cloud connect and an approver allows it."))
                @if page.can_edit() { (import_form(&page)) }
            } @else {
                div.table-wrap { table.table {
                    thead { tr { th { "PC" } th { "Last checked in" } @if page.can_edit() { th {} } } }
                    tbody { @for h in &hosts {
                        tr {
                            td { (h.name) br; span.muted.small { "connected " (text::ago(h.created_at)) } }
                            td { @match h.last_seen_at { Some(t) => (text::ago(t)), None => span.muted { "Not yet" } } }
                            @if page.can_edit() {
                                td.actions { (ui::action_button(&page.path(&format!("connect/hosts/{}/forget", h.id)), "Forget", Tone::Bad, true)) }
                            }
                        }
                    } }
                } }
            }
        }))
    };
    Ok(ui::layout(&ctx, &page, "App approvals", None, body).into_response())
}

fn outcome(state: &str) -> Markup {
    match state {
        "allowed" => ui::badge("Allowed", Tone::Ok),
        "claimed" => ui::badge("Connected", Tone::Ok),
        "denied" => ui::badge("Denied", Tone::Bad),
        _ => ui::badge("Expired", Tone::Muted),
    }
}

fn approvers_section(ctx: &Ctx, page: &Page, approvers: &[u64]) -> Markup {
    let guild = page.guild_id;
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
        (ui::section("Approvers", Some("Each approver gets a DM with Allow and Deny. The first answer counts, and the other DMs show who answered."), html! {
            div.table-wrap { table.table {
                tbody { @for id in approvers {
                    tr {
                        td { (ui::person(&name_in(ctx, guild, *id), &avatar(ctx, *id), (*id == ctx.cfg.owner_id).then_some("bot owner"))) }
                        td.actions {
                            @if page.can_edit() && approvers.len() > 1 {
                                (ui::action_button(&page.path(&format!("connect/approvers/{id}/remove")), "Remove", Tone::Bad, true))
                            }
                        }
                    }
                } }
            } }
            @if page.can_edit() {
                form.inline-form method="post" action=(page.path("connect/approvers")) style="margin-top:12px" {
                    (ui::select("member", &members, ""))
                    input.input.mono type="text" name="user_id" inputmode="numeric" placeholder="or paste a user ID" autocomplete="off";
                    button.btn type="submit" { "Add approver" }
                }
            }
        }))
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
