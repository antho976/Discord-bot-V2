use bot_core::axum::body::Bytes;
use bot_core::axum::extract::{DefaultBodyLimit, Path, Query, State};
use bot_core::axum::response::{IntoResponse, Json};
use bot_core::axum::routing::{get, post};
use bot_core::axum::Router;
use bot_core::maud::{Markup, html};
use bot_core::rusqlite::params;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, serde_json, text};
use serde::Deserialize;

use crate::{KEY, Settings, curve, import};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/levels", get(settings_page).post(save_settings))
        .route("/g/{guild}/levels/rewards", post(add_reward))
        .route("/g/{guild}/levels/rewards/{level}/delete", post(delete_reward))
        .route("/g/{guild}/levels/members", get(members_page))
        .route("/g/{guild}/levels/members/{user}", post(set_member_xp))
        .route("/g/{guild}/levels/import", get(import_page).post(import_upload).layer(DefaultBodyLimit::max(64 * 1024 * 1024)))
        .route("/g/{guild}/levels/import-local", post(import_local))
}

fn tabs(page: &Page, active: &str) -> Markup {
    ui::tabs(
        &[
            ("Settings", page.path("levels")),
            ("Members", page.path("levels/members")),
            ("Import", page.path("levels/import")),
        ],
        active,
    )
}

async fn settings_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let rewards = crate::rewards(&ctx, guild)?;
    let members = crate::count(&ctx, guild)?;
    let custom = !s.custom_curve.is_empty();
    let first_levels: Vec<String> = (1..=5u32)
        .map(|l| text::num(curve::xp_to_reach(l, s.curve_base, s.curve_factor, &s.custom_curve)))
        .collect();

    let body = html! {
        (tabs(&page, "Settings"))
        (ui::readings(&[
            ("members with XP", text::num(members)),
            ("reward roles", rewards.len().to_string()),
            ("XP per message", format!("{}–{}", s.xp_min, s.xp_max)),
        ]))
        (ui::settings_form(&page, &page.path("levels"), html! {
            (ui::section("Earning XP", None, ui::card(html! {
                (ui::field_number("xp_min", "XP per message, lowest", None, s.xp_min, 0, 1000, Some("XP")))
                (ui::field_number("xp_max", "XP per message, highest", Some("Each message earns a random amount between the two."), s.xp_max, 0, 1000, Some("XP")))
                (ui::field_number("cooldown_secs", "Cooldown", Some("Messages inside this window after one that earned XP earn nothing."), s.cooldown_secs, 0, 3600, Some("seconds")))
                (ui::field_number("voice_xp", "Voice XP", Some("Per minute in a voice channel with someone else. 0 turns it off."), s.voice_xp, 0, 1000, Some("XP / min")))
                (ui::field_toggle("voice_require_active", "Only unmuted voice counts", Some("Muted or deafened members earn no voice XP."), s.voice_require_active))
                (ui::field_number("reaction_xp", "Reaction XP", Some("For adding a reaction, with the same cooldown. 0 turns it off."), s.reaction_xp, 0, 1000, Some("XP")))
            })))
            (ui::section("Level curve", Some(&format!("Levels 1 to 5 start at {} XP.", first_levels.join(", "))), ui::card(html! {
                @if custom {
                    (ui::field_custom("Custom table", Some("This server uses a custom XP table carried over from the old bot. Switch it off to use the curve below."), html! {
                        label.switch { input type="checkbox" name="keep_custom" checked; span.track { span.thumb {} } }
                    }))
                }
                (ui::field_number("curve_base", "First level", Some("XP needed to go from level 0 to 1."), s.curve_base, 1, 100_000, Some("XP")))
                (ui::field_text("curve_factor", "Growth per level", Some("Each level costs this many times the one before. 1.2 means 20% more."), &s.curve_factor.to_string(), "1.2"))
            })))
            (ui::section("Level-up messages", None, ui::card(html! {
                (ui::field_toggle("announce", "Post level-ups", None, s.announce))
                (ui::field_channel(&ctx, guild, "announce_channel", "Channel", Some("None posts in the channel where they levelled up."), s.announce_channel))
                (ui::field_toggle("announce_ping", "Mention the member", None, s.announce_ping))
                (ui::field_number("announce_min_level", "Start at level", Some("Quieter: skip messages for the first few levels."), s.announce_min_level as i64, 0, 500, None))
                (ui::field_text("message", "Message", Some("{user} is the member, {level} the new level."), &s.message, crate::DEFAULT_MESSAGE))
            })))
            (ui::section("No XP in", None, ui::card(html! {
                (ui::field_channels(&ctx, guild, "no_xp_channels", "Channels", None, &s.no_xp_channels))
                (ui::field_roles(&ctx, guild, "no_xp_roles", "Roles", Some("Members with any of these roles earn nothing."), &s.no_xp_roles))
                (ui::field_toggle("stack_rewards", "Keep earlier reward roles", Some("Off: members only keep the reward role for their highest level."), s.stack_rewards))
            })))
        }))
        (ui::section("Reward roles", Some("Given automatically when a member reaches the level."), html! {
            @if rewards.is_empty() {
                (ui::empty(Icon::Trophy, "No reward roles", "Add a role below and members get it at that level."))
            } @else {
                div.table-wrap { table.table {
                    thead { tr { th { "Level" } th { "Role" } th {} } }
                    tbody { @for (level, role) in &rewards {
                        tr {
                            td.num style="text-align:left" { (level) }
                            td { (role_name(&ctx, guild, *role)) }
                            td.actions { @if page.can_edit() { (ui::action_button(&page.path(&format!("levels/rewards/{level}/delete")), "Remove", Tone::Bad, true)) } }
                        }
                    } }
                } }
            }
            @if page.can_edit() {
                form.inline-form method="post" action=(page.path("levels/rewards")) {
                    input.input.num type="number" name="level" min="1" max="500" placeholder="Level" required style="max-width:110px";
                    (ui::select("role", &std::iter::once((String::new(), "Pick a role".to_owned())).chain(ctx.roles(guild).into_iter().map(|(id, n, _)| (id.to_string(), format!("@{n}")))).collect::<Vec<_>>(), ""))
                    button.btn type="submit" { "Add reward" }
                }
            }
        }))
    };
    Ok(ui::layout(&ctx, &page, "Levels", None, body).into_response())
}

