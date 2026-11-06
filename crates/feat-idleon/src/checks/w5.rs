//! World 5: Smolderin' Plateau.

use bot_core::serde_json::Value;

use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![
        artifacts(save),
        gods(save),
        blessings(save),
        god_links(save),
        slab(save),
        superbits(save),
        snail(save),
        islands(save),
        boats(save),
    ]
}

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

/// Blessings: `Divinity[28 + god]`. The max (N.js `BlessingMaxLV`) is 100 plus five raises:
/// Coral Kid upgrade 1 (+2 a level, `OptLacc[428]`, no cap), Minehead bonus 9
/// (`Research` table [20][9]), vault upgrade 76 (1 a level, its max is `UpgradeVault` col 4
/// plus 10 from a bundle) and the Jelly Operator's RoG bonuses 49 and 63 (`Research` table
/// [47]). The capped raises count at their full value; the Coral Kid one, which has no cap, at
/// the level this account has bought.
fn blessings(save: &Save) -> Mechanic {
    let levels = save.numbers("Divinity", &[]);
    let coral = save.option(428).max(0.0);
    let minehead = rows("Research", &[20]).get(9).map_or(0.0, |v| num(v));
    let vault = rows("UpgradeVault", &[]).get(76).map_or(0.0, |r| cell(r, 4) + 10.0);
    let jelly: f64 = [49, 63].iter().map(|&i| rows("Research", &[47]).get(i).map_or(0.0, |v| num(v))).sum();
    let max = 100.0 + 2.0 * coral + minehead + vault + jelly;
    let items = rows("GodsInfo", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| Item::new(format!("{} Blessing", label(text(r, 0))), pic(format!("DivGod{i}")), at(&levels, 28 + i), max))
        .collect();
    let mut m = mech("blessings", 5, "Divinity Blessings", pic("DivGod0".into()), Count::Levels, "levels", items);
    m.note = Some(format!(
        "The cap is 100, raised by {minehead} from a Minehead bonus, {vault} from the Big Blessings vault upgrade, {jelly} from two Jelly Operator bonuses, and 2 per Coral Kid upgrade level. That last one has no cap, so the max here counts the {coral} levels you have."
    ));
    m
}

/// God links: `Divinity[12 + character]` is the god a character is linked to, -1 for none.
/// An unlinked character gets no link bonus at all.
fn god_links(save: &Save) -> Mechanic {
    let gods = rows("GodsInfo", &[]);
    let n = save.characters().max(1);
    let items = (0..n)
        .map(|c| {
            let god = save.at("Divinity", &[12 + c]).map(num).unwrap_or(-1.0);
            let linked = save.get("Divinity").is_some() && god >= 0.0 && (god as usize) < gods.len();
            let who = save.names.get(c).cloned().unwrap_or_else(|| format!("Character {}", c + 1));
            let mut it = Item::new(format!("{who}: god link"), linked.then(|| format!("DivGod{}", god as usize)), f64::from(linked), 1.0);
            it.level_name = Some(if linked { label(text(gods[god as usize], 0)) } else { "None".into() });
            it
        })
        .collect();
    mech("god-links", 5, "God Links", pic("DivGod0".into()), Count::Done, "linked", items)
}

/// The Slab: items ever looted (`Cards1`) against every item it lists.
fn slab(save: &Save) -> Mechanic {
    let found = save.get("Cards1").and_then(Value::as_array).map_or(0, Vec::len) as f64;
    let total = rows("SlabItemSort", &[]).len() as f64;
    let items = vec![Item::new("Items looted", None, found.min(total), total)];
    mech("slab", 5, "The Slab", None, Count::Levels, "items", items)
}

/// The game's `Number2Letter` list: superbit `i` is bought when this list's `i`th character is
/// in `Gaming[12]` (N.js `SuperBitType`). The list is set at runtime, not in N.js; this order is
/// the one every save's string follows.
const NUMBER_TO_LETTER: &str = "_abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ肥肢肖肋肉職耐者箱管算箔策答筒筍白発癒";

