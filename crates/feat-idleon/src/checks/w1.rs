//! World 1: Blunder Hills.

use std::collections::HashMap;

use bot_core::serde_json::Value;

use super::*;
use crate::review::{Have, Raise, State};

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![
        statues(save),
        bribes(save),
        star_signs(save),
        stamps(save),
        statue_flair(save),
        owl(save),
        owl_lasting(save),
        forge(save),
        anvil_points(save),
        minigame(save, "basketball", "Basketball", "hoops", "HoopShop"),
        minigame(save, "darts", "Darts", "darts", "DartShop"),
        upgrade_vault(save),
    ]
}

const STATUE_TIERS: [&str; 4] = ["Normal", "Gold", "Onyx", "Zenith"];

/// Statues: `StuG[i]` is the tier, 0 Normal to 3 Zenith. Levels have no cap, so done is Zenith.
fn statues(save: &Save) -> Mechanic {
    let tiers = save.numbers("StuG", &[]);
    let chisels = save.option(69);
    let chest = storage(save);
    let items = rows("StatueInfo", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let tier = at(&tiers, i).clamp(0.0, 3.0);
            let prefix = ["Statue", "StatueG", "StatueO", "StatueZ"][tier as usize];
            let mut it = Item::new(format!("{} Statue", label(text(r, 0))), pic(format!("{prefix}{}", i + 1)), tier, 3.0);
            it.level_name = Some(STATUE_TIERS[tier as usize].into());
            statue_step(&mut it, i, tier, chisels, held(&chest, &format!("EquipmentStatues{}", i + 1)));
            // `StatueInfo` col 4 is the statue's world, 0 for World 1.
            it.world = later(cell(r, 4) + 1.0, 1);
            it
        })
        .collect();
    let mut m = mech("statues", 1, "Statues", pic("StatueZ1".into()), Count::Levels, "tiers", items);
    m.tier_names = Some(STATUE_TIERS[1..].iter().map(|s| s.to_string()).collect());
    m.note = Some("Each statue goes Normal, Gold, Onyx, then Zenith. Levels keep going, so Zenith counts as done.".into());
    m
}

/// The next tier for statue `i` (N.js, the Statue Man's drop handler in W1 town): with the
/// Onyx Tools given to him (`OptLacc[69]` 2), a stack of exactly 20,000 of a Gold statue
/// dropped on him makes it Onyx; with the Zenith Tools (3), 1,000,000 of an Onyx one makes it
/// Zenith. The Power Statue skips the stacks: its Onyx comes from the Monolith, its Zenith with
/// the Zenith Tools themselves.
fn statue_step(it: &mut Item, i: usize, tier: f64, chisels: f64, stored: f64) {
    let (next, stack, tools, needed) = match tier as u8 {
        1 => ("Onyx", 20_000.0, "SmithingHammerChisel3", 2.0),
        2 => ("Zenith", 1_000_000.0, "SmithingHammerChisel4", 3.0),
        _ => return,
    };
    let tools = GAME.item_name(tools).unwrap_or(next);
    if chisels < needed {
        it.how = Some(format!("{next} opens when you give the Statue Man the {tools}."));
        it.state = State::Locked;
    } else if i == 0 {
        it.how = Some(if tier < 2.0 { "Its Onyx form comes from the Monolith.".into() } else { format!("Its Zenith form comes with the {tools}.") });
    } else {
        it.how = Some(format!("Drop a stack of exactly {} at the Statue Man. You have {} in storage.", big(stack), big(stored)));
        if stored >= stack {
            it.state = State::Ready;
        }
    }
}

