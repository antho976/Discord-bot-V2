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

/// Construction: `Tower[i]` is the building's level; `TowerInfo` col 8 is its base max, and the
/// game adds (N.js `ExtraMaxLvAtom`, keyed by that base max): wizard towers (50) +2 per Carbon
/// atom level, +30 from Construct Mastery and +100 from the Gambit cavern; the library (101)
/// +100 from Construct Mastery; shrines (100) +100 from Construct Mastery, +150 from a Sushi
/// RoG bonus (`Research[47][58]`), +100 each from Spelunking (`Spelunk[4][6]` shrines, up to all
/// nine) and +10 per Menagerie boost; the trapper drone (15) +35 from Construct Mastery.
fn buildings(save: &Save) -> Mechanic {
    let levels = save.numbers("Tower", &[]);
    let carbon = atom_max(save);
    let shrine_boosts = research(58, 5) * menagerie_boosts(save, 5);
    let items = rows("TowerInfo", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let base = cell(r, 8);
            let extra = match base as i64 {
                50 => 2.0 * carbon + 30.0 + 100.0,
                101 => 100.0,
                100 => 100.0 + research(47, 58) + 100.0 + shrine_boosts,
                15 => 35.0,
                _ => 0.0,
            };
            Item::new(label(text(r, 0)), pic(format!("ConTower{i}")), at(&levels, i), base + extra)
        })
        .collect();
    let mut m = mech("construction", 3, "Construction Buildings", pic("ConTower0".into()), Count::Levels, "levels", items);
    m.note = Some(format!(
        "Caps include every raise: wizard towers get 2 per Carbon atom level ({carbon} at the atom cap), 30 from Construct Mastery and 100 from the Gambit cavern; shrines get 100 from Construct Mastery, 150 from a Sushi RoG bonus, 100 from Spelunking and 10 per Menagerie boost, so more boosts raise them further."
    ));
    m
}

/// Salt Lick: `SaltLick[i]`; `SaltLicks` col 4 is the max, col 0 the salt.
fn salt_lick(save: &Save) -> Mechanic {
    let levels = save.numbers("SaltLick", &[]);
    let items = rows("SaltLicks", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| Item::new(label(text(r, 1)), pic(text(r, 0).to_string()), at(&levels, i), cell(r, 4)))
        .collect();
    mech("salt-lick", 3, "Salt Lick", pic("Refinery1".into()), Count::Levels, "levels", items)
}

