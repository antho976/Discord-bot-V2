use std::collections::{HashMap, HashSet};

use bot_core::axum::body::Bytes;
use bot_core::axum::extract::{DefaultBodyLimit, Path, Query, State};
use bot_core::axum::response::{IntoResponse, Json, Redirect};
use bot_core::axum::routing::{get, post};
use bot_core::axum::Router;
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::rusqlite::params;
use bot_core::twilight_model::id::Id;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, serde_json, text};
use serde::Deserialize;

use crate::{KEY, Settings, curve, import};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/levels", get(members_page))
        .route("/g/{guild}/levels/settings", get(settings_page).post(save_settings))
        .route("/g/{guild}/levels/rewards", get(rewards_page).post(add_reward))
        .route("/g/{guild}/levels/rewards/{level}/delete", post(delete_reward))
        .route("/g/{guild}/levels/rewards/{level}/edit", post(edit_reward))
        .route("/g/{guild}/levels/members", get(|Path(guild): Path<u64>| async move { Redirect::to(&format!("/g/{guild}/levels")) }))
        .route("/g/{guild}/levels/members/{user}", post(set_member_xp))
        .route("/g/{guild}/levels/import", get(import_page).post(import_upload).layer(DefaultBodyLimit::max(64 * 1024 * 1024)))
        .route("/g/{guild}/levels/import-local", post(import_local))
}

fn tabs(page: &Page, active: &str) -> Markup {
    ui::tabs(
        &[
            ("Leaderboard", page.path("levels")),
            ("Rewards", page.path("levels/rewards")),
            ("Settings", page.path("levels/settings")),
            ("Import", page.path("levels/import")),
        ],
        active,
    )
}

