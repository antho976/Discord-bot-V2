//! One check per mechanic: read the save, compare with the game's maxes.
//!
//! Where the save lives and which table column holds the max was worked out from the game's
//! code and checked against seven saves (four from 2.43/2.45, three current ones with World 7).
//! A max is always the highest the game allows: where something raises a cap (a gaming
//! superbit, a Menagerie unlock), the check uses the raised cap and says what raises it, so
//! the share is the real distance to done.

use bot_core::serde_json::Value;

use crate::game::{GAME, num, words};
use crate::review::{Count, Item, Mechanic};
use crate::save::Save;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![
        statues(save),
        bribes(save),
        star_signs(save),
        stamps(save),
        vials(save),
        sigils(save),
        arcade(save),
        salt_lick(save),
        prayers(save),
        atoms(save),
        jewels(save),
        artifacts(save),
        gods(save),
        slab(save),
        crops(save),
        summoning(save),
        research_grid(save),
        capped(save, "sushi", 7, "Sushi Station", "SushiUPG", "Sushi", &[2], "SushiUpg"),
        capped(save, "jelly", 7, "Jelly Operator", "JellyUPG", "Research", &[17], "JellyUpg"),
        capped(save, "minehead", 7, "Minehead", "MineheadUPG", "Research", &[8], "MineUpg"),
        coral_reef(save),
        zenith_market(save),
        legend_talents(save),
        spelunk_upgrades(save),
        cards(save),
        achievements(save),
    ]
}

// --- reading tables -------------------------------------------------------------------------

/// `table[path..]`: the rows of a table, or of a list inside it.
fn rows(table: &str, path: &[usize]) -> Vec<&'static Value> {
    let mut v: Vec<&'static Value> = GAME.rows(table).iter().collect();
    for &i in path {
        v = v.get(i).and_then(|x| x.as_array()).map(|a| a.iter().collect()).unwrap_or_default();
    }
    v
}

fn text(row: &Value, col: usize) -> &str {
    row.get(col).and_then(Value::as_str).unwrap_or("")
}

fn cell(row: &Value, col: usize) -> f64 {
    row.get(col).map(num).unwrap_or(0.0)
}

/// The game's text made readable: "+{%_Sample_Size" is "Sample Size", "COPPER_CORONA" is
/// "Copper Corona". `{`, `}`, `$` and `^` stand for numbers the game fills in; `@` is a line break.
fn label(s: &str) -> String {
    let s = words(s).replace('@', " ");
    let s: String = s.chars().filter(|c| !matches!(c, '{' | '}' | '$' | '^' | '|')).collect();
    let s = s.trim_start_matches(['+', '%', ' ']);
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.trim_end_matches(['!', '.']).to_string();
    if s.chars().any(char::is_lowercase) {
        return s;
    }
    s.split(' ')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c.flat_map(char::to_lowercase)).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn pic(p: String) -> Option<String> {
    Some(p)
}

fn mech(key: &'static str, world: u8, name: &'static str, picture: Option<String>, count: Count, unit: &'static str, items: Vec<Item>) -> Mechanic {
    Mechanic { key, world, name, picture, count, unit, items, note: None }
}

fn at(v: &[f64], i: usize) -> f64 {
    v.get(i).copied().unwrap_or(0.0)
}

// --- World 1 --------------------------------------------------------------------------------

const STATUE_TIERS: [&str; 4] = ["Normal", "Gold", "Onyx", "Zenith"];

/// Statues: `StuG[i]` is the tier, 0 Normal to 3 Zenith. Levels have no cap, so done is Zenith.
fn statues(save: &Save) -> Mechanic {
    let tiers = save.numbers("StuG", &[]);
    let items = rows("StatueInfo", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let tier = at(&tiers, i).clamp(0.0, 3.0);
            let prefix = ["Statue", "StatueG", "StatueO", "StatueZ"][tier as usize];
            let mut it = Item::new(format!("{} Statue", label(text(r, 0))), pic(format!("{prefix}{}", i + 1)), tier, 3.0);
            it.level_name = Some(STATUE_TIERS[tier as usize].into());
            it
        })
        .collect();
    let mut m = mech("statues", 1, "Statues", pic("StatueZ1".into()), Count::Levels, "tiers", items);
    m.note = Some("Each statue goes Normal, Gold, Onyx, then Zenith. Levels keep going, so Zenith counts as done.".into());
    m
}