/// Prayers: `PrayOwned[i]`; `PrayerInfo` col 9 is the max. Rows named "Some Prayer Name" are fillers.
fn prayers(save: &Save) -> Mechanic {
    let levels = save.numbers("PrayOwned", &[]);
    let items = rows("PrayerInfo", &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| !text(r, 0).starts_with("Some_Prayer"))
        .map(|(i, r)| Item::new(label(text(r, 0)), pic(format!("Prayer{i}")), at(&levels, i), cell(r, 9)))
        .collect();
    mech("prayers", 3, "Prayers", pic("Prayer0".into()), Count::Levels, "levels", items)
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

fn atoms(save: &Save) -> Mechanic {
    let levels = save.numbers("Atoms", &[]);
    let max = atom_max(save);
    let particles = save.at("Divinity", &[39]).map(num).unwrap_or(0.0).max(0.0);
    let table = rows("AtomInfo", &[]);
    let items: Vec<Item> = table
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let name = text(r, 0).split("_-_").next().unwrap_or("");
            let level = at(&levels, i);
            let mut it = Item::new(label(name), pic(format!("Atom{i}")), level, max);
            // Chlorine, the 17th, comes with Menagerie unlock 16.
            if i == 16 && level <= 0.0 && !super::w7::menagerie_unlock(save, 16) {
                it.how = Some("Unlocked by Menagerie unlock 16.".into());
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
    m.raises = vec![Raise { name: "Menagerie boost 17".into(), amount: "+1 each".into(), owned: menagerie_boosts(save, 17) > 0.0 }];
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
    for world in rows("DeathNoteMobs", &[]) {
        for id in world.as_array().into_iter().flatten().filter_map(Value::as_str) {
            let kills = targets.iter().position(|t| t.as_str() == Some(id)).map_or(0.0, |map| {
                let req = details.get(map).and_then(|d| d.get(0)).and_then(|d| d.get(0)).map(num).unwrap_or(0.0);
                (0..chars)
                    .filter_map(|c| save.at(&format!("KLA_{c}"), &[map, 0]).map(num))
                    .map(|left| (req - left).max(0.0))
                    .sum()
            });
            let name = GAME.monster_name(id).unwrap_or(id);
            items.push(Item::new(name, face(id), skull(kills, false, eclipse), 20.0));
        }
    }
    let mini_kills = save.numbers("Ninja", &[105]);
    for (i, id) in rows("NinjaInfo", &[30]).iter().filter_map(|v| v.as_str()).enumerate() {
        let name = format!("{} (miniboss)", GAME.monster_name(id).unwrap_or(id));
        items.push(Item::new(name, face(id), skull(at(&mini_kills, i), true, false), 10.0));
    }
    let mut m = mech("death-note", 3, "Death Note", face("mushG"), Count::Levels, "skull points", items);
    m.note = Some("Skull points per monster: 20 is an Eclipse Skull, a billion kills once rift 20 unlocks them. Minibosses top out at 10.".into());
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

/// Equinox dreams: `WeeklyBoss["d_i"]` is -1 once dream i is done; `DreamChallenge` lists them.
fn equinox_dreams(save: &Save) -> Mechanic {
    let done = |i: usize| save.get("WeeklyBoss").and_then(|w| w.get(format!("d_{i}"))).map(num) == Some(-1.0);
    let items = rows("DreamChallenge", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| Item::new(challenge(text(r, 0)), None, f64::from(u8::from(done(i))), 1.0))
        .collect();
    mech("equinox-dreams", 3, "Equinox Dreams", pic("Dream_Cloud".into()), Count::Done, "done", items)
}

/// Max levels each dream adds to an Equinox upgrade, from its reward text
/// ("+5_Max_LV_for_'Matching_Scims'_Equinox_Upgrade"), by upgrade name.
fn dream_raises() -> HashMap<&'static str, f64> {
    let mut out = HashMap::new();
    for r in rows("DreamChallenge", &[]) {
        let reward = text(r, 2);
        let Some(rest) = reward.strip_prefix('+') else { continue };
        if !reward.contains("_Max_LV_for_") {
            continue;
        }
        let n: f64 = rest.split('_').next().and_then(|d| d.parse().ok()).unwrap_or(0.0);
        if let Some(name) = reward.split('\'').nth(1) {
            *out.entry(name).or_insert(0.0) += n;
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
    let summon = summoning_equinox(save);
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
            Item::new(label(text(r, 0)), None, at(&levels, i + 2), max)
        })
        .collect();
    let mut m = mech("equinox-upgrades", 3, "Equinox Upgrades", pic("Dream_Cloud".into()), Count::Levels, "levels", items);
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
        items.push(Item::new(label(text(r, 0)), pic("Arti21".into()), cell(r, 3) * tier, cell(r, 3) * 7.0));
    }
    if let Some(r) = rows("AtomInfo", &[]).get(7) {
        let name = text(r, 0).split("_-_").next().unwrap_or("");
        let have = if at(&save.numbers("Atoms", &[]), 7) >= 1.0 { 10.0 } else { 0.0 };
        items.push(Item::new(format!("{} atom", label(name)), pic("Atom7".into()), have, 10.0));
    }
    if let Some(r) = rows("RegAchieves", &[]).get(145) {
        let have = if at(&save.numbers("AchieveReg", &[]), 145) == -1.0 { 5.0 } else { 0.0 };
        items.push(Item::new(format!("{} achievement", label(text(r, 0))), None, have, 5.0));
    }
    if let Some(r) = rows("SaltLicks", &[]).get(4) {
        let lv = at(&save.numbers("SaltLick", &[]), 4);
        items.push(Item::new("Salt Lick", pic(text(r, 0).to_string()), lv * cell(r, 3), cell(r, 4) * cell(r, 3)));
    }
    if let Some(r) = rows("TaskShopDesc", &[2]).get(2) {
        let lv = at(&save.numbers("TaskZZ2", &[2]), 2);
        items.push(Item::new("World 3 merit", pic("TaskSc3".into()), lv * cell(r, 11), cell(r, 5) * cell(r, 11)));
    }
    let mut m = mech("library", 3, "Talent Book Library", pic("ConTower1".into()), Count::Levels, "book levels", items);
    m.note = Some("How high the library prints talent books, on top of its base of 125. Summoning wins add more that isn't counted here.".into());
    m
}

/// Refinery: `Refinery[3 + i][1]` is salt i's rank; salts from `Refinery[0][0]` on aren't unlocked.
/// Ranks have no max.
fn refinery(save: &Save) -> Mechanic {
    let unlocked = save.at("Refinery", &[0, 0]).map(num).unwrap_or(0.0);
    let items = rows("RefineryInfo", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let id = text(r, 12);
            let rank = if (i as f64) < unlocked { save.at("Refinery", &[3 + i, 1]).map(num).unwrap_or(0.0) } else { 0.0 };
            Item::new(GAME.item_name(id).unwrap_or(id), pic(id.to_string()), rank, 0.0)
        })
        .collect();
    let mut m = mech("refinery", 3, "Refinery", pic("Refinery1".into()), Count::Open, "ranks", items);
    m.note = Some("Refinery ranks have no cap.".into());
    m
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
    let names = rows("RANDOlist", &[10]);
    let items = rows("WorshipBASEinfos", &[])
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let name = names.get(i).and_then(|v| v.as_str()).map_or_else(|| format!("Tower Defence {}", i + 1), label);
            Item::new(name, None, at(&waves, i), 0.0)
        })
        .collect();
    let mut m = mech("worship", 3, "Worship Totems", pic("WorshipSkull1".into()), Count::Open, "best wave", items);
    m.note = Some("Your best wave at each totem. Waves have no cap.".into());
    m
}

/// Armor sets: `EquipmentSets` (read by extra_w3.py) names every set; `OptLacc[379]` lists the
/// finished ones by name, comma-separated.
fn armor_sets(save: &Save) -> Mechanic {
    let owned: Vec<&str> = save.at("OptLacc", &[379]).and_then(Value::as_str).map(|s| s.split(',').collect()).unwrap_or_default();
    let items = GAME
        .extra("w3")
        .get("sets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|s| Some((s.get(0)?.as_str()?, s.get(1)?.as_str()?)))
        .map(|(name, hat)| Item::new(label(name), pic(hat.to_string()), f64::from(u8::from(owned.contains(&name))), 1.0))
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
