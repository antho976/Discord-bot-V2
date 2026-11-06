//! World 3: Frostbite Tundra.

use std::collections::HashMap;

use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![
        buildings(save),
        salt_lick(save),
        prayers(save),
        atoms(save),
        death_note(save),
        equinox_dreams(save),
        equinox_upgrades(save),
        library(save),
        refinery(save),
        sampling(save),
        worship(save),
        armor_sets(save),
    ]
}

/// `Research[row][col]`: the World 7 lists other mechanics read their bonuses from.
fn research(row: usize, col: usize) -> f64 {
    rows("Research", &[row]).get(col).map(|v| num(v)).unwrap_or(0.0)
}

/// Menagerie boosts bought, `Menagerie[3][i]`. They cost more each time but never stop.
fn menagerie_boosts(save: &Save, i: usize) -> f64 {
    at(&save.numbers("Menagerie", &[3]), i)
}

/// How many of an item the storage chest holds: `ChestOrder[k]` is the item, `ChestQuantity[k]`
/// the amount. Inventories aren't counted, so this is the least the player has.
fn stored(save: &Save, id: &str) -> f64 {
    let Some(Value::Array(order)) = save.get("ChestOrder") else { return 0.0 };
    let qty = save.numbers("ChestQuantity", &[]);
    order.iter().enumerate().filter(|(_, v)| v.as_str() == Some(id)).map(|(k, _)| at(&qty, k)).sum()
}

fn item_name(id: &str) -> &str {
    GAME.item_name(id).unwrap_or(id)
}

/// Unique sushi made (N.js `UniqueSushi`): the run of tiers from 0 with `Sushi[5][n] >= 0`.
fn unique_sushi(save: &Save) -> usize {
    save.numbers("Sushi", &[5]).iter().take_while(|&&v| v >= 0.0).count()
}

/// A Sushi Station bonus (N.js `SushiStuff("RoG_BonusQTY")`): open once you've made more unique
/// sushi than its index; worth `Research[37][i]`.
fn sushi_bonus(save: &Save, i: usize) -> f64 {
    if unique_sushi(save) > i { research(37, i) } else { 0.0 }
}

/// Construction: `Tower[i]` is the building's level; `TowerInfo` col 8 is its base max, and the
/// game adds (N.js `ExtraMaxLvAtom`, keyed by that base max): wizard towers (50) +2 per Carbon
/// atom level, +30 from Construct Mastery and +100 from the Gambit cavern; the library (101)
/// +100 from Construct Mastery; shrines (100) +100 from Construct Mastery, +10 from Sushi Station
/// bonus 58 (`Research[37][58]`), +100 each from Spelunking (`Spelunk[4][6]` shrines, up to all
/// nine) and +10 per Menagerie boost 5; the trapper drone (15) +35 from Construct Mastery.
///
/// A level costs (N.js `TowerSaltCost`, `TowerMatCost`) `col 6 * (lv + 1)` of the salt in col 4
/// and `min(1e7, col 7 * (s + .03 - (s + .03 - 1.05) * lv / (col 8 / 2 + lv)) ^ lv)` of the
/// material in col 5 (`s` is col 9), both times the Cost Cruncher's discount: `max(.2, 1 -
/// (min(.1, .1 * floor((cc + 999) / 1000)) + max(0, cc - 1) * TowerInfo[5][2] / 100))`. Then it
/// builds (N.js `TowerBuildReq`): `RANDOlist[13][i] * col 9 ^ lv`, or `2 (lv+1)^2 1.3^(lv+1)` for
/// the 3D printer. Ready means the salts are in storage; the build time still follows.
fn buildings(save: &Save) -> Mechanic {
    let levels = save.numbers("Tower", &[]);
    let carbon = atom_max(save);
    let shrine_boosts = research(58, 5) * menagerie_boosts(save, 5);
    let table = rows("TowerInfo", &[]);
    let cc = at(&levels, 5);
    let cc_per = table.get(5).map_or(0.0, |r| cell(r, 2));
    let build_base: Vec<f64> = rows("RANDOlist", &[13]).iter().map(|v| num(v)).collect();
    let discount = (1.0 - ((0.1 * ((cc + 999.0) / 1000.0).floor()).min(0.1) + (cc - 1.0).max(0.0) * cc_per / 100.0)).max(0.2);
    let items = table
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let base = cell(r, 8);
            let extra = match base as i64 {
                50 => 2.0 * carbon + 30.0 + 100.0,
                101 => 100.0,
                100 => 100.0 + research(37, 58) + 100.0 + shrine_boosts,
                15 => 35.0,
                _ => 0.0,
            };
            let level = at(&levels, i);
            let mut it = Item::new(label(text(r, 0)), pic(format!("ConTower{i}")), level, base + extra);
            if level < it.max {
                let lv = level;
                let salt = (discount * cell(r, 6) * (lv + 1.0)).floor();
                let s9 = cell(r, 9);
                let mat = (discount * cell(r, 7) * (s9 + 0.03 - (s9 + 0.03 - 1.05) * lv / (base / 2.0 + lv)).powf(lv)).min(1e7).floor();
                let mut parts = vec![(text(r, 4), salt)];
                if text(r, 5) != "Blank" && mat > 0.0 {
                    parts.push((text(r, 5), mat));
                }
                let line: Vec<String> = parts.iter().map(|(id, n)| format!("{} {}", big(*n), item_name(id))).collect();
                let build = if i == 0 { 2.0 * (lv + 1.0).powi(2) * 1.3f64.powf(lv + 1.0) } else { at(&build_base, i) * s9.powf(lv) };
                it.how = Some(format!("Next level costs {}, then {} build progress.", line.join(" and "), big(build)));
                if parts.iter().all(|(id, n)| n.is_finite() && stored(save, id) >= *n) {
                    it.state = State::Ready;
                }
            }
            it
        })
        .collect();
    let mut m = mech("construction", 3, "Construction Buildings", pic("ConTowerA0".into()), Count::Levels, "levels", items);
    let spelunk = save.at("Spelunk", &[4, 6]).map(num).unwrap_or(0.0);
    m.raises = vec![
        Raise { name: "Carbon atom, per level".into(), amount: "+2 max levels on each of the 9 wizard towers".into(), owned: at(&save.numbers("Atoms", &[]), 5) > 0.0 },
        Raise { name: "Sushi Station bonus 59 (59 unique sushi)".into(), amount: format!("+{} max levels on all 9 shrines", research(37, 58)), owned: sushi_bonus(save, 58) > 0.0 },
        Raise { name: format!("Spelunking shrine unlocks, {} of 9 done", spelunk.clamp(0.0, 9.0)), amount: "+100 max levels on that shrine, one shrine each".into(), owned: spelunk >= 9.0 },
        Raise { name: "Menagerie boost 5, per boost".into(), amount: format!("+{} max levels on all 9 shrines", research(58, 5)), owned: menagerie_boosts(save, 5) > 0.0 },
    ];
    m.note = Some(format!(
        "Caps include every raise: wizard towers get 2 per Carbon atom level ({carbon} at the atom cap), 30 from Construct Mastery and 100 from the Gambit cavern; shrines get 100 from Construct Mastery, 10 from a Sushi Station bonus, 100 from Spelunking and 10 per Menagerie boost, so more boosts raise them further. Costs count your Cost Cruncher and only what's in storage."
    ));
    m
}

