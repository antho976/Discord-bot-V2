//! A whole player, played by simple rules for a number of days, to check the pace and the
//! economy. Used by the `sim` example and the pace tests. It follows the same rules as the bot
//! (the engine, the recipes, the caps), with the choices a sensible player would make.

use std::collections::BTreeMap;

use super::combat::{End, Fight, autoplay};
use super::expedition::{self, Plan, Rates};
use super::{Hero, progress};
use crate::data::{self, Danger, God, Slot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Two or three short visits a day: two long runs and a few fights.
    Light,
    /// Around six hours a day, mostly hunting.
    Heavy,
}

pub struct SimReport {
    pub days: Vec<String>,
    pub boss_day: Option<u32>,
    pub level_day10: u32,
    pub gold_earned: i64,
    pub gold_spent: i64,
    /// Materials earned and used, per item.
    pub mats: BTreeMap<String, (i64, i64)>,
    pub tribute: i64,
}

impl SimReport {
    pub fn summary(&self) -> String {
        let piles: Vec<String> = self
            .mats
            .iter()
            .filter(|(_, (got, used))| *got > 0 && (*used as f64) < *got as f64 * 0.3)
            .map(|(id, (got, used))| format!("{id} {used}/{got}"))
            .collect();
        format!(
            "boss on day {} · level {} on day 10 · gold {} earned, {} spent · tribute {} · barely used: {}",
            self.boss_day.map(|d| d.to_string()).unwrap_or_else(|| "never".into()),
            self.level_day10,
            self.gold_earned,
            self.gold_spent,
            self.tribute,
            if piles.is_empty() { "none".into() } else { piles.join(", ") }
        )
    }
}

struct P {
    main: God,
    level: u32,
    xp: i64,
    gold: i64,
    items: BTreeMap<String, i64>,
    /// Gear: (id, upgrade), equipped best per slot.
    gear: Vec<(String, i64)>,
    tribute: [i64; 3],
    facts: Vec<String>,
    pity: BTreeMap<String, u32>,
    hp: i64,
    vigor: f64,
    boss_won: bool,
    earned: i64,
    spent: i64,
    mats: BTreeMap<String, (i64, i64)>,
}