/// Bribes: `BribeStatus[i]` is 1 when bought. The last row is the game's "no more bribes".
fn bribes(save: &Save) -> Mechanic {
    let status = save.numbers("BribeStatus", &[]);
    let table = rows("BribeDescriptions", &[]);
    let real = table.len().saturating_sub(1);
    let items = table
        .iter()
        .take(real)
        .enumerate()
        .map(|(i, r)| Item::new(label(text(r, 0)), pic(format!("BribeO{}", text(r, 3))), f64::from(at(&status, i) >= 1.0), 1.0))
        .collect();
    mech("bribes", 1, "Bribes", pic("BribeOpigbank".into()), Count::Done, "bought", items)
}

/// Star signs: `StarSg` maps a sign's name to 1 once found. The obtainable ones are those the
/// star map places (`SSignInfoUI` col 4 isn't 999).
fn star_signs(save: &Save) -> Mechanic {
    let found = save.get("StarSg").and_then(Value::as_object);
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
            Item::new(label(name), None, f64::from(on), 1.0)
        })
        .collect();
    mech("star-signs", 1, "Star Signs", None, Count::Done, "found", items)
}

/// Stamps: `StampLv[tab][i]`. No hard max, so this counts stamps collected; the level shows
/// beside each.
fn stamps(save: &Save) -> Mechanic {
    let mut items = Vec::new();
    for (tab, letter) in ["A", "B", "C"].iter().enumerate() {
        let levels = save.numbers("StampLv", &[tab]);
        for i in 0..rows("StampDescriptions", &[tab]).len() {
            let id = format!("Stamp{letter}{}", i + 1);
            let lv = at(&levels, i);
            let name = GAME.item_name(&id).map(str::to_string).unwrap_or_else(|| id.clone());
            let mut it = Item::new(name, pic(id), f64::from(lv > 0.0), 1.0);
            it.level_name = Some(if lv > 0.0 { format!("Lv {lv:.0}") } else { "Not found".into() });
            items.push(it);
        }
    }
    let mut m = mech("stamps", 1, "Stamps", pic("StampA1".into()), Count::Done, "collected", items);
    m.note = Some("Stamps have no level cap, so this counts the ones you have.".into());
    m
}

// --- World 2 --------------------------------------------------------------------------------

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

// --- World 3 --------------------------------------------------------------------------------

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

/// The atom cap, fully raised: 20, +10 (gaming superbit), +20 (compass), +20 (event shop), +1
/// (Menagerie).
const ATOM_MAX: f64 = 71.0;

/// Atom Collider: `Atoms[i]`, one cap for every atom.
fn atoms(save: &Save) -> Mechanic {
    let levels = save.numbers("Atoms", &[]);
    let items = rows("AtomInfo", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let name = text(r, 0).split("_-_").next().unwrap_or("");
            Item::new(label(name), pic(format!("Atom{i}")), at(&levels, i), ATOM_MAX)
        })
        .collect();
    let mut m = mech("atoms", 3, "Atom Collider", pic("Atom0".into()), Count::Levels, "levels", items);
    m.note = Some("The cap starts at 20. A gaming superbit, a compass upgrade, an event shop item and a Menagerie boost raise it to 71.".into());
    m
}

// --- World 4 --------------------------------------------------------------------------------

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

// --- World 5 --------------------------------------------------------------------------------

const ARTIFACT_TIERS: [&str; 8] = ["Not found", "Found", "Ancient", "Eldritch", "Sovereign", "Omnipotent", "Transcendent", "Verisimilitude"];

/// Sailing artifacts: `Sailing[3][i]` is the tier, 0 not found to 7 Verisimilitude.
fn artifacts(save: &Save) -> Mechanic {
    let tiers = save.numbers("Sailing", &[3]);
    let items = rows("ArtifactInfo", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let tier = at(&tiers, i).clamp(0.0, 7.0);
            let mut it = Item::new(label(text(r, 0)), pic(format!("Arti{i}")), tier, 7.0);
            it.level_name = Some(ARTIFACT_TIERS[tier as usize].into());
            it
        })
        .collect();
    mech("artifacts", 5, "Artifacts", pic("Arti0".into()), Count::Levels, "tiers", items)
}

/// Gods: `Divinity[25]` counts the gods unlocked (above 10 it's the god rank).
fn gods(save: &Save) -> Mechanic {
    let unlocked = save.at("Divinity", &[25]).map(num).unwrap_or(0.0);
    let items = rows("GodsInfo", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| Item::new(label(text(r, 0)), pic(format!("DivGod{i}")), f64::from((i as f64) < unlocked), 1.0))
        .collect();
    mech("gods", 5, "Divinity Gods", pic("DivGod0".into()), Count::Done, "unlocked", items)
}

