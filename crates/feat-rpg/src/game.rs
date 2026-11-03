//! Every screen and every action. A click arrives as a route (`screen`, `action`, `arg`), runs
//! inside one transaction on the player, and returns the screen to show next.
//!
//! Buttons carry the player's state version: an action from a stale message (or a double
//! click) is refused and the current state shown instead, so nothing happens twice.

use bot_core::anyhow::Result;
use bot_core::rusqlite::params;
use bot_core::{Ctx, now_ms};

use crate::banner;
use crate::data::{self, Danger, God, Kind, Move, Skill, Slot};
use crate::emoji::get as e;
use crate::engine::combat::{Action, Element, End, Fight, SkillId};
use crate::engine::expedition::{self, Plan, Rates, Report};
use crate::engine::progress;
use crate::store::{self, Expedition, Game, Player};
use crate::ui::{Btn, Opt, Screen};

/// What a click produced: the screen, and a public line for the channel when something worth
/// telling everyone happened.
pub struct Outcome {
    pub screen: Screen,
    pub announce: Option<String>,
}

/// Where a click goes, read from its custom id `rpg:{owner}:{screen}:{action}:{arg}:{version}`.
#[derive(Debug, Clone)]
pub struct Route {
    pub owner: u64,
    pub screen: String,
    pub action: String,
    pub arg: String,
    pub version: i64,
}

impl Route {
    pub fn parse(id: &str) -> Option<Route> {
        let mut p = id.split(':');
        if p.next()? != "rpg" {
            return None;
        }
        Some(Route {
            owner: p.next()?.parse().ok()?,
            screen: p.next()?.to_owned(),
            action: p.next().unwrap_or("").to_owned(),
            arg: p.next().unwrap_or("").to_owned(),
            version: p.next().and_then(|v| v.parse().ok()).unwrap_or(-1),
        })
    }
}

/// Builds custom ids for one player at one state version.
#[derive(Clone, Copy)]
pub struct Ids {
    pub owner: u64,
    pub version: i64,
}

impl Ids {
    pub fn go(&self, screen: &str) -> String {
        format!("rpg:{}:{screen}:::0", self.owner)
    }
    pub fn act(&self, screen: &str, action: &str, arg: &str) -> String {
        format!("rpg:{}:{screen}:{action}:{arg}:{}", self.owner, self.version)
    }
}

/// Server-wide numbers the screens show.
pub struct World {
    pub week: i64,
    pub offerings: [i64; 3],
    pub goal: i64,
    pub blessing: Option<God>,
    pub rates: Rates,
}

impl World {
    pub fn load(g: &Game, rates: Rates) -> Result<World> {
        let week = g.p.week();
        Ok(World {
            week,
            offerings: store::offerings(&g.tx, g.p.guild, week)?,
            goal: store::goal(&g.tx, g.p.guild, week)?,
            blessing: store::blessing(&g.tx, g.p.guild, week)?,
            rates,
        })
    }
    pub fn total(&self) -> i64 {
        self.offerings.iter().sum()
    }
    pub fn pct(&self) -> i64 {
        (self.total() * 100 / self.goal.max(1)).min(999)
    }
    /// Rates with this week's blessing.
    fn xp_rate(&self) -> i64 {
        self.rates.xp * if self.blessing == Some(God::Athena) { 110 } else { 100 } / 100
    }
    fn expedition_rates(&self) -> Rates {
        let loot = if self.blessing == Some(God::Hades) { 110 } else { 100 };
        Rates { xp: self.xp_rate(), loot: self.rates.loot * loot / 100, gold: self.rates.gold * loot / 100 }
    }
    fn hunt_mult(&self) -> i64 {
        if self.blessing == Some(God::Thor) { 110 } else { 100 }
    }
}

fn dur(ms: i64) -> String {
    let m = (ms.max(0) + 59_999) / 60_000;
    if m >= 60 { if m % 60 == 0 { format!("{}h", m / 60) } else { format!("{}h {}m", m / 60, m % 60) } } else { format!("{m}m") }
}

fn items_line(items: &[(String, i64)]) -> String {
    items.iter().filter(|(_, n)| *n > 0).map(|(id, n)| format!("{} {}", e(id), data::count(id, *n))).collect::<Vec<_>>().join("  ·  ")
}

fn title(md: &mut Screen, icon: &str, text: &str, meta: &str) {
    md.text(format!("### {} {text}", e(icon)));
    if !meta.is_empty() {
        md.text(format!("-# {meta}"));
    }
}

fn back(ids: &Ids) -> Btn {
    Btn::new(ids.go("hub"), "← Back")
}

fn back_to(ids: &Ids, screen: &str, label: &str) -> Btn {
    Btn::new(ids.go(screen), format!("← {label}"))
}

// ---------------------------------------------------------------------------------------------
// Screens
// ---------------------------------------------------------------------------------------------

/// The first thing a new player sees.
pub fn patron_choice(owner: u64, name: &str) -> Screen {
    let ids = Ids { owner, version: 0 };
    let mut s = Screen::default();
    title(&mut s, "offering", "Swear to a god", &format!("Welcome, {name}"));
    s.text("Olympus and Asgard have gone quiet. Between them, the realms fill with monsters and old gold. No mortal walks them alone. Choose who you serve.");
    for g in data::GODS {
        s.section(
            format!("{} **{}**\n-# {}", e(g.id()), g.name(), g.pitch()),
            Btn::new(ids.act("patron", "swear", g.id()), "Swear").emoji(g.id()),
        );
    }
    s.text("-# Your patron gives you gear to start with. You can change patron later at the temple, for a price.");
    s
}

pub fn hub(ctx: &Ctx, p: &Player, w: &World) -> Screen {
    let ids = Ids { owner: p.user, version: p.version };
    let mut s = Screen::default();
    let xp10 = (p.xp * 10 / p.xp_needed().max(1)).clamp(0, 10) as u32;
    s.banner = Some(banner::Spec { patron: p.main(), name: p.name.clone(), level: p.level, power: p.power(), xp10 }.url(ctx));
    s.text("-# Right now");
    let mut primary_used = false;
    match &p.expedition {
        Some(x) if p.now >= x.ends_at => {
            s.section(
                format!("{} **Back from {}**\n-# {} · {}h · ready to collect", e("expedition"), x.plan.region().name, x.plan.danger.name(), x.plan.hours),
                Btn::new(ids.act("exp", "collect", ""), "Collect").emoji("expedition").primary(),
            );
            primary_used = true;
        }
        Some(x) => {
            let f = expedition::forecast(&x.plan, x.power, x.scouting, w.expedition_rates());
            let top: Vec<(String, i64)> = f.items.into_iter().filter(|(_, n)| *n > 0).take(3).collect();
            s.text(format!(
                "{} **{} · {}** · back in {}\n-# Expected: {}",
                e("expedition"),
                x.plan.region().name,
                x.plan.danger.name(),
                dur(x.ends_at - p.now),
                top.iter().map(|(id, n)| data::count(id, *n)).collect::<Vec<_>>().join(", ")
            ));
        }
        None => {
            s.section(format!("{} **No expedition out**\n-# Send one before you go: it works while you're away.", e("expedition")), Btn::new(ids.go("exp"), "Plan one").emoji("expedition"));
        }
    }
    if let Some(f) = &p.fight {
        let what = if f.kind == crate::engine::combat::Kind::Boss { "Fighting the Barrow King" } else { "In a fight" };
        s.section(format!("{} **{what}**\n-# {} · turn {}", e("hunt"), f.foe.name, f.turn), Btn::new(ids.go(if f.kind == crate::engine::combat::Kind::Boss { "boss" } else { "hunt" }), "Continue").emoji("hunt"));
    } else {
        let wins = (p.vigor() / data::VIGOR_PER_WIN as f64).floor() as i64;
        let hp = p.hp();
        let max = p.max_hp();
        let line = if hp < max / 2 {
            format!("{} **Resting** · {hp} / {max} HP\n-# Full in {}. A potion works too.", e("hp"), dur(progress::minutes_to_full(hp, max) * 60_000))
        } else if wins > 0 {
            format!("{} **Free to hunt**\n-# Vigor doubles your next {wins} win{}", e("hunt"), if wins == 1 { "" } else { "s" })
        } else {
            format!("{} **Free to hunt**\n-# Vigor builds back while you're away", e("hunt"))
        };
        s.text(line);
    }
    s.gap();
    s.text(format!(
        "{} {} gold   ·   {} Vigor {:.0}%   ·   {} Offering {}%",
        e("gold"),
        thousands(p.gold),
        e("vigor"),
        p.vigor(),
        e("offering"),
        w.pct()
    ));
    let hunt = Btn::new(ids.go("hunt"), "Hunt").emoji("hunt");
    s.buttons(vec![
        if primary_used { hunt } else { hunt.primary() },
        Btn::new(ids.go("exp"), "Expedition").emoji("expedition"),
        Btn::new(ids.go("forge"), "Forge").emoji("forge"),
        Btn::new(ids.go("goals"), "Goals").emoji("goals"),
    ]);
    s.select(
        ids.act("hub", "nav", ""),
        "Pack, temple, patron and skills, how to play…",
        vec![
            Opt::new("Pack", "pack").emoji("pack").desc("Your gear, materials and supplies"),
            Opt::new("Temple", "temple").emoji("temple").desc("Altars, bonds and the server's offering"),
            Opt::new("Patron and skills", "patron").emoji(p.main().id()).desc("Your skills, lesser vow and patron"),
            Opt::new("How to play", "help").emoji("help"),
        ],
    );
    s
}

