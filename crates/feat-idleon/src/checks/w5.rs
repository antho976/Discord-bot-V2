//! World 5: Smolderin' Plateau.

use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![
        artifacts(save),
        gods(save),
        blessings(save),
        slab(save),
        superbits(save),
        snail(save),
        islands(save),
        boats(save),
    ]
}

/// The game's `Number2Letter` list: superbit `i` is bought when this list's `i`th character is
/// in `Gaming[12]` (N.js `SuperBitType`), Jade Emporium upgrade `i` when it is in
/// `Ninja[102][9]` (`EmporiumBonus`). The list is set at runtime, not in N.js; this order is the
/// one every save's string follows.
const NUMBER_TO_LETTER: &str = "_abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ肥肢肖肋肉職耐者箱管算箔策答筒筍白発癒";

fn letter_in(s: &str, i: usize) -> bool {
    NUMBER_TO_LETTER.chars().nth(i).is_some_and(|c| s.contains(c))
}

fn superbit_owned(save: &Save, i: usize) -> bool {
    letter_in(save.at("Gaming", &[12]).and_then(Value::as_str).unwrap_or(""), i)
}

/// A research grid node has at least one level (`Research[0][i]`), which is what the game's
/// unlock checks on `Grid_Bonus` amount to.
fn grid_node(save: &Save, i: usize) -> bool {
    save.at("Research", &[0, i]).map(num).unwrap_or(0.0) >= 1.0
}

fn grid_name(i: usize) -> String {
    rows("ResGridSquares", &[]).get(i).map_or_else(|| format!("research node {i}"), |r| label(text(r, 0)))
}

/// Spelunking lore bonus `i` is in (`Spelunk[0][i]` at least 1, N.js `DoWeHaveLoreN1`).
fn lore(save: &Save, i: usize) -> bool {
    save.at("Spelunk", &[0, i]).map(num).unwrap_or(0.0) >= 1.0
}

fn value(save: &Save, key: &str, path: &[usize]) -> f64 {
    save.at(key, path).map(num).unwrap_or(0.0).max(0.0)
}

const ARTIFACT_TIERS: [&str; 8] = ["Not found", "Found", "Ancient", "Eldritch", "Sovereign", "Omnipotent", "Transcendent", "Verisimilitude"];

/// Sailing artifacts: `Sailing[3][i]` is the tier, 0 not found to 7 Verisimilitude. What opens
/// each form (N.js, the artifact tooltip): Eldritch once 30 rifts are done (`Rift[0]` over 29), Sovereign
/// with Jade Emporium 35, Omnipotent with Spelunking lore bonus 0 (`OmnipotentArtifactsUnlocked`),
/// Transcendent with research node 109 (`TranscendentArtifactsUnlocked`), Verisimilitude with
/// Menagerie unlock 0 (`VerisimilitudeUnlocked`).
fn artifacts(save: &Save) -> Mechanic {
    let tiers = save.numbers("Sailing", &[3]);
    let emporium = save.at("Ninja", &[102, 9]).and_then(Value::as_str).unwrap_or("");
    let gate = |tier: usize| -> Option<String> {
        match tier {
            3 if value(save, "Rift", &[0]) <= 29.0 => Some("Eldritch forms open once rift 30 is done.".into()),
            4 if !letter_in(emporium, 35) => Some("Sovereign forms come with the Jade Emporium's Sovereign Artifacts.".into()),
            5 if !lore(save, 0) => Some("Omnipotent forms come with Spelunking's first lore bonus.".into()),
            6 if !grid_node(save, 109) => Some(format!("Transcendent forms come with research node {}.", grid_name(109))),
            7 if !super::w7::menagerie_unlock(save, 0) => Some(format!("Verisimilitude forms come with {}.", super::w7::menagerie_unlock_name(0))),
            _ => None,
        }
    };
    let items = rows("ArtifactInfo", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let tier = at(&tiers, i).clamp(0.0, 7.0);
            let mut it = Item::new(label(text(r, 0)), pic(format!("Arti{i}")), tier, 7.0);
            it.level_name = Some(ARTIFACT_TIERS[tier as usize].into());
            let next = tier as usize + 1;
            if tier < 1.0 {
                it.how = Some("Found in sailing chests.".into());
            } else if next <= 7 {
                match gate(next) {
                    Some(why) => {
                        it.how = Some(why);
                        it.state = State::Locked;
                    }
                    None => it.how = Some(format!("Next: its {} form, from sailing chests.", ARTIFACT_TIERS[next])),
                }
            }
            it
        })
        .collect();
    let mut m = mech("artifacts", 5, "Artifacts", pic("Arti0".into()), Count::Levels, "tiers", items);
    m.tier_names = Some(ARTIFACT_TIERS[1..].iter().map(|s| s.to_string()).collect());
    m
}

