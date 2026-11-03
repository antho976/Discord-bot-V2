//! Every piece of content and every balance number in one place: patrons, skills, regions,
//! monsters, the boss, materials, supplies, gear and the curves. Tuned with
//! `cargo run -p feat-rpg --example sim`.

/// The three patrons at launch. Their order is their index everywhere (bonds, pairs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum God {
    Thor,
    Athena,
    Hades,
}

pub const GODS: [God; 3] = [God::Thor, God::Athena, God::Hades];

impl God {
    pub fn id(self) -> &'static str {
        match self {
            God::Thor => "thor",
            God::Athena => "athena",
            God::Hades => "hades",
        }
    }
    pub fn from_id(s: &str) -> Option<God> {
        GODS.into_iter().find(|g| g.id() == s)
    }
    pub fn name(self) -> &'static str {
        match self {
            God::Thor => "Thor",
            God::Athena => "Athena",
            God::Hades => "Hades",
        }
    }
    /// How the god plays, on the patron choice.
    pub fn pitch(self) -> &'static str {
        match self {
            God::Thor => "Throws Mjolnir and calls thunder. Hits hard, takes hits.",
            God::Athena => "Counters with her spear and sees what's coming.",
            God::Hades => "Drains and curses. Thrives on dangerous expeditions.",
        }
    }
    /// The passive a lesser vow gives, at about half strength.
    pub fn lesser_passive(self) -> &'static str {
        match self {
            God::Thor => "+6% damage",
            God::Athena => "Guarding restores 1 more stamina",
            God::Hades => "Your hits heal you for 4% of the damage, on top of your patron's",
        }
    }
    /// The week's blessing when this god's altar got the most tribute.
    pub fn blessing(self) -> (&'static str, &'static str) {
        match self {
            God::Thor => ("Thor's Might", "+10% rewards from hunts"),
            God::Athena => ("Athena's Wisdom", "+10% XP from everything"),
            God::Hades => ("Hades' Wealth", "+10% loot from expeditions"),
        }
    }
    /// Max HP, attack and how much a Guard stops.
    pub fn body(self) -> (f64, f64, f64) {
        match self {
            God::Thor => (1.10, 1.0, 0.60),
            God::Athena => (1.05, 1.20, 0.70),
            God::Hades => (1.00, 0.98, 0.50),
        }
    }
    /// Part of every hit that heals the player.
    pub fn lifesteal(self) -> f64 {
        match self {
            God::Hades => 0.20,
            _ => 0.0,
        }
    }
    /// The patron's two skills: the first on swearing, the second at bond 2.
    pub fn skills(self) -> [Skill; 2] {
        match self {
            God::Thor => [Skill::Mjolnir, Skill::Thunderclap],
            God::Athena => [Skill::SpearCounter, Skill::OwlSight],
            God::Hades => [Skill::SoulRend, Skill::CerberusBite],
        }
    }
}