pub fn thousands(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

fn goals(p: &Player, w: &World, givers: &[(u64, String, i64)]) -> Screen {
    let ids = Ids { owner: p.user, version: p.version };
    let mut s = Screen::default();
    title(&mut s, "goals", "Goals", &format!("Midgard · week {}", w.week.rem_euclid(52) + 1));
    s.text("-# Realm boss");
    let power = p.power();
    let ready = power >= data::BOSS_POWER;
    let mut boss = format!(
        "{} **{}**  ·  needs {} Power\n{}",
        e("boss"),
        data::BOSS_NAME,
        data::BOSS_POWER,
        if p.boss_won {
            format!("{} Beaten. Heroic expeditions and your lesser vow are open.", e("check"))
        } else if ready {
            format!("You have **{power}**. You're ready.")
        } else {
            format!("You have **{power}**, {} to go", data::BOSS_POWER - power)
        }
    );
    for f in data::FACTS {
        if p.facts.iter().any(|k| k == f) {
            boss.push_str(&format!("\n-# {} {}", e("check"), data::fact_text(f, w.week)));
        }
    }
    let unknown = data::FACTS.len() - p.facts.len();
    if unknown > 0 {
        boss.push_str(&format!("\n-# {} {unknown} more unknown · expeditions to the Barrow Fields scout the most", e("unknown")));
    }
    s.text(boss);
    s.gap();
    s.text("-# Server offering");
    let lead = data::GODS[w.offerings.iter().enumerate().max_by_key(|(_, t)| **t).map(|(i, _)| i).unwrap_or(0)];
    let (bname, bdesc) = lead.blessing();
    s.text(format!(
        "{} **{} of {} tribute this week** ({}%)\n-# At 100%, everyone gets the blessing of the god given the most: {} is ahead with {bname}, {bdesc}.",
        e("offering"),
        thousands(w.total()),
        thousands(w.goal),
        w.pct(),
        lead.name()
    ));
    if let Some(b) = w.blessing {
        let (n, d) = b.blessing();
        s.text(format!("-# {} Active now: {n}, {d}, from last week", e("vigor")));
    }
    if let Some(pos) = givers.iter().position(|(u, ..)| *u == p.user) {
        s.text(format!("-# You gave {} this week, {} in the server", thousands(givers[pos].2), ordinal(pos + 1)));
    }
    s.gap();
    s.text(format!(
        "-# Bonds   {} Thor {}   ·   {} Athena {}   ·   {} Hades {}",
        e("thor"),
        p.bond(God::Thor),
        e("athena"),
        p.bond(God::Athena),
        e("hades"),
        p.bond(God::Hades)
    ));
    s.buttons(vec![
        back(&ids),
        Btn::new(ids.act("boss", "start", ""), if p.boss_won { "Fight again" } else { "Fight the Barrow King" }).emoji("boss").danger().disabled(p.fight.is_some()),
        Btn::new(ids.go("exp"), "Expedition").emoji("expedition"),
        Btn::new(ids.go("temple"), "Temple").emoji("temple"),
    ]);
    s
}

fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, 11) | (2, 12) | (3, 13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// The plan picked in the menus, carried in the custom ids: `region.danger.hours.supply`.
fn plan_key(p: &Plan) -> String {
    format!("{}.{}.{}.{}", p.region, p.danger.id(), p.hours, p.supply.as_deref().unwrap_or("none"))
}

fn parse_plan(s: &str) -> Option<Plan> {
    let mut it = s.split('.');
    let region = data::region(it.next()?)?.id.to_owned();
    let danger = Danger::from_id(it.next()?)?;
    let hours: u32 = it.next()?.parse().ok()?;
    let supply = it.next().filter(|s| *s != "none" && data::EXPEDITION_SUPPLIES.contains(s)).map(str::to_owned);
    data::HOURS.contains(&hours).then_some(Plan { region, danger, hours, supply })
}

/// A sensible plan to start from: the best region you can handle on Normal.
fn default_plan(p: &Player) -> Plan {
    let power = p.power();
    let region = data::REGIONS.iter().rev().find(|r| power >= r.power).unwrap_or(&data::REGIONS[0]);
    Plan { region: region.id.into(), danger: Danger::Normal, hours: 4, supply: (p.have("rations") > 0).then(|| "rations".into()) }
}

fn danger_open(p: &Player, d: Danger) -> bool {
    d != Danger::Heroic || p.boss_won
}

fn expedition_plan(p: &Player, w: &World, plan: &Plan) -> Screen {
    let ids = Ids { owner: p.user, version: p.version };
    let mut s = Screen::default();
    title(&mut s, "expedition", "Plan an expedition", "Midgard");
    let key = plan_key(plan);
    let power = p.power();
    s.inner_select(
        format!("rpg:{}:exp:set:region|{key}:0", p.user),
        "Region",
        data::REGIONS
            .iter()
            .map(|r| {
                Opt::new(r.name, r.id)
                    .emoji(r.id)
                    .desc(format!("{} · needs {} Power on Normal", r.blurb.split('.').nth(1).map(str::trim).unwrap_or(r.blurb), r.power))
                    .default(r.id == plan.region)
            })
            .collect(),
    );
    s.inner_select(
        format!("rpg:{}:exp:set:danger|{key}:0", p.user),
        "Danger",
        data::DANGERS
            .iter()
            .filter(|d| danger_open(p, **d))
            .map(|d| {
                let pl = Plan { danger: *d, ..plan.clone() };
                Opt::new(format!("{} · ×{} loot", d.name(), d.loot()), d.id()).emoji(if *d == Danger::Normal { "check" } else { "warn" }).desc(format!("Needs {} Power here", pl.needs())).default(*d == plan.danger)
            })
            .collect(),
    );
    s.inner_select(
        format!("rpg:{}:exp:set:hours|{key}:0", p.user),
        "Length",
        data::HOURS.iter().map(|h| Opt::new(format!("{h} hour{}", if *h == 1 { "" } else { "s" }), h.to_string()).emoji("clock").default(*h == plan.hours)).collect(),
    );
    let mut supplies = vec![Opt::new("No supplies", "none").default(plan.supply.is_none())];
    for id in data::EXPEDITION_SUPPLIES {
        let n = p.have(id);
        if n > 0 {
            let it = data::item(id).unwrap();
            supplies.push(Opt::new(format!("{} ({n})", capital(it.name)), id).emoji(it.emoji).desc(it.used_for).default(plan.supply.as_deref() == Some(id)));
        }
    }
    s.inner_select(format!("rpg:{}:exp:set:supply|{key}:0", p.user), "Supplies", supplies);
    let f = expedition::forecast(plan, power, p.scouting(), w.expedition_rates());
    let mut fc = String::from("-# Forecast\n");
    fc.push_str(&format!("{} About {}\n", e("pack"), f.items.iter().filter(|(_, n)| *n > 0).map(|(id, n)| data::count(id, *n)).collect::<Vec<_>>().join(", ")));
    fc.push_str(&format!("{} {} gold · {} XP\n", e("gold"), f.gold, f.xp));
    let risk = if f.ratio >= 1.4 {
        format!("{} Low risk: {} Power clears this with room to spare", e("check"), plan.effective(power))
    } else if f.ratio >= 1.0 {
        format!("{} Some risk: {} Power is just enough", e("warn"), plan.effective(power))
    } else {
        format!("{} Too dangerous: needs {}, you have {}. You'd come back early and hurt.", e("warn"), plan.needs(), plan.effective(power))
    };
    fc.push_str(&risk);
    if !p.boss_won && p.facts.len() < data::FACTS.len() {
        fc.push_str(&format!("\n{} {}% chance to learn something about the Barrow King", e("intel"), (f.intel_chance * 100.0).round()));
    }
    fc.push_str(&format!("\n{} +{} tribute to {}'s altar", e("offering"), f.tribute, p.main().name()));
    s.text(fc);
    s.buttons(vec![
        back(&ids),
        Btn::new(ids.act("exp", "go", &key), format!("Send out · {}h", plan.hours)).emoji("expedition").primary().disabled(p.expedition.is_some()),
    ]);
    s
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn expedition_out(p: &Player, w: &World, x: &Expedition) -> Screen {
    let ids = Ids { owner: p.user, version: p.version };
    let mut s = Screen::default();
    title(&mut s, "expedition", &format!("{} · {}", x.plan.region().name, x.plan.danger.name()), &format!("{}h expedition", x.plan.hours));
    let done = p.now >= x.ends_at;
    let f = expedition::forecast(&x.plan, x.power, x.scouting, w.expedition_rates());
    if done {
        s.text(format!("{} **Back and waiting for you.**", e("check")));
    } else {
        s.text(format!("{} Back in **{}**", e("clock"), dur(x.ends_at - p.now)));
    }
    s.text(format!("-# Expected: {}", f.items.iter().filter(|(_, n)| *n > 0).map(|(id, n)| data::count(id, *n)).collect::<Vec<_>>().join(", ")));
    s.text("-# While it runs you can hunt, forge and give at the temple.".to_string());
    let mut b = vec![back(&ids)];
    if done {
        b.push(Btn::new(ids.act("exp", "collect", ""), "Collect").emoji("expedition").primary());
    } else {
        b.push(Btn::new(ids.go("hunt"), "Hunt meanwhile").emoji("hunt").primary());
        b.push(Btn::new(ids.go("exp"), "Refresh"));
    }
    s.buttons(b);
    s
}

fn expedition_report(p: &Player, r: &Report, levels: u32) -> Screen {
    let ids = Ids { owner: p.user, version: p.version };
    let region = data::region(&r.region).map(|x| x.name).unwrap_or("the wilds");
    let mut s = Screen::default();
    title(&mut s, "expedition", &format!("Back from {region}"), &format!("{} · {}h", capital(&r.danger), r.hours));
    let story: Vec<String> = r.story.iter().map(|(h, l)| format!("-# **{h}**  {l}")).collect();
    s.text(story.join("\n"));
    s.gap();
    s.text("-# Brought back");
    let mut lines = vec![items_line(&r.items)];
    lines.push(format!("{} {} gold  ·  {} XP  ·  +{} tribute", e("gold"), r.gold, r.xp, r.tribute));
    if let Some(f) = &r.intel {
        lines.push(format!("{} **New intel:** {}", e("intel"), data::fact_text(f, p.week())));
    }
    if levels > 0 {
        lines.push(format!("{} **Level {}!**", e("power"), p.level));
    }
    if r.wounded {
        lines.push(format!("{} You came back hurt: rest, or drink a potion before a fight.", e("hp")));
    }
    s.text(lines.join("\n"));
    let again = r.region.clone() + "." + &r.danger + "." + &r.hours.to_string();
    s.buttons(vec![
        back(&ids),
        Btn::new(ids.act("exp", "again", &again), "Send again").emoji("expedition").primary().disabled(p.expedition.is_some()),
        Btn::new(ids.go("exp"), "Plan new"),
        Btn::new(ids.go("hunt"), "Hunt").emoji("hunt"),
    ]);
    s
}

fn hunt_pick(p: &Player, region: Option<&str>) -> Screen {
    let ids = Ids { owner: p.user, version: p.version };
    let mut s = Screen::default();
    let power = p.power();
    let pick = region.and_then(data::region).unwrap_or_else(|| data::REGIONS.iter().rev().find(|r| power as f64 >= r.power as f64 * 1.3).unwrap_or(&data::REGIONS[0]));
    title(&mut s, "hunt", "Hunt", "Fight one foe at a time, turn by turn");
    let wins = (p.vigor() / data::VIGOR_PER_WIN as f64).floor() as i64;
    let (hp, max) = (p.hp(), p.max_hp());
    s.text(format!(
        "{} {hp} / {max} HP{}\n{} Vigor {:.0}%: {}",
        e("hp"),
        if hp < max { format!(" · full in {}", dur(progress::minutes_to_full(hp, max) * 60_000)) } else { String::new() },
        e("vigor"),
        p.vigor(),
        if wins > 0 { format!("your next {wins} win{} pay double", if wins == 1 { "" } else { "s" }) } else { "builds back while you're away".into() }
    ));
    s.inner_select(
        format!("rpg:{}:hunt:pick::0", p.user),
        "Where to hunt",
        data::REGIONS
            .iter()
            .map(|r| {
                let (xp, gold) = data::hunt_reward(r);
                Opt::new(r.name, r.id).emoji(r.id).desc(format!("Comfortable at {}+ Power · {xp} XP, {gold} gold a win", (r.power as f64 * 1.3) as i64)).default(r.id == pick.id)
            })
            .collect(),
    );
    let mons: Vec<String> = pick.monsters.iter().filter_map(|m| data::monster(m)).map(|m| m.name.to_owned()).collect();
    s.text(format!("-# {}: {}. Wins drop {}.", pick.name, mons.join(", "), pick.drops.iter().map(|(id, _)| data::item(id).map(|i| i.plural).unwrap_or(id)).collect::<Vec<_>>().join(", ")));
    s.buttons(vec![back(&ids), Btn::new(ids.act("hunt", "start", pick.id), "Start hunting").emoji("hunt").primary().disabled(hp < 1)]);
    s
}

fn pips(n: i32) -> String {
    (0..crate::engine::combat::MAX_STAMINA).map(|i| if i < n { e("pip") } else { e("pipoff") }).collect::<Vec<_>>().join("")
}

fn move_name(m: Move) -> &'static str {
    match m {
        Move::Hit => "a strike",
        Move::Heavy => "a heavy blow",
        Move::Brace => "its guard",
        Move::Roar => "a roar",
    }
}

fn skill_emoji(s: Skill) -> &'static str {
    match s {
        Skill::Mjolnir => "thor",
        Skill::Thunderclap => "thunder",
        Skill::SpearCounter => "spear",
        Skill::OwlSight => "owl",
        Skill::SoulRend => "rend",
        Skill::CerberusBite => "bite",
        Skill::AegisStorm => "athena",
        Skill::GraveThunder => "thunder",
        Skill::Judgement => "judgement",
    }
}