/// Gods: `Divinity[25]` counts the gods unlocked, in `GodsInfo` order (above 10 it's the god
/// rank). The next one comes from an offering at the altar, paid in divinity points
/// (`Divinity[24]`); the price depends on a server value past the third god (N.js
/// `OfferingCost`), so it isn't shown.
fn gods(save: &Save) -> Mechanic {
    let unlocked = value(save, "Divinity", &[25]);
    let points = value(save, "Divinity", &[24]);
    let info = rows("GodsInfo", &[]);
    let items = info
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut it = Item::new(label(text(r, 0)), pic(format!("DivGod{i}")), f64::from((i as f64) < unlocked), 1.0);
            if (i as f64) >= unlocked {
                if i as f64 == unlocked.floor() {
                    it.how = Some(format!("Next: an offering at the Divinity altar. You hold {} divinity points.", big(points)));
                } else {
                    let prev = info.get(i.wrapping_sub(1)).map_or_else(String::new, |p| label(text(p, 0)));
                    it.how = Some(format!("Comes after {prev}."));
                    it.state = State::Locked;
                }
            }
            it
        })
        .collect();
    mech("gods", 5, "Divinity Gods", pic("DivGod0".into()), Count::Done, "unlocked", items)
}

/// The blessing cap's raises (N.js `BlessingMaxLV`), at their full value, and how much of each
/// this account has: Coral Kid upgrade 1 (2 per level of `OptLacc[428]`, no cap), Minehead
/// bonus 9 (`Research` table [20][9], on once save `Research[7][4]` passes 9), vault upgrade 76
/// Big Blessings (`UpgVault[76]` times `UpgradeVault[76][5]`, max col 4 plus 10 from a bundle)
/// and Jelly Operator RoG bonuses 49 and 63 (`Research` table [47], on once save
/// `Research[7][9]` passes the index).
struct BlessingRaises {
    coral: f64,
    minehead: (f64, bool),
    vault: (f64, f64),
    jelly: [(f64, bool); 2],
}

fn blessing_raises(save: &Save) -> BlessingRaises {
    let research = |r: usize, c: usize| rows("Research", &[r]).get(c).map_or(0.0, |v| num(v));
    let vault = rows("UpgradeVault", &[]).get(76).copied();
    let per = vault.map_or(1.0, |r| cell(r, 5));
    BlessingRaises {
        coral: 2.0 * save.option(428).max(0.0),
        minehead: (research(20, 9), value(save, "Research", &[7, 4]) > 9.0),
        vault: (vault.map_or(0.0, |r| (cell(r, 4) + 10.0) * per), value(save, "UpgVault", &[76]) * per),
        jelly: [49, 63].map(|i| (research(47, i), value(save, "Research", &[7, 9]) > i as f64)),
    }
}

