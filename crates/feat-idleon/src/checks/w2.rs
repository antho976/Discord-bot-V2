//! World 2: Yum-Yum Desert.

use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![
        bubbles(save),
        cauldron_upgrades(save),
        pay2win(save),
        vials(save),
        sigils(save),
        arcade(save),
        post_office(save),
        islands(save),
        fractal_island(save),
        killroy(save),
    ]
}

const CAULDRONS: [&str; 4] = ["Power", "Quicc", "High-IQ", "Kazam"];

/// Bubbles: `CauldronInfo[c][i]` for the four cauldrons, matching `AlchemyDescription[c][i]`.
/// Rows named "BUBBLE" are placeholders, and the save keeps slots past the table; both read Lv 1
/// in the save and are left out. Bubbles have no level cap.
fn bubbles(save: &Save) -> Mechanic {
    let mut items = Vec::new();
    for (c, colour) in ['O', 'G', 'P', 'Y'].into_iter().enumerate() {
        let levels = save.numbers("CauldronInfo", &[c]);
        for (i, r) in rows("AlchemyDescription", &[c]).iter().enumerate() {
            if text(r, 0) == "BUBBLE" {
                continue;
            }
            items.push(Item::new(label(text(r, 0)), pic(format!("aUpgrades{colour}{i}")), at(&levels, i), 0.0));
        }
    }
    let mut m = mech("bubbles", 2, "Alchemy Bubbles", pic("aUpgradesO0".into()), Count::Open, "levels", items);
    m.note = Some("Bubbles level up without end, so these are your levels, lowest first.".into());
    m
}

const BREW: [&str; 4] = ["Speed", "Luck", "Cost", "Extra"];

/// Cauldron brew upgrades: `CauldUpgLVs` is 8 groups of 4; the first four groups are the
/// bubble cauldrons (speed, luck, cost, extra), capped at 170 by the level-up code
/// (`Math.min(170, ...)`), and the panel shows 170 and up as maxed. A repair the game runs on
/// load (when 15 of the 16 read 0) can set them to Alchemy level^1.07, past 170; those count as
/// 170. The last four groups are the liquids' cap and rate, which have no cap.
fn cauldron_upgrades(save: &Save) -> Mechanic {
    let levels = save.numbers("CauldUpgLVs", &[]);
    let items = (0..16)
        .map(|k| Item::new(format!("{} Cauldron: {}", CAULDRONS[k / 4], BREW[k % 4]), None, at(&levels, k).min(170.0), 170.0))
        .collect();
    let mut m = mech("cauldron-upgrades", 2, "Cauldron Upgrades", pic("aEmptyCauldronB0".into()), Count::Levels, "levels", items);
    m.note = Some("Speed, Luck, Cost and Extra on each bubble cauldron, levelled by brewing. The liquid cauldrons' Cap and Rate have no cap, so they are left out.".into());
    m
}

const LIQUIDS: [&str; 4] = ["Water Droplets", "Liquid Nitrogen", "Trench Seawater", "Toxic Mercury"];

/// Pay2Win tab: `CauldronP2W[0]` cauldron (speed, new bubble, boost req) x4, `[1]` liquid
/// (regen, capacity) x4, `[2]` vials (attempts, RNG), `[3]` player (alchemy speed, extra EXP).
/// Maxes from `cauldronp2wbonuses("CauldronLvMAX")`: 150/125/100, 100/80, 15/45, and the
/// highest Alchemy level of any character for the player upgrades. A locked liquid reads -1.
fn pay2win(save: &Save) -> Mechanic {
    let cauldron = save.numbers("CauldronP2W", &[0]);
    let liquid = save.numbers("CauldronP2W", &[1]);
    let vial = save.numbers("CauldronP2W", &[2]);
    let player = save.numbers("CauldronP2W", &[3]);
    let alchemy = (0..save.characters().max(12)).map(|i| at(&save.numbers(&format!("Lv0_{i}"), &[]), 5)).fold(0.0, f64::max);
    let mut items = Vec::new();
    for (c, name) in CAULDRONS.iter().enumerate() {
        for (k, (what, max)) in [("Speed", 150.0), ("New Bubble", 125.0), ("Boost Req", 100.0)].into_iter().enumerate() {
            items.push(Item::new(format!("{name} Cauldron: {what}"), None, at(&cauldron, 3 * c + k), max));
        }
    }
    for (l, name) in LIQUIDS.iter().enumerate() {
        for (k, (what, max)) in [("Regen", 100.0), ("Capacity", 80.0)].into_iter().enumerate() {
            items.push(Item::new(format!("{name}: {what}"), None, at(&liquid, 2 * l + k), max));
        }
    }
    items.push(Item::new("Vials: Attempts", None, at(&vial, 0), 15.0));
    items.push(Item::new("Vials: RNG", None, at(&vial, 1), 45.0));
    items.push(Item::new("Player: Alchemy Speed", None, at(&player, 0), alchemy));
    items.push(Item::new("Player: Extra EXP", None, at(&player, 1), alchemy));
    let mut m = mech("pay2win", 2, "Alchemy Pay2Win", pic("aEmptyCauldronB1".into()), Count::Levels, "levels", items);
    m.note = Some("The two Player upgrades cap at your highest Alchemy level, so they open up again each time it goes up.".into());
    m
}