fn fight_screen(p: &Player, f: &Fight, result: Option<String>) -> Screen {
    let ids = Ids { owner: p.user, version: p.version };
    let boss = f.kind == crate::engine::combat::Kind::Boss;
    let screen = if boss { "boss" } else { "hunt" };
    let mut s = Screen::default();
    let meta = if boss { format!("Phase {} · turn {}", f.foe.phase, f.turn) } else { format!("{} · turn {}", data::region(&f.region).map(|r| r.name).unwrap_or(""), f.turn) };
    title(&mut s, if boss { "boss" } else { "hunt" }, &f.foe.name, &meta);
    let mut foe = format!("{} **{}** / {} HP", e("boss"), f.foe.hp, f.foe.max_hp);
    let mut tags = Vec::new();
    if f.foe.burn > 0 {
        tags.push("burning".to_string());
    }
    if f.foe.bleed > 0 {
        tags.push("bleeding".into());
    }
    if f.foe.cursed > 0 {
        tags.push("cursed".into());
    }
    if f.foe.braced {
        tags.push("guarded".into());
    }
    if !tags.is_empty() {
        foe.push_str(&format!("  ·  {}", tags.join(", ")));
    }
    if f.end.is_none() {
        let (t, scouted) = f.telegraph();
        let warn = matches!(f.intent(), Move::Heavy | Move::Roar);
        foe.push_str(&format!("\n{} {t}{}", if warn { e("warn") } else { e("clock") }, if scouted { "  ·  scouted" } else { "" }));
        let up = f.upcoming();
        if !up.is_empty() {
            foe.push_str(&format!("\n-# {} Then: {}", e("owl"), up.iter().map(|m| move_name(*m)).collect::<Vec<_>>().join(", then ")));
        }
    }
    s.text(foe);
    let mut you = format!("{} **{}** / {} HP\n{} {} stamina", e("hp"), f.hp, f.max_hp, pips(f.stamina), f.stamina);
    if let Some((el, n)) = f.coat {
        you.push_str(&format!("  ·  {} oil on your next {n} hit{}", el.id(), if n == 1 { "" } else { "s" }));
    }
    if f.hero.main == God::Thor && f.charge > 0 {
        you.push_str(&format!("  ·  Mjolnir charged ×{}", f.charge));
    }
    s.text(you);
    if !f.log.is_empty() {
        s.text(format!("-# Last turn\n{}", f.log.iter().map(|l| format!("-# {l}")).collect::<Vec<_>>().join("\n")));
    }
    if let Some(r) = result {
        s.gap();
        s.text(r);
    }
    if f.end.is_some() {
        if boss {
            s.buttons(vec![back(&ids), Btn::new(ids.go("goals"), "Goals").emoji("goals")]);
        } else {
            let again = Btn::new(ids.act("hunt", "start", &f.region), "Hunt again").emoji("hunt").primary().disabled(p.hp() < 1);
            s.buttons(vec![back(&ids), again]);
        }
        return s;
    }
    let potions = p.have("potion");
    let oils = (p.have("fireoil"), p.have("frostoil"));
    let act = |a: &str| ids.act(screen, "act", a);
    let mut row1 = vec![Btn::new(act("strike"), "Strike · 1").emoji("hunt").primary().disabled(f.blocked(Action::Strike, potions, oils).is_some())];
    for sk in f.hero.skills() {
        let blocked = f.blocked(Action::Skill(SkillId::of(sk)), potions, oils);
        let cd = f.cooldown(sk);
        let label = if cd > 0 { format!("{} · in {cd}", sk.name()) } else { format!("{} · {}", sk.name(), sk.stamina()) };
        row1.push(Btn::new(act(sk.id()), label).emoji(skill_emoji(sk)).disabled(blocked.is_some()));
    }
    row1.push(Btn::new(act("guard"), "Guard").emoji("guard"));
    let mut row2 = Vec::new();
    if row1.len() > 5 {
        row2.push(row1.pop().unwrap());
    }
    row2.insert(0, Btn::new(act("potion"), format!("Potion · {potions}")).emoji("potion").disabled(potions < 1));
    if oils.0 > 0 {
        row2.push(Btn::new(act("fireoil"), format!("Fire oil · {}", oils.0)).emoji("fireoil"));
    }
    if oils.1 > 0 {
        row2.push(Btn::new(act("frostoil"), format!("Frost oil · {}", oils.1)).emoji("frostoil"));
    }
    row2.push(Btn::new(ids.act(screen, "leave", ""), if boss { "Retreat" } else { "Leave" }));
    s.buttons(row1);
    s.buttons(row2);
    // Locked skills, so players know what's coming.
    let locked: Vec<String> = {
        let have = f.hero.skills();
        let mut out = Vec::new();
        let [_, second] = f.hero.main.skills();
        if !have.contains(&second) {
            out.push(format!("{} at bond 2 with {}", second.name(), f.hero.main.name()));
        }
        if f.hero.lesser.is_none() {
            out.push("a merged skill with your lesser vow, after the Barrow King".into());
        }
        out
    };
    if !locked.is_empty() {
        s.text(format!("-# {} Still to unlock: {}", e("lock"), locked.join("; ")));
    }
    s
}