/// The Slab: items ever looted (`Cards1`) against every item it lists.
fn slab(save: &Save) -> Mechanic {
    let found = save.get("Cards1").and_then(Value::as_array).map_or(0, Vec::len) as f64;
    let total = rows("SlabItemSort", &[]).len() as f64;
    let items = vec![Item::new("Items looted", None, found.min(total), total)];
    mech("slab", 5, "The Slab", None, Count::Levels, "items", items)
}

// --- World 6 --------------------------------------------------------------------------------

/// Crops discovered: the keys in `FarmCrop` (a bean trade resets counts but keeps the keys).
fn crops(save: &Save) -> Mechanic {
    let found = save.get("FarmCrop").and_then(Value::as_object).map_or(0, |o| o.len()) as f64;
    let total = rows("SeedInfo", &[]).last().map_or(0.0, |r| cell(r, 3) + 1.0);
    let items = vec![Item::new("Crops discovered", pic("FarmCrop0".into()), found.min(total), total)];
    mech("crops", 6, "Farming Crops", pic("FarmCrop0".into()), Count::Levels, "crops", items)
}

/// Summoning upgrades: `Summon[0][i]`; `SummonUPG` col 8 is the max, 9999 for no cap.
fn summoning(save: &Save) -> Mechanic {
    let levels = save.numbers("Summon", &[0]);
    let items = rows("SummonUPG", &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| cell(r, 8) > 0.0 && cell(r, 8) < 999.0)
        .map(|(i, r)| Item::new(label(text(r, 3)), None, at(&levels, i), cell(r, 8)))
        .collect();
    let mut m = mech("summoning", 6, "Summoning Upgrades", None, Count::Levels, "levels", items);
    m.note = Some("Only the upgrades with a cap. The rest level up without end.".into());
    m
}

// --- World 7 --------------------------------------------------------------------------------

/// Research grid: `Research[0][i]`; `ResGridSquares` col 1 is the max. Rows named "Name" are
/// empty squares.
fn research_grid(save: &Save) -> Mechanic {
    let levels = save.numbers("Research", &[0]);
    let items = rows("ResGridSquares", &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| text(r, 0) != "Name" && cell(r, 1) > 0.0)
        .map(|(i, r)| Item::new(label(text(r, 0)), None, at(&levels, i), cell(r, 1)))
        .collect();
    mech("research", 7, "Research Grid", None, Count::Levels, "levels", items)
}

/// Upgrade shops kept the same way: levels at `key[path][i]`, max in col 1 (0 for a row that
/// isn't bought, 999 and over for no cap). Only capped upgrades count.
#[allow(clippy::too_many_arguments)]
fn capped(save: &Save, key: &'static str, world: u8, name: &'static str, table: &str, save_key: &str, path: &[usize], picture: &str) -> Mechanic {
    let levels = save.numbers(save_key, path);
    let items: Vec<Item> = rows(table, &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| cell(r, 1) > 0.0 && cell(r, 1) < 999.0)
        .map(|(i, r)| Item::new(label(text(r, 0)), pic(format!("{picture}{i}")), at(&levels, i), cell(r, 1)))
        .collect();
    let mut m = mech(key, world, name, pic(format!("{picture}0")), Count::Levels, "levels", items);
    m.note = Some("Only the upgrades with a cap. The rest level up without end.".into());
    m
}

/// Coral Reef: `Spelunk[13][i]`; col 1 is the max. Row 3's cap is raised from 12 to 15 by a
/// Menagerie unlock; the last row is a filler.
fn coral_reef(save: &Save) -> Mechanic {
    let levels = save.numbers("Spelunk", &[13]);
    let letters = ['z', 'A', 'B', 'C', 'D'];
    let items = rows("CoralReef", &[])
        .iter()
        .enumerate()
        .take(5)
        .map(|(i, r)| {
            let max = if i == 3 { 15.0 } else { cell(r, 1) };
            Item::new(label(text(r, 0)), pic(format!("Reef{}1", letters[i])), at(&levels, i), max)
        })
        .collect();
    mech("coral-reef", 7, "Coral Reef", pic("Reefz1".into()), Count::Levels, "levels", items)
}

