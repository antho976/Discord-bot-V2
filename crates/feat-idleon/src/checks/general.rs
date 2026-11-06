//! Account-wide: cards, achievements and the rest that belongs to no world.

use std::collections::{HashMap, HashSet};

use bot_core::serde_json::Value;

use super::*;
use super::w1::{char_name, characters, class_icon};

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![
        cards(save),
        achievements(save),
        gem_shop(save),
        event_shop(save),
        greenstacks(save),
        secret_path(save),
        class_levels(save),
        storage_chests(save),
        inventory_bags(save),
        carry_capacity(save),
    ]
}

/// The most stars a card can have: 4, then 5 with the Rift unlock and 6 with Spelunking's.
const CARD_STARS: usize = 6;

/// Stars for `count` cards of a card whose first star costs `base` (the game's `CardLv`).
pub(super) fn card_stars(count: f64, base: f64, chizoar: bool) -> f64 {
    if count <= 0.0 {
        return 0.0;
    }
    let mut level = 1;
    for s in 0..CARD_STARS {
        let f = s as f64;
        let step = if chizoar {
            1.5 * (f + 1.0 + (f / 3.0).floor()).powi(2)
        } else {
            base * (f + 1.0 + (f / 3.0).floor() + 16.0 * (f / 4.0).floor() + 100.0 * (f / 5.0).floor()).powi(2)
        };
        if count > step {
            level = s + 2;
        }
    }
    (level - 1) as f64
}

/// Cards: `Cards0` maps a monster to the cards you have of it.
fn cards(save: &Save) -> Mechanic {
    let have = save.get("Cards0");
    let mut items = Vec::new();
    for group in GAME.rows("CardStuff") {
        for r in group.as_array().into_iter().flatten() {
            let mob = text(r, 0);
            if mob == "Blank" || mob.is_empty() {
                continue;
            }
            let count = have.and_then(|h| h.get(mob)).map(num).unwrap_or(0.0);
            let stars = card_stars(count, cell(r, 2), mob == "Boss3B");
            let name = GAME.monster_name(mob).map(str::to_string).unwrap_or_else(|| words(mob));
            let mut it = Item::new(name, pic(format!("Cards{}", text(r, 1))), stars, CARD_STARS as f64);
            it.level_name = Some(match (count > 0.0, stars as u8) {
                (false, _) => "Not found".into(),
                (true, 0) => "Found".into(),
                (true, 1) => "1 star".into(),
                (true, n) => format!("{n} stars"),
            });
            items.push(it);
        }
    }
    let mut m = mech("cards", 0, "Cards", pic("CardsA0".into()), Count::Levels, "stars", items);
    m.tier_names = Some(["Found", "1 star", "2 stars", "3 stars", "4 stars", "5 stars", "6 stars"].iter().map(|s| s.to_string()).collect());
    m.note = Some("4 stars at first, 5 with the Rift unlock, 6 with Spelunking's.".into());
    m
}

/// Achievements: `AchieveReg[i]` is -1 once done. Only the first 420 are ever saved.
fn achievements(save: &Save) -> Mechanic {
    let reg = save.numbers("AchieveReg", &[]);
    let items = rows("RegAchieves", &[])
        .iter()
        .enumerate()
        .take(420)
        .filter(|(_, r)| text(r, 0) != "FILLERZZZ_ACH")
        .map(|(i, r)| Item::new(label(text(r, 0)), None, f64::from(at(&reg, i) == -1.0), 1.0))
        .collect();
    mech("achievements", 0, "Achievements", None, Count::Done, "done", items)
}

/// Gem shop: `MTXinfo[section][tab][slot]` is one offer: col 0 its picture, col 1 its name,
/// col 4 the `GemItemsPurchased` index and col 5 the most you can buy (the shop prints
/// "YOU HAVE BOUGHT: n/col5" when col 5 is under 50000). Left out: blank slots, offers with no
/// limit, stock the game restocks daily, weekly or each update (found by the extractor from
/// where the game zeroes the count), and the Complete Class Redo, which is a one-off reset.
fn gem_shop(save: &Save) -> Mechanic {
    let bought = save.numbers("GemItemsPurchased", &[]);
    let restocks: Vec<usize> = GAME.extra("general").get("gem_restocks").map(crate::save::numbers).unwrap_or_default().into_iter().map(|v| v as usize).collect();
    let mut items = Vec::new();
    for section in GAME.rows("MTXinfo") {
        for tab in section.as_array().into_iter().flatten() {
            for r in tab.as_array().into_iter().flatten() {
                let (code, name, index, max) = (text(r, 0), text(r, 1), cell(r, 4) as usize, cell(r, 5));
                if name == "NAME_OF_ITEM" || max <= 0.0 || max >= 50000.0 || restocks.contains(&index) || code == "ClassSwapC" {
                    continue;
                }
                let picture = (code != "Blank").then(|| code.to_string());
                items.push(Item::new(label(name), picture, at(&bought, index), max));
            }
        }
    }
    let mut m = mech("gem-shop", 0, "Gem Shop", pic("GemP1".into()), Count::Levels, "bought", items);
    m.note = Some("Permanent upgrades only: no restocking items, cosmetics or packs.".into());
    m
}