/// Salt Lick: `SaltLick[i]`; `SaltLicks` col 4 is the max, col 0 the salt. Upgrade i opens at
/// Salt Lick building level i + 1 (`i < Tower[3]`) and the next level costs `floor(col 2 *
/// col 5 ^ lv)` of its salt, from storage (the buy button's own check).
fn salt_lick(save: &Save) -> Mechanic {
    let levels = save.numbers("SaltLick", &[]);
    let built = at(&save.numbers("Tower", &[]), 3);
    let items = rows("SaltLicks", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let level = at(&levels, i);
            let mut it = Item::new(label(text(r, 1)), pic(text(r, 0).to_string()), level, cell(r, 4));
            if (i as f64) >= built && level <= 0.0 {
                it.how = Some(format!("Opens at Salt Lick building Lv {}.", i + 1));
                it.state = State::Locked;
            } else if level < it.max {
                let cost = salt_cost(r, level);
                it.how = Some(format!("Next level costs {} {}.", big(cost), item_name(text(r, 0))));
                if cost.is_finite() && stored(save, text(r, 0)) >= cost {
                    it.state = State::Ready;
                }
            }
            it
        })
        .collect();
    let mut m = mech("salt-lick", 3, "Salt Lick", pic("Refinery1".into()), Count::Levels, "levels", items);
    m.note = Some("Costs count only the salt in storage.".into());
    m
}

fn salt_cost(r: &Value, level: f64) -> f64 {
    (cell(r, 2) * cell(r, 5).powf(level)).floor()
}

/// Prayers: `PrayOwned[i]`; `PrayerInfo` col 9 is the max. Rows named "Some Prayer Name" are
/// fillers. A prayer is found at wave col 7 of totem col 8 (`TotemInfo[0]`), and a level costs
/// souls `Soul{col 5}` (N.js `PrayerCost`): `col 6 * (1 + (4 + i/25) * lv)` below Lv 6, then
/// `min(2e9, col 6 * (1 + (1 + i/20) * lv) * 1.12 ^ (lv - 5))` (prayer 9: `2 + i/20` and 1.3).
fn prayers(save: &Save) -> Mechanic {
    let levels = save.numbers("PrayOwned", &[]);
    let waves = save.numbers("TotemInfo", &[0]);
    let items = rows("PrayerInfo", &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| !text(r, 0).starts_with("Some_Prayer"))
        .map(|(i, r)| {
            let level = at(&levels, i);
            let mut it = Item::new(label(text(r, 0)), pic(format!("Prayer{i}")), level, cell(r, 9));
            it.world = totem_world(cell(r, 8) as usize);
            if level <= 0.0 {
                let (wave, totem) = (cell(r, 7), cell(r, 8) as usize);
                it.how = Some(format!("Found by reaching wave {wave} at {}.", totem_name(totem)));
                if at(&waves, totem) < wave {
                    it.state = State::Locked;
                }
            } else if level < it.max {
                let cost = prayer_cost(r, i, level);
                let soul = format!("Soul{}", text(r, 5));
                it.how = Some(format!("Next level costs {} {}.", big(cost), item_name(&soul)));
                if stored(save, &soul) >= cost {
                    it.state = State::Ready;
                }
            }
            it
        })
        .collect();
    let mut m = mech("prayers", 3, "Prayers", pic("Prayer0".into()), Count::Levels, "levels", items);
    m.note = Some("Costs count only the souls in storage.".into());
    m
}

fn prayer_cost(r: &Value, i: usize, lv: f64) -> f64 {
    let (base, t) = (cell(r, 6), i as f64);
    if lv < 6.0 {
        (base * (1.0 + (4.0 + t / 25.0) * lv)).round()
    } else if i == 9 {
        (base * (1.0 + (2.0 + t / 20.0) * lv) * 1.3f64.powf(lv - 5.0)).min(2e9).round()
    } else {
        (base * (1.0 + (1.0 + t / 20.0) * lv) * 1.12f64.powf(lv - 5.0)).min(2e9).round()
    }
}

