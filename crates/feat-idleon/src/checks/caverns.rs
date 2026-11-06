//! The Caverns (World 5 underground): Holes and villagers.
//!
//! Everything is in the save's `Holes` list. `Holes[1][v]` is villager v's level (Polonai,
//! Kaipu, Cosmos, Minau, Bolaia); Polonai's level is the number of caverns found, capped at 18
//! by the game (`CavernsOwned`). Before Polonai's first level nothing here counts, since the
//! game fills `Holes` with defaults for every account.

use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    let h = Holes::read(save);
    vec![
        villagers(save, &h),
        schematics(&h),
        majiks(&h),
        measurements(&h),
        studies(&h),
        shallow_caverns(&h),
        glowshroom_tunnels(&h),
        underground_overgrowth(&h),
    ]
}

/// Caverns there are: `CavernsOwned` is `min(18, Polonai's level)` in N.js.
const CAVERNS: f64 = 18.0;
/// Schematics Kaipu can unlock: `SchematicsAvailable` is
/// `min(105, 1 + 3 * level + floor(level / 5), HolesBuildings.length)` in N.js.
const SCHEMATICS: usize = 105;
/// Majiks shown per type (`CosmoUpgCoded`: 5 Hole, 7 Village, 7 IdleOn); rows past these are
/// moved off screen by the game.
const MAJIKS_SHOWN: [usize; 3] = [5, 7, 7];

struct Holes<'a> {
    save: &'a Save,
    /// Caverns found, 0 to 18.
    caverns: f64,
}

impl Holes<'_> {
    fn read(save: &Save) -> Holes<'_> {
        let caverns = save.numbers("Holes", &[1]).first().copied().unwrap_or(0.0).clamp(0.0, CAVERNS);
        Holes { save, caverns }
    }

    /// `Holes[path]` as numbers, or nothing before the Caverns are found.
    fn nums(&self, path: &[usize]) -> Vec<f64> {
        if self.caverns < 1.0 { Vec::new() } else { self.save.numbers("Holes", path) }
    }

    fn get(&self, path: &[usize], i: usize) -> f64 {
        at(&self.nums(path), i)
    }

    /// Cavern `n` (1 to 18) is found.
    fn has(&self, n: usize) -> bool {
        self.caverns >= n as f64
    }
}

fn info(row: usize) -> Vec<&'static Value> {
    rows("HolesInfo", &[row])
}

/// Cavern `i`'s name (0 is The Well), from `HolesInfo[68]`; the monuments read as the game's
/// "Monument of ..." places.
fn cave_name(i: usize) -> String {
    let raw = info(68).get(i).and_then(|v| v.as_str()).unwrap_or("");
    let name = label(raw);
    match raw {
        "BRAVERY" | "JUSTICE" | "WISDOM" => format!("{name} Monument"),
        _ => name,
    }
}

/// The picture of cavern `i` (0 is The Well). The Jars has one per look; the first is the plain one.
fn cave_pic(i: usize) -> Option<String> {
    pic(if i == 10 { "HolePOI10_0".into() } else { format!("HolePOI{i}") })
}

/// Villagers: what each one's levels unlock, against all there is. Their levels have no cap.
fn villagers(save: &Save, h: &Holes) -> Mechanic {
    let lv = h.nums(&[1]);
    let kaipu = at(&lv, 1);
    let schematics = if kaipu < 1.0 { 0.0 } else { (1.0 + 3.0 * kaipu + (kaipu / 5.0).floor()).min(SCHEMATICS as f64) };
    // Majik points: Cosmos' level plus the gem shop's Conjuror Pts (`CosmoPtsLeft`).
    let cosmos = at(&lv, 2);
    let points = if cosmos < 1.0 { 0.0 } else { cosmos + at(&save.numbers("GemItemsPurchased", &[]), 4) };
    let measurements = info(54).len() as f64;
    let studies = info(69).len();
    let studied = h.nums(&[26]).iter().take(studies).filter(|&&v| v >= 1.0).count() as f64;
    let items = vec![
        Item::new("Polonai: caverns found", pic("HoleUItab0".into()), h.caverns, CAVERNS),
        Item::new("Kaipu: schematics unlocked", pic("HoleUItab1".into()), schematics, SCHEMATICS as f64),
        Item::new("Cosmos: majik points", pic("HoleUItab2".into()), points.min(majik_total()), majik_total()),
        Item::new("Minau: measurements unlocked", pic("HoleUItab3".into()), at(&lv, 3).clamp(0.0, measurements), measurements),
        Item::new("Bolaia: caverns studied", pic("HoleUItab4".into()), studied, studies as f64),
    ];
    let mut m = mech("cavern-villagers", 5, "Villagers", pic("HoleUItab0".into()), Count::Levels, "unlocked", items);
    m.note = Some("Villager levels have no cap, so this is what they unlock: Kaipu has every schematic at level 33, Minau every measurement at 17, Polonai every cavern at 18. Majik points count the gem shop's Conjuror Pts.".into());
    m
}

