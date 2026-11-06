//! Account-wide: cards, achievements and the rest that belongs to no world.

use std::collections::{HashMap, HashSet};

use bot_core::serde_json::Value;

use super::*;
use super::w1::{char_name, characters, class_icon, found_where, later, monster_world, shop_of, source_world};
use crate::review::{Have, State};

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

/// Cards for star `s` (0 is the first star) of a card whose first star costs `base`: the
/// count must pass this (the game's `CardLv`, as in [`card_stars`]).
fn card_step(s: usize, base: f64, chizoar: bool) -> f64 {
    let f = s as f64;
    if chizoar {
        1.5 * (f + 1.0 + (f / 3.0).floor()).powi(2)
    } else {
        base * (f + 1.0 + (f / 3.0).floor() + 16.0 * (f / 4.0).floor() + 100.0 * (f / 5.0).floor()).powi(2)
    }
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
            let mut it = Item::new(name.clone(), pic(format!("Cards{}", text(r, 1))), stars, CARD_STARS as f64);
            it.world = monster_world(mob).and_then(|w| later(f64::from(w), 1));
            if count <= 0.0 {
                it.how = Some(format!("Drops from {name}."));
            } else if (stars as usize) < CARD_STARS {
                let need = (card_step(stars as usize, cell(r, 2), mob == "Boss3B") + 1.0 - count).ceil().max(1.0);
                it.how = Some(format!("{} more cards for the next star.", big(need)));
            }
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
    // Levels are stars, so the game's tiers start at the first star; "Found" is level 0.
    m.tier_names = Some(["1 star", "2 stars", "3 stars", "4 stars", "5 stars", "6 stars"].iter().map(|s| s.to_string()).collect());
    m.note = Some("Cards go to 4 stars at first; a Rift unlock raises that to 5 stars, and a Spelunking one to 6.".into());
    m
}

/// Achievements: `AchieveReg[i]` is -1 once done. Only the first 420 are ever saved. They come
/// in blocks of 70 per world (N.js names each by `27 + floor(i / 70)` for its world's page, and
/// rows 70, 140, 210 start the World 2, 3 and 4 sets).
///
/// Pictures: the achievements page draws achievement i as `TaskAch` + `Number2Letter[27 + i/70]`
/// (A for World 1) + its place in that world counted from 1, `TaskAchA1` for row 0.
fn achievements(save: &Save) -> Mechanic {
    let reg = save.numbers("AchieveReg", &[]);
    let items = rows("RegAchieves", &[])
        .iter()
        .enumerate()
        .take(420)
        .filter(|(_, r)| text(r, 0) != "FILLERZZZ_ACH")
        .map(|(i, r)| {
            let done = at(&reg, i) == -1.0;
            let mut it = Item::new(label(text(r, 0)), achievement_picture(i), f64::from(done), 1.0);
            it.world = later((i / 70 + 1) as f64, 1);
            if !done {
                it.how = achievement_how(text(r, 2));
            }
            it
        })
        .collect();
    mech("achievements", 0, "Achievements", None, Count::Done, "done", items)
}