/// The world a worship totem belongs to, from its monster (`WorshipBASEinfos` col 2, "w5b3"),
/// when that's after World 3.
fn totem_world(i: usize) -> Option<u8> {
    let mob = rows("WorshipBASEinfos", &[]).get(i).map(|r| text(r, 2)).unwrap_or("");
    let w = mob.strip_prefix('w').and_then(|r| r.chars().next()).and_then(|c| c.to_digit(10))?;
    (w > 3).then_some(w as u8)
}

/// A worship totem's name, `RANDOlist[10][i]`; the game's list stops short of the newest.
fn totem_name(i: usize) -> String {
    rows("RANDOlist", &[10]).get(i).and_then(|v| v.as_str()).map_or_else(|| format!("Tower Defence {}", i + 1), label)
}

/// The atom cap (N.js `AtomMaxLv`): 20, +10 (gaming superbit), +20 (compass), +20 (event shop),
/// plus `Research[58][17]` per Menagerie boost 17. Boosts have no end, so the cap counts the
/// boosts bought, and at least one.
fn atom_max(save: &Save) -> f64 {
    70.0 + research(58, 17) * menagerie_boosts(save, 17).max(1.0)
}

/// Atom Collider: `Atoms[i]`, one cap for every atom. `AtomInfo` lists all 17, the six the game
/// unlocks later (compass, Jelly, Minehead, Sushi and Menagerie unlock 16) included.
/// The particles atom `row` costs at `level`, before the player's discounts (N.js `AtomCost`:
/// `(AtomInfo[3] + AtomInfo[1] * lv) * AtomInfo[2] ^ lv`, then divided by their bonuses).
fn atom_cost(row: &Value, level: f64) -> f64 {
    (cell(row, 3) + cell(row, 1) * level) * cell(row, 2).powf(level)
}

/// The level `particles` would take an atom to on its own, from `level`, up to `max`. Each level
/// costs more than the last, so this stops within a few hundred steps.
fn atom_reach(row: &Value, level: f64, max: f64, particles: f64) -> f64 {
    let (mut lv, mut left) = (level, particles);
    while lv < max {
        let c = atom_cost(row, lv);
        if !(c.is_finite() && c <= left) {
            break;
        }
        left -= c;
        lv += 1.0;
    }
    lv
}

/// Where atoms 12 to 17 come from, in the order the game's descriptions point to.
const LATE_ATOMS: [&str; 6] = [
    "the Magnesium Atom compass upgrade",
    "the Aluminium Atom compass upgrade",
    "Minehead bonus 8",
    "making 23 unique sushi",
    "Jelly Operator bonus 18",
    "Menagerie unlock 16",
];

/// Atoms open (N.js `AtomsAvailableAtAll`): 11, plus one each for compass upgrades 47 and 58
/// (`Compass[0][i]`), Jelly Operator bonus 17 (`Research[7][9] > 17`), Minehead bonus 7
/// (`Research[7][4] > 7`), Sushi Station bonus 22 (more than 22 unique sushi) and Menagerie
/// unlock 16. The game counts them, so the next atom in the list opens whichever comes first.
fn atoms_available(save: &Save) -> usize {
    let compass = save.numbers("Compass", &[0]);
    let r7 = save.numbers("Research", &[7]);
    let sources = [
        at(&compass, 47) > 0.0,
        at(&compass, 58) > 0.0,
        at(&r7, 9) > 17.0,
        at(&r7, 4) > 7.0,
        unique_sushi(save) > 22,
        super::w7::menagerie_unlock(save, 16),
    ];
    11 + sources.iter().filter(|&&b| b).count()
}

fn atoms(save: &Save) -> Mechanic {
    let levels = save.numbers("Atoms", &[]);
    let max = atom_max(save);
    let particles = save.at("Divinity", &[39]).map(num).unwrap_or(0.0).max(0.0);
    let table = rows("AtomInfo", &[]);
    let available = atoms_available(save);
    let items: Vec<Item> = table
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let name = text(r, 0).split("_-_").next().unwrap_or("");
            let level = at(&levels, i);
            let mut it = Item::new(label(name), pic(format!("Atom{i}")), level, max);
            // Their unlocks are World 6 (the Windwalker's compass) and World 7 content.
            it.world = match i {
                11 | 12 => Some(6),
                13.. => Some(7),
                _ => None,
            };
            if i >= available && level <= 0.0 {
                let source = LATE_ATOMS.get(i.wrapping_sub(11)).copied().unwrap_or("another atom unlock");
                it.how = Some(format!("Opens with one more atom unlock, such as {source}."));
                it.state = State::Locked;
            } else if level < max {
                let cost = atom_cost(r, level);
                it.how = Some(format!("Next level costs {} particles.", big(cost)));
                it.state = if cost <= particles { State::Ready } else { State::Unknown };
            }
            it
        })
        .collect();
    // What the particles on hand buy: the two lowest atoms that can move, each on its own.
    let mut open: Vec<(usize, &Item)> = items.iter().enumerate().filter(|(_, it)| it.state == State::Ready).collect();
    open.sort_by(|a, b| a.1.level.total_cmp(&b.1.level));
    let reach: Vec<String> = open
        .iter()
        .take(2)
        .filter_map(|(i, it)| table.get(*i).map(|r| format!("{} to Lv {}", it.name, atom_reach(r, it.level, max, particles))))
        .collect();
    let have = Have {
        figure: format!("{} particles", big(particles)),
        text: match reach.as_slice() {
            [] => "Not enough for the next level of any atom yet.".into(),
            [one] => format!("Enough to take {one}."),
            [a, b, ..] => format!("Enough to take {a}, or {b}. Costs here are before your discounts, so you get a little further."),
        },
    };
    let mut m = mech("atoms", 3, "Atom Collider", pic("Atom0".into()), Count::Levels, "levels", items);
    m.have = Some(have);
    m.raises = vec![Raise { name: "Menagerie boost 17, per boost".into(), amount: "+1 max level on every atom".into(), owned: menagerie_boosts(save, 17) > 0.0 }];
    m.note = Some(format!(
        "The cap starts at 20. A gaming superbit, a compass upgrade and an event shop item raise it to 70, and each Menagerie boost adds 1 more (counted here: {max})."
    ));
    m
}