fn role_name(ctx: &Ctx, guild: u64, role: u64) -> String {
    ctx.roles(guild).into_iter().find(|(id, _, _)| *id == role).map(|(_, n, _)| format!("@{n}")).unwrap_or_else(|| "Deleted role".into())
}

async fn save_settings(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let old = ctx.settings::<Settings>(guild, KEY)?;
    let xp_min = form.int_in("xp_min", 0, 1000, 15);
    let s = Settings {
        xp_min,
        xp_max: form.int_in("xp_max", 0, 1000, 25).max(xp_min),
        cooldown_secs: form.int_in("cooldown_secs", 0, 3600, 60),
        voice_xp: form.int_in("voice_xp", 0, 1000, 5),
        voice_require_active: form.bool("voice_require_active"),
        reaction_xp: form.int_in("reaction_xp", 0, 1000, 0),
        curve_base: form.int_in("curve_base", 1, 100_000, 100),
        curve_factor: form.f64("curve_factor").filter(|f| (1.0..=5.0).contains(f)).unwrap_or(1.2),
        custom_curve: if form.bool("keep_custom") { old.custom_curve.clone() } else { Default::default() },
        announce: form.bool("announce"),
        announce_channel: form.channel(&ctx, guild, "announce_channel"),
        announce_ping: form.bool("announce_ping"),
        announce_min_level: form.int_in("announce_min_level", 0, 500, 0) as u32,
        message: { let m = form.string("message"); if m.is_empty() { crate::DEFAULT_MESSAGE.into() } else { text::clip(&m, 500) } },
        stack_rewards: form.bool("stack_rewards"),
        no_xp_channels: form.channels(&ctx, guild, "no_xp_channels"),
        no_xp_roles: form.roles(&ctx, guild, "no_xp_roles"),
    };
    let curve_changed = s.curve_base != old.curve_base || s.curve_factor != old.curve_factor || s.custom_curve != old.custom_curve;
    ctx.save_settings(guild, KEY, &s)?;
    if curve_changed {
        recompute_levels(&ctx, guild, &s)?;
    }
    ctx.audit(&page.user, Some(guild), KEY, "Changed XP settings");
    Ok(saved(&page.path("levels")))
}

