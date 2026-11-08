//! World 2: Yum-Yum Desert.

use std::collections::HashMap;

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
const LIQUIDS: [&str; 4] = ["Water Droplets", "Liquid Nitrogen", "Trench Seawater", "Toxic Mercury"];

/// What you hold of each item: the storage chest (`ChestOrder`/`ChestQuantity`) plus the four
/// liquids, which N.js `ItemsAndStorageOWNED` adds as `Liquid1..4` from `CauldronInfo[6]`.
/// Inventories aren't counted, so this can read low, never high.
fn held(save: &Save) -> HashMap<String, f64> {
    let order = save.get("ChestOrder").and_then(Value::as_array);
    let qty = save.numbers("ChestQuantity", &[]);
    let mut out: HashMap<String, f64> = HashMap::new();
    for (i, id) in order.into_iter().flatten().enumerate() {
        if let Some(id) = id.as_str() {
            *out.entry(id.to_string()).or_default() += at(&qty, i).max(0.0);
        }
    }
    let liquid = save.numbers("CauldronInfo", &[6]);
    for l in 0..4 {
        out.insert(format!("Liquid{}", l + 1), at(&liquid, l).max(0.0));
    }
    out
}

/// An item's name as the game shows it; liquids aren't in the item list.
fn item_name(id: &str) -> String {
    if let Some(n) = id.strip_prefix("Liquid").and_then(|n| n.parse::<usize>().ok()).filter(|n| (1..=4).contains(n)) {
        return LIQUIDS[n - 1].into();
    }
    GAME.item_name(id).map(str::to_string).unwrap_or_else(|| words(id))
}

/// "1M Copper Ore and 36 Water Droplets", and whether you hold all of it.
fn costs(parts: &[(String, f64)], have: &HashMap<String, f64>) -> (String, bool) {
    let text: Vec<String> = parts.iter().map(|(id, n)| format!("{} {}", big(*n), item_name(id))).collect();
    let ok = parts.iter().all(|(id, n)| have.get(id).copied().unwrap_or(0.0) >= *n);
    (text.join(" and "), ok)
}

/// What one bubble level costs in material `s` (0 to 3), before your discounts (N.js
/// `CauldronStats("CauldronCosts")`): liquids cost base + level/20; Bits base * 2^level;
/// anything else base * (1.35 - 0.3L/(50+L))^L, or (1.37 - 0.28L/(60+L))^L past the 15th
/// bubble, never over 1B.
fn bubble_cost(row: &Value, bubble: usize, s: usize, level: f64) -> Option<(String, f64)> {
    let mat = text(row, 5 + s);
    if mat == "Blank" || mat.is_empty() {
        return None;
    }
    let base = cell(row, 11 + s);
    let cost = if mat.starts_with("Liquid") {
        base + (level / 20.0).floor()
    } else if mat == "Bits" {
        base * 2f64.powf(level)
    } else {
        let g = if bubble > 14 { (1.37 - 0.28 * level / (60.0 + level)).powf(level) } else { (1.35 - 0.3 * level / (50.0 + level)).powf(level) };
        (base * g).clamp(1.0, 1e9)
    };
    Some((mat.to_string(), cost))
}

const BUBBLE_TIERS: [&str; 5] = ["Half bonus", "75% bonus", "90% bonus", "95% bonus", "Softcap"];
const BUBBLE_SHARES: [f64; 5] = [0.5, 0.75, 0.9, 0.95, 0.99];
/// Levels for bubbles whose bonus has no cap, for a World 1 bubble; each later world's bubbles
/// aim a quarter higher (World 7: 2.5 times).
const FLAT_STEPS: [f64; 5] = [100.0, 250.0, 500.0, 1000.0, 2000.0];

/// The world an unfound bubble waits for. Nothing in N.js holds a bubble back by world: each
/// cauldron finds the next one in order as it brews (`PctChanceNewBubble`; `CauldronInfo[7][c]`
/// is only the page of five shown). Slot i is read as World i/5 + 1, roughly when it came out,
/// so a bubble not found yet leaves the ladder until then. One you have always counts.
fn bubble_world(i: usize) -> u8 {
    (i / 5 + 1).min(7) as u8
}

/// The five levels a bubble aims for. Its bonus is `AlchemyDescription` col 3 of cols 1 and 2
/// (N.js `ArbitraryCode5Inputs`): "decay" and "decayMulti" grow as x1 L/(L+x2), reaching a
/// share p of their cap at L = p x2/(1-p), so 99% (the softcap) is at 99 x2. "bigBase"
/// (x1 + x2 L) and "addDECAY" (x1 L up to Lv 50,000) add a flat amount per level, so they get
/// set levels that grow with the bubble's world.
fn bubble_targets(row: &Value, world: u8) -> ([f64; 5], bool) {
    let x2 = cell(row, 2);
    if matches!(text(row, 3), "decay" | "decayMulti") && x2 > 0.0 {
        (BUBBLE_SHARES.map(|p| (p / (1.0 - p) * x2 - 1e-6).ceil()), true)
    } else {
        let scale = 1.0 + 0.25 * f64::from(world.saturating_sub(1));
        (FLAT_STEPS.map(|l| l * scale), false)
    }
}

