//! World 4: Hyperion Nebula.

use bot_core::serde_json::Value;

use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![
        meals(save),
        kitchens(save),
        jewels(save),
        chips(save),
        pets(save),
        shiny(save),
        breeding_upgrades(save),
        territories(save),
        rift(save),
        rift_bonuses(save),
        cape_vine(save),
    ]
}

fn menagerie_unlock(save: &Save, i: usize) -> bool {
    save.at("Menagerie", &[2, i]).map(num).unwrap_or(0.0) >= 1.0
}

/// A Jade Emporium upgrade is bought when `Ninja[102][9]` holds its letter (N.js
/// `EmporiumBonus`: `Number2Letter[i]`, "_" for 0, then a to z, then A to Z).
fn emporium(save: &Save, i: usize) -> bool {
    let letters: Vec<char> = "_abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ".chars().collect();
    let bought = save.at("Ninja", &[102, 9]).and_then(Value::as_str).unwrap_or("");
    letters.get(i).is_some_and(|c| bought.contains(*c))
}

/// The meal cap with every raise in (N.js `CookingMealMaxLVlol`).
const MEAL_MAX: f64 = 180.0;

/// The account's meal cap now, and the raises it's missing. N.js `CookingMealMaxLVlol`: 30 +
/// Causticolumn's artifact bonus (10 per tier, 70 at Verisimilitude) + 10 for each of Jade
/// Emporium 20 and 21 + 30 for Spelunking lore bonus 5 + Grimoire upgrade 26 (up to 20) + 10
/// for Menagerie unlock 20.
fn meal_cap(save: &Save) -> (f64, Vec<String>) {
    let mut cap = 30.0;
    let mut missing = Vec::new();
    let art_base = rows("ArtifactInfo", &[]).get(17).map_or(10.0, |r| cell(r, 3));
    let tier = save.at("Sailing", &[3, 17]).map(num).unwrap_or(0.0).clamp(0.0, 7.0);
    cap += art_base * tier;
    if tier < 7.0 {
        missing.push(format!("Causticolumn at Verisimilitude (+{})", art_base * (7.0 - tier)));
    }
    for (i, name) in [(20, "Papa Blob's Quality Guarantee"), (21, "Chef Geustloaf's Cutting Edge Philosophy")] {
        if emporium(save, i) {
            cap += 10.0;
        } else {
            missing.push(format!("{name} in the Jade Emporium (+10)"));
        }
    }
    if save.at("Spelunk", &[0, 5]).map(num).unwrap_or(0.0) >= 1.0 {
        cap += 30.0;
    } else {
        missing.push("the Lunarheim lore bonus in Spelunking (+30)".into());
    }
    let per = rows("GrimoireUpg", &[]).get(26).map_or(1.0, |r| cell(r, 5));
    let grimoire = (save.at("Grimoire", &[26]).map(num).unwrap_or(0.0) * per).min(20.0);
    cap += grimoire;
    if grimoire < 20.0 {
        missing.push(format!("Supreme Head Chef Status in the Grimoire (+{})", 20.0 - grimoire));
    }
    if menagerie_unlock(save, 20) {
        cap += 10.0;
    } else {
        missing.push("Menagerie unlock 21 (+10)".into());
    }
    (cap, missing)
}

/// Meals: `Meals[0][i]`; one cap for every meal, fully raised.
fn meals(save: &Save) -> Mechanic {
    let levels = save.numbers("Meals", &[0]);
    let items = rows("MealINFO", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| Item::new(label(text(r, 0)), pic(format!("CookingM{i}")), at(&levels, i), MEAL_MAX))
        .collect();
    let mut m = mech("meals", 4, "Meals", pic("CookingM0".into()), Count::Levels, "levels", items);
    let (cap, missing) = meal_cap(save);
    let mut note = format!(
        "Meals cap at {MEAL_MAX:.0} with every raise: Causticolumn, two Jade Emporium upgrades, a Spelunking lore bonus, the Grimoire and the Menagerie. Your cap is {cap:.0} now."
    );
    if !missing.is_empty() {
        note.push_str(&format!(" Still missing: {}.", missing.join(", ")));
    }
    m.note = Some(note);
    m
}