/// Zenith Market: `Spelunk[45][i]`; col 3 is the max.
fn zenith_market(save: &Save) -> Mechanic {
    let levels = save.numbers("Spelunk", &[45]);
    let items = rows("ZenithMarket", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| Item::new(label(text(r, 0)), None, at(&levels, i), cell(r, 3)))
        .collect();
    mech("zenith", 7, "Zenith Market", None, Count::Levels, "levels", items)
}

/// Legend Talents: `Spelunk[18][i]`. The max is col 1, raised by four research nodes (each for
/// five talents, by its own max) and by 3 for nine talents with a Menagerie unlock.
fn legend_talents(save: &Save) -> Mechanic {
    let levels = save.numbers("Spelunk", &[18]);
    let grid = rows("ResGridSquares", &[]);
    let node_max = |n: usize| grid.get(n).map_or(0.0, |r| cell(r, 1));
    let raises: [(usize, [usize; 5]); 4] = [(130, [2, 13, 3, 18, 5]), (131, [9, 23, 33, 1, 16]), (132, [20, 31, 19, 27, 8]), (152, [34, 30, 29, 6, 26])];
    let menagerie = [35, 33, 36, 0, 20, 31, 15, 32, 6];
    // The talent shown in each icon slot: `Spelunky[26][k]` is the talent index.
    let order = rows("Spelunky", &[26]);
    let icon_for = |i: usize| order.iter().position(|v| num(v) as usize == i).map(|k| format!("LegendTalentIcon{k}"));
    let runtime_names = ["Minehead My Beloved", "Two Time Jelly Operation"];
    let items = rows("LegendTalents", &[])
        .iter()
        .enumerate()
        .take(42)
        .map(|(i, r)| {
            let mut max = cell(r, 1);
            for (node, talents) in &raises {
                if talents.contains(&i) {
                    max += node_max(*node);
                }
            }
            if menagerie.contains(&i) {
                max += 3.0;
            }
            let name = if i >= 40 { runtime_names[i - 40].to_string() } else { label(text(r, 0)) };
            Item::new(name, icon_for(i), at(&levels, i), max)
        })
        .collect();
    let mut m = mech("legend-talents", 7, "Legend Talents", pic("LegendTalentIcon0".into()), Count::Levels, "levels", items);
    m.note = Some("Caps include what research nodes and the Menagerie add.".into());
    m
}

/// Spelunking shop: `Spelunk[5][i]` (-1 is locked); `SpelunkUpg` col 3 is the max, 99999 for no cap.
fn spelunk_upgrades(save: &Save) -> Mechanic {
    let levels = save.numbers("Spelunk", &[5]);
    let items = rows("SpelunkUpg", &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| cell(r, 3) > 0.0 && cell(r, 3) < 999.0)
        .map(|(i, r)| Item::new(label(text(r, 0)), pic(format!("CaveShopUpg{i}")), at(&levels, i).max(0.0), cell(r, 3)))
        .collect();
    let mut m = mech("spelunk-shop", 7, "Spelunking Shop", pic("CaveShopUpg0".into()), Count::Levels, "levels", items);
    m.note = Some("Only the upgrades with a cap. The rest level up without end.".into());
    m
}

// --- Account-wide ---------------------------------------------------------------------------

/// The most stars a card can have: 4, then 5 with the Rift unlock and 6 with Spelunking's.
const CARD_STARS: usize = 6;

/// Stars for `count` cards of a card whose first star costs `base` (the game's `CardLv`).
fn card_stars(count: f64, base: f64, chizoar: bool) -> f64 {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_read_like_the_game() {
        assert_eq!(label("+{%_Sample_Size"), "Sample Size");
        assert_eq!(label("COPPER_CORONA"), "Copper Corona");
        assert_eq!(label("Big_Brain_Time"), "Big Brain Time");
    }

    #[test]
    fn card_stars_follow_the_game() {
        // Green Mushroom: 5 cards for the first star.
        assert_eq!(card_stars(0.0, 5.0, false), 0.0);
        assert_eq!(card_stars(1.0, 5.0, false), 0.0);
        assert_eq!(card_stars(6.0, 5.0, false), 1.0);
        assert_eq!(card_stars(21.0, 5.0, false), 2.0);
    }

    #[test]
    fn every_check_runs_on_an_empty_save() {
        let s = Save::parse(r#"{"OptLacc": []}"#).unwrap();
        let ms = all(&s);
        assert!(ms.len() >= 25);
        for m in &ms {
            assert!(!m.items.is_empty(), "{} has no items", m.key);
            assert_eq!(m.have(), 0.0, "{} reads progress from nothing", m.key);
        }
    }
}