/// Bubbles: `CauldronInfo[c][i]` for the four cauldrons, matching `AlchemyDescription[c][i]`.
/// Rows named "BUBBLE" are placeholders, and the save keeps slots past the table; both read Lv 1
/// in the save and are left out. Levels have no cap, so each bubble is counted in tiers of its
/// own bonus (see `bubble_targets`): level 0 to 5 is how many of its five targets it has passed.
fn bubbles(save: &Save) -> Mechanic {
    let mut have = held(save);
    have.insert("Bits".into(), save.at("Gaming", &[0]).map(num).unwrap_or(0.0).max(0.0));
    let mut items = Vec::new();
    for (c, colour) in ['O', 'G', 'P', 'Y'].into_iter().enumerate() {
        let levels = save.numbers("CauldronInfo", &[c]);
        for (i, r) in rows("AlchemyDescription", &[c]).iter().enumerate() {
            if text(r, 0) == "BUBBLE" {
                continue;
            }
            let level = at(&levels, i).max(0.0);
            let world = bubble_world(i);
            let (targets, capped) = bubble_targets(r, world);
            let reached = targets.iter().filter(|t| level >= **t).count();
            let mut it = Item::new(label(text(r, 0)), pic(format!("aUpgrades{colour}{i}")), reached as f64, 5.0);
            it.raw = Some(level);
            it.level_name = Some(if capped && level > 0.0 {
                let x2 = cell(r, 2);
                format!("Lv {} · {:.0}% of its bonus", crate::page::num(level), (100.0 * level / (level + x2)).floor())
            } else {
                format!("Lv {}", crate::page::num(level))
            });
            if world > 2 && level <= 0.0 {
                it.world = Some(world);
            }
            let parts: Vec<(String, f64)> = (0..4).filter_map(|s| bubble_cost(r, i, s, level)).collect();
            if level <= 0.0 {
                it.how = Some(format!("Not found yet: the {} Cauldron finds new bubbles as it brews.", CAULDRONS[c]));
            } else if reached < 5 {
                let target = crate::page::num(targets[reached]);
                let goal = if capped { format!("{:.0}% of its bonus at Lv {target}", BUBBLE_SHARES[reached] * 100.0) } else { format!("Next target Lv {target} (flat bonus, no cap)") };
                if parts.iter().any(|(_, n)| !n.is_finite()) {
                    it.how = Some(format!("{goal}; each level costs more Bits than the game can count."));
                } else if !parts.is_empty() {
                    let (text, ok) = costs(&parts, &have);
                    it.how = Some(format!("{goal}; next level costs {text}."));
                    if ok {
                        it.state = State::Ready;
                    }
                }
            }
            items.push(it);
        }
    }
    let mut m = mech("bubbles", 2, "Alchemy Bubbles", pic("aUpgradesO0".into()), Count::Levels, "tiers", items);
    m.tier_names = Some(BUBBLE_TIERS.iter().map(|s| s.to_string()).collect());
    let w7 = 1.0 + 0.25 * 6.0;
    m.tier_notes = BUBBLE_SHARES
        .iter()
        .zip(FLAT_STEPS)
        .map(|(p, l)| {
            format!(
                "Bubbles with a capped bonus at {:.0}% of it; bubbles that add a flat amount per level at Lv {} (World 1 bubbles) to Lv {} (World 7).",
                p * 100.0,
                crate::page::num(l),
                crate::page::num(l * w7)
            )
        })
        .collect();
    m.note = Some("Levels never stop, so each bubble aims for its own bonus: 50, 75, 90, 95 and 99% of its cap, and 99% is its softcap. Bubbles that add a flat amount per level aim for Lv 100, 250, 500, 1,000 and 2,000, a quarter more for each later world. Costs are before your discounts and count the storage chest only.".into());
    m
}

const BREW: [&str; 4] = ["Speed", "Luck", "Cost", "Extra"];

/// Brew progress a bubble cauldron upgrade needs for its next level (N.js
/// `CauldronStats("CauldronLvsBrewREQ")`): 1 + (1.5n)^1.6 * 1.073^n * 2 * the Boost Req discount,
/// n the next level. Boost Req is the Pay2Win upgrade `CauldronP2W[0][3c+2]`, worth 70L/(L+100)%.
fn brew_req(level: f64, boost_req: f64) -> f64 {
    let n = level + 1.0;
    let discount = ((100.0 - 70.0 * boost_req / (boost_req + 100.0)) / 100.0).max(0.05);
    (1.0 + (1.5 * n).powf(1.6) * 1.073f64.powf(n) * 2.0 * discount).floor()
}

/// Cauldron brew upgrades: `CauldUpgLVs` is 8 groups of 4; the first four groups are the
/// bubble cauldrons (speed, luck, cost, extra), capped at 170 by the level-up code
/// (`Math.min(170, ...)`), and the panel shows 170 and up as maxed. A repair the game runs on
/// load (when 15 of the 16 read 0) can set them to Alchemy level^1.07, past 170; those count as
/// 170. The last four groups are the liquids' cap and rate, which have no cap. Progress toward
/// the next level is `CauldUpgXPs`.
fn cauldron_upgrades(save: &Save) -> Mechanic {
    let levels = save.numbers("CauldUpgLVs", &[]);
    let xp = save.numbers("CauldUpgXPs", &[]);
    let p2w = save.numbers("CauldronP2W", &[0]);
    let items = (0..16)
        .map(|k| {
            let level = at(&levels, k).min(170.0);
            let mut it = Item::new(format!("{} Cauldron: {}", CAULDRONS[k / 4], BREW[k % 4]), None, level, 170.0);
            if level < 170.0 {
                let need = brew_req(level, at(&p2w, 3 * (k / 4) + 2));
                it.how = Some(format!("Next level at {} brew progress, {} so far. Brewing in this cauldron with it selected fills it.", big(need), big(at(&xp, k).max(0.0))));
            }
            it
        })
        .collect();
    let mut m = mech("cauldron-upgrades", 2, "Cauldron Upgrades", pic("aEmptyCauldronB0".into()), Count::Levels, "levels", items);
    m.note = Some("Speed, Luck, Cost and Extra on each bubble cauldron, levelled by brewing. The liquid cauldrons' Cap and Rate upgrades have no level cap, so they are left out.".into());
    m
}

