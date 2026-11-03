//! The Labyrinth of Daedalus: under the Loom, where cut threads fall. Floor by floor you pick
//! a door. A guardian waits every fifth floor. Leave whenever you like and keep your haul; fall
//! and you keep half. Loose threads found here refine and reforge gear at the forge, and the
//! deepest floor each week goes on the server's board.

use std::collections::BTreeMap;

use bot_core::anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::data::{self, Rarity};
use crate::emoji::get as e;
use crate::engine::Rng;
use crate::engine::combat::Fight;
use crate::engine::loot::{self, Traits};
use crate::game::{Ids, back_to, count_event, items_line, seed, title};
use crate::quests;
use crate::store::{self, Game, Player};
use crate::ui::{Btn, Screen};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Door {
    Fight,
    Hard,
    Treasure,
    Shrine,
    Guardian,
}

impl Door {
    fn id(self) -> &'static str {
        match self {
            Door::Fight => "fight",
            Door::Hard => "hard",
            Door::Treasure => "treasure",
            Door::Shrine => "shrine",
            Door::Guardian => "guardian",
        }
    }
    fn from_id(s: &str) -> Option<Door> {
        [
            Door::Fight,
            Door::Hard,
            Door::Treasure,
            Door::Shrine,
            Door::Guardian,
        ]
        .into_iter()
        .find(|d| d.id() == s)
    }
    fn describe(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Door::Fight => ("A fight", "Something breathes behind this door.", "hunt"),
            Door::Hard => (
                "A hard fight",
                "Claw marks on the frame. Tougher, and twice the haul.",
                "warn",
            ),
            Door::Treasure => (
                "A treasure room",
                "Gold, materials, maybe threads. Maybe a trap.",
                "gold",
            ),
            Door::Shrine => ("A shrine", "Still water and an old altar. Heal 30%.", "hp"),
            Door::Guardian => (
                "The guardian",
                "The Minotaur. Threads and a piece of gear if you win.",
                "elite",
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Drop {
    pub gear: String,
    pub rarity: Rarity,
    pub traits: Traits,
}

/// A run in progress: the floor, the doors in front of you, and what you're carrying out.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub floor: u32,
    pub doors: Vec<Door>,
    /// The door being fought behind, while a fight is on.
    pub fighting: Option<Door>,
    pub gold: i64,
    pub xp: i64,
    pub items: BTreeMap<String, i64>,
    pub gear: Vec<Drop>,
    pub log: Vec<String>,
    pub seed: u64,
}

impl Run {
    fn rng(&mut self) -> Rng {
        self.seed = self.seed.wrapping_add(0x9E37_79B9);
        Rng::new(self.seed ^ self.floor as u64)
    }

    fn new_doors(&mut self) {
        let mut r = self.rng();
        self.doors = if data::guardian_floor(self.floor) {
            vec![Door::Guardian]
        } else {
            let mut d = vec![Door::Fight];
            let others = [Door::Hard, Door::Treasure, Door::Shrine, Door::Fight];
            while d.len() < 3 {
                let x = *r.pick(&others);
                if x == Door::Fight || !d.contains(&x) {
                    d.push(x);
                }
            }
            // Shuffle so the plain fight isn't always first.
            let k = (r.next_u64() % 3) as usize;
            d.swap(0, k);
            d
        };
    }

    /// The haul in a few words, for the overview.
    pub fn haul_short(&self) -> String {
        let n: i64 = self.items.values().sum();
        format!(
            "{} gold, {} items{}",
            self.gold,
            n,
            if self.gear.is_empty() {
                String::new()
            } else {
                format!(", {} gear", self.gear.len())
            }
        )
    }

    fn haul(&self) -> String {
        let mut parts = Vec::new();
        if self.gold > 0 {
            parts.push(format!("{} {} gold", e("gold"), self.gold));
        }
        if self.xp > 0 {
            parts.push(format!("{} XP", self.xp));
        }
        let items: Vec<(String, i64)> = self.items.iter().map(|(k, v)| (k.clone(), *v)).collect();
        if !items.is_empty() {
            parts.push(items_line(&items));
        }
        for d in &self.gear {
            if let Some(g) = data::gear(&d.gear) {
                parts.push(format!(
                    "{} {} {}",
                    e(d.rarity.emoji()),
                    d.rarity.name(),
                    g.name
                ));
            }
        }
        if parts.is_empty() {
            "nothing yet".into()
        } else {
            parts.join("  ·  ")
        }
    }
}

fn add(items: &mut BTreeMap<String, i64>, id: &str, n: i64) {
    if n > 0 {
        *items.entry(id.to_owned()).or_default() += n;
    }
}

pub fn screen(p: &Player, board: &[(u64, String, i64)]) -> Screen {
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    let best = board
        .iter()
        .find(|(u, ..)| *u == p.user)
        .map(|x| x.2)
        .unwrap_or(0);
    match &p.run {
        None => {
            title(
                &mut s,
                "labyrinth",
                "The Labyrinth of Daedalus",
                "Under the Loom, where cut threads fall",
            );
            s.text(format!(
                "Daedalus built a maze under the Loom to catch what falls from it. Loose threads collect down there, and so do the things that feed on them.\n\n\
                 {} **How it works:** pick a door on each floor. Every fifth floor, the Minotaur. Leave whenever you like and keep everything; fall and you keep half.\n\
                 {} **Why:** loose threads refine and reforge gear at the forge, guardians drop gear, and the deepest floor each week goes on the board.",
                e("labyrinth"),
                e("thread")
            ));
            let (hp, max) = (p.hp(), p.max_hp());
            if hp * 10 < max * 6 {
                s.text(format!(
                    "-# {} You're at {hp} / {max} HP. Go in with at least 60%.",
                    e("hp")
                ));
            }
            if !board.is_empty() {
                let list: Vec<String> = board
                    .iter()
                    .enumerate()
                    .map(|(i, (u, n, f))| {
                        format!(
                            "-# {}. {}{} · floor {f}",
                            i + 1,
                            if n.is_empty() { "Someone" } else { n },
                            if *u == p.user { " (you)" } else { "" }
                        )
                    })
                    .collect();
                s.text(format!("-# Deepest this week\n{}", list.join("\n")));
            }
            s.buttons(vec![
                back_to(&ids, "combat", "Combat"),
                Btn::new(ids.act("lab", "enter", ""), "Enter")
                    .emoji("labyrinth")
                    .primary()
                    .disabled(hp * 10 < max * 6 || p.fight.is_some()),
            ]);
        }
        Some(run) => {
            title(
                &mut s,
                "labyrinth",
                &format!("Floor {}", run.floor),
                &format!("The Labyrinth · your best this week: floor {best}"),
            );
            if !run.log.is_empty() {
                s.text(
                    run.log
                        .iter()
                        .map(|l| format!("-# {l}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
            s.text(format!(
                "{} {} / {} HP\n{} Carrying: {}",
                e("hp"),
                p.hp(),
                p.max_hp(),
                e("pack"),
                run.haul()
            ));
            s.gap();
            for (i, d) in run.doors.iter().enumerate() {
                let (name, hint, em) = d.describe();
                let b =
                    Btn::new(ids.act("lab", "door", &format!("{i}.{}", d.id())), "Open").emoji(em);
                s.section(
                    format!("{} **{name}**\n-# {hint}", e(em),),
                    if *d == Door::Guardian { b.danger() } else { b },
                );
            }
            s.buttons(vec![
                Btn::new(ids.act("lab", "out", ""), "Leave with your haul")
                    .emoji("pack")
                    .primary(),
                Btn::new(ids.go("hub"), "← Overview"),
            ]);
        }
    }
    s
}

/// Start a run.
pub fn enter(g: &mut Game) -> Result<Option<String>> {
    if g.p.run.is_some() {
        return Ok(None);
    }
    if g.p.fight.is_some() {
        return Ok(Some("Finish the fight you're in first.".into()));
    }
    if g.p.hp() * 10 < g.p.max_hp() * 6 {
        return Ok(Some("Go in with at least 60% HP.".into()));
    }
    let mut run = Run {
        floor: 1,
        doors: Vec::new(),
        fighting: None,
        gold: 0,
        xp: 0,
        items: BTreeMap::new(),
        gear: Vec::new(),
        log: vec!["The door closes behind you. The air smells of old wool and older blood.".into()],
        seed: seed(&g.p, 61),
    };
    run.new_doors();
    g.save_run(Some(run))?;
    crate::game::tutorial_done(g, crate::game::Tut::Labyrinth)?;
    g.bump()?;
    Ok(None)
}

/// Open a door: fights start; treasure and shrines resolve at once.
pub fn open(g: &mut Game, arg: &str) -> Result<Option<String>> {
    let Some(mut run) = g.p.run.clone() else {
        return Ok(None);
    };
    if g.p.fight.is_some() || run.fighting.is_some() {
        return Ok(Some("You're already in a fight.".into()));
    }
    let (idx, door) = arg.split_once('.').unwrap_or(("", ""));
    let Some(door) = Door::from_id(door)
        .filter(|d| idx.parse::<usize>().ok().and_then(|i| run.doors.get(i)) == Some(d))
    else {
        return Ok(Some("That door is gone.".into()));
    };
    let mut r = run.rng();
    run.log.clear();
    match door {
        Door::Fight | Door::Hard | Door::Guardian => {
            let monster = if door == Door::Guardian {
                data::monster("minotaur").unwrap()
            } else {
                let pool: Vec<&data::Monster> =
                    data::MONSTERS.iter().filter(|m| !m.elite).collect();
                pool[(r.next_u64() % pool.len() as u64) as usize]
            };
            let mult = if door == Door::Hard { 1.3 } else { 1.0 };
            let f = Fight::floor(
                g.p.hero(),
                g.p.hp(),
                monster,
                data::floor_power(run.floor),
                mult,
                r.next_u64(),
            );
            run.fighting = Some(door);
            g.save_fight(Some(f))?;
        }
        Door::Treasure => {
            let region = data::floor_region(run.floor);
            let (mat, _) = *r.pick(region.drops);
            if r.chance(0.2) {
                let (hp, max) = (g.p.hp(), g.p.max_hp());
                g.set_hp(hp - max * 12 / 100)?;
                run.log
                    .push("A plate clicks under your foot. Darts. (-12% HP)".into());
            }
            let gold = 10 + 6 * run.floor as i64;
            run.gold += gold;
            add(&mut run.items, mat, 2 + run.floor as i64 / 3);
            let mut found = format!(
                "Treasure: {gold} gold and {}.",
                data::count(mat, 2 + run.floor as i64 / 3)
            );
            if r.chance(0.4) {
                add(&mut run.items, "thread", 1);
                found.push_str(" And a loose thread.");
            }
            run.log.push(found);
            next_floor(g, &mut run)?;
        }
        Door::Shrine => {
            let (hp, max) = (g.p.hp(), g.p.max_hp());
            g.set_hp(hp + max * 3 / 10)?;
            run.log.push(
                "You drink from the still water. Your wounds close a little. (+30% HP)".into(),
            );
            next_floor(g, &mut run)?;
        }
    }
    g.save_run(Some(run))?;
    g.bump()?;
    Ok(None)
}

fn next_floor(g: &mut Game, run: &mut Run) -> Result<()> {
    if g.lab_best(run.floor)? && run.floor >= 3 {
        run.log
            .push(format!("Your deepest this week: floor {}.", run.floor));
    }
    count_event(g, quests::Event::Floor(run.floor))?;
    run.floor += 1;
    run.new_doors();
    Ok(())
}

/// A Labyrinth fight ended. Returns what to say.
pub fn fight_over(g: &mut Game, f: &Fight, won: bool) -> Result<String> {
    let Some(mut run) = g.p.run.clone() else {
        return Ok(String::new());
    };
    let door = run.fighting.take().unwrap_or(Door::Fight);
    run.log.clear();
    if !won {
        // Fall: keep half the haul, lose the gear you found.
        let keep = data::LAB_KEEP_ON_DEATH;
        run.gold = (run.gold as f64 * keep) as i64;
        run.xp = (run.xp as f64 * keep) as i64;
        for v in run.items.values_mut() {
            *v = (*v as f64 * keep) as i64;
        }
        run.gear.clear();
        let carried = run.haul();
        bank(g, &run)?;
        g.save_run(None)?;
        g.set_hp(1)?;
        return Ok(format!(
            "{} **You fall on floor {}.** You wake at the Loom with half your haul: {carried}.",
            e("hp"),
            run.floor
        ));
    }
    let mut r = run.rng();
    let m = if door == Door::Hard { 2 } else { 1 };
    let floor = run.floor as i64;
    let region = data::floor_region(run.floor);
    let (mat, _) = *r.pick(region.drops);
    run.gold += (8 + 3 * floor) * m;
    run.xp += (20 + 8 * floor) * m;
    add(&mut run.items, mat, m);
    let mut said = format!(
        "{} **{} falls.** Into your haul: {} gold, {}.",
        e("check"),
        f.foe.name,
        (8 + 3 * floor) * m,
        data::count(mat, m)
    );
    let threads = if door == Door::Guardian {
        3
    } else if r.chance(0.25 * m as f64) {
        1
    } else {
        0
    };
    if threads > 0 {
        add(&mut run.items, "thread", threads);
        said.push_str(&format!(
            " {} {}.",
            e("thread"),
            data::count("thread", threads)
        ));
    }
    if door == Door::Guardian || r.chance(0.06 * m as f64) {
        let piece = loot::drop_piece(g.p.level, g.p.boss_won, &mut r);
        let (rarity, traits) = loot::roll(
            piece,
            if door == Door::Guardian {
                "guardian"
            } else {
                "hunt"
            },
            &mut r,
        );
        said.push_str(&format!(
            " {} **{} {}**.",
            e(rarity.emoji()),
            rarity.name(),
            piece.name
        ));
        run.gear.push(Drop {
            gear: piece.id.into(),
            rarity,
            traits,
        });
    }
    let (hp, max) = (f.hp, f.max_hp);
    g.set_hp(hp + (max as f64 * data::LAB_WIN_HEAL) as i64)?;
    run.log.push(said.clone());
    next_floor(g, &mut run)?;
    g.save_run(Some(run))?;
    Ok(said)
}

/// Fleeing a Labyrinth fight: back to the doors, the floor not cleared.
pub fn fled(g: &mut Game) -> Result<()> {
    if let Some(mut run) = g.p.run.clone() {
        run.fighting = None;
        run.log = vec!["You back out of the room and pull the door shut.".into()];
        g.save_run(Some(run))?;
    }
    Ok(())
}

/// Walk out with everything.
pub fn leave(g: &mut Game) -> Result<Option<String>> {
    let Some(run) = g.p.run.clone() else {
        return Ok(None);
    };
    if g.p.fight.is_some() {
        return Ok(Some("Finish or flee the fight first.".into()));
    }
    let carried = run.haul();
    let out = bank(g, &run)?;
    g.save_run(None)?;
    g.bump()?;
    Ok(Some(format!(
        "You climb out after floor {}. You carry out: {carried}.{out}",
        run.floor.saturating_sub(1)
    )))
}

/// Put a run's haul in the pack. Gear past the pack's room is salvaged into threads.
fn bank(g: &mut Game, run: &Run) -> Result<String> {
    g.gold(run.gold, "the Labyrinth")?;
    g.xp(run.xp)?;
    for (id, n) in &run.items {
        g.item(id, *n, "the Labyrinth")?;
    }
    let mut salvaged = 0;
    for d in &run.gear {
        let Some(piece) = data::gear(&d.gear) else {
            continue;
        };
        let spare = g.p.gear.iter().filter(|x| !x.equipped).count();
        let worn = g.p.equipped(piece.slot).map(|x| x.power()).unwrap_or(0);
        let power =
            data::gear_power(piece, 0) + data::gear_power(piece, 0) * d.rarity.power_pct() / 100;
        let wear = power > worn && piece.level <= g.p.level;
        if !wear && spare >= data::PACK_GEAR {
            salvaged += 1 + d.rarity.index();
            continue;
        }
        g.add_piece(piece, d.rarity, d.traits.clone(), wear, "the Labyrinth")?;
    }
    if salvaged > 0 {
        g.item("thread", salvaged, "salvaged: pack full")?;
        return Ok(format!(
            " Your pack was full: spare gear became {}.",
            data::count("thread", salvaged)
        ));
    }
    Ok(String::new())
}

pub fn board(g: &Game, limit: i64) -> Result<Vec<(u64, String, i64)>> {
    store::lab_board(&g.tx, g.p.guild, g.p.week(), limit)
}