fn forge(p: &Player) -> Screen {
    let ids = Ids { owner: p.user, version: p.version };
    let mut s = Screen::default();
    title(&mut s, "forge", "Forge", &format!("Gear up to level {}", p.level));
    let mats: Vec<String> = data::ITEMS.iter().filter(|i| i.kind == Kind::Material && p.have(i.id) > 0).map(|i| format!("{} {}", p.have(i.id), i.plural)).collect();
    s.text(format!("-# You have {} · {} gold", if mats.is_empty() { "no materials yet".into() } else { mats.join(" · ") }, thousands(p.gold)));
    s.text("-# Gear");
    // Per slot: the next piece that beats what you wear.
    let mut shown = 0;
    for slot in data::SLOTS {
        let worn = p.equipped(slot).map(|g| g.power()).unwrap_or(0);
        let next = data::GEAR.iter().filter(|g| g.slot == slot && g.power > worn && (!g.after_boss || p.boss_won)).min_by_key(|g| g.level);
        let Some(g) = next else { continue };
        shown += 1;
        let cost = g.cost.iter().map(|(id, n)| data::count(id, *n)).collect::<Vec<_>>().join(", ");
        let missing: Vec<String> = g.cost.iter().filter(|(id, n)| p.have(id) < *n).map(|(id, n)| data::count(id, n - p.have(id))).collect();
        let why = if g.level > p.level {
            format!("{} level {}", e("lock"), g.level)
        } else if !missing.is_empty() || p.gold < g.gold {
            let mut m = missing;
            if p.gold < g.gold {
                m.push(format!("{} gold", g.gold - p.gold));
            }
            format!("you need {} more", m.join(", "))
        } else {
            format!("{} ready", e("check"))
        };
        let can = g.level <= p.level && p.gold >= g.gold && g.cost.iter().all(|(id, n)| p.have(id) >= *n);
        s.section(
            format!("{} **{}**  +{} Power\n-# {cost}, {} gold · {why}", e(slot_emoji(slot)), g.name, g.power - worn, g.gold),
            Btn::new(ids.act("forge", "gear", g.id), "Forge").emoji("forge").disabled(!can).then_primary(can),
        );
    }
    if shown == 0 {
        s.text(format!("{} You wear the best gear Midgard has until the Barrow King falls.", e("check")));
    }
    let ups: Vec<Opt> = data::SLOTS
        .iter()
        .filter_map(|sl| p.equipped(*sl))
        .filter(|g| g.upgrade < data::MAX_UPGRADE)
        .map(|g| {
            let (gold, mat, n) = data::upgrade_cost(g.gear, g.upgrade);
            Opt::new(format!("{} → +{}", g.label(), g.upgrade + 1), g.id.to_string())
                .emoji(slot_emoji(g.gear.slot))
                .desc(format!("+{} Power · {gold} gold, {}", data::gear_power(g.gear, g.upgrade + 1) - g.power(), data::count(mat, n)))
        })
        .collect();
    s.inner_select(ids.act("forge", "upgrade", ""), "Upgrade what you wear (up to +3 in Midgard)…", ups);
    s.text("-# Supplies");
    let opts: Vec<Opt> = data::SUPPLY_RECIPES
        .iter()
        .map(|r| {
            let it = data::item(r.makes).unwrap();
            let cost = r.cost.iter().map(|(id, n)| data::count(id, *n)).collect::<Vec<_>>().join(", ");
            let can = p.gold >= r.gold && r.cost.iter().all(|(id, n)| p.have(id) >= *n);
            Opt::new(format!("{} ×{}{}", capital(it.plural), r.qty, if can { "" } else { " (can't afford)" }), r.id)
                .emoji(it.emoji)
                .desc(format!("{} · {cost}, {} gold · have {}", it.used_for, r.gold, p.have(r.makes)))
        })
        .collect();
    s.inner_select(ids.act("forge", "make", ""), "Make supplies…", opts);
    s.buttons(vec![back(&ids), Btn::new(ids.go("pack"), "Pack").emoji("pack")]);
    s
}

