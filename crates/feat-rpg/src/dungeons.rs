//! Dungeons: fixed delves, one per region. Four rooms, then a boss with its own pattern. The
//! first clear each day opens the boss's chest: a piece of gear (Fine or better), threads and
//! gold. Later clears that day still pay, just less. Fall and you keep half of what you
//! picked up on the way.

use std::collections::BTreeMap;

use bot_core::anyhow::Result;
use bot_core::rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::data::{self, Dungeon};
use crate::emoji::get as e;
use crate::engine::Rng;
use crate::engine::combat::Fight;
use crate::engine::loot;
use crate::game::{Ids, back, count_event, items_line, seed, title};
use crate::quests;
use crate::store::{Game, Player};
use crate::ui::{Btn, Screen};

/// A delve in progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delve {
    pub dungeon: String,
    /// Rooms cleared so far; the boss is after the last room.
    pub room: usize,
    pub fighting: bool,
    pub gold: i64,
    pub xp: i64,
    pub items: BTreeMap<String, i64>,
    pub log: Vec<String>,
}

impl Delve {
    pub fn dungeon(&self) -> &'static Dungeon {
        data::dungeon(&self.dungeon).unwrap_or(&data::DUNGEONS[0])
    }
    fn at_boss(&self) -> bool {
        self.room >= self.dungeon().rooms.len()
    }
}

