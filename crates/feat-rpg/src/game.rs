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
use crate::quests;
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
        format!(
            "rpg:{}:{screen}:{action}:{arg}:{}",
            self.owner, self.version
        )
    }
}

/// Server-wide numbers the screens show.
pub struct World {
    /// A Titan that's up, and the guild's raid: (name, % left, ends at).
    pub titan: Option<(&'static str, i64, i64)>,
    pub raid: Option<(&'static str, i64, i64)>,
    pub modules: store::Modules,
    /// Guild perks, in percent.
    pub guild_xp: i64,
    pub guild_loot: i64,
    pub guild_gold: i64,
    pub week: i64,
    pub offerings: [i64; 3],
    pub goal: i64,
    pub blessing: Option<God>,
    pub rates: Rates,
    /// For the overview: players asking to join (if you lead), or guilds that invited you.
    pub guild_asks: usize,
    pub guild_invites: usize,
}

impl World {
    pub fn load(g: &Game, rates: Rates) -> Result<World> {
        let week = g.p.week();
        let modules = store::modules(g.p.guild);
        let now = g.p.now;
        let up = |x: Option<crate::shared::Shared>| {
            x.filter(|t| !t.over(now))
                .map(|t| (t.name(), t.hp * 100 / t.max_hp.max(1), t.ends))
        };
        let titan = if modules.titans {
            up(crate::shared::active(&g.tx, g.p.guild, "titan", 0)?)
        } else {
            None
        };
        let raid = match &g.p.clan {
            Some(c) if modules.raids => up(crate::shared::active(&g.tx, g.p.guild, "raid", c.id)?),
            _ => None,
        };
        Ok(World {
            titan,
            raid,
            modules,
            guild_xp: crate::guild::xp_bonus(&g.p),
            guild_loot: crate::guild::loot_bonus(&g.p),
            guild_gold: crate::guild::gold_bonus(&g.p),
            week,
            offerings: store::offerings(&g.tx, g.p.guild, week)?,
            goal: store::goal(&g.tx, g.p.guild, week)?,
            blessing: store::blessing(&g.tx, g.p.guild, week)?,
            rates,
            guild_asks: match &g.p.clan {
                Some(c) if modules.guilds && c.leader => {
                    crate::guild::asks(&g.tx, c.id, now)?.len()
                }
                _ => 0,
            },
            guild_invites: if modules.guilds && g.p.clan.is_none() {
                crate::guild::pending(&g.tx, g.p.guild, g.p.user, now)?
                    .iter()
                    .filter(|x| x.1 == "invite")
                    .count()
            } else {
                0
            },
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
        self.rates.xp
            * if self.blessing == Some(God::Athena) {
                110
            } else {
                100
            }
            / 100
            * (100 + self.guild_xp)
            / 100
    }
    fn expedition_rates(&self) -> Rates {
        let loot = if self.blessing == Some(God::Hades) {
            110
        } else {
            100
        };
        Rates {
            xp: self.xp_rate(),
            loot: self.rates.loot * loot / 100 * (100 + self.guild_loot) / 100,
            gold: self.rates.gold * loot / 100 * (100 + self.guild_gold) / 100,
        }
    }
    fn hunt_mult(&self) -> i64 {
        if self.blessing == Some(God::Thor) {
            110
        } else {
            100
        }
    }
}

pub(crate) fn dur(ms: i64) -> String {
    let m = (ms.max(0) + 59_999) / 60_000;
    if m >= 60 {
        if m % 60 == 0 {
            format!("{}h", m / 60)
        } else {
            format!("{}h {}m", m / 60, m % 60)
        }
    } else {
        format!("{m}m")
    }
}

pub(crate) fn items_line(items: &[(String, i64)]) -> String {
    items
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(id, n)| format!("{} {}", e(id), data::count(id, *n)))
        .collect::<Vec<_>>()
        .join("  ·  ")
}

pub(crate) fn title(md: &mut Screen, icon: &str, text: &str, meta: &str) {
    md.text(format!("### {} {text}", e(icon)));
    if !meta.is_empty() {
        md.text(format!("-# {meta}"));
    }
}

/// A bold line with a quiet one under it, or the bold line alone when there's nothing to say.
fn sub_line(head: &str, sub: &str) -> String {
    if sub.is_empty() {
        head.to_string()
    } else {
        format!("{head}\n-# {sub}")
    }
}

pub(crate) fn back(ids: &Ids) -> Btn {
    Btn::new(ids.go("hub"), "← Back")
}

pub(crate) fn back_to(ids: &Ids, screen: &str, label: &str) -> Btn {
    Btn::new(ids.go(screen), format!("← {label}"))
}

// ---------------------------------------------------------------------------------------------
// Screens
// ---------------------------------------------------------------------------------------------

/// The first thing a new player sees.
pub fn patron_choice(owner: u64, name: &str) -> Screen {
    let ids = Ids { owner, version: 0 };
    let mut s = Screen::default();
    title(
        &mut s,
        "offering",
        "Swear to a god",
        &format!("Welcome, {name}"),
    );
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
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    let xp10 = (p.xp * 10 / p.xp_needed().max(1)).clamp(0, 10) as u32;
    s.banner = Some(
        banner::Spec {
            patron: p.main(),
            name: p.name.clone(),
            level: p.level,
            power: p.power(),
            xp10,
            title: p.title.clone().unwrap_or_default(),
        }
        .url(ctx),
    );
    s.accent = Some(banner::accent(p.main()));
    let mut primary_used = tutorial_block(&mut s, p, &ids, &w.modules);
    s.text("-# Right now");
    let open = |f| crate::unlocks::open(p, f);
    if let Some((name, pct, ends)) = w.titan.filter(|_| open(data::Feature::Titans)) {
        s.section(format!("{} **{name} is attacking Midgard**\n-# {pct}% left · {} to bring it down, together", e("boss"), dur(ends - p.now)), Btn::new(ids.go("titan"), "Fight").emoji("boss").danger());
    }
    if let Some((name, pct, ends)) = w.raid {
        s.section(
            format!(
                "{} **Guild raid: {name}**\n-# {pct}% left · {} left",
                e("guild"),
                dur(ends - p.now)
            ),
            Btn::new(ids.go("raid"), "Raid").emoji("boss"),
        );
    }
    if w.guild_asks > 0 {
        s.section(
            format!(
                "{} **{} asking to join your guild**\n-# Accept or decline under Manage",
                e("guild"),
                w.guild_asks
            ),
            Btn::new(ids.go("guildmanage"), "Manage").emoji("guild"),
        );
    }
    if w.guild_invites > 0 {
        s.section(
            format!(
                "{} **{} invited you**\n-# Invites last a week",
                e("guild"),
                if w.guild_invites == 1 {
                    "A guild".to_string()
                } else {
                    format!("{} guilds", w.guild_invites)
                }
            ),
            Btn::new(ids.go("guild"), "See").emoji("guild"),
        );
    }
    if let Some(run) = &p.run {
        s.section(
            format!(
                "{} **In the Labyrinth, floor {}**\n-# Carrying: {}",
                e("labyrinth"),
                run.floor,
                crate::ui::truncate(&run.haul_short(), 90)
            ),
            Btn::new(ids.go("lab"), "Continue")
                .emoji("labyrinth")
                .primary(),
        );
        primary_used = true;
    }
    if let Some(d) = &p.delve {
        s.section(
            format!(
                "{} **In {}**\n-# Room {} of {}",
                e("dungeon"),
                d.dungeon().name,
                (d.room + 1).min(d.dungeon().rooms.len() + 1),
                d.dungeon().rooms.len() + 1
            ),
            Btn::new(ids.go("dungeon"), "Continue")
                .emoji("dungeon")
                .then_primary(!primary_used),
        );
        primary_used = true;
    }
    match &p.expedition {
        Some(x) if p.now >= x.ends_at => {
            s.section(
                format!(
                    "{} **Back from {}**\n-# {} · {}h · ready to collect",
                    e("expedition"),
                    x.plan.region().name,
                    x.plan.danger.name(),
                    x.plan.hours
                ),
                Btn::new(ids.act("exp", "collect", ""), "Collect")
                    .emoji("expedition")
                    .primary(),
            );
            primary_used = true;
        }
        Some(x) => {
            let f = expedition::forecast(&x.plan, x.power, x.scouting, w.expedition_rates());
            let top: Vec<(String, i64)> = f
                .items
                .into_iter()
                .filter(|(_, n)| *n > 0)
                .take(3)
                .collect();
            s.text(format!(
                "{} **{} · {}** · back in {}\n-# Expected: {}",
                e("expedition"),
                x.plan.region().name,
                x.plan.danger.name(),
                dur(x.ends_at - p.now),
                top.iter()
                    .map(|(id, n)| data::count(id, *n))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        None if open(data::Feature::Expedition) => {
            s.section(format!("{} **No expedition out**\n-# Send one before you go: it works while you're away.", e("expedition")), Btn::new(ids.go("exp"), "Plan one").emoji("expedition"));
        }
        None => {}
    }
    if let Some(f) = &p.fight {
        let what = if f.kind == crate::engine::combat::Kind::Boss {
            "Fighting the Barrow King"
        } else {
            "In a fight"
        };
        s.section(
            format!(
                "{} **{what}**\n-# {} · turn {}",
                e("hunt"),
                f.foe.name,
                f.turn
            ),
            Btn::new(
                ids.go(if f.kind == crate::engine::combat::Kind::Boss {
                    "boss"
                } else {
                    "hunt"
                }),
                "Continue",
            )
            .emoji("hunt"),
        );
    } else {
        let wins = (p.vigor() / data::VIGOR_PER_WIN as f64).floor() as i64;
        let hp = p.hp();
        let max = p.max_hp();
        let saga_ready = quests::chapter(p.saga).is_some() && chapter_done(p);
        let line = if hp < max / 2 {
            format!(
                "{} **Resting** · {hp} / {max} HP\n-# Full in {}. A potion works too.",
                e("hp"),
                dur(p.minutes_to_full() * 60_000)
            )
        } else if wins > 0 {
            format!(
                "{} **Free to hunt**\n-# Vigor doubles your next {wins} win{}",
                e("hunt"),
                if wins == 1 { "" } else { "s" }
            )
        } else {
            format!(
                "{} **Free to hunt**\n-# Vigor builds back while you're away",
                e("hunt")
            )
        };
        s.section(
            line,
            Btn::new(ids.go("hunt"), "Hunt")
                .emoji("hunt")
                .then_primary(!primary_used && !saga_ready),
        );
        primary_used |= !saga_ready;
    }
    if let Some(ch) = quests::chapter(p.saga) {
        if chapter_done(p) {
            s.section(
                format!(
                    "{} **{}** is ready\n-# The next part of your saga",
                    e("saga"),
                    ch.title
                ),
                Btn::new(ids.go("saga"), "Read")
                    .emoji("saga")
                    .then_primary(!primary_used),
            );
        } else {
            let next = saga_next(p, ch);
            s.text(format!("{} **{}**\n-# {next}", e("saga"), ch.title));
        }
    }
    let ready = todays_bounties(p)
        .iter()
        .enumerate()
        .filter(|(i, b)| !p.bounty[*i].1 && p.bounty[*i].0 >= b.goal.target())
        .count();
    if ready > 0 && w.modules.bounties && open(data::Feature::Bounties) {
        s.text(format!(
            "-# {} {ready} bount{} ready to claim",
            e("bounty"),
            if ready == 1 { "y" } else { "ies" }
        ));
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
    // Everything else lives one click down, in a menu per kind of thing.
    let m = &w.modules;
    let mut row = vec![
        Btn::new(ids.go("combat"), "Combat").emoji("boss"),
        Btn::new(ids.go("hero"), "Character").emoji(p.main().id()),
        Btn::new(ids.go("questlog"), "Quests").emoji("quest"),
    ];
    if m.market && open(data::Feature::Market) {
        row.push(Btn::new(ids.go("skagi"), "Skagi").emoji("temple"));
    } else if open(data::Feature::Temple) {
        row.push(Btn::new(ids.go("temple"), "Temple").emoji("temple"));
    }
    if m.guilds && crate::unlocks::usable(p, data::Feature::Guild) {
        row.push(
            Btn::new(
                ids.go("guild"),
                p.clan
                    .as_ref()
                    .map(|c| format!("[{}]", c.tag))
                    .unwrap_or_else(|| "Guild".into()),
            )
            .emoji("guild"),
        );
    }
    s.buttons(row);
    s
}

/// The current chapter's first unmet goal, with progress where it counts up.
fn saga_next(p: &Player, ch: &quests::Chapter) -> String {
    ch.goals
        .iter()
        .enumerate()
        .find(|(i, goal)| {
            goal_now(p, goal, p.saga_progress.get(*i).copied().unwrap_or(0)) < goal.target()
        })
        .map(|(i, goal)| {
            let have = goal_now(p, goal, p.saga_progress.get(i).copied().unwrap_or(0));
            if goal.target() > 1 && !matches!(goal, quests::Goal::Level(_)) {
                format!("{} · {have} / {}", goal.describe(), goal.target())
            } else {
                goal.describe()
            }
        })
        .unwrap_or_default()
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

fn goals(
    p: &Player,
    w: &World,
    givers: &[(u64, String, i64)],
    lab: &[(u64, String, i64)],
) -> Screen {
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    title(
        &mut s,
        "goals",
        "Goals",
        "Midgard · offerings reset on Monday",
    );
    s.text("-# Realm boss");
    let power = p.power();
    let ready = power >= data::BOSS_POWER;
    let mut boss = format!(
        "{} **{}**  ·  needs {} Power\n{}",
        e("boss"),
        data::BOSS_NAME,
        data::BOSS_POWER,
        if p.boss_won {
            format!(
                "{} Beaten. Heroic expeditions and your lesser vow are open.",
                e("check")
            )
        } else if ready {
            format!("You have **{power}**. You're ready.")
        } else {
            format!("You have **{power}**, {} to go", data::BOSS_POWER - power)
        }
    );
    for f in data::FACTS {
        if p.facts.iter().any(|k| k == f) {
            boss.push_str(&format!(
                "\n-# {} {}",
                e("check"),
                data::fact_text(f, w.week)
            ));
        }
    }
    let unknown = data::FACTS.len() - p.facts.len();
    if unknown > 0 {
        boss.push_str(&format!(
            "\n-# {} {unknown} more unknown · scout him in the Barrow Fields",
            e("unknown")
        ));
    }
    s.text(boss);
    s.gap();
    s.text("-# Server offering");
    let lead = data::GODS[w
        .offerings
        .iter()
        .enumerate()
        .max_by_key(|(_, t)| **t)
        .map(|(i, _)| i)
        .unwrap_or(0)];
    let (bname, bdesc) = lead.blessing();
    s.text(format!(
        "{} **{} of {} tribute this week** ({}%)\n-# {} leads: {bname}, {bdesc}, for everyone at 100%.",
        e("offering"),
        thousands(w.total()),
        thousands(w.goal),
        w.pct(),
        lead.name()
    ));
    if let Some(b) = w.blessing {
        let (n, d) = b.blessing();
        s.text(format!(
            "-# {} Active now: {n}, {d}, from last week",
            e("vigor")
        ));
    }
    if let Some(pos) = givers.iter().position(|(u, ..)| *u == p.user) {
        s.text(format!(
            "-# You gave {} this week, {} in the server",
            thousands(givers[pos].2),
            ordinal(pos + 1)
        ));
    }
    s.gap();
    if !lab.is_empty() {
        let list: Vec<String> = lab
            .iter()
            .take(3)
            .enumerate()
            .map(|(i, (u, n, f))| {
                format!(
                    "{}. {}{} · floor {f}",
                    i + 1,
                    if n.is_empty() { "Someone" } else { n },
                    if *u == p.user { " (you)" } else { "" }
                )
            })
            .collect();
        s.text(format!(
            "-# {} Deepest in the Labyrinth this week: {}",
            e("labyrinth"),
            list.join("  ·  ")
        ));
        s.gap();
    }
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
        back_to(&ids, "questlog", "Quests"),
        Btn::new(
            ids.act("boss", "start", ""),
            if p.boss_won {
                "Fight again"
            } else {
                "Fight the Barrow King"
            },
        )
        .emoji("boss")
        .danger()
        .disabled(p.fight.is_some()),
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
    format!(
        "{}.{}.{}.{}",
        p.region,
        p.danger.id(),
        p.hours,
        p.supply.as_deref().unwrap_or("none")
    )
}

fn parse_plan(s: &str) -> Option<Plan> {
    let mut it = s.split('.');
    let region = data::region(it.next()?)?.id.to_owned();
    let danger = Danger::from_id(it.next()?)?;
    let hours: u32 = it.next()?.parse().ok()?;
    let supply = it
        .next()
        .filter(|s| *s != "none" && data::EXPEDITION_SUPPLIES.contains(s))
        .map(str::to_owned);
    data::HOURS.contains(&hours).then_some(Plan {
        region,
        danger,
        hours,
        supply,
    })
}

/// A sensible plan to start from: the best region you can handle on Normal.
fn default_plan(p: &Player) -> Plan {
    let power = p.power();
    let region = data::REGIONS
        .iter()
        .rev()
        .find(|r| power >= r.power)
        .unwrap_or(&data::REGIONS[0]);
    Plan {
        region: region.id.into(),
        danger: Danger::Normal,
        hours: 4,
        supply: (p.have("rations") > 0).then(|| "rations".into()),
    }
}

fn danger_open(p: &Player, d: Danger) -> bool {
    d != Danger::Heroic || p.boss_won
}

fn expedition_plan(p: &Player, w: &World, plan: &Plan) -> Screen {
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
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
                    .desc(format!(
                        "{} · needs {} Power on Normal",
                        r.blurb.split('.').nth(1).map(str::trim).unwrap_or(r.blurb),
                        r.power
                    ))
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
                let pl = Plan {
                    danger: *d,
                    ..plan.clone()
                };
                Opt::new(format!("{} · ×{} loot", d.name(), d.loot()), d.id())
                    .emoji(if *d == Danger::Normal {
                        "check"
                    } else {
                        "warn"
                    })
                    .desc(format!("Needs {} Power here", pl.needs()))
                    .default(*d == plan.danger)
            })
            .collect(),
    );
    s.inner_select(
        format!("rpg:{}:exp:set:hours|{key}:0", p.user),
        "Length",
        data::HOURS
            .iter()
            .map(|h| {
                Opt::new(
                    format!("{h} hour{}", if *h == 1 { "" } else { "s" }),
                    h.to_string(),
                )
                .emoji("clock")
                .default(*h == plan.hours)
            })
            .collect(),
    );
    let mut supplies = vec![Opt::new("No supplies", "none").default(plan.supply.is_none())];
    for id in data::EXPEDITION_SUPPLIES {
        let n = p.have(id);
        if n > 0 {
            let it = data::item(id).unwrap();
            supplies.push(
                Opt::new(format!("{} ({n})", capital(it.name)), id)
                    .emoji(it.emoji)
                    .desc(it.used_for)
                    .default(plan.supply.as_deref() == Some(id)),
            );
        }
    }
    s.inner_select(
        format!("rpg:{}:exp:set:supply|{key}:0", p.user),
        "Supplies",
        supplies,
    );
    let f = expedition::forecast(plan, power, p.scouting(), w.expedition_rates());
    let mut fc = String::from("-# Forecast\n");
    fc.push_str(&format!(
        "{} About {}\n",
        e("pack"),
        f.items
            .iter()
            .filter(|(_, n)| *n > 0)
            .map(|(id, n)| data::count(id, *n))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    fc.push_str(&format!("{} {} gold · {} XP\n", e("gold"), f.gold, f.xp));
    let risk = if f.ratio >= 1.4 {
        format!(
            "{} Low risk: {} Power clears this with room to spare",
            e("check"),
            plan.effective(power)
        )
    } else if f.ratio >= 1.0 {
        format!(
            "{} Some risk: {} Power is just enough",
            e("warn"),
            plan.effective(power)
        )
    } else {
        format!(
            "{} Too dangerous: needs {}, you have {}. You'd come back early and hurt.",
            e("warn"),
            plan.needs(),
            plan.effective(power)
        )
    };
    fc.push_str(&risk);
    if !p.boss_won && p.facts.len() < data::FACTS.len() {
        fc.push_str(&format!(
            "\n{} {}% chance to learn something about the Barrow King",
            e("intel"),
            (f.intel_chance * 100.0).round()
        ));
    }
    fc.push_str(&format!(
        "\n{} +{} tribute to {}'s altar",
        e("offering"),
        f.tribute,
        p.main().name()
    ));
    s.text(fc);
    s.buttons(vec![
        back(&ids),
        Btn::new(
            ids.act("exp", "go", &key),
            format!("Send out · {}h", plan.hours),
        )
        .emoji("expedition")
        .primary()
        .disabled(p.expedition.is_some()),
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
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    title(
        &mut s,
        "expedition",
        &format!("{} · {}", x.plan.region().name, x.plan.danger.name()),
        &format!("{}h expedition", x.plan.hours),
    );
    let done = p.now >= x.ends_at;
    let f = expedition::forecast(&x.plan, x.power, x.scouting, w.expedition_rates());
    if done {
        s.text(format!("{} **Back and waiting for you.**", e("check")));
    } else {
        s.text(format!(
            "{} Back in **{}**",
            e("clock"),
            dur(x.ends_at - p.now)
        ));
    }
    s.text(format!(
        "-# Expected: {}",
        f.items
            .iter()
            .filter(|(_, n)| *n > 0)
            .map(|(id, n)| data::count(id, *n))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    s.text("-# While it runs you can hunt, forge and give at the temple.".to_string());
    let mut b = vec![back(&ids)];
    if done {
        b.push(
            Btn::new(ids.act("exp", "collect", ""), "Collect")
                .emoji("expedition")
                .primary(),
        );
    } else {
        b.push(
            Btn::new(ids.go("hunt"), "Hunt meanwhile")
                .emoji("hunt")
                .primary(),
        );
        b.push(Btn::new(ids.go("exp"), "Refresh"));
    }
    s.buttons(b);
    s
}

fn expedition_report(p: &Player, r: &Report, levels: u32) -> Screen {
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let region = data::region(&r.region)
        .map(|x| x.name)
        .unwrap_or("the wilds");
    let mut s = Screen::default();
    title(
        &mut s,
        "expedition",
        &format!("Back from {region}"),
        &format!("{} · {}h", capital(&r.danger), r.hours),
    );
    let story: Vec<String> = r
        .story
        .iter()
        .map(|(h, l)| format!("-# **{h}**  {l}"))
        .collect();
    s.text(story.join("\n"));
    s.gap();
    s.text("-# Brought back");
    let mut lines = vec![items_line(&r.items)];
    lines.push(format!(
        "{} {} gold  ·  {} XP  ·  +{} tribute",
        e("gold"),
        r.gold,
        r.xp,
        r.tribute
    ));
    if let Some(f) = &r.intel {
        lines.push(format!(
            "{} **New intel:** {}",
            e("intel"),
            data::fact_text(f, p.week())
        ));
    }
    if levels > 0 {
        lines.push(format!(
            "{} **Level {}!**{}",
            e("power"),
            p.level,
            crate::unlocks::opened_line(p, &store::modules(p.guild), p.level - levels)
        ));
    }
    if r.wounded {
        lines.push(format!(
            "{} You came back hurt: rest, or drink a potion before a fight.",
            e("hp")
        ));
    }
    s.text(lines.join("\n"));
    let again = r.region.clone() + "." + &r.danger + "." + &r.hours.to_string();
    s.buttons(vec![
        back(&ids),
        Btn::new(ids.act("exp", "again", &again), "Send again")
            .emoji("expedition")
            .primary()
            .disabled(p.expedition.is_some()),
        Btn::new(ids.go("exp"), "Plan new"),
        Btn::new(ids.go("hunt"), "Hunt").emoji("hunt"),
    ]);
    s
}

fn hunt_pick(p: &Player, region: Option<&str>) -> Screen {
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    let power = p.power();
    let pick = region.and_then(data::region).unwrap_or_else(|| {
        data::REGIONS
            .iter()
            .rev()
            .find(|r| power as f64 >= r.power as f64 * 1.3)
            .unwrap_or(&data::REGIONS[0])
    });
    title(
        &mut s,
        "hunt",
        "Hunt",
        "Fight one foe at a time, turn by turn",
    );
    let wins = (p.vigor() / data::VIGOR_PER_WIN as f64).floor() as i64;
    let (hp, max) = (p.hp(), p.max_hp());
    s.text(format!(
        "{} {hp} / {max} HP{}\n{} Vigor {:.0}%: {}",
        e("hp"),
        if hp < max {
            format!(" · full in {}", dur(p.minutes_to_full() * 60_000))
        } else {
            String::new()
        },
        e("vigor"),
        p.vigor(),
        if wins > 0 {
            format!(
                "your next {wins} win{} pay double",
                if wins == 1 { "" } else { "s" }
            )
        } else {
            "builds back while you're away".into()
        }
    ));
    s.inner_select(
        format!("rpg:{}:hunt:pick::0", p.user),
        "Where to hunt",
        data::REGIONS
            .iter()
            .map(|r| {
                let (xp, gold) = data::hunt_reward(r);
                Opt::new(r.name, r.id)
                    .emoji(r.id)
                    .desc(format!(
                        "Comfortable at {}+ Power · {xp} XP, {gold} gold a win",
                        (r.power as f64 * 1.3) as i64
                    ))
                    .default(r.id == pick.id)
            })
            .collect(),
    );
    encounter_block(&mut s, p, &ids);
    let mons: Vec<String> = pick
        .monsters
        .iter()
        .filter_map(|m| data::monster(m))
        .map(|m| m.name.to_owned())
        .collect();
    s.text(format!(
        "-# {}: {}. Wins drop {}.",
        pick.name,
        mons.join(", "),
        pick.drops
            .iter()
            .map(|(id, _)| data::item(id).map(|i| i.plural).unwrap_or(id))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    let mut b = vec![
        back(&ids),
        Btn::new(ids.act("hunt", "start", pick.id), "Start hunting")
            .emoji("hunt")
            .primary()
            .disabled(hp * 10 < max * 3),
    ];
    if hp < max && p.have("potion") > 0 {
        b.push(
            Btn::new(
                ids.act("hunt", "drink", ""),
                format!("Potion · {}", p.have("potion")),
            )
            .emoji("potion"),
        );
    }
    if hp < max {
        let cost = inn_cost(p);
        b.push(
            Btn::new(
                ids.act("hunt", "inn", ""),
                format!("Rest at the inn · {cost} gold"),
            )
            .emoji("hp")
            .disabled(p.gold < cost),
        );
    }
    s.buttons(b);
    s
}

fn inn_cost(p: &Player) -> i64 {
    data::INN_GOLD_PER_LEVEL * p.level as i64
}

fn pips(n: i32) -> String {
    (0..crate::engine::combat::MAX_STAMINA)
        .map(|i| if i < n { e("pip") } else { e("pipoff") })
        .collect::<Vec<_>>()
        .join("")
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
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let boss = f.kind == crate::engine::combat::Kind::Boss;
    let lab = f.kind == crate::engine::combat::Kind::Lab;
    let dungeon = f.kind == crate::engine::combat::Kind::Dungeon;
    let shared = f.kind == crate::engine::combat::Kind::Shared;
    let shared_kind = if f.region.starts_with("raid") {
        "raid"
    } else {
        "titan"
    };
    let screen = if boss {
        "boss"
    } else if lab {
        "lab"
    } else if dungeon {
        "dungeon"
    } else if shared {
        shared_kind
    } else {
        "hunt"
    };
    let mut s = Screen::default();
    let meta = if boss {
        format!("Phase {} · turn {}", f.foe.phase, f.turn)
    } else if lab {
        format!(
            "The Labyrinth · floor {} · turn {}",
            p.run.as_ref().map(|r| r.floor).unwrap_or(0),
            f.turn
        )
    } else if dungeon {
        format!(
            "{} · turn {}",
            p.delve
                .as_ref()
                .map(|d| d.dungeon().name)
                .unwrap_or("A dungeon"),
            f.turn
        )
    } else if shared {
        format!(
            "{} · turn {} of {} · you've dealt {}",
            if shared_kind == "raid" {
                "Guild raid"
            } else {
                "Titan"
            },
            f.turn.min(data::SHARED_TURNS),
            data::SHARED_TURNS,
            thousands(f.dealt())
        )
    } else {
        format!(
            "{} · turn {}",
            data::region(&f.region).map(|r| r.name).unwrap_or(""),
            f.turn
        )
    };
    title(
        &mut s,
        if boss || shared {
            "boss"
        } else if lab {
            "labyrinth"
        } else if dungeon {
            "dungeon"
        } else {
            "hunt"
        },
        &f.foe.name,
        &meta,
    );
    if !p.tutorial_has(Tut::Hunt) && f.end.is_none() && f.turn <= 3 {
        s.text(format!(
            "-# {} **How fights work:** each turn you act, then the foe does what it showed. A warning means a big hit is coming: **Guard** it. Strike costs 1 stamina; you get 1 back each turn, 2 when you guard.",
            e("help")
        ));
    }
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
        foe.push_str(&format!(
            "\n{} {t}{}",
            if warn { e("warn") } else { e("clock") },
            if scouted { "  ·  scouted" } else { "" }
        ));
        let up = f.upcoming();
        if !up.is_empty() {
            foe.push_str(&format!(
                "\n-# {} Then: {}",
                e("owl"),
                up.iter()
                    .map(|m| move_name(*m))
                    .collect::<Vec<_>>()
                    .join(", then ")
            ));
        }
    }
    s.text(foe);
    let mut you = format!(
        "{} **{}** / {} HP\n{} {} stamina",
        e("hp"),
        f.hp,
        f.max_hp,
        pips(f.stamina),
        f.stamina
    );
    if let Some((el, n)) = f.coat {
        you.push_str(&format!(
            "  ·  {} oil on your next {n} hit{}",
            el.id(),
            if n == 1 { "" } else { "s" }
        ));
    }
    if f.hero.main == God::Thor && f.charge > 0 {
        you.push_str(&format!("  ·  Mjolnir charged ×{}", f.charge));
    }
    s.text(you);
    if !f.log.is_empty() {
        s.text(format!(
            "-# Last turn\n{}",
            f.log
                .iter()
                .map(|l| format!("-# {l}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    if let Some(r) = result {
        s.gap();
        s.text(r);
    }
    if f.end.is_some() {
        encounter_block(&mut s, p, &ids);
        if boss {
            s.buttons(vec![
                back(&ids),
                Btn::new(ids.go("goals"), "Goals").emoji("goals"),
            ]);
        } else if lab {
            let on = p.run.is_some();
            s.buttons(vec![
                back(&ids),
                Btn::new(
                    ids.go("lab"),
                    if on { "Choose a door" } else { "The Labyrinth" },
                )
                .emoji("labyrinth")
                .primary(),
            ]);
        } else if dungeon {
            let on = p.delve.is_some();
            s.buttons(vec![
                back(&ids),
                Btn::new(
                    ids.go(if on { "dungeon" } else { "dungeons" }),
                    if on { "Onward" } else { "Dungeons" },
                )
                .emoji("dungeon")
                .primary(),
            ]);
        } else if shared {
            s.buttons(vec![
                back(&ids),
                Btn::new(
                    ids.go(shared_kind),
                    if shared_kind == "raid" {
                        "The raid"
                    } else {
                        "The Titan"
                    },
                )
                .emoji("boss")
                .primary(),
            ]);
        } else {
            let again = Btn::new(ids.act("hunt", "start", &f.region), "Hunt again")
                .emoji("hunt")
                .primary()
                .disabled(p.hp() * 10 < p.max_hp() * 3);
            s.buttons(vec![back(&ids), again]);
        }
        return s;
    }
    let potions = p.have("potion");
    let oils = (p.have("fireoil"), p.have("frostoil"));
    let act = |a: &str| ids.act(screen, "act", a);
    let mut row1 = vec![
        Btn::new(act("strike"), "Strike · 1")
            .emoji("hunt")
            .primary()
            .disabled(f.blocked(Action::Strike, potions, oils).is_some()),
    ];
    for sk in f.hero.skills() {
        let blocked = f.blocked(Action::Skill(SkillId::of(sk)), potions, oils);
        let cd = f.cooldown(sk);
        let label = if cd > 0 {
            format!("{} · in {cd}", sk.name())
        } else {
            format!("{} · {}", sk.name(), sk.stamina())
        };
        row1.push(
            Btn::new(act(sk.id()), label)
                .emoji(skill_emoji(sk))
                .disabled(blocked.is_some()),
        );
    }
    row1.push(Btn::new(act("guard"), "Guard").emoji("guard"));
    let mut row2 = Vec::new();
    if row1.len() > 5 {
        row2.push(row1.pop().unwrap());
    }
    row2.insert(
        0,
        Btn::new(act("potion"), format!("Potion · {potions}"))
            .emoji("potion")
            .disabled(potions < 1),
    );
    if oils.0 > 0 {
        row2.push(Btn::new(act("fireoil"), format!("Fire oil · {}", oils.0)).emoji("fireoil"));
    }
    if oils.1 > 0 {
        row2.push(Btn::new(act("frostoil"), format!("Frost oil · {}", oils.1)).emoji("frostoil"));
    }
    row2.push(Btn::new(
        ids.act(screen, "leave", ""),
        if boss {
            "Retreat"
        } else if lab || dungeon {
            "Flee"
        } else if shared {
            "Break off"
        } else {
            "Leave"
        },
    ));
    s.buttons(row1);
    s.buttons(row2);
    s
}

/// The next piece per slot: the first tier above what you wear (or the first one, if
/// nothing), whether or not you can make it yet.
fn next_piece(p: &Player, slot: Slot) -> Option<&'static data::Gear> {
    let worn = p.equipped(slot).map(|g| g.gear.level).unwrap_or(0);
    data::GEAR
        .iter()
        .filter(|g| g.slot == slot && g.level > worn && (!g.after_boss || p.boss_won))
        .min_by_key(|g| g.level)
}

/// Why a piece can't be forged yet: level, then smith rank.
fn forge_lock(p: &Player, g: &data::Gear) -> Option<String> {
    let mut why = Vec::new();
    if g.level > p.level {
        why.push(format!("level {}", g.level));
    }
    if g.smith > p.smith_rank() {
        why.push(format!("smith rank {}", g.smith));
    }
    (!why.is_empty()).then(|| format!("{} {}", e("lock"), why.join(" · ")))
}

fn forge_view(g: &Game) -> Result<Screen> {
    Ok(forge(&g.p, crate::orders::done(g)?))
}

fn forge(p: &Player, order_done: bool) -> Screen {
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    let rank = p.smith_rank();
    title(
        &mut s,
        "forge",
        "Forge",
        &format!("Smith rank {rank} · {} gold", thousands(p.gold)),
    );
    if let Some(left) = data::smith_to_next(p.smith_xp) {
        s.text(format!(
            "-# Rank {} in {left} smithing: {}",
            rank + 1,
            data::smith_perk(rank + 1)
        ));
    }
    // Per slot: the next tier, with what it takes as have / need on one line.
    let mut shown = 0;
    let mut temper: Vec<Opt> = Vec::new();
    for slot in data::SLOTS {
        let Some(g) = next_piece(p, slot) else {
            continue;
        };
        shown += 1;
        let worn = p.equipped(slot).map(|x| x.power()).unwrap_or(0);
        let mut cost: Vec<String> = g
            .cost
            .iter()
            .map(|(id, n)| format!("{}/{}", p.have(id).min(*n), data::count(id, *n)))
            .collect();
        cost.push(format!("{} gold", g.gold));
        let lock = forge_lock(p, g);
        let cost = match &lock {
            Some(l) => format!("{l} · {}", cost.join(" · ")),
            None => cost.join(" · "),
        };
        let can =
            lock.is_none() && p.gold >= g.gold && g.cost.iter().all(|(id, n)| p.have(id) >= *n);
        let diff = data::gear_power(g, 0) - worn;
        s.section(
            format!(
                "{} **{}**  {}{} Power\n-# {cost}",
                e(slot_emoji(slot)),
                g.name,
                if diff >= 0 { "+" } else { "" },
                diff,
            ),
            Btn::new(ids.act("forge", "gear", g.id), "Forge")
                .emoji("forge")
                .disabled(!can)
                .then_primary(can),
        );
        if lock.is_none() {
            for (i, t) in data::TRAITS.iter().enumerate() {
                if !t.fits(slot) {
                    continue;
                }
                let cat = data::catalyst(*t);
                // The catalyst on top of whatever the recipe already takes of it.
                let n = data::temper_cost(g)
                    + g.cost.iter().find(|c| c.0 == cat).map(|c| c.1).unwrap_or(0);
                let (lo, hi) = t.range();
                let mid = (lo + hi + 1) / 2;
                temper.push(
                    Opt::new(
                        format!("{} · {}", g.name, t.name()),
                        format!("{}.{i}", g.id),
                    )
                    .emoji(cat)
                    .desc(crate::ui::truncate(
                        &format!(
                            "{} · at least Fine · {}/{}",
                            if lo == hi {
                                t.describe(lo)
                            } else {
                                t.describe(mid)
                                    .replace(&mid.to_string(), &format!("{mid}-{hi}"))
                            },
                            p.have(cat).min(n),
                            data::count(cat, n)
                        ),
                        100,
                    )),
                );
            }
        }
    }
    if shown == 0 {
        s.text(format!(
            "{} You wear the best gear Midgard has until the Barrow King falls.",
            e("check")
        ));
    }
    // Sigrun's order: the day's job for the forge.
    let o = crate::orders::today(p);
    if order_done {
        s.text(format!(
            "-# {} Sigrun's order is done for today",
            e("check")
        ));
    } else {
        let (have, need) = o.have(p);
        s.section(
            format!(
                "{} **Sigrun's order:** {}  ·  {}/{need}\n-# {}",
                e(o.emoji()),
                o.describe(),
                have.min(need),
                o.pays()
            ),
            Btn::new(ids.act("forge", "order", ""), "Hand over")
                .emoji("check")
                .disabled(have < need),
        );
    }
    if !temper.is_empty() {
        s.inner_select(
            ids.act("forge", "temper", ""),
            "Temper as you forge: choose a trait…",
            temper.into_iter().take(25).collect(),
        );
    }
    let ups: Vec<Opt> = data::SLOTS
        .iter()
        .filter_map(|sl| p.equipped(*sl))
        .filter(|g| g.upgrade < g.cap(rank))
        .map(|g| {
            let (gold, mat, n, threads) = data::upgrade_cost(g.gear, g.upgrade);
            let mut cost = format!("{gold} gold, {}", data::count(mat, n));
            if threads > 0 {
                cost.push_str(&format!(", {}", data::count("thread", threads)));
            }
            Opt::new(
                format!("{} → +{}", g.label(), g.upgrade + 1),
                g.id.to_string(),
            )
            .emoji(slot_emoji(g.gear.slot))
            .desc(format!(
                "+{} Power · {cost}",
                data::gear_power(g.gear, g.upgrade + 1) * (100 + g.rarity.power_pct()) / 100
                    - g.power(),
            ))
        })
        .collect();
    s.inner_select(
        ids.act("forge", "upgrade", ""),
        format!(
            "Upgrade what you wear: your work to +{}, found gear to +{}…",
            data::upgrade_cap(rank, true),
            data::upgrade_cap(rank, false)
        ),
        ups,
    );
    if p.have("thread") > 0 {
        let refine: Vec<Opt> = p
            .gear
            .iter()
            .filter(|g| g.rarity != data::Rarity::Epic)
            .take(25)
            .map(|g| {
                let to = data::Rarity::from_index(g.rarity.index() + 1);
                let (t, gold) = data::refine_cost(to, p.level);
                Opt::new(
                    format!(
                        "{} → {}{}",
                        g.label(),
                        to.name(),
                        if g.equipped { " (worn)" } else { "" }
                    ),
                    g.id.to_string(),
                )
                .emoji(to.emoji())
                .desc(format!(
                    "Adds a trait · {} and {gold} gold",
                    data::count("thread", t)
                ))
            })
            .collect();
        s.text(format!(
            "-# {} {} for refining and reforging",
            e("thread"),
            data::count("thread", p.have("thread"))
        ));
        s.inner_select(
            ids.act("forge", "refine", ""),
            "Refine: raise a piece's rarity…",
            refine,
        );
        let reforge: Vec<Opt> = p
            .gear
            .iter()
            .filter(|g| g.rarity != data::Rarity::Common)
            .take(25)
            .map(|g| {
                let (t, gold) = data::reforge_cost(g.rarity, p.level);
                Opt::new(
                    format!("{}{}", g.label(), if g.equipped { " (worn)" } else { "" }),
                    g.id.to_string(),
                )
                .emoji(g.rarity.emoji())
                .desc(format!(
                    "New traits · {} and {gold} gold · now: {}",
                    data::count("thread", t),
                    crate::ui::truncate(&g.traits_text(), 50)
                ))
            })
            .collect();
        s.inner_select(
            ids.act("forge", "reforge", ""),
            "Reforge: new traits on a piece…",
            reforge,
        );
    }
    let opts: Vec<Opt> = data::SUPPLY_RECIPES
        .iter()
        .map(|r| {
            let it = data::item(r.makes).unwrap();
            let cost = r
                .cost
                .iter()
                .map(|(id, n)| data::count(id, *n))
                .collect::<Vec<_>>()
                .join(", ");
            let can = p.gold >= r.gold && r.cost.iter().all(|(id, n)| p.have(id) >= *n);
            Opt::new(
                format!(
                    "{} ×{}{}",
                    capital(it.plural),
                    r.qty,
                    if can { "" } else { " (can't afford)" }
                ),
                r.id,
            )
            .emoji(it.emoji)
            .desc(format!(
                "{} · {cost}, {} gold · have {}",
                it.used_for,
                r.gold,
                p.have(r.makes)
            ))
        })
        .collect();
    s.inner_select(ids.act("forge", "make", ""), "Make supplies…", opts);
    s.buttons(vec![
        back_to(&ids, "hero", "Character"),
        Btn::new(ids.go("pack"), "Pack").emoji("pack"),
    ]);
    s
}

/// Forge a piece, plain or tempered with a chosen trait.
fn forge_piece(
    g: &mut Game,
    gear: &'static data::Gear,
    temper: Option<data::Trait>,
    announce: &mut Option<String>,
) -> Result<Option<String>> {
    if let Some(l) = forge_lock(&g.p, gear) {
        return Ok(Some(format!("Not yet: {l}.")));
    }
    if gear.after_boss && !g.p.boss_won {
        return Ok(Some("That needs the Barrow King's crown shards.".into()));
    }
    let mut cost: Vec<(&str, i64)> = gear.cost.to_vec();
    if let Some(t) = temper {
        if !t.fits(gear.slot) {
            return Ok(None);
        }
        let cat = data::catalyst(t);
        let n = data::temper_cost(gear);
        match cost.iter_mut().find(|c| c.0 == cat) {
            Some(c) => c.1 += n,
            None => cost.push((cat, n)),
        }
    }
    if !g.can_pay(&cost, gear.gold) {
        return Ok(Some(format!(
            "You need {} and {} gold.",
            cost.iter()
                .map(|(id, n)| data::count(id, *n))
                .collect::<Vec<_>>()
                .join(", "),
            gear.gold
        )));
    }
    g.pay(&cost, gear.gold, "forged gear")?;
    let rank = g.p.smith_rank();
    let mut rng = crate::engine::Rng::new(seed(&g.p, 71));
    let (rarity, traits) = match temper {
        Some(t) => crate::engine::loot::temper(gear, rank, t, &mut rng),
        None => crate::engine::loot::forge(gear, rank, &mut rng),
    };
    let power = data::gear_power(gear, 0) + data::gear_power(gear, 0) * rarity.power_pct() / 100;
    let worn = g.p.equipped(gear.slot).map(|x| x.power()).unwrap_or(0);
    let wear = power > worn;
    g.add_piece(gear, rarity, traits.clone(), wear, "forged")?;
    let up = g.smith(data::smith_xp_forge(gear) * if temper.is_some() { 3 } else { 2 } / 2)?;
    if rarity == data::Rarity::Epic {
        *announce = Some(format!(
            "{} **{}** forged an **Epic {}** at Skagi's forge.",
            e("epic"),
            g.p.name,
            gear.name
        ));
    }
    count_event(g, quests::Event::Forged)?;
    tutorial_done(g, Tut::Forge)?;
    g.bump()?;
    let t: Vec<String> = traits
        .iter()
        .map(|(t, v)| format!("{} ({})", t.name(), t.describe(*v)))
        .collect();
    Ok(Some(format!(
        "It came out **{}**: {}{}.{}{}",
        rarity.name(),
        gear.name,
        if t.is_empty() {
            String::new()
        } else {
            format!(", {}", t.join(", "))
        },
        if wear {
            " You're wearing it."
        } else {
            " It's in your pack."
        },
        rank_line(up)
    )))
}

/// "Smith rank 4: upgrades to +3, ..." when a rank was just reached.
pub(crate) fn rank_line(up: Option<u32>) -> String {
    match up {
        Some(r) => format!(
            "\n{} **Smith rank {r}!** {}{}",
            e("forge"),
            capital(&data::smith_perk(r)),
            data::smith_title(r)
                .map(|t| format!(". A new title to wear: **{t}**"))
                .unwrap_or_default()
        ),
        None => String::new(),
    }
}

pub(crate) trait ThenPrimary {
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
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    let gear_power = p.power() - data::level_power(p.level);
    title(
        &mut s,
        "pack",
        "Pack",
        &format!(
            "Level {} gives {} Power, gear {}",
            p.level,
            data::level_power(p.level),
            gear_power
        ),
    );
    s.text("-# Wearing");
    let mut wear = Vec::new();
    for slot in data::SLOTS {
        match p.equipped(slot) {
            Some(g) => {
                wear.push(format!(
                    "{} **{}** · {}  ·  +{} Power{}{}",
                    e(slot_emoji(slot)),
                    g.label(),
                    rarity_tag(g.rarity),
                    g.power(),
                    if g.gear.scouting > 0 {
                        format!(", +{}% scouting", g.gear.scouting)
                    } else {
                        String::new()
                    },
                    if g.forged {
                        String::new()
                    } else {
                        format!("  ·  found, upgrades to +{}", data::FOUND_UPGRADE)
                    }
                ));
                if !g.traits.is_empty() {
                    wear.push(format!("-# {}", g.traits_text()));
                }
            }
            None => wear.push(format!(
                "{} {}: nothing yet",
                e(slot_emoji(slot)),
                slot.name()
            )),
        }
    }
    s.text(wear.join("\n"));
    let mats: Vec<String> = data::ITEMS
        .iter()
        .filter(|i| i.kind == Kind::Material && p.have(i.id) > 0)
        .map(|i| {
            format!(
                "{} {}  ·  -# for {}",
                e(i.emoji),
                data::count(i.id, p.have(i.id)),
                i.used_for
            )
            .replace("  ·  -# ", "  ·  ")
        })
        .collect();
    if !mats.is_empty() {
        s.text("-# Materials");
        s.text(mats.join("\n"));
    }
    let sup: Vec<String> = data::ITEMS
        .iter()
        .filter(|i| {
            matches!(i.kind, Kind::Supply | Kind::Combat | Kind::Trophy) && p.have(i.id) > 0
        })
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
        .take(25)
        .map(|g| {
            let worn = p.equipped(g.gear.slot).map(|x| x.power()).unwrap_or(0);
            let diff = g.power() - worn;
            Opt::new(
                format!("{} {}", g.rarity.name(), g.label()),
                g.id.to_string(),
            )
            .emoji(g.rarity.emoji())
            .desc(crate::ui::truncate(
                &format!(
                    "{} · {}{} vs worn · {}",
                    g.gear.slot.name(),
                    if diff >= 0 { "+" } else { "" },
                    diff,
                    if g.traits.is_empty() {
                        "no traits".into()
                    } else {
                        g.traits_text()
                    }
                ),
                100,
            ))
        })
        .collect();
    if !spare.is_empty() {
        s.inner_select(
            ids.act("pack", "equip", ""),
            "Wear something else…",
            spare.clone(),
        );
        s.inner_select(
            ids.act("pack", "salvage", ""),
            "Salvage for gold, half the materials, and threads from Fine or better…",
            spare,
        );
    }
    let mut b = vec![back_to(&ids, "hero", "Character")];
    if crate::unlocks::open(p, data::Feature::Forge) {
        b.push(Btn::new(ids.go("forge"), "Forge").emoji("forge"));
    }
    s.buttons(b);
    s
}

fn temple(p: &Player, w: &World, givers: &[(u64, String, i64)]) -> Screen {
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    title(
        &mut s,
        "temple",
        "Temple of the Gods",
        "Resets every Monday",
    );
    s.text(format!(
        "**{} of {} tribute this week** ({}%)\n-# At 100%, everyone gets the blessing of the god given the most.",
        thousands(w.total()),
        thousands(w.goal),
        w.pct()
    ));
    if let Some(b) = w.blessing {
        let (n, d) = b.blessing();
        s.text(format!("{} Active now: **{n}**, {d}", e("vigor")));
    }
    s.text("-# Altars");
    for g in data::GODS {
        let bond = p.bond(g);
        let mut line = format!(
            "{} **{}** · bond {bond}{}",
            e(g.id()),
            g.name(),
            if p.patron == Some(g) {
                " · your patron"
            } else if p.lesser == Some(g) {
                " · lesser vow"
            } else {
                ""
            }
        );
        // One short line: how far to the next bond, and what the patron's next bond unlocks.
        // What a god gives as a lesser vow lives on the patron screen.
        let mut bits = Vec::new();
        if let Some(n) = progress::to_next_bond(p.tribute[g as usize], data::BOND_CAP) {
            bits.push(format!("{} to bond {}", thousands(n), bond + 1));
        } else {
            bits.push("Midgard's cap".to_string());
        }
        if p.patron == Some(g) && bond < 2 {
            bits.push(format!("bond 2 unlocks {}", g.skills()[1].name()));
        }
        let week = w.offerings[g as usize];
        if week > 0 {
            bits.push(format!("{} this week", thousands(week)));
        }
        line.push_str(&format!("\n-# {}", bits.join(" · ")));
        let b = Btn::new(ids.go(&format!("give{}", g.id())), "Give").emoji("offering");
        s.section(line, if p.patron == Some(g) { b.primary() } else { b });
    }
    if !givers.is_empty() {
        let list: Vec<String> = givers
            .iter()
            .enumerate()
            .map(|(i, (u, n, t))| {
                format!(
                    "-# {}. {}{} · {}",
                    i + 1,
                    if n.is_empty() { "Someone" } else { n },
                    if *u == p.user { " (you)" } else { "" },
                    thousands(*t)
                )
            })
            .collect();
        s.text(format!("-# Most generous this week\n{}", list.join("\n")));
    }
    s.buttons(vec![
        back_to(&ids, "skagi", "Skagi"),
        Btn::new(ids.go("patron"), "Patron and skills").emoji(p.main().id()),
    ]);
    s
}

/// Spare materials: anything past 20 of each.
fn spare_materials(p: &Player) -> Vec<(&'static str, i64, i64)> {
    data::ITEMS
        .iter()
        .filter(|i| i.kind == Kind::Material && p.have(i.id) > 20)
        .map(|i| (i.id, p.have(i.id) - 20, (p.have(i.id) - 20) * i.tribute))
        .collect()
}

fn give(p: &Player, g: God) -> Screen {
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    title(
        &mut s,
        g.id(),
        &format!("Give to {}", g.name()),
        &format!("Bond {}", p.bond(g)),
    );
    let t = p.tribute[g as usize];
    s.text(match progress::to_next_bond(t, data::BOND_CAP) {
        Some(n) => format!(
            "You've given {} in all. **{}** more for bond {}.",
            thousands(t),
            thousands(n),
            p.bond(g) + 1
        ),
        None => format!(
            "You've given {} in all. Bond {} is Midgard's cap: gifts still count for the server.",
            thousands(t),
            p.bond(g)
        ),
    });
    let spare = spare_materials(p);
    let worth: i64 = spare.iter().map(|x| x.2).sum();
    s.text(format!(
        "-# Spare materials (past 20 of each): {}",
        if spare.is_empty() {
            "none right now".into()
        } else {
            format!(
                "{} · worth {worth} tribute",
                spare
                    .iter()
                    .map(|(id, n, _)| data::count(id, *n))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    ));
    s.buttons(vec![
        back_to(&ids, "temple", "Temple"),
        Btn::new(
            ids.act("temple", "give", &format!("{}.gold100", g.id())),
            "100 gold",
        )
        .emoji("gold")
        .disabled(p.gold < 100),
        Btn::new(
            ids.act("temple", "give", &format!("{}.gold500", g.id())),
            "500 gold",
        )
        .emoji("gold")
        .disabled(p.gold < 500),
        Btn::new(
            ids.act("temple", "give", &format!("{}.spare", g.id())),
            "Spare materials",
        )
        .emoji("pack")
        .primary()
        .disabled(spare.is_empty()),
    ]);
    s
}

fn patron_screen(p: &Player) -> Screen {
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let main = p.main();
    let mut s = Screen::default();
    title(
        &mut s,
        main.id(),
        &format!("Sworn to {}", main.name()),
        &format!("Bond {}", p.bond(main)),
    );
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
        let state = if have.contains(&sk) {
            format!("{} · {} stamina", e("check"), sk.stamina())
        } else if sk == main.skills()[1] {
            format!("{} bond 2 with {}", e("lock"), main.name())
        } else {
            format!("{} locked", e("lock"))
        };
        lines.push(format!(
            "{} **{}** {state}\n-# {}",
            e(skill_emoji(sk)),
            sk.name(),
            sk.describe(main)
        ));
    }
    s.text(lines.join("\n"));
    s.gap();
    match (p.boss_won, p.lesser) {
        (false, _) => {
            s.text(format!(
                "-# Lesser vow\n{} After the Barrow King: a second god's passive at half strength, and a merged skill.",
                e("lock")
            ));
        }
        (true, None) => {
            s.text("-# Lesser vow\nChoose a second god. You get their passive at half strength, and your pair's merged skill once you have bond 1 with them.");
            let opts = data::GODS
                .iter()
                .filter(|g| **g != main)
                .map(|g| {
                    Opt::new(g.name(), g.id()).emoji(g.id()).desc(format!(
                        "{} · {}",
                        g.lesser_passive(),
                        data::pair_skill(main, *g).map(|s| s.name()).unwrap_or("")
                    ))
                })
                .collect();
            s.inner_select(ids.act("patron", "vow", ""), "Take a lesser vow…", opts);
        }
        (true, Some(l)) => {
            s.text(format!(
                "-# Lesser vow\n{} **{}** · {}\n-# Bond {} with {}",
                e(l.id()),
                l.name(),
                l.lesser_passive(),
                p.bond(l),
                l.name()
            ));
        }
    }
    if !p.titles.is_empty() {
        s.gap();
        s.text("-# Title");
        let opts = p
            .titles
            .iter()
            .map(|t| {
                Opt::new(t.clone(), t.clone())
                    .emoji("elite")
                    .default(p.title.as_deref() == Some(t.as_str()))
            })
            .collect();
        s.inner_select(ids.act("patron", "title", ""), "Wear a title…", opts);
    }
    s.gap();
    let opts = data::GODS
        .iter()
        .filter(|g| **g != main)
        .map(|g| {
            Opt::new(
                format!(
                    "Swear to {} · {} gold",
                    g.name(),
                    thousands(data::PATRON_CHANGE_GOLD)
                ),
                g.id(),
            )
            .emoji(g.id())
            .desc(g.pitch())
        })
        .collect();
    s.inner_select(
        ids.act("patron", "change", ""),
        "Change patron (your bonds stay)…",
        opts,
    );
    s.buttons(vec![
        back_to(&ids, "hero", "Character"),
        Btn::new(ids.go("temple"), "Temple").emoji("temple"),
    ]);
    s
}

fn help(p: &Player) -> Screen {
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    title(&mut s, "help", "How to play", "The short version");
    use data::Feature as F;
    // What's open, then what's still to come: one line each.
    let lines: [(Option<F>, &str, String); 9] = [
        (
            None,
            "hunt",
            "**Hunts** go turn by turn: guard the heavy blows, strike when it's open.".into(),
        ),
        (
            None,
            "saga",
            "**Your saga** is the story, told in chapters you finish by playing.".into(),
        ),
        (
            Some(F::Expedition),
            "expedition",
            "**Expeditions** work while you're away. The forecast shows what comes back.".into(),
        ),
        (
            Some(F::Forge),
            "forge",
            "**The forge** makes all your gear. Forging raises your smith rank: better odds, further upgrades, new recipes. Temper a piece with a rare material to choose its trait.".into(),
        ),
        (
            Some(F::Bounties),
            "bounty",
            "**Bounties** are three small jobs a day.".into(),
        ),
        (
            Some(F::Temple),
            "temple",
            "**The temple** builds your bond with a god, which opens skills.".into(),
        ),
        (
            Some(F::Labyrinth),
            "labyrinth",
            "**The Labyrinth** goes floor by floor. Leave any time with your haul.".into(),
        ),
        (
            Some(F::Market),
            "market",
            "**The market** sells materials for what you'll forge next.".into(),
        ),
        (
            None,
            "boss",
            format!(
                "**The Barrow King** needs {} Power. Expeditions to the Barrow Fields scout him.",
                data::BOSS_POWER
            ),
        ),
    ];
    let mut open = Vec::new();
    let mut later = Vec::new();
    for (f, icon, line) in lines {
        match f {
            Some(f) if !crate::unlocks::open(p, f) => {
                let u = data::unlock(f);
                later.push(format!("{} (level {})", u.name, u.level));
            }
            _ => open.push(format!("{} {line}", e(icon))),
        }
    }
    open.push(
        "-# Midgard caps level, gear and bonds: playing more gets you there sooner, not stronger."
            .into(),
    );
    s.text(open.join("\n"));
    if !later.is_empty() {
        s.text(format!(
            "-# {} Still to come: {}",
            e("lock"),
            later.join(", ")
        ));
    }
    let mut b = vec![back_to(&ids, "hero", "Character")];
    if p.tutorial < Tut::ALL {
        b.push(Btn::new(ids.act("tut", "skip", ""), "Skip the tutorial"));
    }
    s.buttons(b);
    s
}

// ---------------------------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------------------------

pub(crate) fn seed(p: &Player, salt: u64) -> u64 {
    (p.now as u64).wrapping_mul(6364136223846793005)
        ^ p.user.rotate_left(17)
        ^ (p.version as u64).wrapping_mul(0x9E37)
        ^ salt
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
    let length = plan.hours as i64 * 3_600_000 / store::speed(g.p.guild);
    let e = Expedition {
        power: g.p.power(),
        scouting: g.p.scouting(),
        started_at: now,
        ends_at: now + length,
        seed: seed(&g.p, 11),
        plan,
    };
    g.start_expedition(e)?;
    tutorial_done(g, Tut::Expedition)?;
    g.bump()?;
    Ok(None)
}

fn collect(g: &mut Game, w: &World) -> Result<Option<(Report, u32)>> {
    let Some(x) = g.p.expedition.clone() else {
        return Ok(None);
    };
    if g.p.now < x.ends_at {
        return Ok(None);
    }
    let pity = g.pity(&x.plan.region)?;
    let facts = if g.p.boss_won {
        data::FACTS.iter().map(|s| s.to_string()).collect()
    } else {
        g.p.facts.clone()
    };
    let r = expedition::run(
        &x.plan,
        x.power,
        x.scouting,
        &facts,
        pity,
        w.expedition_rates(),
        x.seed,
    );
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
    count_event(
        g,
        quests::Event::Expedition {
            region: &x.plan.region,
        },
    )?;
    tutorial_done(g, Tut::Collect)?;
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
fn fight_turn(
    g: &mut Game,
    w: &World,
    a: Action,
) -> Result<(Option<String>, Option<String>, Option<String>)> {
    let Some(mut f) = g.p.fight.clone() else {
        return Ok((Some("That fight is over.".into()), None, None));
    };
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
        Some(_) if f.kind == crate::engine::combat::Kind::Shared => {
            let hp = if f.end == Some(End::Lost) { 1 } else { f.hp };
            g.set_hp(hp)?;
            g.save_fight(None)?;
            let (said, ann) = crate::shared::settle(g, &f)?;
            count_event(g, quests::Event::Shared)?;
            result = Some(said);
            announce = ann;
            g.p.fight = Some(f);
        }
        Some(End::Won) | Some(End::Lost) if f.kind == crate::engine::combat::Kind::Dungeon => {
            g.save_fight(None)?;
            let (said, ann) = crate::dungeons::fight_over(g, &f, f.end == Some(End::Won))?;
            result = Some(said);
            announce = ann;
            g.p.fight = Some(f);
        }
        Some(End::Spent) => {
            g.set_hp(f.hp)?;
            g.save_fight(None)?;
            g.p.fight = Some(f);
        }
        Some(End::Lost) if f.kind == crate::engine::combat::Kind::Lab => {
            g.save_fight(None)?;
            result = Some(crate::labyrinth::fight_over(g, &f, false)?);
            g.p.fight = Some(f);
        }
        Some(End::Lost) => {
            g.set_hp(1)?;
            result = Some(format!(
                "{} **You fell.** You wake at camp with 1 HP. Rest or drink a potion, then try again.",
                e("hp")
            ));
            g.save_fight(None)?;
            g.p.fight = Some(f);
        }
        Some(End::Won) if f.kind == crate::engine::combat::Kind::Lab => {
            g.save_fight(None)?;
            result = Some(crate::labyrinth::fight_over(g, &f, true)?);
            g.p.fight = Some(f);
        }
        Some(End::Won) => {
            let breath = if f.kind == crate::engine::combat::Kind::Hunt {
                (f.max_hp as f64 * data::WIN_HEAL) as i64
            } else {
                0
            };
            g.set_hp(f.hp + breath)?;
            match f.kind {
                crate::engine::combat::Kind::Lab
                | crate::engine::combat::Kind::Dungeon
                | crate::engine::combat::Kind::Shared => {}
                crate::engine::combat::Kind::Hunt => {
                    let r = data::region(&f.region).unwrap_or(&data::REGIONS[0]);
                    let (xp, gold) = data::hunt_reward(r);
                    let double = g.p.vigor() >= data::VIGOR_PER_WIN as f64;
                    let m = if double { 2 } else { 1 };
                    if double {
                        g.vigor(-(data::VIGOR_PER_WIN as f64))?;
                    }
                    let elite = data::monster(&f.foe.id).is_some_and(|x| x.elite);
                    let em = if elite { 3 } else { 1 };
                    let xp = xp * m * em * w.xp_rate() / 100 * w.hunt_mult() / 100;
                    let gold = gold * m * em * w.rates.gold / 100 * w.hunt_mult() / 100;
                    let mut rng = crate::engine::Rng::new(seed(&g.p, 29));
                    let (mat, _) = *rng.pick(r.drops);
                    let mut drops = vec![(mat.to_string(), m * em)];
                    if elite {
                        drops.push((r.rare.0.to_string(), 2));
                    } else if rng.chance(data::HUNT_RARE) {
                        drops.push((r.rare.0.to_string(), 1));
                    }
                    let mut elite_line = String::new();
                    if elite {
                        let first = g.record_kill(&f.foe.id, f.turn)?;
                        if first && let Some(t) = data::elite_title(&f.foe.id) {
                            g.add_title(t)?;
                            elite_line = format!(
                                "\n{} **{}** is yours to wear: you beat {} for the first time.",
                                e("elite"),
                                t,
                                f.foe.name
                            );
                        }
                        if first {
                            announce = Some(format!(
                                "{} **{}** brought down **{}** in {}.",
                                e("elite"),
                                g.p.name,
                                f.foe.name,
                                r.name
                            ));
                        }
                    }
                    for (id, n) in &drops {
                        g.item(id, *n, "hunt")?;
                    }
                    g.gold(gold, "hunt")?;
                    let levels = g.xp(xp)?;
                    count_event(
                        g,
                        quests::Event::Win {
                            region: r.id,
                            monster: &f.foe.id,
                        },
                    )?;
                    tutorial_done(g, Tut::Hunt)?;
                    // An elite carries a piece; ordinary foes leave gear to the forge.
                    let gear_drop = if elite {
                        let piece =
                            crate::engine::loot::drop_piece(g.p.level, g.p.boss_won, &mut rng);
                        let (rarity, traits) = crate::engine::loot::roll(piece, "elite", &mut rng);
                        let worn = g.p.equipped(piece.slot).map(|x| x.power()).unwrap_or(0);
                        let power = data::gear_power(piece, 0)
                            + data::gear_power(piece, 0) * rarity.power_pct() / 100;
                        let wear = power > worn && piece.level <= g.p.level;
                        if wear || g.p.gear.iter().filter(|x| !x.equipped).count() < data::PACK_GEAR
                        {
                            g.add_piece(piece, rarity, traits, wear, "hunt")?;
                            format!(
                                "\n{} It dropped **{} {}**{}",
                                e(rarity.emoji()),
                                rarity.name(),
                                piece.name,
                                if wear {
                                    ": you're wearing it."
                                } else {
                                    ": it's in your pack."
                                }
                            )
                        } else {
                            g.item("thread", 1 + rarity.index(), "salvaged: pack full")?;
                            format!(
                                "\n{} It dropped a {} {}; your pack is full, so you salvaged it.",
                                e(rarity.emoji()),
                                rarity.name(),
                                piece.name
                            )
                        }
                    } else {
                        String::new()
                    };
                    elite_line.push_str(&gear_drop);
                    result = Some(format!(
                        "{} **{} falls.** +{xp} XP · +{gold} gold · {}{}{}{}\n-# You catch your breath: +{}% HP",
                        e("check"),
                        f.foe.name,
                        items_line(&drops),
                        if double {
                            format!("\n-# {} Doubled by Vigor", e("vigor"))
                        } else {
                            String::new()
                        },
                        if levels > 0 {
                            format!(
                                "\n{} **Level {}!**{}",
                                e("power"),
                                g.p.level,
                                crate::unlocks::opened_line(&g.p, &w.modules, g.p.level - levels)
                            )
                        } else {
                            String::new()
                        },
                        elite_line,
                        (data::WIN_HEAL * 100.0) as i64
                    ));
                    // Now and then, something on the way back.
                    if !elite
                        && w.modules.encounters
                        && crate::unlocks::open(&g.p, data::Feature::Encounters)
                        && rng.chance(0.14)
                    {
                        let enc = rng.pick(ENCOUNTERS);
                        g.set_event(Some(enc.id))?;
                    }
                }
                crate::engine::combat::Kind::Boss => {
                    let week = g.p.week();
                    let first = !g.p.boss_won;
                    let rewarded = first || g.p.boss_reward_week != week;
                    g.boss_won(f.turn)?;
                    if rewarded {
                        let shards = if first { 3 } else { 1 };
                        let mut rng = crate::engine::Rng::new(seed(&g.p, 83));
                        let piece =
                            crate::engine::loot::drop_piece(data::LEVEL_CAP, true, &mut rng);
                        let (rarity, traits) = crate::engine::loot::roll(piece, "boss", &mut rng);
                        g.add_piece(piece, rarity, traits, false, "the Barrow King")?;
                        g.item("shard", shards, "the Barrow King")?;
                        g.gold(if first { 500 } else { 200 }, "the Barrow King")?;
                        g.xp(if first { 1500 } else { 400 })?;
                        result = Some(
                            format!(
                                "{} **The Barrow King falls!** +{} crown shard{} · +{} gold{}",
                                e("shard"),
                                shards,
                                if shards == 1 { "" } else { "s" },
                                if first { 500 } else { 200 },
                                if first {
                                    "\nHeroic expeditions and your lesser vow are open. The forge has new gear."
                                } else {
                                    ""
                                }
                            ) + &format!(
                                "\n{} It dropped **{} {}**: it's in your pack.",
                                e(rarity.emoji()),
                                rarity.name(),
                                piece.name
                            ),
                        );
                    } else {
                        result = Some(format!(
                            "{} **The Barrow King falls again.** Its reward comes back on Monday.",
                            e("shard")
                        ));
                    }
                    if first {
                        announce = Some(format!(
                            "{} **{}** has defeated **The Barrow King** in {} turns.",
                            e("shard"),
                            g.p.name,
                            f.turn
                        ));
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
pub fn handle(
    ctx: &Ctx,
    g: &mut Game,
    route: &Route,
    values: &[String],
    rates: Rates,
) -> Result<Outcome> {
    let w = World::load(g, rates)?;
    let stale = !route.action.is_empty()
        && route.version >= 0
        && route.version != g.p.version
        && !matches!(route.action.as_str(), "set" | "pick" | "nav");
    let note_stale = Some(
        "That button was from before your last move. Here's where things are now.".to_string(),
    );
    let value = values.first().cloned().unwrap_or_default();
    let mut announce = None;
    let mut note: Option<String> = None;
    let locked = crate::unlocks::for_screen(&route.screen)
        .filter(|f| !crate::unlocks::usable(&g.p, *f))
        .map(data::unlock);
    let screen = if let Some(u) = locked {
        note = Some(format!("{} opens at level {}.", u.name, u.level));
        view(ctx, g, &w, "hub", None)?
    } else if stale {
        note = note_stale;
        view(ctx, g, &w, &route.screen, None)?
    } else {
        match (route.screen.as_str(), route.action.as_str()) {
            ("hub", "nav") => view(ctx, g, &w, &value, None)?,
            ("exp", "set") => {
                let (field, key) = route.arg.split_once('|').unwrap_or(("", ""));
                let mut plan = parse_plan(key).unwrap_or_else(|| default_plan(&g.p));
                match field {
                    "region" => {
                        plan.region = data::region(&value)
                            .map(|r| r.id.to_owned())
                            .unwrap_or(plan.region)
                    }
                    "danger" => plan.danger = Danger::from_id(&value).unwrap_or(plan.danger),
                    "hours" => {
                        plan.hours = value
                            .parse()
                            .ok()
                            .filter(|h| data::HOURS.contains(h))
                            .unwrap_or(plan.hours)
                    }
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
                let mut plan = parse_plan(&format!("{}.none", route.arg))
                    .unwrap_or_else(|| default_plan(&g.p));
                plan.supply = data::EXPEDITION_SUPPLIES
                    .iter()
                    .find(|s| g.p.have(s) > 0 && **s == "rations")
                    .map(|s| s.to_string());
                note = send_out(g, plan)?;
                view(ctx, g, &w, "exp", None)?
            }
            ("exp", "collect") => match collect(g, &w)? {
                Some((r, levels)) => expedition_report(&g.p, &r, levels),
                None => view(ctx, g, &w, "exp", None)?,
            },
            ("hunt", "pick") => hunt_pick(&g.p, Some(&value)),
            ("saga", "claim") => {
                note = claim_chapter(g, &route.arg)?;
                saga_screen(&g.p)
            }
            ("bounties", "claim") => {
                note = claim_bounty(g, route.arg.parse().unwrap_or(9))?;
                bounties_screen(&g.p)
            }
            ("hunt", "event") => {
                note = resolve_encounter(g, &route.arg)?;
                hunt_pick(&g.p, None)
            }
            ("patron", "title") => {
                if g.p.titles.contains(&value) {
                    g.set_title(Some(&value))?;
                    g.bump()?;
                }
                patron_screen(&g.p)
            }
            ("hunt", "inn") => {
                let cost = inn_cost(&g.p);
                if g.p.fight.is_none() && g.p.hp() < g.p.max_hp() && g.p.gold >= cost {
                    g.gold(-cost, "rested at the inn")?;
                    let max = g.p.max_hp();
                    g.set_hp(max)?;
                    g.bump()?;
                    note = Some("A warm meal and a bed. You're at full health.".into());
                }
                hunt_pick(&g.p, None)
            }
            ("hunt", "drink") => {
                if g.p.fight.is_none() && g.p.have("potion") > 0 && g.p.hp() < g.p.max_hp() {
                    let heal = g.p.max_hp() * 4 / 10;
                    let hp = g.p.hp();
                    g.item("potion", -1, "drank a potion")?;
                    g.set_hp(hp + heal)?;
                    g.bump()?;
                    note = Some(format!(
                        "You drank a potion: {} / {} HP.",
                        g.p.hp(),
                        g.p.max_hp()
                    ));
                }
                hunt_pick(&g.p, None)
            }
            ("hunt", "start") => {
                if g.p.fight.is_some() {
                    note = Some("Finish the fight you're in first.".into());
                } else if g.p.hp() * 10 < g.p.max_hp() * 3 {
                    note = Some(format!(
                        "You're too hurt to hunt ({} / {} HP). Rest a little or drink a potion.",
                        g.p.hp(),
                        g.p.max_hp()
                    ));
                } else {
                    let r = data::region(&route.arg).unwrap_or(&data::REGIONS[0]);
                    if g.p.event.is_some() {
                        g.set_event(None)?;
                    }
                    let f = Fight::hunt_with(
                        g.p.hero(),
                        g.p.hp(),
                        r,
                        seed(&g.p, 5),
                        w.modules.elites && crate::unlocks::open(&g.p, data::Feature::Elites),
                    );
                    g.save_fight(Some(f))?;
                    g.bump()?;
                }
                view(ctx, g, &w, "hunt", None)?
            }
            ("boss", "start") => {
                if g.p.fight.is_some() {
                    note = Some("Finish the fight you're in first.".into());
                } else if g.p.hp() < g.p.max_hp() / 2 {
                    note = Some(format!(
                        "You're hurt ({} / {} HP). Rest or drink a potion before facing the King.",
                        g.p.hp(),
                        g.p.max_hp()
                    ));
                } else {
                    let f = Fight::boss(
                        g.p.hero(),
                        g.p.hp(),
                        w.week,
                        g.p.facts.clone(),
                        seed(&g.p, 7),
                    )
                    .with_ally(quests::ally(&g.p.saga_choices));
                    g.save_fight(Some(f))?;
                    g.bump()?;
                }
                view(ctx, g, &w, "boss", None)?
            }
            ("hunt" | "boss" | "lab" | "dungeon" | "titan" | "raid", "act") => {
                match parse_action(&route.arg) {
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
                }
            }
            ("hunt" | "boss" | "lab" | "dungeon" | "titan" | "raid", "leave") => {
                let mut to = "hub";
                if let Some(f) = g.p.fight.clone() {
                    g.set_hp(f.hp)?;
                    g.save_fight(None)?;
                    match f.kind {
                        crate::engine::combat::Kind::Lab => {
                            crate::labyrinth::fled(g)?;
                            to = "lab";
                        }
                        crate::engine::combat::Kind::Dungeon => {
                            crate::dungeons::fled(g)?;
                            to = "dungeon";
                        }
                        crate::engine::combat::Kind::Shared => {
                            // Breaking off early still counts what you dealt.
                            let (said, ann) = crate::shared::settle(g, &f)?;
                            count_event(g, quests::Event::Shared)?;
                            announce = ann;
                            to = if route.screen == "raid" {
                                "raid"
                            } else {
                                "titan"
                            };
                            note = Some(said);
                        }
                        _ => {}
                    }
                    g.bump()?;
                    if note.is_none() {
                        note = Some(match f.kind {
                            crate::engine::combat::Kind::Boss => {
                                "You retreat. The King's wounds will heal; yours too.".into()
                            }
                            crate::engine::combat::Kind::Lab
                            | crate::engine::combat::Kind::Dungeon => {
                                "You back out and pull the door shut.".into()
                            }
                            _ => "You slip away from the fight.".into(),
                        });
                    }
                }
                view(ctx, g, &w, to, None)?
            }
            ("guild", "create") => {
                note = crate::guild::found(
                    g,
                    values.first().map(String::as_str).unwrap_or(""),
                    values.get(1).map(String::as_str).unwrap_or(""),
                )?;
                crate::guild::screen(g)?
            }
            ("guild", "join") => {
                let clan = if route.arg.is_empty() {
                    &value
                } else {
                    &route.arg
                };
                note = crate::guild::join(g, clan.parse().unwrap_or(-1))?;
                crate::guild::screen(g)?
            }
            ("guild", "find") => {
                let f = crate::guild::Find {
                    page: 0,
                    open_only: false,
                    query: crate::guild::clean(&value),
                };
                crate::guild::find_screen(g, &f)?
            }
            ("guild", "page") => {
                crate::guild::find_screen(g, &crate::guild::Find::parse(&route.arg))?
            }
            ("guild", "ask" | "withdraw") => {
                let (clan, rest) = route.arg.split_once('.').unwrap_or((&route.arg, ""));
                let clan = clan.parse().unwrap_or(-1);
                note = if route.action == "ask" {
                    crate::guild::ask(g, clan)?
                } else {
                    crate::guild::withdraw(g, clan)?
                };
                if g.p.clan.is_some() {
                    crate::guild::screen(g)?
                } else {
                    crate::guild::find_screen(g, &crate::guild::Find::parse(rest))?
                }
            }
            ("guild", "accept") => {
                note = crate::guild::accept(g, route.arg.parse().unwrap_or(0))?;
                crate::guild::manage_screen(g)?
            }
            ("guild", "decline") => {
                note = crate::guild::decline(g, value.parse().unwrap_or(0))?;
                crate::guild::manage_screen(g)?
            }
            ("guild", "invite") => {
                note = crate::guild::invite(g, value.parse().unwrap_or(0))?;
                crate::guild::manage_screen(g)?
            }
            ("guild", "policy") => {
                note = crate::guild::set_policy(g, &value)?;
                crate::guild::manage_screen(g)?
            }
            ("guild", "minimum") => {
                note = crate::guild::set_minimum(g, value.parse().unwrap_or(0))?;
                crate::guild::manage_screen(g)?
            }
            ("guild", "leave") => {
                note = crate::guild::leave(g)?;
                crate::guild::screen(g)?
            }
            ("guild", "give") => {
                note = crate::guild::give(g, route.arg.parse().unwrap_or(0).clamp(0, 100_000))?;
                crate::guild::screen(g)?
            }
            ("guild", "buy") => {
                note = crate::guild::buy(g, &value)?;
                crate::guild::screen(g)?
            }
            ("guild", "kick") => {
                note = crate::guild::kick(g, value.parse().unwrap_or(0))?;
                crate::guild::manage_screen(g)?
            }
            ("guild", "raid") => {
                note = start_raid(g)?;
                crate::guild::screen(g)?
            }
            ("titan" | "raid", "fight") => {
                note = crate::shared::attempt(g, &route.screen)?;
                view(ctx, g, &w, &route.screen, None)?
            }
            ("titan" | "raid", "claim") => {
                note = crate::shared::claim(g, &route.screen)?;
                view(ctx, g, &w, &route.screen, None)?
            }
            ("dungeon", "enter") => {
                note = crate::dungeons::enter(g, &route.arg)?;
                view(ctx, g, &w, "dungeon", None)?
            }
            ("dungeon", "next") => {
                note = crate::dungeons::next(g)?;
                view(ctx, g, &w, "dungeon", None)?
            }
            ("dungeon", "out") => {
                note = crate::dungeons::leave(g)?;
                view(ctx, g, &w, "dungeons", None)?
            }
            ("quests", "claim") => {
                note = claim_side(g, &route.arg)?;
                view(ctx, g, &w, "quests", None)?
            }
            ("market", "gear") => {
                note = crate::market::buy_gear(g, route.arg.parse().unwrap_or(9))?;
                view(ctx, g, &w, "market", None)?
            }
            ("market", "supply") => {
                note = crate::market::buy_supply(g, &value)?;
                view(ctx, g, &w, "market", None)?
            }
            ("exp", "plan") => {
                let mut plan = default_plan(&g.p);
                if let Some(r) = data::region(&route.arg) {
                    plan.region = r.id.into();
                }
                expedition_plan(&g.p, &w, &plan)
            }
            ("lab", "enter") => {
                note = crate::labyrinth::enter(g)?;
                view(ctx, g, &w, "lab", None)?
            }
            ("lab", "door") => {
                note = crate::labyrinth::open(g, &route.arg)?;
                view(ctx, g, &w, "lab", None)?
            }
            ("lab", "out") => {
                note = crate::labyrinth::leave(g)?;
                view(ctx, g, &w, "lab", None)?
            }
            ("tut", "ok") => {
                tutorial_done(g, Tut::Welcome)?;
                g.bump()?;
                view(ctx, g, &w, "hub", None)?
            }
            ("tut", "skip") => {
                g.set_tutorial(Tut::ALL)?;
                g.bump()?;
                view(ctx, g, &w, "hub", None)?
            }
            ("forge", "refine") => {
                note = refine(g, value.parse().unwrap_or(-1), false)?;
                forge_view(g)?
            }
            ("forge", "reforge") => {
                note = refine(g, value.parse().unwrap_or(-1), true)?;
                forge_view(g)?
            }
            ("forge", "gear") => {
                note = match data::gear(&route.arg) {
                    Some(gear) => forge_piece(g, gear, None, &mut announce)?,
                    None => None,
                };
                forge_view(g)?
            }
            ("forge", "order") => {
                note = crate::orders::fill(g)?;
                forge_view(g)?
            }
            ("forge", "temper") => {
                let (id, t) = value.split_once('.').unwrap_or(("", ""));
                let t = t
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| data::TRAITS.get(i))
                    .copied();
                note = match (data::gear(id), t) {
                    (Some(gear), Some(t)) => forge_piece(g, gear, Some(t), &mut announce)?,
                    _ => None,
                };
                forge_view(g)?
            }
            ("forge", "upgrade") => {
                let id: i64 = value.parse().unwrap_or(-1);
                let rank = g.p.smith_rank();
                match g.p.gear.iter().find(|x| x.id == id && x.equipped).cloned() {
                    Some(row) if row.upgrade < row.cap(rank) => {
                        let (gold, mat, n, threads) = data::upgrade_cost(row.gear, row.upgrade);
                        let cost = [(mat, n), ("thread", threads)];
                        if g.can_pay(&cost, gold) {
                            g.pay(&cost, gold, "upgrade")?;
                            g.upgrade(id)?;
                            let up = g.smith(data::smith_xp_upgrade(row.upgrade + 1))?;
                            g.bump()?;
                            note = Some(format!(
                                "{} is now +{}.{}",
                                row.gear.name,
                                row.upgrade + 1,
                                rank_line(up)
                            ));
                        } else {
                            note = Some(format!(
                                "Upgrading needs {gold} gold and {}{}.",
                                data::count(mat, n),
                                if threads > 0 {
                                    format!(" and {}", data::count("thread", threads))
                                } else {
                                    String::new()
                                }
                            ));
                        }
                    }
                    Some(row) if !row.forged && row.upgrade >= data::FOUND_UPGRADE => {
                        note = Some(format!(
                            "Found gear takes upgrades to +{} only. Pieces you forge yourself go further.",
                            data::FOUND_UPGRADE
                        ))
                    }
                    Some(_) => {
                        note = Some(format!(
                            "Your smith rank takes your own work to +{}. Forge more to go further.",
                            data::upgrade_cap(rank, true)
                        ))
                    }
                    None => {}
                }
                forge_view(g)?
            }
            ("forge", "make") => {
                match data::SUPPLY_RECIPES.iter().find(|r| r.id == value) {
                    Some(r) if g.can_pay(r.cost, r.gold) => {
                        g.pay(r.cost, r.gold, "made supplies")?;
                        g.item(r.makes, r.qty, "made at the forge")?;
                        let up = g.smith(data::SMITH_XP_SUPPLY)?;
                        count_event(g, quests::Event::Forged)?;
                        tutorial_done(g, Tut::Forge)?;
                        g.bump()?;
                        note = Some(format!(
                            "Made {}.{}",
                            data::count(r.makes, r.qty),
                            rank_line(up)
                        ));
                    }
                    Some(r) => {
                        note = Some(format!(
                            "You need {} and {} gold.",
                            r.cost
                                .iter()
                                .map(|(id, n)| data::count(id, *n))
                                .collect::<Vec<_>>()
                                .join(", "),
                            r.gold
                        ))
                    }
                    None => {}
                }
                forge_view(g)?
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
                    if row.rarity.index() > 0 {
                        g.item("thread", row.rarity.index(), "salvaged")?;
                    }
                    let up = g.smith(data::SMITH_XP_SALVAGE)?;
                    g.bump()?;
                    note = Some(format!("Salvaged the {}.{}", row.gear.name, rank_line(up)));
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
                                count_event(g, quests::Event::Gave(n))?;
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
                            count_event(g, quests::Event::Gave(total))?;
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
                    let cost = if g.p.lesser.is_some() {
                        data::PATRON_CHANGE_GOLD
                    } else {
                        0
                    };
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
                        note = Some(format!(
                            "Changing patron costs {} gold.",
                            data::PATRON_CHANGE_GOLD
                        ));
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
    Ok(Outcome {
        screen: screen.note(note),
        announce,
    })
}

/// Show a screen without changing anything.
pub fn view(
    ctx: &Ctx,
    g: &mut Game,
    w: &World,
    screen: &str,
    _arg: Option<&str>,
) -> Result<Screen> {
    if let Some(f) = crate::unlocks::for_screen(screen) {
        if !crate::unlocks::usable(&g.p, f) {
            return view(ctx, g, w, "hub", None);
        }
        g.see_unlocks(data::unlock(f).bit)?;
    }
    let p = &g.p;
    Ok(match screen {
        "goals" => goals(
            p,
            w,
            &store::top_givers(&g.tx, p.guild, w.week, 50)?,
            &crate::labyrinth::board(g, 3)?,
        ),
        "exp" => match &p.expedition {
            Some(x) => expedition_out(p, w, &x.clone()),
            None => expedition_plan(p, w, &default_plan(p)),
        },
        "guild" if w.modules.guilds => crate::guild::screen(g)?,
        "guildfind" if w.modules.guilds => {
            crate::guild::find_screen(g, &crate::guild::Find::parse(""))?
        }
        "guildmanage" if w.modules.guilds => crate::guild::manage_screen(g)?,
        "titan" if w.modules.titans => match &p.fight {
            Some(f) if f.kind == crate::engine::combat::Kind::Shared => {
                fight_screen(p, &f.clone(), None)
            }
            _ => crate::shared::screen(g, "titan")?,
        },
        "raid" if w.modules.raids => match &p.fight {
            Some(f) if f.kind == crate::engine::combat::Kind::Shared => {
                fight_screen(p, &f.clone(), None)
            }
            _ => crate::shared::screen(g, "raid")?,
        },
        "dungeons" if w.modules.dungeons => crate::dungeons::list_screen(g)?,
        "dungeon" if w.modules.dungeons => match (&p.fight, &p.delve) {
            (Some(f), _) if f.kind == crate::engine::combat::Kind::Dungeon => {
                fight_screen(p, &f.clone(), None)
            }
            (_, Some(d)) => crate::dungeons::delve_screen(p, &d.clone()),
            _ => crate::dungeons::list_screen(g)?,
        },
        "map" if w.modules.map => map_screen(ctx, g, w)?,
        "quests" if w.modules.quests => side_screen(p),
        "market" if w.modules.market => crate::market::screen(g)?,
        x if x.starts_with("region_") => region_screen(g, w, &x[7..])?,
        "lab" => match &p.fight {
            Some(f) if f.kind == crate::engine::combat::Kind::Lab => {
                fight_screen(p, &f.clone(), None)
            }
            _ => crate::labyrinth::screen(p, &crate::labyrinth::board(g, 5)?),
        },
        "hero" => hero(p),
        "skagi" => skagi(p, w),
        "combat" => combat_menu(p, w),
        "questlog" => quest_menu(p, w),
        "hunt" | "boss" => match &p.fight {
            Some(f) => fight_screen(p, &f.clone(), None),
            None if screen == "boss" => goals(
                p,
                w,
                &store::top_givers(&g.tx, p.guild, w.week, 50)?,
                &crate::labyrinth::board(g, 3)?,
            ),
            None => hunt_pick(p, None),
        },
        "forge" => forge_view(g)?,
        "pack" => pack(p),
        "temple" => temple(p, w, &store::top_givers(&g.tx, p.guild, w.week, 5)?),
        "givethor" => give(p, God::Thor),
        "giveathena" => give(p, God::Athena),
        "givehades" => give(p, God::Hades),
        "patron" => patron_screen(p),
        "help" => help(p),
        "saga" => saga_screen(p),
        "bounties" => bounties_screen(p),
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
    if route.action.is_empty() {
        route.screen.clone()
    } else {
        format!("{}:{}", route.screen, route.action)
    }
}

/// Delete a player's character entirely (dashboard tool).
pub fn reset_player(db: &bot_core::Db, guild: u64, user: u64) -> Result<()> {
    let mut c = db.lock();
    let tx = c.transaction()?;
    for t in [
        "rpg_players",
        "rpg_items",
        "rpg_gear",
        "rpg_bonds",
        "rpg_expeditions",
        "rpg_fights",
        "rpg_intel",
        "rpg_pity",
        "rpg_kills",
    ] {
        tx.execute(
            &format!("DELETE FROM {t} WHERE guild_id = ?1 AND user_id = ?2"),
            params![guild as i64, user as i64],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn now() -> i64 {
    now_ms()
}

// ---------------------------------------------------------------------------------------------
// The saga, bounties, encounters and titles
// ---------------------------------------------------------------------------------------------

/// Where a player hunts comfortably: the strongest region they clear with room to spare.
pub fn comfortable(p: &Player, level: u32) -> &'static data::Region {
    let power = data::level_power(level) + (p.power() - data::level_power(p.level));
    data::REGIONS
        .iter()
        .rev()
        .find(|r| power as f64 >= r.power as f64 * 1.3)
        .unwrap_or(&data::REGIONS[0])
}

pub fn todays_bounties(p: &Player) -> Vec<quests::Bounty> {
    let level = p.bounty_level.unwrap_or(p.level);
    quests::bounties(p.user, quests::day(p.now), level, comfortable(p, level))
}

/// Count something that happened toward the saga and today's bounties.
pub(crate) fn count_event(g: &mut Game, ev: quests::Event) -> Result<()> {
    count_side(g, ev)?;
    match ev {
        quests::Event::Win { .. } => crate::guild::credit(g, 4, 1)?,
        quests::Event::Expedition { .. } => crate::guild::credit(g, 15, 0)?,
        quests::Event::Floor(_) => crate::guild::credit(g, 6, 0)?,
        quests::Event::Forged => crate::guild::credit(g, 5, 0)?,
        quests::Event::Gave(n) => crate::guild::credit(g, n / 20, 0)?,
        _ => {}
    }
    if let Some(ch) = quests::chapter(g.p.saga) {
        if g.p.saga_progress.len() < ch.goals.len() {
            g.p.saga_progress.resize(ch.goals.len(), 0);
        }
        let mut changed = false;
        for (i, goal) in ch.goals.iter().enumerate() {
            let n = goal.counts(ev);
            if n > 0 && g.p.saga_progress[i] < goal.target() {
                g.p.saga_progress[i] = (g.p.saga_progress[i] + n).min(goal.target());
                changed = true;
            }
        }
        if changed {
            g.save_saga()?;
        }
    }
    let bs = todays_bounties(&g.p);
    for (i, b) in bs.iter().enumerate() {
        let n = b.goal.counts(ev);
        if n > 0 && !g.p.bounty[i].1 && g.p.bounty[i].0 < b.goal.target() {
            g.p.bounty[i].0 = (g.p.bounty[i].0 + n).min(b.goal.target());
            g.save_bounty(i)?;
        }
    }
    Ok(())
}

fn goal_now(p: &Player, goal: &quests::Goal, counter: i64) -> i64 {
    quests::goal_progress(goal, counter, p.level, p.facts.len(), p.boss_won).min(goal.target())
}

/// Whether the current chapter's goals are all met.
pub fn chapter_done(p: &Player) -> bool {
    let Some(ch) = quests::chapter(p.saga) else {
        return false;
    };
    ch.goals.iter().enumerate().all(|(i, goal)| {
        goal_now(p, goal, p.saga_progress.get(i).copied().unwrap_or(0)) >= goal.target()
    })
}

fn reward_line(r: &quests::Reward, patron: God) -> String {
    let mut parts = Vec::new();
    if r.gold > 0 {
        parts.push(format!("{} {} gold", e("gold"), r.gold));
    }
    if r.xp > 0 {
        parts.push(format!("{} XP", r.xp));
    }
    for (id, n) in r.items {
        parts.push(format!("{} {}", e(id), data::count(id, *n)));
    }
    if r.tribute > 0 {
        parts.push(format!(
            "{} +{} tribute to {}",
            e("offering"),
            r.tribute,
            patron.name()
        ));
    }
    if let Some(t) = r.title {
        parts.push(format!("the title **{t}**"));
    }
    parts.join("  ·  ")
}

/// "Level 6!" and what opened, after a reward that may have levelled you.
fn level_news(g: &Game, level: u32, saga: usize) -> String {
    let m = store::modules(g.p.guild);
    let mut out = String::new();
    if g.p.level > level {
        out.push_str(&format!("\n{} **Level {}!**", e("power"), g.p.level));
    }
    out.push_str(&crate::unlocks::opened(&g.p, &m, level, saga));
    out
}

fn give_reward(g: &mut Game, r: &quests::Reward, why: &str) -> Result<()> {
    g.gold(r.gold, why)?;
    g.xp(r.xp)?;
    for (id, n) in r.items {
        g.item(id, *n, why)?;
    }
    let main = g.p.main();
    g.tribute(main, r.tribute)?;
    if let Some(t) = r.title {
        g.add_title(t)?;
    }
    Ok(())
}

fn saga_screen(p: &Player) -> Screen {
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    let Some(ch) = quests::chapter(p.saga) else {
        title(&mut s, "saga", "The Loom", "Between the worlds");
        s.text("The Norns are weaving the next saga. Its thread leads beyond Midgard, and it arrives with the next realm.");
        let picked: Vec<String> = quests::CHAPTERS
            .iter()
            .filter_map(|c| {
                p.saga_choices
                    .get(c.id)
                    .and_then(|id| c.choices.iter().find(|x| x.id == id))
                    .map(|x| format!("-# {} · {}", c.title, x.label))
            })
            .collect();
        if !picked.is_empty() {
            s.text(format!("-# Your choices in Midgard\n{}", picked.join("\n")));
        }
        s.buttons(vec![back_to(&ids, "questlog", "Quests")]);
        return s;
    };
    title(
        &mut s,
        "saga",
        ch.title,
        &format!(
            "{} · chapter {} of {}",
            ch.place,
            p.saga + 1,
            quests::CHAPTERS.len()
        ),
    );
    s.text(ch.text);
    if let Some(line) = quests::patron_line(ch, p.main()) {
        s.text(format!("-# {} {line}", e(p.main().id())));
    }
    if !ch.goals.is_empty() {
        let lines: Vec<String> = ch
            .goals
            .iter()
            .enumerate()
            .map(|(i, goal)| {
                let have = goal_now(p, goal, p.saga_progress.get(i).copied().unwrap_or(0));
                let done = have >= goal.target();
                let count = if goal.target() > 1 && !matches!(goal, quests::Goal::Level(_)) {
                    format!("  ·  {have} / {}", goal.target())
                } else {
                    String::new()
                };
                format!(
                    "{} {}{count}",
                    if done { e("check") } else { e("goals") },
                    goal.describe()
                )
            })
            .collect();
        s.gap();
        s.text(lines.join("\n"));
    }
    let done = chapter_done(p);
    if !ch.choices.is_empty() {
        s.gap();
        if done {
            s.text("-# Choose. Each road is worth about the same; they change the story and what you carry.");
        } else {
            s.text("-# When it's done you'll choose:");
        }
        for c in ch.choices {
            let extra = reward_line(&c.reward, p.main());
            let line = format!(
                "**{}**\n-# {}{}",
                c.label,
                c.hint,
                if extra.is_empty() {
                    String::new()
                } else {
                    format!(" · {extra}")
                }
            );
            if done {
                s.section(
                    line,
                    Btn::new(ids.act("saga", "claim", c.id), "Choose")
                        .emoji("saga")
                        .primary(),
                );
            } else {
                s.text(line);
            }
        }
    }
    let r = reward_line(&ch.reward, p.main());
    if !r.is_empty() {
        s.text(format!("-# Reward: {r}"));
    }
    let mut b = vec![back_to(&ids, "questlog", "Quests")];
    if ch.choices.is_empty() {
        b.push(
            Btn::new(ids.act("saga", "claim", ""), ch.claim)
                .emoji("saga")
                .primary()
                .disabled(!done),
        );
    }
    s.buttons(b);
    s
}

fn claim_chapter(g: &mut Game, choice: &str) -> Result<Option<String>> {
    let Some(ch) = quests::chapter(g.p.saga) else {
        return Ok(None);
    };
    if !chapter_done(&g.p) {
        return Ok(Some("That chapter isn't finished yet.".into()));
    }
    let picked = if ch.choices.is_empty() {
        None
    } else {
        match ch.choices.iter().find(|c| c.id == choice) {
            Some(c) => Some(c),
            None => return Ok(Some("Choose one of the options.".into())),
        }
    };
    let (level, saga) = (g.p.level, g.p.saga);
    give_reward(g, &ch.reward, &format!("saga: {}", ch.title))?;
    let mut said = format!(
        "**{}** done. {}",
        ch.title,
        reward_line(&ch.reward, g.p.main())
    );
    if let Some(c) = picked {
        give_reward(g, &c.reward, &format!("saga: {}", ch.title))?;
        g.p.saga_choices.insert(ch.id.to_owned(), c.id.to_owned());
        said = format!("{}\n{}", c.after, reward_line(&c.reward, g.p.main()));
    }
    g.p.saga += 1;
    g.p.saga_progress = Vec::new();
    g.save_saga()?;
    tutorial_done(g, Tut::Saga)?;
    g.bump()?;
    said.push_str(&level_news(g, level, saga));
    Ok(Some(said))
}

fn bounties_screen(p: &Player) -> Screen {
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    title(
        &mut s,
        "bounty",
        "Bounties",
        "New ones every day at midnight UTC",
    );
    let bs = todays_bounties(p);
    // When every bounty pays the same, say it once at the top instead of on each line.
    let same = bs
        .windows(2)
        .all(|w| w[0].gold == w[1].gold && w[0].xp == w[1].xp);
    if same && let Some(b) = bs.first() {
        s.text(format!(
            "-# Each pays {} {} gold · {} XP. All three add 2 healing potions.",
            e("gold"),
            b.gold,
            b.xp
        ));
    }
    for (i, b) in bs.iter().enumerate() {
        let (have, claimed) = p.bounty[i];
        let target = b.goal.target();
        let mut line = format!(
            "{} **{}**{}",
            if claimed { e("check") } else { e("bounty") },
            b.goal.describe(),
            if target > 1 {
                format!("  ·  {} / {target}", have.min(target))
            } else {
                String::new()
            },
        );
        if !same {
            line.push_str(&format!("\n-# {} {} gold · {} XP", e("gold"), b.gold, b.xp));
        }
        if claimed {
            s.text(line);
        } else {
            s.section(
                line,
                Btn::new(ids.act("bounties", "claim", &i.to_string()), "Claim")
                    .emoji("bounty")
                    .disabled(have < target)
                    .then_primary(have >= target),
            );
        }
    }
    if p.bounty.iter().all(|b| b.1) {
        s.text(format!(
            "-# {} All three done today. Back tomorrow.",
            e("check")
        ));
    } else if !same {
        s.text("-# Finish all three for 2 healing potions on top.");
    }
    s.buttons(vec![
        back_to(&ids, "questlog", "Quests"),
        Btn::new(ids.go("hunt"), "Hunt").emoji("hunt"),
    ]);
    s
}

fn claim_bounty(g: &mut Game, idx: usize) -> Result<Option<String>> {
    let bs = todays_bounties(&g.p);
    let Some(b) = bs.get(idx) else {
        return Ok(None);
    };
    let (have, claimed) = g.p.bounty[idx];
    if claimed || have < b.goal.target() {
        return Ok(None);
    }
    g.p.bounty[idx].1 = true;
    g.save_bounty(idx)?;
    let level = g.p.level;
    g.gold(b.gold, "bounty")?;
    g.xp(b.xp)?;
    let mut said = format!("Bounty claimed: {} gold, {} XP.", b.gold, b.xp);
    said.push_str(&level_news(g, level, g.p.saga));
    if g.p.bounty.iter().all(|x| x.1) {
        g.item("potion", 2, "all three bounties")?;
        said.push_str(" All three done: 2 healing potions too.");
        if g.p.titles.iter().all(|t| t != "Bounty Hunter") {
            g.add_title("Bounty Hunter")?;
        }
    }
    g.bump()?;
    Ok(Some(said))
}

/// Encounters after a hunt: a small choice, both sides worth having.
pub struct Encounter {
    pub id: &'static str,
    pub title: &'static str,
    pub text: &'static str,
}

pub const ENCOUNTERS: &[Encounter] = &[
    Encounter {
        id: "merchant",
        title: "A wandering merchant",
        text: "She has more potions than she can carry, and a long road ahead.",
    },
    Encounter {
        id: "shrine",
        title: "An old shrine",
        text: "Moss over a carving of your patron. Someone left flowers here, not long ago.",
    },
    Encounter {
        id: "traveller",
        title: "A lost traveller",
        text: "He's limping, far from the road, and very glad to see you.",
    },
    Encounter {
        id: "cache",
        title: "A cache under the roots",
        text: "Someone buried a chest here and never came back. The roots have grown through the lid.",
    },
];

/// The two options of an encounter: (id, label, emoji).
fn encounter_options(p: &Player, id: &str) -> [(&'static str, String, &'static str); 2] {
    let lvl = p.level as i64;
    match id {
        "merchant" => [
            (
                "buy",
                format!("Buy 2 potions · {} gold", 20 + 2 * lvl),
                "potion",
            ),
            ("leave", "Move on".into(), "expedition"),
        ],
        "shrine" => [
            (
                "offer",
                format!("Leave {} gold: +20 Vigor", 10 + 3 * lvl),
                "vigor",
            ),
            ("pray", "Pray: heal 40%".into(), "hp"),
        ],
        "traveller" => [
            (
                "walk",
                format!("Walk him back: +{} XP", 40 + 15 * lvl),
                "power",
            ),
            (
                "purse",
                format!("Take his thanks: +{} gold", 15 + 6 * lvl),
                "gold",
            ),
        ],
        _ => [
            ("dig", "Dig it out: it'll hurt".into(), "pack"),
            ("leave", "Leave it".into(), "expedition"),
        ],
    }
}

fn encounter_block(s: &mut Screen, p: &Player, ids: &Ids) {
    let Some(ev) = p
        .event
        .as_deref()
        .and_then(|id| ENCOUNTERS.iter().find(|x| x.id == id))
    else {
        return;
    };
    s.gap();
    s.text(format!("{} **{}**\n{}", e("event"), ev.title, ev.text));
    let [a, b] = encounter_options(p, ev.id);
    s.buttons(vec![
        Btn::new(ids.act("hunt", "event", a.0), a.1)
            .emoji(a.2)
            .primary(),
        Btn::new(ids.act("hunt", "event", b.0), b.1).emoji(b.2),
    ]);
}

fn resolve_encounter(g: &mut Game, choice: &str) -> Result<Option<String>> {
    let Some(ev) = g.p.event.clone() else {
        return Ok(None);
    };
    let lvl = g.p.level as i64;
    g.set_event(None)?;
    let said = match (ev.as_str(), choice) {
        ("merchant", "buy") => {
            let cost = 20 + 2 * lvl;
            if g.p.gold >= cost {
                g.gold(-cost, "wandering merchant")?;
                g.item("potion", 2, "wandering merchant")?;
                "Two potions, wrapped in old cloth. She waves you off.".to_string()
            } else {
                "You can't afford them. She shrugs and walks on.".into()
            }
        }
        ("shrine", "offer") => {
            let cost = 10 + 3 * lvl;
            if g.p.gold >= cost {
                g.gold(-cost, "shrine")?;
                g.vigor(20.0)?;
                "The flowers stir. You feel rested in a way sleep doesn't give you: +20 Vigor."
                    .into()
            } else {
                "Your purse is too light. You leave a bow instead.".into()
            }
        }
        ("shrine", "pray") => {
            let (hp, max) = (g.p.hp(), g.p.max_hp());
            g.set_hp(hp + max * 4 / 10)?;
            "Warmth goes through you. Your wounds close a little.".into()
        }
        ("traveller", "walk") => {
            g.xp(40 + 15 * lvl)?;
            "He tells stories the whole way back. Some of them are even true.".into()
        }
        ("traveller", "purse") => {
            g.gold(15 + 6 * lvl, "a traveller's thanks")?;
            "He presses coins into your hand and limps off toward the road.".into()
        }
        ("cache", "dig") => {
            let (hp, max) = (g.p.hp(), g.p.max_hp());
            g.set_hp(hp - max * 15 / 100)?;
            let region = comfortable(&g.p, g.p.level);
            let mut rng = crate::engine::Rng::new(seed(&g.p, 41));
            let (mat, _) = *rng.pick(region.drops);
            g.item(mat, 3, "a buried cache")?;
            g.gold(20 + 5 * lvl, "a buried cache")?;
            let mut found = format!(
                "You tear your hands on the roots. Inside: {}, and some old coins.",
                data::count(mat, 3)
            );
            if rng.chance(0.35) {
                g.item(region.rare.0, 1, "a buried cache")?;
                found.push_str(&format!(" And {}.", data::count(region.rare.0, 1)));
            }
            found
        }
        _ => "You leave it be.".into(),
    };
    g.bump()?;
    Ok(Some(said))
}

// ---------------------------------------------------------------------------------------------
// The tutorial, gear refining, and the Character, Skagi, Combat and Quests screens
// ---------------------------------------------------------------------------------------------

/// Tutorial steps, as bits: each is done once, in any order. The hub shows the first not done.
#[derive(Debug, Clone, Copy)]
pub enum Tut {
    Welcome = 1,
    Expedition = 2,
    Hunt = 4,
    Collect = 8,
    Forge = 16,
    Saga = 32,
    Labyrinth = 64,
}

impl Tut {
    pub const ALL: i64 = 127;
    const ORDER: [Tut; 7] = [
        Tut::Welcome,
        Tut::Hunt,
        Tut::Saga,
        Tut::Expedition,
        Tut::Collect,
        Tut::Forge,
        Tut::Labyrinth,
    ];

    /// The system a step teaches: the step waits until it's open.
    fn feature(self) -> Option<data::Feature> {
        match self {
            Tut::Expedition | Tut::Collect => Some(data::Feature::Expedition),
            Tut::Forge => Some(data::Feature::Forge),
            Tut::Labyrinth => Some(data::Feature::Labyrinth),
            _ => None,
        }
    }

    /// (what, why, how, screen to open, button)
    fn text(
        self,
    ) -> (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    ) {
        match self {
            Tut::Welcome => (
                "How this works",
                "**Hunts** bring gold, XP and materials. Your **saga** is the story, and as it goes on, and as you level, more of Midgard opens: expeditions first, then the **forge**, where materials become the gear that carries you to the **Barrow King**.",
                "A few short steps, each when its time comes. Each one pays a little.",
                "hub",
                "Let's go",
            ),
            Tut::Expedition => (
                "Send an expedition",
                "It works while you're away and brings back materials, gold and XP.",
                "Open **Expedition**, keep the suggested region and **Normal** danger, and press **Send out**.",
                "exp",
                "Expedition",
            ),
            Tut::Hunt => (
                "Win a hunt",
                "Hunts pay right away and are where you learn to fight.",
                "Open **Hunt** and **Start hunting**. When the foe shows a warning, **Guard**. Otherwise, **Strike**.",
                "hunt",
                "Hunt",
            ),
            Tut::Collect => (
                "Collect your expedition",
                "What it found only reaches your pack when you collect it.",
                "When it's back, the overview shows **Collect**.",
                "exp",
                "Expedition",
            ),
            Tut::Forge => (
                "Make something at the forge",
                "Gear is Power, and the forge is where it comes from. Everything you make raises your smith rank: better odds, further upgrades, finer recipes.",
                "Open **Character**, then **Forge**. Forge a piece you can afford, or make potions.",
                "forge",
                "Forge",
            ),
            Tut::Saga => (
                "Read your saga",
                "It's the story, and every chapter pays out.",
                "Open **Quests**, then **Saga**, and claim the first chapter.",
                "saga",
                "Saga",
            ),
            Tut::Labyrinth => (
                "Go into the Labyrinth",
                "It drops loose threads: the forge uses them to refine, reforge and temper gear.",
                "Open **Combat**, then **Labyrinth**, and **Enter**. Leave whenever you like and keep everything.",
                "lab",
                "Labyrinth",
            ),
        }
    }
}

impl Player {
    pub fn tutorial_has(&self, t: Tut) -> bool {
        self.tutorial & t as i64 != 0
    }
}

/// Mark a tutorial step done (once), with a small reward; finishing all of them pays more.
pub fn tutorial_done(g: &mut Game, t: Tut) -> Result<()> {
    if g.p.tutorial_has(t) {
        return Ok(());
    }
    let next = g.p.tutorial | t as i64;
    g.set_tutorial(next)?;
    if !matches!(t, Tut::Welcome) {
        g.gold(25, "tutorial")?;
    }
    if next == Tut::ALL {
        g.item("potion", 2, "tutorial")?;
        g.gold(100, "tutorial")?;
        g.add_title("Initiate of the Loom")?;
    }
    Ok(())
}

/// The tutorial step or the news card, if there is one. Returns whether it took the primary
/// button.
fn tutorial_block(s: &mut Screen, p: &Player, ids: &Ids, m: &store::Modules) -> bool {
    let ready = |t: &Tut| {
        t.feature()
            .is_none_or(|f| crate::unlocks::enabled(m, f) && crate::unlocks::open(p, f))
            && (!matches!(t, Tut::Saga) || m.saga)
    };
    let step = if p.tutorial >= Tut::ALL {
        None
    } else {
        Tut::ORDER
            .into_iter()
            .find(|t| !p.tutorial_has(*t) && ready(t))
    };
    let Some(step) = step else {
        // Nothing to learn right now: say what opened since the last visit, once.
        if let Some(u) = crate::unlocks::news(p, m) {
            s.section(
                format!("-# New\n{} **{}**\n{}", e(u.emoji), u.name, u.why),
                Btn::new(ids.go(u.screen), "Take a look")
                    .emoji(u.emoji)
                    .primary(),
            );
            return true;
        }
        return false;
    };
    let (what, why, how, screen, button) = step.text();
    let head = match step.feature() {
        Some(_) => "-# New".to_string(),
        None => "-# Getting started".to_string(),
    };
    if matches!(step, Tut::Welcome) {
        s.text(format!("{head}\n### {} {what}\n{why}", e("help")));
        s.section(
            format!("-# {how} Rather find your own way? Skip it from Help."),
            Btn::new(ids.act("tut", "ok", ""), button)
                .emoji("check")
                .primary(),
        );
        true
    } else {
        s.section(
            format!("{head}\n{} **{what}**\n{why}\n-# How: {how}", e("help")),
            Btn::new(ids.go(screen), button).emoji("help"),
        );
        false
    }
}

/// Raise a piece's rarity (adds a trait) or reforge its traits, with loose threads.
fn refine(g: &mut Game, gear_id: i64, reforge: bool) -> Result<Option<String>> {
    let Some(row) = g.p.gear.iter().find(|x| x.id == gear_id).cloned() else {
        return Ok(None);
    };
    let mut rng = crate::engine::Rng::new(seed(&g.p, 97));
    if reforge {
        if row.rarity == data::Rarity::Common {
            return Ok(Some(
                "Common gear has no traits to reforge. Refine it first.".into(),
            ));
        }
        let (threads, gold) = data::reforge_cost(row.rarity, g.p.level);
        if g.p.have("thread") < threads || g.p.gold < gold {
            return Ok(Some(format!(
                "Reforging needs {} and {gold} gold.",
                data::count("thread", threads)
            )));
        }
        g.item("thread", -threads, "reforged")?;
        g.gold(-gold, "reforged")?;
        let traits =
            crate::engine::loot::roll_traits(row.gear, row.rarity.index() as usize, &mut rng);
        g.set_piece(gear_id, row.rarity, traits.clone())?;
        let up = g.smith(data::SMITH_XP_REFORGE)?;
        g.bump()?;
        let t: Vec<String> = traits
            .iter()
            .map(|(t, v)| format!("{} ({})", t.name(), t.describe(*v)))
            .collect();
        return Ok(Some(format!(
            "Reforged the {}: {}.{}",
            row.gear.name,
            t.join(", "),
            rank_line(up)
        )));
    }
    if row.rarity == data::Rarity::Epic {
        return Ok(Some(
            "That's as fine as Midgard's forge can make it.".into(),
        ));
    }
    let to = data::Rarity::from_index(row.rarity.index() + 1);
    let (threads, gold) = data::refine_cost(to, g.p.level);
    if g.p.have("thread") < threads || g.p.gold < gold {
        return Ok(Some(format!(
            "Refining to {} needs {} and {gold} gold.",
            to.name(),
            data::count("thread", threads)
        )));
    }
    g.item("thread", -threads, "refined")?;
    g.gold(-gold, "refined")?;
    let mut traits = row.traits.clone();
    let have: Vec<data::Trait> = traits.iter().map(|x| x.0).collect();
    let mut extra = crate::engine::loot::roll_traits(row.gear, 4, &mut rng);
    extra.retain(|(t, _)| !have.contains(t));
    if let Some(n) = extra.first() {
        traits.push(*n);
    }
    g.set_piece(gear_id, to, traits)?;
    let up = g.smith(data::smith_xp_refine(to))?;
    g.bump()?;
    Ok(Some(format!(
        "The {} is now **{}**{}.{}",
        row.gear.name,
        to.name(),
        extra
            .first()
            .map(|(t, v)| format!(", and {} ({})", t.name(), t.describe(*v)))
            .unwrap_or_default(),
        rank_line(up)
    )))
}

fn rarity_tag(r: data::Rarity) -> String {
    format!("{} {}", e(r.emoji()), r.name())
}

fn hero(p: &Player) -> Screen {
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    title(
        &mut s,
        p.main().id(),
        &p.name,
        &format!(
            "{}Level {} · sworn to {}",
            p.title
                .as_deref()
                .map(|t| format!("{t} · "))
                .unwrap_or_default(),
            p.level,
            p.main().name()
        ),
    );
    let (max, atk) = p.hero().stats();
    let m = p.mods();
    let gear = p.power() - data::level_power(p.level);
    let mut lines = vec![
        format!(
            "{} **{} Power**: {} from level {}, {} from gear",
            e("power"),
            p.power(),
            data::level_power(p.level),
            p.level,
            gear
        ),
        format!(
            "{} {} HP  ·  {} attack  ·  {}% critical hits",
            e("hp"),
            max,
            atk,
            10 + (m.crit * 100.0).round() as i64
        ),
        format!(
            "{} {} / {} XP to level {}",
            e("saga"),
            thousands(p.xp),
            thousands(p.xp_needed()),
            p.level + 1
        ),
    ];
    let mut extras = Vec::new();
    if m.damage > 0.0 {
        extras.push(format!("+{:.0}% damage", m.damage * 100.0));
    }
    if m.steal > 0.0 {
        extras.push(format!("+{:.0}% lifesteal", m.steal * 100.0));
    }
    if m.guard > 0.0 {
        extras.push(format!("guard +{:.0}%", m.guard * 100.0));
    }
    if m.stamina > 0 {
        extras.push(format!("+{} starting stamina", m.stamina));
    }
    if m.loot > 0.0 {
        extras.push(format!("+{:.0}% materials", m.loot * 100.0));
    }
    if !extras.is_empty() {
        lines.push(format!("-# From traits: {}", extras.join(" · ")));
    }
    s.text(lines.join("\n"));
    s.text("-# Wearing");
    let mut wear = Vec::new();
    for slot in data::SLOTS {
        match p.equipped(slot) {
            Some(g) => {
                wear.push(format!(
                    "{} **{}** · {} · +{} Power",
                    e(slot_emoji(slot)),
                    g.label(),
                    rarity_tag(g.rarity),
                    g.power()
                ));
                if !g.traits.is_empty() {
                    wear.push(format!("-# {}", g.traits_text()));
                }
            }
            None => wear.push(format!(
                "{} {}: nothing yet",
                e(slot_emoji(slot)),
                slot.name()
            )),
        }
    }
    s.text(wear.join("\n"));
    if crate::unlocks::open(p, data::Feature::Forge) {
        forge_section(&mut s, &ids, p);
    } else {
        let u = data::unlock(data::Feature::Forge);
        s.text(format!(
            "-# {} {} at level {}, or sooner as your saga goes on",
            e("lock"),
            u.name,
            u.level
        ));
    }
    s.buttons(vec![
        back(&ids),
        Btn::new(ids.go("pack"), "Pack").emoji("pack"),
        Btn::new(ids.go("patron"), "Skills and titles").emoji(p.main().id()),
        Btn::new(ids.go("help"), "Help").emoji("help"),
    ]);
    s
}

/// The forge line on the Character screen: what's ready to make, and threads to refine with.
fn forge_section(s: &mut Screen, ids: &Ids, p: &Player) {
    let n = data::SLOTS
        .iter()
        .filter_map(|sl| next_piece(p, *sl))
        .filter(|g| {
            forge_lock(p, g).is_none()
                && p.gold >= g.gold
                && g.cost.iter().all(|(id, n)| p.have(id) >= *n)
        })
        .count();
    s.section(
        format!(
            "{} **Forge** · smith rank {}\n-# {}{}",
            e("forge"),
            p.smith_rank(),
            if n > 0 {
                format!(
                    "{n} new piece{} ready to forge",
                    if n == 1 { "" } else { "s" }
                )
            } else {
                "Gear, upgrades, supplies".into()
            },
            if p.have("thread") > 0 {
                format!(" · {} loose threads", p.have("thread"))
            } else {
                String::new()
            }
        ),
        Btn::new(ids.go("forge"), "Forge")
            .emoji("forge")
            .then_primary(n > 0),
    );
}

/// The village: the market and the temple.
fn skagi(p: &Player, w: &World) -> Screen {
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    title(&mut s, "temple", "Skagi", "The village on the Grey Shore");
    if w.modules.market && crate::unlocks::open(p, data::Feature::Market) {
        s.section(
            format!("{} **Market**\n-# New stock every day", e("market")),
            Btn::new(ids.go("market"), "Market").emoji("market"),
        );
    }
    if crate::unlocks::open(p, data::Feature::Temple) {
        s.section(
            format!(
                "{} **Temple**\n-# This week's offering {}%",
                e("temple"),
                w.pct()
            ),
            Btn::new(ids.go("temple"), "Temple").emoji("temple"),
        );
    }
    s.buttons(vec![back(&ids)]);
    s
}

/// Everything you fight in: hunts, expeditions, the Labyrinth, dungeons, the map and shared bosses.
fn combat_menu(p: &Player, w: &World) -> Screen {
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let m = &w.modules;
    let mut s = Screen::default();
    title(&mut s, "boss", "Combat", "");
    let collect = p.expedition.as_ref().is_some_and(|x| p.now >= x.ends_at);
    let usable = |f| crate::unlocks::usable(p, f);
    if let Some((name, pct, ends)) = w.titan.filter(|_| usable(data::Feature::Titans)) {
        s.section(
            format!(
                "{} **{name} is attacking Midgard**\n-# {pct}% left · {} to bring it down, together",
                e("boss"),
                dur(ends - p.now)
            ),
            Btn::new(ids.go("titan"), "Fight").emoji("boss").danger(),
        );
    }
    if let Some((name, pct, ends)) = w.raid {
        s.section(
            format!(
                "{} **Guild raid: {name}**\n-# {pct}% left · {} left",
                e("guild"),
                dur(ends - p.now)
            ),
            Btn::new(ids.go("raid"), "Raid").emoji("boss"),
        );
    }
    let hunt_line = match &p.fight {
        Some(f) => format!("In a fight with {} · turn {}", f.foe.name, f.turn),
        None => format!("{} / {} HP · Vigor {:.0}%", p.hp(), p.max_hp(), p.vigor()),
    };
    s.section(
        format!("{} **Hunt**\n-# {hunt_line}", e("hunt")),
        Btn::new(
            ids.go("hunt"),
            if p.fight.is_some() {
                "Continue"
            } else {
                "Hunt"
            },
        )
        .emoji("hunt")
        .then_primary(!collect && p.run.is_none()),
    );
    let exp_line = match &p.expedition {
        Some(x) if collect => format!("Back from {}, ready to collect", x.plan.region().name),
        Some(x) => format!(
            "Out in {} · back in {}",
            x.plan.region().name,
            dur(x.ends_at - p.now)
        ),
        None => "Nothing out".into(),
    };
    if usable(data::Feature::Expedition) {
        s.section(
            format!("{} **Expedition**\n-# {exp_line}", e("expedition")),
            if collect {
                Btn::new(ids.act("exp", "collect", ""), "Collect")
                    .emoji("expedition")
                    .primary()
            } else {
                Btn::new(ids.go("exp"), "Expedition").emoji("expedition")
            },
        );
    }
    if m.labyrinth && usable(data::Feature::Labyrinth) {
        let line = match &p.run {
            Some(run) => format!(
                "On floor {} · carrying {}",
                run.floor,
                crate::ui::truncate(&run.haul_short(), 60)
            ),
            None => String::new(),
        };
        s.section(
            sub_line(&format!("{} **Labyrinth**", e("labyrinth")), &line),
            Btn::new(
                ids.go("lab"),
                if p.run.is_some() {
                    "Continue"
                } else {
                    "Labyrinth"
                },
            )
            .emoji("labyrinth")
            .then_primary(p.run.is_some() && !collect),
        );
    }
    if m.dungeons && usable(data::Feature::Dungeons) {
        let line = match &p.delve {
            Some(d) => format!(
                "In {} · room {} of {}",
                d.dungeon().name,
                (d.room + 1).min(d.dungeon().rooms.len() + 1),
                d.dungeon().rooms.len() + 1
            ),
            None => String::new(),
        };
        s.section(
            sub_line(&format!("{} **Dungeons**", e("dungeon")), &line),
            Btn::new(
                ids.go(if p.delve.is_some() {
                    "dungeon"
                } else {
                    "dungeons"
                }),
                if p.delve.is_some() {
                    "Continue"
                } else {
                    "Dungeons"
                },
            )
            .emoji("dungeon"),
        );
    }
    if m.map && usable(data::Feature::Expedition) {
        s.section(
            format!("{} **Map**", e("map")),
            Btn::new(ids.go("map"), "Map").emoji("map"),
        );
    }
    if let Some(line) = crate::unlocks::next_line(
        p,
        m,
        &[
            data::Feature::Expedition,
            data::Feature::Dungeons,
            data::Feature::Labyrinth,
            data::Feature::Titans,
        ],
    ) {
        s.text(line);
    }
    s.buttons(vec![back(&ids)]);
    s
}

/// The saga, the townsfolk's side quests, the bounty board and the goals.
fn quest_menu(p: &Player, w: &World) -> Screen {
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let m = &w.modules;
    let mut s = Screen::default();
    title(&mut s, "quest", "Quests", "");
    let mut primary_used = false;
    if m.saga {
        let (line, ready) = match quests::chapter(p.saga) {
            Some(ch) if chapter_done(p) => (format!("**{}** is ready to read", ch.title), true),
            Some(ch) => (format!("**{}**\n-# {}", ch.title, saga_next(p, ch)), false),
            None => ("**Midgard's saga is told**\n-# For now".into(), false),
        };
        s.section(
            format!("{} {line}", e("saga")),
            Btn::new(ids.go("saga"), if ready { "Read" } else { "Saga" })
                .emoji("saga")
                .then_primary(ready),
        );
        primary_used |= ready;
    }
    if m.quests && crate::unlocks::open(p, data::Feature::Townsfolk) {
        let ready = quests::SIDE
            .iter()
            .filter(|q| side_step(p, q).is_some_and(|(st, pr)| pr >= q.steps[st].goal.target()))
            .count();
        let open = quests::SIDE
            .iter()
            .filter(|q| side_step(p, q).is_some())
            .count();
        s.section(
            format!(
                "{} **Townsfolk**\n-# {open} side quest{} open{}",
                e("quest"),
                if open == 1 { "" } else { "s" },
                if ready > 0 {
                    format!(" · {ready} ready to hand in")
                } else {
                    String::new()
                }
            ),
            Btn::new(ids.go("quests"), "Side quests")
                .emoji("quest")
                .then_primary(ready > 0 && !primary_used),
        );
        primary_used |= ready > 0;
    }
    if m.bounties && crate::unlocks::open(p, data::Feature::Bounties) {
        let done = p.bounty.iter().filter(|b| b.1).count();
        let claim = todays_bounties(p)
            .iter()
            .enumerate()
            .filter(|(i, b)| !p.bounty[*i].1 && p.bounty[*i].0 >= b.goal.target())
            .count();
        s.section(
            format!(
                "{} **Bounty board**\n-# {done} of 3 done today{}",
                e("bounty"),
                if claim > 0 {
                    format!(" · {claim} ready to claim")
                } else {
                    String::new()
                }
            ),
            Btn::new(ids.go("bounties"), "Bounties")
                .emoji("bounty")
                .then_primary(claim > 0 && !primary_used),
        );
    }
    s.section(
        format!(
            "{} **Goals**\n-# {}",
            e("goals"),
            if p.boss_won {
                "The Barrow King is beaten · offerings and your guild".to_string()
            } else {
                format!(
                    "The Barrow King needs {} Power, you have {}",
                    data::BOSS_POWER,
                    p.power()
                )
            }
        ),
        Btn::new(ids.go("goals"), "Goals").emoji("goals"),
    );
    if let Some(line) =
        crate::unlocks::next_line(p, m, &[data::Feature::Bounties, data::Feature::Townsfolk])
    {
        s.text(line);
    }
    s.buttons(vec![back(&ids)]);
    s
}

// ---------------------------------------------------------------------------------------------
// The map, regions, side quests and raids
// ---------------------------------------------------------------------------------------------

fn start_raid(g: &mut Game) -> Result<Option<String>> {
    if !store::modules(g.p.guild).raids {
        return Ok(None);
    }
    let Some(m) = g.p.clan.clone() else {
        return Ok(None);
    };
    if !m.leader {
        return Ok(Some("Only the guild's leader starts a raid.".into()));
    }
    if let Some(r) = crate::shared::active(&g.tx, g.p.guild, "raid", m.id)?
        && !r.over(g.p.now)
    {
        return Ok(Some("A raid is already on.".into()));
    }
    if m.treasury < data::RAID_COST {
        return Ok(Some(format!(
            "Starting a raid takes {} from the treasury.",
            thousands(data::RAID_COST)
        )));
    }
    let members = crate::guild::members(&g.tx, g.p.guild, m.id, g.p.week())?;
    let avg: i64 = members.iter().map(|x| x.level).sum::<i64>() / members.len().max(1) as i64;
    let power = (60 + avg * 16).max(120);
    let hp = data::RAID_HP_PER_MEMBER * (members.len() as i64).max(3);
    g.tx.execute(
        "UPDATE rpg_clans SET treasury = treasury - ?2 WHERE id = ?1",
        bot_core::rusqlite::params![m.id, data::RAID_COST],
    )?;
    crate::shared::spawn(
        &g.tx,
        g.p.guild,
        "raid",
        m.id,
        data::RAID_BOSS,
        power,
        hp,
        data::RAID_HOURS,
    )?;
    g.p.clan = crate::guild::membership(&g.tx, g.p.guild, g.p.user)?;
    g.bump()?;
    Ok(Some(format!(
        "Fafnir stirs. Your guild has {} hours, {} attempts each a day.",
        data::RAID_HOURS,
        data::RAID_ATTEMPTS
    )))
}

/// A side quest's current step, if the line is open and not finished.
fn side_step(p: &Player, q: &quests::SideQuest) -> Option<(usize, i64)> {
    if p.level < q.level {
        return None;
    }
    let (step, progress) = p
        .side
        .iter()
        .find(|x| x.0 == q.id)
        .map(|x| (x.1, x.2))
        .unwrap_or((0, 0));
    (step < q.steps.len()).then_some((step, progress))
}

fn count_side(g: &mut Game, ev: quests::Event) -> Result<()> {
    if !store::modules(g.p.guild).quests {
        return Ok(());
    }
    for q in quests::SIDE {
        let Some((step, progress)) = side_step(&g.p, q) else {
            continue;
        };
        let goal = q.steps[step].goal;
        let n = goal.counts(ev);
        if n > 0 && progress < goal.target() {
            g.save_side(q.id, step, (progress + n).min(goal.target()))?;
        }
    }
    Ok(())
}

fn claim_side(g: &mut Game, id: &str) -> Result<Option<String>> {
    let Some(q) = quests::side(id) else {
        return Ok(None);
    };
    let Some((step, progress)) = side_step(&g.p, q) else {
        return Ok(None);
    };
    let st = q.steps[step];
    if progress < st.goal.target() {
        return Ok(Some("Not done yet.".into()));
    }
    let level = g.p.level;
    give_reward(g, &st.reward, &format!("{}: {}", q.npc, q.title))?;
    let news = level_news(g, level, g.p.saga);
    g.save_side(q.id, step + 1, 0)?;
    g.bump()?;
    let done = step + 1 >= q.steps.len();
    Ok(Some(format!(
        "{} {}{}",
        reward_line(&st.reward, g.p.main()),
        if done {
            format!(
                "\n**{}** is done. {} thanks you.",
                q.title,
                q.npc.split(',').next().unwrap_or(q.npc)
            )
        } else {
            // The next ask, in their own words.
            format!("\n*{}*", q.steps[step + 1].text)
        },
        news
    )))
}

fn side_screen(p: &Player) -> Screen {
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    title(&mut s, "quest", "Skagi's townsfolk", "Side quests");
    let mut any = false;
    let mut later = Vec::new();
    for q in quests::SIDE {
        if p.level < q.level {
            later.push(format!("{} at level {}", q.npc, q.level));
            continue;
        }
        let Some((step, progress)) = side_step(p, q) else {
            s.text(format!("-# {} **{}** · done", e("check"), q.title));
            continue;
        };
        any = true;
        let st = q.steps[step];
        let target = st.goal.target();
        // The ask and the pay on one line; the townsfolk's words for the next step come with
        // the claim, where they read as the story moving on.
        let line = format!(
            "{} **{}** · {}\n-# {}{} · {}",
            e("quest"),
            q.title,
            q.npc,
            st.goal.describe(),
            if target > 1 {
                format!(" · {} / {target}", progress.min(target))
            } else {
                String::new()
            },
            reward_line(&st.reward, p.main())
        );
        s.section(
            line,
            Btn::new(ids.act("quests", "claim", q.id), "Claim")
                .emoji("quest")
                .disabled(progress < target)
                .then_primary(progress >= target),
        );
    }
    if !later.is_empty() {
        s.text(format!("-# {} Later: {}", e("lock"), later.join(", ")));
    }
    if !any {
        s.text("Nobody needs you right now. Come back as you grow.");
    }
    s.buttons(vec![back_to(&ids, "questlog", "Quests")]);
    s
}

fn map_spec(g: &Game) -> Result<crate::map::Spec> {
    let p = &g.p;
    let power = p.power();
    let mut open = 0u8;
    for (i, r) in data::REGIONS.iter().enumerate() {
        if power >= r.power || i == 0 {
            open |= 1 << i;
        }
    }
    let expedition = p
        .expedition
        .as_ref()
        .and_then(|x| data::REGIONS.iter().position(|r| r.id == x.plan.region))
        .map(|i| i as u8 + 1)
        .unwrap_or(0);
    let titan = store::modules(p.guild).titans
        && crate::shared::active(&g.tx, p.guild, "titan", 0)?.is_some_and(|t| !t.over(p.now));
    Ok(crate::map::Spec {
        open,
        expedition,
        titan,
    })
}

fn map_screen(ctx: &Ctx, g: &Game, w: &World) -> Result<Screen> {
    let p = &g.p;
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    let spec = map_spec(g)?;
    s.banner = Some(spec.url(ctx));
    s.text(format!("### {} Midgard", e("map")));
    for r in data::REGIONS {
        let open = p.power() >= r.power || r.id == "shore";
        s.section(
            format!(
                "{} **{}**\n-# {}{}",
                e(r.id),
                r.name,
                r.blurb,
                if open {
                    String::new()
                } else {
                    format!(" · {} needs {} Power", e("lock"), r.power)
                }
            ),
            Btn::new(ids.go(&format!("region_{}", r.id)), "Go").emoji(r.id),
        );
    }
    let mut row = vec![back_to(&ids, "combat", "Combat")];
    if w.modules.labyrinth {
        row.push(Btn::new(ids.go("lab"), "Labyrinth").emoji("labyrinth"));
    }
    if w.modules.dungeons {
        row.push(Btn::new(ids.go("dungeons"), "Dungeons").emoji("dungeon"));
    }
    if spec.titan {
        row.push(
            Btn::new(ids.go("titan"), "The Titan")
                .emoji("boss")
                .danger(),
        );
    }
    s.buttons(row);
    Ok(s)
}

fn region_screen(g: &Game, w: &World, id: &str) -> Result<Screen> {
    let p = &g.p;
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    let Some(r) = data::region(id) else {
        return Ok(hub_fallback(p));
    };
    title(
        &mut s,
        r.id,
        r.name,
        &format!(
            "Needs {} Power on Normal · {} on Hard",
            r.power,
            r.power * 2
        ),
    );
    s.text(r.blurb);
    let mons: Vec<&str> = r
        .monsters
        .iter()
        .filter_map(|m| data::monster(m))
        .map(|m| m.name)
        .collect();
    let mut lines = vec![
        format!("{} **Foes:** {}", e("hunt"), mons.join(", ")),
        format!(
            "{} **Drops:** {}, and now and then {}",
            e("pack"),
            r.drops
                .iter()
                .map(|(i, _)| data::item(i).map(|x| x.plural).unwrap_or(i))
                .collect::<Vec<_>>()
                .join(", "),
            data::item(r.rare.0).map(|x| x.name).unwrap_or("")
        ),
    ];
    if w.modules.elites
        && let Some(el) = data::elite(r.id)
    {
        lines.push(format!(
            "{} **Elite:** {} walks here, rarely",
            e("elite"),
            el.name
        ));
    }
    if r.intel > 0.05 {
        lines.push(format!(
            "{} Expeditions here scout the Barrow King",
            e("intel")
        ));
    }
    s.text(lines.join("\n"));
    let mut b = vec![
        Btn::new(ids.go("map"), "← Map"),
        Btn::new(ids.act("hunt", "start", r.id), "Hunt here")
            .emoji("hunt")
            .primary()
            .disabled(p.fight.is_some() || p.hp() * 10 < p.max_hp() * 3),
    ];
    b.push(
        Btn::new(ids.act("exp", "plan", r.id), "Expedition here")
            .emoji("expedition")
            .disabled(p.expedition.is_some()),
    );
    if w.modules.dungeons
        && let Some(d) = data::DUNGEONS.iter().find(|d| d.region == r.id)
    {
        let today = crate::dungeons::clears_today(g, d.id)?;
        s.text(format!(
            "{} **{}** · {}\n-# Opens at level {} · fights at {} Power{}",
            e("dungeon"),
            d.name,
            d.blurb,
            d.level,
            d.power,
            if today == 0 {
                " · chest waiting today"
            } else {
                " · cleared today"
            }
        ));
        b.push(
            Btn::new(ids.act("dungeon", "enter", d.id), "Enter the dungeon")
                .emoji("dungeon")
                .disabled(p.level < d.level || p.delve.is_some() || p.fight.is_some()),
        );
    }
    s.buttons(b);
    Ok(s)
}

fn hub_fallback(p: &Player) -> Screen {
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    s.text("That place isn't on the map.");
    s.buttons(vec![back(&ids)]);
    s
}