/// Bribes: `BribeStatus[i]` is 1 when bought. The last row is the game's "no more bribes".
///
/// N.js (the bribe shop's buy button): a bribe costs col 2 in coins of the character buying it;
/// -1 is locked until the "Art of the..." bribe before it (col 4 `BribeExpansion`, col 5
/// "first&count") opens its set. Two cost 99999999, which means they open another way: row 26
/// with a Fishing Island shop purchase (`OptLacc[165]`), row 33 with the Jade Emporium's
/// New Bribes.
fn bribes(save: &Save) -> Mechanic {
    let status = save.numbers("BribeStatus", &[]);
    let table = rows("BribeDescriptions", &[]);
    let real = table.len().saturating_sub(1);
    let coins = richest(save);
    let opener = |i: usize| {
        table.iter().take(real).find(|r| {
            let span: Vec<usize> = text(r, 5).split('&').filter_map(|x| x.parse().ok()).collect();
            text(r, 4) == "BribeExpansion" && span.len() == 2 && (span[0]..span[0] + span[1]).contains(&i)
        })
    };
    let mut affordable = 0;
    let items: Vec<Item> = table
        .iter()
        .take(real)
        .enumerate()
        .map(|(i, r)| {
            let st = at(&status, i);
            let mut it = Item::new(label(text(r, 0)), pic(format!("BribeO{}", text(r, 3))), f64::from(st >= 1.0), 1.0);
            let cost = cell(r, 2);
            if st >= 1.0 {
            } else if st < 0.0 {
                let by = opener(i).map_or_else(|| "an earlier bribe".to_string(), |o| label(text(o, 0)));
                it.how = Some(format!("Opens when you buy {by}."));
                it.state = State::Locked;
            } else if cost == 99_999_999.0 {
                it.how = Some(match i {
                    26 => "Opens with a purchase in the Fishing Island shop.".into(),
                    33 => "Opens with New Bribes in the Jade Emporium.".into(),
                    _ => "Opens another way, not with coins.".into(),
                });
            } else {
                it.how = Some(format!("Costs {} coins.", big(cost)));
                if cost <= coins {
                    it.state = State::Ready;
                    affordable += 1;
                }
            }
            it
        })
        .collect();
    let mut m = mech("bribes", 1, "Bribes", pic("BribeOpigbank".into()), Count::Done, "bought", items);
    m.have = Some(coin_have(coins, if affordable == 0 { "Not enough for any bribe open to you yet.".into() } else { format!("Enough for {affordable} of the bribes open to you.") }));
    m
}

/// Star signs: `StarSg` maps a sign's name to 1 once found. The obtainable ones are those the
/// star map places (`SSignInfoUI` col 4 isn't 999).
fn star_signs(save: &Save) -> Mechanic {
    let found = save.get("StarSg").and_then(Value::as_object);
    // N.js (the star map's buy): a sign costs `SSignInfoUI` col 4 star points, `OptLacc[40]`.
    let cost_of: Vec<(usize, f64)> = rows("SSignInfoUI", &[]).iter().map(|r| (cell(r, 0) as usize, cell(r, 4))).collect();
    let placed: Vec<usize> = rows("SSignInfoUI", &[])
        .iter()
        .filter(|r| cell(r, 4) != 999.0)
        .map(|r| cell(r, 0) as usize)
        .collect();
    let signs = rows("StarSigns", &[]);
    let mut idx: Vec<usize> = placed.into_iter().filter(|&i| i < signs.len()).collect();
    idx.sort_unstable();
    idx.dedup();
    let items = idx
        .into_iter()
        .map(|i| {
            let name = text(signs[i], 0);
            let on = found.and_then(|f| f.get(name)).map(num).unwrap_or(0.0) > 0.0;
            let mut it = Item::new(label(name), None, f64::from(on), 1.0);
            if !on {
                let cost = cost_of.iter().find(|(k, _)| *k == i).map_or(0.0, |(_, c)| *c);
                it.how = Some(if cost > 0.0 { format!("Costs {cost:.0} star points on the star map.") } else { "Free on the star map once you reach it.".into() });
            }
            it
        })
        .collect();
    let mut m = mech("star-signs", 1, "Star Signs", None, Count::Done, "found", items);
    let points = save.option(40);
    m.have = Some(Have { figure: format!("{points:.0} star points"), text: "Star points come from constellations. A sign opens once the map reaches it.".into() });
    m
}

/// Stamps: `StampLv[tab][i]`. No hard max, so this counts stamps collected; the level shows
/// beside each.
fn stamps(save: &Save) -> Mechanic {
    let mut items = Vec::new();
    let mut gone = 0;
    for (tab, letter) in ["A", "B", "C"].iter().enumerate() {
        let levels = save.numbers("StampLv", &[tab]);
        for i in 0..rows("StampDescriptions", &[tab]).len() {
            let id = format!("Stamp{letter}{}", i + 1);
            let lv = at(&levels, i);
            let name = GAME.item_name(&id).map(str::to_string).unwrap_or_else(|| id.clone());
            let mut it = Item::new(name, pic(id.clone()), f64::from(lv > 0.0), 1.0);
            it.level_name = Some(if lv > 0.0 { format!("Lv {lv:.0}") } else { "Not found".into() });
            it.world = source_world(&id).and_then(|w| later(f64::from(w), 1));
            if lv <= 0.0 {
                it.how = found_where(&id);
                // No shop, drop, quest or prize gives it: an event stamp that's gone or one not
                // out yet. It counts only for the accounts that already have it.
                if it.how.is_none() {
                    gone += 1;
                    continue;
                }
            }
            items.push(it);
        }
    }
    let mut m = mech("stamps", 1, "Stamps", pic("StampA1".into()), Count::Done, "collected", items);
    m.note = Some(match gone {
        0 => "Stamps have no level cap, so this counts the ones you have.".into(),
        n => format!("Stamps have no level cap, so this counts the ones you have. {n} more exist that nothing in the game gives now (past events, or not out yet), so they aren't counted."),
    });
    m
}