/// Schematics (Kaipu's buildings): `Holes[13][i]` is 1 when built. Kaipu unlocks them in the
/// order of `HolesInfo[40]`; the first 105 there are the ones the game lets you unlock.
fn schematics(h: &Holes) -> Mechanic {
    let built = h.nums(&[13]);
    let table = rows("HolesBuildings", &[]);
    let items = info(40)
        .iter()
        .take(SCHEMATICS)
        .filter_map(|v| {
            let i = num(v) as usize;
            let r = table.get(i)?;
            Some(Item::new(label(text(r, 0)), pic(format!("HoleUIbuildUpg{i}")), at(&built, i).min(1.0), 1.0))
        })
        .collect();
    mech("cavern-schematics", 5, "Schematics", pic("HoleUIbuildUpg0".into()), Count::Done, "built", items)
}

/// The majiks the game offers, by type (Hole, Village, IdleOn), with their max level. The max
/// is `HolesInfo[56 + type][i] + 1` (`EnhanceMaxLV`, past level 1 once Cosmo, Enhance! is
/// bought). The Confused Bonus the game says does nothing yet is left out.
fn majik_rows() -> Vec<(usize, usize, String, f64)> {
    let mut out = Vec::new();
    for (t, &shown) in MAJIKS_SHOWN.iter().enumerate() {
        let maxes = info(56 + t);
        for (i, r) in rows("CosmoUpgrades", &[t]).iter().enumerate().take(shown) {
            if text(r, 3).contains("does_nothing") {
                continue;
            }
            let max = maxes.get(i).map_or(0.0, |v| num(v)) + 1.0;
            out.push((t, i, label(text(r, 2)), max));
        }
    }
    out
}

fn majik_total() -> f64 {
    majik_rows().iter().map(|r| r.3).sum()
}

/// Cosmos' majiks: `Holes[4 + type][i]` is the level.
fn majiks(h: &Holes) -> Mechanic {
    let levels: Vec<Vec<f64>> = (0..3).map(|t| h.nums(&[4 + t])).collect();
    let items = majik_rows().into_iter().map(|(t, i, name, max)| Item::new(name, None, at(&levels[t], i), max)).collect();
    let mut m = mech("cavern-majiks", 5, "Majiks", pic("HoleUItab2".into()), Count::Levels, "levels", items);
    m.note = Some("Caps past level 1 need the Cosmo, Enhance majik.".into());
    m
}

/// Minau's measurements: `Holes[22][i]`. Each scales with something else on the account
/// (`HolesInfo[52]` into the names in `HolesInfo[53]`). They have no max level.
fn measurements(h: &Holes) -> Mechanic {
    let levels = h.nums(&[22]);
    let scales = info(53);
    let by = info(52);
    let items = info(54)
        .iter()
        .enumerate()
        .map(|(i, v)| {
            // `|` is a line break here and `访` the game's ampersand.
            let what = label(&v.as_str().unwrap_or("").replace('|', "_").replace('访', "&"));
            let scale = by.get(i).map(|s| num(s) as usize).and_then(|s| scales.get(s)).and_then(|s| s.as_str()).map(label).unwrap_or_default();
            let name = if scale.is_empty() { what } else { format!("{what}, by {scale}") };
            Item::new(name, None, at(&levels, i), 0.0)
        })
        .collect();
    let mut m = mech("cavern-measurements", 5, "Measurements", pic("HoleUItab3".into()), Count::Open, "levels", items);
    m.note = Some("Measurements level up without end.".into());
    m
}

/// Bolaia's studies, one per cavern: `Holes[26][i]`. No max level, except Bravery Monument's,
/// whose bonus stops at 32% (`StudyBolaiaBonuses`: `min(32, 12 + level * HolesInfo[70][3])`).
fn studies(h: &Holes) -> Mechanic {
    let levels = h.nums(&[26]);
    let items = info(69).iter().enumerate().map(|(i, _)| Item::new(cave_name(i), cave_pic(i), at(&levels, i), 0.0)).collect();
    let per = info(70).get(3).map_or(0.0, |v| num(v));
    let bravery = if per > 0.0 { ((32.0 - 12.0) / per).ceil() } else { 0.0 };
    let mut m = mech("cavern-studies", 5, "Studies", pic("HoleUItab4".into()), Count::Open, "levels", items);
    m.note = Some(format!("Studies level up without end, but the Bravery Monument study stops adding at level {bravery}."));
    m
}

