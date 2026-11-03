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
        let mut st = c.prepare("SELECT user_id FROM rpg_players WHERE guild_id = ?1 ORDER BY last_seen DESC LIMIT 200")?;
        st.query_map(params![guild as i64], |r| Ok(r.get::<_, i64>(0)? as u64))?.collect()
    })?;
    let mut out = Vec::new();
    for u in ids {
        if let Some(p) = store::get(&ctx.db, guild, u)? {
            out.push(Row { user: u, name: p.name.clone(), patron: p.patron, level: p.level as i64, gold: p.gold, boss: p.boss_won, last_seen: last_seen(ctx, guild, u), power: p.power() });
        }
    }
    out.sort_by_key(|r| std::cmp::Reverse((r.power, r.level)));
    Ok(out)
}

fn last_seen(ctx: &Ctx, guild: u64, user: u64) -> i64 {
    ctx.db
        .with(|c| c.query_row("SELECT last_seen FROM rpg_players WHERE guild_id = ?1 AND user_id = ?2", params![guild as i64, user as i64], |r| r.get(0)))
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

async fn page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<Q>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let view = q.view.as_deref().unwrap_or("test");
    let tabs = ui::tabs(
        &[("Test", page.path("rpg")), ("Players", page.path("rpg?view=players")), ("Economy", page.path("rpg?view=economy")), ("Screens", page.path("rpg?view=screens")), ("Settings", page.path("rpg?view=settings"))],
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
        "settings" => settings_view(&ctx, &page, guild, &s),
        _ => test_view(&ctx, &page, guild, &s)?,
    };
    Ok(ui::layout(&ctx, &page, "RPG", None, html! { style { (PreEscaped(CSS)) } (tabs) (body) }).into_response())
}

fn test_view(ctx: &Ctx, page: &Page, guild: u64, s: &Settings) -> Result<Markup> {
    let week = store::week_now();
    let (total, goal, active, kills, players_n) = ctx.db.with(|c| {
        let o = store::offerings(c, guild, week).unwrap_or([0; 3]);
        let goal = store::goal(c, guild, week).unwrap_or(1);
        let active = store::active_players(c, guild, week).unwrap_or(0);
        let kills: i64 = c.query_row("SELECT COUNT(*) FROM rpg_kills WHERE guild_id = ?1", params![guild as i64], |r| r.get(0))?;
        let n: i64 = c.query_row("SELECT COUNT(*) FROM rpg_players WHERE guild_id = ?1", params![guild as i64], |r| r.get(0))?;
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
            Some(r) => format!("Members with the {} role, and anyone with Manage Server, can play.", ctx.roles(guild).into_iter().find(|x| x.0 == r).map(|x| x.1).unwrap_or_else(|| "tester".into())),
            None => "Only people with Manage Server can play until you pick a tester role in Settings.".into(),
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
        (ui::section("How fast the buttons answer", Some("From Discord's click arriving to the new screen being sent. Work is the bot's own time: loading the player, the rules, saving. Reply is Discord accepting the new screen."), clicks))
        @if page.can_edit() { p.muted.small { "Tip: open Screens to see exactly what any player sees right now." } }
    })
}