/// After a curve change, every stored level is recomputed from XP. Roles catch up on the
/// member's next XP gain.
fn recompute_levels(ctx: &Ctx, guild: u64, s: &Settings) -> bot_core::anyhow::Result<()> {
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    let rows: Vec<(i64, i64)> = {
        let mut st = tx.prepare("SELECT user_id, xp FROM levels WHERE guild_id = ?1")?;
        st.query_map([guild as i64], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
    };
    for (user, xp) in rows {
        tx.execute("UPDATE levels SET level = ?3 WHERE guild_id = ?1 AND user_id = ?2", params![guild as i64, user, s.progress(xp).level as i64])?;
    }
    tx.commit()?;
    Ok(())
}

async fn add_reward(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let level = form.i64("level").filter(|l| (1..=500).contains(l)).ok_or_else(|| WebError::Bad("Pick a level from 1 to 500.".into()))?;
    let role = form.role(&ctx, guild, "role").ok_or_else(|| WebError::Bad("Pick a role.".into()))?;
    if !page.user.admin && !ctx.safe_role(guild, role) {
        return Err(WebError::Bad("That role has moderation or admin permissions, or sits above the bot's role. Only a dashboard admin can make it a reward.".into()));
    }
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO level_rewards (guild_id, level, role_id) VALUES (?1, ?2, ?3)
             ON CONFLICT (guild_id, level) DO UPDATE SET role_id = excluded.role_id",
            params![guild as i64, level, role as i64],
        )
    })?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Set the level {level} reward to {}", role_name(&ctx, guild, role)));
    Ok(saved(&page.path("levels")))
}