/// The merged skill of a pair of gods, whichever is the main one.
pub fn pair_skill(a: God, b: God) -> Option<Skill> {
    use God::*;
    match (a.min(b), a.max(b)) {
        (Thor, Athena) => Some(Skill::AegisStorm),
        (Thor, Hades) => Some(Skill::GraveThunder),
        (Athena, Hades) => Some(Skill::Judgement),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skill {
    Mjolnir,
    Thunderclap,
    SpearCounter,
    OwlSight,
    SoulRend,
    CerberusBite,
    AegisStorm,
    GraveThunder,
    Judgement,
}

pub const SKILLS: [Skill; 9] = [
    Skill::Mjolnir,
    Skill::Thunderclap,
    Skill::SpearCounter,
    Skill::OwlSight,
    Skill::SoulRend,
    Skill::CerberusBite,
    Skill::AegisStorm,
    Skill::GraveThunder,
    Skill::Judgement,
];

impl Skill {
    pub fn id(self) -> &'static str {
        match self {
            Skill::Mjolnir => "mjolnir",
            Skill::Thunderclap => "thunderclap",
            Skill::SpearCounter => "spear",
            Skill::OwlSight => "owl",
            Skill::SoulRend => "rend",
            Skill::CerberusBite => "bite",
            Skill::AegisStorm => "aegis",
            Skill::GraveThunder => "grave",
            Skill::Judgement => "judgement",
        }
    }
    pub fn from_id(s: &str) -> Option<Skill> {
        SKILLS.into_iter().find(|k| k.id() == s)
    }
    pub fn name(self) -> &'static str {
        match self {
            Skill::Mjolnir => "Mjolnir",
            Skill::Thunderclap => "Thunderclap",
            Skill::SpearCounter => "Spear Counter",
            Skill::OwlSight => "Owl's Sight",
            Skill::SoulRend => "Soul Rend",
            Skill::CerberusBite => "Cerberus Bite",
            Skill::AegisStorm => "Aegis Storm",
            Skill::GraveThunder => "Grave Thunder",
            Skill::Judgement => "Judgement",
        }
    }
    pub fn stamina(self) -> i32 {
        match self {
            Skill::Mjolnir => 2,
            Skill::Thunderclap => 3,
            Skill::SpearCounter => 1,
            Skill::OwlSight => 2,
            Skill::SoulRend => 2,
            Skill::CerberusBite => 3,
            Skill::AegisStorm => 2,
            Skill::GraveThunder => 3,
            Skill::Judgement => 2,
        }
    }
    /// Turns before it can be used again (0: any turn).
    pub fn cooldown(self) -> u32 {
        match self {
            Skill::Mjolnir => 2,
            Skill::AegisStorm => 4,
            Skill::GraveThunder | Skill::Judgement => 3,
            Skill::OwlSight => 3,
            _ => 0,
        }
    }
    /// What it does, for the main god `main`.
    pub fn describe(self, main: God) -> &'static str {
        match (self, main) {
            (Skill::Mjolnir, _) => "A big hit, stronger for every Strike before it. The hammer comes back after a turn.",
            (Skill::Thunderclap, _) => "A heavy hit that stuns a foe winding up, cancelling its attack.",
            (Skill::SpearCounter, _) => "Guard, and strike back if you're hit.",
            (Skill::OwlSight, _) => "See the foe's next two moves, and your next hit lands 50% harder.",
            (Skill::SoulRend, _) => "A hit that heals you for 60% of the damage.",
            (Skill::CerberusBite, _) => "Wounds that bleed for three turns.",
            (Skill::AegisStorm, God::Athena) => "Block the next hit completely and throw it back as lightning.",
            (Skill::AegisStorm, _) => "A shield bash, and the next hit can't touch you.",
            (Skill::GraveThunder, God::Hades) => "A hit and a curse: the foe takes 60% more damage for three turns.",
            (Skill::GraveThunder, _) => "Lightning that heals you for a third of the damage.",
            (Skill::Judgement, God::Hades) => "A drain that heals you and shields you for the damage.",
            (Skill::Judgement, _) => "Guard, and heal for three turns.",
        }
    }
}

// ---- materials, supplies, trophies ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Material,
    /// Taken on expeditions.
    Supply,
    /// Used in fights.
    Combat,
    Trophy,
}

#[derive(Debug)]
pub struct Item {
    pub id: &'static str,
    pub name: &'static str,
    pub plural: &'static str,
    pub kind: Kind,
    /// Tribute it's worth at an altar.
    pub tribute: i64,
    /// What it's for, shown in the pack.
    pub used_for: &'static str,
    pub emoji: &'static str,
}