/// Death Note skull points for a kill count (N.js `DeathNoteRank`). Eclipse skulls (20) need a
/// billion kills and rift 20.
fn skull(kills: f64, mini: bool, eclipse: bool) -> f64 {
    let (steps, vals): (&[f64], &[f64]) = if mini {
        (&[100.0, 250.0, 1e3, 5e3, 25e3, 1e5, 1e6], &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 7.0])
    } else {
        (&[25e3, 1e5, 25e4, 5e5, 1e6, 5e6, 1e8], &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 7.0])
    };
    if let Some(k) = steps.iter().position(|&s| kills < s) {
        return vals[k];
    }
    if !mini && kills > 1e9 && eclipse { 20.0 } else { 10.0 }
}

/// The kills the next skull needs, or `None` past the last one. `Err` when it's the Eclipse
/// skull and rift 20 isn't reached yet.
fn next_skull(kills: f64, mini: bool, eclipse: bool) -> Option<Result<f64, f64>> {
    let steps: &[f64] = if mini { &[100.0, 250.0, 1e3, 5e3, 25e3, 1e5, 1e6] } else { &[25e3, 1e5, 25e4, 5e5, 1e6, 5e6, 1e8] };
    if let Some(&s) = steps.iter().find(|&&s| kills < s) {
        return Some(Ok(s));
    }
    if mini || kills > 1e9 && eclipse {
        return None;
    }
    Some(if eclipse { Ok(1e9) } else { Err(1e9) })
}

fn skull_item(name: String, id: &str, kills: f64, mini: bool, eclipse: bool) -> Item {
    let mut it = Item::new(name, face(id), skull(kills, mini, eclipse), if mini { 10.0 } else { 20.0 });
    match next_skull(kills, mini, eclipse) {
        Some(Ok(s)) => it.how = Some(format!("{} more kills for the next skull ({} of {}).", big((s - kills).max(1.0)), big(kills), big(s))),
        Some(Err(_)) => {
            it.how = Some("Eclipse skulls open at rift 20, then take a billion kills.".into());
            it.state = State::Locked;
        }
        None => {}
    }
    it
}

fn face(id: &str) -> Option<String> {
    GAME.extra("w3").get("faces").and_then(|f| f.get(id)).map(|v| format!("Mface{}", num(v)))
}

/// Death Note: for each monster in `DeathNoteMobs[world]`, its map is `MapAFKtarget.indexOf(id)`
/// and the kills are, over every character, `MapDetails[map][0][0] - KLA_c[map][0]` (kills left
/// count down from the portal requirement). Minibosses are `NinjaInfo[30]`, kills in `Ninja[105]`.
fn death_note(save: &Save) -> Mechanic {
    let targets = rows("MapAFKtarget", &[]);
    let details = rows("MapDetails", &[]);
    let chars = save.characters();
    let eclipse = at(&save.numbers("Rift", &[]), 0) >= 20.0;
    let mut items = Vec::new();
    for (w, world) in rows("DeathNoteMobs", &[]).into_iter().enumerate() {
        for id in world.as_array().into_iter().flatten().filter_map(Value::as_str) {
            let kills = targets.iter().position(|t| t.as_str() == Some(id)).map_or(0.0, |map| {
                let req = details.get(map).and_then(|d| d.get(0)).and_then(|d| d.get(0)).map(num).unwrap_or(0.0);
                (0..chars)
                    .filter_map(|c| save.at(&format!("KLA_{c}"), &[map, 0]).map(num))
                    .map(|left| (req - left).max(0.0))
                    .sum()
            });
            let name = GAME.monster_name(id).unwrap_or(id).to_string();
            let mut it = skull_item(name, id, kills, false, eclipse);
            it.world = (w + 1 > 3).then_some(w as u8 + 1);
            items.push(it);
        }
    }
    let mini_kills = save.numbers("Ninja", &[105]);
    for (i, id) in rows("NinjaInfo", &[30]).iter().filter_map(|v| v.as_str()).enumerate() {
        let name = format!("{} (miniboss)", GAME.monster_name(id).unwrap_or(id));
        let mut it = skull_item(name, id, at(&mini_kills, i), true, eclipse);
        // Miniboss kills are kept by Sneaking (`Ninja[105]`), World 6 content.
        it.world = Some(6);
        items.push(it);
    }
    let mut m = mech("death-note", 3, "Death Note", face("mushG"), Count::Levels, "skull points", items);
    m.note = Some("Skull points per monster: 20 is an Eclipse Skull, a billion kills once rift 20 unlocks them. Minibosses (their page came with Sneaking in World 6) top out at 10.".into());
    m
}

