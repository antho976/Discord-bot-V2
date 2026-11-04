//! The RPG's dashboard page: the closed test's settings, who's playing, the economy, how fast
//! the buttons answer, and a preview of any player's screens.

use bot_core::anyhow::Result;
use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::{IntoResponse, Redirect};
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::rusqlite::params;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms};
use serde::Deserialize;

use crate::data::{self, God};
use crate::engine::progress;
use crate::game::{self, thousands};
use crate::{KEY, Settings, store};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/rpg", get(page))
        .route("/g/{guild}/rpg/settings", post(save))
        .route("/g/{guild}/rpg/player/{user}/reset", post(reset))
        .route("/g/{guild}/rpg/player/{user}/grant", post(grant))
        .route("/g/{guild}/rpg/player/{user}/tool", post(tool))
        .route("/g/{guild}/rpg/titan", post(titan_now))
}

#[derive(Deserialize)]
struct Q {
    view: Option<String>,
    player: Option<u64>,
    screen: Option<String>,
}

struct Row {
    user: u64,
    name: String,
    patron: Option<God>,
    level: i64,
    gold: i64,
    boss: bool,
    last_seen: i64,
    power: i64,
}

fn players(ctx: &Ctx, guild: u64) -> Result<Vec<Row>> {
    let ids: Vec<u64> = ctx.db.with(|c| {
        let mut st = c.prepare(
            "SELECT user_id FROM rpg_players WHERE guild_id = ?1 ORDER BY last_seen DESC LIMIT 200",
        )?;
        st.query_map(params![guild as i64], |r| Ok(r.get::<_, i64>(0)? as u64))?
            .collect()
    })?;
    let mut out = Vec::new();
    for u in ids {
        if let Some(p) = store::get(&ctx.db, guild, u)? {
            out.push(Row {
                user: u,
                name: p.name.clone(),
                patron: p.patron,
                level: p.level as i64,
                gold: p.gold,
                boss: p.boss_won,
                last_seen: last_seen(ctx, guild, u),
                power: p.power(),
            });
        }
    }
    out.sort_by_key(|r| std::cmp::Reverse((r.power, r.level)));
    Ok(out)
}

fn last_seen(ctx: &Ctx, guild: u64, user: u64) -> i64 {
    ctx.db
        .with(|c| {
            c.query_row(
                "SELECT last_seen FROM rpg_players WHERE guild_id = ?1 AND user_id = ?2",
                params![guild as i64, user as i64],
                |r| r.get(0),
            )
        })
        .unwrap_or(0)
}

fn ago(ms: i64) -> String {
    let m = (now_ms() - ms).max(0) / 60_000;
    match m {
        0 => "just now".into(),
        1..=59 => format!("{m} min ago"),
        60..=1439 => format!("{} h ago", m / 60),
        _ => format!("{} days ago", m / 1440),
    }
}

fn pct(v: &mut [i64], q: f64) -> i64 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[((v.len() - 1) as f64 * q).round() as usize]
}