pub const ITEMS: &[Item] = &[
    Item { id: "iron", name: "iron", plural: "iron", kind: Kind::Material, tribute: 4, used_for: "weapons and mail", emoji: "iron" },
    Item { id: "yew", name: "yew", plural: "yew", kind: Kind::Material, tribute: 3, used_for: "hafts and bows", emoji: "yew" },
    Item { id: "pelt", name: "wolf pelt", plural: "wolf pelts", kind: Kind::Material, tribute: 5, used_for: "armour and bait", emoji: "pelt" },
    Item { id: "herbs", name: "herb", plural: "herbs", kind: Kind::Material, tribute: 2, used_for: "potions, oils and tonics", emoji: "herbs" },
    Item { id: "fish", name: "herring", plural: "herring", kind: Kind::Material, tribute: 2, used_for: "rations and potions", emoji: "fish" },
    Item { id: "amber", name: "sea amber", plural: "sea amber", kind: Kind::Material, tribute: 14, used_for: "charms and fire oil", emoji: "amber" },
    Item { id: "fang", name: "wolf fang", plural: "wolf fangs", kind: Kind::Material, tribute: 14, used_for: "better weapons and charms", emoji: "fang" },
    Item { id: "frost", name: "frost hide", plural: "frost hides", kind: Kind::Material, tribute: 16, used_for: "the best armour, frost oil and tonics", emoji: "frost" },
    Item { id: "rations", name: "trail ration", plural: "trail rations", kind: Kind::Supply, tribute: 0, used_for: "+20% Power on an expedition", emoji: "rations" },
    Item { id: "tonic", name: "frost tonic", plural: "frost tonics", kind: Kind::Supply, tribute: 0, used_for: "+40% Power in the Barrow Fields", emoji: "tonic" },
    Item { id: "bait", name: "hunter's bait", plural: "hunter's bait", kind: Kind::Supply, tribute: 0, used_for: "more rare drops on an expedition", emoji: "bait" },
    Item { id: "potion", name: "healing potion", plural: "healing potions", kind: Kind::Combat, tribute: 0, used_for: "heals 40% in a fight", emoji: "potion" },
    Item { id: "fireoil", name: "fire oil", plural: "fire oil", kind: Kind::Combat, tribute: 0, used_for: "fire on your next 3 hits", emoji: "fireoil" },
    Item { id: "frostoil", name: "frost oil", plural: "frost oil", kind: Kind::Combat, tribute: 0, used_for: "frost on your next 3 hits", emoji: "frostoil" },
    Item { id: "shard", name: "crown shard", plural: "crown shards", kind: Kind::Trophy, tribute: 60, used_for: "the Barrow King's gear", emoji: "shard" },
];

pub fn item(id: &str) -> Option<&'static Item> {
    ITEMS.iter().find(|i| i.id == id)
}

/// `n` and the right form of the item's name: "1 wolf pelt", "3 wolf pelts".
pub fn count(id: &str, n: i64) -> String {
    match item(id) {
        Some(i) => format!("{n} {}", if n == 1 { i.name } else { i.plural }),
        None => format!("{n} {id}"),
    }
}

/// Something made at the forge from materials and gold.
pub struct Recipe {
    pub id: &'static str,
    pub makes: &'static str,
    pub qty: i64,
    pub cost: &'static [(&'static str, i64)],
    pub gold: i64,
}

pub const SUPPLY_RECIPES: &[Recipe] = &[
    Recipe { id: "rations", makes: "rations", qty: 2, cost: &[("fish", 3), ("herbs", 1)], gold: 10 },
    Recipe { id: "tonic", makes: "tonic", qty: 1, cost: &[("herbs", 2), ("frost", 1)], gold: 15 },
    Recipe { id: "bait", makes: "bait", qty: 1, cost: &[("pelt", 1), ("fish", 2)], gold: 10 },
    Recipe { id: "potion", makes: "potion", qty: 2, cost: &[("herbs", 3), ("fish", 1)], gold: 20 },
    Recipe { id: "fireoil", makes: "fireoil", qty: 1, cost: &[("herbs", 2), ("amber", 1)], gold: 20 },
    Recipe { id: "frostoil", makes: "frostoil", qty: 1, cost: &[("herbs", 2), ("frost", 1)], gold: 20 },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Weapon,
    Armour,
    Charm,
}

