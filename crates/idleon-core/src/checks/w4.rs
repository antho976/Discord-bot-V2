//! World 4: Hyperion Nebula.

use serde_json::Value;

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

use super::w7::{menagerie_unlock, menagerie_unlock_name};

/// The first letter in capitals.
fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
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

/// The account's meal cap now, and every raise with whether it's in. N.js `CookingMealMaxLVlol`:
/// 30 + Causticolumn's artifact bonus (10 per tier, 70 at Verisimilitude) + 10 for each of Jade
/// Emporium 20 and 21 + 30 for Spelunking lore bonus 5 + Grimoire upgrade 26 (up to 20) + 10
/// for Menagerie unlock 20.
fn meal_cap(save: &Save) -> (f64, Vec<Raise>) {
    let mut cap = 30.0;
    let mut raises = Vec::new();
    let art_base = rows("ArtifactInfo", &[]).get(17).map_or(10.0, |r| cell(r, 3));
    let tier = save.at("Sailing", &[3, 17]).map(num).unwrap_or(0.0).clamp(0.0, 7.0);
    cap += art_base * tier;
    raises.push(Raise { name: "Causticolumn artifact".into(), amount: format!("+{art_base} max levels on every meal per artifact tier, +{} at Verisimilitude", art_base * 7.0), owned: tier >= 7.0 });
    for (i, name) in [(20, "Papa Blob's Quality Guarantee (Jade Emporium)"), (21, "Chef Geustloaf's Cutting Edge Philosophy (Jade Emporium)")] {
        let owned = emporium(save, i);
        cap += if owned { 10.0 } else { 0.0 };
        raises.push(Raise { name: name.into(), amount: "+10 max levels on every meal".into(), owned });
    }
    let lore = save.at("Spelunk", &[0, 5]).map(num).unwrap_or(0.0) >= 1.0;
    cap += if lore { 30.0 } else { 0.0 };
    raises.push(Raise { name: "Lunarheim lore bonus (Spelunking)".into(), amount: "+30 max levels on every meal".into(), owned: lore });
    let per = rows("GrimoireUpg", &[]).get(26).map_or(1.0, |r| cell(r, 5));
    let grimoire = (save.at("Grimoire", &[26]).map(num).unwrap_or(0.0) * per).clamp(0.0, 20.0);
    cap += grimoire;
    raises.push(Raise { name: "Supreme Head Chef Status (Grimoire)".into(), amount: "+1 max level on every meal per upgrade level, up to +20".into(), owned: grimoire >= 20.0 });
    let men = menagerie_unlock(save, 20);
    cap += if men { 10.0 } else { 0.0 };
    raises.push(Raise { name: capital(&menagerie_unlock_name(20)), amount: "+10 max levels on every meal".into(), owned: men });
    (cap, raises)
}

/// Meals a meal's next level costs from `level`, before the player's discounts (N.js
/// `CookingMenuMealCosts`): (10 + L + L^2) x (1.2 + 0.05L)^L x (1 + 0.4k)^L x 10^(22k), where
/// k = floor((L + 1000) / 1111). The discounts (a companion, achievement 233, an Equinox cloud)
/// only lower it, so holding this much is always enough.
fn meal_cost(level: f64) -> f64 {
    let k = ((level + 1000.0) / 1111.0).floor();
    (10.0 + level + level * level) * (1.2 + 0.05 * level).powf(level) * (1.0 + 0.4 * k).powf(level) * 10f64.powf(22.0 * k)
}

/// Meals: `Meals[0][i]` is the level, `Meals[2][i]` the meals held; one cap for every meal,
/// fully raised.
fn meals(save: &Save) -> Mechanic {
    let levels = save.numbers("Meals", &[0]);
    let held = save.numbers("Meals", &[2]);
    let (cap, raises) = meal_cap(save);
    let items = rows("MealINFO", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let level = at(&levels, i);
            let mut it = Item::new(label(text(r, 0)), pic(format!("CookingM{i}")), level, MEAL_MAX);
            if level <= 0.0 {
                it.how = Some("Found by cooking new recipes in a kitchen.".into());
            } else if level >= cap && level < MEAL_MAX {
                it.how = Some(format!("At your meal cap of {cap}. The raises listed below lift it."));
                it.state = State::Locked;
            } else if level < MEAL_MAX {
                let cost = meal_cost(level);
                it.how = Some(format!("Next level costs {} of this meal before your discounts.", big(cost)));
                if at(&held, i) >= cost {
                    it.state = State::Ready;
                }
            }
            it
        })
        .collect();
    let mut m = mech("meals", 4, "Meals", pic("CookingM0".into()), Count::Levels, "levels", items);
    m.note = Some(format!("Meals cap at {MEAL_MAX:.0} with every raise in. Your cap is {cap} now."));
    m.raises = raises;
    m
}