/// Coins for the next Pay2Win level (N.js `cauldronp2wbonuses("CauldronCosts")`): `kind` 0
/// cauldron (`s` 0 speed, 1 new bubble, 2 boost req), 1 liquid (0 regen, 1 capacity), 2 vials
/// (0 attempts, 1 RNG), 3 player (0 alchemy speed, 1 extra EXP).
fn p2w_cost(kind: usize, s: usize, level: f64) -> f64 {
    let l = level.max(0.0);
    let curve = |base: f64, a: f64, b: f64| base * (a - b * l / (100.0 + l)).powf(l);
    match (kind, s) {
        (0, 0) => curve(2500.0, 1.15, 0.117),
        (0, 1) => curve(3200.0, 1.18, 0.145),
        (0, _) => curve(3750.0, 1.2, 0.14),
        (1, 0) => curve(2500.0, 1.19, 0.135),
        (1, _) => curve(3500.0, 1.2, 0.13),
        (2, 0) => 1e4 * 2f64.powf(l),
        (2, _) => 5e3 * 1.25f64.powf(l),
        (_, s) => curve(4000.0 + 2000.0 * s as f64, 1.15, 0.1 - 0.01 * s as f64),
    }
    .round()
}

/// Pay2Win tab: `CauldronP2W[0]` cauldron (speed, new bubble, boost req) x4, `[1]` liquid
/// (regen, capacity) x4, `[2]` vials (attempts, RNG), `[3]` player (alchemy speed, extra EXP).
/// Maxes from `cauldronp2wbonuses("CauldronLvMAX")`: 150/125/100, 100/80, 15/45, and the
/// highest Alchemy level of any character for the player upgrades. A locked liquid reads -1 and
/// opens at Alchemy level 20, 35 and 80 (`LiquidCauldronLvReq`). Paid in coins from the
/// character you're on, which can take more from the bank first (see `w1::spendable`).
fn pay2win(save: &Save) -> Mechanic {
    let cauldron = save.numbers("CauldronP2W", &[0]);
    let liquid = save.numbers("CauldronP2W", &[1]);
    let vial = save.numbers("CauldronP2W", &[2]);
    let player = save.numbers("CauldronP2W", &[3]);
    // The game has 12 character slots; the save can't be trusted to say fewer or more.
    let chars = 12;
    let alchemy = (0..chars).map(|i| at(&save.numbers(&format!("Lv0_{i}"), &[]), 5)).fold(0.0, f64::max);
    let coins = super::w1::spendable(save);
    // (name, level, max, kind, s)
    let mut rows: Vec<(String, f64, f64, usize, usize)> = Vec::new();
    for (c, name) in CAULDRONS.iter().enumerate() {
        for (k, (what, max)) in [("Speed", 150.0), ("New Bubble", 125.0), ("Boost Req", 100.0)].into_iter().enumerate() {
            rows.push((format!("{name} Cauldron: {what}"), at(&cauldron, 3 * c + k), max, 0, k));
        }
    }
    for (l, name) in LIQUIDS.iter().enumerate() {
        for (k, (what, max)) in [("Regen", 100.0), ("Capacity", 80.0)].into_iter().enumerate() {
            rows.push((format!("{name}: {what}"), at(&liquid, 2 * l + k), max, 1, k));
        }
    }
    rows.push(("Vials: Attempts".into(), at(&vial, 0), 15.0, 2, 0));
    rows.push(("Vials: RNG".into(), at(&vial, 1), 45.0, 2, 1));
    rows.push(("Player: Alchemy Speed".into(), at(&player, 0), alchemy, 3, 0));
    rows.push(("Player: Extra EXP".into(), at(&player, 1), alchemy, 3, 1));
    let mut ready: Vec<(usize, f64, usize, usize, f64)> = Vec::new();
    let items: Vec<Item> = rows
        .iter()
        .enumerate()
        .map(|(n, (name, level, max, kind, s))| {
            // The tab draws each cauldron's and liquid's jar beside its upgrade buttons (N.js
            // `"aJarB"+c`, `"aJarL"+l`); the vial and player rows are buttons with no picture.
            let picture = match kind {
                0 => pic(format!("aJarB{}", n / 3)),
                1 => pic(format!("aJarL{}", (n - 12) / 2)),
                _ => None,
            };
            let mut it = Item::new(name.clone(), picture, *level, *max);
            // Liquids open at Alchemy 20, 35 and 80: about World 2, 3 and 5.
            if *kind == 1 {
                it.ladder_from = [None, None, Some(3), Some(5)][(n - 12) / 2];
            }
            if *kind == 1 && *level < 0.0 {
                let need = [0.0, 20.0, 35.0, 80.0][(n - 12) / 2];
                it.how = Some(format!("Opens when a character reaches Alchemy level {need:.0}; your highest is {alchemy:.0}."));
                it.state = State::Locked;
            } else if level < max {
                let cost = p2w_cost(*kind, *s, *level);
                it.how = Some(format!("Next level costs {} coins.", big(cost)));
                if cost <= coins {
                    it.state = State::Ready;
                    ready.push((n, *level, *kind, *s, *max));
                }
            }
            it
        })
        .collect();
    ready.sort_by(|a, b| a.1.total_cmp(&b.1));
    let reach: Vec<String> = ready
        .iter()
        .take(2)
        .map(|&(n, level, kind, s, max)| {
            let (mut lv, mut left) = (level, coins);
            for _ in 0..300 {
                let c = p2w_cost(kind, s, lv);
                if lv >= max || !(c.is_finite() && c <= left) {
                    break;
                }
                left -= c;
                lv += 1.0;
            }
            format!("{} to Lv {lv:.0}", items[n].name)
        })
        .collect();
    let mut m = mech("pay2win", 2, "Alchemy Pay2Win", pic("aEmptyCauldronB1".into()), Count::Levels, "levels", items);
    m.have = Some(super::w1::coin_have(
        coins,
        match reach.as_slice() {
            [] => "Not enough for the next level of anything here yet.".into(),
            [one] => format!("Enough to take {one}."),
            [a, b, ..] => format!("Enough to take {a}, or {b}."),
        },
    ));
    m.note = Some("The two Player upgrades cap at your highest character's Alchemy level, so each Alchemy level up adds 1 to both caps.".into());
    m
}