pub const SLOTS: [Slot; 3] = [Slot::Weapon, Slot::Armour, Slot::Charm];

impl Slot {
    pub fn id(self) -> &'static str {
        match self {
            Slot::Weapon => "weapon",
            Slot::Armour => "armour",
            Slot::Charm => "charm",
        }
    }
    pub fn from_id(s: &str) -> Option<Slot> {
        SLOTS.into_iter().find(|x| x.id() == s)
    }
    pub fn name(self) -> &'static str {
        match self {
            Slot::Weapon => "Weapon",
            Slot::Armour => "Armour",
            Slot::Charm => "Charm",
        }
    }
}

#[derive(Debug)]
pub struct Gear {
    pub id: &'static str,
    pub name: &'static str,
    pub slot: Slot,
    pub level: u32,
    pub power: i64,
    /// Scouting chance added on expeditions, in percent.
    pub scouting: i64,
    pub cost: &'static [(&'static str, i64)],
    pub gold: i64,
    /// Needs the Barrow King beaten.
    pub after_boss: bool,
}

pub const GEAR: &[Gear] = &[
    Gear { id: "handaxe", name: "Iron Hand-axe", slot: Slot::Weapon, level: 1, power: 20, scouting: 0, cost: &[("iron", 5), ("yew", 2)], gold: 25, after_boss: false },
    Gear { id: "warhammer", name: "Yew-hafted Warhammer", slot: Slot::Weapon, level: 6, power: 45, scouting: 0, cost: &[("iron", 12), ("yew", 6), ("fang", 1)], gold: 120, after_boss: false },
    Gear { id: "barrowblade", name: "Barrow-steel Blade", slot: Slot::Weapon, level: 12, power: 80, scouting: 0, cost: &[("iron", 22), ("frost", 3), ("fang", 3)], gold: 320, after_boss: false },
    Gear { id: "crownbreaker", name: "Crown-breaker", slot: Slot::Weapon, level: 20, power: 120, scouting: 0, cost: &[("iron", 30), ("shard", 2), ("fang", 4)], gold: 600, after_boss: true },
    Gear { id: "vest", name: "Wolf-hide Vest", slot: Slot::Armour, level: 1, power: 15, scouting: 0, cost: &[("pelt", 3)], gold: 20, after_boss: false },
    Gear { id: "mail", name: "Iron Mail", slot: Slot::Armour, level: 8, power: 40, scouting: 0, cost: &[("iron", 14), ("pelt", 4)], gold: 150, after_boss: false },
    Gear { id: "mantle", name: "Frost-hide Mantle", slot: Slot::Armour, level: 14, power: 70, scouting: 0, cost: &[("frost", 5), ("pelt", 8), ("iron", 6)], gold: 360, after_boss: false },
    Gear { id: "kingsmantle", name: "Barrow King's Mantle", slot: Slot::Armour, level: 20, power: 100, scouting: 0, cost: &[("shard", 2), ("frost", 6), ("pelt", 10)], gold: 600, after_boss: true },
    Gear { id: "ambercharm", name: "Amber Charm", slot: Slot::Charm, level: 3, power: 10, scouting: 5, cost: &[("amber", 2)], gold: 20, after_boss: false },
    Gear { id: "fangnecklace", name: "Fang Necklace", slot: Slot::Charm, level: 10, power: 30, scouting: 5, cost: &[("fang", 4), ("amber", 2)], gold: 160, after_boss: false },
    Gear { id: "ravencharm", name: "Raven Charm", slot: Slot::Charm, level: 16, power: 50, scouting: 10, cost: &[("amber", 3), ("fang", 3), ("frost", 2)], gold: 300, after_boss: false },
    Gear { id: "crownshard", name: "Crown Shard Circlet", slot: Slot::Charm, level: 20, power: 70, scouting: 10, cost: &[("shard", 3), ("amber", 4)], gold: 600, after_boss: true },
];