trait ThenPrimary {
    fn then_primary(self, on: bool) -> Self;
}
impl ThenPrimary for Btn {
    fn then_primary(self, on: bool) -> Self {
        if on { self.primary() } else { self }
    }
}

fn slot_emoji(s: Slot) -> &'static str {
    match s {
        Slot::Weapon => "hunt",
        Slot::Armour => "guard",
        Slot::Charm => "amber",
    }
}

fn pack(p: &Player) -> Screen {
    let ids = Ids { owner: p.user, version: p.version };
    let mut s = Screen::default();
    let gear_power = p.power() - data::level_power(p.level);
    title(&mut s, "pack", "Pack", &format!("Level {} gives {} Power, gear {}", p.level, data::level_power(p.level), gear_power));
    s.text("-# Wearing");
    let mut wear = Vec::new();
    for slot in data::SLOTS {
        match p.equipped(slot) {
            Some(g) => wear.push(format!("{} **{}**  ·  +{} Power{}", e(slot_emoji(slot)), g.label(), g.power(), if g.gear.scouting > 0 { format!(", +{}% scouting", g.gear.scouting) } else { String::new() })),
            None => wear.push(format!("{} {}: nothing yet", e(slot_emoji(slot)), slot.name())),
        }
    }
    s.text(wear.join("\n"));
    let mats: Vec<String> = data::ITEMS
        .iter()
        .filter(|i| i.kind == Kind::Material && p.have(i.id) > 0)
        .map(|i| format!("{} {}  ·  -# for {}", e(i.emoji), data::count(i.id, p.have(i.id)), i.used_for).replace("  ·  -# ", "  ·  "))
        .collect();
    if !mats.is_empty() {
        s.text("-# Materials");
        s.text(mats.join("\n"));
    }
    let sup: Vec<String> = data::ITEMS
        .iter()
        .filter(|i| matches!(i.kind, Kind::Supply | Kind::Combat | Kind::Trophy) && p.have(i.id) > 0)
        .map(|i| format!("{} {}", e(i.emoji), data::count(i.id, p.have(i.id))))
        .collect();
    if !sup.is_empty() {
        s.text("-# Supplies and trophies");
        s.text(sup.join("  ·  "));
    }
    let spare: Vec<Opt> = p
        .gear
        .iter()
        .filter(|g| !g.equipped)
        .map(|g| Opt::new(g.label(), g.id.to_string()).emoji(slot_emoji(g.gear.slot)).desc(format!("+{} Power · {}", g.power(), g.gear.slot.name())))
        .collect();
    if !spare.is_empty() {
        s.inner_select(ids.act("pack", "equip", ""), "Wear something else…", spare.clone());
        s.inner_select(ids.act("pack", "salvage", ""), "Salvage for gold and half the materials…", spare);
    }
    s.buttons(vec![back(&ids), Btn::new(ids.go("forge"), "Forge").emoji("forge")]);
    s
}

fn temple(p: &Player, w: &World, givers: &[(u64, String, i64)]) -> Screen {
    let ids = Ids { owner: p.user, version: p.version };
    let mut s = Screen::default();
    title(&mut s, "temple", "Temple of the Gods", "Resets every Monday");
    s.text(format!(
        "**{} of {} tribute this week** ({}%)\n-# Every gift fills the server's offering and builds your bond with that god. At 100%, everyone gets the blessing of the god given the most.",
        thousands(w.total()),
        thousands(w.goal),
        w.pct()
    ));
    if let Some(b) = w.blessing {
        let (n, d) = b.blessing();
        s.text(format!("{} Active now: **{n}**, {d}", e("vigor")));
    }
    s.text("-# Altars · your bonds");
    for g in data::GODS {
        let bond = p.bond(g);
        let mut line = format!("{} **{}** · bond {bond}{}", e(g.id()), g.name(), if p.patron == Some(g) { " · your patron" } else if p.lesser == Some(g) { " · lesser vow" } else { "" });
        let next = progress::to_next_bond(p.tribute[g as usize], data::BOND_CAP);
        let unlock = if p.patron == Some(g) {
            if bond >= 2 { format!("{} unlocked. Bond 2 is Midgard's cap.", g.skills()[1].name()) } else { format!("Bond 2 unlocks {}", g.skills()[1].name()) }
        } else {
            let pair = data::pair_skill(p.main(), g).map(|s| s.name()).unwrap_or("");
            format!("As your lesser vow: {} and {pair}", g.lesser_passive().to_lowercase())
        };
        line.push_str(&format!("\n-# {unlock}"));
        if let Some(n) = next {
            line.push_str(&format!(" · {} tribute to bond {}", thousands(n), bond + 1));
        }
        line.push_str(&format!(" · this week: {}", thousands(w.offerings[g as usize])));
        let b = Btn::new(ids.go(&format!("give{}", g.id())), "Give").emoji("offering");
        s.section(line, if p.patron == Some(g) { b.primary() } else { b });
    }
    if !givers.is_empty() {
        let list: Vec<String> = givers.iter().enumerate().map(|(i, (u, n, t))| format!("-# {}. {}{} · {}", i + 1, if n.is_empty() { "Someone" } else { n }, if *u == p.user { " (you)" } else { "" }, thousands(*t))).collect();
        s.text(format!("-# Most generous this week\n{}", list.join("\n")));
    }
    s.buttons(vec![back(&ids), Btn::new(ids.go("patron"), "Patron and skills").emoji(p.main().id())]);
    s
}

/// Spare materials: anything past 20 of each.
fn spare_materials(p: &Player) -> Vec<(&'static str, i64, i64)> {
    data::ITEMS.iter().filter(|i| i.kind == Kind::Material && p.have(i.id) > 20).map(|i| (i.id, p.have(i.id) - 20, (p.have(i.id) - 20) * i.tribute)).collect()
}

fn give(p: &Player, g: God) -> Screen {
    let ids = Ids { owner: p.user, version: p.version };
    let mut s = Screen::default();
    title(&mut s, g.id(), &format!("Give to {}", g.name()), &format!("Bond {}", p.bond(g)));
    let t = p.tribute[g as usize];
    s.text(match progress::to_next_bond(t, data::BOND_CAP) {
        Some(n) => format!("You've given {} in all. **{}** more for bond {}.", thousands(t), thousands(n), p.bond(g) + 1),
        None => format!("You've given {} in all. Bond {} is Midgard's cap: gifts still count for the server.", thousands(t), p.bond(g)),
    });
    let spare = spare_materials(p);
    let worth: i64 = spare.iter().map(|x| x.2).sum();
    s.text(format!(
        "-# Spare materials (past 20 of each): {}",
        if spare.is_empty() { "none right now".into() } else { format!("{} · worth {worth} tribute", spare.iter().map(|(id, n, _)| data::count(id, *n)).collect::<Vec<_>>().join(", ")) }
    ));
    s.buttons(vec![
        back_to(&ids, "temple", "Temple"),
        Btn::new(ids.act("temple", "give", &format!("{}.gold100", g.id())), "100 gold").emoji("gold").disabled(p.gold < 100),
        Btn::new(ids.act("temple", "give", &format!("{}.gold500", g.id())), "500 gold").emoji("gold").disabled(p.gold < 500),
        Btn::new(ids.act("temple", "give", &format!("{}.spare", g.id())), "Spare materials").emoji("pack").primary().disabled(spare.is_empty()),
    ]);
    s
}