/// Vials: `CauldronInfo[4][i]`, max level 13.
fn vials(save: &Save) -> Mechanic {
    let levels = save.numbers("CauldronInfo", &[4]);
    let items = rows("AlchemyDescription", &[4])
        .iter()
        .enumerate()
        .map(|(i, r)| Item::new(label(text(r, 0)), pic(text(r, 5).to_string()), at(&levels, i), 13.0))
        .collect();
    mech("vials", 2, "Vials", pic("aVials13".into()), Count::Levels, "levels", items)
}

const SIGIL_TIERS: [&str; 6] = ["Locked", "Blue", "Yellow", "Red", "Purple", "Green"];

/// Sigils: `CauldronP2W[4]` is [progress, tier] pairs; tier -1 locked to 4 green.
fn sigils(save: &Save) -> Mechanic {
    let p2w = save.numbers("CauldronP2W", &[4]);
    let items = rows("SigilDesc", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let tier = (p2w.get(2 * i + 1).copied().unwrap_or(-1.0) + 1.0).clamp(0.0, 5.0);
            let mut it = Item::new(label(text(r, 0)), pic(format!("aSiga{i}")), tier, 5.0);
            it.level_name = Some(SIGIL_TIERS[tier as usize].into());
            it
        })
        .collect();
    let mut m = mech("sigils", 2, "Sigils", pic("aSiga0".into()), Count::Levels, "tiers", items);
    m.tier_names = Some(SIGIL_TIERS[1..].iter().map(|s| s.to_string()).collect());
    m.note = Some("Blue, Yellow, Red, Purple, then Green. Red needs Ionized Sigils in the Emporium, Purple a Spelunking lore boss, Green the Eclectic Sigils research.".into());
    m
}

/// Arcade: `ArcadeUpg[i]`, max 102 (101 Cosmic, 102 Omega). The last six rows are fillers.
fn arcade(save: &Save) -> Mechanic {
    let levels = save.numbers("ArcadeUpg", &[]);
    let items = rows("ArcadeShopInfo", &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| !text(r, 0).contains("who_knows"))
        .map(|(i, r)| Item::new(label(text(r, 0)), pic(format!("PachiShopICON{i}")), at(&levels, i), 102.0))
        .collect();
    let mut m = mech("arcade", 2, "Arcade", pic("PachiShopICON0".into()), Count::Levels, "levels", items);
    m.note = Some("100 with gold balls, then 101 (Cosmic, 2x) and 102 (Omega, 6x) with their own balls.".into());
    m
}

/// Post Office: each character's `POu_<i>[box]`. The game only buys a level while it stays
/// under `PostOffUpgradeInfo` col 15 (400.1, 800.1, 100000.1), so the max is that, rounded down.
fn post_office(save: &Save) -> Mechanic {
    let boxes = rows("PostOffUpgradeInfo", &[]);
    let mut items = Vec::new();
    for c in 0..save.characters() {
        let levels = save.numbers(&format!("POu_{c}"), &[]);
        let who = character(save, c);
        for (i, r) in boxes.iter().enumerate() {
            items.push(Item::new(format!("{who}: {}", label(text(r, 0))), pic(format!("UIboxUpg{i}")), at(&levels, i), cell(r, 15).floor()));
        }
    }
    if items.is_empty() {
        items = boxes.iter().enumerate().map(|(i, r)| Item::new(label(text(r, 0)), pic(format!("UIboxUpg{i}")), 0.0, cell(r, 15).floor())).collect();
    }
    let mut m = mech("post-office", 2, "Post Office", pic("UIboxUpg0".into()), Count::Done, "maxed", items);
    m.note = Some("Every character has its own boxes, all paid from the same box points. Most cap at 400, the last three at 800 and the Myriad Crate at 100,000.".into());
    m
}