/// What a blessing level is paid in, by god (N.js, the blessing button): Snehebatu, Nobisect,
/// Goharut and Kattlekruk take gaming bits (`Gaming[0]`), Arctis gold bars (`Sailing[1][0]`),
/// Harriep and Purrmep coins (a character's own `Money_c`), the rest particles (`Divinity[39]`).
fn blessing_purse(save: &Save, god: usize) -> (&'static str, f64) {
    match god {
        0 | 2 | 4 | 8 => ("bits", value(save, "Gaming", &[0])),
        1 => ("gold bars", value(save, "Sailing", &[1, 0])),
        3 | 6 => ("coins", (0..save.characters().min(12)).map(|c| value(save, &format!("Money_{c}"), &[])).fold(0.0, f64::max)),
        _ => ("particles", value(save, "Divinity", &[39])),
    }
}

/// Blessings: `Divinity[28 + god]`. A level costs `GodsInfo` col 4 times col 5 to the power of
/// the level. The max is 100 plus every raise at full; the Coral Kid one, which has no cap,
/// counts what this account has bought.
fn blessings(save: &Save) -> Mechanic {
    let levels = save.numbers("Divinity", &[]);
    let unlocked = value(save, "Divinity", &[25]);
    let r = blessing_raises(save);
    let max = 100.0 + r.coral + r.minehead.0 + r.vault.0 + r.jelly.iter().map(|j| j.0).sum::<f64>();
    let cap = 100.0 + r.coral + if r.minehead.1 { r.minehead.0 } else { 0.0 } + r.vault.1 + r.jelly.iter().filter(|j| j.1).map(|j| j.0).sum::<f64>();
    let items = rows("GodsInfo", &[])
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let level = at(&levels, 28 + i);
            let mut it = Item::new(format!("{} Blessing", label(text(g, 0))), pic(format!("DivGod{i}")), level, max);
            if (i as f64) >= unlocked {
                it.how = Some(format!("Unlock {} first.", label(text(g, 0))));
                it.state = State::Locked;
            } else if level >= cap && level < max {
                it.how = Some(format!("At your blessing cap of {cap}. The raises listed below lift it."));
                it.state = State::Locked;
            } else if level < max {
                let cost = cell(g, 4) * cell(g, 5).powf(level);
                let (what, held) = blessing_purse(save, i);
                it.how = Some(format!("Next level costs {} {what}.", big(cost)));
                if cost.is_finite() && cost <= held {
                    it.state = State::Ready;
                }
            }
            it
        })
        .collect();
    let mut m = mech("blessings", 5, "Divinity Blessings", pic("DivGod0".into()), Count::Levels, "levels", items);
    m.raises = vec![
        Raise { name: "Coral Kid's second upgrade".into(), amount: "+2 max levels on every blessing per upgrade level, no limit".into(), owned: r.coral > 0.0 },
        Raise { name: "Minehead reward: beat 10 opponents in Mr. Minehead's Depth Charge".into(), amount: format!("+{} max levels on every blessing", r.minehead.0), owned: r.minehead.1 },
        Raise { name: "Big Blessings (Upgrade Vault)".into(), amount: format!("+1 max level on every blessing per vault level, up to +{}", r.vault.0), owned: r.vault.1 > 0.0 },
        Raise { name: "Jelly Operator reward: 50 successful operations".into(), amount: format!("+{} max levels on every blessing", r.jelly[0].0), owned: r.jelly[0].1 },
        Raise { name: "Jelly Operator reward: 64 successful operations".into(), amount: format!("+{} max levels on every blessing", r.jelly[1].0), owned: r.jelly[1].1 },
    ];
    m.note = Some(format!(
        "Every blessing caps at 100, raised by {} from a Minehead reward, {} from the Big Blessings vault upgrade, {} from two Jelly Operator rewards, and 2 per level of Coral Kid's second upgrade. The Coral Kid raise has no limit, so the max here counts the levels you have. Your cap now is {cap}. Snehebatu, Nobisect, Goharut and Kattlekruk take gaming bits; Arctis gold bars; Harriep and Purrmep coins; the rest particles.",
        r.minehead.0,
        r.vault.0,
        r.jelly[0].0 + r.jelly[1].0
    ));
    m
}