impl P {
    fn power(&self) -> i64 {
        let gear: i64 = data::SLOTS.iter().map(|s| self.best(*s).map(|(g, u)| data::gear_power(g, u)).unwrap_or(0)).sum();
        data::level_power(self.level) + gear
    }
    fn best(&self, slot: Slot) -> Option<(&'static data::Gear, i64)> {
        self.gear
            .iter()
            .filter_map(|(id, u)| data::gear(id).map(|g| (g, *u)))
            .filter(|(g, _)| g.slot == slot)
            .max_by_key(|(g, u)| data::gear_power(g, *u))
    }
    fn hero(&self) -> Hero {
        let bonds = [0, 1, 2].map(|i| progress::bond_rank(self.tribute[i], data::BOND_CAP));
        let scouting = data::SLOTS.iter().filter_map(|s| self.best(*s)).map(|(g, _)| g.scouting).sum();
        Hero { main: self.main, lesser: None, bonds, power: self.power(), scouting }
    }
    fn have(&self, id: &str) -> i64 {
        self.items.get(id).copied().unwrap_or(0)
    }
    fn add(&mut self, id: &str, n: i64) {
        *self.items.entry(id.into()).or_default() += n;
        let e = self.mats.entry(id.into()).or_default();
        if n > 0 { e.0 += n } else { e.1 += -n }
    }
    fn gain_gold(&mut self, n: i64) {
        self.gold += n;
        self.earned += n;
    }
    fn spend_gold(&mut self, n: i64) {
        self.gold -= n;
        self.spent += n;
    }
    fn xp(&mut self, n: i64) {
        let (l, x, _) = progress::add_xp(self.level, self.xp, n);
        self.level = l;
        self.xp = x;
    }
    fn can_pay(&self, cost: &[(&str, i64)], gold: i64) -> bool {
        self.gold >= gold && cost.iter().all(|(id, n)| self.have(id) >= *n)
    }
    fn pay(&mut self, cost: &[(&str, i64)], gold: i64) {
        for (id, n) in cost {
            self.add(id, -n);
        }
        self.spend_gold(gold);
    }
    /// The next piece worth making: a slot upgrade at our level.
    fn wanted(&self) -> Option<&'static data::Gear> {
        data::GEAR
            .iter()
            .filter(|g| !g.after_boss || self.boss_won)
            .filter(|g| g.level <= self.level + 2)
            .filter(|g| self.best(g.slot).map(|(b, u)| data::gear_power(b, u)).unwrap_or(0) < g.power)
            .min_by_key(|g| (g.level, g.power))
    }
    fn craft(&mut self) {
        loop {
            let next = data::GEAR.iter().filter(|g| !g.after_boss || self.boss_won).find(|g| {
                g.level <= self.level
                    && self.best(g.slot).map(|(b, u)| data::gear_power(b, u)).unwrap_or(0) < g.power
                    && self.can_pay(g.cost, g.gold)
            });
            let Some(g) = next else { break };
            self.pay(g.cost, g.gold);
            self.gear.push((g.id.into(), 0));
        }
        // Upgrades with spare gold, keeping some for supplies.
        loop {
            let mut done = false;
            for s in data::SLOTS {
                let Some((g, u)) = self.best(s) else { continue };
                if u >= data::MAX_UPGRADE {
                    continue;
                }
                let (gold, mat, n) = data::upgrade_cost(g, u);
                if self.gold - gold > 150 && self.have(mat) >= n + 4 {
                    self.spend_gold(gold);
                    self.add(mat, -n);
                    if let Some(e) = self.gear.iter_mut().find(|(id, up)| id == g.id && *up == u) {
                        e.1 += 1;
                    }
                    done = true;
                }
            }
            if !done {
                break;
            }
        }
        // Supplies: rations always, potions and oils for the boss.
        let r = &data::SUPPLY_RECIPES;
        for (want, recipe) in [("rations", 0usize), ("potion", 3)] {
            let target = if want == "potion" { 3 } else { 2 };
            while self.have(want) < target && self.can_pay(r[recipe].cost, r[recipe].gold) {
                self.pay(r[recipe].cost, r[recipe].gold);
                self.add(want, r[recipe].qty);
            }
        }
        if self.facts.iter().any(|f| f == "weakness") {
            for i in [4usize, 5] {
                while self.have(r[i].makes) < 2 && self.can_pay(r[i].cost, r[i].gold) {
                    self.pay(r[i].cost, r[i].gold);
                    self.add(r[i].makes, r[i].qty);
                }
            }
        }
    }
    /// Where to send the next run: the safest-best danger in the region that drops what the
    /// next piece needs, else the richest region we can handle.
    fn plan(&self, hours: u32) -> Plan {
        let power = self.power();
        let supply = (self.have("rations") > 0).then(|| "rations".to_string());
        let need: Vec<&str> = self
            .wanted()
            .map(|g| g.cost.iter().filter(|(id, n)| self.have(id) < *n).map(|(id, _)| *id).collect())
            .unwrap_or_default();
        let mut best: Option<(f64, Plan)> = None;
        for r in data::REGIONS {
            for d in data::DANGERS {
                if d == Danger::Heroic && !self.boss_won {
                    continue;
                }
                let p = Plan { region: r.id.into(), danger: d, hours, supply: supply.clone() };
                if (p.effective(power) as f64) < p.needs() as f64 * 1.05 {
                    continue;
                }
                let drops_need = r.drops.iter().any(|(id, _)| need.contains(id)) || need.contains(&r.rare.0);
                let intel = if !self.boss_won && self.facts.len() < 4 && r.id == "barrow" { 1.3 } else { 1.0 };
                let score = r.xp * d.loot() * intel * if drops_need { 2.0 } else { 1.0 };
                if best.as_ref().is_none_or(|(s, _)| score > *s) {
                    best = Some((score, p));
                }
            }
        }
        best.map(|(_, p)| p).unwrap_or(Plan { region: "shore".into(), danger: Danger::Normal, hours, supply })
    }
    fn expedition(&mut self, hours: u32, seed: u64) {
        let p = self.plan(hours);
        if p.supply.is_some() {
            self.add("rations", -1);
        }
        let pity = self.pity.get(&p.region).copied().unwrap_or(0);
        let h = self.hero();
        let r = expedition::run(&p, h.power, h.scouting, &self.facts, pity, Rates::default(), seed);
        for (id, n) in &r.items {
            self.add(id, *n);
        }
        self.gain_gold(r.gold);
        self.xp(r.xp);
        self.tribute[self.main as usize] += r.tribute;
        self.pity.insert(p.region.clone(), r.pity);
        if let Some(f) = r.intel {
            self.facts.push(f);
        }
    }
    fn hunt(&mut self, minutes: u32, seed: u64) {
        let power = self.power();
        let Some(region) = data::REGIONS.iter().rev().find(|r| power as f64 >= r.power as f64 * 1.4).or(data::REGIONS.first()) else { return };
        let mut t = 0u32;
        let mut s = seed;
        let max = self.hero().stats().0;
        while t + 2 <= minutes {
            if self.hp < max / 2 {
                // Wait for HP to come back.
                t += 5;
                self.hp = (self.hp as f64 + max as f64 * data::REGEN_PER_MIN * 5.0).min(max as f64) as i64;
                continue;
            }
            s += 1;
            let mut f = Fight::hunt(self.hero(), self.hp, region, s);
            let mut turns = 0;
            while f.end.is_none() && turns < 60 {
                let pot = self.have("potion").min(1);
                let a = autoplay(&f, pot, (0, 0));
                f.act(a, pot, (0, 0)).unwrap();
                for u in f.used.drain(..) {
                    self.add(&u, -1);
                }
                turns += 1;
            }
            t += 1 + turns / 4;
            self.hp = f.hp.max(1);
            if f.end == Some(End::Won) {
                let (xp, gold) = data::hunt_reward(region);
                let double = if self.vigor >= data::VIGOR_PER_WIN as f64 { 2 } else { 1 };
                if double == 2 {
                    self.vigor -= data::VIGOR_PER_WIN as f64;
                }
                self.xp(xp * double);
                self.gain_gold(gold * double);
                let (mat, _) = region.drops[(s % region.drops.len() as u64) as usize];
                self.add(mat, double);
            }
        }
    }
    fn try_boss(&mut self, seed: u64) -> bool {
        if self.boss_won || self.power() < data::BOSS_POWER * 9 / 10 || self.facts.len() < 2 {
            return false;
        }
        let h = self.hero();
        let max = h.stats().0;
        let mut f = Fight::boss(h, max, 0, self.facts.clone(), seed);
        for _ in 0..300 {
            if f.end.is_some() {
                break;
            }
            let pots = self.have("potion");
            let oils = (self.have("fireoil"), self.have("frostoil"));
            let a = autoplay(&f, pots, oils);
            f.act(a, pots, oils).unwrap();
            for u in f.used.drain(..) {
                self.add(&u, -1);
            }
        }
        if f.end == Some(End::Won) {
            self.boss_won = true;
            self.add("shard", 3);
            self.xp(1500);
            self.gain_gold(500);
            return true;
        }
        false
    }
}