/// A monument's hour rewards: `Holes[14][1 + 2 * monument]` bricks broken, one per hour mark in
/// `HolesInfo[30]` (8 of them).
fn monument(h: &Holes, cave: usize, monument: usize) -> Item {
    let marks = info(30).len() as f64;
    let got = if h.has(cave + 1) { h.get(&[14], 1 + 2 * monument) } else { 0.0 };
    Item::new(format!("{}: hour rewards", cave_name(cave)), cave_pic(cave), got, marks)
}

const BIOME_NOTE: &str = "Only what has a cap. Buildings are under Schematics; bonuses that level without end are left out.";

/// The Well, Motherlode, The Den, Bravery Monument, The Bell (caverns 1 to 5).
fn shallow_caverns(h: &Holes) -> Mechanic {
    // A sediment is found once its rock layer is cleared: `Holes[9][i]` stops being negative.
    let sediments = info(21).len();
    let found = if h.has(1) { h.nums(&[9]).iter().take(sediments).filter(|&&v| v >= 0.0).count() as f64 } else { 0.0 };
    // `BellMethodsOwned` is `min(6, Holes[18][5] + 1)`.
    let methods = if h.has(5) { (h.get(&[18], 5) + 1.0).min(6.0) } else { 0.0 };
    let items = vec![
        Item::new(format!("{}: sediments found", cave_name(0)), cave_pic(0), found, sediments as f64),
        monument(h, 3, 0),
        Item::new(format!("{}: improvement methods", cave_name(4)), cave_pic(4), methods, 6.0),
    ];
    let mut m = mech("cavern-shallow", 5, "Shallow Caverns", cave_pic(0), Count::Levels, "levels", items);
    m.note = Some(BIOME_NOTE.into());
    m
}

/// The Harp, The Lamp, The Hive, Grotto, Justice Monument (caverns 6 to 10).
fn glowshroom_tunnels(h: &Holes) -> Mechanic {
    // New notes are bought while `Holes[11][20] < 9` (`CanWeAfford_Harp`), on top of the first.
    let notes = if h.has(6) { 1.0 + h.get(&[11], 20) } else { 0.0 };
    // More Wishes costs 999999 from level 11 (`LampWishCost`).
    let wishes = if h.has(7) { h.get(&[21], 0) } else { 0.0 };
    let more_wishes = rows("LampWishes", &[]).first().map(|r| label(text(r, 0))).unwrap_or_default();
    let items = vec![
        Item::new(format!("{}: notes unlocked", cave_name(5)), pic("HoleHarpNote0".into()), notes, 10.0),
        Item::new(format!("{}: {more_wishes}", cave_name(6)), pic("HoleLampUpg0".into()), wishes, 11.0),
        monument(h, 9, 1),
    ];
    let mut m = mech("cavern-glowshroom", 5, "Glowshroom Tunnels", cave_pic(5), Count::Levels, "levels", items);
    m.note = Some(BIOME_NOTE.into());
    m
}

/// The Jars, Evertree, Wisdom Monument, Gambit, Temple (caverns 11 to 15).
fn underground_overgrowth(h: &Holes) -> Mechanic {
    // New jar types are made while `Holes[11][37] < 9` (`CanWeAfford_Jar`), on top of the first.
    let jars = if h.has(11) { 1.0 + h.get(&[11], 37) } else { 0.0 };
    // Collectibles: `Holes[24][i]` is the level, 1 once found; `HolesInfo[67]` lists them.
    let collectibles = info(67).len();
    let found = h.nums(&[24]).iter().take(collectibles).filter(|&&v| v >= 1.0).count() as f64;
    let items = vec![
        Item::new(format!("{}: jar types", cave_name(10)), pic("HoleJar0".into()), jars, 10.0),
        Item::new(format!("{}: collectibles found", cave_name(10)), pic("HoleJarC0".into()), found, collectibles as f64),
        monument(h, 12, 2),
    ];
    let mut m = mech("cavern-overgrowth", 5, "Underground Overgrowth", cave_pic(10), Count::Levels, "levels", items);
    m.note = Some(BIOME_NOTE.into());
    m
}