/// A cost item from the lab's tables: an item ID, a meal ("Meal3") or a spice ("Spice2", from
/// breeding territory 3).
fn cost_name(id: &str) -> String {
    if let Some(n) = id.strip_prefix("Meal").and_then(|n| n.parse::<usize>().ok()) {
        return rows("MealINFO", &[]).get(n).map_or_else(|| id.to_string(), |r| label(text(r, 0)));
    }
    if let Some(n) = id.strip_prefix("Spice").and_then(|n| n.parse::<usize>().ok()) {
        return format!("spice from breeding territory {}", n + 1);
    }
    GAME.item_name(id).map_or_else(|| label(id), str::to_string)
}

/// "Costs 5 Strung Jewels, 2K Egg, 200 territory 1 spice": (item, quantity) pairs from `from`.
fn costs(row: &Value, from: usize) -> String {
    let parts: Vec<String> = (0..3).map(|k| (text(row, from + 2 * k), cell(row, from + 2 * k + 1))).filter(|(id, _)| !id.is_empty()).map(|(id, q)| {
            let name = cost_name(id);
            // "10x 1 HR Time Candy", not "10 1 HR Time Candy".
            let x = if name.starts_with(|c: char| c.is_ascii_digit()) { "x" } else { "" };
            format!("{}{x} {name}", big(q))
        })
        .collect();
    format!("Costs {}.", parts.join(", "))
}

/// Kitchens: `Cooking[i][0]` is 1 or more once bought. The game prices kitchens 2 to 10 (N.js
/// `CookingNewKitchenCoinCost`); kitchen upgrade levels have no cap.
fn kitchens(save: &Save) -> Mechanic {
    // N.js `CookingNewKitchenCoinCost`, by kitchen index.
    const COINS: [f64; 10] = [0.0, 82e5, 75e6, 4e8, 8e9, 5e10, 213e10, 6e13, 2e15, 1e17];
    let items = (0..10)
        .map(|i| {
            // 0 is not bought; buying sets 1 (idle), cooking 2, a new recipe 3, and stopping
            // goes back to 1 (N.js, the kitchen's click handler).
            let owned = save.at("Cooking", &[i, 0]).map(num).unwrap_or(0.0) >= 1.0;
            let mut it = Item::new(format!("Kitchen {}", i + 1), None, f64::from(owned), 1.0);
            if !owned {
                it.how = Some(if i == 0 { "Comes with the Cooking skill.".into() } else { format!("Costs {} coins at the kitchen sign.", big(COINS[i])) });
            }
            it
        })
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
        .map(|(i, r)| {
            let mut it = Item::new(label(text(r, 11)), pic(format!("ConsoleJwl{i}")), f64::from(at(&owned, i) >= 1.0), 1.0);
            if !it.done() {
                it.how = Some(costs(r, 5));
            }
            // Jewels paid for with World 6 drops ("SpiA1" is Leafy Droplet, "SpiB2b" Dark Lantern,
            // with World 6 territory spices) are World 6 content; a World 6 account can own them.
            if text(r, 5).starts_with("Spi") {
                it.world = Some(6);
            }
            it
        })
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
            let mut it = Item::new(label(text(r, 0)), pic(format!("ConsoleChip{i}")), f64::from(owned), 1.0);
            if !owned {
                it.how = Some(costs(r, 3));
            }
            it
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
    let items = breedable()
        .into_iter()
        .map(|(w, j, r)| {
            let mut it = Item::new(pet_name(r), None, f64::from(at(&found, w) > j as f64), 1.0);
            if !it.done() {
                it.how = Some(if at(&found, w) == j as f64 {
                    format!("The next new mob from World {} eggs.", w + 1)
                } else {
                    format!("Comes after the World {} mobs before it.", w + 1)
                });
                if at(&found, w) < j as f64 {
                    it.state = State::Locked;
                }
            }
            it
        })
        .collect();
    mech("breeding-pets", 4, "Breeding Pets", pic("PetUpg0".into()), Count::Done, "found", items)
}