fn speed() -> Markup {
    let clicks = crate::clicks();
    if clicks.is_empty() {
        return ui::empty(Icon::Clock, "No clicks yet", "Timings appear here as people play. They're kept for the last 500 clicks.");
    }
    let mut work: Vec<i64> = clicks.iter().map(|c| c.work_us).collect();
    let mut reply: Vec<i64> = clicks.iter().map(|c| c.reply_ms).collect();
    let fmt_us = |us: i64| if us < 1000 { format!("{us} µs") } else { format!("{:.1} ms", us as f64 / 1000.0) };
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
        return Ok(ui::empty(Icon::Star, "No players yet", "Players appear here once they use /rpg and swear to a patron."));
    }
    Ok(html! {
        (ui::section("Players", Some("Strongest first. Reset deletes a character so they start again; grant adds gold for testing."), html! {
            div.card {
                table.table {
                    thead { tr { th { "Player" } th { "Patron" } th.num { "Level" } th.num { "Power" } th.num { "Gold" } th { "Barrow King" } th { "Seen" } th {} } }
                    tbody {
                        @for r in &rows {
                            tr {
                                td { a href=(page.path(&format!("rpg?view=screens&player={}", r.user))) { (r.name) } }
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
    let gold_out: i64 = -by_reason.iter().filter(|r| r.1 < 0).map(|r| r.1).sum::<i64>();
    let mats: Vec<&(String, i64, i64)> = by_item.iter().filter(|(id, ..)| data::item(id).is_some_and(|i| i.kind == data::Kind::Material)).collect();
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

fn screens_view(ctx: &Ctx, page: &Page, guild: u64, player: Option<u64>, focus: Option<&str>) -> Result<Markup> {
    let rows = players(ctx, guild)?;
    let pick = player.or_else(|| rows.first().map(|r| r.user));
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let names = ["hub", "goals", "exp", "hunt", "forge", "pack", "temple", "patron", "help"];
    let labels = ["Overview", "Goals", "Expedition", "Hunt or fight", "Forge", "Pack", "Temple", "Patron", "How to play"];
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
    shots.push(("A new player's first /rpg".into(), game::patron_choice(0, "Mira").html()));
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
    ui::settings_form(page, &page.path("rpg/settings"), html! {
        (ui::card(html! {
            (ui::field_channel(ctx, guild, "channel", "RPG channel", Some("Where /rpg works and hubs live. Leave empty to allow any channel."), s.channel))
            (ui::field_toggle("test_only", "Closed test", Some("Only testers (and anyone with Manage Server) can play."), s.test_only))
            (ui::field_role(ctx, guild, "tester_role", "Tester role", Some("Members with this role can play during the test."), s.tester_role))
            (ui::field_toggle("announce", "Announce boss kills", Some("Post in the channel when someone beats the Barrow King for the first time."), s.announce))
        }))
        (ui::card(html! {
            (ui::field_number("xp_rate", "XP rate", Some("100% is the designed pace."), s.xp_rate, 10, 1000, Some("%")))
            (ui::field_number("loot_rate", "Loot rate", Some("Materials from expeditions."), s.loot_rate, 10, 1000, Some("%")))
            (ui::field_number("gold_rate", "Gold rate", Some("Gold from expeditions and hunts."), s.gold_rate, 10, 1000, Some("%")))
        }))
    })
}

async fn save(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let mut s = (*ctx.settings::<Settings>(guild, KEY)?).clone();
    s.channel = form.channel(&ctx, guild, "channel");
    s.test_only = form.bool("test_only");
    s.tester_role = form.role(&ctx, guild, "tester_role");
    s.announce = form.bool("announce");
    s.xp_rate = form.int_in("xp_rate", 10, 1000, 100);
    s.loot_rate = form.int_in("loot_rate", 10, 1000, 100);
    s.gold_rate = form.int_in("gold_rate", 10, 1000, 100);
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed RPG settings");
    Ok(saved(&page.path("rpg?view=settings")))
}

async fn reset(State(ctx): State<Ctx>, user: User, Path((guild, player)): Path<(u64, u64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let name = store::get(&ctx.db, guild, player)?.map(|p| p.name).unwrap_or_default();
    game::reset_player(&ctx.db, guild, player)?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Reset {name}'s RPG character"));
    Ok(Redirect::to(&page.path("rpg?view=players")).into_response())
}

async fn grant(State(ctx): State<Ctx>, user: User, Path((guild, player)): Path<(u64, u64)>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let n = form.int_in("gold", 1, 100_000, 500);
    let name = store::run(&ctx.db, guild, player, None, |g| {
        g.gold(n, "granted from the dashboard")?;
        g.bump()?;
        Ok(g.p.name.clone())
    })?;
    if let Some(name) = name {
        ctx.audit(&page.user, Some(guild), KEY, format!("Granted {n} RPG gold to {name}"));
    }
    Ok(Redirect::to(&page.path("rpg?view=players")).into_response())
}

/// Sample players for `bot preview`: a newcomer, one mid-way with an expedition out, one in a
/// boss fight, and one past the boss with a lesser vow.
pub fn seed(ctx: &Ctx, guild: u64) -> Result<()> {
    let s = Settings { test_only: true, tester_role: Some(guild + 100), ..Settings::default() };
    ctx.save_settings(guild, KEY, &s)?;
    let m = bot_core::preview::MEMBERS;
    let plan = |region: &str, danger: data::Danger, hours: u32| crate::engine::expedition::Plan { region: region.into(), danger, hours, supply: Some("rations".into()) };
    // A newcomer.
    store::create(&ctx.db, guild, m[0].0, m[0].1, God::Athena)?;
    // Mid-way, out on an expedition.
    store::create(&ctx.db, guild, m[1].0, m[1].1, God::Thor)?;
    store::run(&ctx.db, guild, m[1].0, None, |g| {
        g.xp(9000)?;
        g.gold(4700, "seed")?;
        for (id, n) in [("iron", 28), ("yew", 11), ("pelt", 9), ("herbs", 14), ("fish", 12), ("fang", 3), ("frost", 2), ("amber", 3)] {
            g.item(id, n, "seed")?;
        }
        for id in ["warhammer", "mail", "ambercharm"] {
            g.add_gear(data::gear(id).unwrap(), true)?;
        }
        g.tribute(God::Thor, 1600)?;
        g.tribute(God::Athena, 400)?;
        g.learn("slam")?;
        g.learn("weakness")?;
        let now = g.p.now;
        g.start_expedition(store::Expedition { plan: plan("barrow", data::Danger::Hard, 4), power: g.p.power(), scouting: 5, started_at: now - 3_600_000 * 3, ends_at: now + 4_320_000, seed: 3 })?;
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
        let w = g.p.week();
        let mut f = crate::engine::combat::Fight::boss(g.p.hero(), g.p.max_hp(), w, g.p.facts.clone(), 9);
        for a in [crate::engine::combat::Action::Strike, crate::engine::combat::Action::Strike] {
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
        Ok(())
    })?;
    let _ = progress::week(now_ms());
    Ok(())
}
