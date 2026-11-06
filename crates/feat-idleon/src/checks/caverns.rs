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
        majiks(save, &h),
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


/// What `Holes[9][i]` holds, numbered the way each cavern lists them: 0 to 9 sediments at The
/// Well, 10 to 19 Harp notes, 20 to 29 Jar rupies, 30 and up Fountain coins.
fn resource(i: usize) -> String {
    match i {
        0..=9 => format!("Well sediment {}", i + 1),
        10..=19 => format!("Harp note {}", i - 9),
        20..=29 => format!("Jar rupie {}", i - 19),
        _ => format!("Fountain coin {}", i.saturating_sub(29)),
    }
}

/// The EXP villager `v` needs for its next level (N.js `VillagerExpREQ`). Polonai's curve gets
/// steeper past level 12 from game version 247 on (`DoOnceREAL`).
fn villager_exp_req(v: usize, lv: f64, version: f64) -> f64 {
    match v {
        0 if lv == 1.0 => 5.0,
        0 => {
            let late = ((1e5 + version) / 100_247.3).floor().clamp(0.0, 1.0);
            10.0 * ((10.0 + 7.0 * lv.powf(2.1)) * 2.1f64.powf(lv) * (1.0 + 0.75 * (lv - 4.0).max(0.0)) * 3.4f64.powf(late * (lv - 12.0).max(0.0)) - 1.5)
        }
        1 => 30.0 * (10.0 + 6.0 * lv.powf(1.8)) * 1.57f64.powf(lv),
        2 => 50.0 * (10.0 + 5.0 * lv.powf(1.7)) * 1.4f64.powf(lv),
        3 => 120.0 * (30.0 + 10.0 * lv.powi(2)) * 2f64.powf(lv),
        4 => 500.0 * (10.0 + 5.0 * lv.powf(1.3)) * 1.13f64.powf(lv),
        _ => 1e21,
    }
}

/// Schematics Kaipu's level unlocks (N.js `SchematicsAvailable`).
fn schematics_unlocked(kaipu: f64) -> f64 {
    if kaipu < 1.0 { 0.0 } else { (1.0 + 3.0 * kaipu + (kaipu / 5.0).floor()).min(SCHEMATICS as f64) }
}

/// The Kaipu level that unlocks the schematic at unlock position `pos` (0-based).
fn kaipu_level_for(pos: usize) -> f64 {
    (1..=60).map(f64::from).find(|&l| schematics_unlocked(l) > pos as f64).unwrap_or(60.0)
}