async fn delete_reward(State(ctx): State<Ctx>, user: User, Path((guild, level)): Path<(u64, i64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    ctx.db.with(|c| c.execute("DELETE FROM level_rewards WHERE guild_id = ?1 AND level = ?2", params![guild as i64, level]))?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Removed the level {level} reward"));
    Ok(saved(&page.path("levels")))
}

#[derive(Deserialize)]
struct MembersQuery {
    q: Option<String>,
    page: Option<i64>,
}

async fn members_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<MembersQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let per = 50;
    let n = q.page.unwrap_or(0).max(0);
    let search = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let ids: Option<Vec<u64>> = search.map(|needle| {
        let needle = needle.to_lowercase();
        let by_id: Option<u64> = needle.parse().ok();
        ctx.cache
            .guild_members(bot_core::twilight_model::id::Id::new(guild))
            .map(|set| set.iter().map(|u| u.get()).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .filter(|u| Some(*u) == by_id || ctx.display_name(guild, *u).to_lowercase().contains(&needle))
            .chain(by_id)
            .collect()
    });
    let rows = crate::top(&ctx, guild, n * per, per, ids.as_deref())?;
    let total = crate::count(&ctx, guild)?;
    let body = html! {
        (tabs(&page, "Members"))
        form.inline-form method="get" action=(page.path("levels/members")) {
            input.input type="search" name="q" value=(search.unwrap_or_default()) placeholder="Search by name or ID";
            button.btn type="submit" { "Search" }
        }
        @if rows.is_empty() {
            @if search.is_some() {
                (ui::empty(Icon::Users, "No match", "Nobody with XP matches that name or ID."))
            } @else {
                (ui::empty(Icon::Trophy, "Nobody has XP yet", "Members show up here once they chat, or after you import XP from the old bot."))
            }
        } @else {
            div.table-wrap { table.table {
                thead { tr { th.num style="text-align:left" { "#" } th { "Member" } th.num { "Level" } th.num { "XP" } th.num { "Messages" } th.num { "Voice" } @if page.can_edit() { th { "Set XP" } } } }
                tbody { @for (i, m) in rows.iter().enumerate() {
                    @let p = s.progress(m.xp);
                    tr {
                        td.num style="text-align:left" { @if search.is_none() { (n * per + i as i64 + 1) } }
                        td { (ui::person(&ctx.display_name(guild, m.user_id), &member_avatar(&ctx, guild, m.user_id), None)) }
                        td.num { (p.level) }
                        td.num { (text::num(m.xp)) }
                        td.num { (text::num(m.messages)) }
                        td.num { (text::duration(m.voice_minutes * 60)) }
                        @if page.can_edit() {
                            td { form.inline-form method="post" action=(page.path(&format!("levels/members/{}", m.user_id))) {
                                input.input.num type="number" name="xp" value=(m.xp) min="0" style="max-width:110px;min-width:0";
                                button.btn.small type="submit" { "Set" }
                            } }
                        }
                    }
                } }
            } }
            @if search.is_none() && total > per {
                div.inline-form {
                    @if n > 0 { a.btn.small href={ (page.path("levels/members")) "?page=" (n - 1) } { "Previous" } }
                    span.muted.small { "Page " (n + 1) " of " ((total + per - 1) / per) }
                    @if (n + 1) * per < total { a.btn.small href={ (page.path("levels/members")) "?page=" (n + 1) } { "Next" } }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Levels", None, body).into_response())
}

fn member_avatar(ctx: &Ctx, guild: u64, user: u64) -> String {
    let _ = guild;
    let hash = ctx.cache.user(bot_core::twilight_model::id::Id::new(user)).and_then(|u| u.avatar.map(|a| a.to_string()));
    bot_core::discord::avatar_url(user, hash.as_deref())
}

async fn set_member_xp(State(ctx): State<Ctx>, user: User, Path((guild, member)): Path<(u64, u64)>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let xp = form.i64("xp").filter(|x| *x >= 0).ok_or_else(|| WebError::Bad("XP must be 0 or more.".into()))?;
    crate::set_xp(&ctx, guild, member, xp).await?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Set {}'s XP to {}", ctx.display_name(guild, member), text::num(xp)));
    Ok(saved(&page.path("levels/members")))
}

async fn import_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let local = legacy_db_path().filter(|p| std::path::Path::new(p).exists());
    let body = html! {
        (tabs(&page, "Import"))
        @if let Some(path) = &local {
            (ui::section("The old bot's database is on this server", Some(&format!("Found at {path}. Importing reads it and leaves it as it is.")), html! {
                @if page.can_edit() {
                    form.inline-form method="post" action=(page.path("levels/import-local")) {
                        (ui::select("mode", &[("replace".into(), "Replace XP with the old bot's".into()), ("higher".into(), "Keep whichever is higher".into())], "replace"))
                        button.btn.primary type="submit" { "Import from the old bot" }
                    }
                }
            }))
        }
        (ui::section(if local.is_some() { "Or upload a file" } else { "Bring XP over from antho-s-bot" }, Some("Use a backup of its database, or the JSON the Render Shell command prints. Only members of this server are imported. Reward roles and XP settings come too."), html! {
            div.card style="padding:18px" {
                div.stack {
                    input.input id="import-file" type="file" accept=".db,.sqlite,.json,application/json" style="height:auto;padding:8px";
                    div.inline-form {
                        (ui::select("import-mode", &[("replace".into(), "Replace XP with the file's".into()), ("higher".into(), "Keep whichever is higher".into())], "replace"))
                        button.btn.primary id="import-go" type="button" { "Import" }
                    }
                    p.muted.small id="import-result" {}
                }
            }
        }))
        script { (bot_core::maud::PreEscaped(format!(r#"
document.getElementById("import-go").addEventListener("click", async () => {{
  const file = document.getElementById("import-file").files[0];
  const out = document.getElementById("import-result");
  if (!file) {{ out.textContent = "Pick a file first."; return; }}
  out.textContent = "Importing…";
  const mode = document.getElementById("import-mode").value;
  try {{
    const res = await fetch("{path}?mode=" + mode, {{ method: "POST", body: file }});
    const data = await res.json();
    out.textContent = data.message;
    if (data.ok) window.botToast("Imported");
  }} catch (e) {{ out.textContent = "The upload failed. Try again."; }}
}});
"#, path = page.path("levels/import")))) }
    };
    Ok(ui::layout(&ctx, &page, "Levels", None, body).into_response())
}

/// Where antho-s-bot kept its database on the shared Render disk.
fn legacy_db_path() -> Option<String> {
    let p = std::env::var("LEGACY_DB_PATH").unwrap_or_else(|_| "/data/bot.db".into());
    (!p.trim().is_empty() && p != "off").then_some(p)
}

async fn import_local(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let path = legacy_db_path().filter(|p| std::path::Path::new(p).exists()).ok_or(WebError::NotFound)?;
    let mode = if form.get("mode") == Some("higher") { import::Mode::KeepHigher } else { import::Mode::Replace };
    let ctx2 = ctx.clone();
    let page_admin = page.user.admin;
    let report = bot_core::tokio::task::spawn_blocking(move || import::import_db(&ctx2, std::path::Path::new(&path), Some(guild), mode, page_admin))
        .await
        .map_err(|e| WebError::Internal(e.into()))?
        .map_err(|e| WebError::Bad(format!("Couldn't import: {e}")))?;
    let msg = report.summary(&ctx);
    ctx.audit(&page.user, Some(guild), KEY, msg.clone());
    ctx.notify(Some(guild), KEY, msg, Some(page.path("levels/members")));
    Ok(saved(&page.path("levels/members")))
}

#[derive(Deserialize)]
struct ImportQuery {
    mode: Option<String>,
}

async fn import_upload(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<ImportQuery>, body: Bytes) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let mode = if q.mode.as_deref() == Some("higher") { import::Mode::KeepHigher } else { import::Mode::Replace };
    let res = tokio_blocking(ctx.clone(), body, guild, mode, page.user.admin).await;
    let (ok, message) = match res {
        Ok(report) => {
            let m = report.summary(&ctx);
            ctx.audit(&page.user, Some(guild), KEY, m.clone());
            (true, m)
        }
        Err(e) => (false, format!("Couldn't import: {e}")),
    };
    Ok(Json(serde_json::json!({ "ok": ok, "message": message })).into_response())
}

async fn tokio_blocking(ctx: Ctx, body: Bytes, guild: u64, mode: import::Mode, admin: bool) -> bot_core::anyhow::Result<import::Report> {
    // SQLite work on a large file shouldn't hold up the async workers.
    bot_core::tokio::task::spawn_blocking(move || import::import_bytes(&ctx, &body, Some(guild), mode, admin)).await?
}

/// The overview's "Most XP this week" block and recent level-ups.
pub(crate) fn overview(ctx: &Ctx, page: &Page) -> bot_core::Overview {
    let guild = page.guild_id;
    let week: Vec<(u64, i64)> = ctx
        .db
        .with(|c| {
            let mut s = c.prepare(
                "SELECT l.user_id, l.xp - COALESCE(w.xp, 0) AS gain FROM levels l
                 LEFT JOIN level_week w ON w.guild_id = l.guild_id AND w.user_id = l.user_id
                 WHERE l.guild_id = ?1 AND gain > 0 ORDER BY gain DESC LIMIT 4",
            )?;
            let rows = s.query_map([guild as i64], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?)))?.collect::<bot_core::rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap_or_default();
    let ups: Vec<(u64, i64, Option<i64>, i64)> = ctx
        .db
        .with(|c| {
            let mut s = c.prepare("SELECT user_id, level, role_id, at FROM level_ups WHERE guild_id = ?1 AND at > ?2 ORDER BY at DESC LIMIT 5")?;
            let rows = s
                .query_map(params![guild as i64, bot_core::now_ms() - 2 * 86_400_000], |r| {
                    Ok((r.get::<_, i64>(0)? as u64, r.get(1)?, r.get(2)?, r.get(3)?))
                })?
                .collect::<bot_core::rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap_or_default();
    let roles = ctx.roles(guild);
    let mut o = bot_core::Overview::default();
    if !week.is_empty() {
        o.panel.push((20, html! {
            h2 { "Most XP this week" }
            @for (user, gain) in &week {
                a.mini href=(page.path("levels/members")) {
                    img.avatar src=(ctx.member_avatar(guild, *user)) alt="" width="24" height="24" loading="lazy";
                    div.grow { span { (ctx.display_name(guild, *user)) } }
                    span.mono { "+" (text::num(*gain)) }
                }
            }
        }));
    }
    for (user, level, role, at) in ups {
        let role = role.and_then(|r| roles.iter().find(|x| x.0 as i64 == r)).map(|r| format!(" and got @{}", r.1)).unwrap_or_default();
        o.feed.push(bot_core::Happening {
            at,
            avatar: ctx.member_avatar(guild, user),
            who: ctx.display_name(guild, user),
            text: format!("reached level {level}{role}"),
        });
    }
    o
}