/// Kitchens: `Cooking[i][0]` is 2 once bought. The game prices kitchens 2 to 10 (N.js
/// `CookingNewKitchenCoinCost`); kitchen upgrade levels have no cap.
fn kitchens(save: &Save) -> Mechanic {
    let items = (0..10)
        .map(|i| Item::new(format!("Kitchen {}", i + 1), None, f64::from(save.at("Cooking", &[i, 0]).map(num).unwrap_or(0.0) >= 2.0), 1.0))
        .collect();
    let mut m = mech("kitchens", 4, "Kitchens", None, Count::Done, "bought", items);
    m.note = Some("Kitchen upgrades level up without end, so only the kitchens count.".into());
    m
}

/// Lab jewels: `Lab[14][i]` is 1 once owned.
fn jewels(save: &Save) -> Mechanic {
    let owned = save.numbers("Lab", &[14]);
    let items = rows("JewelDesc", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| Item::new(label(text(r, 11)), pic(format!("ConsoleJwl{i}")), f64::from(at(&owned, i) >= 1.0), 1.0))
        .collect();
    mech("jewels", 4, "Lab Jewels", pic("ConsoleJwl0".into()), Count::Done, "owned", items)
}

/// Lab chips: `Lab[15][i]` is how many you hold (-1 never found); chips a character has
/// slotted are in `Lab[1 + character]`.
fn chips(save: &Save) -> Mechanic {
    let held = save.numbers("Lab", &[15]);
    let slotted: Vec<f64> = (0..save.characters()).flat_map(|c| save.numbers("Lab", &[1 + c])).collect();
    let items = rows("ChipDesc", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let owned = at(&held, i) >= 1.0 || slotted.contains(&(i as f64));
            Item::new(label(text(r, 0)), pic(format!("ConsoleChip{i}")), f64::from(owned), 1.0)
        })
        .collect();
    mech("lab-chips", 4, "Lab Chips", pic("ConsoleChip0".into()), Count::Done, "owned", items)
}

/// Breedable pets by world: `PetStats[w]` rows (col 0 the monster); placeholder worlds start
/// with "_" or have no species column.
fn breedable() -> Vec<(usize, usize, &'static Value)> {
    let mut out = Vec::new();
    for (w, world) in rows("PetStats", &[]).iter().enumerate() {
        let pets = world.as_array().map(Vec::as_slice).unwrap_or(&[]);
        let real = pets.first().is_some_and(|p| text(p, 0) != "_" && p.as_array().is_some_and(|a| a.len() > 5));
        if real {
            out.extend(pets.iter().enumerate().map(|(j, p)| (w, j, p)));
        }
    }
    out
}

fn pet_name(row: &Value) -> String {
    let id = text(row, 0);
    GAME.monster_name(id).map_or_else(|| label(id), str::to_string)
}

/// Pets bred: `Breeding[1][w]` counts the species found in world w, in `PetStats` order.
fn pets(save: &Save) -> Mechanic {
    let found = save.numbers("Breeding", &[1]);
    let items = breedable().into_iter().map(|(w, j, r)| Item::new(pet_name(r), None, f64::from(at(&found, w) > j as f64), 1.0)).collect();
    mech("breeding-pets", 4, "Breeding Pets", pic("PetUpg0".into()), Count::Done, "found", items)
}

/// Shiny levels: `Breeding[22 + w][j]` is the pet's shiny days. N.js `SpecialPassives` "Lv":
/// level k+1 once the days pass floor((1 + k^1.6) * 1.7^k), up to 20, or 30 with Menagerie
/// unlock 12. A pet never shiny reads 0.
fn shiny(save: &Save) -> Mechanic {
    let top = if menagerie_unlock(save, 11) { 29 } else { 19 };
    let level = |days: f64| -> f64 {
        if days <= 0.0 {
            return 0.0;
        }
        let mut lv = 1;
        for r in 0..top {
            let k = f64::from(r + 1);
            if days > ((1.0 + k.powf(1.6)) * 1.7f64.powf(k)).floor() {
                lv = r + 2;
            }
        }
        f64::from(lv)
    };
    let items = breedable()
        .into_iter()
        .map(|(w, j, r)| Item::new(pet_name(r), None, level(at(&save.numbers("Breeding", &[22 + w]), j)), 30.0))
        .collect();
    let mut m = mech("shiny", 4, "Shiny Pets", pic("PetUpg8".into()), Count::Levels, "levels", items);
    m.note = Some("Shiny levels stop at 20. Menagerie unlock 12 raises that to 30.".into());
    m
}