/// Shiny days a pet needs past level `lv` (N.js `SpecialPassives` "Lv"/"REQ"):
/// floor((1 + lv^1.6) x 1.7^lv).
fn shiny_req(lv: f64) -> f64 {
    ((1.0 + lv.powf(1.6)) * 1.7f64.powf(lv)).floor()
}

/// Shiny levels: `Breeding[22 + w][j]` is the pet's shiny days. N.js `SpecialPassives` "Lv":
/// level k+1 once the days pass `shiny_req(k)`, up to 20, or 30 with Menagerie unlock 12. A pet
/// never shiny reads 0.
fn shiny(save: &Save) -> Mechanic {
    let unlocked = menagerie_unlock(save, 11);
    let cap = if unlocked { 30.0 } else { 20.0 };
    let level = |days: f64| -> f64 {
        if days <= 0.0 {
            return 0.0;
        }
        let mut lv = 1.0;
        while lv < cap && days > shiny_req(lv) {
            lv += 1.0;
        }
        lv
    };
    let items = breedable()
        .into_iter()
        .map(|(w, j, r)| {
            let days = at(&save.numbers("Breeding", &[22 + w]), j);
            let lv = level(days);
            let mut it = Item::new(pet_name(r), None, lv, 30.0);
            if lv <= 0.0 {
                it.how = Some("Breed a shiny one first.".into());
            } else if lv >= cap && lv < 30.0 {
                it.how = Some(format!("At the shiny cap of 20 until {}.", menagerie_unlock_name(11)));
                it.state = State::Locked;
            } else if lv < 30.0 {
                it.how = Some(format!("Next level past {} shiny days; it has {}.", big(shiny_req(lv)), big(days.floor())));
            }
            it
        })
        .collect();
    let mut m = mech("shiny", 4, "Shiny Pets", pic("PetUpg8".into()), Count::Levels, "levels", items);
    m.note = Some(format!("Shiny levels stop at 20; {} raises that to 30 for every pet. Shiny days add up while the pet is in the Fenceyard.", menagerie_unlock_name(11)));
    m.raises = vec![Raise { name: capital(&menagerie_unlock_name(11)), amount: "+10 max shiny levels on every pet".into(), owned: unlocked }];
    m
}

/// A breeding upgrade's next-level cost at `level` (N.js `PetUpgCostREAL`): for s = 0 (dead
/// cells, `Breeding[3][8]`) and s = 1 (meals of `PetUpgradeINFO` col 3), col 4+2s x (1 + lv) x
/// col 5+2s ^ lv. No discounts apply.
fn upgrade_cost(row: &Value, level: f64, s: usize) -> f64 {
    cell(row, 4 + 2 * s) * (1.0 + level) * cell(row, 5 + 2 * s).powf(level)
}