/// Play `days` days.
pub fn play(main: God, style: Style, days: u32, seed: u64) -> SimReport {
    let mut p = P {
        main,
        level: 1,
        xp: 0,
        gold: 50,
        items: BTreeMap::new(),
        gear: data::STARTER_GEAR.iter().map(|g| (g.to_string(), 0)).collect(),
        tribute: [0; 3],
        facts: Vec::new(),
        pity: BTreeMap::new(),
        hp: 999,
        vigor: 0.0,
        boss_won: false,
        earned: 0,
        spent: 0,
        mats: BTreeMap::new(),
    };
    p.hp = p.hero().stats().0;
    let mut out = SimReport { days: Vec::new(), boss_day: None, level_day10: 0, gold_earned: 0, gold_spent: 0, mats: BTreeMap::new(), tribute: 0 };
    let mut s = seed * 1000;
    for day in 1..=days {
        // Runs back to back through the day, as each style checks in.
        let runs: &[u32] = match style {
            Style::Light => &[8, 8],
            Style::Heavy => &[8, 4, 4, 4, 4],
        };
        let hunt_minutes = match style {
            Style::Light => 15,
            Style::Heavy => 240,
        };
        p.vigor = (p.vigor + data::VIGOR_PER_HOUR * if style == Style::Light { 22.0 } else { 14.0 }).min(100.0);
        for h in runs {
            s += 1;
            p.expedition(*h, s);
            p.craft();
            // Vigor builds while away; HP is back after a run.
            p.hp = p.hero().stats().0;
        }
        s += 1;
        p.hunt(hunt_minutes, s);
        p.craft();
        // Spare materials, and gold past what the next pieces need, go to the altar.
        let keep_gold = 1500;
        if p.gold > keep_gold {
            let give = p.gold - keep_gold;
            p.spend_gold(give);
            p.tribute[p.main as usize] += give;
        }
        let spare: Vec<(String, i64)> = p
            .items
            .iter()
            .filter(|(id, n)| data::item(id).is_some_and(|i| i.kind == data::Kind::Material) && **n > 60)
            .map(|(id, n)| (id.clone(), n - 60))
            .collect();
        for (id, n) in spare {
            p.add(&id, -n);
            p.tribute[p.main as usize] += n * data::item(&id).map(|i| i.tribute).unwrap_or(0);
        }
        s += 1;
        if out.boss_day.is_none() && p.try_boss(s) {
            out.boss_day = Some(day);
        }
        if day == 10 {
            out.level_day10 = p.level;
        }
        let gear: Vec<String> = data::SLOTS.iter().filter_map(|sl| p.best(*sl)).map(|(g, u)| if u > 0 { format!("{}+{u}", g.id) } else { g.id.into() }).collect();
        out.days.push(format!(
            "day {day:>2}: level {:>2}, Power {:>3}, gold {:>5}, bond {}, intel {}/4 | {}",
            p.level,
            p.power(),
            p.gold,
            progress::bond_rank(p.tribute[p.main as usize], data::BOND_CAP),
            p.facts.len(),
            gear.join(", ")
        ));
    }
    out.gold_earned = p.earned;
    out.gold_spent = p.spent;
    out.mats = p.mats;
    out.tribute = p.tribute.iter().sum();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pace from the plan: a light player reaches the first boss in about ten days, a
    /// heavy one sooner but not in a day or two, for every patron.
    #[test]
    fn pace_matches_the_plan() {
        for main in data::GODS {
            let light = play(main, Style::Light, 20, 3).boss_day.unwrap_or(99);
            let heavy = play(main, Style::Heavy, 20, 3).boss_day.unwrap_or(99);
            assert!((8..=13).contains(&light), "{main:?}: light player beat the boss on day {light}");
            assert!((4..=9).contains(&heavy), "{main:?}: heavy player beat the boss on day {heavy}");
            assert!(heavy <= light, "{main:?}: heavy {heavy} slower than light {light}");
        }
    }
}