/// Clears today of a dungeon.
pub fn clears_today(g: &Game, id: &str) -> Result<i64> {
    Ok(g
        .tx
        .query_row(
            "SELECT clears FROM rpg_dclears WHERE guild_id = ?1 AND user_id = ?2 AND dungeon = ?3 AND day = ?4",
            params![g.p.guild as i64, g.p.user as i64, id, quests::day(g.p.now)],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

pub fn list_screen(g: &Game) -> Result<Screen> {
    let p = &g.p;
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    title(&mut s, "dungeon", "Dungeons", "One in each region");
    s.text("Four rooms and a boss, in order. The first clear each day opens the boss's chest: gear (Fine or better), loose threads and gold.");
    for d in data::DUNGEONS {
        let open = p.level >= d.level;
        let today = clears_today(g, d.id)?;
        let region = data::region(d.region).map(|r| r.name).unwrap_or("");
        let status = if !open {
            format!("{} level {}", e("lock"), d.level)
        } else if today == 0 {
            format!("{} chest waiting today", e("gold"))
        } else {
            format!("{} cleared today", e("check"))
        };
        s.section(
            format!(
                "{} **{}** · {region}\n-# {} Fights at {} Power. {status}",
                e("dungeon"),
                d.name,
                d.blurb,
                d.power
            ),
            Btn::new(ids.act("dungeon", "enter", d.id), "Enter")
                .emoji("dungeon")
                .disabled(!open || p.delve.is_some() || p.fight.is_some()),
        );
    }
    s.buttons(vec![
        back(&ids),
        Btn::new(ids.go("map"), "Map").emoji("map"),
    ]);
    Ok(s)
}

pub fn delve_screen(p: &Player, d: &Delve) -> Screen {
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    let dg = d.dungeon();
    title(
        &mut s,
        "dungeon",
        dg.name,
        &format!(
            "Room {} of {} · then the boss",
            (d.room + 1).min(dg.rooms.len() + 1),
            dg.rooms.len()
        ),
    );
    if !d.log.is_empty() {
        s.text(
            d.log
                .iter()
                .map(|l| format!("-# {l}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    let mut haul = Vec::new();
    if d.gold > 0 {
        haul.push(format!("{} {} gold", e("gold"), d.gold));
    }
    let items: Vec<(String, i64)> = d.items.iter().map(|(k, v)| (k.clone(), *v)).collect();
    if !items.is_empty() {
        haul.push(items_line(&items));
    }
    s.text(format!(
        "{} {} / {} HP\n{} Carrying: {}",
        e("hp"),
        p.hp(),
        p.max_hp(),
        e("pack"),
        if haul.is_empty() {
            "nothing yet".into()
        } else {
            haul.join("  ·  ")
        }
    ));
    let next = if d.at_boss() {
        let boss = data::monster(dg.boss).map(|m| m.name).unwrap_or("The boss");
        format!("{} **{boss}** waits in the last room.", e("elite"))
    } else {
        let m = data::monster(dg.rooms[d.room])
            .map(|m| m.name)
            .unwrap_or("Something");
        format!("{} Next room: **{m}**", e("hunt"))
    };
    s.text(next);
    s.buttons(vec![
        Btn::new(
            ids.act("dungeon", "next", ""),
            if d.at_boss() {
                "Face the boss"
            } else {
                "Next room"
            },
        )
        .emoji(if d.at_boss() { "elite" } else { "hunt" })
        .then_danger(d.at_boss())
        .disabled(p.fight.is_some()),
        Btn::new(ids.act("dungeon", "out", ""), "Leave with what you carry").emoji("pack"),
    ]);
    s
}

trait ThenDanger {
    fn then_danger(self, on: bool) -> Self;
}
impl ThenDanger for Btn {
    fn then_danger(self, on: bool) -> Self {
        if on { self.danger() } else { self.primary() }
    }
}

pub fn enter(g: &mut Game, id: &str) -> Result<Option<String>> {
    let Some(dg) = data::dungeon(id) else {
        return Ok(None);
    };
    if g.p.delve.is_some() || g.p.fight.is_some() {
        return Ok(Some("Finish what you're doing first.".into()));
    }
    if g.p.level < dg.level {
        return Ok(Some(format!("{} opens at level {}.", dg.name, dg.level)));
    }
    if g.p.hp() * 10 < g.p.max_hp() * 6 {
        return Ok(Some("Go in with at least 60% HP.".into()));
    }
    g.save_delve(Some(Delve {
        dungeon: id.into(),
        room: 0,
        fighting: false,
        gold: 0,
        xp: 0,
        items: BTreeMap::new(),
        log: vec![format!("You step into {}.", dg.name)],
    }))?;
    g.bump()?;
    Ok(None)
}

pub fn next(g: &mut Game) -> Result<Option<String>> {
    let Some(mut d) = g.p.delve.clone() else {
        return Ok(None);
    };
    if g.p.fight.is_some() {
        return Ok(None);
    }
    let dg = d.dungeon();
    let (monster, mult) = if d.at_boss() {
        (data::monster(dg.boss), 1.0)
    } else {
        (data::monster(dg.rooms[d.room]), 1.0)
    };
    let Some(m) = monster else { return Ok(None) };
    let f = Fight::room(g.p.hero(), g.p.hp(), m, dg.power, mult, seed(&g.p, 151));
    d.fighting = true;
    d.log.clear();
    g.save_fight(Some(f))?;
    g.save_delve(Some(d))?;
    g.bump()?;
    Ok(None)
}

/// A room or the boss ended.
pub fn fight_over(g: &mut Game, f: &Fight, won: bool) -> Result<(String, Option<String>)> {
    let Some(mut d) = g.p.delve.clone() else {
        return Ok((String::new(), None));
    };
    d.fighting = false;
    let dg = d.dungeon();
    if !won {
        let carried = (d.gold / 2, d.xp / 2);
        g.gold(carried.0, dg.name)?;
        g.xp(carried.1)?;
        for (id, n) in &d.items {
            g.item(id, n / 2, dg.name)?;
        }
        g.save_delve(None)?;
        g.set_hp(1)?;
        return Ok((
            format!(
                "{} **You fall in {}.** You crawl out with half of what you carried.",
                e("hp"),
                dg.name
            ),
            None,
        ));
    }
    let mut r = Rng::new(seed(&g.p, 163));
    let region = data::region(dg.region).unwrap_or(&data::REGIONS[0]);
    if !d.at_boss() {
        let (mat, _) = *r.pick(region.drops);
        let gold = 12 + dg.level as i64 * 2;
        d.gold += gold;
        d.xp += 40 + dg.level as i64 * 8;
        *d.items.entry(mat.to_owned()).or_default() += 2;
        d.room += 1;
        let (hp, max) = (f.hp, f.max_hp);
        g.set_hp(hp + max / 10)?;
        let said = format!(
            "{} **{} falls.** +{gold} gold, {}.",
            e("check"),
            f.foe.name,
            data::count(mat, 2)
        );
        d.log = vec![said.clone()];
        g.save_delve(Some(d))?;
        return Ok((said, None));
    }
    // The boss: bank everything, and the chest if it's today's first clear.
    let first = clears_today(g, dg.id)? == 0;
    g.tx.execute(
        "INSERT INTO rpg_dclears (guild_id, user_id, dungeon, day, clears) VALUES (?1, ?2, ?3, ?4, 1)
         ON CONFLICT (guild_id, user_id, dungeon, day) DO UPDATE SET clears = clears + 1",
        params![g.p.guild as i64, g.p.user as i64, dg.id, quests::day(g.p.now)],
    )?;
    g.gold(d.gold, dg.name)?;
    g.xp(d.xp + 150 + dg.level as i64 * 15)?;
    for (id, n) in &d.items {
        g.item(id, *n, dg.name)?;
    }
    let mut said = format!(
        "{} **{} falls.** You carry out {} gold and your materials.",
        e("elite"),
        f.foe.name,
        d.gold
    );
    if first {
        let gold = 100 + 12 * dg.level as i64;
        g.gold(gold, "dungeon chest")?;
        g.item("thread", 2, "dungeon chest")?;
        g.item(region.rare.0, 2, "dungeon chest")?;
        let piece = loot::drop_piece(dg.level.max(g.p.level), g.p.boss_won, &mut r);
        let (mut rarity, mut traits) = loot::roll(piece, "elite", &mut r);
        if rarity == data::Rarity::Common {
            rarity = data::Rarity::Fine;
            traits = loot::roll_traits(piece, 1, &mut r);
        }
        let worn = g.p.equipped(piece.slot).map(|x| x.power()).unwrap_or(0);
        let power =
            data::gear_power(piece, 0) + data::gear_power(piece, 0) * rarity.power_pct() / 100;
        let wear = power > worn && piece.level <= g.p.level;
        g.add_piece(piece, rarity, traits, wear, "dungeon chest")?;
        said.push_str(&format!(
            "\n{} **The chest:** {gold} gold, {}, {}, and a **{} {}**{}",
            e("gold"),
            data::count("thread", 2),
            data::count(region.rare.0, 2),
            rarity.name(),
            piece.name,
            if wear {
                ": you're wearing it."
            } else {
                ": it's in your pack."
            }
        ));
    } else {
        said.push_str("\n-# The chest is empty until tomorrow.");
    }
    g.save_delve(None)?;
    count_event(g, quests::Event::Dungeon(dg.id))?;
    crate::guild::credit(g, 40, 0)?;
    Ok((said, None))
}

pub fn fled(g: &mut Game) -> Result<()> {
    if let Some(mut d) = g.p.delve.clone() {
        d.fighting = false;
        d.log = vec!["You back out of the room.".into()];
        g.save_delve(Some(d))?;
    }
    Ok(())
}

pub fn leave(g: &mut Game) -> Result<Option<String>> {
    let Some(d) = g.p.delve.clone() else {
        return Ok(None);
    };
    if g.p.fight.is_some() {
        return Ok(Some("Finish or flee the fight first.".into()));
    }
    g.gold(d.gold, d.dungeon().name)?;
    g.xp(d.xp)?;
    for (id, n) in &d.items {
        g.item(id, *n, d.dungeon().name)?;
    }
    g.save_delve(None)?;
    g.bump()?;
    Ok(Some(format!(
        "You leave {} with {} gold and your materials. The boss's chest stays shut.",
        d.dungeon().name,
        d.gold
    )))
}
