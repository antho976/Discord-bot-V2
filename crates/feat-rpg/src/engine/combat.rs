//! Turn-based fights: hunts and the realm boss. One call to [`Fight::act`] is one turn: your
//! move, then the foe's, then the foe shows what it will do next. The whole fight is plain data
//! so it's stored between clicks and replays from its seed.

use serde::{Deserialize, Serialize};

use super::{Hero, Rng};
use crate::data::{self, God, Move, Skill};

pub const MAX_STAMINA: i32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Strike,
    Guard,
    Potion,
    Oil(Element),
    Skill(SkillId),
}

/// A skill, stored by id so fights deserialize even if skills are renamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillId(pub u8);

impl SkillId {
    pub fn of(s: Skill) -> Self {
        Self(data::SKILLS.iter().position(|k| *k == s).unwrap_or(0) as u8)
    }
    pub fn skill(self) -> Skill {
        data::SKILLS[self.0 as usize % data::SKILLS.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Element {
    Fire,
    Frost,
}

impl Element {
    pub fn id(self) -> &'static str {
        match self {
            Element::Fire => "fire",
            Element::Frost => "frost",
        }
    }
    pub fn oil(self) -> &'static str {
        match self {
            Element::Fire => "fireoil",
            Element::Frost => "frostoil",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Hunt,
    Boss,
    /// A floor of the Labyrinth.
    Lab,
    /// A room of a dungeon.
    Dungeon,
    /// An attempt on a shared boss: a Titan or a guild raid.
    Shared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum End {
    Won,
    Lost,
    /// A shared-boss attempt ran out of turns.
    Spent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Foe {
    pub id: String,
    pub name: String,
    pub hp: i64,
    pub max_hp: i64,
    pub atk: i64,
    pub step: usize,
    pub phase: u8,
    pub weak: Option<String>,
    pub braced: bool,
    pub burn: u32,
    pub bleed: u32,
    pub bleed_dmg: i64,
    pub cursed: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fight {
    pub kind: Kind,
    pub region: String,
    pub hero: Hero,
    pub foe: Foe,
    pub hp: i64,
    pub max_hp: i64,
    pub atk: i64,
    pub stamina: i32,
    pub turn: u32,
    /// Strikes stored up for Mjolnir (Thor).
    pub charge: u32,
    /// (skill, turns left).
    pub cooldowns: Vec<(SkillId, u32)>,
    pub coat: Option<(Element, u32)>,
    pub block_next: bool,
    pub reflect: bool,
    pub counter: bool,
    pub guarding: bool,
    pub heal_turns: u32,
    pub shield: i64,
    pub sight: u32,
    pub bonus_next: f64,
    /// What happened last turn, newest last.
    pub log: Vec<String>,
    /// Boss facts this player has scouted.
    pub scouted: Vec<String>,
    pub rng: Rng,
    pub end: Option<End>,
    /// Items the fight used up, for the caller to take from the pack.
    pub used: Vec<String>,
    /// Who stands with you (the saga's choice at the völva), and whether their help is spent.
    #[serde(default)]
    pub ally: Option<String>,
    #[serde(default)]
    pub ally_used: bool,
    /// Turns an attempt lasts (shared bosses).
    #[serde(default)]
    pub limit: Option<u32>,
}

fn scale(r: &mut Rng, v: f64) -> i64 {
    (v * r.range(0.85, 1.15)).round().max(1.0) as i64
}

impl Fight {
    /// A hunt against one of the region's monsters (now and then its elite), starting at `hp`.
    pub fn hunt(hero: Hero, hp: i64, region: &data::Region, seed: u64) -> Fight {
        Self::hunt_with(hero, hp, region, seed, true)
    }

    /// A hunt, with or without the chance of meeting the region's elite.
    pub fn hunt_with(hero: Hero, hp: i64, region: &data::Region, seed: u64, elites: bool) -> Fight {
        let mut rng = Rng::new(seed);
        let regular = data::monster(rng.pick(region.monsters)).unwrap_or(&data::MONSTERS[0]);
        let elite = rng.chance(data::ELITE_CHANCE);
        let m = if elite && elites {
            data::elite(region.id).unwrap_or(regular)
        } else {
            regular
        };
        let p = region.power as f64;
        let foe = Foe {
            id: m.id.into(),
            name: m.name.into(),
            hp: (p * m.hp * data::HUNT_HP) as i64,
            max_hp: (p * m.hp * data::HUNT_HP) as i64,
            atk: (p * m.atk * data::HUNT_ATK).max(3.0) as i64,
            step: (rng.next_u64() % m.pattern.len() as u64) as usize,
            phase: 1,
            weak: m.weak.map(str::to_owned),
            braced: false,
            burn: 0,
            bleed: 0,
            bleed_dmg: 0,
            cursed: 0,
        };
        Self::start(Kind::Hunt, region.id, hero, hp, foe, Vec::new(), rng)
    }

    /// A Labyrinth floor: `monster` at `power`, harder by `mult`.
    pub fn floor(
        hero: Hero,
        hp: i64,
        monster: &data::Monster,
        power: i64,
        mult: f64,
        seed: u64,
    ) -> Fight {
        let mut rng = Rng::new(seed);
        let p = power as f64 * mult;
        let foe = Foe {
            id: monster.id.into(),
            name: monster.name.into(),
            hp: (p * monster.hp * data::HUNT_HP) as i64,
            max_hp: (p * monster.hp * data::HUNT_HP) as i64,
            atk: (p * monster.atk * data::HUNT_ATK).max(3.0) as i64,
            step: (rng.next_u64() % monster.pattern.len() as u64) as usize,
            phase: 1,
            weak: monster.weak.map(str::to_owned),
            braced: false,
            burn: 0,
            bleed: 0,
            bleed_dmg: 0,
            cursed: 0,
        };
        Self::start(Kind::Lab, "labyrinth", hero, hp, foe, Vec::new(), rng)
    }

    /// A dungeon room or boss: like a floor, but its own kind.
    pub fn room(
        hero: Hero,
        hp: i64,
        monster: &data::Monster,
        power: i64,
        mult: f64,
        seed: u64,
    ) -> Fight {
        let mut f = Self::floor(hero, hp, monster, power, mult, seed);
        f.kind = Kind::Dungeon;
        f.region = "dungeon".into();
        f
    }

    /// An attempt on a shared boss: it has far more HP than one attempt can take, and the
    /// attempt ends after `turns`. What you take off goes to everyone's total.
    pub fn shared(
        hero: Hero,
        hp: i64,
        monster: &data::Monster,
        power: i64,
        turns: u32,
        id: i64,
        seed: u64,
    ) -> Fight {
        let mut f = Self::floor(hero, hp, monster, power, 1.0, seed);
        f.kind = Kind::Shared;
        f.region = id.to_string();
        f.foe.hp *= 8;
        f.foe.max_hp *= 8;
        f.limit = Some(turns);
        f
    }

    /// Damage dealt so far (shared bosses count this).
    pub fn dealt(&self) -> i64 {
        (self.foe.max_hp - self.foe.hp).max(0)
    }

    /// The realm boss. `week` decides its weakness; `scouted` is what the player knows.
    pub fn boss(hero: Hero, hp: i64, week: i64, scouted: Vec<String>, seed: u64) -> Fight {
        let foe = Foe {
            id: data::BOSS_ID.into(),
            name: data::BOSS_NAME.into(),
            hp: data::BOSS_HP,
            max_hp: data::BOSS_HP,
            atk: data::BOSS_ATK,
            step: 0,
            phase: 1,
            weak: Some(data::boss_weakness(week).into()),
            braced: false,
            burn: 0,
            bleed: 0,
            bleed_dmg: 0,
            cursed: 0,
        };
        Self::start(Kind::Boss, "barrow", hero, hp, foe, scouted, Rng::new(seed))
    }

    fn start(
        kind: Kind,
        region: &str,
        hero: Hero,
        hp: i64,
        foe: Foe,
        scouted: Vec<String>,
        rng: Rng,
    ) -> Fight {
        let (max_hp, atk) = hero.stats();
        let stamina = (3 + hero.mods.stamina).min(MAX_STAMINA);
        let mut f = Fight {
            kind,
            region: region.into(),
            hero,
            foe,
            hp: hp.clamp(1, max_hp),
            max_hp,
            atk,
            stamina,
            turn: 1,
            charge: 0,
            cooldowns: Vec::new(),
            coat: None,
            block_next: false,
            reflect: false,
            counter: false,
            guarding: false,
            heal_turns: 0,
            shield: 0,
            sight: 0,
            bonus_next: 0.0,
            log: Vec::new(),
            scouted,
            rng,
            end: None,
            used: Vec::new(),
            ally: None,
            ally_used: false,
            limit: None,
        };
        f.log.push(format!("{} blocks your way.", f.foe.name));
        f
    }

    /// Bring an ally into a boss fight. The völva tells you one thing you haven't scouted.
    pub fn with_ally(mut self, ally: Option<&str>) -> Fight {
        if self.kind != Kind::Boss {
            return self;
        }
        self.ally = ally.map(str::to_owned);
        match ally {
            Some("volva") => {
                if let Some(f) = data::FACTS
                    .iter()
                    .find(|f| !self.scouted.iter().any(|k| k == *f))
                {
                    self.scouted.push(f.to_string());
                    self.log.push(format!(
                        "The völva whispers: \"{}.\"",
                        data::fact_text(f, 0)
                            .split(" this week")
                            .next()
                            .unwrap_or("")
                    ));
                }
                self.ally_used = true;
            }
            Some("shieldmaiden") => self.log.push("Eydis raises her shield beside you.".into()),
            Some("jarlsmen") => self.log.push("The jarl's men bang their shields.".into()),
            _ => {}
        }
        self
    }

    fn pattern(&self) -> &'static [Move] {
        match self.kind {
            Kind::Boss if self.foe.phase == 2 => data::BOSS_P2,
            Kind::Boss => data::BOSS_P1,
            _ => data::monster(&self.foe.id)
                .map(|m| m.pattern)
                .unwrap_or(&[Move::Hit]),
        }
    }

    /// The foe's move this turn.
    pub fn intent(&self) -> Move {
        let p = self.pattern();
        p[self.foe.step % p.len()]
    }

    /// The foe's moves after this one, when Owl's Sight shows them.
    pub fn upcoming(&self) -> Vec<Move> {
        let p = self.pattern();
        (1..=self.sight as usize)
            .map(|i| p[(self.foe.step + i) % p.len()])
            .collect()
    }

    pub fn knows(&self, fact: &str) -> bool {
        self.kind != Kind::Boss || self.scouted.iter().any(|f| f == fact)
    }

    /// What the player sees of the foe's next move, and whether it's thanks to scouting.
    pub fn telegraph(&self) -> (String, bool) {
        let boss = self.kind == Kind::Boss;
        match self.intent() {
            Move::Hit => ("It readies a strike".into(), false),
            Move::Brace => (
                "It raises its guard: your next hit lands at half".into(),
                false,
            ),
            Move::Heavy if boss && self.knows("slam") => {
                ("Lifts the barrow stone: it slams next turn".into(), true)
            }
            Move::Heavy if boss => ("It shifts its weight. Something is coming".into(), false),
            Move::Heavy => {
                let w = data::monster(&self.foe.id)
                    .map(|m| m.windup)
                    .unwrap_or("winds up");
                (format!("It {w}: a heavy blow lands next turn"), false)
            }
            Move::Roar if boss && self.knows("roar") => (
                "It fills its lungs: a roar that drains 2 stamina unless you guard".into(),
                true,
            ),
            Move::Roar if boss => ("It fills its lungs".into(), false),
            Move::Roar => ("It's about to howl: guard or lose stamina".into(), false),
        }
    }

    pub fn cooldown(&self, s: Skill) -> u32 {
        let id = SkillId::of(s);
        self.cooldowns
            .iter()
            .find(|(k, _)| *k == id)
            .map(|(_, t)| *t)
            .unwrap_or(0)
    }

    /// Why an action can't be used now, if it can't.
    pub fn blocked(&self, a: Action, potions: i64, oils: (i64, i64)) -> Option<String> {
        match a {
            Action::Strike if self.stamina < 1 => Some("Not enough stamina".into()),
            Action::Potion if potions < 1 => Some("No potions left".into()),
            Action::Oil(Element::Fire) if oils.0 < 1 => Some("No fire oil".into()),
            Action::Oil(Element::Frost) if oils.1 < 1 => Some("No frost oil".into()),
            Action::Skill(id) => {
                let s = id.skill();
                if !self.hero.skills().contains(&s) {
                    Some("Not unlocked".into())
                } else if self.cooldown(s) > 0 {
                    Some(format!("Ready in {} turns", self.cooldown(s)))
                } else if self.stamina < s.stamina() {
                    Some("Not enough stamina".into())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Your hit for `mult` of your attack, with every modifier. Returns the damage dealt.
    fn hit(&mut self, mult: f64, skill: bool) -> i64 {
        let mut m = mult;
        if skill {
            m *= self.hero.skill_mult();
        }
        if self.hero.lesser == Some(God::Thor) {
            m *= 1.06;
        }
        if self.bonus_next > 0.0 {
            m *= 1.0 + self.bonus_next;
            self.bonus_next = 0.0;
        }
        if self.foe.cursed > 0 {
            m *= 1.6;
        }
        if self.ally.as_deref() == Some("jarlsmen") {
            m *= 1.08;
        }
        m *= 1.0 + self.hero.mods.damage;
        match self.foe.weak.as_deref() {
            Some("fire") => m *= 1.0 + self.hero.mods.fire,
            Some("frost") => m *= 1.0 + self.hero.mods.frost,
            _ => {}
        }
        if let Some((el, left)) = self.coat {
            let weak = self.foe.weak.as_deref() == Some(el.id());
            m *= if weak { 1.6 } else { 1.1 };
            if weak {
                self.foe.burn = self.foe.burn.max(2);
            }
            self.coat = (left > 1).then_some((el, left - 1));
        }
        if self.foe.braced {
            m *= 0.5;
            self.foe.braced = false;
        }
        let crit = self.rng.chance(0.10 + self.hero.mods.crit);
        if crit {
            m *= 1.5;
        }
        let dmg = scale(&mut self.rng, self.atk as f64 * m);
        if crit {
            self.log.push("A critical hit.".into());
        }
        self.foe.hp -= dmg;
        let mut steal = self.hero.main.lifesteal()
            + if self.hero.lesser == Some(God::Hades) {
                0.04
            } else {
                0.0
            }
            + self.hero.mods.steal;
        if self.kind == Kind::Boss {
            // The dead don't bleed much: lifesteal is weaker against the realm boss.
            steal *= data::BOSS_LIFESTEAL;
        }
        if steal > 0.0 {
            self.heal((dmg as f64 * steal) as i64);
        }
        dmg
    }

    fn heal(&mut self, n: i64) {
        self.hp = (self.hp + n.max(0)).min(self.max_hp);
    }

    fn set_cooldown(&mut self, s: Skill) {
        let id = SkillId::of(s);
        self.cooldowns.retain(|(k, _)| *k != id);
        if s.cooldown() > 0 {
            self.cooldowns.push((id, s.cooldown() + 1));
        }
    }

    /// Play one turn. Returns an error message, changing nothing, when the action isn't allowed.
    pub fn act(&mut self, a: Action, potions: i64, oils: (i64, i64)) -> Result<(), String> {
        if self.end.is_some() {
            return Err("The fight is over".into());
        }
        if let Some(why) = self.blocked(a, potions, oils) {
            return Err(why);
        }
        self.log.clear();
        self.guarding = false;
        self.counter = false;
        let intent = self.intent();
        let mut stunned = false;
        let name = self.foe.name.clone();
        match a {
            Action::Strike => {
                self.stamina -= 1;
                let d = self.hit(1.0, false);
                if self.hero.main == God::Thor {
                    self.charge = (self.charge + 1).min(3);
                }
                self.log.push(format!("You struck for {d}."));
            }
            Action::Guard => {
                self.guarding = true;
                self.stamina += 1 + i32::from(self.hero.lesser == Some(God::Athena));
                self.log.push("You raised your guard.".into());
            }
            Action::Potion => {
                let n = (self.max_hp as f64 * 0.4) as i64;
                self.heal(n);
                self.used.push("potion".into());
                self.log.push(format!("You drank a potion and healed {n}."));
            }
            Action::Oil(el) => {
                self.coat = Some((el, 3));
                self.used.push(el.oil().into());
                let weak = self.foe.weak.as_deref() == Some(el.id());
                self.log.push(format!(
                    "You coated your weapon in {} oil.{}",
                    el.id(),
                    if weak && self.knows("weakness") {
                        " It's weak to it."
                    } else {
                        ""
                    }
                ));
            }
            Action::Skill(id) => {
                let s = id.skill();
                self.stamina -= s.stamina();
                self.set_cooldown(s);
                let main = self.hero.main;
                match s {
                    Skill::Mjolnir => {
                        let d = self.hit(1.7 + 0.2 * self.charge as f64, true);
                        self.log.push(format!(
                            "Mjolnir landed for {d}. It flies back to you next turn."
                        ));
                        self.charge = 0;
                    }
                    Skill::Thunderclap => {
                        let d = self.hit(1.7, true);
                        if intent == Move::Heavy || intent == Move::Roar {
                            stunned = true;
                            self.log.push(format!(
                                "Thunderclap hit for {d} and stunned it. Its attack is cancelled."
                            ));
                        } else {
                            self.log.push(format!("Thunderclap hit for {d}."));
                        }
                    }
                    Skill::SpearCounter => {
                        self.guarding = true;
                        self.counter = true;
                        self.log.push("You set your spear to counter.".into());
                    }
                    Skill::OwlSight => {
                        self.sight = 2;
                        self.bonus_next = 0.6;
                        self.log.push(
                            "The owl shows you its next moves. Your next hit lands harder.".into(),
                        );
                    }
                    Skill::SoulRend => {
                        let d = self.hit(1.6, true);
                        let h = d * 45 / 100;
                        self.heal(h);
                        self.log
                            .push(format!("Soul Rend tore for {d} and healed you {h}."));
                    }
                    Skill::CerberusBite => {
                        let d = self.hit(0.8, true);
                        self.foe.bleed = 3;
                        self.foe.bleed_dmg =
                            (self.atk as f64 * 0.6 * self.hero.skill_mult()) as i64;
                        self.log
                            .push(format!("Cerberus Bite hit for {d}. It bleeds."));
                    }
                    Skill::AegisStorm if main == God::Athena => {
                        self.block_next = true;
                        self.reflect = true;
                        self.log.push(
                            "You raised the Aegis. The next hit comes back as lightning.".into(),
                        );
                    }
                    Skill::AegisStorm => {
                        let d = self.hit(1.3, true);
                        self.block_next = true;
                        self.log.push(format!(
                            "Aegis Storm bashed for {d}. The next hit can't touch you."
                        ));
                    }
                    Skill::GraveThunder if main == God::Hades => {
                        let d = self.hit(0.8, true);
                        self.foe.cursed = 4;
                        self.log.push(format!("Grave Thunder hit for {d} and cursed it: it takes 60% more for three turns."));
                    }
                    Skill::GraveThunder => {
                        let d = self.hit(2.0, true);
                        self.heal(d / 3);
                        self.log.push(format!(
                            "Grave Thunder struck for {d} and healed you {}.",
                            d / 3
                        ));
                    }
                    Skill::Judgement if main == God::Hades => {
                        let d = self.hit(1.2, true);
                        self.heal(d / 2);
                        self.shield = d;
                        self.log.push(format!(
                            "Judgement drained {d}, healed you and shielded you for it."
                        ));
                    }
                    Skill::Judgement => {
                        self.guarding = true;
                        self.heal_turns = 3;
                        self.log
                            .push("Judgement: you guard and will heal for three turns.".into());
                    }
                }
            }
        }

        // Damage over time.
        if self.foe.burn > 0 && self.foe.hp > 0 {
            let d = (self.atk as f64 * 0.3) as i64;
            self.foe.hp -= d;
            self.foe.burn -= 1;
            self.log.push(format!("It burns for {d}."));
        }
        if self.foe.bleed > 0 && self.foe.hp > 0 {
            self.foe.hp -= self.foe.bleed_dmg;
            self.foe.bleed -= 1;
            self.log
                .push(format!("It bleeds for {}.", self.foe.bleed_dmg));
        }
        if self.foe.hp <= 0 {
            self.foe.hp = 0;
            self.end = Some(End::Won);
            self.log.push(format!("{name} falls."));
            return Ok(());
        }

        // Phase change.
        if self.kind == Kind::Boss && self.foe.phase == 1 && self.foe.hp * 2 < self.foe.max_hp {
            self.foe.phase = 2;
            self.foe.step = 0;
            self.foe.atk = (self.foe.atk as f64 * data::BOSS_P2_ATK) as i64;
            self.log
                .push("The Barrow King rises in fury. Phase 2.".into());
        }

        // The foe's move.
        if stunned {
            // Cancelled.
        } else {
            self.foe_moves(intent);
        }
        if self.hp <= 0 {
            self.hp = 0;
            self.end = Some(End::Lost);
            self.log.push("You fall. You wake up back at camp.".into());
            return Ok(());
        }

        // End of turn.
        if self.heal_turns > 0 {
            let n = self.max_hp / 10;
            self.heal(n);
            self.heal_turns -= 1;
        }
        self.foe.cursed = self.foe.cursed.saturating_sub(1);
        for (_, t) in self.cooldowns.iter_mut() {
            *t = t.saturating_sub(1);
        }
        self.cooldowns.retain(|(_, t)| *t > 0);
        self.sight = self.sight.saturating_sub(1);
        self.stamina = (self.stamina + 1).min(MAX_STAMINA);
        self.foe.step += 1;
        self.turn += 1;
        if let Some(limit) = self.limit
            && self.turn > limit
        {
            self.end = Some(End::Spent);
            self.log
                .push(format!("{name} throws you back. Your attempt is over."));
        }
        Ok(())
    }

    fn take(&mut self, raw: i64) -> i64 {
        let mut d = raw;
        if self.block_next {
            self.block_next = false;
            if self.reflect {
                self.reflect = false;
                let back = raw;
                self.foe.hp -= back;
                self.log
                    .push(format!("The Aegis threw {back} back as lightning."));
            }
            return 0;
        }
        if self.guarding {
            let stop = (self.hero.main.body().2 + self.hero.mods.guard).min(0.85);
            d = (d as f64 * (1.0 - stop)) as i64;
        }
        if self.shield > 0 {
            let s = self.shield.min(d);
            self.shield -= s;
            d -= s;
        }
        self.hp -= d;
        d
    }

    fn foe_moves(&mut self, m: Move) {
        let atk = self.foe.atk as f64;
        match m {
            Move::Hit => {
                let raw = scale(&mut self.rng, atk);
                let d = self.take(raw);
                self.log.push(format!("It hit you for {d}."));
                self.counter_back();
            }
            Move::Heavy if self.ally.as_deref() == Some("shieldmaiden") && !self.ally_used => {
                self.ally_used = true;
                self.log
                    .push("Eydis steps in front of you and takes the slam on her shield.".into());
            }
            Move::Heavy => {
                let raw = scale(&mut self.rng, atk * 2.4);
                // Reading the warning and guarding it opens the foe up. Not the realm boss: it is
                // balanced around guarding the slam already, and Hades' lifesteal ran away with it.
                let perfect = self.guarding && self.kind != Kind::Boss;
                let d = self.take(raw);
                if perfect {
                    self.bonus_next = self.bonus_next.max(0.5);
                    self.log.push(
                        "A perfect guard. It's off balance: your next hit lands 50% harder.".into(),
                    );
                }
                let what = if self.kind == Kind::Boss {
                    "It slammed the stone down"
                } else {
                    "Its heavy blow landed"
                };
                self.log.push(format!("{what} for {d}."));
                self.counter_back();
            }
            Move::Brace => {
                self.foe.braced = true;
                self.log.push("It braced itself.".into());
            }
            Move::Roar => {
                let raw = scale(&mut self.rng, atk * 0.5);
                let d = self.take(raw);
                if self.guarding {
                    self.log
                        .push(format!("It roared. Your guard held ({d} damage)."));
                } else {
                    self.stamina = (self.stamina - 2).max(0);
                    self.log
                        .push(format!("It roared for {d} and drained 2 stamina."));
                }
            }
        }
    }

    fn counter_back(&mut self) {
        if self.counter && self.hp > 0 {
            self.counter = false;
            let d = self.hit(1.6, true);
            self.log.push(format!("Your spear countered for {d}."));
            if self.foe.hp <= 0 {
                self.foe.hp = 0;
            }
        }
    }
}

/// A fixed, decent way to fight, for the simulator and the balance tests: guard what's
/// telegraphed, use skills when they're up, drink a potion when low.
pub fn autoplay(f: &Fight, potions: i64, oils: (i64, i64)) -> Action {
    let intent = f.intent();
    let low = f.hp * 100 / f.max_hp.max(1) < 35;
    if low && potions > 0 {
        return Action::Potion;
    }
    if f.coat.is_none()
        && f.turn == 1
        && f.knows("weakness")
        && let Some(w) = f.foe.weak.as_deref()
    {
        let el = if w == "fire" {
            Element::Fire
        } else {
            Element::Frost
        };
        if f.blocked(Action::Oil(el), potions, oils).is_none() {
            return Action::Oil(el);
        }
    }
    let skills = f.hero.skills();
    let ready = |s: Skill| {
        skills.contains(&s)
            && f.blocked(Action::Skill(SkillId::of(s)), potions, oils)
                .is_none()
    };
    if matches!(intent, Move::Heavy | Move::Roar) {
        for s in [
            Skill::Thunderclap,
            Skill::SpearCounter,
            Skill::AegisStorm,
            Skill::Judgement,
        ] {
            if ready(s) {
                return Action::Skill(SkillId::of(s));
            }
        }
        // Without scouting the boss's slam is a vague warning: a player guards it about half
        // the time.
        if f.kind != Kind::Boss
            || f.knows(if intent == Move::Heavy {
                "slam"
            } else {
                "roar"
            })
            || f.turn.is_multiple_of(2)
        {
            return Action::Guard;
        }
    }
    // Its guard is up and it won't attack: build stamina, or poke it if full.
    if intent == Move::Brace {
        return if f.stamina >= 4 {
            Action::Strike
        } else {
            Action::Guard
        };
    }
    // Hades curses first when there's stamina to follow it up.
    if f.hero.main == God::Hades && f.stamina >= 4 && ready(Skill::GraveThunder) {
        return Action::Skill(SkillId::of(Skill::GraveThunder));
    }
    for s in [
        Skill::GraveThunder,
        Skill::Mjolnir,
        Skill::SoulRend,
        Skill::CerberusBite,
        Skill::OwlSight,
    ] {
        if s == Skill::GraveThunder && f.hero.main == God::Hades {
            continue;
        }
        if ready(s) && (s != Skill::Mjolnir || f.charge >= 2 || f.stamina >= 4) {
            return Action::Skill(SkillId::of(s));
        }
    }
    if f.stamina >= 1 {
        Action::Strike
    } else {
        Action::Guard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hero(main: God, lesser: Option<God>, power: i64) -> Hero {
        Hero {
            main,
            lesser,
            bonds: [2, 2, 2],
            power,
            scouting: 0,
            mods: Default::default(),
        }
    }

    fn run(mut f: Fight) -> Fight {
        let (mut potions, mut oils) = (3i64, (2i64, 2i64));
        for _ in 0..200 {
            if f.end.is_some() {
                break;
            }
            let a = autoplay(&f, potions, oils);
            f.act(a, potions, oils).unwrap();
            for u in f.used.drain(..) {
                match u.as_str() {
                    "potion" => potions -= 1,
                    "fireoil" => oils.0 -= 1,
                    "frostoil" => oils.1 -= 1,
                    _ => {}
                }
            }
        }
        f
    }

    #[test]
    fn same_seed_same_fight() {
        let r = data::region("wolfwood").unwrap();
        let a = run(Fight::hunt(hero(God::Thor, None, 150), 999, r, 7));
        let b = run(Fight::hunt(hero(God::Thor, None, 150), 999, r, 7));
        assert_eq!(a.turn, b.turn);
        assert_eq!(a.hp, b.hp);
    }

    #[test]
    fn refused_actions_change_nothing() {
        let r = data::region("shore").unwrap();
        let mut f = Fight::hunt(hero(God::Athena, None, 60), 999, r, 1);
        let before = (f.hp, f.turn, f.stamina);
        assert!(f.act(Action::Potion, 0, (0, 0)).is_err());
        assert!(
            f.act(Action::Skill(SkillId::of(Skill::Mjolnir)), 0, (0, 0))
                .is_err()
        );
        assert_eq!(before, (f.hp, f.turn, f.stamina));
    }

    fn rate(main: God, lesser: Option<God>, power: i64, intel: bool) -> u32 {
        let facts: Vec<String> = if intel {
            data::FACTS.iter().map(|s| s.to_string()).collect()
        } else {
            Vec::new()
        };
        (0..60u64)
            .filter(|seed| {
                let h = hero(main, lesser, power);
                let (max, _) = h.stats();
                run(Fight::boss(
                    h,
                    max,
                    *seed as i64,
                    facts.clone(),
                    seed * 7 + 1,
                ))
                .end == Some(End::Won)
            })
            .count() as u32
    }

    /// The first boss comes before any lesser vow: every patron alone, at bond 2, beats it
    /// most of the time at the recommended Power with intel, sometimes without, rarely well
    /// under it, and never far under it.
    #[test]
    fn boss_pace_for_every_patron() {
        for main in data::GODS {
            let full = rate(main, None, data::BOSS_POWER, true);
            let blind = rate(main, None, data::BOSS_POWER, false);
            let under = rate(main, None, data::BOSS_POWER * 8 / 10, true);
            let far = rate(main, None, data::BOSS_POWER * 6 / 10, false);
            assert!(
                (40..=60).contains(&full),
                "{main:?}: {full}/60 at recommended Power with intel"
            );
            assert!(
                blind < full,
                "{main:?}: intel should help ({blind} without, {full} with)"
            );
            assert!(under <= 30, "{main:?}: {under}/60 at 80% Power");
            assert_eq!(far, 0, "{main:?}: won at 60% Power with no intel");
        }
    }

    /// A lesser vow makes you stronger, never weaker.
    #[test]
    fn lesser_vows_help() {
        for main in data::GODS {
            let solo = rate(main, None, data::BOSS_POWER * 9 / 10, true);
            for lesser in data::GODS.into_iter().filter(|l| *l != main) {
                let pair = rate(main, Some(lesser), data::BOSS_POWER * 9 / 10, true);
                assert!(
                    pair + 12 >= solo,
                    "{main:?}+{lesser:?}: {pair}/60, alone {solo}/60"
                );
            }
        }
    }
}