/// The world a vial's item comes from, past World 2. The vial table runs in world order:
/// slots 20 to 40 take World 3 items (and the Liquid2/3 of that world), 41 to 51 World 4
/// (Galaxy maps, Kraken), 52 to 63 World 5 (lava maps, Marble), 64 to 74 World 6 (Spirited
/// Valley, Godshard) and 75 on World 7 (Spelunking chapters, `w7` items).
fn vial_world(i: usize) -> Option<u8> {
    match i {
        0..=19 => None,
        20..=40 => Some(3),
        41..=51 => Some(4),
        52..=63 => Some(5),
        64..=74 => Some(6),
        _ => Some(7),
    }
}

/// Vials: `CauldronInfo[4][i]`, max level 13. A vial is found by tossing its item into the
/// cauldron in town (a chance roll); after that each level is bought outright (N.js
/// `CauldronStats("VialCosts")`): `AlchemyVialQTYreq[level]` of the item and 3 x level of the liquid.
fn vials(save: &Save) -> Mechanic {
    let levels = save.numbers("CauldronInfo", &[4]);
    let qty: Vec<f64> = rows("AlchemyVialQTYreq", &[]).iter().map(|v| num(v)).collect();
    let have = held(save);
    let items = rows("AlchemyDescription", &[4])
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let level = at(&levels, i);
            let mut it = Item::new(label(text(r, 0)), pic(text(r, 5).to_string()), level, 13.0);
            if let Some(w) = vial_world(i) {
                it.world = Some(w);
            }
            if level <= 0.0 {
                it.how = Some(format!("Not found yet: toss {} into the cauldron in World 2 town to roll for it.", item_name(text(r, 5))));
            } else if level < 13.0 {
                let mut parts = vec![(text(r, 5).to_string(), at(&qty, level as usize))];
                if text(r, 6).starts_with("Liquid") {
                    parts.push((text(r, 6).to_string(), 3.0 * level));
                }
                let (text, ok) = costs(&parts, &have);
                it.how = Some(format!("Next level: {text}."));
                if ok {
                    it.state = State::Ready;
                }
            }
            it
        })
        .collect();
    let mut m = mech("vials", 2, "Vials", pic("aVials13".into()), Count::Levels, "levels", items);
    m.note = Some("Each level is bought with the vial's item and liquid. What you hold counts the storage chest and your liquids, not inventories.".into());
    m
}

const SIGIL_TIERS: [&str; 6] = ["Locked", "Blue", "Yellow", "Red", "Purple", "Green"];