/// A dream's challenge up to its first sentence: "Reach Lv 100 on all of your characters".
fn challenge(s: &str) -> String {
    let mut end = s.len();
    let mut from = 0;
    while let Some(p) = s[from..].find("._") {
        let p = from + p;
        let next = s[p + 2..].chars().next().unwrap_or(' ');
        let word = s[..p].rsplit('_').next().unwrap_or("");
        if (next.is_uppercase() || next.is_ascii_digit()) && !matches!(word, "Mr" | "Lv" | "LV") {
            end = p;
            break;
        }
        from = p + 2;
    }
    label(&s[..end])
}

/// A `how` line kept to about 90 characters, cut at a word.
fn clip(s: &str) -> String {
    if s.chars().count() <= 90 {
        return s.to_string();
    }
    let cut: String = s.chars().take(87).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(a, _)| a).trim_end_matches([',', ':', ' ']);
    format!("{cut}...")
}

fn dream_done(save: &Save, i: usize) -> bool {
    save.get("WeeklyBoss").and_then(|w| w.get(format!("d_{i}"))).map(num) == Some(-1.0)
}

/// Equinox dreams: `WeeklyBoss["d_i"]` is -1 once dream i is done; `DreamChallenge` lists them,
/// col 2 the reward. Dreams from 36 on (the nightmares) only come up once research grid node 86
/// has a level (N.js `CloudsTask`: `36 > i || Grid_Bonus(86) >= 1`, `Research[0][86]`).
fn equinox_dreams(save: &Save) -> Mechanic {
    let nightmares = at(&save.numbers("Research", &[0]), 86) >= 1.0;
    let node = rows("ResGridSquares", &[]).get(86).map(|r| label(text(r, 0))).unwrap_or_else(|| "Equinox Nightmares".into());
    let items = rows("DreamChallenge", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let done = dream_done(save, i);
            let mut it = Item::new(challenge(text(r, 0)), None, f64::from(u8::from(done)), 1.0);
            // The nightmares are gated on a World 7 research node.
            if i >= 36 {
                it.world = Some(7);
            }
            if !done {
                if i >= 36 && !nightmares {
                    it.how = Some(format!("Comes up once the research node {node} has a level."));
                    it.state = State::Locked;
                } else {
                    let reward = words(text(r, 2)).trim_end_matches(['!', '.']).to_string();
                    it.how = Some(clip(&format!("Reward: {reward}.")));
                }
            }
            it
        })
        .collect();
    mech("equinox-dreams", 3, "Equinox Dreams", pic("Dream_Cloud".into()), Count::Done, "done", items)
}

/// Max levels each dream adds to an Equinox upgrade, from its reward text
/// ("+5_Max_LV_for_'Matching_Scims'_Equinox_Upgrade"), by upgrade name.
fn dream_raises() -> HashMap<&'static str, f64> {
    let mut out = HashMap::new();
    for (_, name, n) in dream_raise_list() {
        *out.entry(name).or_insert(0.0) += n;
    }
    out
}

/// Each dream that raises an upgrade's cap: (dream index, upgrade name, levels).
fn dream_raise_list() -> Vec<(usize, &'static str, f64)> {
    let mut out = Vec::new();
    for (i, r) in rows("DreamChallenge", &[]).iter().enumerate() {
        let reward = text(r, 2);
        let Some(rest) = reward.strip_prefix('+') else { continue };
        if !reward.contains("_Max_LV_for_") {
            continue;
        }
        let n: f64 = rest.split('_').next().and_then(|d| d.parse().ok()).unwrap_or(0.0);
        if let Some(name) = reward.split('\'').nth(1) {
            out.push((i, name, n));
        }
    }
    out
}

/// What Summoning adds to the Equinox caps (N.js `WinBonus` 24, unmultiplied): every normal
/// battle of that bonus type, plus endless wins, which cycle through `SummonEnemies[9..10]` 40
/// at a time and never stop.
fn summoning_equinox(save: &Save) -> f64 {
    let se = |r: usize| rows("SummonEnemies", &[r]);
    let (kind, value) = (se(5), se(7));
    let battles: f64 = kind.iter().zip(&value).filter(|(k, _)| k.as_str() == Some("25")).map(|(_, v)| num(v)).sum();
    let (ekind, evalue) = (se(9), se(10));
    // Endless wins cycle through the endless rewards: whole cycles, then the part of the last.
    // Counted, not looped, since the win count comes from the paste.
    let wins = save.option(319).clamp(0.0, 1e12) as usize;
    let endless: f64 = if ekind.is_empty() {
        0.0
    } else {
        let per = |k: usize| if ekind[k].as_str() == Some("25") { evalue.get(k).map_or(0.0, |v| num(v)) } else { 0.0 };
        let cycle: f64 = (0..ekind.len()).map(per).sum();
        let rest: f64 = (0..wins % ekind.len()).map(per).sum();
        (wins / ekind.len()) as f64 * cycle + rest
    };
    battles + endless
}

