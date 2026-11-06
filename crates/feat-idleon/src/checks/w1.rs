//! World 1: Blunder Hills.

use bot_core::serde_json::Value;

use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![
        statues(save),
        bribes(save),
        star_signs(save),
        stamps(save),
        statue_flair(save),
        owl(save),
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

/// Statue Flair: `RoyalG[22][i]` per statue. N.js `SF_maxLV` is 3, plus 1 with Menagerie
/// unlock 17; the system itself opens with the Statue Flair upgrade in the Armory.
fn statue_flair(save: &Save) -> Mechanic {
    let levels = save.numbers("RoyalG", &[22]);
    let items = rows("StatueInfo", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| Item::new(format!("{} Statue", label(text(r, 0))), pic(format!("Statue{}", i + 1)), at(&levels, i), 4.0))
        .collect();
    // World 7: flair is unlocked from the W7 armory, so it is weighed against W7 progress.
    let mut m = mech("statue-flair", 7, "Statue Flair", pic("Statue1".into()), Count::Levels, "levels", items);
    let fourth = if super::w7::menagerie_unlock(save, 17) { "which you have" } else { "which you don't have yet" };
    m.note = Some(format!("Opens with the Statue Flair upgrade in the Armory. Each statue goes to 3, and to 4 with the Menagerie unlock that raises Flair, {fourth}."));
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
        .map(|(i, n)| Item::new(format!("{} Megafeather", n.as_str().unwrap_or("")), pic(format!("Feaz{i}")), f64::from(owned > i as f64), 1.0))
        .collect();
    let mut m = mech("owl", 1, "Owl", pic("Feaz0".into()), Count::Done, "earned", items);
    let orion = save.option(255);
    m.note = Some(format!(
        "You have {owned:.0} Megafeathers and {orion:.0} Bonuses of Orion. Past the tenth, Fractal Megafeathers and Bonuses of Orion keep going with no cap."
    ));
    m
}

/// Forge upgrades: `ForgeLV[i]`; the game stops each at `RANDOlist[42][i]` and shows
/// "-MAX_LEVEL-". Names are the forge page's own text.
fn forge(save: &Save) -> Mechanic {
    let levels = save.numbers("ForgeLV", &[]);
    let names = GAME.extra("w1").get("forge_upgrades").and_then(Value::as_array).cloned().unwrap_or_default();
    let maxes = rows("RANDOlist", &[42]);
    let items = maxes
        .iter()
        .enumerate()
        .map(|(i, mx)| {
            let name = names.get(i).and_then(Value::as_str).map(label).unwrap_or_else(|| format!("Forge upgrade {}", i + 1));
            Item::new(name, None, at(&levels, i), num(mx))
        })
        .collect();
    mech("forge", 1, "Forge Upgrades", pic("ForgeA".into()), Count::Levels, "levels", items)
}

/// Anvil points: `AnvilPAstats_i[1]` bought with coins and `[2]` with monster parts. The buy
/// buttons stop at 600 and 700 (read from the game's code by the extractor).
fn anvil_points(save: &Save) -> Mechanic {
    let caps = GAME.extra("w1").get("anvil_points").map(crate::save::numbers).unwrap_or_default();
    let max = at(&caps, 0) + at(&caps, 1);
    let items = (0..characters(save))
        .map(|i| {
            let s = save.numbers(&format!("AnvilPAstats_{i}"), &[]);
            Item::new(format!("{}: Anvil points", char_name(save, i)), class_icon(save, i), at(&s, 1) + at(&s, 2), max)
        })
        .collect();
    let mut m = mech("anvil-points", 1, "Anvil Points", None, Count::Levels, "points", items);
    m.note = Some(format!("Each character can buy {:.0} points with coins and {:.0} with monster parts.", at(&caps, 0), at(&caps, 1)));
    m
}

/// Basketball (Hoops) and Darts shops: levels at `OptLacc[first + i]`. The cost goes up
/// forever and nothing stops a level, so these have no max.
fn minigame(save: &Save, key: &'static str, name: &'static str, extra: &str, picture: &str) -> Mechanic {
    let shop = GAME.extra("w1").get(extra);
    let first = shop.and_then(|s| s.get("first")).map(num).unwrap_or(0.0) as usize;
    let names = shop.and_then(|s| s.get("upgrades")).and_then(Value::as_array).cloned().unwrap_or_default();
    let items = names
        .iter()
        .enumerate()
        .map(|(i, d)| Item::new(label(&d.as_str().unwrap_or("").replace("}x_", "")), pic(format!("{picture}{i}")), save.option(first + i), 0.0))
        .collect();
    let mut m = mech(key, 1, name, pic(format!("{picture}0")), Count::Open, "levels", items);
    m.note = Some("These upgrades have no cap.".into());
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
fn upgrade_vault(save: &Save) -> Mechanic {
    let levels = save.numbers("UpgVault", &[]);
    let trades = save.numbers("Research", &[12]);
    let list = |i: usize| rows("Research", &[i]).iter().map(|v| num(v) as usize).collect::<Vec<usize>>();
    let (glimbo, scaled, awnings) = (list(26), list(29), list(63));
    let bogo = rows("ResGridSquares", &[]).get(169).map_or(0.0, |r| cell(r, 1));
    let items = rows("UpgradeVault", &[])
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
            Item::new(vault_name(text(r, 0)), pic(format!("VaultUpg{i}")), at(&levels, i), max)
        })
        .collect();
    let mut m = mech("upgrade-vault", 1, "Upgrade Vault", pic("VaultUpg0".into()), Count::Levels, "levels", items);
    m.note = Some("Caps include +25 from Gilded Awnings in the event shop, +10 from the Potluck Pack, and what your Glimbo trades add with Glimbo BOGO Offer and the Glimbo VIP Pass.".into());
    m
}