/// Sigils: `CauldronP2W[4]` is [progress, tier] pairs; tier -1 locked to 4 green. The lab
/// charges progress; a sigil moves up when it reaches `SigilDesc` col 1 (unlock, which spends
/// it), 2 (Yellow), 7 (Red), 9 (Purple) or 11 (Green). Red needs Jade Emporium upgrade 31
/// (Ionized Sigils), Purple Spelunking lore boss 7 (`Spelunk[0][6]`), Green also the Eclectic
/// Sigils research node (`Research[0][128]`), all in N.js `_event_coinshopP2w` and
/// `GreenSigilsUnlocked`. A sigil shows once the one before it is unlocked.
fn sigils(save: &Save) -> Mechanic {
    let p2w = save.numbers("CauldronP2W", &[4]);
    let ionized = super::general::emporium_owned(save, 31);
    let lore = at(&save.numbers("Spelunk", &[0]), 6) >= 1.0;
    let eclectic = lore && at(&save.numbers("Research", &[0]), 128) >= 1.0;
    let table = rows("SigilDesc", &[]);
    let tier_of = |i: usize| (p2w.get(2 * i + 1).copied().unwrap_or(-1.0) + 1.0).clamp(0.0, 5.0);
    let items = table
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let tier = tier_of(i);
            let progress = at(&p2w, 2 * i).max(0.0);
            let mut it = Item::new(label(text(r, 0)), pic(format!("aSiga{i}")), tier, 5.0);
            it.level_name = Some(SIGIL_TIERS[tier as usize].into());
            let t = tier as usize;
            if t < 5 {
                let need = cell(r, [1, 2, 7, 9, 11][t]);
                let next = if t == 0 { "Unlocks" } else { SIGIL_TIERS[t + 1] };
                let gate = match t {
                    0 if i > 0 && tier_of(i - 1) < 1.0 => Some(format!("Shows once {} is unlocked.", label(text(table[i - 1], 0)))),
                    2 if !ionized => Some("Red needs Ionized Sigils in the Jade Emporium.".to_string()),
                    3 if !lore => Some("Purple needs Spelunking lore boss 7 beaten.".to_string()),
                    4 if !eclectic => Some("Green needs the Eclectic Sigils research and lore boss 7.".to_string()),
                    _ => None,
                };
                match gate {
                    Some(g) => {
                        it.how = Some(g);
                        it.state = State::Locked;
                    }
                    // The game moves a sigil up only while its page is drawn (N.js sets the tier
                    // in the Sigils tab's drawing code), so progress past the mark waits for that.
                    None if progress >= need => {
                        let goes = if t == 0 { "it unlocks".to_string() } else { format!("it goes {next}") };
                        let what = if t == 0 { "to unlock".to_string() } else { format!("{next} needs") };
                        it.how = Some(format!("Has the {} lab progress {what} ({} so far): open the Sigils page in Alchemy and {goes}.", big(need), big(progress)));
                        it.state = State::Ready;
                    }
                    None => it.how = Some(format!("{next} at {} lab progress, {} so far.", big(need), big(progress))),
                }
            }
            it
        })
        .collect();
    let mut m = mech("sigils", 4, "Sigils", pic("aSiga0".into()), Count::Levels, "tiers", items);
    m.tier_names = Some(SIGIL_TIERS[1..].iter().map(|s| s.to_string()).collect());
    m.raises = vec![
        Raise { name: "Ionized Sigils (Jade Emporium)".into(), amount: "Every sigil can go to Red".into(), owned: ionized },
        Raise { name: "Spelunking lore boss 7 beaten".into(), amount: "Every sigil can go to Purple".into(), owned: lore },
        Raise { name: "Eclectic Sigils research (and lore boss 7)".into(), amount: "Every sigil can go to Green".into(), owned: eclectic },
    ];
    m.tier_notes = vec![
        "Needs Sigils of Olden Alchemy in the World 4 Lab Mainframe, and the sigil before it unlocked".into(),
        String::new(),
        "Needs Ionized Sigils (Jade Emporium)".into(),
        "Needs Spelunking lore boss 7 beaten".into(),
        "Needs the Eclectic Sigils research node and lore boss 7".into(),
    ];
    m.note = Some("Sigils are charged by characters assigned in the World 4 Lab and go Blue, Yellow, Red, Purple, then Green. Red needs Ionized Sigils in the Jade Emporium, Purple Spelunking lore boss 7, Green also the Eclectic Sigils research.".into());
    m
}

/// Gold balls for the next Arcade level, before your Golden Balls stamp (N.js
/// `DungeonCalc("ArcadeCost")`, at most 40% off). Level 100 to 101 costs 5 Cosmic balls
/// (`OptLacc[324]`) and 101 to 102 2 Omega balls (`OptLacc[625]`).
fn arcade_cost(i: usize, l: f64) -> f64 {
    let c = match i {
        39 => 5.0 + 3.0 * l,
        51 | 54 => 10.0 + 5.0 * l + l.powf(1.43),
        _ if i >= 70 => 20.0 + 10.0 * l + l.powf(1.92),
        _ if i >= 62 => 20.0 + 10.0 * l + l.powf(1.69),
        _ if i >= 55 && i != 60 && i != 61 => 20.0 + 10.0 * l + l.powf(1.83),
        _ => 5.0 + 3.0 * l + l.powf(1.3),
    };
    c.round()
}