/// Equinox upgrades: `Dream[2 + i]`; `DreamUpg` col 2 is the base max. N.js `UpgMaxLV` adds the
/// dreams' raises, +10 from a gaming superbit and Summoning wins for upgrades 4, 5 and 7 to 11,
/// and Summoning alone for 12. Rows marked "Filler" aren't upgrades.
fn equinox_upgrades(save: &Save) -> Mechanic {
    let levels = save.numbers("Dream", &[]);
    let raises = dream_raises();
    let list = dream_raise_list();
    let summon = summoning_equinox(save);
    // A level costs one full bar (N.js the upgrade button: `IsBarFULL`, then `Dream[0] = 0`); the
    // bar needs `(120 + 40 L) * 1.02 ^ L`, L the levels bought across `Dream[2..16]` (`BarFillReq`).
    let bought: f64 = (2..16).map(|k| at(&levels, k).max(0.0)).sum();
    let bar_req = (120.0 + 40.0 * bought) * 1.02f64.powf(bought);
    let bar = at(&levels, 0).max(0.0);
    // Upgrade i opens with the i-th "Unlock next Equinox upgrade" dream (N.js `UpgUnlocked`).
    let unlockers: Vec<usize> = rows("DreamChallenge", &[]).iter().enumerate().filter(|(_, r)| text(r, 2) == "Unlock_next_Equinox_upgrade").map(|(i, _)| i).collect();
    let dream_name = |d: usize| rows("DreamChallenge", &[]).get(d).map(|r| challenge(text(r, 0))).unwrap_or_default();
    let items = rows("DreamUpg", &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| text(r, 3) != "Filler")
        .map(|(i, r)| {
            let mut max = cell(r, 2) + raises.get(text(r, 0)).copied().unwrap_or(0.0);
            if matches!(i, 4 | 5 | 7..=11) {
                max += 10.0 + summon;
            } else if i == 12 {
                max += summon;
            }
            let level = at(&levels, i + 2);
            let mut it = Item::new(label(text(r, 0)), None, level, max);
            let opener = i.checked_sub(1).and_then(|k| unlockers.get(k)).copied();
            // The cap as it stands: the full one less the raises from dreams not done yet.
            let pending: f64 = list.iter().filter(|(d, name, _)| *name == text(r, 0) && !dream_done(save, *d)).map(|(_, _, n)| n).sum();
            if let Some(d) = opener.filter(|&d| level <= 0.0 && !dream_done(save, d)) {
                it.how = Some(clip(&format!("Opens with dream {}: {}.", d + 1, dream_name(d))));
                it.state = State::Locked;
            } else if level < max - pending {
                it.how = Some(format!("Each level takes a full Equinox bar: {} of {} now.", big(bar), big(bar_req)));
                if bar >= bar_req {
                    it.state = State::Ready;
                }
            } else if let Some((d, _, n)) = list.iter().find(|(d, name, _)| *name == text(r, 0) && !dream_done(save, *d)) {
                it.how = Some(clip(&format!("Dream {} raises the cap by {n}: {}.", d + 1, dream_name(*d))));
            }
            it
        })
        .collect();
    let mut m = mech("equinox-upgrades", 3, "Equinox Upgrades", pic("Dream_Cloud".into()), Count::Levels, "levels", items);
    m.have = Some(Have {
        figure: format!("{} of {} Equinox bar", big(bar.min(bar_req)), big(bar_req)),
        text: if bar >= bar_req { "The bar is full: enough for one level of any open upgrade.".into() } else { "Not full yet. Each level bought makes the next bar a little longer.".into() },
    });
    let upg = |name: &str| label(name);
    m.raises = list.iter().map(|(d, name, n)| Raise { name: format!("Dream {}", d + 1), amount: format!("+{n} max levels on {}", upg(name)), owned: dream_done(save, *d) }).collect();
    m.note = Some(format!(
        "Caps include every dream's raise and a gaming superbit. Endless Summoning wins keep adding to most of them (+{summon} from your wins so far)."
    ));
    m
}

/// Talent book library (N.js `maxBookLv`): 125, plus the Fury Relic (col 3 per artifact tier, 7
/// tiers), 10 once Oxygen has a level, 5 for the Checkout Takeout achievement, Salt Lick row 4
/// (col 3 per level) and World 3 merit 2 (`TaskShopDesc[2][2]`, col 11 per level, col 5 levels),
/// read from `TaskZZ2[2][2]`.
fn library(save: &Save) -> Mechanic {
    let mut items = Vec::new();
    if let Some(r) = rows("ArtifactInfo", &[]).get(21) {
        let tier = at(&save.numbers("Sailing", &[3]), 21).clamp(0.0, 7.0);
        let mut it = Item::new(label(text(r, 0)), pic("Arti21".into()), cell(r, 3) * tier, cell(r, 3) * 7.0);
        it.world = Some(5);
        it.how = Some(format!("A Sailing artifact: each of its 7 tiers adds {} book levels.", cell(r, 3)));
        items.push(it);
    }
    if let Some(r) = rows("AtomInfo", &[]).get(7) {
        let name = text(r, 0).split("_-_").next().unwrap_or("");
        let have = if at(&save.numbers("Atoms", &[]), 7) >= 1.0 { 10.0 } else { 0.0 };
        let mut it = Item::new(format!("{} atom", label(name)), pic("Atom7".into()), have, 10.0);
        it.how = Some("Its first level in the Atom Collider adds all 10.".into());
        items.push(it);
    }
    if let Some(r) = rows("RegAchieves", &[]).get(145) {
        let have = if at(&save.numbers("AchieveReg", &[]), 145) == -1.0 { 5.0 } else { 0.0 };
        let mut it = Item::new(format!("{} achievement", label(text(r, 0))), None, have, 5.0);
        it.how = Some(format!("{}.", label(text(r, 2))));
        items.push(it);
    }
    if let Some(r) = rows("SaltLicks", &[]).get(4) {
        let lv = at(&save.numbers("SaltLick", &[]), 4);
        let mut it = Item::new("Salt Lick", pic(text(r, 0).to_string()), lv * cell(r, 3), cell(r, 4) * cell(r, 3));
        if lv < cell(r, 4) {
            let cost = salt_cost(r, lv);
            it.how = Some(format!("Salt Lick upgrade 5: next level costs {} {}.", big(cost), item_name(text(r, 0))));
            if cost.is_finite() && stored(save, text(r, 0)) >= cost && at(&save.numbers("Tower", &[]), 3) > 4.0 {
                it.state = State::Ready;
            }
        }
        items.push(it);
    }
    if let Some(r) = rows("TaskShopDesc", &[2]).get(2) {
        let lv = at(&save.numbers("TaskZZ2", &[2]), 2);
        let mut it = Item::new("World 3 merit", pic("TaskSc3".into()), lv * cell(r, 11), cell(r, 5) * cell(r, 11));
        it.how = Some(format!("Bought in the World 3 merit shop: +{} book levels per merit level.", cell(r, 11)));
        items.push(it);
    }
    let mut m = mech("library", 3, "Talent Book Library", pic("ConTower1".into()), Count::Levels, "book levels", items);
    m.note = Some("How high the library prints talent books, on top of its base of 125. Summoning wins add more that isn't counted here.".into());
    m
}