/// Villagers: what each one's levels unlock, against all there is. Their levels have no cap.
fn villagers(save: &Save, h: &Holes) -> Mechanic {
    let lv = h.nums(&[1]);
    let exp = h.nums(&[2]);
    let version = save.get("DoOnceREAL").map(num).unwrap_or(0.0);
    let kaipu = at(&lv, 1);
    // Majik points: Cosmos' level plus the gem shop's Conjuror Pts (`CosmoPtsLeft`).
    let cosmos = at(&lv, 2);
    let points = if cosmos < 1.0 { 0.0 } else { cosmos + at(&save.numbers("GemItemsPurchased", &[]), 4) };
    let measurements = info(54).len() as f64;
    let studies = info(69).len();
    let study = h.nums(&[26]);
    let studied = study.iter().take(studies).filter(|&&v| v >= 1.0).count() as f64;
    let total = majik_total();
    let mut items = vec![
        Item::new("Polonai: caverns found", pic("HoleUItab0".into()), h.caverns, CAVERNS),
        Item::new("Kaipu: schematics unlocked", pic("HoleUItab1".into()), schematics_unlocked(kaipu), SCHEMATICS as f64),
        Item::new("Cosmos: majik points", pic("HoleUItab2".into()), points.min(total), total),
        Item::new("Minau: measurements unlocked", pic("HoleUItab3".into()), at(&lv, 3).clamp(0.0, measurements), measurements),
        Item::new("Bolaia: caverns studied", pic("HoleUItab4".into()), studied, studies as f64),
    ];
    for (v, it) in items.iter_mut().enumerate() {
        if it.done() {
            continue;
        }
        let level = at(&lv, v);
        if v == 4 {
            // A study is picked on Bolaia's page; only caverns you've found can be studied.
            if level < 1.0 {
                it.how = Some("You haven't found Bolaia yet.".into());
                it.state = State::Locked;
            } else if let Some(next) = (0..studies).find(|&i| at(&study, i) < 1.0) {
                if h.has(next + 1) {
                    it.how = Some(format!("Pick {} as Bolaia's study to start it.", cave_name(next)));
                    it.state = State::Ready;
                } else {
                    it.how = Some(format!("Find {} first, then pick it as Bolaia's study.", cave_name(next)));
                    it.state = State::Locked;
                }
            }
            continue;
        }
        if level < 1.0 {
            it.how = Some(format!("You haven't found {} yet.", ["Polonai", "Kaipu", "Cosmos", "Minau", "Bolaia"][v]));
            it.state = State::Locked;
            continue;
        }
        let req = villager_exp_req(v, level, version);
        let have = at(&exp, v);
        let what = match v {
            0 => "finds the next cavern".to_string(),
            1 => format!("unlocks {} more schematics", schematics_unlocked(level + 1.0) - schematics_unlocked(level)),
            2 => "gives 1 majik point".to_string(),
            _ => "unlocks the next measurement".to_string(),
        };
        it.how = Some(format!("Next level {what}: {} of {} EXP.", big(have), big(req)));
        it.state = if req.is_finite() && have >= req { State::Ready } else { State::Unknown };
    }
    let mut m = mech("cavern-villagers", 5, "Villagers", pic("HoleUItab0".into()), Count::Levels, "unlocked", items);
    m.note = Some(format!("Villager levels have no cap, so this is what they unlock: Kaipu has every schematic at level {}, Minau every measurement at 17, Polonai every cavern at 18. Cosmos' majik points are his level plus the Conjuror Pts bought in the gem shop, against the {total} levels all majiks add up to.", kaipu_level_for(SCHEMATICS - 1)));
    m
}

/// A schematic's cost by its unlock position `t` (N.js `BuildCost`): a fixed price when column
/// 4 is "1", otherwise a curve by position that depends on which cavern's resource it takes.
/// Each level of the Lamp's A Moderate Discount wish (`Holes[21][5]`) takes 15% off.
fn schematic_cost(row: &Value, t: f64, discount_wishes: f64) -> f64 {
    let d = 0.85f64.powf(discount_wishes).max(0.01);
    if text(row, 4) == "1" {
        return d * cell(row, 3);
    }
    let res = cell(row, 2);
    if res < 10.0 {
        50.0 * d * 1.28f64.powf(t + (t / 2.7).floor())
    } else if res < 20.0 {
        let u = t - 16.0;
        50.0 * d * 1.28f64.powf(u + (u / 2.7).floor()) * 1.23f64.powf((u / 2.0).clamp(0.0, 14.0))
    } else {
        let u = t - 54.0;
        40.0 * d * 1.34f64.powf(u + (u / 2.7).floor()) * 1.26f64.powf((u / 2.0).clamp(0.0, 14.0))
    }
}

