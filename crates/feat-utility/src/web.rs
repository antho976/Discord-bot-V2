use std::collections::HashSet;

use bot_core::axum::Router;
use bot_core::axum::extract::{Path, State};
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::{get, post};
use bot_core::jiff::Timestamp;
use bot_core::maud::html;
use bot_core::twilight_model::id::Id;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, User, WebResult, saved};
use bot_core::{Ctx, Icon, text};

use crate::{KEY, Settings, afk_clear, afk_list, restore_nick, tz_all, zones};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/utility", get(page).post(save_settings))
        .route("/g/{guild}/utility/afk/{user}/clear", post(clear_afk))
}

/// Members of this server with a timezone, by what their clock reads, earliest first.
fn member_times(ctx: &Ctx, guild: u64) -> bot_core::anyhow::Result<Vec<(u64, String, zones::Local)>> {
    let members: HashSet<u64> = ctx
        .cache
        .guild_members(Id::new(guild))
        .map(|set| set.iter().map(|u| u.get()).collect())
        .unwrap_or_default();
    let now = Timestamp::now();
    let mut rows: Vec<_> = tz_all(ctx)?
        .into_iter()
        .filter(|(u, _)| members.contains(u))
        .filter_map(|(u, zone)| zones::local(&zone, now).map(|l| (u, zone, l)))
        .collect();
    rows.sort_by(|a, b| a.2.offset_secs.cmp(&b.2.offset_secs).then_with(|| a.1.cmp(&b.1)));
    Ok(rows)
}

fn avatar(ctx: &Ctx, user: u64) -> String {
    let hash = ctx.cache.user(Id::new(user)).and_then(|u| u.avatar.map(|a| a.to_string()));
    bot_core::discord::avatar_url(user, hash.as_deref())
}

async fn page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let away = afk_list(&ctx, guild)?;
    let times = member_times(&ctx, guild)?;
    const SHOWN: usize = 200;

    let body = html! {
        (ui::readings(&[
            ("AFK now", away.len().to_string()),
            ("members with a timezone", times.len().to_string()),
        ]))
        (ui::settings_form(&page, &page.path(KEY), html! {
            (ui::section("AFK", Some("/afk marks someone away. Their next message ends it."), ui::card(html! {
                (ui::field_toggle("afk_enabled", "AFK", Some("Off: /afk does nothing and mentions get no notice."), s.afk_enabled))
                (ui::field_toggle("nick_prefix", "Put [AFK] in the nickname", Some("Skipped for the owner and for members above the bot's role."), s.nick_prefix))
                (ui::field_toggle("welcome_back", "Say welcome back", Some("A short message that deletes itself after a few seconds."), s.welcome_back))
            })))
            (ui::section("Timezones", None, ui::card(html! {
                (ui::field_toggle("timezones_enabled", "Timezones", Some("Off: /timezone and /time do nothing. Saved zones are kept."), s.timezones_enabled))
            })))
        }))
        (ui::section("AFK now", None, html! {
            @if away.is_empty() {
                (ui::empty(Icon::Moon, "Nobody is AFK", "Members show up here after they use /afk."))
            } @else {
                div.table-wrap { table.table {
                    thead { tr { th { "Member" } th { "Reason" } th { "Since" } th {} } }
                    tbody { @for a in &away {
                        tr {
                            td { (ui::person(&ctx.display_name(guild, a.user_id), &avatar(&ctx, a.user_id), None)) }
                            td { @if a.reason.is_empty() { span.muted { "No reason" } } @else { (a.reason) } }
                            td.muted { (text::ago(a.since_ms)) }
                            td.actions { @if page.can_edit() { (ui::action_button(&page.path(&format!("{KEY}/afk/{}/clear", a.user_id)), "Clear", Tone::Muted, false)) } }
                        }
                    } }
                } }
            }
        }))
        (ui::section("Member times", Some("Members who set a timezone, by the time on their clock."), html! {
            @if times.is_empty() {
                (ui::empty(Icon::Clock, "No timezones yet", "Members add theirs with /timezone set."))
            } @else {
                div.table-wrap { table.table {
                    thead { tr { th { "Member" } th.num { "Their time" } th.num { "Offset" } th { "Zone" } } }
                    tbody { @for (u, zone, l) in times.iter().take(SHOWN) {
                        tr {
                            td { (ui::person(&ctx.display_name(guild, *u), &avatar(&ctx, *u), None)) }
                            td.num { (l.time) }
                            td.num { (l.offset) }
                            td.muted { (zone) }
                        }
                    } }
                } }
                @if times.len() > SHOWN { p.muted.small { "Showing the first " (SHOWN) " of " (times.len()) "." } }
            }
        }))
    };
    Ok(ui::layout(&ctx, &page, "AFK and timezones", None, body).into_response())
}

async fn save_settings(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = Settings {
        afk_enabled: form.bool("afk_enabled"),
        nick_prefix: form.bool("nick_prefix"),
        welcome_back: form.bool("welcome_back"),
        timezones_enabled: form.bool("timezones_enabled"),
    };
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed AFK and timezone settings");
    Ok(saved(&page.path(KEY)))
}

async fn clear_afk(State(ctx): State<Ctx>, user: User, Path((guild, member)): Path<(u64, u64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    if let Some(was) = afk_clear(&ctx, guild, member)? {
        restore_nick(&ctx, guild, member, &was).await;
        ctx.audit(&page.user, Some(guild), KEY, format!("Cleared AFK for {}", ctx.display_name(guild, member)));
    }
    Ok(saved(&page.path(KEY)))
}