/// The character's name as the save has it, or "Character 3" when the paste has no names.
pub(super) fn char_name(save: &Save, i: usize) -> String {
    save.names.get(i).cloned().unwrap_or_else(|| format!("Character {}", i + 1))
}

/// Characters to list: at least one, so an empty save still reads as an account.
pub(super) fn characters(save: &Save) -> usize {
    save.characters().max(1)
}

/// The class icon for character `i` (`CharacterClass_i` is the class number).
pub(super) fn class_icon(save: &Save, i: usize) -> Option<String> {
    let class = save.get(&format!("CharacterClass_{i}")).map(num).unwrap_or(0.0);
    (class > 0.0).then(|| format!("ClassIcons{class:.0}"))
}

/// Coins: each character keeps its own (`Money_i`), and an account purchase is paid by the
/// character buying it, so the richest character is what one purchase can spend.
pub(super) fn richest(save: &Save) -> f64 {
    (0..characters(save)).map(|i| save.get(&format!("Money_{i}")).map(num).unwrap_or(0.0)).filter(|c| c.is_finite()).fold(0.0, f64::max)
}

pub(super) fn coin_have(coins: f64, text: String) -> Have {
    Have { figure: format!("{} coins on your richest character", big(coins)), text }
}

/// The storage chest's stacks by item (`ChestOrder`, `ChestQuantity`).
pub(super) fn storage(save: &Save) -> HashMap<String, f64> {
    let order = save.get("ChestOrder").and_then(Value::as_array);
    let qty = save.numbers("ChestQuantity", &[]);
    let mut out: HashMap<String, f64> = HashMap::new();
    for (i, id) in order.into_iter().flatten().enumerate() {
        if let Some(id) = id.as_str() {
            let q = at(&qty, i);
            if q.is_finite() && q > 0.0 {
                *out.entry(id.to_string()).or_default() += q;
            }
        }
    }
    out
}

pub(super) fn held(chest: &HashMap<String, f64>, id: &str) -> f64 {
    chest.get(id).copied().unwrap_or(0.0)
}

/// "Sold at the shop in Blunder Hills.": the town shop whose stock (`ShopNames[k]`) has the
/// item; `ShopLocations[k]` is the shop's map, `MapDispName` its name.
pub(super) fn shop_of(id: &str) -> Option<String> {
    let k = GAME.rows("ShopNames").iter().position(|r| r.as_array().is_some_and(|a| a.iter().any(|x| x.as_str() == Some(id))))?;
    let map = rows("ShopLocations", &[]).get(k).map(|v| num(v) as usize)?;
    let town = rows("MapDispName", &[]).get(map).map(|v| label(v.as_str().unwrap_or("")))?;
    Some(format!("Sold at the shop in {town}."))
}

/// `Some(world)` when `world` is later than the mechanic's own (`own`, 0 for account-wide
/// mechanics counts as 1), for `Item.world`.
pub(super) fn later(world: f64, own: u8) -> Option<u8> {
    let w = world.clamp(1.0, 7.0) as u8;
    (w > own.max(1)).then_some(w)
}

/// The world of a map: maps are numbered by world in fifties (`ShopLocations` puts the towns at
/// 0, 50, 100, ... 300, and `MapDispName` runs World 1 then World 2 and so on).
fn map_world(map: usize) -> u8 {
    (map / 50 + 1).min(7) as u8
}

/// The earliest world a monster is fought in: the maps whose `MapAFKtarget` it is.
pub(super) fn monster_world(mob: &str) -> Option<u8> {
    rows("MapAFKtarget", &[]).iter().enumerate().filter(|(_, v)| v.as_str() == Some(mob)).map(|(m, _)| map_world(m)).min()
}

/// The earliest world an item can be had in: the town shop that sells it, the monsters that
/// drop it, World 2 for Arcade prizes. `None` when the data doesn't say.
pub(super) fn source_world(id: &str) -> Option<u8> {
    let shop = GAME
        .rows("ShopNames")
        .iter()
        .position(|r| r.as_array().is_some_and(|a| a.iter().any(|x| x.as_str() == Some(id))))
        .and_then(|k| rows("ShopLocations", &[]).get(k).map(|v| map_world(num(v) as usize)));
    let general = GAME.extra("general");
    let drop = general.get("drops").and_then(|d| d.get(id)).and_then(Value::as_array).and_then(|mobs| mobs.iter().filter_map(Value::as_str).filter_map(monster_world).min());
    let prize = (general.get("sources").and_then(|x| x.get(id)).and_then(Value::as_str) == Some("arcade")).then_some(2);
    [shop, drop, prize].into_iter().flatten().min()
}