/// Breeding upgrades: `Breeding[2][i]`; `PetUpgradeINFO` col 8 is the max (N.js `PetUpgMaxLV`).
/// Blooming Axe also gets Minehead bonus 3 (`Research` table [20][3], once the save's
/// `Research[7][4]` passes 3) on top. Row 0 is the empty slot.
fn breeding_upgrades(save: &Save) -> Mechanic {
    let levels = save.numbers("Breeding", &[2]);
    let cells = save.at("Breeding", &[3, 8]).map(num).unwrap_or(0.0).max(0.0);
    let meals = save.numbers("Meals", &[2]);
    let minehead = rows("Research", &[20]).get(3).map_or(0.0, |v| num(v));
    let mut ready = Vec::new();
    let items: Vec<Item> = rows("PetUpgradeINFO", &[])
        .iter()
        .enumerate()
        .skip(1)
        .map(|(i, r)| {
            let name = label(text(r, 0).split("_}").next().unwrap_or(""));
            let max = cell(r, 8) + if i == 6 { minehead } else { 0.0 };
            let level = at(&levels, i);
            let mut it = Item::new(name.clone(), pic(format!("PetUpg{}", i - 1)), level, max);
            if level < max {
                let c0 = upgrade_cost(r, level, 0);
                let meal = text(r, 3).parse::<usize>().ok();
                let (c1, meal_name) = meal.map_or((0.0, String::new()), |n| (upgrade_cost(r, level, 1), cost_name(&format!("Meal{n}"))));
                it.how = Some(if meal.is_some() {
                    format!("Next level costs {} dead cells and {} {meal_name}.", big(c0), big(c1))
                } else {
                    format!("Next level costs {} dead cells.", big(c0))
                });
                // Upgrades open with Breeding levels; only one already started is known to be open.
                if level >= 1.0 && cells >= c0 && meal.is_none_or(|n| at(&meals, n) >= c1) {
                    it.state = State::Ready;
                    ready.push(name);
                }
            }
            it
        })
        .collect();
    let mut m = mech("breeding-upgrades", 4, "Breeding Upgrades", pic("PetUpg0".into()), Count::Levels, "levels", items);
    m.note = Some(format!("Blooming Axe's cap includes +{minehead} max levels from a Minehead reward (beating 4 Minehead opponents)."));
    m.raises = vec![Raise {
        name: "Minehead reward: beat 4 opponents in Mr. Minehead's Depth Charge".into(),
        amount: format!("+{minehead} max levels on Blooming Axe"),
        owned: save.at("Research", &[7, 4]).map(num).unwrap_or(0.0) > 3.0,
    }];
    m.have = Some(Have {
        figure: format!("{} dead cells", big(cells)),
        text: match ready.as_slice() {
            [] => "With your dead cells and meals, no upgrade's next level is paid for yet.".into(),
            [one] => format!("With your dead cells and meals you can pay for the next level of {one}."),
            more => format!("With your dead cells and meals you can pay for the next level of any one of: {}.", more.join(", ")),
        },
    });
    m
}

/// Breeding territories per world, in order (each is a world's area, as its picture
/// `PetTerritory{i + 2}` names it: Grasslands to Stinky Sewers in World 1, Desert Oasis to The
/// Crypt in World 2, Frosty Peaks to Pristalle Lake in World 3, Nebulon Mantle to Shores of
/// Eternity in World 4, Molten Bay to Wurm Catacombs in World 5, Spirit Fields to Dharma Mesa in
/// World 6, Shallow Shoals and Murky Trenches in World 7).
const TERRITORIES_PER_WORLD: [usize; 7] = [5, 5, 4, 3, 3, 4, 2];

fn territory_world(i: usize) -> u8 {
    let mut end = 0;
    for (w, n) in TERRITORIES_PER_WORLD.iter().enumerate() {
        end += n;
        if i < end {
            return w as u8 + 1;
        }
    }
    7
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
        .map(|(i, (_, r))| {
            // `PetTerritory{i + 2}` is the whole territory panel, with no icon in it: none reads small.
            let mut it = Item::new(label(text(r, 4)), None, f64::from((i as f64) < beaten), 1.0);
            let world = territory_world(i);
            if world > 4 {
                it.world = Some(world);
            }
            if !it.done() {
                if i as f64 == beaten.floor() {
                    it.how = Some("Next: beat them with your Fenceyard mobs to open the territory.".into());
                } else {
                    it.how = Some("Opens once the territory before it is beaten.".into());
                    it.state = State::Locked;
                }
            }
            it
        })
        .collect();
    mech("territories", 4, "Breeding Territories", None, Count::Done, "beaten", items)
}

/// The last rift: the first one whose task (`RiftStuff[4][rift]`, text in `RiftStuff[0]`) is
/// "you cannot progress further".
fn last_rift() -> f64 {
    let tasks = rows("RiftStuff", &[0]);
    let end = tasks.iter().position(|t| t.as_str().is_some_and(|s| s.starts_with("YOU_CANNOT_PROGRESS")));
    let per_rift = rows("RiftStuff", &[4]);
    end.and_then(|e| per_rift.iter().position(|v| num(v) as usize == e)).map_or(0.0, |i| i as f64)
}