/// The Slab: items ever looted (`Cards1`) against every item it lists.
fn slab(save: &Save) -> Mechanic {
    let found = save.get("Cards1").and_then(Value::as_array).map_or(0, Vec::len) as f64;
    let total = rows("SlabItemSort", &[]).len() as f64;
    let mut it = Item::new("Items looted", None, found.min(total), total);
    if found < total {
        it.how = Some(format!("{} items you have never picked up. Each one counts once looted.", total - found.min(total)));
    }
    mech("slab", 5, "The Slab", None, Count::Levels, "items", vec![it])
}

/// A superbit's cost in bits (N.js `SuperbitCost`): `GamingUpg` col 1 times 10 to col 2.
fn superbit_cost(row: &Value) -> f64 {
    cell(row, 1) * 10f64.powf(cell(row, 2))
}

/// Gaming superbits: `GamingUpg` row i (col 3 the name, "Line|Line"). Bought ones are in
/// `Gaming[12]`. They sit 24 to a page in rows of six; one can be bought once a neighbour above,
/// left or right on its page is (the first of each page is always open). The second page needs
/// Spelunking lore bonus 1 (`DuperBits`), the third research node 87 (`ZuperBitsUnlocked`).
/// The game's rule for buying one (the superbit page's click handler) is only that: not owned,
/// a neighbour bought (or first on its page), and bits at least the cost. The last two rows
/// ("No Way", "No How") cost 2 x 10^150 bits and say they won't be sold, so they can't be
/// bought and aren't counted. Pages two and three are World 7 content (their unlocks are).
fn superbits(save: &Save) -> Mechanic {
    let bits = value(save, "Gaming", &[0]);
    let table = rows("GamingUpg", &[]);
    let mut seen: Vec<String> = Vec::new();
    let mut costs: Vec<f64> = Vec::new();
    let items: Vec<Item> = table
        .iter()
        .enumerate()
        .filter(|(_, r)| cell(r, 2) < 150.0)
        .map(|(i, r)| {
            let base = label(&text(r, 3).replace('|', "_"));
            let repeat = seen.iter().filter(|s| **s == base).count();
            seen.push(base.clone());
            let name = if repeat == 0 { base } else { format!("{base} {}", ["II", "III", "IV", "V", "VI", "VII"][(repeat - 1).min(5)]) };
            let has = superbit_owned(save, i);
            let mut it = Item::new(name, None, f64::from(has), 1.0);
            if i >= 24 {
                it.world = Some(7);
            }
            if !has {
                let (page, slot) = (i / 24, i % 24);
                let neighbour = slot == 0
                    || (slot >= 6 && superbit_owned(save, i - 6))
                    || (slot % 6 != 0 && superbit_owned(save, i - 1))
                    || (slot % 6 != 5 && slot + 1 < 24 && superbit_owned(save, i + 1));
                let cost = superbit_cost(r);
                if page == 1 && !lore(save, 1) {
                    it.how = Some("This page opens with Spelunking's second lore bonus.".into());
                    it.state = State::Locked;
                } else if page == 2 && !grid_node(save, 87) {
                    it.how = Some(format!("This page opens with research node {}.", grid_name(87)));
                    it.state = State::Locked;
                } else if !neighbour {
                    it.how = Some(format!("Buy a superbit next to it first. It costs {} bits.", big(cost)));
                    it.state = State::Locked;
                } else {
                    // Two say they do nothing until a World 7 skill comes out; buyable all the same.
                    let later = text(r, 0).contains("W7_Skill_3") || text(r, 0).contains("W7_skill_3");
                    it.how = Some(if later { format!("Costs {} bits. Does nothing until World 7's third skill comes out.", big(cost)) } else { format!("Costs {} bits.", big(cost)) });
                    if cost <= bits {
                        it.state = State::Ready;
                    }
                }
            }
            costs.push(superbit_cost(r));
            it
        })
        .collect();
    let mut ready: Vec<(f64, &Item)> = items.iter().zip(&costs).filter(|(it, _)| it.state == State::Ready).map(|(it, c)| (*c, it)).collect();
    ready.sort_by(|a, b| a.0.total_cmp(&b.0));
    let names: Vec<&str> = ready.iter().take(2).map(|(_, it)| it.name.as_str()).collect();
    let have = Have {
        figure: format!("{} bits", big(bits)),
        text: match names.as_slice() {
            [] => "Not enough bits for any superbit you can buy now.".into(),
            [one] => format!("Enough bits for {one}."),
            [a, b, ..] => format!("Enough bits for {a} or {b}, the two cheapest you can buy now."),
        },
    };
    let mut m = mech("superbits", 5, "Gaming Superbits", None, Count::Done, "bought", items);
    m.have = Some(have);
    m
}