const CSS: &str = r#"
.rpg-msg { background:#313338; border-radius:10px; padding:16px; color:#dbdee1; font:15px/1.4 var(--sans); max-width:560px; display:flex; flex-direction:column; gap:8px; }
.rpg-panel { background:#2b2d31; border-left:4px solid #d2a75e; border-radius:8px; padding:14px 16px; display:flex; flex-direction:column; gap:10px; }
.rpg-banner { width:100%; border-radius:6px; display:block; }
.rpg-text b { color:#f2f3f5; }
.rpg-h { font-size:19px; font-weight:700; color:#f2f3f5; }
.rpg-sub { font-size:12.5px; color:#949ba4; }
.rpg-blank { height:6px; }
.rpg-section { display:flex; gap:12px; align-items:center; justify-content:space-between; }
.rpg-gap { height:2px; }
.rpg-row { display:flex; gap:8px; flex-wrap:wrap; }
.rpg-btn { display:inline-flex; align-items:center; gap:6px; height:32px; padding:0 14px; border-radius:8px; background:#4e5058; color:#fff; font-size:14px; font-weight:500; white-space:nowrap; }
.rpg-btn.primary { background:#5865f2; } .rpg-btn.danger { background:#da373c; } .rpg-btn.success { background:#248046; }
.rpg-btn[data-off] { opacity:.5; }
.rpg-select { display:flex; justify-content:space-between; height:40px; align-items:center; padding:0 12px; background:#1e1f22; border-radius:8px; color:#949ba4; font-size:14px; }
.rpg-emoji { width:20px; height:20px; vertical-align:-4px; display:inline-block; }
.rpg-shots { display:flex; flex-wrap:wrap; gap:24px; align-items:flex-start; }
.rpg-shot { display:flex; flex-direction:column; gap:8px; }
.rpg-shot h3 { margin:0; font-size:13px; color:var(--muted, #a29d97); font-weight:500; }
"#;

async fn page(
    State(ctx): State<Ctx>,
    user: User,
    Path(guild): Path<u64>,
    Query(q): Query<Q>,
) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    store::set_speed(guild, s.speed);
    let view = q.view.as_deref().unwrap_or("test");
    let tabs = ui::tabs(
        &[
            ("Test", page.path("rpg")),
            ("Players", page.path("rpg?view=players")),
            ("Economy", page.path("rpg?view=economy")),
            ("Screens", page.path("rpg?view=screens")),
            ("Settings", page.path("rpg?view=settings")),
        ],
        match view {
            "players" => "Players",
            "economy" => "Economy",
            "screens" => "Screens",
            "settings" => "Settings",
            _ => "Test",
        },
    );
    let body = match view {
        "players" => players_view(&ctx, &page, guild)?,
        "economy" => economy_view(&ctx, guild)?,
        "screens" => screens_view(&ctx, &page, guild, q.player, q.screen.as_deref())?,
        "player" => player_view(&ctx, &page, guild, q.player)?,
        "settings" => settings_view(&ctx, &page, guild, &s),
        _ => test_view(&ctx, &page, guild, &s)?,
    };
    Ok(ui::layout(
        &ctx,
        &page,
        "RPG",
        None,
        html! { style { (PreEscaped(CSS)) } (tabs) (body) },
    )
    .into_response())
}

fn test_view(ctx: &Ctx, page: &Page, guild: u64, s: &Settings) -> Result<Markup> {
    let week = store::week_now();
    let (total, goal, active, kills, players_n) = ctx.db.with(|c| {
        let o = store::offerings(c, guild, week).unwrap_or([0; 3]);
        let goal = store::goal(c, guild, week).unwrap_or(1);
        let active = store::active_players(c, guild, week).unwrap_or(0);
        let kills: i64 = c.query_row(
            "SELECT COUNT(*) FROM rpg_kills WHERE guild_id = ?1",
            params![guild as i64],
            |r| r.get(0),
        )?;
        let n: i64 = c.query_row(
            "SELECT COUNT(*) FROM rpg_players WHERE guild_id = ?1",
            params![guild as i64],
            |r| r.get(0),
        )?;
        Ok((o.iter().sum::<i64>(), goal, active, kills, n))
    })?;
    let clicks = speed();
    let status = if !ctx.enabled(guild, KEY) {
        ui::badge("Off in this server", Tone::Muted)
    } else if s.test_only {
        ui::badge("Closed test", Tone::Warn)
    } else {
        ui::badge("Open to everyone", Tone::Ok)
    };
    let who = if !s.test_only {
        "Everyone in the server can play.".to_string()
    } else {
        match s.tester_role {
            Some(r) => format!(
                "Members with the {} role, and anyone with Manage Server, can play.",
                ctx.roles(guild)
                    .into_iter()
                    .find(|x| x.0 == r)
                    .map(|x| x.1)
                    .unwrap_or_else(|| "tester".into())
            ),
            None => {
                "Only people with Manage Server can play until you pick a tester role in Settings."
                    .into()
            }
        }
    };
    Ok(html! {
        (ui::readings(&[
            ("players", players_n.to_string()),
            ("played this week", active.to_string()),
            ("beat the Barrow King", kills.to_string()),
            ("of this week's offering", format!("{}%", total * 100 / goal.max(1))),
        ]))
        (ui::section("The test", Some(&who), html! {
            div.card {
                div.row { div.row-text { label { "Status" } p.help { "Switch the feature on per server on the Features page; the closed test is in Settings." } } div.row-control { (status) } }
                div.row { div.row-text { label { "Where people play" } p.help { "Each player gets one hub message. /rpg posts it again and removes the old one." } } div.row-control {
                    @match s.channel { Some(ch) => span { "#" (ctx.text_channels(guild).into_iter().find(|x| x.0 == ch).map(|x| x.1).unwrap_or_else(|| "deleted channel".into())) }, None => span.muted { "Any channel" } }
                } }
                div.row { div.row-text { label { "Rates" } p.help { "XP, loot and gold, for tuning during the test without a deploy." } } div.row-control { span { (s.xp_rate) "% · " (s.loot_rate) "% · " (s.gold_rate) "%" } } }
            }
        }))
        (ui::section("Titan", Some("Server-wide boss events. One rises on the schedule in Settings, or now."), titan_block(ctx, page, guild)?))
        (ui::section("How fast the buttons answer", Some("From Discord's click arriving to the new screen being sent. Work is the bot's own time: loading the player, the rules, saving. Reply is Discord accepting the new screen."), clicks))
        @if page.can_edit() { p.muted.small { "Tip: open Screens to see exactly what any player sees right now." } }
    })
}

fn speed() -> Markup {
    let clicks = crate::clicks();
    if clicks.is_empty() {
        return ui::empty(
            Icon::Clock,
            "No clicks yet",
            "Timings appear here as people play. They're kept for the last 500 clicks.",
        );
    }
    let mut work: Vec<i64> = clicks.iter().map(|c| c.work_us).collect();
    let mut reply: Vec<i64> = clicks.iter().map(|c| c.reply_ms).collect();
    let fmt_us = |us: i64| {
        if us < 1000 {
            format!("{us} µs")
        } else {
            format!("{:.1} ms", us as f64 / 1000.0)
        }
    };
    ui::readings(&[
        ("clicks timed", clicks.len().to_string()),
        ("bot work, typical", fmt_us(pct(&mut work, 0.5))),
        ("bot work, slowest 5%", fmt_us(pct(&mut work, 0.95))),
        ("reply, typical", format!("{} ms", pct(&mut reply, 0.5))),
        ("reply, slowest 5%", format!("{} ms", pct(&mut reply, 0.95))),
    ])
}

fn players_view(ctx: &Ctx, page: &Page, guild: u64) -> Result<Markup> {
    let rows = players(ctx, guild)?;
    if rows.is_empty() {
        return Ok(ui::empty(
            Icon::Star,
            "No players yet",
            "Players appear here once they use /rpg and swear to a patron.",
        ));
    }
    Ok(html! {
        (ui::section("Players", Some("Strongest first. Reset deletes a character so they start again; grant adds gold for testing."), html! {
            div.card {
                table.table {
                    thead { tr { th { "Player" } th { "Patron" } th.num { "Level" } th.num { "Power" } th.num { "Gold" } th { "Barrow King" } th { "Seen" } th {} } }
                    tbody {
                        @for r in &rows {
                            tr {
                                td { a href=(page.path(&format!("rpg?view=player&player={}", r.user))) { (r.name) } }
                                td { (r.patron.map(|g| g.name()).unwrap_or("None yet")) }
                                td.num { (r.level) }
                                td.num { (r.power) }
                                td.num { (thousands(r.gold)) }
                                td { @if r.boss { (ui::badge("Beaten", Tone::Ok)) } @else { span.muted { "Not yet" } } }
                                td.muted { (ago(r.last_seen)) }
                                td {
                                    @if page.can_edit() {
                                        div.row-actions {
                                            form.inline method="post" action=(page.path(&format!("rpg/player/{}/grant", r.user))) {
                                                input type="number" name="gold" value="500" min="1" max="100000" style="width:90px" aria-label="Gold to grant";
                                                button.btn.small type="submit" { "Grant gold" }
                                            }
                                            (ui::action_button(&page.path(&format!("rpg/player/{}/reset", r.user)), "Reset", Tone::Bad, true))
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }))
    })
}

fn economy_view(ctx: &Ctx, guild: u64) -> Result<Markup> {
    let since = now_ms() - 7 * 86_400_000;
    let (by_reason, by_item) = ctx.db.with(|c| {
        let mut st = c.prepare(
            "SELECT reason, SUM(delta) FROM rpg_ledger WHERE guild_id = ?1 AND at >= ?2 AND what = 'gold' GROUP BY reason ORDER BY SUM(delta) DESC",
        )?;
        let by_reason: Vec<(String, i64)> = st.query_map(params![guild as i64, since], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        let mut st = c.prepare(
            "SELECT what, SUM(CASE WHEN delta > 0 THEN delta ELSE 0 END), SUM(CASE WHEN delta < 0 THEN -delta ELSE 0 END)
             FROM rpg_ledger WHERE guild_id = ?1 AND at >= ?2 AND what != 'gold' GROUP BY what ORDER BY 2 DESC",
        )?;
        let by_item: Vec<(String, i64, i64)> = st.query_map(params![guild as i64, since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
        Ok((by_reason, by_item))
    })?;
    let gold_in: i64 = by_reason.iter().filter(|r| r.1 > 0).map(|r| r.1).sum();
    let gold_out: i64 = -by_reason
        .iter()
        .filter(|r| r.1 < 0)
        .map(|r| r.1)
        .sum::<i64>();
    let mats: Vec<&(String, i64, i64)> = by_item
        .iter()
        .filter(|(id, ..)| data::item(id).is_some_and(|i| i.kind == data::Kind::Material))
        .collect();
    Ok(html! {
        (ui::readings(&[
            ("gold earned, 7 days", thousands(gold_in)),
            ("gold spent, 7 days", thousands(gold_out)),
            ("spent for every 100 earned", if gold_in > 0 { (gold_out * 100 / gold_in).to_string() } else { "–".into() }),
        ]))
        (ui::section("Gold by reason", Some("Where gold came from and went in the last 7 days. If earned keeps running far ahead of spent, prices or sinks need raising."), html! {
            @if by_reason.is_empty() { (ui::empty(Icon::Chart, "Nothing yet", "Gold movements show here once people play.")) } @else {
                div.card { table.table { tbody { @for (r, n) in &by_reason { tr { td { (r) } td.num { @if *n > 0 { "+" } (thousands(*n)) } } } } } }
            }
        }))
        (ui::section("Materials", Some("Found against used (forged, offered, salvaged) in the last 7 days. A material found far more than it's used is piling up."), html! {
            @if mats.is_empty() { (ui::empty(Icon::Chart, "Nothing yet", "Materials show here once expeditions come back.")) } @else {
                div.card { table.table {
                    thead { tr { th { "Material" } th.num { "Found" } th.num { "Used" } th.num { "Used share" } } }
                    tbody { @for (id, got, used) in &mats { tr {
                        td { (data::item(id).map(|i| i.plural).unwrap_or(id)) }
                        td.num { (thousands(*got)) } td.num { (thousands(*used)) }
                        td.num { @if *got > 0 { (used * 100 / got) "%" } @else { "–" } }
                    } } }
                } }
            }
        }))
    })
}

fn screens_view(
    ctx: &Ctx,
    page: &Page,
    guild: u64,
    player: Option<u64>,
    focus: Option<&str>,
) -> Result<Markup> {
    let rows = players(ctx, guild)?;
    let pick = player.or_else(|| rows.first().map(|r| r.user));
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let names = [
        "hub",
        "map",
        "region_wolfwood",
        "guild",
        "guildfind",
        "guildmanage",
        "titan",
        "raid",
        "dungeons",
        "quests",
        "market",
        "lab",
        "combat",
        "hero",
        "questlog",
        "skagi",
        "saga",
        "bounties",
        "goals",
        "exp",
        "hunt",
        "forge",
        "pack",
        "temple",
        "patron",
        "help",
    ];
    let labels = [
        "Overview",
        "Map",
        "Region",
        "Guild",
        "Find a guild",
        "Manage guild",
        "Titan",
        "Raid",
        "Dungeons",
        "Side quests",
        "Market",
        "Labyrinth",
        "Combat",
        "Character",
        "Quests",
        "Skagi",
        "Saga",
        "Bounties",
        "Goals",
        "Expedition",
        "Hunt or fight",
        "Forge",
        "Pack",
        "Temple",
        "Patron",
        "How to play",
    ];
    let mut shots: Vec<(String, Markup)> = Vec::new();
    if let Some(u) = pick {
        let only: Vec<usize> = match focus {
            Some(f) => names.iter().position(|n| *n == f).into_iter().collect(),
            None => (0..names.len()).collect(),
        };
        for i in only {
            let sc = store::peek(&ctx.db, guild, u, |g| {
                let w = game::World::load(g, s.rates())?;
                game::view(ctx, g, &w, names[i], None)
            })?;
            if let Some(sc) = sc {
                shots.push((labels[i].to_string(), sc.html()));
            }
        }
    }
    shots.push((
        "A new player's first /rpg".into(),
        game::patron_choice(0, "Mira").html(),
    ));
    Ok(html! {
        (ui::section("Screens", Some("Exactly what the player sees in Discord right now, read without changing anything. Custom emojis show once the bot has uploaded them."), html! {
            @if !rows.is_empty() {
                form.inline method="get" action=(page.path("rpg")) {
                    input type="hidden" name="view" value="screens";
                    select name="player" aria-label="Player" onchange="this.form.submit()" {
                        @for r in &rows { option value=(r.user) selected[Some(r.user) == pick] { (r.name) " · level " (r.level) } }
                    }
                }
            }
            div.rpg-shots {
                @for (label, m) in &shots { div.rpg-shot { h3 { (label) } (m) } }
            }
        }))
    })
}

fn settings_view(ctx: &Ctx, page: &Page, guild: u64, s: &Settings) -> Markup {
    ui::settings_form(
        page,
        &page.path("rpg/settings"),
        html! {
            (ui::card(html! {
                @for (key, label, what) in store::Modules::LIST {
                    (ui::field_toggle(&format!("mod_{key}"), label, Some(what), s.modules.get(key)))
                }
            }))
            (ui::card(html! {
                (ui::field_channel(ctx, guild, "channel", "RPG channel", Some("Where /rpg works and hubs live. Leave empty to allow any channel."), s.channel))
                (ui::field_toggle("test_only", "Closed test", Some("Only testers (and anyone with Manage Server) can play."), s.test_only))
                (ui::field_role(ctx, guild, "tester_role", "Tester role", Some("Members with this role can play during the test."), s.tester_role))
                (ui::field_toggle("announce", "Announce kills", Some("Post in the channel when someone beats an elite or the realm boss for the first time."), s.announce))
                (ui::field_number("announce_minutes", "Delete them after", Some("So the channel doesn't fill up. 0 keeps them."), s.announce_minutes, 0, crate::MAX_ANNOUNCE_MINUTES, Some("minutes")))
            }))
            (ui::card(html! {
                (ui::field_number("xp_rate", "XP rate", Some("100% is the designed pace."), s.xp_rate, 10, 1000, Some("%")))
                (ui::field_number("loot_rate", "Loot rate", Some("Materials from expeditions."), s.loot_rate, 10, 1000, Some("%")))
                (ui::field_number("gold_rate", "Gold rate", Some("Gold from expeditions and hunts."), s.gold_rate, 10, 1000, Some("%")))
                (ui::field_number("titan_every_hours", "A Titan every", Some("Hours between Titans. 0: only when you raise one from the Test tab."), s.titan_every_hours, 0, 720, Some("hours")))
                (ui::field_number("titan_hours", "Titans stay for", None, s.titan_hours, 1, 168, Some("hours")))
                (ui::field_select("speed", "Test speed", Some("Expeditions, healing and Vigor run this many times faster, for testing. A 4-hour expedition takes 4 minutes at ×60. Set it back to ×1 before opening to everyone."),
                    &[("1".into(), "×1, real time".into()), ("10".into(), "×10".into()), ("60".into(), "×60".into()), ("240".into(), "×240".into())], &s.speed.to_string()))
            }))
        },
    )
}

async fn save(
    State(ctx): State<Ctx>,
    user: User,
    Path(guild): Path<u64>,
    form: FormData,
) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let mut s = (*ctx.settings::<Settings>(guild, KEY)?).clone();
    s.channel = form.channel(&ctx, guild, "channel");
    s.test_only = form.bool("test_only");
    s.tester_role = form.role(&ctx, guild, "tester_role");
    s.announce = form.bool("announce");
    s.announce_minutes = form.int_in("announce_minutes", 0, crate::MAX_ANNOUNCE_MINUTES, 30);
    s.xp_rate = form.int_in("xp_rate", 10, 1000, 100);
    s.loot_rate = form.int_in("loot_rate", 10, 1000, 100);
    s.gold_rate = form.int_in("gold_rate", 10, 1000, 100);
    s.speed = form.int_in("speed", 1, 600, 1);
    for (key, ..) in store::Modules::LIST {
        s.modules.set(key, form.bool(&format!("mod_{key}")));
    }
    store::set_modules(guild, s.modules);
    s.titan_every_hours = form.int_in("titan_every_hours", 0, 720, 72);
    s.titan_hours = form.int_in("titan_hours", 1, 168, 48);
    store::set_speed(guild, s.speed);
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed RPG settings");
    Ok(saved(&page.path("rpg?view=settings")))
}

fn player_view(ctx: &Ctx, page: &Page, guild: u64, player: Option<u64>) -> Result<Markup> {
    let Some(u) = player else {
        return Ok(ui::empty(
            Icon::Star,
            "Pick a player",
            "Open a player from the Players tab.",
        ));
    };
    let Some(p) = store::get(&ctx.db, guild, u)? else {
        return Ok(ui::empty(
            Icon::Star,
            "No such player",
            "They may have been reset.",
        ));
    };
    let tool = page.path(&format!("rpg/player/{u}/tool"));
    let items: Vec<(String, String)> = data::ITEMS
        .iter()
        .map(|i| (i.id.to_string(), i.plural.to_string()))
        .collect();
    let chapters: Vec<(String, String)> = crate::quests::CHAPTERS
        .iter()
        .enumerate()
        .map(|(i, c)| (i.to_string(), format!("{}. {}", i + 1, c.title)))
        .collect();
    let form = |action: &str, label: &str, fields: Markup| {
        html! {
            form.inline method="post" action=(tool) {
                input type="hidden" name="action" value=(action);
                (fields)
                button.btn.small type="submit" disabled[!page.can_edit()] { (label) }
            }
        }
    };
    Ok(html! {
        (ui::readings(&[
            ("level", p.level.to_string()),
            ("Power", p.power().to_string()),
            ("smith rank", p.smith_rank().to_string()),
            ("gold", thousands(p.gold)),
            ("saga chapter", format!("{} of {}", (p.saga + 1).min(crate::quests::CHAPTERS.len()), crate::quests::CHAPTERS.len())),
        ]))
        (ui::section(&format!("{} · tools", p.name), Some("For testing. Every change is logged in the dashboard's activity log."), html! {
            div.card {
                div.row { div.row-text { label { "Level" } p.help { "Sets the level; gear stays." } } div.row-control {
                    (form("level", "Set level", html! { input type="number" name="level" value=(p.level) min="1" max=(data::LEVEL_CAP) style="width:80px" aria-label="Level"; }))
                } }
                div.row { div.row-text { label { "Smith rank" } p.help { "Sets their forge rank: odds, how far upgrades go, which recipes open." } } div.row-control {
                    (form("smith", "Set rank", html! { input type="number" name="rank" value=(p.smith_rank()) min="1" max=(data::SMITH_CAP) style="width:80px" aria-label="Smith rank"; }))
                } }
                div.row { div.row-text { label { "Give items" } } div.row-control {
                    (form("item", "Give", html! {
                        (ui::select("item", &items, "potion"))
                        input type="number" name="qty" value="5" min="1" max="999" style="width:80px" aria-label="How many";
                    }))
                } }
                div.row { div.row-text { label { "Expedition" } p.help { "Bring their expedition back now." } } div.row-control {
                    (form("finish", "Finish now", html! {}))
                } }
                div.row { div.row-text { label { "Ready for the boss" } p.help { "Level cap, the best Midgard gear at +4, all four facts, three potions and full health." } } div.row-control {
                    (form("boss", "Make boss-ready", html! {}))
                } }
                div.row { div.row-text { label { "Saga" } p.help { "Jump to a chapter (its goals start from zero)." } } div.row-control {
                    (form("chapter", "Go to chapter", html! { (ui::select("chapter", &chapters, &p.saga.to_string())) }))
                } }
                div.row { div.row-text { label { "Health" } } div.row-control { (form("heal", "Heal fully", html! {})) } }
            }
        }))
        p { a href=(page.path(&format!("rpg?view=screens&player={u}"))) { "See their screens →" } }
    })
}

async fn tool(
    State(ctx): State<Ctx>,
    user: User,
    Path((guild, player)): Path<(u64, u64)>,
    form: FormData,
) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let action = form.string("action");
    let mut what = String::new();
    store::run(&ctx.db, guild, player, None, |g| {
        match action.as_str() {
            "level" => {
                let lvl = form.int_in("level", 1, data::LEVEL_CAP as i64, 1) as u32;
                g.tx.execute("UPDATE rpg_players SET level = ?3, xp = 0 WHERE guild_id = ?1 AND user_id = ?2", params![guild as i64, player as i64, lvl as i64])?;
                what = format!("set level to {lvl}");
            }
            "smith" => {
                let r = form.int_in("rank", 1, data::SMITH_CAP as i64, 1) as usize;
                let xp = data::SMITH_AT[r] - g.p.smith_xp;
                g.smith(xp)?;
                what = format!("set their smith rank to {r}");
            }
            "item" => {
                let id = form.string("item");
                let n = form.int_in("qty", 1, 999, 1);
                if data::item(&id).is_some() {
                    g.item(&id, n, "given from the dashboard")?;
                    what = format!("gave {}", data::count(&id, n));
                }
            }
            "finish" => {
                g.tx.execute(
                    "UPDATE rpg_expeditions SET ends_at = 0 WHERE guild_id = ?1 AND user_id = ?2",
                    params![guild as i64, player as i64],
                )?;
                what = "finished their expedition".into();
            }
            "boss" => {
                g.tx.execute("UPDATE rpg_players SET level = ?3, xp = 0 WHERE guild_id = ?1 AND user_id = ?2", params![guild as i64, player as i64, data::LEVEL_CAP as i64])?;
                g.p.level = data::LEVEL_CAP;
                for id in ["barrowblade", "mantle", "ravencharm"] {
                    let row = g.add_gear(data::gear(id).unwrap(), true)?;
                    for _ in 0..4 {
                        g.upgrade(row)?;
                    }
                }
                for f in data::FACTS {
                    g.learn(f)?;
                }
                g.item("potion", 3, "given from the dashboard")?;
                let max = g.p.max_hp();
                g.set_hp(max)?;
                what = "made them boss-ready".into();
            }
            "chapter" => {
                let ch =
                    form.int_in("chapter", 0, crate::quests::CHAPTERS.len() as i64 - 1, 0) as usize;
                g.p.saga = ch;
                g.p.saga_progress = Vec::new();
                g.save_saga()?;
                what = format!("moved their saga to chapter {}", ch + 1);
            }
            "heal" => {
                let max = g.p.max_hp();
                g.set_hp(max)?;
                what = "healed them".into();
            }
            _ => {}
        }
        g.bump()
    })?;
    if !what.is_empty() {
        let name = store::get(&ctx.db, guild, player)?
            .map(|p| p.name)
            .unwrap_or_default();
        ctx.audit(
            &page.user,
            Some(guild),
            KEY,
            format!("RPG test tools: {what} for {name}"),
        );
    }
    Ok(Redirect::to(&page.path(&format!("rpg?view=player&player={player}"))).into_response())
}

async fn reset(
    State(ctx): State<Ctx>,
    user: User,
    Path((guild, player)): Path<(u64, u64)>,
) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let name = store::get(&ctx.db, guild, player)?
        .map(|p| p.name)
        .unwrap_or_default();
    game::reset_player(&ctx.db, guild, player)?;
    ctx.audit(
        &page.user,
        Some(guild),
        KEY,
        format!("Reset {name}'s RPG character"),
    );
    Ok(Redirect::to(&page.path("rpg?view=players")).into_response())
}

async fn grant(
    State(ctx): State<Ctx>,
    user: User,
    Path((guild, player)): Path<(u64, u64)>,
    form: FormData,
) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let n = form.int_in("gold", 1, 100_000, 500);
    let name = store::run(&ctx.db, guild, player, None, |g| {
        g.gold(n, "granted from the dashboard")?;
        g.bump()?;
        Ok(g.p.name.clone())
    })?;
    if let Some(name) = name {
        ctx.audit(
            &page.user,
            Some(guild),
            KEY,
            format!("Granted {n} RPG gold to {name}"),
        );
    }
    Ok(Redirect::to(&page.path("rpg?view=players")).into_response())
}

/// Sample players for `bot preview`: a newcomer, one mid-way with an expedition out, one in a
/// boss fight, and one past the boss with a lesser vow.
pub fn seed(ctx: &Ctx, guild: u64) -> Result<()> {
    let s = Settings {
        test_only: true,
        tester_role: Some(guild + 100),
        ..Settings::default()
    };
    ctx.save_settings(guild, KEY, &s)?;
    let m = bot_core::preview::MEMBERS;
    let plan = |region: &str, danger: data::Danger, hours: u32| crate::engine::expedition::Plan {
        region: region.into(),
        danger,
        hours,
        supply: Some("rations".into()),
    };
    // A newcomer.
    store::create(&ctx.db, guild, m[0].0, m[0].1, God::Athena)?;
    // Mid-way, out on an expedition.
    store::create(&ctx.db, guild, m[1].0, m[1].1, God::Thor)?;
    store::run(&ctx.db, guild, m[1].0, None, |g| {
        g.xp(9000)?;
        g.gold(4700, "seed")?;
        for (id, n) in [
            ("iron", 28),
            ("yew", 11),
            ("pelt", 9),
            ("herbs", 14),
            ("fish", 12),
            ("fang", 3),
            ("frost", 2),
            ("amber", 3),
        ] {
            g.item(id, n, "seed")?;
        }
        use data::{Rarity, Trait};
        g.add_piece(
            data::gear("warhammer").unwrap(),
            Rarity::Rare,
            vec![(Trait::Fierce, 5), (Trait::Keen, 4)],
            true,
            "seed",
        )?;
        g.add_piece(
            data::gear("mail").unwrap(),
            Rarity::Fine,
            vec![(Trait::Sturdy, 6)],
            true,
            "seed",
        )?;
        g.add_gear(data::gear("ambercharm").unwrap(), true)?;
        g.add_piece(
            data::gear("handaxe").unwrap(),
            Rarity::Fine,
            vec![(Trait::Flame, 11)],
            false,
            "seed",
        )?;
        g.item("thread", 4, "seed")?;
        g.smith(400)?;
        g.set_tutorial(1 | 2 | 4 | 32)?;
        g.tribute(God::Thor, 1600)?;
        g.tribute(God::Athena, 400)?;
        g.learn("slam")?;
        g.learn("weakness")?;
        g.p.saga = 5;
        g.p.saga_choices.insert("singer".into(), "spare".into());
        g.save_saga()?;
        g.add_title("Friend of Nixies")?;
        g.set_event(Some("merchant"))?;
        let now = g.p.now;
        g.start_expedition(store::Expedition {
            plan: plan("barrow", data::Danger::Hard, 4),
            power: g.p.power(),
            scouting: 5,
            started_at: now - 3_600_000 * 3,
            ends_at: now + 4_320_000,
            seed: 3,
        })?;
        g.vigor(20.0)?;
        Ok(())
    })?;
    // In a boss fight.
    store::create(&ctx.db, guild, m[2].0, m[2].1, God::Hades)?;
    store::run(&ctx.db, guild, m[2].0, None, |g| {
        g.xp(30000)?;
        g.gold(1800, "seed")?;
        g.item("potion", 2, "seed")?;
        g.item("fireoil", 1, "seed")?;
        for id in ["barrowblade", "mantle", "ravencharm"] {
            g.add_gear(data::gear(id).unwrap(), true)?;
        }
        g.tribute(God::Hades, 2000)?;
        for f in data::FACTS {
            g.learn(f)?;
        }
        g.p.saga = 10;
        g.p.saga_choices.insert("singer".into(), "silence".into());
        g.p.saga_choices.insert("wolfmother".into(), "hunt".into());
        g.p.saga_choices.insert("volva".into(), "jarlsmen".into());
        g.save_saga()?;
        g.add_title("Wolf-bane")?;
        g.set_tutorial(crate::game::Tut::ALL)?;
        let w = g.p.week();
        let mut f =
            crate::engine::combat::Fight::boss(g.p.hero(), g.p.max_hp(), w, g.p.facts.clone(), 9)
                .with_ally(Some("jarlsmen"));
        for a in [
            crate::engine::combat::Action::Strike,
            crate::engine::combat::Action::Strike,
        ] {
            let _ = f.act(a, 2, (1, 0));
        }
        g.save_fight(Some(f))?;
        Ok(())
    })?;
    // Past the boss.
    store::create(&ctx.db, guild, m[3].0, m[3].1, God::Thor)?;
    store::run(&ctx.db, guild, m[3].0, None, |g| {
        g.xp(60000)?;
        g.gold(9000, "seed")?;
        for id in ["barrowblade", "mantle", "ravencharm"] {
            g.add_gear(data::gear(id).unwrap(), true)?;
        }
        g.tribute(God::Thor, 2500)?;
        g.tribute(God::Athena, 700)?;
        g.boss_won(31)?;
        g.item("shard", 3, "seed")?;
        g.set_lesser(Some(God::Athena))?;
        g.p.saga = crate::quests::CHAPTERS.len();
        g.p.saga_choices.insert("singer".into(), "spare".into());
        g.p.saga_choices.insert("wolfmother".into(), "offer".into());
        g.p.saga_choices
            .insert("volva".into(), "shieldmaiden".into());
        g.save_saga()?;
        for t in [
            "Friend of Nixies",
            "Wolf-friend",
            "Mound-breaker",
            "Thread-finder",
            "Grimtooth's Bane",
        ] {
            g.add_title(t)?;
        }
        g.set_title(Some("Mound-breaker"))?;
        g.set_tutorial(crate::game::Tut::ALL)?;
        let mut items = std::collections::BTreeMap::new();
        items.insert("frost".to_string(), 3);
        items.insert("thread".to_string(), 4);
        items.insert("iron".to_string(), 6);
        g.save_run(Some(crate::labyrinth::Run {
            floor: 8,
            doors: vec![
                crate::labyrinth::Door::Treasure,
                crate::labyrinth::Door::Fight,
                crate::labyrinth::Door::Hard,
            ],
            fighting: None,
            gold: 212,
            xp: 640,
            items,
            gear: vec![crate::labyrinth::Drop {
                gear: "fangnecklace".into(),
                rarity: data::Rarity::Rare,
                traits: vec![(data::Trait::Lucky, 6), (data::Trait::Vampiric, 3)],
            }],
            log: vec!["A Barrow Shade falls. Into your haul: 32 gold, 1 wolf pelt.".into()],
            seed: 5,
        }))?;
        g.lab_best(8)?;
        Ok(())
    })?;
    // A guild with two members, a Titan up, and a raid on.
    store::run(&ctx.db, guild, m[1].0, None, |g| {
        g.gold(2000, "seed")?;
        crate::guild::found(g, "Ravenhold", "RVN")?;
        Ok(())
    })?;
    let clan = store::get(&ctx.db, guild, m[1].0)?
        .and_then(|p| p.clan)
        .map(|c| c.id)
        .unwrap_or(0);
    store::run(&ctx.db, guild, m[3].0, None, |g| {
        crate::guild::join(g, clan).map(|_| ())
    })?;
    ctx.db.with(|c| {
        c.execute(
            "UPDATE rpg_clans SET xp = 2400, treasury = 5200, policy = 'invite', min_week = 100 WHERE id = ?1",
            params![clan],
        )?;
        Ok(())
    })?;
    // Someone asking to join, for the leader's Manage page.
    store::run(&ctx.db, guild, m[2].0, None, |g| {
        crate::guild::ask(g, clan).map(|_| ())
    })?;
    ctx.db.with(|c| {
        let id = crate::shared::spawn(c, guild, "titan", 0, "jormungandr", 180, 36000, 48).unwrap_or(0);
        c.execute("UPDATE rpg_shared SET hp = 21500 WHERE id = ?1", params![id])?;
        c.execute("INSERT INTO rpg_shared_dmg (shared_id, user_id, damage, attempts) VALUES (?1, ?2, 6400, 2), (?1, ?3, 5100, 3), (?1, ?4, 3000, 1)", params![id, m[3].0 as i64, m[1].0 as i64, m[2].0 as i64])?;
        let raid = crate::shared::spawn(c, guild, "raid", clan, "fafnir", 260, 21000, 72).unwrap_or(0);
        c.execute("UPDATE rpg_shared SET hp = 15800 WHERE id = ?1", params![raid])?;
        Ok(())
    })?;
    store::run(&ctx.db, guild, m[1].0, None, |g| {
        g.save_side("nets", 1, 0)?;
        g.save_side("drills", 0, 6)?;
        Ok(())
    })?;
    let _ = progress::week(now_ms());
    Ok(())
}

fn titan_block(ctx: &Ctx, page: &Page, guild: u64) -> Result<Markup> {
    let t = ctx
        .db
        .with(|c| Ok(crate::shared::active(c, guild, "titan", 0).unwrap_or(None)))?;
    let now = now_ms();
    Ok(html! {
        div.card {
            div.row {
                div.row-text {
                    @match &t {
                        Some(t) if !t.over(now) => { label { (t.name()) " is up" } p.help { (t.hp * 100 / t.max_hp.max(1)) "% left · " (crate::game::dur(t.ends - now)) " to go" } }
                        Some(t) => { label { "Last: " (t.name()) } p.help { @if t.killed_at.is_some() { "Brought down." } @else { "Got away." } } }
                        None => { label { "No Titan yet" } p.help { "The first rises a day after people start playing, if the schedule is on." } }
                    }
                }
                div.row-control {
                    @if page.can_edit() && t.as_ref().is_none_or(|t| t.over(now)) {
                        (ui::action_button(&page.path("rpg/titan"), "Raise a Titan now", Tone::Accent, false))
                    }
                }
            }
        }
    })
}

async fn titan_now(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let text = ctx.db.with(|c| {
        Ok(crate::shared::rise_titan(c, guild, s.titan_hours.clamp(1, 168)).unwrap_or(None))
    })?;
    if let Some(t) = text {
        if let Some(ch) = s.channel {
            let _ = ctx.say(ch, &t).await;
        }
        ctx.audit(&page.user, Some(guild), KEY, "Raised a Titan");
    }
    Ok(Redirect::to(&page.path("rpg")).into_response())
}