pub fn gear(id: &str) -> Option<&'static Gear> {
    GEAR.iter().find(|g| g.id == id)
}

/// What every new player starts with, equipped: a gift from their patron.
pub const STARTER_GEAR: [&str; 2] = ["handaxe", "vest"];

/// Upgrades in Midgard go to +3; each adds 10% of the piece's base Power.
pub const MAX_UPGRADE: i64 = 3;

/// Gold and the piece's main material to go from `upgrade` to `upgrade + 1`.
pub fn upgrade_cost(g: &Gear, upgrade: i64) -> (i64, &'static str, i64) {
    let mat = g.cost.first().map(|c| c.0).unwrap_or("iron");
    (g.power * 3 * (upgrade + 1), mat, 2 * (upgrade + 1))
}

pub fn gear_power(g: &Gear, upgrade: i64) -> i64 {
    g.power + g.power * upgrade / 10
}

// ---- regions and danger ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Danger {
    Normal,
    Hard,
    Heroic,
}

pub const DANGERS: [Danger; 3] = [Danger::Normal, Danger::Hard, Danger::Heroic];

impl Danger {
    pub fn id(self) -> &'static str {
        match self {
            Danger::Normal => "normal",
            Danger::Hard => "hard",
            Danger::Heroic => "heroic",
        }
    }
    pub fn from_id(s: &str) -> Option<Danger> {
        DANGERS.into_iter().find(|d| d.id() == s)
    }
    pub fn name(self) -> &'static str {
        match self {
            Danger::Normal => "Normal",
            Danger::Hard => "Hard",
            Danger::Heroic => "Heroic",
        }
    }
    /// Loot and XP multiplier.
    pub fn loot(self) -> f64 {
        match self {
            Danger::Normal => 1.0,
            Danger::Hard => 1.5,
            Danger::Heroic => 2.0,
        }
    }
    /// How much harder: the Power it needs, times the region's base.
    pub fn hard(self) -> f64 {
        match self {
            Danger::Normal => 1.0,
            Danger::Hard => 2.0,
            Danger::Heroic => 3.0,
        }
    }
}

#[derive(Debug)]
pub struct Region {
    pub id: &'static str,
    pub name: &'static str,
    pub blurb: &'static str,
    /// Power a Normal run needs.
    pub power: i64,
    /// Drops per hour on Normal: (item, amount).
    pub drops: &'static [(&'static str, f64)],
    /// The rare drop: chance per hour, and hours without one before it's guaranteed.
    pub rare: (&'static str, f64, u32),
    /// Chance per hour of learning something about the realm boss.
    pub intel: f64,
    pub gold: f64,
    pub xp: f64,
    pub monsters: &'static [&'static str],
}

pub const REGIONS: &[Region] = &[
    Region {
        id: "shore",
        name: "Grey Shore",
        blurb: "Cold beaches where nixies sing. Herring, herbs and amber.",
        power: 40,
        drops: &[("fish", 1.6), ("herbs", 1.3), ("iron", 0.5)],
        rare: ("amber", 0.12, 8),
        intel: 0.03,
        gold: 14.0,
        xp: 40.0,
        monsters: &["nixie", "harpy", "drowned", "serpent"],
    },
    Region {
        id: "wolfwood",
        name: "Wolfwood",
        blurb: "Old pines and older wolves. Yew, pelts and fangs.",
        power: 90,
        drops: &[("yew", 1.5), ("pelt", 1.0), ("herbs", 0.8)],
        rare: ("fang", 0.14, 7),
        intel: 0.03,
        gold: 20.0,
        xp: 60.0,
        monsters: &["wolf", "satyr", "centaur", "troll"],
    },
    Region {
        id: "barrow",
        name: "Barrow Fields",
        blurb: "Grave mounds where the draugr walk. Iron, frost hides, and the King's trail.",
        power: 150,
        drops: &[("iron", 2.0), ("yew", 0.4), ("pelt", 0.4)],
        rare: ("frost", 0.2, 6),
        intel: 0.09,
        gold: 28.0,
        xp: 80.0,
        monsters: &["draugr", "wight", "hound", "shade"],
    },
];