fn patron_screen(p: &Player) -> Screen {
    let ids = Ids { owner: p.user, version: p.version };
    let main = p.main();
    let mut s = Screen::default();
    title(&mut s, main.id(), &format!("Sworn to {}", main.name()), &format!("Bond {}", p.bond(main)));
    let hero = p.hero();
    let have = hero.skills();
    let mut lines = vec!["-# Skills".to_string()];
    let mut all = main.skills().to_vec();
    if let Some(l) = p.lesser
        && let Some(ps) = data::pair_skill(main, l)
    {
        all.push(ps);
    }
    for sk in all {
        let state = if have.contains(&sk) { format!("{} · {} stamina", e("check"), sk.stamina()) } else { format!("{} locked", e("lock")) };
        lines.push(format!("{} **{}** {state}\n-# {}", e(skill_emoji(sk)), sk.name(), sk.describe(main)));
    }
    if !have.contains(&main.skills()[1]) {
        lines.push(format!("-# {} {} unlocks at bond 2 with {}: give at the temple, and every expedition adds a little.", e("lock"), main.skills()[1].name(), main.name()));
    }
    s.text(lines.join("\n"));
    s.gap();
    match (p.boss_won, p.lesser) {
        (false, _) => {
            s.text(format!("-# Lesser vow\n{} After the Barrow King you can take a second god's passive at half strength, and your pair's merged skill.", e("lock")));
        }
        (true, None) => {
            s.text("-# Lesser vow\nChoose a second god. You get their passive at half strength, and your pair's merged skill once you have bond 1 with them.");
            let opts = data::GODS
                .iter()
                .filter(|g| **g != main)
                .map(|g| Opt::new(g.name(), g.id()).emoji(g.id()).desc(format!("{} · {}", g.lesser_passive(), data::pair_skill(main, *g).map(|s| s.name()).unwrap_or(""))))
                .collect();
            s.inner_select(ids.act("patron", "vow", ""), "Take a lesser vow…", opts);
        }
        (true, Some(l)) => {
            s.text(format!("-# Lesser vow\n{} **{}** · {}\n-# Bond {} with {}", e(l.id()), l.name(), l.lesser_passive(), p.bond(l), l.name()));
        }
    }
    s.gap();
    s.text(format!("-# Change patron for {} gold. Your bonds stay.", data::PATRON_CHANGE_GOLD));
    let opts = data::GODS.iter().filter(|g| **g != main).map(|g| Opt::new(format!("Swear to {} instead", g.name()), g.id()).emoji(g.id()).desc(g.pitch())).collect();
    s.inner_select(ids.act("patron", "change", ""), "Change patron…", opts);
    s.buttons(vec![back(&ids), Btn::new(ids.go("temple"), "Temple").emoji("temple")]);
    s
}

fn help(p: &Player) -> Screen {
    let ids = Ids { owner: p.user, version: p.version };
    let mut s = Screen::default();
    title(&mut s, "help", "How to play", "The short version");
    s.text(format!(
        "{} **Expeditions** work while you're away. Pick a region, a danger and a length; the forecast shows what to expect. Too dangerous and you come back early and hurt.\n\
         {} **Hunts** are fights you play turn by turn. Watch what the foe will do next: guard heavy blows, strike when it's open. Vigor builds while you're away and doubles your wins.\n\
         {} **The forge** turns materials into gear (Power) and supplies (potions, oils, rations). Every material is used for something.\n\
         {} **The Barrow King** is Midgard's boss. It needs {} Power. Expeditions to the Barrow Fields scout it: what you learn shows up in the fight.\n\
         {} **The temple** has an altar for each god. Gifts fill the server's weekly offering and build your bond, which unlocks skills.\n\
         -# Level, gear and bonds stop at Midgard's caps until the next realm, so playing a lot gets you there sooner, not ten times stronger.",
        e("expedition"),
        e("hunt"),
        e("forge"),
        e("boss"),
        data::BOSS_POWER,
        e("temple"),
    ));
    s.buttons(vec![back(&ids)]);
    s
}

// ---------------------------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------------------------

fn seed(p: &Player, salt: u64) -> u64 {
    (p.now as u64).wrapping_mul(6364136223846793005) ^ p.user.rotate_left(17) ^ (p.version as u64).wrapping_mul(0x9E37) ^ salt
}

/// Start an expedition from a plan.
fn send_out(g: &mut Game, plan: Plan) -> Result<Option<String>> {
    if g.p.expedition.is_some() {
        return Ok(Some("You already have an expedition out.".into()));
    }
    if !danger_open(&g.p, plan.danger) {
        return Ok(Some("Heroic opens after the Barrow King falls.".into()));
    }
    let mut plan = plan;
    if let Some(sup) = plan.supply.clone() {
        if g.p.have(&sup) < 1 {
            plan.supply = None;
        } else {
            g.item(&sup, -1, "expedition supply")?;
        }
    }
    let now = g.p.now;
    let e = Expedition { power: g.p.power(), scouting: g.p.scouting(), started_at: now, ends_at: now + plan.hours as i64 * 3_600_000, seed: seed(&g.p, 11), plan };
    g.start_expedition(e)?;
    g.bump()?;
    Ok(None)
}

fn collect(g: &mut Game, w: &World) -> Result<Option<(Report, u32)>> {
    let Some(x) = g.p.expedition.clone() else { return Ok(None) };
    if g.p.now < x.ends_at {
        return Ok(None);
    }
    let pity = g.pity(&x.plan.region)?;
    let facts = if g.p.boss_won { data::FACTS.iter().map(|s| s.to_string()).collect() } else { g.p.facts.clone() };
    let r = expedition::run(&x.plan, x.power, x.scouting, &facts, pity, w.expedition_rates(), x.seed);
    for (id, n) in &r.items {
        g.item(id, *n, "expedition")?;
    }
    g.gold(r.gold, "expedition")?;
    let levels = g.xp(r.xp)?;
    let main = g.p.main();
    g.tribute(main, r.tribute)?;
    g.set_pity(&x.plan.region, r.pity)?;
    if let Some(f) = &r.intel {
        g.learn(f)?;
    }
    if r.wounded {
        let max = g.p.max_hp();
        g.set_hp(max * 3 / 10)?;
    }
    g.end_expedition()?;
    g.set_report(Some(r.clone()))?;
    g.bump()?;
    Ok(Some((r, levels)))
}

fn parse_action(a: &str) -> Option<Action> {
    Some(match a {
        "strike" => Action::Strike,
        "guard" => Action::Guard,
        "potion" => Action::Potion,
        "fireoil" => Action::Oil(Element::Fire),
        "frostoil" => Action::Oil(Element::Frost),
        s => Action::Skill(SkillId::of(Skill::from_id(s)?)),
    })
}