/// Where a found-once item comes from, per the game data, in this order: a town shop
/// ([`shop_of`]), monster drops (`extra.general.drops`, most likely first, up to three named), a
/// quest reward (`extra.general.quests`, the NPC is the quest name without its number), an
/// Arcade prize or the Fishing Island shop (`extra.general.sources`), or for
/// storage chests 31 to 42 the gem shop's Storage Chest Space (N.js hands out
/// `"InvStorage" + min(31 + bought, 42)`).
pub(super) fn found_where(id: &str) -> Option<String> {
    if let Some(shop) = shop_of(id) {
        return Some(shop);
    }
    let general = GAME.extra("general");
    let mobs: Vec<&str> = general.get("drops").and_then(|d| d.get(id)).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if !mobs.is_empty() {
        let mut names: Vec<String> = Vec::new();
        for mob in &mobs {
            let name = GAME.monster_name(mob).map(str::to_string).unwrap_or_else(|| words(mob));
            if !names.contains(&name) {
                names.push(name);
            }
        }
        let more = names.len() > 3;
        names.truncate(3);
        let list = match names.as_slice() {
            [a] => a.clone(),
            [a, b] => format!("{a} and {b}"),
            [a, b, c] => format!("{a}, {b} and {c}"),
            _ => String::new(),
        };
        return Some(format!("Drops from {list}{}.", if more { ", and others" } else { "" }));
    }
    if let Some(quest) = general.get("quests").and_then(|q| q.get(id)).and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_str) {
        let npc = words(quest.trim_end_matches(|c: char| c.is_ascii_digit()));
        return Some(format!("A quest reward from {npc}."));
    }
    match general.get("sources").and_then(|x| x.get(id)).and_then(Value::as_str) {
        Some("arcade") => return Some("A prize at the Arcade.".into()),
        Some("fishing_island") => return Some("Sold at the Fishing Island shop.".into()),
        _ => {}
    }
    let n: Option<u32> = id.strip_prefix("InvStorage").and_then(|n| n.parse().ok());
    if n.is_some_and(|n| (31..=42).contains(&n)) {
        return Some("From Storage Chest Space in the gem shop.".into());
    }
    None
}

/// The level `budget` takes an item to from `level`, one level at a time, stopping at `max`
/// and after 300 levels (costs only grow, so a real budget runs out well before).
pub(super) fn reach(level: f64, max: f64, budget: f64, cost: impl Fn(f64) -> f64) -> f64 {
    let (mut lv, mut left) = (level, budget);
    for _ in 0..300 {
        if lv >= max {
            break;
        }
        let c = cost(lv);
        if !(c.is_finite() && c <= left) {
            break;
        }
        left -= c;
        lv += 1.0;
    }
    lv
}

/// "Enough to take A to Lv 12, or B to Lv 40." for the two lowest items `budget` can move,
/// each on its own. `cost(i, level)` is item i's next level.
pub(super) fn reach_text(items: &[Item], budget: f64, cost: impl Fn(usize, f64) -> f64, none: &str) -> String {
    let mut open: Vec<(usize, &Item)> = items.iter().enumerate().filter(|(i, it)| !it.done() && cost(*i, it.level) <= budget).collect();
    open.sort_by(|a, b| a.1.share().total_cmp(&b.1.share()).then(a.1.level.total_cmp(&b.1.level)));
    let parts: Vec<String> = open
        .iter()
        .take(2)
        .map(|(i, it)| format!("{} to Lv {}", it.name, reach(it.level, if it.max > 0.0 { it.max } else { f64::INFINITY }, budget, |lv| cost(*i, lv))))
        .collect();
    match parts.as_slice() {
        [] => none.to_string(),
        [one] => format!("Enough to take {one}."),
        [a, b, ..] => format!("Enough to take {a}, or {b}."),
    }
}