/// Schematics (Kaipu's buildings): `Holes[13][i]` is 1 when built. Kaipu unlocks them in the
/// order of `HolesInfo[40]`; the first 105 there are the ones the game lets you unlock. One is
/// built when you hold its cost in its resource, `Holes[9][col 2]` (`CanWeAffordCreate`).
fn schematics(h: &Holes) -> Mechanic {
    let built = h.nums(&[13]);
    let held = h.nums(&[9]);
    let unlocked = schematics_unlocked(h.get(&[1], 1));
    let discount = h.get(&[21], 5);
    let table = rows("HolesBuildings", &[]);
    let items = info(40)
        .iter()
        .take(SCHEMATICS)
        .enumerate()
        .filter_map(|(pos, v)| {
            let i = num(v) as usize;
            let r = table.get(i)?;
            let mut it = Item::new(label(text(r, 0)), pic(format!("HoleUIbuildUpg{i}")), at(&built, i).min(1.0), 1.0);
            if it.done() {
                return Some(it);
            }
            if pos as f64 >= unlocked {
                it.how = Some(format!("Kaipu level {} unlocks it.", kaipu_level_for(pos)));
                it.state = State::Locked;
                return Some(it);
            }
            let res = cell(r, 2) as usize;
            let cost = schematic_cost(r, pos as f64, discount);
            let have = at(&held, res);
            if res < 10 && have < 0.0 {
                it.how = Some(format!("Costs {} {}; find that sediment at The Well first.", big(cost), resource(res)));
                it.state = State::Locked;
            } else {
                it.how = Some(format!("Costs {} {}; you have {}.", big(cost), resource(res), big(have.max(0.0))));
                it.state = if cost.is_finite() && have >= cost { State::Ready } else { State::Unknown };
            }
            Some(it)
        })
        .collect();
    let mut m = mech("cavern-schematics", 5, "Schematics", pic("HoleUIbuildUpg0".into()), Count::Done, "built", items);
    m.note = Some("Costs include the 15% off per level of the Lamp's A Moderate Discount wish. Resources are numbered in the order their cavern lists them: Well sediment 1 is the first sediment at The Well.".into());
    m
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

/// Cosmos' majiks: `Holes[4 + type][i]` is the level. Every level costs one majik point; the
/// points are Cosmos' level plus gem shop Conjuror Pts, less every level bought (`CosmoPtsLeft`).
/// Going past level 1 needs Cosmo, Enhance (`CanWeEnhanceCosmo`: `Holes[5][2] >= 1`).
fn majiks(save: &Save, h: &Holes) -> Mechanic {
    let levels: Vec<Vec<f64>> = (0..3).map(|t| h.nums(&[4 + t])).collect();
    let cosmos = h.get(&[1], 2);
    let earned = if cosmos < 1.0 { 0.0 } else { cosmos + at(&save.numbers("GemItemsPurchased", &[]), 4) };
    // The game counts every row of each type, shown or not.
    let spent: f64 = (0..3).map(|t| levels[t].iter().take(info(2 + t).len()).sum::<f64>()).sum();
    let left = (earned - spent).max(0.0);
    let enhance = at(&levels[1], 2) >= 1.0;
    let enhance_name = rows("CosmoUpgrades", &[1]).get(2).map(|r| label(text(r, 2))).unwrap_or_default();
    let items: Vec<Item> = majik_rows()
        .into_iter()
        .map(|(t, i, name, max)| {
            let mut it = Item::new(name, None, at(&levels[t], i), max);
            if !it.done() {
                if it.level >= 1.0 && !enhance {
                    it.how = Some(format!("Past level 1 needs the {enhance_name} majik."));
                    it.state = State::Locked;
                } else {
                    it.how = Some(format!("Next level costs 1 majik point; you have {left} left."));
                    it.state = if left >= 1.0 { State::Ready } else { State::Unknown };
                }
            }
            it
        })
        .collect();
    let open = items.iter().filter(|it| it.state == State::Ready).map(|it| it.max - it.level).sum::<f64>();
    let have = Have {
        figure: format!("{left} majik points"),
        text: if left < 1.0 {
            "None unspent. Each Cosmos level gives one more.".into()
        } else {
            format!("Enough for {} more majik levels.", left.min(open))
        },
    };
    let mut m = mech("cavern-majiks", 5, "Majiks", pic("HoleUItab2".into()), Count::Levels, "levels", items);
    m.note = Some(format!("Every majik stops at level 1 until you have the {enhance_name} majik; the caps shown are with it."));
    m.have = Some(have);
    m.raises = vec![Raise { name: enhance_name, amount: "lets every majik go past level 1".into(), owned: enhance }];
    m
}

/// A measurement's cost (N.js `MeasurementCost`), before the Fountain's discount:
/// `(250 + 50 * level) * 1.6^(i - 6 * floor(i / 10)) * 1.1^level`.
fn measurement_cost(i: usize, level: f64) -> f64 {
    let i = i as f64;
    (250.0 + 50.0 * level) * 1.6f64.powf(i - 6.0 * (i / 10.0).floor()) * 1.1f64.powf(level)
}

/// Minau's measurements: `Holes[22][i]`. Each scales with something else on the account
/// (`HolesInfo[52]` into the names in `HolesInfo[53]`) and is paid in the resource
/// `HolesInfo[50][i]`. They have no max level; Minau's level opens them one by one
/// (`MeasurementsOwned`: `min(level, 17)`).
fn measurements(h: &Holes) -> Mechanic {
    let levels = h.nums(&[22]);
    let held = h.nums(&[9]);
    let minau = h.get(&[1], 3);
    let scales = info(53);
    let by = info(52);
    let pay = info(50);
    let items = info(54)
        .iter()
        .enumerate()
        .map(|(i, v)| {
            // `|` is a line break here and `访` the game's ampersand.
            let what = label(&v.as_str().unwrap_or("").replace('|', "_").replace('访', "&"));
            let scale = by.get(i).map(|s| num(s) as usize).and_then(|s| scales.get(s)).and_then(|s| s.as_str()).map(label).unwrap_or_default();
            let name = if scale.is_empty() { what } else { format!("{what}, by {scale}") };
            let level = at(&levels, i);
            let mut it = Item::new(name, None, level, 0.0);
            if (i as f64) >= minau {
                it.how = Some(format!("Minau level {} unlocks it.", i + 1));
                it.state = State::Locked;
            } else {
                let res = pay.get(i).map_or(0, |r| num(r) as usize);
                let cost = measurement_cost(i, level);
                let have = at(&held, res);
                it.how = Some(format!("Next level costs {} {}; you have {}.", big(cost), resource(res), big(have.max(0.0))));
                it.state = if cost.is_finite() && have >= cost { State::Ready } else { State::Unknown };
            }
            it
        })
        .collect();
    let mut m = mech("cavern-measurements", 5, "Measurements", pic("HoleUItab3".into()), Count::Open, "levels", items);
    m.note = Some("Measurements level up without end. Costs shown are before the Fountain's measurement discount, so yours can be lower.".into());
    m
}

/// Bolaia's studies, one per cavern: `Holes[26][i]`, with progress `Holes[27][i]` toward the
/// next level at `4000 * 1.25^level * 1.5^floor(i / 5)` (N.js `StudyReqForBolaia`). No max level, except Bravery
/// Monument's, whose bonus stops at 32% (`StudyBolaiaBonuses`: `min(32, 12 + level *
/// HolesInfo[70][3])`).
fn studies(h: &Holes) -> Mechanic {
    let levels = h.nums(&[26]);
    let progress = h.nums(&[27]);
    let bolaia = h.get(&[1], 4);
    let items = info(69)
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let level = at(&levels, i);
            let mut it = Item::new(cave_name(i), cave_pic(i), level, 0.0);
            if bolaia < 1.0 {
                it.how = Some("You haven't found Bolaia yet.".into());
                it.state = State::Locked;
            } else if !h.has(i + 1) {
                it.how = Some(format!("Find {} to study it.", cave_name(i)));
                it.state = State::Locked;
            } else {
                let req = 4000.0 * 1.25f64.powf(level) * 1.5f64.powf((i / 5) as f64);
                it.how = Some(format!("{} of {} study progress to the next level.", big(at(&progress, i)), big(req)));
            }
            it
        })
        .collect();
    let per = info(70).get(3).map_or(0.0, |v| num(v));
    let bravery = if per > 0.0 { ((32.0 - 12.0) / per).ceil() } else { 0.0 };
    let mut m = mech("cavern-studies", 5, "Studies", pic("HoleUItab4".into()), Count::Open, "levels", items);
    m.note = Some(format!("Studies level up without end, but the Bravery Monument study's bonus stops growing at level {bravery} (32%)."));
    m
}