/// The Immortal Snail: `GamingSprout[32][1]` is its level. N.js `MaxSnailLV` is 25, +10, +5 and
/// +10 from superbits 29, 30 and 40, so 50; but from level 49 the success chance (`SnailStuff`
/// 0: 1 - 0.6 x (level - 24)^0.16 past 24, else 1 - 0.1 x level^0.71, times
/// 1 + encouragement, `GamingSprout[32][2]`, and a Cavern bonus) is zero, so 49 is the top.
fn snail(save: &Save) -> Mechanic {
    let level = value(save, "GamingSprout", &[32, 1]);
    let enc = value(save, "GamingSprout", &[32, 2]);
    let table = rows("GamingUpg", &[]);
    let bit = |i: usize| table.get(i).map_or_else(|| format!("superbit {i}"), |r| label(&text(r, 3).replace('|', "_")));
    let raises = [(29, 10.0), (30, 5.0), (40, 10.0)];
    let cap = 25.0 + raises.iter().filter(|(i, _)| superbit_owned(save, *i)).map(|(_, a)| a).sum::<f64>();
    // The snail is gaming item 7 (`GamingSprout[25 + 7]`), drawn as `GamingItem7`.
    let mut it = Item::new("Immortal Snail", pic("GamingItem7".into()), level, 49.0);
    if level < 49.0 {
        if level >= cap {
            let missing: Vec<String> = raises.iter().filter(|(i, _)| !superbit_owned(save, *i)).map(|(i, _)| bit(*i)).collect();
            it.how = Some(format!("At your snail cap of {cap}. These superbits raise it: {}.", missing.join(", ")));
            it.state = State::Locked;
        } else {
            let chance = if level > 24.0 {
                (1.0 - 0.6 * (level - 24.0).powf(0.16)) * (1.0 + 50.0 * enc / (3.0 + enc) / 100.0)
            } else {
                (1.0 - 0.1 * level.powf(0.71)) * (1.0 + 110.0 * enc / (25.0 + enc) / 100.0)
            };
            it.how = Some(format!("Next level: {:.1}% chance each try, before your Cavern bonus.", (chance.clamp(0.0, 1.0) * 100.0)));
        }
    }
    let mut m = mech("snail", 5, "Immortal Snail", pic("GamingItem7".into()), Count::Levels, "levels", vec![it]);
    m.raises = raises.iter().map(|(i, a)| Raise { name: format!("Superbit {}", bit(*i)), amount: format!("+{a} max snail levels"), owned: superbit_owned(save, *i) }).collect();
    m.note = Some("The snail's cap starts at 25. Three superbits raise it to 50, but the snail can't level past 49: the chance of a level at 49 is zero.".into());
    m
}

/// Island names as the sailing map shows them; the game's `IslandInfo` calls every one "Sahara".
const ISLANDS: [&str; 17] = [
    "Safari Island", "Beachy Coast", "Isolated Woods", "Rocky Peaks", "Stormy North", "Snowy South", "Toxic Bay Inc", "Candied Island", "Fungi Meadows",
    "Cloudy Quay", "Dungeon Cove", "Crystal Enclave", "Petulent Garage", "Isle of Note", "The Edge", "World's End", "The Maw",
];