/// The letters the event shop writes into `OptLacc[311]`, one per item owned: the game's
/// `Number2Letter`, "_", a to z, A to Z, then the `TournyStuff` characters in order.
fn number_to_letter() -> Vec<String> {
    let mut v: Vec<String> = std::iter::once('_').chain('a'..='z').chain('A'..='Z').map(String::from).collect();
    for row in GAME.rows("TournyStuff").iter().take(13) {
        v.extend(row.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string));
    }
    v
}

/// Event shop: `NinjaInfo[39]` pairs "Name_@_what_it_does" and cost. Item i is owned when
/// its letter (see [`number_to_letter`]) is in `OptLacc[311]`.
fn event_shop(save: &Save) -> Mechanic {
    let owned = save.at("OptLacc", &[311]).and_then(Value::as_str).unwrap_or("");
    let letters = number_to_letter();
    let shop = rows("NinjaInfo", &[39]);
    let items = shop
        .chunks(2)
        .enumerate()
        .map(|(i, pair)| {
            let name = pair[0].as_str().unwrap_or("").split('@').next().unwrap_or("");
            let has = letters.get(i).is_some_and(|l| owned.contains(l.as_str()));
            Item::new(label(name), pic(format!("EventShopBuy{i}")), f64::from(has), 1.0)
        })
        .collect();
    mech("event-shop", 0, "Event Shop", pic("EventShopBuy0".into()), Count::Done, "bought", items)
}

/// "2.4M", "380K": a stack size as the storage chest shows it.
fn short(n: f64) -> String {
    match n {
        n if n >= 1e9 => format!("{:.1}B", n / 1e9),
        n if n >= 1e6 => format!("{:.1}M", n / 1e6),
        n if n >= 1e3 => format!("{:.0}K", n / 1e3),
        n => format!("{n:.0}"),
    }
}

/// Greenstacks: 10M or more of one item in storage. The game keeps every item that has ever
/// been at 10M in `GreenStacks`, and counts that list. Checked against the stackable items
/// (`extra.general.stackable`: item types starting b, c or d) in your storage chest
/// (`ChestOrder`/`ChestQuantity`) plus the ones already on the list, biggest stacks first;
/// with nothing stored, against every stackable item.
fn greenstacks(save: &Save) -> Mechanic {
    let stackable: Vec<&str> = GAME.extra("general").get("stackable").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let green: Vec<&str> = save.get("GreenStacks").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let order: Vec<&str> = save.get("ChestOrder").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let qty = save.numbers("ChestQuantity", &[]);
    let stacks: HashSet<&str> = stackable.iter().copied().collect();
    let mut totals: HashMap<&str, f64> = HashMap::new();
    for (i, id) in order.iter().enumerate() {
        if stacks.contains(id) {
            *totals.entry(id).or_default() += at(&qty, i);
        }
    }
    for id in &green {
        totals.entry(id).or_default();
    }
    let mut held: Vec<(&str, f64)> = totals.into_iter().collect();
    if held.is_empty() {
        held = stackable.iter().map(|id| (*id, 0.0)).collect();
    }
    held.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
    let greens: HashSet<&str> = green.iter().copied().collect();
    let items = held
        .into_iter()
        .map(|(id, n)| {
            let done = n >= 1e7 || greens.contains(id);
            let name = GAME.item_name(id).map(str::to_string).unwrap_or_else(|| words(id));
            let mut it = Item::new(name, None, f64::from(done), 1.0);
            if !done {
                it.level_name = Some(if n > 0.0 { format!("{} of 10M", short(n)) } else { "Not in storage".into() });
            }
            it
        })
        .collect();
    let mut m = mech("greenstacks", 0, "Greenstacks", None, Count::Done, "green", items);
    m.note = Some(format!("Out of the stackable items in your storage chest. The game remembers every item that has ever been at 10M: you have {}.", green.len()));
    m
}

/// Secret class path: Journeyman (class 2), Maestro (3), Voidwalker (4), Infinilyte (5), each
/// promotes to the next (`ClassPromotionChoices`). Done up to the best `CharacterClass_i`.
/// A class whose family bonus text is just its own name (`ClassFamilyBonuses`, Infinilyte
/// today) isn't out yet, and no save has it, so it's left out.
fn secret_path(save: &Save) -> Mechanic {
    let best = (0..characters(save))
        .map(|i| save.get(&format!("CharacterClass_{i}")).map(num).unwrap_or(0.0))
        .filter(|c| (2.0..=5.0).contains(c))
        .fold(0.0, f64::max);
    let names = rows("ClassNames", &[]);
    let family = rows("ClassFamilyBonuses", &[]);
    let items = (2..=5)
        .filter(|&c| names.get(c).is_some_and(|n| family.get(c).is_some_and(|f| text(f, 0) != n.as_str().unwrap_or(""))))
        .map(|c| Item::new(label(names[c].as_str().unwrap_or("")), pic(format!("ClassIcons{c}")), f64::from(best >= c as f64), 1.0))
        .collect();
    let mut m = mech("secret-path", 0, "Secret Class Path", pic("ClassIcons2".into()), Count::Done, "reached", items);
    m.note = Some("Infinilyte, after Voidwalker, is left out while its family bonus in the game data is still a placeholder.".into());
    m
}