/// A monument's hour rewards: `Holes[14][1 + 2 * monument]` bricks broken, one per hour mark in
/// `HolesInfo[30]` (8 of them); a brick breaks once `Holes[14][2 * monument]` AFK hours there
/// reach its mark.
fn monument(h: &Holes, cave: usize, monument: usize) -> Item {
    let marks = info(30);
    let got = if h.has(cave + 1) { h.get(&[14], 1 + 2 * monument) } else { 0.0 };
    let mut it = Item::new(format!("{}: hour rewards", cave_name(cave)), cave_pic(cave), got, marks.len() as f64);
    if !it.done() {
        if !h.has(cave + 1) {
            it.how = Some(format!("Find {} first.", cave_name(cave)));
            it.state = State::Locked;
        } else if let Some(mark) = marks.get(got as usize).map(|v| num(v)) {
            let hours = h.get(&[14], 2 * monument);
            if hours >= mark {
                it.how = Some(format!("You have the {} AFK hours for the next reward: break its brick at the monument.", big(mark)));
                it.state = State::Ready;
            } else {
                it.how = Some(format!("Next reward at {} AFK hours there; you have {}.", big(mark), big(hours)));
            }
        }
    }
    it
}

const BIOME_NOTE: &str = "Only the parts with a cap. These caverns' buildings count under Schematics; bonuses that level without end are left out.";