/// Play a turn, pay for what the fight used, and settle the end.
fn fight_turn(g: &mut Game, w: &World, a: Action) -> Result<(Option<String>, Option<String>, Option<String>)> {
    let Some(mut f) = g.p.fight.clone() else { return Ok((Some("That fight is over.".into()), None, None)) };
    let potions = g.p.have("potion");
    let oils = (g.p.have("fireoil"), g.p.have("frostoil"));
    if let Err(why) = f.act(a, potions, oils) {
        return Ok((Some(why), None, None));
    }
    for u in std::mem::take(&mut f.used) {
        g.item(&u, -1, "used in a fight")?;
    }
    g.bump()?;
    let mut result = None;
    let mut announce = None;
    match f.end {
        None => {
            g.save_fight(Some(f))?;
        }
        Some(End::Lost) => {
            g.set_hp(1)?;
            result = Some(format!("{} **You fell.** You wake at camp with 1 HP. Rest or drink a potion, then try again.", e("hp")));
            g.save_fight(None)?;
            g.p.fight = Some(f);
        }
        Some(End::Won) => {
            g.set_hp(f.hp)?;
            match f.kind {
                crate::engine::combat::Kind::Hunt => {
                    let r = data::region(&f.region).unwrap_or(&data::REGIONS[0]);
                    let (xp, gold) = data::hunt_reward(r);
                    let double = g.p.vigor() >= data::VIGOR_PER_WIN as f64;
                    let m = if double { 2 } else { 1 };
                    if double {
                        g.vigor(-(data::VIGOR_PER_WIN as f64))?;
                    }
                    let xp = xp * m * w.xp_rate() / 100 * w.hunt_mult() / 100;
                    let gold = gold * m * w.rates.gold / 100 * w.hunt_mult() / 100;
                    let mut rng = crate::engine::Rng::new(seed(&g.p, 29));
                    let (mat, _) = *rng.pick(r.drops);
                    let mut drops = vec![(mat.to_string(), m)];
                    if rng.chance(0.06) {
                        drops.push((r.rare.0.to_string(), 1));
                    }
                    for (id, n) in &drops {
                        g.item(id, *n, "hunt")?;
                    }
                    g.gold(gold, "hunt")?;
                    let levels = g.xp(xp)?;
                    result = Some(format!(
                        "{} **{} falls.** +{xp} XP · +{gold} gold · {}{}{}",
                        e("check"),
                        f.foe.name,
                        items_line(&drops),
                        if double { format!("\n-# {} Doubled by Vigor", e("vigor")) } else { String::new() },
                        if levels > 0 { format!("\n{} **Level {}!**", e("power"), g.p.level) } else { String::new() }
                    ));
                }
                crate::engine::combat::Kind::Boss => {
                    let week = g.p.week();
                    let first = !g.p.boss_won;
                    let rewarded = first || g.p.boss_reward_week != week;
                    g.boss_won(f.turn)?;
                    if rewarded {
                        let shards = if first { 3 } else { 1 };
                        g.item("shard", shards, "the Barrow King")?;
                        g.gold(if first { 500 } else { 200 }, "the Barrow King")?;
                        g.xp(if first { 1500 } else { 400 })?;
                        result = Some(format!(
                            "{} **The Barrow King falls!** +{} crown shard{} · +{} gold{}",
                            e("shard"),
                            shards,
                            if shards == 1 { "" } else { "s" },
                            if first { 500 } else { 200 },
                            if first { "\nHeroic expeditions and your lesser vow are open. The forge has new gear." } else { "" }
                        ));
                    } else {
                        result = Some(format!("{} **The Barrow King falls again.** Its reward comes back on Monday.", e("shard")));
                    }
                    if first {
                        announce = Some(format!("{} **{}** has defeated **The Barrow King** in {} turns.", e("shard"), g.p.name, f.turn));
                    }
                }
            }
            g.save_fight(None)?;
            g.p.fight = Some(f);
        }
    }
    Ok((None, result, announce))
}