/// Statue Flair: `RoyalG[22][i]` per statue. N.js `SF_maxLV` is 3, plus 1 with Menagerie
/// unlock 17; the system itself opens with the Statue Flair upgrade in the Armory.
///
/// N.js `RoyalG`: `SF_unlocked` is Armory upgrade 78 (`RoyalG[2][78]`) at 1 or more;
/// `SF_costo` is 10 × (i + 1) × 5^level of the marble `RGshard{StatueInfo col 4}`, counted
/// from your inventory and storage (`SF_weOwn`); this reads storage only.
fn statue_flair(save: &Save) -> Mechanic {
    let levels = save.numbers("RoyalG", &[22]);
    let unlocked = at(&save.numbers("RoyalG", &[2]), 78) >= 1.0;
    let raised = super::w7::menagerie_unlock(save, 17);
    let chest = storage(save);
    let items = rows("StatueInfo", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let level = at(&levels, i);
            let mut it = Item::new(format!("{} Statue", label(text(r, 0))), pic(format!("Statue{}", i + 1)), level, 4.0);
            let shard = format!("RGshard{}", text(r, 4));
            let marble = GAME.item_name(&shard).unwrap_or("marble").to_string();
            if !unlocked {
                it.how = Some("Opens with the Statue Flair upgrade in the Armory.".into());
                it.state = State::Locked;
            } else if level >= 3.0 && !raised && level < 4.0 {
                it.how = Some(format!("Level 4 needs {}.", super::w7::menagerie_unlock_name(17)));
                it.state = State::Locked;
            } else if level < 4.0 {
                let cost = 10.0 * (i as f64 + 1.0) * 5f64.powf(level);
                let have = held(&chest, &shard);
                it.how = Some(format!("Next level costs {} {marble}. You have {} in storage.", big(cost), big(have)));
                if have >= cost {
                    it.state = State::Ready;
                }
            }
            it
        })
        .collect();
    // World 7: flair is unlocked from the W7 armory, so it is weighed against W7 progress.
    let mut m = mech("statue-flair", 7, "Statue Flair", pic("Statue1".into()), Count::Levels, "levels", items);
    m.note = Some("Opens with the Statue Flair upgrade in the Armory. Each statue's flair goes to level 3, and to level 4 with a Menagerie unlock.".into());
    let unlock = super::w7::menagerie_unlock_name(17);
    let mut name = unlock.clone();
    if let Some(f) = name.get_mut(0..1) {
        f.make_ascii_uppercase();
    }
    m.raises = vec![Raise { name, amount: "+1 max level on every statue's flair, 3 to 4".into(), owned: raised }];
    m
}

/// Owl: Orion's Megafeathers. `OptLacc[262]` is how many you have; the game counts the first
/// ten as all of them (the Equinox dream asks for "all 10"), and the tenth, Fractal, keeps
/// stacking past that. The feather upgrades reset with every Feather Restart, so only the
/// Megafeathers and Bonuses of Orion (`OptLacc[255]`, no cap) last.
fn owl(save: &Save) -> Mechanic {
    let owned = save.option(262);
    let names = GAME.extra("w1").get("megafeathers").and_then(Value::as_array).cloned().unwrap_or_default();
    let items = names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let mut it = Item::new(format!("{} Megafeather", n.as_str().unwrap_or("")), pic(format!("Feaz{i}")), f64::from(owned > i as f64), 1.0);
            if owned <= i as f64 {
                it.how = Some("Buy The Great Mega Reset in Orion's shop: it resets your feathers and gives one.".into());
            }
            it
        })
        .collect();
    let mut m = mech("owl", 1, "Owl", pic("Feaz0".into()), Count::Done, "earned", items);
    m.note = Some(format!(
        "The ten kinds of Megafeather; you have {owned:.0} in all. Fractal ones keep stacking past the tenth: see Owl: Lasting Bonuses."
    ));
    m
}

/// The two Owl things that outlast every Feather Restart and have no cap (N.js "OwlBonuses"
/// and "OwlMegafeather"): Bonuses of Orion (`OptLacc[255]`, the upgrade that "never resets";
/// the six bonuses each get a level every sixth purchase) and Fractal Megafeather stacks (each
/// Megafeather past the tenth makes Orion's bonuses 0.5x bigger: 6 + 0.5 × (count - 10) times).
/// Costs are N.js `OwlCost` without its discounts: `Owlz[t]` col 1 × col 2^level, in feathers
/// (`OptLacc[253]`), for t 1 (Bonuses of Orion) and 8 (The Great Mega Reset, level = your
/// Megafeathers, `OptLacc[262]`).
fn owl_lasting(save: &Save) -> Mechanic {
    let owlz = rows("Owlz", &[]);
    let cost = |t: usize, level: f64| owlz.get(t).map_or(f64::INFINITY, |r| cell(r, 1) * cell(r, 2).powf(level));
    let feathers = save.option(253).max(0.0);
    let (orion, mega) = (save.option(255), save.option(262));
    let mut bonuses = Item::new("Bonuses of Orion", pic("Feaz0".into()), orion, 0.0);
    let next = cost(1, orion);
    bonuses.how = Some(format!("Next one costs {} feathers before discounts. You have {}.", big(next), big(feathers)));
    if next <= feathers {
        bonuses.state = State::Ready;
    }
    let mut fractal = Item::new("Fractal Megafeather stacks", pic("Feaz9".into()), (mega - 9.0).max(0.0), 0.0);
    fractal.how = Some(format!(
        "Each Great Mega Reset past the tenth adds one, +0.5x to Orion's bonuses. Next costs {} feathers before discounts.",
        big(cost(8, mega))
    ));
    let mut m = mech("owl-lasting", 1, "Owl: Lasting Bonuses", pic("Feaz9".into()), Count::Open, "levels", vec![bonuses, fractal]);
    m.have = Some(Have { figure: format!("{} feathers", big(feathers)), text: "Feathers reset with every Feather Restart; these two stay.".into() });
    m.note = Some("Neither has a cap, and neither resets with a Feather Restart or a Great Mega Reset.".into());
    m
}