/// The Well, Motherlode, The Den, Bravery Monument, The Bell (caverns 1 to 5).
fn shallow_caverns(h: &Holes) -> Mechanic {
    // A sediment is found once its rock layer is cleared: `Holes[9][i]` counts up from minus the
    // layer's size to 0.
    let sediments = info(21).len();
    let held = h.nums(&[9]);
    let found = if h.has(1) { held.iter().take(sediments).filter(|&&v| v >= 0.0).count() as f64 } else { 0.0 };
    let mut well = Item::new(format!("{}: sediments found", cave_name(0)), cave_pic(0), found, sediments as f64);
    if let Some(i) = (0..sediments).find(|&i| at(&held, i) < 0.0).filter(|_| h.has(1)) {
        well.how = Some(format!("Point a bucket at the rock layer: {} left to clear for {}.", big(-at(&held, i)), resource(i)));
    }
    // `BellMethodsOwned` is `min(6, Holes[18][5] + 1)`; a Clean needs `100 * 3^Holes[18][5]`
    // Clean charge (`BellEXPreq`), held in `Holes[18][4]`, and finds a method by chance.
    let cleans = h.get(&[18], 5);
    let methods = if h.has(5) { (cleans + 1.0).min(6.0) } else { 0.0 };
    let mut bell = Item::new(format!("{}: improvement methods", cave_name(4)), cave_pic(4), methods, 6.0);
    if !bell.done() {
        if h.has(5) {
            let req = 100.0 * 3f64.powf(cleans);
            bell.how = Some(format!("Each Clean of the Bell may add one: {} of {} Clean charge.", big(h.get(&[18], 4)), big(req)));
        } else {
            bell.how = Some(format!("Find {} first.", cave_name(4)));
            bell.state = State::Locked;
        }
    }
    let items = vec![well, monument(h, 3, 0), bell];
    let mut m = mech("cavern-shallow", 5, "Shallow Caverns", cave_pic(0), Count::Levels, "levels", items);
    m.note = Some(BIOME_NOTE.into());
    m
}

/// A cave purchase paid in one resource: the next one costs `cost`, `res` is where it's held.
fn priced(it: &mut Item, what: &str, owned: bool, cave: usize, cost: f64, res: usize, held: f64) {
    if it.done() {
        return;
    }
    if !owned {
        it.how = Some(format!("Find {} first.", cave_name(cave)));
        it.state = State::Locked;
        return;
    }
    it.how = Some(format!("Next {what} costs {} {}; you have {}.", big(cost), resource(res), big(held.max(0.0))));
    it.state = if cost.is_finite() && held >= cost { State::Ready } else { State::Unknown };
}