/// A rift task in sentence case, up to its first sentence: "Defeat 500 monsters within 5 min".
fn rift_task(s: &str) -> String {
    let first = s.split("._").next().unwrap_or(s).trim_end_matches('.');
    let lower = words(first).to_lowercase();
    let mut c = lower.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Rift: `Rift[0]` counts the rifts done (the game greets you in the next, "WELCOME_TO_RIFT_" +
/// (Rift[0] + 1)), `Rift[1]` is the progress on that rift's task. The task is
/// `RiftStuff[4][Rift[0] % 100]` (N.js `RiftStuff` "Task"), its goal `RiftStuff[5][task]`, done
/// once progress reaches it (`RiftCompleted`). The rift opens with the Rift Ripper's quest
/// (`QuestComplete_c.Rift_Ripper1` is 1), before any rift is done.
fn rift(save: &Save) -> Mechanic {
    let done = save.at("Rift", &[0]).map(num).unwrap_or(0.0).max(0.0);
    let open = done > 0.0 || (0..save.characters().clamp(1, 12)).any(|c| save.at(&format!("QuestComplete_{c}"), &[]).and_then(|q| q.get("Rift_Ripper1")).map(num).unwrap_or(0.0) >= 1.0);
    let last = last_rift();
    let mut it = Item::new("Rifts done", None, done, last);
    if open && done < last {
        let task = rows("RiftStuff", &[4]).get((done.min(1e6) as usize) % 100).map_or(0, |v| num(v) as usize);
        let goal = rows("RiftStuff", &[5]).get(task).map_or(0.0, |v| num(v));
        let progress = save.at("Rift", &[1]).map(num).unwrap_or(0.0);
        let text = rows("RiftStuff", &[0]).get(task).and_then(|v| v.as_str()).map(rift_task).unwrap_or_default();
        it.how = Some(format!("Rift {}: {text}, {} of {} so far.", done + 1.0, big(progress.floor()), big(goal)));
        if goal > 0.0 && progress >= goal {
            it.state = State::Ready;
        }
    } else if !open {
        it.how = Some("Opens with the Rift Ripper's quest in World 4.".into());
    }
    mech("rift", 4, "Rift", None, Count::Levels, "rifts", vec![it])
}

/// Rift bonuses: `RiftStuff[1]` in fives (x, y, name, ..); bonus t is on once 5(t+1) rifts are
/// done (N.js `riftBonus`: `Rift[0]` >= 5(t+1)). The last entry, Rift Guy, isn't a bonus.
fn rift_bonuses(save: &Save) -> Mechanic {
    let raw = save.at("Rift", &[0]).map(num).unwrap_or(0.0).max(0.0);
    let list = rows("RiftStuff", &[1]);
    let items = list
        .chunks(5)
        .enumerate()
        .filter_map(|(t, c)| {
            let name = c.get(2)?.as_str()?;
            let at_rift = 5.0 * (t as f64 + 1.0);
            let mut it = Item::new(label(name), None, f64::from(raw >= at_rift), 1.0);
            if !it.done() {
                it.how = Some(format!("Opens once rift {at_rift} is done; you've done {raw}."));
                it.state = State::Locked;
            }
            (name != "Rift_Guy").then_some(it)
        })
        .collect();
    mech("rift-bonuses", 4, "Rift Bonuses", None, Count::Done, "unlocked", items)
}

/// Cape Vine: `Spelunk[54]` lists the capes deposited; every cape counts but Lava Wings (see
/// extra_w4.py). When the vine opens, the game hangs every cape you've looted once (`Cards1`)
/// that isn't sitting in the inventory. Ready only on an account already using the vine, since
/// the save doesn't say whether it's unlocked.
fn cape_vine(save: &Save) -> Mechanic {
    let list = |key: &str, path: &[usize]| -> Vec<&str> { save.at(key, path).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default() };
    let deposited = list("Spelunk", &[54]);
    let looted = list("Cards1", &[]);
    let capes = GAME.extra("w4").get("capes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    let items = capes
        .iter()
        .map(|c| {
            let id = text(c, 0);
            let mut it = Item::new(label(text(c, 1)), pic(id.to_string()), f64::from(deposited.contains(&id)), 1.0);
            if !it.done() {
                if looted.contains(&id) && !deposited.is_empty() {
                    it.how = Some("You've looted one: open the vine with it out of your inventory.".into());
                    it.state = State::Ready;
                } else if looted.contains(&id) {
                    it.how = Some("You've looted one; it goes on the vine when you open the vine.".into());
                } else {
                    it.how = Some("Loot one; the vine takes it once it's in the Slab.".into());
                }
            }
            it
        })
        .collect();
    // World 7: the vine is Spelunking content (`Spelunk[54]`), so it is weighed against W7 progress.
    mech("cape-vine", 7, "Cape Vine", pic("EquipmentCape0".into()), Count::Done, "deposited", items)
}