async fn settings_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let custom = !s.custom_curve.is_empty();
    let first_levels: Vec<String> = (1..=5u32)
        .map(|l| text::num(curve::xp_to_reach(l, s.curve_base, s.curve_factor, &s.custom_curve)))
        .collect();

    let body = html! {
        (tabs(&page, "Settings"))
        (ui::settings_form(&page, &page.path("levels/settings"), html! {
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
    };
    Ok(ui::layout(&ctx, &page, "Levels", None, body).into_response())
}

/// Each place keeps its spot on the podium even when fewer than three members have XP.
const CSS: &str = "
.podium .step.p1 { grid-column: 2; grid-row: 1; } .podium .step.p2 { grid-column: 1; grid-row: 1; } .podium .step.p3 { grid-column: 3; grid-row: 1; }
.lv-role { display: inline-flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.lv-acts { display: flex; gap: 6px; justify-content: flex-end; }
.lv-acts .pop-body { width: 240px; right: 0; left: auto; }
.lv-acts .pop-body .stack { gap: 10px; }
.lv-name { display: inline-flex; align-items: center; gap: 8px; white-space: nowrap; }
@media (max-width: 560px) {
  .lv-table th:nth-child(3), .lv-table td:nth-child(3) { display: none; }
  .lv-acts { flex-direction: column; align-items: flex-end; }
}
";

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
    Ok(saved(&page.path("levels/settings")))
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

/// Why the bot can't give a reward role, or None when it can.
fn cant_give(ctx: &Ctx, guild: u64, role: u64) -> Option<&'static str> {
    let Some(r) = ctx.cache.role(Id::new(role)) else { return Some("Deleted role") };
    if r.managed {
        return Some("Managed by an integration");
    }
    let me = ctx.bot_id().and_then(|bot| ctx.cache.member(Id::new(guild), Id::new(bot)))?;
    let top = me.roles().iter().filter_map(|id| ctx.cache.role(*id).map(|r| r.position)).max().unwrap_or(0);
    (r.position >= top).then_some("Above the bot's role")
}

/// A role's colour for its dot; roles without one get the caption grey.
fn role_color(color: u32) -> String {
    if color == 0 { "var(--ink-3)".to_owned() } else { format!("#{color:06x}") }
}

fn role_options(ctx: &Ctx, guild: u64) -> Vec<(String, String)> {
    ctx.roles(guild).into_iter().map(|(id, n, _)| (id.to_string(), format!("@{n}"))).collect()
}

/// Change a reward's level or role where it stands.
async fn edit_reward(State(ctx): State<Ctx>, user: User, Path((guild, level)): Path<(u64, i64)>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let new_level = form.i64("level").filter(|l| (1..=500).contains(l)).ok_or_else(|| WebError::Bad("Pick a level from 1 to 500.".into()))?;
    // Each row's select has its own name, so the page has no duplicate ids.
    let role = form.role(&ctx, guild, &format!("role-{level}")).ok_or_else(|| WebError::Bad("Pick a role.".into()))?;
    let old_role = crate::rewards(&ctx, guild)?.into_iter().find(|(l, _)| *l as i64 == level).map(|(_, r)| r).ok_or(WebError::NotFound)?;
    if role != old_role && !page.user.admin && !ctx.safe_role(guild, role) {
        return Err(WebError::Bad("That role has moderation or admin permissions, or sits above the bot's role. Only a dashboard admin can make it a reward.".into()));
    }
    let moved = {
        let mut conn = ctx.db.lock();
        let tx = conn.transaction().map_err(|e| WebError::Internal(e.into()))?;
        let taken = new_level != level
            && tx
                .query_row("SELECT 1 FROM level_rewards WHERE guild_id = ?1 AND level = ?2", params![guild as i64, new_level], |_| Ok(()))
                .is_ok();
        if !taken {
            tx.execute(
                "UPDATE level_rewards SET level = ?3, role_id = ?4 WHERE guild_id = ?1 AND level = ?2",
                params![guild as i64, level, new_level, role as i64],
            )
            .and_then(|_| tx.commit())
            .map_err(|e| WebError::Internal(e.into()))?;
        }
        !taken
    };
    if !moved {
        return Err(WebError::Bad(format!("Level {new_level} already has a reward. Change or remove that one first.")));
    }
    ctx.audit(&page.user, Some(guild), KEY, format!("Changed the level {level} reward to {} at level {new_level}", role_name(&ctx, guild, role)));
    Ok(saved(&page.path("levels/rewards")))
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
    Ok(saved(&page.path("levels/rewards")))
}

async fn delete_reward(State(ctx): State<Ctx>, user: User, Path((guild, level)): Path<(u64, i64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    ctx.db.with(|c| c.execute("DELETE FROM level_rewards WHERE guild_id = ?1 AND level = ?2", params![guild as i64, level]))?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Removed the level {level} reward"));
    Ok(saved(&page.path("levels/rewards")))
}

#[derive(Deserialize)]
struct MembersQuery {
    q: Option<String>,
    page: Option<i64>,
}

/// Name and avatar of members Discord found for a search, who the bot's cache doesn't hold.
type Found = HashMap<u64, (String, String)>;

/// Members whose name contains `needle`, or whose id it is. The cache only holds members the bot
/// has seen lately, so on a big server most are missing: Discord's member search fills the gap
/// (it matches the start of a username or nickname) and brings their name and avatar along.
async fn find_members(ctx: &Ctx, guild: u64, needle: &str) -> (HashSet<u64>, Found) {
    let lower = needle.to_lowercase();
    let mut ids: HashSet<u64> = ctx
        .cache
        .guild_members(Id::new(guild))
        .map(|set| set.iter().map(|u| u.get()).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|u| ctx.display_name(guild, *u).to_lowercase().contains(&lower))
        .collect();
    let mut found = Found::new();
    if let Ok(id) = needle.parse::<u64>() {
        ids.insert(id);
        return (ids, found);
    }
    let members = match ctx.http.search_guild_members(Id::new(guild), needle).limit(1000).await {
        Ok(res) => res.models().await.unwrap_or_default(),
        Err(e) => {
            bot_core::tracing::debug!(guild, error = %e, "member search failed");
            Vec::new()
        }
    };
    for m in members {
        let id = m.user.id.get();
        if ids.insert(id) {
            let name = m.nick.clone().or_else(|| m.user.global_name.clone()).unwrap_or_else(|| m.user.name.clone());
            let avatar = match m.avatar {
                Some(a) => format!("https://cdn.discordapp.com/guilds/{guild}/users/{id}/avatars/{a}.png?size=128"),
                None => bot_core::discord::avatar_url(id, m.user.avatar.map(|a| a.to_string()).as_deref()),
            };
            found.insert(id, (name, avatar));
        }
    }
    (ids, found)
}

async fn members_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<MembersQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let per = 25;
    let search = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let total = crate::count(&ctx, guild)?;
    // The podium takes the first three; the pages of the list start after it.
    let pages = ((total - 3).max(0) + per - 1) / per;
    let n = q.page.unwrap_or(0).clamp(0, (pages - 1).max(0));
    let podium = if search.is_none() && n == 0 { crate::top(&ctx, guild, 0, 3)? } else { Vec::new() };
    let start = 3 + n * per;
    let (rows, found, matched): (Vec<(i64, crate::Member)>, Found, i64) = match search {
        Some(needle) => {
            let (ids, found) = find_members(&ctx, guild, needle).await;
            let (rows, matched) = crate::find(&ctx, guild, &ids, per as usize)?;
            (rows, found, matched)
        }
        None => {
            let rows = crate::top(&ctx, guild, start, per)?;
            (rows.into_iter().enumerate().map(|(i, m)| (start + i as i64 + 1, m)).collect(), Found::new(), 0)
        }
    };
    let rewards = crate::rewards(&ctx, guild)?;
    let roles = ctx.roles(guild);
    let at_least = |level: u32| -> i64 {
        ctx.db
            .with(|c| c.query_row("SELECT COUNT(*) FROM levels WHERE guild_id = ?1 AND level >= ?2", params![guild as i64, level as i64], |r| r.get(0)))
            .unwrap_or(0)
    };
    let week: (i64, i64) = ctx
        .db
        .with(|c| {
            c.query_row(
                "SELECT COALESCE(SUM(l.xp - COALESCE(w.xp, 0)), 0), COUNT(*) FROM levels l
                 LEFT JOIN level_week w ON w.guild_id = l.guild_id AND w.user_id = l.user_id
                 WHERE l.guild_id = ?1 AND l.xp > COALESCE(w.xp, 0)",
                [guild as i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
        })
        .unwrap_or((0, 0));
    let name = |u: u64| found.get(&u).map(|f| f.0.clone()).unwrap_or_else(|| ctx.display_name(guild, u));
    let avatar = |u: u64| found.get(&u).map(|f| f.1.clone()).unwrap_or_else(|| ctx.member_avatar(guild, u));
    let line = |m: &crate::Member| format!("Level {} · {} XP", s.progress(m.xp).level, text::num(m.xp));
    let earning = {
        let mut parts = vec![format!("{} to {} per message, once every {}.", s.xp_min, s.xp_max, text::duration(s.cooldown_secs))];
        if s.voice_xp > 0 {
            parts.push(format!("{} per minute in voice with someone else{}.", s.voice_xp, if s.voice_require_active { ", unmuted" } else { "" }));
        }
        if s.reaction_xp > 0 {
            parts.push(format!("{} for a reaction.", s.reaction_xp));
        }
        parts.join(" ")
    };

    let main = html! {
        style { (PreEscaped(CSS)) }
        @if !podium.is_empty() {
            div.podium {
                @for (place, i) in [(2usize, 1usize), (1, 0), (3, 2)] {
                    @if let Some(m) = podium.get(i) {
                        div class={ "step p" (place) } {
                            img.avatar src=(ctx.member_avatar(guild, m.user_id)) alt="" loading="lazy";
                            strong { (name(m.user_id)) }
                            span.muted.small { (line(m)) }
                            div.block { (place) }
                        }
                    }
                }
            }
        }
        div.list-head {
            h2 { @if podium.is_empty() { "Members" } @else { "Everyone else" } }
            form method="get" action=(page.path("levels")) {
                input.input.search type="search" name="q" value=(search.unwrap_or_default()) placeholder="Search members";
            }
        }
        @if rows.is_empty() && podium.is_empty() {
            @if search.is_some() {
                (ui::empty(Icon::Users, "No match", "Nobody with XP matches that name or ID."))
            } @else {
                (ui::empty(Icon::Trophy, "Nobody has XP yet", "Members show up here once they chat, or after you import XP from the old bot."))
            }
        } @else if rows.is_empty() && search.is_none() {
            p.muted.small { "Everyone with XP is on the podium so far." }
        } @else {
            div.ranks {
                @for (place, m) in &rows {
                    @let p = s.progress(m.xp);
                    @let pct = if p.needed > 0 { (p.into * 100 / p.needed).clamp(0, 100) } else { 0 };
                    div.rank {
                        span.mono.muted.pos { (place) }
                        img.avatar src=(avatar(m.user_id)) alt="" width="28" height="28" loading="lazy";
                        div.who { strong { (name(m.user_id)) } span.muted.small { "Level " (p.level) } }
                        div.progress { span.meter { span style={ "width:" (pct) "%" } {} } span.muted.small { (pct) "% to level " (p.level + 1) } }
                        span.mono.xp { (text::num(m.xp)) " XP" }
                        @if page.can_edit() {
                            details.pop.set-xp {
                                summary.btn.small.outline { "Set XP" }
                                div.pop-body {
                                    form.inline-form method="post" action=(page.path(&format!("levels/members/{}", m.user_id))) {
                                        input.input.num type="number" name="xp" value=(m.xp) min="0";
                                        button.btn.primary type="submit" { "Set" }
                                    }
                                    p.muted.small { "Their level and reward roles follow the new XP." }
                                }
                            }
                        }
                    }
                }
            }
            @if matched > rows.len() as i64 {
                p.muted.small { "Showing the top " (rows.len()) " of " (text::num(matched)) " matches. Type more of the name to narrow it." }
            }
            @if search.is_none() && pages > 1 {
                div.inline-form {
                    @if n > 0 { a.btn.small.outline href={ (page.path("levels")) "?page=" (n - 1) } { "Previous" } }
                    span.muted.small { "Page " (n + 1) " of " (pages) }
                    @if n + 1 < pages { a.btn.small.outline href={ (page.path("levels")) "?page=" (n + 1) } { "Next" } }
                }
            }
        }
    };
    let body = html! {
        (tabs(&page, "Leaderboard"))
        div.with-rail {
            div.stack.lb { (main) }
            aside.rail {
                div.rail-block {
                    h2 { "Reward roles" }
                    @if rewards.is_empty() {
                        p.muted.small { "None yet. " a href=(page.path("levels/rewards")) { "Add one" } " and members get it at that level." }
                    }
                    @for (level, role) in &rewards {
                        @let r = roles.iter().find(|x| x.0 == *role);
                        div.mini {
                            span.lvl { (level) }
                            span.dot style={ "background:" (r.map(|r| if r.2 == 0 { "var(--ink-3)".to_owned() } else { format!("#{:06x}", r.2) }).unwrap_or_else(|| "var(--ink-3)".into())) } {}
                            div.grow { span { (r.map(|r| r.1.clone()).unwrap_or_else(|| "Deleted role".into())) } }
                            span.mono { (text::num(at_least(*level))) }
                        }
                    }
                }
                div.rail-block {
                    h2 { "XP earned this week" }
                    div.big-num { (text::num(week.0)) }
                    span.muted.small { "by " (text::num(week.1)) @if week.1 == 1 { " member" } @else { " members" } " since Monday" }
                }
                div.rail-block {
                    h2 { "How XP is earned" }
                    p.muted.small { (earning) }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Levels", None, body).into_response())
}

async fn rewards_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let rewards = crate::rewards(&ctx, guild)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let roles = ctx.roles(guild);
    let options = role_options(&ctx, guild);
    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, "Rewards"))
        (ui::section("Reward roles", Some(if s.stack_rewards { "Given when a member reaches the level. Members keep earlier rewards." } else { "Given when a member reaches the level. Members keep only the reward for their highest level." }), html! {
            @if rewards.is_empty() {
                (ui::empty(Icon::Trophy, "No reward roles", "Add a role below and members get it at that level."))
            } @else {
                div.table-wrap { table.table.lv-table {
                    thead { tr { th { "Level" } th { "Role" } th.num { "Members" } th {} } }
                    tbody { @for (level, role) in &rewards {
                        @let known = roles.iter().find(|r| r.0 == *role);
                        @let problem = cant_give(&ctx, guild, *role);
                        tr {
                            td.num style="text-align:left" { (level) }
                            td {
                                span.lv-role {
                                    span.lv-name {
                                        span.dot style={ "background:" (known.map(|r| role_color(r.2)).unwrap_or_else(|| "var(--ink-3)".into())) } {}
                                        (role_name(&ctx, guild, *role))
                                    }
                                    @match (known, problem) {
                                        (None, _) => { (ui::badge("Can't give", Tone::Warn)) },
                                        (Some(_), Some(why)) => { (ui::badge("Can't give", Tone::Warn)) span.muted.small { (why) } },
                                        _ => {},
                                    }
                                }
                            }
                            td.num {
                                (text::num(ctx.db.with(|c| c.query_row("SELECT COUNT(*) FROM levels WHERE guild_id = ?1 AND level >= ?2", params![guild as i64, *level as i64], |r| r.get::<_, i64>(0))).unwrap_or(0)))
                            }
                            td.actions {
                                @if page.can_edit() {
                                    div.lv-acts {
                                        details.pop {
                                            summary.btn.small { "Change" }
                                            div.pop-body {
                                                form.stack method="post" action=(page.path(&format!("levels/rewards/{level}/edit"))) {
                                                    input.input.num type="number" name="level" min="1" max="500" value=(level) aria-label="Level" required;
                                                    (ui::select(&format!("role-{level}"), &options, &role.to_string()))
                                                    button.btn.primary type="submit" { "Save" }
                                                }
                                            }
                                        }
                                        (ui::action_button(&page.path(&format!("levels/rewards/{level}/delete")), "Remove", Tone::Bad, true))
                                    }
                                }
                            }
                        }
                    } }
                } }
                @if rewards.iter().any(|(_, r)| cant_give(&ctx, guild, *r).is_some()) {
                    p.muted.small { "Members reaching a level marked Can't give get nothing. Pick another role, or move the bot's role above it in Discord." }
                }
            }
            @if page.can_edit() {
                form.inline-form method="post" action=(page.path("levels/rewards")) {
                    input.input.num type="number" name="level" min="1" max="500" placeholder="Level" required style="max-width:110px";
                    (ui::select("role", &std::iter::once((String::new(), "Pick a role".to_owned())).chain(options.iter().cloned()).collect::<Vec<_>>(), ""))
                    button.btn type="submit" { "Add reward" }
                }
            }
        }))
    };
    Ok(ui::layout(&ctx, &page, "Levels", None, body).into_response())
}

async fn set_member_xp(State(ctx): State<Ctx>, user: User, Path((guild, member)): Path<(u64, u64)>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let xp = form.i64("xp").filter(|x| *x >= 0).ok_or_else(|| WebError::Bad("XP must be 0 or more.".into()))?;
    crate::set_xp(&ctx, guild, member, xp).await?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Set {}'s XP to {}", ctx.display_name(guild, member), text::num(xp)));
    Ok(saved(&page.path("levels")))
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
                a.mini href=(page.path("levels")) {
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
