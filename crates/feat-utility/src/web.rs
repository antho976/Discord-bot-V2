use std::collections::HashSet;

use bot_core::axum::Router;
use bot_core::axum::extract::{Path, State};
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::{get, post};
use bot_core::jiff::Timestamp;
use bot_core::maud::{PreEscaped, html};
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

/// "Europe/Warsaw" -> "Warsaw", "America/New_York" -> "New York".
fn city(zone: &str) -> String {
    zone.rsplit('/').next().unwrap_or(zone).replace('_', " ")
}

/// Daytime is 06:00 to 18:00 on the member's clock.
fn is_day(hour: u32) -> bool {
    (6..18).contains(&hour)
}

fn member_line(n: usize) -> String {
    match n {
        1 => "1 member sets a timezone with /timezone".into(),
        n => format!("{} members set a timezone with /timezone", text::num(n as i64)),
    }
}

/// A sun in the same drawing style as the dashboard's own icons.
fn sun() -> bot_core::maud::Markup {
    html! {
        svg.icon viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" aria-hidden="true" {
            circle cx="8" cy="8" r="2.75" {}
            (PreEscaped(r#"<path d="M8 1.5v1.5M8 13v1.5M1.5 8H3M13 8h1.5M3.4 3.4l1.05 1.05M11.55 11.55l1.05 1.05M3.4 12.6l1.05-1.05M11.55 4.45l1.05-1.05"/>"#))
        }
    }
}

/// The options that used to sit in a card: one switch per setting, with its help line.
fn opt_switch(name: &str, label: &str, help: &str, on: bool) -> bot_core::maud::Markup {
    html! {
        div.au-opt {
            div.au-opt-text {
                label for=(name) { (label) }
                p.help { (help) }
            }
            label.switch {
                input type="checkbox" id=(name) name=(name) checked[on];
                span.track { span.thumb {} }
            }
        }
    }
}

async fn page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let away = afk_list(&ctx, guild)?;
    let times = member_times(&ctx, guild)?;
    const SHOWN: usize = 200;
    let who = |u: u64| ctx.display_name(guild, u);

    let body = html! {
        style { (PreEscaped(CSS)) }
        div.with-rail.au {
            div.stack.au-main {
                (ui::section("Where everyone is right now", None, html! {
                    @if times.is_empty() {
                        (ui::empty(Icon::Clock, "Nobody has set a timezone", "Members add theirs with /timezone set, and their local time shows here."))
                    } @else {
                        div.au-grid {
                            @for (u, zone, l) in times.iter().take(SHOWN) {
                                div.au-card title={ (zone) " · " (l.offset) } {
                                    img.avatar src=(ctx.member_avatar(guild, *u)) alt="" width="40" height="40" loading="lazy";
                                    div.au-who {
                                        strong.clip { (who(*u)) }
                                        span.clip { (city(zone)) }
                                    }
                                    span.au-time {
                                        @if is_day(l.hour) {
                                            span.au-glyph.sun role="img" aria-label="Daytime" { (sun()) }
                                        } @else {
                                            span.au-glyph.moon role="img" aria-label="Night" { (ui::icon(Icon::Moon)) }
                                        }
                                        span.au-clock { (l.clock) }
                                    }
                                }
                            }
                        }
                        p.muted.small { (member_line(times.len())) }
                        @if times.len() > SHOWN { p.muted.small { "Showing the first " (SHOWN) " of " (times.len()) "." } }
                    }
                }))
            }
            aside.rail {
                div.rail-block {
                    h2 { "Away right now" }
                    @if away.is_empty() {
                        (ui::empty(Icon::Moon, "Nobody is AFK", "Members show up here after they use /afk."))
                    } @else {
                        div.au-aways {
                            @for a in &away {
                                div.au-away {
                                    img.avatar src=(ctx.member_avatar(guild, a.user_id)) alt="" width="28" height="28" loading="lazy";
                                    div.grow {
                                        span {
                                            strong { (who(a.user_id)) }
                                            @if !a.reason.is_empty() { span.muted { " · " (a.reason) } }
                                        }
                                        span.muted.small { "since " (text::ago(a.since_ms)) }
                                    }
                                    @if page.can_edit() {
                                        (ui::action_button(&page.path(&format!("{KEY}/afk/{}/clear", a.user_id)), "Clear", Tone::Muted, false))
                                    }
                                }
                            }
                        }
                    }
                }
                div.rail-block {
                    h2 { "Options" }
                    (ui::settings_form(&page, &page.path(KEY), html! {
                        div.au-opts {
                            (opt_switch("afk_enabled", "AFK", "Off: /afk does nothing and mentions get no notice.", s.afk_enabled))
                            (opt_switch("nick_prefix", "Put [AFK] in their nickname", "Skipped for the owner and for members above the bot's role.", s.nick_prefix))
                            (opt_switch("welcome_back", "Say welcome back when they return", "A short message that deletes itself after a few seconds.", s.welcome_back))
                            (opt_switch("timezones_enabled", "Timezones", "Off: /timezone and /time do nothing. Saved zones are kept.", s.timezones_enabled))
                        }
                    }))
                }
            }
        }
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

const CSS: &str = r#"
.au-main { min-width: 0; }
.au-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 12px; }
.au-card { display: flex; align-items: center; gap: 12px; min-width: 0; padding: 14px 16px; border-radius: var(--r-card); background: var(--veil); border: 1px solid var(--edge-1); }
.au-card > .avatar { width: 40px; height: 40px; }
.au-who { display: flex; flex-direction: column; gap: 1px; min-width: 0; flex: 1; }
.au-who strong { font-size: 14px; font-weight: 500; }
.au-who span { font-size: 12px; color: var(--ink-3); }
.au-time { display: flex; align-items: center; gap: 8px; flex: none; }
.au-glyph { display: inline-grid; place-items: center; }
.au-glyph.sun { color: var(--waiting); }
.au-glyph.moon { color: var(--ink-3); }
.au-clock { font: 14px/1 var(--mono); color: var(--ink); font-variant-numeric: tabular-nums; }
.au-aways { display: flex; flex-direction: column; gap: 14px; }
.au-away { display: flex; align-items: center; gap: 10px; font-size: 13px; min-width: 0; }
.au-away > .avatar { width: 28px; height: 28px; }
.au-away .grow { display: flex; flex-direction: column; gap: 1px; min-width: 0; flex: 1; }
.au-away .grow > span { line-height: 1.4; overflow-wrap: anywhere; }
.au-away strong { font-weight: 500; }
.au-away .muted { font-size: 12px; }
.au-away .btn { flex: none; }
.au-opts { display: flex; flex-direction: column; }
.au-opt { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 10px 0; }
.au-opt + .au-opt { border-top: 1px solid var(--edge-1); }
.au-opt label { font-size: 13px; font-weight: 500; }
.au-opt .help { font-size: 12px; color: var(--ink-3); margin-top: 2px; }
.au-opt .switch { flex: none; }
"#;