pub fn region(id: &str) -> Option<&'static Region> {
    REGIONS.iter().find(|r| r.id == id)
}

pub const HOURS: [u32; 3] = [1, 4, 8];

/// Supplies that can go on an expedition.
pub const EXPEDITION_SUPPLIES: [&str; 3] = ["rations", "tonic", "bait"];

/// Tribute every expedition hour adds to the server's offering, to the patron's altar.
pub const TRIBUTE_PER_HOUR: i64 = 12;

// ---- monsters and the boss ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    /// A normal hit.
    Hit,
    /// A telegraphed big hit.
    Heavy,
    /// Raises its guard: halves your next hit.
    Brace,
    /// Drains stamina unless you guard.
    Roar,
}

pub struct Monster {
    pub id: &'static str,
    pub name: &'static str,
    /// HP and attack as multiples of the region's Power.
    pub hp: f64,
    pub atk: f64,
    /// Its moves, in a loop.
    pub pattern: &'static [Move],
    /// What it says when winding up its Heavy.
    pub windup: &'static str,
    pub weak: Option<&'static str>,
    /// A rare named foe: tougher, richer, announced when beaten.
    pub elite: bool,
}

pub const MONSTERS: &[Monster] = &[
    Monster { id: "nixie", name: "Nixie", hp: 1.6, atk: 0.20, pattern: &[Move::Hit, Move::Brace, Move::Hit, Move::Heavy], windup: "starts a drowning song", weak: Some("fire"), elite: false },
    Monster { id: "harpy", name: "Harpy", hp: 1.3, atk: 0.24, pattern: &[Move::Hit, Move::Hit, Move::Heavy], windup: "climbs high to dive", weak: None, elite: false },
    Monster { id: "wolf", name: "Grey Wolf", hp: 1.4, atk: 0.22, pattern: &[Move::Hit, Move::Roar, Move::Hit, Move::Heavy], windup: "crouches to pounce", weak: Some("fire"), elite: false },
    Monster { id: "satyr", name: "Satyr Raider", hp: 1.7, atk: 0.20, pattern: &[Move::Hit, Move::Brace, Move::Heavy], windup: "lowers its horns", weak: None, elite: false },
    Monster { id: "draugr", name: "Draugr Warrior", hp: 1.6, atk: 0.21, pattern: &[Move::Hit, Move::Hit, Move::Heavy, Move::Brace], windup: "raises its axe", weak: Some("fire"), elite: false },
    Monster { id: "wight", name: "Barrow Wight", hp: 1.4, atk: 0.23, pattern: &[Move::Hit, Move::Roar, Move::Heavy], windup: "draws in a grave-cold breath", weak: Some("frost"), elite: false },
    Monster { id: "drowned", name: "Drowned Sailor", hp: 1.5, atk: 0.22, pattern: &[Move::Hit, Move::Hit, Move::Brace, Move::Heavy], windup: "lifts a barnacled anchor", weak: Some("fire"), elite: false },
    Monster { id: "serpent", name: "Sea Serpent", hp: 1.4, atk: 0.24, pattern: &[Move::Hit, Move::Roar, Move::Heavy], windup: "rears back to strike", weak: Some("frost"), elite: false },
    Monster { id: "centaur", name: "Centaur Scout", hp: 1.3, atk: 0.25, pattern: &[Move::Hit, Move::Hit, Move::Heavy], windup: "draws a long arrow", weak: None, elite: false },
    Monster { id: "troll", name: "Wood Troll", hp: 2.0, atk: 0.20, pattern: &[Move::Brace, Move::Hit, Move::Hit, Move::Heavy], windup: "lifts a whole tree trunk", weak: Some("fire"), elite: false },
    Monster { id: "hound", name: "Grave Hound", hp: 1.3, atk: 0.24, pattern: &[Move::Hit, Move::Hit, Move::Roar, Move::Heavy], windup: "bares its rotten teeth", weak: Some("fire"), elite: false },
    Monster { id: "shade", name: "Barrow Shade", hp: 1.5, atk: 0.22, pattern: &[Move::Hit, Move::Brace, Move::Heavy], windup: "goes still and cold", weak: Some("frost"), elite: false },
    // Elites: one per region, rare.
    Monster { id: "siren", name: "The Siren of the Skerries", hp: 1.6 * ELITE_HP, atk: 0.20 * ELITE_ATK, pattern: &[Move::Hit, Move::Roar, Move::Hit, Move::Heavy], windup: "takes a breath that pulls at your ears", weak: Some("fire"), elite: true },
    Monster { id: "grimtooth", name: "Old Grimtooth", hp: 1.4 * ELITE_HP, atk: 0.22 * ELITE_ATK, pattern: &[Move::Hit, Move::Roar, Move::Hit, Move::Heavy], windup: "crouches, every scar on him tight", weak: Some("fire"), elite: true },
    Monster { id: "champion", name: "Hrolf's Champion", hp: 1.6 * ELITE_HP, atk: 0.21 * ELITE_ATK, pattern: &[Move::Hit, Move::Brace, Move::Hit, Move::Heavy], windup: "raises a sword as long as you are", weak: Some("frost"), elite: true },
];