/// Forge upgrades: `ForgeLV[i]`; the game stops each at `RANDOlist[42][i]` and shows
/// "-MAX_LEVEL-". Names are the forge page's own text.
///
/// Cost in coins, N.js `ForgeUpdateCosts` (its discount is fixed at 1, so there is none): slots
/// 200 × 5.4^(level^0.83); the rest 50 × 2.5^(level^0.51) under level 5, then 400 × b^(level - 5)
/// with b 1.41, 1.2, 1.21, 1.25, 1.33.
fn forge_cost(i: usize, level: f64) -> f64 {
    match i {
        0 => (200.0 * 5.4f64.powf(level.powf(0.83))).round(),
        1..=5 if level < 5.0 => (50.0 * 2.5f64.powf(level.powf(0.51))).round(),
        1..=5 => (400.0 * [1.41f64, 1.2, 1.21, 1.25, 1.33][i - 1].powf(level - 5.0)).round(),
        _ => f64::INFINITY,
    }
}

fn forge(save: &Save) -> Mechanic {
    let levels = save.numbers("ForgeLV", &[]);
    let coins = richest(save);
    let names = GAME.extra("w1").get("forge_upgrades").and_then(Value::as_array).cloned().unwrap_or_default();
    let maxes = rows("RANDOlist", &[42]);
    let items = maxes
        .iter()
        .enumerate()
        .map(|(i, mx)| {
            let name = names.get(i).and_then(Value::as_str).map(label).unwrap_or_else(|| format!("Forge upgrade {}", i + 1));
            let mut it = Item::new(name, None, at(&levels, i), num(mx));
            if !it.done() {
                let cost = forge_cost(i, it.level);
                it.how = Some(format!("Next level costs {} coins.", big(cost)));
                if cost <= coins {
                    it.state = State::Ready;
                }
            }
            it
        })
        .collect::<Vec<Item>>();
    let text = reach_text(&items, coins, forge_cost, "Not enough for the next level of any forge upgrade yet.");
    let mut m = mech("forge", 1, "Forge Upgrades", pic("ForgeA".into()), Count::Levels, "levels", items);
    m.have = Some(coin_have(coins, text));
    m
}

/// Anvil points: `AnvilPAstats_i[1]` bought with coins and `[2]` with monster parts. The buy
/// buttons stop at 600 and 700 (read from the game's code by the extractor).
///
/// N.js `AnvilProduceStats`: a coin point costs (p³ + 50) × (1 + p/100), a monster point
/// (q + 1)^1.5 + q of the part `Costs2TypeAnvilPA` names for q, both before the bubble discount.
fn anvil_points(save: &Save) -> Mechanic {
    let caps = GAME.extra("w1").get("anvil_points").map(crate::save::numbers).unwrap_or_default();
    let ladder = GAME.extra("w1").get("anvil_parts").and_then(Value::as_array).cloned().unwrap_or_default();
    let part_for = |q: f64| {
        ladder
            .iter()
            .find(|step| step.get(0).is_none_or(|b| b.is_null() || q < num(b)))
            .and_then(|step| step.get(1)).and_then(Value::as_str).unwrap_or("").to_string()
    };
    let chest = storage(save);
    let max = at(&caps, 0) + at(&caps, 1);
    let items = (0..characters(save))
        .map(|i| {
            let s = save.numbers(&format!("AnvilPAstats_{i}"), &[]);
            let (p, q) = (at(&s, 1), at(&s, 2));
            let mut it = Item::new(format!("{}: Anvil points", char_name(save, i)), class_icon(save, i), p + q, max);
            if p < at(&caps, 0) {
                let cost = ((p.powi(3) + 50.0) * (1.0 + p / 100.0)).round();
                it.how = Some(format!("Next coin point costs {} coins.", big(cost)));
                if cost <= save.get(&format!("Money_{i}")).map(num).unwrap_or(0.0) {
                    it.state = State::Ready;
                }
            } else if q < at(&caps, 1) {
                let cost = ((q + 1.0).powf(1.5) + q).round();
                let part = part_for(q);
                let have = held(&chest, &part);
                it.how = Some(format!("Next point costs {} {}. You have {} in storage.", big(cost), GAME.item_name(&part).unwrap_or(&part), big(have)));
                if have >= cost {
                    it.state = State::Ready;
                }
            }
            it
        })
        .collect();
    let mut m = mech("anvil-points", 1, "Anvil Points", None, Count::Levels, "points", items);
    m.note = Some(format!("Each character can buy {:.0} points with coins and {:.0} with monster parts. Costs are before your discounts.", at(&caps, 0), at(&caps, 1)));
    m
}