/// Refinery: `Refinery[3 + i][1]` is salt i's rank; salts from `Refinery[0][0]` on aren't unlocked.
/// Ranks have no max.
///
/// A salt ranks up when its power, `Refinery[3 + i][0]`, reaches `PowerCAP(rank) = max(RANDOlist[18]
/// [min(rank, len - 2)], 25)` and you claim it; each cycle adds `PowerPerCycle = floor(min(250K,
/// rank ^ 1.3))` before the Companion bonus. Salt k opens (raising `Refinery[0][0]` from k) by
/// paying `extra_w3 salt_unlocks[1 + k]` of salt k from storage.
fn refinery(save: &Save) -> Mechanic {
    let unlocked = save.at("Refinery", &[0, 0]).map(num).unwrap_or(0.0).max(0.0);
    let caps = rows("RANDOlist", &[18]);
    let power_cap = |rank: f64| -> f64 {
        let k = (rank.max(0.0) as usize).min(caps.len().saturating_sub(2));
        caps.get(k).map_or(25.0, |v| num(v)).max(25.0)
    };
    let unlock_costs = GAME.extra("w3").get("salt_unlocks").map(crate::save::numbers).unwrap_or_default();
    let table = rows("RefineryInfo", &[]);
    // (seconds to the next rank, or a big number for salts not open, item)
    let mut ranked: Vec<(f64, Item)> = table
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let id = text(r, 12);
            let open = (i as f64) < unlocked;
            let rank = if open { save.at("Refinery", &[3 + i, 1]).map(num).unwrap_or(0.0) } else { 0.0 };
            let mut it = Item::new(item_name(id), pic(id.to_string()), rank, 0.0);
            // Salts made from World 6 and 7 materials (RefineryInfo col 0).
            it.world = match text(r, 0) {
                m if m.starts_with("Spi") => Some(6),
                m if m.starts_with("w7") => Some(7),
                _ => None,
            };
            let mut secs = f64::MAX;
            if open {
                let power = save.at("Refinery", &[3 + i, 0]).map(num).unwrap_or(0.0).max(0.0);
                let cap = power_cap(rank);
                let per = rank.max(1.0).powf(1.3).min(25e4).floor().max(1.0);
                if power >= cap {
                    it.how = Some(format!("Full power ({}): claim it now for the next rank.", big(cap)));
                    it.state = State::Ready;
                    secs = 0.0;
                } else {
                    let cycles = ((cap - power) / per).ceil();
                    secs = cycles * base_cycle(i / 3);
                    let unit = if cycles == 1.0 { "cycle" } else { "cycles" };
                    it.how = Some(format!("Next rank in {}: {} {unit} to {} power, {} now.", duration(secs), big(cycles), big(cap), big(power)));
                }
            } else if i == unlocked as usize && i > 0 {
                let pay = format!("Refinery{i}");
                let cost = at(&unlock_costs, i + 1);
                it.how = Some(format!("Unlocks for {} {} from storage.", big(cost), item_name(&pay)));
                if cost > 0.0 && stored(save, &pay) >= cost {
                    it.state = State::Ready;
                }
            } else if i > 0 {
                let prev = table.get(i - 1).map_or("", |p| item_name(text(p, 12)));
                it.how = Some(format!("Opens after {prev} is unlocked."));
                it.state = State::Locked;
            }
            (secs, it)
        })
        .collect();
    // Soonest rank-up first: full power, then the least time left; salts not open last.
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
    let items = ranked.into_iter().map(|(_, it)| it).collect();
    let mut m = mech("refinery", 3, "Refinery", pic("Refinery1".into()), Count::Open, "ranks", items);
    m.open_order = Some("Soonest rank-up first");
    m.note = Some("Refinery ranks have no cap. Times use the game's base cycle length (15 min, 1 h, then 100 h for the third row of salts) before your refinery speed and Companion bonuses, so yours are shorter.".into());
    m
}

/// A refinery cycle's base length for salt row `g` (N.js `CycleInitialTime`, before the speed
/// bonuses it divides by): `900 * 4 ^ g * 25 ^ max(0, g - 1)` seconds.
fn base_cycle(g: usize) -> f64 {
    let g = g.min(10) as f64;
    900.0 * 4f64.powf(g) * 25f64.powf((g - 1.0).max(0.0))
}

/// "45 min", "32 h", "12 days".
fn duration(secs: f64) -> String {
    let m = secs / 60.0;
    if m < 90.0 {
        format!("{} min", m.ceil().max(1.0))
    } else if m < 48.0 * 60.0 {
        format!("{} h", (m / 60.0).round())
    } else {
        format!("{} days", big((m / 1440.0).round()))
    }
}