/// Run one click. `values` are a menu's picked values.
pub fn handle(ctx: &Ctx, g: &mut Game, route: &Route, values: &[String], rates: Rates) -> Result<Outcome> {
    let w = World::load(g, rates)?;
    let stale = !route.action.is_empty() && route.version >= 0 && route.version != g.p.version && !matches!(route.action.as_str(), "set" | "pick" | "nav");
    let note_stale = Some("That button was from before your last move. Here's where things are now.".to_string());
    let value = values.first().cloned().unwrap_or_default();
    let mut announce = None;
    let mut note: Option<String> = None;
    let screen = if stale {
        note = note_stale;
        view(ctx, g, &w, &route.screen, None)?
    } else {
        match (route.screen.as_str(), route.action.as_str()) {
            ("hub", "nav") => view(ctx, g, &w, &value, None)?,
            ("exp", "set") => {
                let (field, key) = route.arg.split_once('|').unwrap_or(("", ""));
                let mut plan = parse_plan(key).unwrap_or_else(|| default_plan(&g.p));
                match field {
                    "region" => plan.region = data::region(&value).map(|r| r.id.to_owned()).unwrap_or(plan.region),
                    "danger" => plan.danger = Danger::from_id(&value).unwrap_or(plan.danger),
                    "hours" => plan.hours = value.parse().ok().filter(|h| data::HOURS.contains(h)).unwrap_or(plan.hours),
                    "supply" => plan.supply = (value != "none").then_some(value.clone()),
                    _ => {}
                }
                expedition_plan(&g.p, &w, &plan)
            }
            ("exp", "go") => {
                let plan = parse_plan(&route.arg).unwrap_or_else(|| default_plan(&g.p));
                note = send_out(g, plan)?;
                view(ctx, g, &w, "exp", None)?
            }
            ("exp", "again") => {
                let mut plan = parse_plan(&format!("{}.none", route.arg)).unwrap_or_else(|| default_plan(&g.p));
                plan.supply = data::EXPEDITION_SUPPLIES.iter().find(|s| g.p.have(s) > 0 && **s == "rations").map(|s| s.to_string());
                note = send_out(g, plan)?;
                view(ctx, g, &w, "exp", None)?
            }
            ("exp", "collect") => match collect(g, &w)? {
                Some((r, levels)) => expedition_report(&g.p, &r, levels),
                None => view(ctx, g, &w, "exp", None)?,
            },
            ("hunt", "pick") => hunt_pick(&g.p, Some(&value)),
            ("hunt", "start") => {
                if g.p.fight.is_some() {
                    note = Some("Finish the fight you're in first.".into());
                } else if g.p.hp() < 1 {
                    note = Some("You need to rest first.".into());
                } else {
                    let r = data::region(&route.arg).unwrap_or(&data::REGIONS[0]);
                    let f = Fight::hunt(g.p.hero(), g.p.hp(), r, seed(&g.p, 5));
                    g.save_fight(Some(f))?;
                    g.bump()?;
                }
                view(ctx, g, &w, "hunt", None)?
            }
            ("boss", "start") => {
                if g.p.fight.is_some() {
                    note = Some("Finish the fight you're in first.".into());
                } else if g.p.hp() < g.p.max_hp() / 2 {
                    note = Some(format!("You're hurt ({} / {} HP). Rest or drink a potion before facing the King.", g.p.hp(), g.p.max_hp()));
                } else {
                    let f = Fight::boss(g.p.hero(), g.p.hp(), w.week, g.p.facts.clone(), seed(&g.p, 7));
                    g.save_fight(Some(f))?;
                    g.bump()?;
                }
                view(ctx, g, &w, "boss", None)?
            }
            ("hunt" | "boss", "act") => match parse_action(&route.arg) {
                Some(a) => {
                    let (n, result, ann) = fight_turn(g, &w, a)?;
                    note = n;
                    announce = ann;
                    match &g.p.fight {
                        Some(f) => fight_screen(&g.p, &f.clone(), result),
                        None => view(ctx, g, &w, &route.screen, None)?,
                    }
                }
                None => view(ctx, g, &w, &route.screen, None)?,
            },
            ("hunt" | "boss", "leave") => {
                if let Some(f) = g.p.fight.clone() {
                    g.set_hp(f.hp)?;
                    g.save_fight(None)?;
                    g.bump()?;
                    note = Some(if f.kind == crate::engine::combat::Kind::Boss { "You retreat. The King's wounds will heal; yours too.".into() } else { "You slip away from the fight.".into() });
                }
                view(ctx, g, &w, "hub", None)?
            }
            ("forge", "gear") => {
                match data::gear(&route.arg) {
                    Some(gear) if gear.level <= g.p.level && (!gear.after_boss || g.p.boss_won) && g.can_pay(gear.cost, gear.gold) => {
                        g.pay(gear.cost, gear.gold, "forged gear")?;
                        let worn = g.p.equipped(gear.slot).map(|x| x.power()).unwrap_or(0);
                        g.add_gear(gear, gear.power > worn)?;
                        g.bump()?;
                        note = Some(format!("Forged the {}. You're wearing it.", gear.name));
                    }
                    _ => note = Some("You can't forge that yet.".into()),
                }
                forge(&g.p)
            }
            ("forge", "upgrade") => {
                let id: i64 = value.parse().unwrap_or(-1);
                match g.p.gear.iter().find(|x| x.id == id && x.equipped).cloned() {
                    Some(row) if row.upgrade < data::MAX_UPGRADE => {
                        let (gold, mat, n) = data::upgrade_cost(row.gear, row.upgrade);
                        if g.can_pay(&[(mat, n)], gold) {
                            g.pay(&[(mat, n)], gold, "upgrade")?;
                            g.upgrade(id)?;
                            g.bump()?;
                            note = Some(format!("{} is now +{}.", row.gear.name, row.upgrade + 1));
                        } else {
                            note = Some(format!("Upgrading needs {gold} gold and {}.", data::count(mat, n)));
                        }
                    }
                    _ => note = Some("That can't be upgraded further in Midgard.".into()),
                }
                forge(&g.p)
            }
            ("forge", "make") => {
                match data::SUPPLY_RECIPES.iter().find(|r| r.id == value) {
                    Some(r) if g.can_pay(r.cost, r.gold) => {
                        g.pay(r.cost, r.gold, "made supplies")?;
                        g.item(r.makes, r.qty, "made at the forge")?;
                        g.bump()?;
                        note = Some(format!("Made {}.", data::count(r.makes, r.qty)));
                    }
                    Some(r) => note = Some(format!("You need {} and {} gold.", r.cost.iter().map(|(id, n)| data::count(id, *n)).collect::<Vec<_>>().join(", "), r.gold)),
                    None => {}
                }
                forge(&g.p)
            }
            ("pack", "equip") => {
                let id: i64 = value.parse().unwrap_or(-1);
                if g.p.gear.iter().any(|x| x.id == id) {
                    g.equip(id)?;
                    g.bump()?;
                }
                pack(&g.p)
            }
            ("pack", "salvage") => {
                let id: i64 = value.parse().unwrap_or(-1);
                if let Some(row) = g.p.gear.iter().find(|x| x.id == id && !x.equipped).cloned() {
                    g.remove_gear(id)?;
                    for (mat, n) in row.gear.cost {
                        if *n / 2 > 0 && data::item(mat).is_some_and(|i| i.kind == Kind::Material) {
                            g.item(mat, n / 2, "salvaged")?;
                        }
                    }
                    g.gold(row.gear.gold / 3, "salvaged")?;
                    g.bump()?;
                    note = Some(format!("Salvaged the {}.", row.gear.name));
                }
                pack(&g.p)
            }
            ("temple", "give") => {
                let (god, what) = route.arg.split_once('.').unwrap_or(("", ""));
                if let Some(god) = God::from_id(god) {
                    let before = g.p.bond(god);
                    match what {
                        "gold100" | "gold500" => {
                            let n = if what == "gold100" { 100 } else { 500 };
                            if g.p.gold >= n {
                                g.gold(-n, &format!("offering to {}", god.name()))?;
                                g.tribute(god, n)?;
                                g.bump()?;
                            }
                        }
                        "spare" => {
                            let spare = spare_materials(&g.p);
                            let mut total = 0;
                            for (id, n, t) in spare {
                                g.item(id, -n, &format!("offering to {}", god.name()))?;
                                total += t;
                            }
                            g.tribute(god, total)?;
                            g.bump()?;
                        }
                        _ => {}
                    }
                    let after = g.p.bond(god);
                    if after > before {
                        note = Some(format!("{} smiles on you. Bond {after}.", god.name()));
                    }
                    give(&g.p, god)
                } else {
                    view(ctx, g, &w, "temple", None)?
                }
            }
            ("patron", "swear") => view(ctx, g, &w, "hub", None)?,
            ("patron", "vow") => {
                if let Some(god) = God::from_id(&value).filter(|x| Some(*x) != g.p.patron)
                    && g.p.boss_won
                {
                    let cost = if g.p.lesser.is_some() { data::PATRON_CHANGE_GOLD } else { 0 };
                    if g.p.gold >= cost {
                        g.gold(-cost, "lesser vow")?;
                        g.set_lesser(Some(god))?;
                        g.bump()?;
                        note = Some(format!("You took a lesser vow to {}.", god.name()));
                    }
                }
                patron_screen(&g.p)
            }
            ("patron", "change") => {
                if let Some(god) = God::from_id(&value).filter(|x| Some(*x) != g.p.patron) {
                    if g.p.fight.is_some() {
                        note = Some("Not in the middle of a fight.".into());
                    } else if g.p.gold < data::PATRON_CHANGE_GOLD {
                        note = Some(format!("Changing patron costs {} gold.", data::PATRON_CHANGE_GOLD));
                    } else {
                        g.gold(-data::PATRON_CHANGE_GOLD, "changed patron")?;
                        if g.p.lesser == Some(god) {
                            g.set_lesser(None)?;
                        }
                        g.set_patron(god)?;
                        g.bump()?;
                        note = Some(format!("You now serve {}.", god.name()));
                    }
                }
                patron_screen(&g.p)
            }
            (screen, _) => view(ctx, g, &w, screen, None)?,
        }
    };
    Ok(Outcome { screen: screen.note(note), announce })
}

/// Show a screen without changing anything.
pub fn view(ctx: &Ctx, g: &mut Game, w: &World, screen: &str, _arg: Option<&str>) -> Result<Screen> {
    let p = &g.p;
    Ok(match screen {
        "goals" => goals(p, w, &store::top_givers(&g.tx, p.guild, w.week, 50)?),
        "exp" => match &p.expedition {
            Some(x) => expedition_out(p, w, &x.clone()),
            None => expedition_plan(p, w, &default_plan(p)),
        },
        "hunt" | "boss" => match &p.fight {
            Some(f) => fight_screen(p, &f.clone(), None),
            None if screen == "boss" => goals(p, w, &store::top_givers(&g.tx, p.guild, w.week, 50)?),
            None => hunt_pick(p, None),
        },
        "forge" => forge(p),
        "pack" => pack(p),
        "temple" => temple(p, w, &store::top_givers(&g.tx, p.guild, w.week, 5)?),
        "givethor" => give(p, God::Thor),
        "giveathena" => give(p, God::Athena),
        "givehades" => give(p, God::Hades),
        "patron" => patron_screen(p),
        "help" => help(p),
        _ => hub(ctx, p, w),
    })
}

/// The overview for `/rpg`, after making sure the player's row is current.
pub fn overview(ctx: &Ctx, g: &mut Game, rates: Rates) -> Result<Screen> {
    let w = World::load(g, rates)?;
    view(ctx, g, &w, "hub", None)
}

/// Clicks per screen, for the dashboard: (action label, count) is kept by the caller.
pub fn label(route: &Route) -> String {
    if route.action.is_empty() { route.screen.clone() } else { format!("{}:{}", route.screen, route.action) }
}

/// Delete a player's character entirely (dashboard tool).
pub fn reset_player(db: &bot_core::Db, guild: u64, user: u64) -> Result<()> {
    let mut c = db.lock();
    let tx = c.transaction()?;
    for t in ["rpg_players", "rpg_items", "rpg_gear", "rpg_bonds", "rpg_expeditions", "rpg_fights", "rpg_intel", "rpg_pity", "rpg_kills"] {
        tx.execute(&format!("DELETE FROM {t} WHERE guild_id = ?1 AND user_id = ?2"), params![guild as i64, user as i64])?;
    }
    tx.commit()?;
    Ok(())
}

pub fn now() -> i64 {
    now_ms()
}