/// The Harp, The Lamp, The Hive, Grotto, Justice Monument (caverns 6 to 10).
fn glowshroom_tunnels(h: &Holes) -> Mechanic {
    let held = h.nums(&[9]);
    // New notes are bought while `Holes[11][20] < 9` (`CanWeAfford_Harp`), on top of the first,
    // for `150 * (1 + n)^1.5 * 4.5^n` of the newest note (`HarpNewNote_Cost`).
    let bought = h.get(&[11], 20);
    let notes = if h.has(6) { 1.0 + bought } else { 0.0 };
    let mut harp = Item::new(format!("{}: notes unlocked", cave_name(5)), pic("HoleHarpNote0".into()), notes, 10.0);
    let res = 10 + (bought as usize).min(9);
    priced(&mut harp, "note", h.has(6), 5, 150.0 * (1.0 + bought).powf(1.5) * 4.5f64.powf(bought), res, at(&held, res));
    // More Wishes costs `floor(1 + 2L + L^2)` wishes, 999999 from level 11 (`LampWishCost`);
    // wishes held are `Holes[11][25]`.
    let wishes = if h.has(7) { h.get(&[21], 0) } else { 0.0 };
    let more_wishes = rows("LampWishes", &[]).first().map(|r| label(text(r, 0))).unwrap_or_default();
    let mut lamp = Item::new(format!("{}: {more_wishes}", cave_name(6)), pic("HoleLampUpg0".into()), wishes, 11.0);
    if !lamp.done() {
        if h.has(7) {
            let cost = (1.0 + 2.0 * wishes + wishes * wishes).floor();
            let stored = h.get(&[11], 25);
            lamp.how = Some(format!("Next level costs {} wishes; you have {}.", big(cost), big(stored.floor())));
            lamp.state = if stored >= cost { State::Ready } else { State::Unknown };
        } else {
            lamp.how = Some(format!("Find {} first.", cave_name(6)));
            lamp.state = State::Locked;
        }
    }
    let items = vec![harp, lamp, monument(h, 9, 1)];
    let mut m = mech("cavern-glowshroom", 5, "Glowshroom Tunnels", cave_pic(5), Count::Levels, "levels", items);
    m.note = Some(BIOME_NOTE.into());
    m
}

/// The Jars, Evertree, Wisdom Monument, Gambit, Temple (caverns 11 to 15).
fn underground_overgrowth(h: &Holes) -> Mechanic {
    let held = h.nums(&[9]);
    // New jar types are made while `Holes[11][37] < 9` (`CanWeAfford_Jar`), on top of the first,
    // for `50 * (1 + n)^1.5 * 4.8^n` of the newest rupie (`JarNewJar_Cost`).
    let made = h.get(&[11], 37);
    let jars = if h.has(11) { 1.0 + made } else { 0.0 };
    let mut jar = Item::new(format!("{}: jar types", cave_name(10)), pic("HoleJar0".into()), jars, 10.0);
    let res = 20 + (made as usize).min(9);
    priced(&mut jar, "jar type", h.has(11), 10, 50.0 * (1.0 + made).powf(1.5) * 4.8f64.powf(made), res, at(&held, res));
    // Collectibles: `Holes[24][i]` is the level, 1 once found; `HolesInfo[67]` lists them. Jars
    // only drop the first 16 until the Advanced Collection schematic (75) is built (`JarExtraUI`).
    let collectibles = info(67).len();
    let levels = h.nums(&[24]);
    let found = levels.iter().take(collectibles).filter(|&&v| v >= 1.0).count() as f64;
    let mut coll = Item::new(format!("{}: collectibles found", cave_name(10)), pic("HoleJarC0".into()), found, collectibles as f64);
    if !coll.done() {
        let first_page = (0..16).all(|i| at(&levels, i) >= 1.0);
        let advanced = h.get(&[13], 75) >= 1.0;
        if !h.has(11) {
            coll.how = Some(format!("Find {} first.", cave_name(10)));
            coll.state = State::Locked;
        } else if first_page && !advanced {
            let name = rows("HolesBuildings", &[]).get(75).map(|r| label(text(r, 0))).unwrap_or_default();
            coll.how = Some(format!("Collectibles 17 to 40 need the {name} schematic."));
            coll.state = State::Locked;
        } else {
            coll.how = Some("Found at random when jars break.".into());
        }
    }
    let items = vec![jar, coll, monument(h, 12, 2)];
    let mut m = mech("cavern-overgrowth", 5, "Underground Overgrowth", cave_pic(10), Count::Levels, "levels", items);
    m.note = Some(BIOME_NOTE.into());
    m
}