/// Basketball (Hoops) and Darts shops: levels at `OptLacc[first + i]`. The cost goes up
/// forever and nothing stops a level, so these have no max.
/// The cost of the next level in shop points, N.js `Thingies` "HoopsUpgCost" (upgrade 2:
/// 3 + level/8, the rest 1 + level/20) and "DartsUpgCost" (upgrade 1: 3 + level/0.25,
/// upgrade 3: 5 + level/0.05, the rest 2 + level/12), each rounded down.
fn minigame_cost(darts: bool, i: usize, level: f64) -> f64 {
    let c = match (darts, i) {
        (false, 2) => 3.0 + level / 8.0,
        (false, _) => 1.0 + level / 20.0,
        (true, 1) => 3.0 + level / 0.25,
        (true, 3) => 5.0 + level / 0.05,
        (true, _) => 2.0 + level / 12.0,
    };
    c.floor()
}

/// The points each shop spends sit just before its levels: `OptLacc[first - 1]`.
fn minigame(save: &Save, key: &'static str, name: &'static str, extra: &str, picture: &str) -> Mechanic {
    let shop = GAME.extra("w1").get(extra);
    let first = shop.and_then(|s| s.get("first")).map(num).unwrap_or(0.0) as usize;
    let names = shop.and_then(|s| s.get("upgrades")).and_then(Value::as_array).cloned().unwrap_or_default();
    let darts = extra == "darts";
    let points = if first > 0 { save.option(first - 1).max(0.0) } else { 0.0 };
    let items: Vec<Item> = names
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let mut it = Item::new(label(&d.as_str().unwrap_or("").replace("}x_", "")), pic(format!("{picture}{i}")), save.option(first + i), 0.0);
            let cost = minigame_cost(darts, i, it.level);
            it.how = Some(format!("Next level costs {} points.", big(cost)));
            if cost <= points {
                it.state = State::Ready;
            }
            it
        })
        .collect();
    let text = reach_text(&items, points, |i, lv| minigame_cost(darts, i, lv), "Not enough for the next level of any upgrade yet.");
    let mut m = mech(key, 1, name, pic(format!("{picture}0")), Count::Open, "levels", items);
    m.have = Some(Have { figure: format!("{} {name} points", big(points)), text });
    m.note = Some("These upgrades have no cap. Points come from playing the minigame.".into());
    m
}

/// A vault upgrade's name without the game's info marker: "Natural_Talent_製_(Tap_for_Info)"
/// is "Natural Talent".
fn vault_name(s: &str) -> String {
    label(s.split('製').next().unwrap_or(s))
}

/// Upgrade Vault: `UpgVault[i]`. N.js `VaultUpgMaxLV`: col 4, plus 25 for the rows in
/// `Research[63]` with the Gilded Awnings event shop item, plus 10 with the Potluck Pack for
/// rows whose cap is 2 or more and for the Glimbo rows (`Research[26]`), plus, for those,
/// your Glimbo trades (`Research[12][slot]` in the save), times 1 + Glimbo BOGO Offer + Glimbo
/// VIP Pass where `Research[29][slot]` is 1. Every raise counts as if you had it.
///
/// Cost in coins, N.js `VaultUpgCost`, before your discounts: (level + (col 1 + level) ×
/// col 2^level), × 1.1^(i - 61) past row 61. A row opens once all vault levels together reach
/// col 6 (`VaultUpgUNLOCKED`).
fn vault_cost(row: &Value, i: usize, level: f64) -> f64 {
    let base = level + (cell(row, 1) + level) * cell(row, 2).powf(level);
    if i > 61 { base * 1.1f64.powf(i as f64 - 61.0) } else { base }
}