/// 3D printer samples: `Print[5..]` then `PrinterXtra` hold (item, sample size) pairs, every
/// character's slots in a row. The best sample of each item; sizes have no cap.
fn sampling(save: &Save) -> Mechanic {
    let mut best: Vec<(String, f64)> = Vec::new();
    let pairs = |key: &str, skip: usize| -> Vec<(String, f64)> {
        let Some(Value::Array(a)) = save.get(key) else { return Vec::new() };
        a.iter().skip(skip).collect::<Vec<_>>().chunks(2).filter_map(|p| Some((p.first()?.as_str()?.to_string(), p.get(1).map_or(0.0, |v| num(v))))).collect()
    };
    for (id, size) in pairs("Print", 5).into_iter().chain(pairs("PrinterXtra", 0)) {
        if id == "Blank" || id == "0" || id.is_empty() || size <= 0.0 {
            continue;
        }
        match best.iter_mut().find(|(b, _)| *b == id) {
            Some(b) => b.1 = b.1.max(size),
            None => best.push((id, size)),
        }
    }
    let mut items: Vec<Item> = best
        .into_iter()
        .map(|(id, size)| {
            let picture = crate::pictures::has(&id).then(|| id.clone());
            Item::new(GAME.item_name(&id).unwrap_or(&id), picture, size, 0.0)
        })
        .collect();
    if items.is_empty() {
        items.push(Item::new("No samples yet", None, 0.0, 0.0));
    }
    let mut m = mech("sampling", 3, "3D Printer Samples", pic("ConTower0".into()), Count::Open, "sample size", items);
    m.note = Some("Your best sample of each item. Sample sizes have no cap.".into());
    m
}

/// Worship: `TotemInfo[0][i]` is totem i's best wave; `WorshipBASEinfos` lists the totems and
/// `RANDOlist[10]` names them. Waves have no cap.
fn worship(save: &Save) -> Mechanic {
    let waves = save.numbers("TotemInfo", &[0]);
    let prayers = save.numbers("PrayOwned", &[]);
    let names = rows("RANDOlist", &[10]);
    let items = rows("WorshipBASEinfos", &[])
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let name = names.get(i).and_then(|v| v.as_str()).map_or_else(|| format!("Tower Defence {}", i + 1), label);
            let best = at(&waves, i);
            let mut it = Item::new(name, None, best, 0.0);
            it.world = totem_world(i);
            // The next prayer this totem finds (PrayerInfo col 7 wave, col 8 totem).
            let next = rows("PrayerInfo", &[])
                .into_iter()
                .enumerate()
                .filter(|(k, p)| !text(p, 0).starts_with("Some_Prayer") && cell(p, 8) as usize == i && cell(p, 7) > best && at(&prayers, *k) <= 0.0)
                .min_by(|a, b| cell(a.1, 7).total_cmp(&cell(b.1, 7)));
            if let Some((_, p)) = next {
                it.how = Some(format!("Wave {} finds the prayer {}.", cell(p, 7), label(text(p, 0))));
            }
            it
        })
        .collect();
    let mut m = mech("worship", 3, "Worship Totems", pic("WorshipSkull1".into()), Count::Open, "best wave", items);
    m.note = Some("Your best wave at each totem. Waves have no cap.".into());
    m
}

/// Armor sets: `EquipmentSets` (read by extra_w3.py) names every set; `OptLacc[379]` lists the
/// finished ones by name, comma-separated. The game adds a set there when every part is worn at
/// once (N.js `GetSetBonus` "PartsOn" >= "PartsReq", with the smithy's permanent unlock).
fn armor_sets(save: &Save) -> Mechanic {
    let owned: Vec<&str> = save.at("OptLacc", &[379]).and_then(Value::as_str).map(|s| s.split(',').collect()).unwrap_or_default();
    let items = GAME
        .extra("w3")
        .get("sets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|s| Some((s.get(0)?.as_str()?, s.get(1)?.as_str()?, s.get(2).and_then(Value::as_array))))
        .map(|(name, hat, pieces)| {
            let done = owned.contains(&name);
            let mut it = Item::new(label(name), pic(hat.to_string()), f64::from(u8::from(done)), 1.0);
            if !done {
                let names: Vec<&str> = pieces.into_iter().flatten().filter_map(Value::as_str).map(item_name).collect();
                it.how = Some(clip(&format!("Armor to wear together: {}.", names.join(", "))));
            }
            it
        })
        .collect();
    mech("armor-sets", 3, "Armor Sets", pic("EquipmentHats17".into()), Count::Done, "finished", items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skulls_follow_the_game() {
        assert_eq!(skull(24_999.0, false, true), 0.0);
        assert_eq!(skull(1e8, false, false), 10.0);
        assert_eq!(skull(2e9, false, false), 10.0);
        assert_eq!(skull(2e9, false, true), 20.0);
        assert_eq!(skull(5e6, true, true), 10.0);
    }

    #[test]
    fn challenges_read_like_the_game() {
        assert_eq!(challenge("Get_400_kills_in_a_single_Killroy_run._Why_do_they"), "Get 400 kills in a single Killroy run");
        assert_eq!(challenge("Hit_a_bomb_instantly_while_playing_Mr._Minehead's_Depth_Charge_5_times"), "Hit a bomb instantly while playing Mr. Minehead's Depth Charge 5 times");
    }

    #[test]
    fn dream_raises_reach_the_game_caps() {
        let r = dream_raises();
        assert_eq!(r.get("Liquidvestment"), Some(&7.0));
        assert_eq!(r.get("Nonstop_Studies"), Some(&57.0));
    }
}