/// Gaming superbits: `GamingUpg` row i (col 3 the name, "Line|Line"; cols 1 and 2 the cost as
/// mantissa and power of ten). Rows costing 10^150 bits are the game's joke rows that say they
/// can't be bought.
fn superbits(save: &Save) -> Mechanic {
    let owned = save.at("Gaming", &[12]).and_then(Value::as_str).unwrap_or("");
    let letters: Vec<char> = NUMBER_TO_LETTER.chars().collect();
    let mut seen: Vec<String> = Vec::new();
    let items = rows("GamingUpg", &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| cell(r, 2) < 150.0)
        .map(|(i, r)| {
            let base = label(&text(r, 3).replace('|', "_"));
            let repeat = seen.iter().filter(|s| **s == base).count();
            seen.push(base.clone());
            let name = if repeat == 0 { base } else { format!("{base} {}", ["II", "III", "IV", "V", "VI", "VII"][(repeat - 1).min(5)]) };
            let has = letters.get(i).is_some_and(|c| owned.contains(*c));
            Item::new(name, None, f64::from(has), 1.0)
        })
        .collect();
    mech("superbits", 5, "Gaming Superbits", None, Count::Done, "bought", items)
}

/// The Immortal Snail: `GamingSprout[32][1]` is its level. N.js `MaxSnailLV` is 25, +10, +5 and
/// +10 from superbits 29, 30 and 40, so 50; but from level 49 the success chance
/// (`SnailStuff` 0: 1 - 0.6 x (level - 24)^0.16, times boosts) is zero, so 49 is the top.
fn snail(save: &Save) -> Mechanic {
    let level = save.at("GamingSprout", &[32, 1]).map(num).unwrap_or(0.0);
    let items = vec![Item::new("Immortal Snail", None, level, 49.0)];
    let mut m = mech("snail", 5, "Immortal Snail", None, Count::Levels, "levels", items);
    m.note = Some("The cap starts at 25. Three superbits raise it to 50, but no snail can level past 49: the chance at 49 is zero.".into());
    m
}

/// Island names as the sailing map shows them; the game's `IslandInfo` calls every one "Sahara".
const ISLANDS: [&str; 17] = [
    "Safari Island", "Beachy Coast", "Isolated Woods", "Rocky Peaks", "Stormy North", "Snowy South", "Toxic Bay Inc", "Candied Island", "Fungi Meadows",
    "Cloudy Quay", "Dungeon Cove", "Crystal Enclave", "Petulent Garage", "Isle of Note", "The Edge", "World's End", "The Maw",
];

/// Sailing islands: `Sailing[0][i]` is -1 once discovered, otherwise the clouds cleared so far.
/// Safari Island is discovered from the start, so it isn't counted.
fn islands(save: &Save) -> Mechanic {
    let state = save.numbers("Sailing", &[0]);
    let items = rows("IslandInfo", &[])
        .iter()
        .enumerate()
        .skip(1)
        .map(|(i, _)| {
            let name = ISLANDS.get(i).map_or_else(|| format!("Island {}", i + 1), |s| (*s).to_string());
            Item::new(name, None, f64::from(state.len() > i && at(&state, i) == -1.0), 1.0)
        })
        .collect();
    mech("islands", 5, "Sailing Islands", pic("SailingNewIsle1".into()), Count::Done, "discovered", items)
}

/// Boats: `Sailing[2][1]` is the boats bought after the free first one; the harbor has a place
/// for each boat the game allows (`BoatsInHarbor`).
fn boats(save: &Save) -> Mechanic {
    let bought = save.at("Sailing", &[2, 1]).map(num).unwrap_or(0.0);
    let max = rows("BoatsInHarbor", &[]).len().saturating_sub(1) as f64;
    let items = vec![Item::new("Boats bought", None, bought, max)];
    mech("boats", 5, "Sailing Boats", None, Count::Levels, "boats", items)
}