/// Elites' HP and attack against the regular monster of their region.
pub const ELITE_HP: f64 = 1.8;
pub const ELITE_ATK: f64 = 1.2;
/// Chance a hunt meets the region's elite.
pub const ELITE_CHANCE: f64 = 0.06;

/// The region's elite.
pub fn elite(region: &str) -> Option<&'static Monster> {
    let id = match region {
        "shore" => "siren",
        "wolfwood" => "grimtooth",
        _ => "champion",
    };
    monster(id)
}

/// A title for beating an elite the first time.
pub fn elite_title(id: &str) -> Option<&'static str> {
    match id {
        "siren" => Some("Siren-deaf"),
        "grimtooth" => Some("Grimtooth's Bane"),
        "champion" => Some("Champion-breaker"),
        _ => None,
    }
}

pub fn monster(id: &str) -> Option<&'static Monster> {
    MONSTERS.iter().find(|m| m.id == id)
}

pub const BOSS_ID: &str = "barrowking";
pub const BOSS_NAME: &str = "The Barrow King";
pub const BOSS_POWER: i64 = 420;
pub const BOSS_HP: i64 = 1600;
pub const BOSS_ATK: i64 = 47;
/// Phase 1 and phase 2 (below half HP) move loops.
pub const BOSS_P1: &[Move] = &[Move::Hit, Move::Hit, Move::Heavy, Move::Hit, Move::Roar];
pub const BOSS_P2: &[Move] = &[Move::Hit, Move::Heavy, Move::Hit, Move::Brace, Move::Hit, Move::Heavy, Move::Roar];
/// Phase 2 hits this much harder.
pub const BOSS_P2_ATK: f64 = 1.3;

/// What scouting can learn about the boss, in the order intel finds it.
pub const FACTS: [&str; 4] = ["weakness", "slam", "phase", "roar"];

pub fn fact_text(fact: &str, week: i64) -> String {
    match fact {
        "weakness" => format!("{} hurts it most this week", if boss_weakness(week) == "fire" { "Fire" } else { "Frost" }),
        "slam" => "It lifts the barrow stone a turn before it slams".into(),
        "phase" => "Below half health it hits harder and slams more often".into(),
        "roar" => "Its roar drains 2 stamina unless you guard".into(),
        _ => String::new(),
    }
}

/// The boss changes one trait every week: its weakness alternates.
pub fn boss_weakness(week: i64) -> &'static str {
    if week.rem_euclid(2) == 0 { "fire" } else { "frost" }
}

// ---- progression ----