/// Sailing islands: `Sailing[0][i]` is -1 once discovered, otherwise the exploration so far; the
/// island can be uncovered once that reaches `IslandInfo` col 5 (N.js, the sailing map). World's
/// End comes with Spelunking's first lore bonus, The Maw with research node 106
/// (`TheMawIslandUnlocked`). Safari Island is discovered from the start, so it isn't counted.
fn islands(save: &Save) -> Mechanic {
    let state = save.numbers("Sailing", &[0]);
    let items = rows("IslandInfo", &[])
        .iter()
        .enumerate()
        .skip(1)
        .map(|(i, r)| {
            let name = ISLANDS.get(i).map_or_else(|| format!("Island {}", i + 1), |s| (*s).to_string());
            let found = state.len() > i && at(&state, i) == -1.0;
            // The island's box on the sailing map shows its treasure, `SailT{1 + 2i}`, beside its name.
            let mut it = Item::new(name, pic(format!("SailT{}", 1 + 2 * i)), f64::from(found), 1.0);
            // World's End and The Maw open with World 7 content.
            if i == 15 || i == 16 {
                it.world = Some(7);
            }
            if !found {
                let progress = at(&state, i).max(0.0);
                let need = cell(r, 5);
                if i == 15 && !lore(save, 0) {
                    it.how = Some("Comes with Spelunking's first lore bonus.".into());
                    it.state = State::Locked;
                } else if i == 16 && !grid_node(save, 106) {
                    it.how = Some(format!("Comes with research node {}.", grid_name(106)));
                    it.state = State::Locked;
                } else if i == 15 || i == 16 {
                    it.how = Some("Open the sailing map to uncover it.".into());
                } else if progress >= need && need > 0.0 {
                    it.how = Some("Explored enough: uncover it on the sailing map.".into());
                    it.state = State::Ready;
                } else {
                    it.how = Some(format!("Explored {} of {} to uncover it.", big(progress), big(need)));
                }
            }
            it
        })
        .collect();
    mech("sailing-islands", 5, "Sailing Islands", pic("SailingNewIsle1".into()), Count::Done, "discovered", items)
}

/// What boat `i` costs in gold bars (N.js `NewCaptBoatSlot` with 1):
/// (60i + 15i^2.25) x 1.55^i.
fn boat_cost(i: f64) -> f64 {
    (60.0 * i + 15.0 * i.powf(2.25)) * 1.55f64.powf(i)
}

/// Boats: `Sailing[2][1]` is the boats bought after the free first one (boat 0), so the next is
/// boat bought + 1; the harbor has a place for each boat the game allows (`BoatsInHarbor`).
/// They're paid in gold bars, `Sailing[1][0]`.
fn boats(save: &Save) -> Mechanic {
    let bought = value(save, "Sailing", &[2, 1]);
    let max = rows("BoatsInHarbor", &[]).len().saturating_sub(1) as f64;
    let gold = value(save, "Sailing", &[1, 0]);
    let mut it = Item::new("Boats bought", None, bought, max);
    // How many more the gold buys: at most the boats left, a few dozen steps.
    let (mut n, mut left) = (bought.min(max), gold);
    while n < max && boat_cost(n + 1.0) <= left {
        left -= boat_cost(n + 1.0);
        n += 1.0;
    }
    if bought < max {
        let cost = boat_cost(bought + 1.0);
        it.how = Some(format!("Next boat costs {} gold bars.", big(cost)));
        if cost <= gold {
            it.state = State::Ready;
        }
    }
    let mut m = mech("boats", 5, "Sailing Boats", None, Count::Levels, "boats", vec![it]);
    m.have = Some(Have {
        figure: format!("{} gold bars", big(gold)),
        text: if bought >= max {
            "Every boat is bought.".into()
        } else if n > bought.min(max) {
            format!("Enough gold bars for {} more boats.", n - bought.min(max))
        } else {
            "Not enough for the next boat yet.".into()
        },
    });
    m
}