/// Class levels: `Lv0_i[0]` per character. The game has no level cap.
fn class_levels(save: &Save) -> Mechanic {
    let items = (0..characters(save))
        .map(|i| Item::new(char_name(save, i), class_icon(save, i), at(&save.numbers(&format!("Lv0_{i}"), &[]), 0), 0.0))
        .collect();
    let mut m = mech("class-levels", 0, "Class Levels", None, Count::Open, "levels", items);
    m.note = Some("Class levels have no cap.".into());
    m
}

/// `PlayerCapacities[k]` rows the game hands out: [slot key, 1 if obtainable, item ID].
fn obtainable(k: usize) -> Vec<(String, String)> {
    rows("PlayerCapacities", &[k]).iter().filter(|r| cell(r, 1) == 1.0).map(|r| (text(r, 0).to_string(), text(r, 2).to_string())).collect()
}

fn has_key(v: Option<&Value>, key: &str) -> bool {
    v.and_then(Value::as_object).is_some_and(|o| o.contains_key(key))
}

/// Storage chests: `InvStorageUsed` has a key for each chest used; the chests are
/// `PlayerCapacities[1]`, without the two the game marks as no longer obtainable.
fn storage_chests(save: &Save) -> Mechanic {
    let used = save.get("InvStorageUsed");
    let items = obtainable(1)
        .into_iter()
        .map(|(key, id)| {
            let name = GAME.item_name(&id).map(str::to_string).unwrap_or_else(|| id.clone());
            Item::new(name, pic(id), f64::from(has_key(used, &key)), 1.0)
        })
        .collect();
    mech("storage-chests", 0, "Storage Chests", pic("InvStorage1".into()), Count::Done, "used", items)
}

/// Inventory bags: `InvBagsUsed_i` has a key per bag a character has used; the bags are
/// `PlayerCapacities[0]`, without the ones the game marks as no longer obtainable.
fn inventory_bags(save: &Save) -> Mechanic {
    let bags = obtainable(0);
    let items = (0..characters(save))
        .map(|i| {
            let used = save.get(&format!("InvBagsUsed_{i}"));
            let n = bags.iter().filter(|(key, _)| has_key(used, key)).count();
            Item::new(format!("{}: Inventory bags", char_name(save, i)), class_icon(save, i), n as f64, bags.len() as f64)
        })
        .collect();
    mech("inventory-bags", 0, "Inventory Bags", pic("InvBag1".into()), Count::Levels, "bags", items)
}

/// Carry capacity: `MaxCarryCap_i[kind]` per character, the capacity of the best bag that
/// character has used for that kind. `PlayerCapacities[2]` row 0 is the capacity at each bag
/// tier, and each other row is a kind with its bag at each tier ("MaxCapBag" and "Blank" are
/// empty tiers). A character's level is the tiers reached across all kinds; the max is every
/// kind at its best bag.
fn carry_capacity(save: &Save) -> Mechanic {
    let table = rows("PlayerCapacities", &[2]);
    let tiers: Vec<f64> = table.first().and_then(|r| r.as_array()).map(|a| a.iter().map(num).collect()).unwrap_or_default();
    let kinds: Vec<(String, usize)> = table
        .iter()
        .skip(1)
        .filter_map(|r| {
            let a = r.as_array()?;
            let top = a.iter().enumerate().skip(1).filter(|(_, b)| !matches!(b.as_str(), Some("MaxCapBag" | "Blank"))).map(|(j, _)| j).max()?;
            Some((text(r, 0).to_string(), top.min(tiers.len().saturating_sub(1))))
        })
        .collect();
    let max: usize = kinds.iter().map(|(_, top)| top).sum();
    let items = (0..characters(save))
        .map(|i| {
            let caps = save.get(&format!("MaxCarryCap_{i}"));
            let reached: usize = kinds
                .iter()
                .map(|(k, top)| {
                    let cap = caps.and_then(|c| c.get(k)).map(num).unwrap_or(0.0);
                    tiers.iter().rposition(|&t| t <= cap).unwrap_or(0).min(*top)
                })
                .sum();
            Item::new(format!("{}: Carry bags", char_name(save, i)), class_icon(save, i), reached as f64, max as f64)
        })
        .collect();
    let mut m = mech("carry-capacity", 0, "Carry Capacity", None, Count::Levels, "bag tiers", items);
    let best = kinds.iter().map(|(_, top)| at(&tiers, *top)).fold(0.0, f64::max);
    m.note = Some(format!("Bag tiers reached across the {} item kinds; the best bags hold {} a slot.", kinds.len(), short(best)));
    m
}