/// An achievement's own description, tidied to one short line: no "SECRET ACHIEVEMENT"
/// banner or dash rules, and cut at a word near 90 characters.
fn achievement_how(desc: &str) -> Option<String> {
    let s = words(desc).replace("SECRET ACHIEVEMENT", "").replace(['{', '}'], "");
    let s: Vec<&str> = s.split_whitespace().filter(|w| !w.chars().all(|c| c == '-')).collect();
    let mut out = String::new();
    for w in s {
        if out.len() + w.len() > 88 {
            out.push_str("...");
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    (!out.is_empty()).then_some(out)
}

/// Gem shop: `MTXinfo[section][tab][slot]` is one offer: col 0 its picture, col 1 its name,
/// col 4 the `GemItemsPurchased` index and col 5 the most you can buy (the shop prints
/// "YOU HAVE BOUGHT: n/col5" when col 5 is under 50000). Left out: blank slots, offers with no
/// limit, stock the game restocks daily, weekly or each update (found by the extractor from
/// where the game zeroes the count), and the Complete Class Redo, which is a one-off reset.
///
/// The next one costs `GemItemsPurchased[i]` × col 7 + col 3 gems (the shop's own `_GemCost`);
/// gems held are `GemsOwned`.
fn gem_shop(save: &Save) -> Mechanic {
    let bought = save.numbers("GemItemsPurchased", &[]);
    let gems = save.get("GemsOwned").map(num).unwrap_or(0.0).max(0.0);
    let mut ready = 0;
    let restocks: Vec<usize> = GAME.extra("general").get("gem_restocks").map(crate::save::numbers).unwrap_or_default().into_iter().map(|v| v as usize).collect();
    let mut items = Vec::new();
    // The shop's own tabs: the third section has one per world (Worlds 1 and 2 share the
    // first), and the first tab of the first section is the World 5 caverns.
    let tab_world = |sec: usize, tab: usize| match (sec, tab) {
        (0, 0) => Some(5),
        (2, t @ 1..=5) => Some(t as u8 + 2),
        _ => None,
    };
    for (sec, section) in GAME.rows("MTXinfo").iter().enumerate() {
        for (tab_i, tab) in section.as_array().into_iter().flatten().enumerate() {
            for r in tab.as_array().into_iter().flatten() {
                let (code, name, index, max) = (text(r, 0), text(r, 1), cell(r, 4) as usize, cell(r, 5));
                if name == "NAME_OF_ITEM" || max <= 0.0 || max >= 50000.0 || restocks.contains(&index) || code == "ClassSwapC" {
                    continue;
                }
                let picture = (code != "Blank").then(|| code.to_string());
                let mut it = Item::new(label(name), picture, at(&bought, index), max);
                it.world = tab_world(sec, tab_i);
                if !it.done() {
                    let cost = (it.level * cell(r, 7) + cell(r, 3)).round();
                    it.how = Some(format!("The next one costs {} gems.", big(cost)));
                    if cost <= gems {
                        it.state = State::Ready;
                        ready += 1;
                    }
                }
                items.push(it);
            }
        }
    }
    let mut m = mech("gem-shop", 0, "Gem Shop", pic("GemP1".into()), Count::Levels, "bought", items);
    m.note = Some("Permanent upgrades only: no restocking items, cosmetics or packs.".into());
    m.have = Some(Have {
        figure: format!("{} gems", big(gems)),
        text: if ready == 0 { "Not enough for the next purchase of any of these yet.".into() } else { format!("Enough for the next purchase of {ready} of these, one at a time.") },
    });
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
/// Event points to spend are `OptLacc[310]`; the shop takes the cost from there.
fn event_shop(save: &Save) -> Mechanic {
    let owned = save.at("OptLacc", &[311]).and_then(Value::as_str).unwrap_or("");
    let points = save.option(310).max(0.0);
    let letters = number_to_letter();
    let shop = rows("NinjaInfo", &[39]);
    let mut ready = 0;
    let items = shop
        .chunks(2)
        .enumerate()
        .map(|(i, pair)| {
            let name = pair[0].as_str().unwrap_or("").split('@').next().unwrap_or("");
            let has = letters.get(i).is_some_and(|l| owned.contains(l.as_str()));
            let mut it = Item::new(label(name), pic(format!("EventShopBuy{i}")), f64::from(has), 1.0);
            // The item's own text names the world it's for ("..._in_World_7").
            let desc = pair[0].as_str().unwrap_or("");
            it.world = (2..=7u8).rev().find(|w| desc.contains(&format!("World_{w}"))).and_then(|w| later(f64::from(w), 1));
            if !has {
                let cost = pair.get(1).map(|v| num(v)).unwrap_or(0.0);
                it.how = Some(format!("Costs {cost:.0} event points."));
                if cost <= points {
                    it.state = State::Ready;
                    ready += 1;
                }
            }
            it
        })
        .collect();
    let mut m = mech("event-shop", 0, "Event Shop", pic("EventShopBuy0".into()), Count::Done, "bought", items);
    m.have = Some(Have {
        figure: format!("{points:.0} event points"),
        text: if ready == 0 { "Not enough for anything left in the shop yet.".into() } else { format!("Enough for any one of {ready} items you don't have.") },
    });
    m
}

/// Whether event shop item `i` is bought.
pub(super) fn event_shop_owned(save: &Save, i: usize) -> bool {
    let owned = save.at("OptLacc", &[311]).and_then(Value::as_str).unwrap_or("");
    number_to_letter().get(i).is_some_and(|l| owned.contains(l.as_str()))
}

/// "2.4M", "380K": a stack size as the storage chest shows it. Rounded down, so 9.98M never
/// reads as the 10M that makes a stack green.
fn short(n: f64) -> String {
    let down = |v: f64, places: i32| {
        let p = 10f64.powi(places);
        (v * p).floor() / p
    };
    match n {
        n if n >= 1e9 => format!("{:.1}B", down(n / 1e9, 1)),
        n if n >= 1e6 => format!("{:.1}M", down(n / 1e6, 1)),
        n if n >= 1e3 => format!("{:.0}K", down(n / 1e3, 0)),
        n => format!("{n:.0}"),
    }
}

/// Achievement `i`'s icon, as the achievements page draws it: "TaskAch" + `Number2Letter[27 +
/// i / 70]` (A for World 1) + its place in that world from 1, "TaskAchA1" for row 0.
pub(super) fn achievement_picture(i: usize) -> Option<String> {
    number_to_letter().get(27 + i / 70).map(|l| format!("TaskAch{l}{}", i % 70 + 1))
}

/// Greenstacks: 10M or more of one item in storage. The game keeps every item that has ever
/// been at 10M in `GreenStacks`, and counts that list. Checked against the stackable items
/// (`extra.general.stackable`: item types starting b, c or d) in your storage chest
/// (`ChestOrder`/`ChestQuantity`) plus the ones already on the list, biggest stacks first;
/// with nothing stored, against every stackable item. An item's picture is its own ID.
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
            let mut it = Item::new(name, pic(id.to_string()), f64::from(done), 1.0);
            if !done {
                it.level_name = Some(if n > 0.0 { format!("{} of 10M", short(n)) } else { "Not in storage".into() });
                if n > 0.0 {
                    it.how = Some(format!("{} more in storage makes it green.", short(1e7 - n)));
                }
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

/// Class levels: `Lv0_i[0]` per character. The game has no level cap. The EXP toward the next
/// level is `Exp0_i[0]` and the EXP that level needs is `ExpReq0_i[0]`, which the game works out
/// and keeps in the save itself (N.js resets it to 15 with Lv 5 on a class redo), so it is
/// read rather than recomputed.
fn class_levels(save: &Save) -> Mechanic {
    let items = (0..characters(save))
        .map(|i| {
            let level = at(&save.numbers(&format!("Lv0_{i}"), &[]), 0);
            let mut it = Item::new(char_name(save, i), class_icon(save, i), level, 0.0);
            let exp = at(&save.numbers(&format!("Exp0_{i}"), &[]), 0).max(0.0);
            let req = at(&save.numbers(&format!("ExpReq0_{i}"), &[]), 0);
            if req.is_finite() && req > 0.0 && exp.is_finite() {
                let left = (req - exp).max(0.0);
                it.how = Some(format!("{} more class EXP for Lv {}, {:.0}% of the way there.", big(left), big(level + 1.0), (exp / req * 100.0).clamp(0.0, 100.0).floor()));
            }
            it
        })
        .collect();
    // The card shows the Beginner's class icon, the class every character starts as.
    let mut m = mech("class-levels", 0, "Class Levels", pic("ClassIcons1".into()), Count::Open, "levels", items);
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
    let mut gone = 0;
    let items = obtainable(1)
        .into_iter()
        .filter_map(|(key, id)| {
            let name = GAME.item_name(&id).map(str::to_string).unwrap_or_else(|| id.clone());
            let done = has_key(used, &key);
            let mut it = Item::new(name, pic(id.clone()), f64::from(done), 1.0);
            it.world = source_world(&id).and_then(|w| later(f64::from(w), 1));
            if !done {
                it.how = found_where(&id);
                // Nothing gives it now (a past event's chest): it counts only for those who have it.
                if it.how.is_none() {
                    gone += 1;
                    return None;
                }
            }
            Some(it)
        })
        .collect();
    let mut m = mech("storage-chests", 0, "Storage Chests", pic("InvStorage1".into()), Count::Done, "used", items);
    if gone > 0 {
        m.note = Some(format!("{gone} more chests came from past events and nothing gives them now, so they aren't counted."));
    }
    m
}

/// Inventory bags: `InvBagsUsed_i` has a key per bag a character has used; the bags are
/// `PlayerCapacities[0]`, without the ones the game marks as no longer obtainable.
fn inventory_bags(save: &Save) -> Mechanic {
    let bags = obtainable(0);
    let items = (0..characters(save))
        .map(|i| {
            let used = save.get(&format!("InvBagsUsed_{i}"));
            let n = bags.iter().filter(|(key, _)| has_key(used, key)).count();
            let mut it = Item::new(format!("{}: Inventory bags", char_name(save, i)), class_icon(save, i), n as f64, bags.len() as f64);
            // The first bag this character hasn't used, with where it's sold when a town shop has it.
            if let Some((_, id)) = bags.iter().find(|(key, _)| !has_key(used, key)) {
                let name = GAME.item_name(id).unwrap_or(id);
                it.how = Some(match shop_of(id) {
                    Some(shop) => format!("Missing {name}. {shop}"),
                    None => format!("Missing {name}."),
                });
            }
            it
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
    let kinds: Vec<(String, usize, &Value)> = table
        .iter()
        .skip(1)
        .filter_map(|r| {
            let a = r.as_array()?;
            let top = a.iter().enumerate().skip(1).filter(|(_, b)| !matches!(b.as_str(), Some("MaxCapBag" | "Blank"))).map(|(j, _)| j).max()?;
            Some((text(r, 0).to_string(), top.min(tiers.len().saturating_sub(1)), *r))
        })
        .collect();
    let max: usize = kinds.iter().map(|(_, top, _)| top).sum();
    let items = (0..characters(save))
        .map(|i| {
            let caps = save.get(&format!("MaxCarryCap_{i}"));
            let tier_of: Vec<usize> = kinds
                .iter()
                .map(|(k, top, _)| {
                    let cap = caps.and_then(|c| c.get(k)).map(num).unwrap_or(0.0);
                    tiers.iter().rposition(|&t| t <= cap).unwrap_or(0).min(*top)
                })
                .collect();
            let reached: usize = tier_of.iter().sum();
            let mut it = Item::new(format!("{}: Carry bags", char_name(save, i)), class_icon(save, i), reached as f64, max as f64);
            // The kind furthest from its best bag, and the bag one tier up from where it is.
            let low = kinds.iter().zip(&tier_of).filter(|((_, top, _), t)| *t < top).min_by_key(|((_, top, _), t)| (**t * 100) / (*top).max(1));
            if let Some(((_, _, row), t)) = low {
                let next = (t + 1..row.as_array().map_or(0, Vec::len)).find_map(|j| text(row, j).starts_with("MaxCapBag").then(|| text(row, j)).filter(|b| *b != "MaxCapBag"));
                if let Some(bag) = next {
                    let name = GAME.item_name(bag).unwrap_or(bag);
                    it.how = Some(match shop_of(bag) {
                        Some(shop) => format!("Next bag up: {name}. {shop}"),
                        None => format!("Next bag up: {name}."),
                    });
                }
            }
            it
        })
        .collect();
    // The card shows the first kind's first bag (the Mini Materials Pouch).
    let first_bag = table.get(1).map(|r| text(r, 1).to_string()).filter(|b| b.starts_with("MaxCapBag"));
    let mut m = mech("carry-capacity", 0, "Carry Capacity", first_bag, Count::Levels, "bag tiers", items);
    let best = kinds.iter().map(|(_, top, _)| at(&tiers, *top)).fold(0.0, f64::max);
    m.note = Some(format!("Bag tiers reached across the {} item kinds; the best bags hold {} a slot.", kinds.len(), short(best)));
    m
}