/// Breeding upgrades: `Breeding[2][i]`; `PetUpgradeINFO` col 8 is the max (N.js `PetUpgMaxLV`).
/// Blooming Axe also gets Minehead bonus 3 (`Research` table [20][3]) on top. Row 0 is the
/// empty slot.
fn breeding_upgrades(save: &Save) -> Mechanic {
    let levels = save.numbers("Breeding", &[2]);
    let minehead = rows("Research", &[20]).get(3).map_or(0.0, |v| num(v));
    let items = rows("PetUpgradeINFO", &[])
        .iter()
        .enumerate()
        .skip(1)
        .map(|(i, r)| {
            let name = text(r, 0).split("_}").next().unwrap_or("");
            let max = cell(r, 8) + if i == 6 { minehead } else { 0.0 };
            Item::new(label(name), pic(format!("PetUpg{}", i - 1)), at(&levels, i), max)
        })
        .collect();
    let mut m = mech("breeding-upgrades", 4, "Breeding Upgrades", pic("PetUpg0".into()), Count::Levels, "levels", items);
    m.note = Some(format!("Blooming Axe's cap includes the +{minehead} a Minehead bonus adds."));
    m
}

/// Territories: `OptLacc[85]` counts the ones beaten. Territory i is `TerritoryInfo` row i,
/// or i + 1 from 14 on (row 14 is the arena fight, N.js `TerritoryID`); filler rows end it.
fn territories(save: &Save) -> Mechanic {
    let beaten = save.option(85);
    let items = rows("TerritoryInfo", &[])
        .iter()
        .enumerate()
        .filter(|(row, r)| *row != 14 && !text(r, 4).starts_with("Filler"))
        .enumerate()
        .map(|(i, (_, r))| Item::new(label(text(r, 4)), pic(format!("PetTerritory{}", i + 2)), f64::from((i as f64) < beaten), 1.0))
        .collect();
    mech("territories", 4, "Breeding Territories", pic("PetTerritory2".into()), Count::Done, "beaten", items)
}

/// The last rift: the first one whose task (`RiftStuff[4][rift]`, text in `RiftStuff[0]`) is
/// "you cannot progress further".
fn last_rift() -> f64 {
    let tasks = rows("RiftStuff", &[0]);
    let end = tasks.iter().position(|t| t.as_str().is_some_and(|s| s.starts_with("YOU_CANNOT_PROGRESS")));
    let per_rift = rows("RiftStuff", &[4]);
    end.and_then(|e| per_rift.iter().position(|v| num(v) as usize == e)).map_or(0.0, |i| i as f64)
}

/// Rift: `Rift[0]` is the rift you're in.
fn rift(save: &Save) -> Mechanic {
    let level = save.at("Rift", &[0]).map(num).unwrap_or(0.0);
    let items = vec![Item::new("Rift reached", None, level, last_rift())];
    mech("rift", 4, "Rift", None, Count::Levels, "rifts", items)
}

/// Rift bonuses: `RiftStuff[1]` in fives (x, y, name, ..); bonus t is on from rift 5(t+1)
/// (N.js `riftBonus`). The last entry, Rift Guy, isn't a bonus.
fn rift_bonuses(save: &Save) -> Mechanic {
    let level = save.at("Rift", &[0]).map(num).unwrap_or(0.0);
    let list = rows("RiftStuff", &[1]);
    let items = list
        .chunks(5)
        .enumerate()
        .filter_map(|(t, c)| {
            let name = c.get(2)?.as_str()?;
            (name != "Rift_Guy").then(|| Item::new(label(name), None, f64::from(level >= 5.0 * (t as f64 + 1.0)), 1.0))
        })
        .collect();
    mech("rift-bonuses", 4, "Rift Bonuses", None, Count::Done, "unlocked", items)
}

/// Cape Vine: `Spelunk[54]` lists the capes deposited; every cape counts but Lava Wings (see
/// extra_w4.py).
fn cape_vine(save: &Save) -> Mechanic {
    let deposited: Vec<&str> = save.at("Spelunk", &[54]).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let capes = GAME.extra("w4").get("capes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    let items = capes
        .iter()
        .map(|c| {
            let id = text(c, 0);
            Item::new(label(text(c, 1)), pic(id.to_string()), f64::from(deposited.contains(&id)), 1.0)
        })
        .collect();
    mech("cape-vine", 4, "Cape Vine", pic("EquipmentCape0".into()), Count::Done, "deposited", items)
}