/// Midgard's caps: level 20, bond 2.
pub const LEVEL_CAP: u32 = 20;
pub const BOND_CAP: u32 = 2;

/// XP from `level` to `level + 1`.
pub fn xp_to_next(level: u32) -> i64 {
    (55.0 * (level as f64).powf(1.4)).round() as i64
}

/// Personal tribute to a god needed for each bond rank (index = rank).
pub const BOND_AT: [i64; 6] = [0, 300, 1500, 5000, 12000, 25000];

/// Power from level.
pub fn level_power(level: u32) -> i64 {
    10 * level as i64
}

/// Combat stats from Power and the patron: (max HP, attack).
pub fn stats(power: i64, main: God) -> (i64, i64) {
    let (hp, atk, _) = main.body();
    (((60.0 + power as f64 * 1.2) * hp).round() as i64, ((4.0 + power as f64 * 0.11) * atk).round() as i64)
}

/// HP regained per minute out of fights, as a share of max HP.
pub const REGEN_PER_MIN: f64 = 1.0 / 15.0;

/// Hunt monsters' HP and attack, times the region's Power and the monster's own multiples.
pub const HUNT_HP: f64 = 1.25;
pub const HUNT_ATK: f64 = 0.75;
/// Share of max HP a hunt win gives back: you catch your breath.
pub const WIN_HEAL: f64 = 0.15;
/// Lifesteal works at this share against the realm boss.
pub const BOSS_LIFESTEAL: f64 = 0.4;
/// Gold per level to rest at the inn and be at full HP at once.
pub const INN_GOLD_PER_LEVEL: i64 = 6;

/// Vigor gained per hour away from hunting, and spent per hunt won (both in percent).
pub const VIGOR_PER_HOUR: f64 = 1.5;
pub const VIGOR_PER_WIN: i64 = 8;

/// Hunt rewards per win, before danger and Vigor.
pub fn hunt_reward(r: &Region) -> (i64, i64) {
    ((r.xp * 1.1).round() as i64, (r.gold * 0.6).round() as i64)
}

/// The server's weekly offering goal per player who played that week.
pub const GOAL_PER_PLAYER: i64 = 5000;
pub const GOAL_MIN_PLAYERS: i64 = 3;

/// Gold a patron change costs, after the first week.
pub const PATRON_CHANGE_GOLD: i64 = 500;

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing exists on its own: every material is used by a recipe or a piece of gear, and
    /// every material drops somewhere.
    #[test]
    fn every_item_has_a_use_and_a_source() {
        for i in ITEMS {
            let used = GEAR.iter().any(|g| g.cost.iter().any(|(id, _)| *id == i.id))
                || SUPPLY_RECIPES.iter().any(|r| r.cost.iter().any(|(id, _)| *id == i.id))
                || matches!(i.kind, Kind::Supply | Kind::Combat);
            assert!(used, "{} has no use", i.id);
            let source = REGIONS.iter().any(|r| r.drops.iter().any(|(id, _)| *id == i.id) || r.rare.0 == i.id)
                || SUPPLY_RECIPES.iter().any(|r| r.makes == i.id)
                || i.kind == Kind::Trophy;
            assert!(source, "{} can't be found or made", i.id);
            assert!(i.kind != Kind::Material || i.tribute > 0, "{} can't be offered", i.id);
        }
        for g in GEAR {
            for (id, _) in g.cost {
                assert!(item(id).is_some(), "{} needs unknown item {id}", g.id);
            }
        }
        for r in SUPPLY_RECIPES {
            assert!(item(r.makes).is_some());
        }
        for g in STARTER_GEAR {
            assert!(gear(g).is_some());
        }
    }

    /// Every patron has two skills, every pair a merged skill, and every skill a description
    /// for every main god.
    #[test]
    fn skills_are_complete() {
        for a in GODS {
            for b in GODS {
                assert_eq!(pair_skill(a, b).is_some(), a != b);
            }
            for s in SKILLS {
                assert!(!s.describe(a).is_empty());
            }
        }
    }
}