const ISLANDS: [&str; 6] = ["Trash", "Rando", "Crystal", "Seasalt", "Shimmer", "Fractal"];

/// Island Expeditions: `OptLacc[169]` holds a letter per island bought ("_abcde": `Number2Letter`
/// 0 to 5, in the order the game lists them).
fn islands(save: &Save) -> Mechanic {
    let owned = match save.at("OptLacc", &[169]) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    let items = ISLANDS
        .iter()
        .zip("_abcde".chars())
        .enumerate()
        .map(|(i, (name, letter))| Item::new(format!("{name} Island"), pic(format!("IslandSail{i}")), f64::from(owned.contains(letter)), 1.0))
        .collect();
    let mut m = mech("islands", 2, "Islands", pic("IslandSail0".into()), Count::Done, "unlocked", items);
    m.note = Some("Islands are bought with bottles from the fishing boat. The island shops' repeat buys have no cap and aren't counted.".into());
    m
}

/// Fractal Island: hours of Nothing AFK (`OptLacc[184]`) against the reward thresholds the game
/// lists on the island (24 to 60,000 hours).
fn fractal_island(save: &Save) -> Mechanic {
    const REWARDS: [(f64, &str); 8] = [
        (24.0, "Master of Nothing Trophy Chance"),
        (200.0, "Dungeon Credits and Flurbos"),
        (750.0, "Kitchen Upgrade Costs"),
        (2500.0, "Sailing Artifact Chance"),
        (10000.0, "Gold Nuggets from the Dirty Shovel"),
        (20000.0, "Star Talent Points"),
        (40000.0, "Ninja Twin Stealth"),
        (60000.0, "World 7 Bonus"),
    ];
    let hours = save.option(184);
    let items = REWARDS.iter().map(|&(need, name)| Item::new(name, None, hours.min(need), need)).collect();
    let mut m = mech("fractal-island", 2, "Fractal Island", pic("IslandSail5".into()), Count::Done, "unlocked", items);
    m.note = Some("Hours spent AFK on Nothing at Fractal Island unlock each reward. The last one is at 60,000 hours.".into());
    m
}

/// Killroy: `OptLacc[106..112]` count the fights won of each kind (timer, talent points, skulls,
/// respawn, dungeon credits, pearls), and the skull shop's permanent buys sit at
/// `OptLacc[228..231]` and `[467..472]`. None of them has a cap: each bonus grows with
/// diminishing returns.
fn killroy(save: &Save) -> Mechanic {
    let fights = ["Timer", "Talent Points", "Skulls", "Respawn", "Dungeon Credits", "Pearls"];
    let shop: [(usize, &str); 8] = [
        (228, "Artifact Find Chance"),
        (229, "Crop Evolution Chance"),
        (230, "Jade Gain"),
        (467, "Gallery Grade"),
        (468, "Masterclass Drops"),
        (469, "World 7 Skill EXP"),
        (470, "Daily Coral"),
        (471, "Mystery Boost"),
    ];
    let mut items: Vec<Item> = fights.iter().enumerate().map(|(i, n)| Item::new(format!("{n} fights"), None, save.option(106 + i), 0.0)).collect();
    items.extend(shop.iter().map(|&(i, n)| Item::new(format!("Skull Shop: {n}"), None, save.option(i), 0.0)));
    let mut m = mech("killroy", 2, "Killroy", pic("Quest69".into()), Count::Open, "levels", items);
    m.note = Some("Fights of each kind and permanent skull shop buys. None has a cap; each bonus grows with diminishing returns.".into());
    m
}

/// A character's name, or "Character 3" when the save has none.
fn character(save: &Save, i: usize) -> String {
    save.names.get(i).cloned().unwrap_or_else(|| format!("Character {}", i + 1))
}