fn upgrade_vault(save: &Save) -> Mechanic {
    let levels = save.numbers("UpgVault", &[]);
    let coins = richest(save);
    let table = rows("UpgradeVault", &[]);
    let total: f64 = levels.iter().take(table.len()).filter(|l| l.is_finite()).sum();
    let trades = save.numbers("Research", &[12]);
    let list = |i: usize| rows("Research", &[i]).iter().map(|v| num(v) as usize).collect::<Vec<usize>>();
    let (glimbo, scaled, awnings) = (list(26), list(29), list(63));
    let bogo = rows("ResGridSquares", &[]).get(169).map_or(0.0, |r| cell(r, 1));
    let items = table
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let base = cell(r, 4);
            let mut max = base;
            if awnings.contains(&i) {
                max += 25.0;
            }
            if let Some(slot) = glimbo.iter().position(|&g| g == i) {
                let mult = if scaled.get(slot) == Some(&1) { 1.0 + bogo + 1.0 } else { 1.0 };
                max += 10.0 + mult * at(&trades, slot);
            } else if base >= 2.0 {
                max += 10.0;
            }
            let mut it = Item::new(vault_name(text(r, 0)), pic(format!("VaultUpg{i}")), at(&levels, i), max);
            if total < cell(r, 6) {
                it.how = Some(format!("Opens at {} total vault levels. You have {}.", cell(r, 6), total));
                it.state = State::Locked;
            } else if !it.done() {
                let cost = vault_cost(r, i, it.level);
                it.how = Some(format!("Next level costs {} coins.", big(cost)));
                if cost <= coins {
                    it.state = State::Ready;
                }
            }
            it
        })
        .collect::<Vec<Item>>();
    let text = reach_text(&items, coins, |i, lv| if items[i].state == State::Locked { f64::INFINITY } else { table.get(i).map_or(f64::INFINITY, |r| vault_cost(r, i, lv)) }, "Not enough for the next level of any open upgrade yet.");
    let mut m = mech("upgrade-vault", 1, "Upgrade Vault", pic("VaultUpg0".into()), Count::Levels, "levels", items);
    m.have = Some(coin_have(coins, format!("{text} Costs are before your discounts, so you get a little further.")));
    let awn_n = awnings.iter().filter(|&&i| i < table.len()).count();
    // Exactly which upgrades each raise skips, by name.
    let names_where = |skip: &dyn Fn(usize, &Value) -> bool| table.iter().enumerate().filter(|(i, r)| skip(*i, r)).map(|(_, r)| vault_name(super::text(r, 0))).collect::<Vec<String>>().join(", ");
    let one_level = names_where(&|i, r| cell(r, 4) < 2.0 && !glimbo.contains(&i));
    let no_awnings = names_where(&|i, r| !awnings.contains(&i) && !(cell(r, 4) < 2.0 && !glimbo.contains(&i)));
    let scaled_n = scaled.iter().filter(|&&f| f == 1).count();
    m.note = Some(format!(
        "Each max counts every raise below as owned. Glimbo trades (W7) add max levels to the {} upgrades Glimbo trades for, one level per trade, more on {} of them with the VIP Pass and BOGO Offer.",
        glimbo.len(),
        scaled_n
    ));
    let bundle = save.get("bon_u").map(num).unwrap_or(0.0) >= 1.0 || save.get("BundlesReceived").and_then(|b| b.get("bon_u")).map(num).unwrap_or(0.0) >= 1.0;
    let grid = at(&save.numbers("Research", &[0]), 169);
    m.raises = vec![
        Raise {
            name: "Gilded Awnings, event shop".into(),
            amount: format!("+25 max levels on {awn_n} of the {} upgrades: all but the one-level unlocks ({one_level}) and {no_awnings}", table.len()),
            owned: super::general::event_shop_owned(save, 65),
        },
        Raise {
            name: "Potluck Pack bundle".into(),
            amount: format!("+10 max levels on every upgrade except the one-level unlocks ({one_level})"),
            owned: bundle,
        },
        Raise {
            name: "Glimbo VIP Pass, event shop".into(),
            amount: format!("+1 more max level per Glimbo trade on {scaled_n} Glimbo upgrades (2 a trade instead of 1)"),
            owned: super::general::event_shop_owned(save, 40),
        },
        Raise {
            name: "Glimbo BOGO Offer, W7 research grid".into(),
            amount: format!("+1 more max level per Glimbo trade on those {scaled_n} upgrades, for each of its {bogo:.0} levels"),
            owned: grid >= bogo && bogo > 0.0,
        },
    ];
    m
}