/// Arcade: `ArcadeUpg[i]`, max 102 (101 Cosmic, 102 Omega). The last six rows are fillers.
/// Gold balls are `OptLacc[75]`.
///
/// The shop sells only this update's rotation: N.js fills its nine slots from the server list
/// `ArcadeBonuses` (`GenINFO[52]`), and every buy, Cosmic and Omega too, goes through a slot.
/// IdleonToolbox copies that list in `serverVars`; without it nothing reads as ready.
fn arcade(save: &Save) -> Mechanic {
    let levels = save.numbers("ArcadeUpg", &[]);
    let gold = save.option(75).max(0.0);
    let cosmic = save.option(324).max(0.0);
    let omega = save.option(625).max(0.0);
    let rotation: Option<Vec<f64>> = save.server_list("ArcadeBonuses");
    let mut ready: Vec<(usize, usize, f64)> = Vec::new();
    let items: Vec<Item> = rows("ArcadeShopInfo", &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| !text(r, 0).contains("who_knows"))
        .map(|(i, r)| {
            let level = at(&levels, i);
            let mut it = Item::new(label(text(r, 0)), pic(format!("PachiShopICON{i}")), level, 102.0);
            let (how, enough) = if level < 100.0 {
                let cost = arcade_cost(i, level);
                (format!("Next level costs {} gold balls.", big(cost)), cost <= gold)
            } else if level < 101.0 {
                ("Level 101 (Cosmic) costs 5 Cosmic balls.".into(), cosmic >= 5.0)
            } else if level < 102.0 {
                ("Level 102 (Omega) costs 2 Omega balls.".into(), omega >= 2.0)
            } else {
                return it;
            };
            match &rotation {
                Some(on) if !on.contains(&(i as f64)) => it.how = Some("Not in this update's shop rotation.".into()),
                on => {
                    it.how = Some(how);
                    if enough && on.is_some() {
                        it.state = State::Ready;
                    }
                }
            }
            it
        })
        .collect();
    for (n, it) in items.iter().enumerate() {
        if it.state == State::Ready && it.level < 100.0 {
            ready.push((n, 0, it.level));
        }
    }
    ready.sort_by(|a, b| a.2.total_cmp(&b.2));
    let index: Vec<usize> = rows("ArcadeShopInfo", &[]).iter().enumerate().filter(|(_, r)| !text(r, 0).contains("who_knows")).map(|(i, _)| i).collect();
    let reach: Vec<String> = ready
        .iter()
        .take(2)
        .map(|&(n, _, level)| {
            let i = index.get(n).copied().unwrap_or(n);
            let (mut lv, mut left) = (level, gold);
            for _ in 0..100 {
                let c = arcade_cost(i, lv);
                if lv >= 100.0 || c > left {
                    break;
                }
                left -= c;
                lv += 1.0;
            }
            format!("{} to Lv {lv:.0}", items[n].name)
        })
        .collect();
    let mut m = mech("arcade", 2, "Arcade", pic("PachiShopICON0".into()), Count::Levels, "levels", items);
    m.have = Some(Have {
        figure: format!("{} gold balls, {} Cosmic, {} Omega", big(gold), big(cosmic), big(omega)),
        text: match reach.as_slice() {
            [] if rotation.is_none() => "Your paste doesn't say which upgrades the shop sells this update, so none reads as ready.".into(),
            [] => "Not enough gold balls for the next level of anything below 100 in this update's rotation.".into(),
            [one] => format!("Enough to take {one}. Costs here are before your Golden Balls stamp."),
            [a, b, ..] => format!("Enough to take {a}, or {b}. Costs here are before your Golden Balls stamp, so you get a little further."),
        },
    });
    m.note = Some("Levels 1 to 100 cost gold balls; level 101 (Cosmic) costs 5 Cosmic balls and level 102 (Omega) 2 Omega balls. The shop sells nine upgrades at a time, a new set each update.".into());
    m
}

/// Boxes the post office only shows once something later is done (N.js, the `UIboxUpg` list):
/// 15, 19 and 20 need Pet Arena bonuses 4, 7 and 10 (World 4 breeding), 21 to 23 the Silver
/// Ankh, Billcye Tri and Cloud Urn sailing artifacts (World 5).
fn box_gate(i: usize) -> Option<(u8, String)> {
    let arena = |n: u32| (4, format!("Opens once your Pet Arena reaches bonus {n} (World 4 breeding)."));
    let artifact = |a: &str| (5, format!("Opens once you find the {a} sailing artifact (World 5)."));
    match i {
        15 => Some(arena(4)),
        19 => Some(arena(7)),
        20 => Some(arena(10)),
        21 => Some(artifact("Silver Ankh")),
        22 => Some(artifact("Billcye Tri")),
        23 => Some(artifact("Cloud Urn")),
        _ => None,
    }
}

/// Post Office: each character's `POu_<i>[box]`. The game only buys a level while it stays
/// under `PostOffUpgradeInfo` col 15 (400.1, 800.1, 100000.1), so the max is that, rounded down.
/// Each level costs 1 box point. Every character has the same points to spend (N.js
/// `PostOfficeINFO("AvailablePoints")`): orders completed, streaks, misc (`CYDeliveryBox*`) and
/// `OptLacc[347]`, less what that character has put in its boxes.
fn post_office(save: &Save) -> Mechanic {
    let boxes = rows("PostOffUpgradeInfo", &[]);
    let earned: f64 = ["CYDeliveryBoxComplete", "CYDeliveryBoxStreak", "CYDeliveryBoxMisc"].iter().map(|k| save.get(k).map(num).unwrap_or(0.0)).sum::<f64>() + save.option(347);
    let earned = earned.max(0.0).round();
    let mut items = Vec::new();
    let mut free_by: Vec<(String, f64)> = Vec::new();
    for c in 0..save.characters().min(12) {
        let levels = save.numbers(&format!("POu_{c}"), &[]);
        let who = character(save, c);
        let spent: f64 = levels.iter().map(|l| l.max(0.0)).sum();
        let free = (earned - spent).max(0.0);
        free_by.push((who.clone(), free));
        for (i, r) in boxes.iter().enumerate() {
            let max = cell(r, 15).floor();
            let level = at(&levels, i);
            let mut it = Item::new(format!("{who}: {}", label(text(r, 0))), pic(format!("UIboxUpg{i}")), level, max);
            let gate = box_gate(i);
            if let Some((w, _)) = gate {
                it.world = Some(w);
            }
            if let Some((_, opens)) = gate.filter(|_| level <= 0.0) {
                it.how = Some(opens);
            } else if level < max {
                it.how = Some(format!("{} levels to go at 1 box point each; {who} has {} free.", big(max - level.max(0.0)), big(free)));
                if free >= 1.0 {
                    it.state = State::Ready;
                }
            }
            items.push(it);
        }
    }
    if items.is_empty() {
        items = boxes.iter().enumerate().map(|(i, r)| Item::new(label(text(r, 0)), pic(format!("UIboxUpg{i}")), 0.0, cell(r, 15).floor())).collect();
    }
    let with: Vec<&(String, f64)> = free_by.iter().filter(|(_, f)| *f >= 1.0).collect();
    let best = free_by.iter().max_by(|a, b| a.1.total_cmp(&b.1));
    let mut m = mech("post-office", 2, "Post Office", pic("UIboxUpg0".into()), Count::Done, "maxed", items);
    m.have = Some(Have {
        figure: format!("{} box points earned", big(earned)),
        text: match (with.len(), best) {
            (0, _) | (_, None) => "Every character has spent all of them.".into(),
            (n, Some((who, f))) => format!("{n} of your characters have points to spend; the most is {} on {who}.", big(*f)),
        },
    });
    m.note = Some("Every character has its own boxes, each paid from the same box points. 20 boxes cap at 400; Scurvy C'arr'ate, Box of Gosh and Gaming Lootcrate at 800; the Myriad Crate at 100,000.".into());
    m
}

const ISLANDS: [&str; 6] = ["Trash", "Rando", "Crystal", "Seasalt", "Shimmer", "Fractal"];

/// The islands you own: `OptLacc[169]` holds a letter per island ("_abcde": `Number2Letter` 0
/// to 5, in the order the game lists them).
fn owned_islands(save: &Save) -> String {
    match save.at("OptLacc", &[169]) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// Island Expeditions, bought with bottles (`OptLacc[162]`). The price (N.js
/// `_customEvent_IslandStuff`, "r") is 10, 12, 15, 50, 25 or 70 by island, plus 0, 15, 45, 100,
/// 200 or 500 by how many you own already.
fn islands(save: &Save) -> Mechanic {
    let owned = owned_islands(save);
    let bottles = save.option(162).max(0.0);
    let count = "_abcde".chars().filter(|c| owned.contains(*c)).count();
    let extra = [0.0, 15.0, 45.0, 100.0, 200.0, 500.0][count.min(5)];
    let mut can: Vec<String> = Vec::new();
    let items = ISLANDS
        .iter()
        .zip("_abcde".chars())
        .enumerate()
        .map(|(i, (name, letter))| {
            let has = owned.contains(letter);
            let mut it = Item::new(format!("{name} Island"), pic(format!("IslandSail{i}")), f64::from(has), 1.0);
            if !has {
                let cost = [10.0, 12.0, 15.0, 50.0, 25.0, 70.0][i] + extra;
                it.how = Some(format!("Costs {cost:.0} bottles from the fishing boat; you have {}.", big(bottles.floor())));
                if bottles >= cost {
                    it.state = State::Ready;
                    can.push(it.name.clone());
                }
            }
            it
        })
        .collect();
    let mut m = mech("islands", 2, "Islands", pic("IslandSail0".into()), Count::Done, "unlocked", items);
    m.have = Some(Have {
        figure: format!("{} bottles", big(bottles.floor())),
        text: if count >= 6 {
            "Every island is yours.".into()
        } else if can.is_empty() {
            "Not enough for the next island yet. Each one you buy raises the price of the rest.".into()
        } else {
            format!("Enough for {}. Each one you buy raises the price of the rest.", can.join(" or "))
        },
    });
    m.note = Some("Islands are bought with bottles from the fishing boat. The island shops' repeat buys have no cap and aren't counted.".into());
    m
}

/// Fractal Island: hours of Nothing AFK (`OptLacc[184]`) against the reward thresholds the game
/// lists on the island (24 to 40,000 hours). The island lists an eighth at 60,000 hours, "World
/// 7 Bonus... I wonder what it will be...", that does nothing yet (N.js never asks
/// `FractalIslandBonus` for it, index 7), so it's left out.
fn fractal_island(save: &Save) -> Mechanic {
    const REWARDS: [(f64, &str); 7] = [
        (24.0, "Master of Nothing Trophy Chance"),
        (200.0, "Dungeon Credits and Flurbos"),
        (750.0, "Kitchen Upgrade Costs"),
        (2500.0, "Sailing Artifact Chance"),
        (10000.0, "Gold Nuggets from the Dirty Shovel"),
        (20000.0, "Star Talent Points"),
        (40000.0, "Ninja Twin Stealth"),
    ];
    let hours = save.option(184).max(0.0);
    let fractal = owned_islands(save).contains('e');
    let items = REWARDS
        .iter()
        .map(|&(need, name)| {
            let mut it = Item::new(name, None, hours.min(need), need);
            if hours < need {
                if fractal || hours > 0.0 {
                    it.how = Some(format!("{} more hours of Nothing AFK.", big((need - hours).ceil())));
                } else {
                    it.how = Some("Buy Fractal Island first, then AFK characters on Nothing.".into());
                    it.state = State::Locked;
                }
            }
            it
        })
        .collect();
    let mut m = mech("fractal-island", 2, "Fractal Island", pic("IslandSail5".into()), Count::Done, "unlocked", items);
    m.note = Some("Hours spent AFK on Nothing at Fractal Island unlock each reward. The last one out is at 40,000 hours; the 60,000-hour World 7 bonus does nothing yet, so it isn't counted.".into());
    m
}

/// Killroy: `OptLacc[106..112]` count the fights won of each kind (timer, talent points, skulls,
/// respawn, dungeon credits, pearls), and the skull shop's permanent buys sit at
/// `OptLacc[228..231]` and `[467..472]`. None of them has a cap: each bonus grows with
/// diminishing returns. Fight k shows once `OptLacc[112]` (all fights) reaches (2k)^min(k,2)
/// and k < 3 + Shades of K (`Dream[4]`). Skull shop rows cost `SkullShopDesc` col 1 skulls
/// (`OptLacc[105]`); rows 10 to 14 open at Rift 50, 15 to 19 also after Billroy's talks
/// (`OptLacc[466]` at 3), per N.js `RiftStuff("KillroyXtra")`.
fn killroy(save: &Save) -> Mechanic {
    let fights = ["Timer", "Talent Points", "Skulls", "Respawn", "Dungeon Credits", "Pearls"];
    // (shop row, save slot, name, the world of what it boosts)
    let shop: [(usize, usize, &str, u8); 8] = [
        (11, 228, "Artifact Find Chance", 5),
        (13, 229, "Crop Evolution Chance", 6),
        (14, 230, "Jade Gain", 6),
        (15, 467, "Gallery Grade", 7),
        (16, 468, "Masterclass Drops", 7),
        (17, 469, "World 7 Skill EXP", 7),
        (18, 470, "Daily Coral", 7),
        (19, 471, "Mystery Boost", 7),
    ];
    let total = save.option(112).max(0.0);
    let shades = save.at("Dream", &[4]).map(num).unwrap_or(0.0).max(0.0);
    let skulls = save.option(105).max(0.0);
    let rift = save.at("Rift", &[0]).map(num).unwrap_or(0.0) >= 50.0;
    let billroy = save.option(466) >= 3.0;
    let desc = rows("SkullShopDesc", &[]);
    let mut items: Vec<Item> = fights
        .iter()
        .enumerate()
        .map(|(k, n)| {
            let mut it = Item::new(format!("{n} fights"), None, save.option(106 + k), 0.0);
            // The last three need Shades of K, an Equinox upgrade from World 3.
            if k >= 3 {
                it.world = Some(3);
            }
            let need = if k == 0 { 0.0 } else { ((2 * k) as f64).powi(k.min(2) as i32) };
            if (k as f64) >= 3.0 + shades {
                it.how = Some(format!("Opens with Shades of K level {} in the Equinox.", k.saturating_sub(2)));
                it.state = State::Locked;
            } else if total < need {
                it.how = Some(format!("Opens after {need:.0} Killroy fights in all; you have {total:.0}."));
                it.state = State::Locked;
            } else {
                it.how = Some("Pick this fight at the end of a Killroy run to add 1.".into());
            }
            it
        })
        .collect();
    items.extend(shop.iter().map(|&(row, opt, n, world)| {
        let count = save.option(opt);
        let mut it = Item::new(format!("Skull Shop: {n}"), None, count, 0.0);
        it.world = Some(world);
        let cost = desc.get(row).map_or(0.0, |r| cell(r, 1));
        let gate = if row >= 15 { "rift 50 is done and you've finished talking to Billroy" } else { "rift 50 is done" };
        let open = rift && (row < 15 || billroy);
        if !open && count <= 0.0 {
            it.how = Some(format!("Opens once {gate}. Then {cost:.0} skulls per buy."));
            it.state = State::Locked;
        } else {
            it.how = Some(format!("{cost:.0} skulls per buy; you have {skulls:.0}."));
            if skulls >= cost && cost > 0.0 {
                it.state = State::Ready;
            }
        }
        it
    }));
    let mut m = mech("killroy", 2, "Killroy", pic("Quest69".into()), Count::Open, "levels", items);
    m.have = Some(Have { figure: format!("{skulls:.0} skulls"), text: "Skulls drop in Killroy runs and buy the skull shop's permanent boosts.".into() });
    m.note = Some("How many times you've picked each fight, and your permanent skull shop buys. None has a cap: each adds less bonus than the one before.".into());
    m
}

/// A character's name, or "Character 3" when the save has none.
fn character(save: &Save, i: usize) -> String {
    save.names.get(i).cloned().unwrap_or_else(|| format!("Character {}", i + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_arcade_rotation_reads_as_ready() {
        let mut opt = vec![0.0; 76];
        opt[75] = 1000.0;
        let paste = |server: &str| Save::parse(&format!(r#"{{"data": {{"OptLacc": {opt:?}}}{server}}}"#)).unwrap();
        let m = arcade(&paste(r#", "serverVars": {"ArcadeBonuses": [5, 7]}"#));
        assert_eq!((m.items[5].state, m.items[7].state), (State::Ready, State::Ready));
        assert_eq!((m.items[3].state, m.items[3].how.as_deref()), (State::Unknown, Some("Not in this update's shop rotation.")));
        // No rotation in the paste: the costs show, nothing is ready.
        let m = arcade(&paste(""));
        assert!(m.items.iter().all(|i| i.state != State::Ready));
        assert!(m.items[5].how.as_deref().is_some_and(|h| h.starts_with("Next level costs")));
    }

    #[test]
    fn a_bubble_you_have_counts_whatever_its_world() {
        let mut orange = vec![0.0; 30];
        orange[20] = 5.0;
        let s = Save::parse(&format!(r#"{{"OptLacc": [0], "CauldronInfo": [{orange:?}]}}"#)).unwrap();
        let m = bubbles(&s);
        let name = |i: usize| label(text(rows("AlchemyDescription", &[0])[i], 0));
        let world = |i: usize| m.items.iter().find(|it| it.name == name(i)).map(|it| it.world);
        assert_eq!(world(20), Some(None));
        assert_eq!(world(21), Some(Some(5)));
    }

    #[test]
    fn fractal_island_leaves_out_the_unreleased_reward() {
        let m = fractal_island(&Save::parse(r#"{"OptLacc": [0]}"#).unwrap());
        assert_eq!(m.items.last().map(|i| i.max), Some(40_000.0));
    }
}
