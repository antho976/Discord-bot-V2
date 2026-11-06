//! World 6: Spirited Valley.

use bot_core::serde_json::Value;

use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![
        crops(save),
        markets(save),
        exotic_market(save),
        land_rank(save),
        jade_emporium(save),
        ninja_upgrades(save),
        pristine_charms(save),
        gemstones(save),
        beanstalk(save),
        emperor(save),
        summoning(save),
        summoning_battles(save),
        summoning_stones(save),
    ]
}

/// Crops discovered: the keys in `FarmCrop` (a bean trade resets counts but keeps the keys).
fn crops(save: &Save) -> Mechanic {
    let found = save.get("FarmCrop").and_then(Value::as_object).map_or(0, |o| o.len()) as f64;
    let total = rows("SeedInfo", &[]).last().map_or(0.0, |r| cell(r, 3) + 1.0);
    let mut it = Item::new("Crops discovered", pic("FarmCrop0".into()), found.min(total), total);
    // Which seed types still have crops to find: `SeedInfo` cols 2 and 3 are each seed's range.
    let known = save.get("FarmCrop").and_then(Value::as_object);
    let missing: Vec<String> = rows("SeedInfo", &[])
        .iter()
        .filter_map(|r| {
            let (a, b) = (cell(r, 2) as usize, cell(r, 3) as usize);
            let left = (a..=b.min(a + 500)).filter(|i| !known.is_some_and(|k| k.contains_key(&i.to_string()))).count();
            (left > 0).then(|| format!("{} {}", left, label(text(r, 0))))
        })
        .collect();
    if !missing.is_empty() {
        it.how = Some(format!("Still to find, by seed: {}.", missing.join(", ")));
    }
    mech("crops", 6, "Farming Crops", pic("FarmCrop0".into()), Count::Levels, "crops", vec![it])
}

/// A crop by its index, the way the seed list orders it: "Bulbo crop 3".
fn crop_name(idx: usize) -> String {
    rows("SeedInfo", &[])
        .iter()
        .find(|r| (cell(r, 2) as usize..=cell(r, 3) as usize).contains(&idx))
        .map_or_else(|| format!("crop {idx}"), |r| format!("{} crop {}", label(text(r, 0)), idx + 1 - cell(r, 2) as usize))
}

/// How many of a crop you hold: `FarmCrop[idx]`.
fn crop_held(save: &Save, idx: usize) -> f64 {
    save.get("FarmCrop").and_then(|c| c.get(idx.to_string())).map_or(0.0, num).max(0.0) + 0.0
}

/// Upgrade names in capitals read as words, with GMO, OG and Roman numerals kept as the game
/// spells them.
fn shout(s: &str) -> String {
    label(s)
        .split(' ')
        .map(|w| {
            let up = w.to_uppercase();
            if matches!(up.as_str(), "GMO" | "OG") || up.chars().all(|c| matches!(c, 'I' | 'V' | 'X')) { up } else { w.to_string() }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Day and Night Market: `FarmUpg[2 + i]`; `MarketInfo` col 7 is the max. N.js `MarketMaxLV`
/// adds the Day 'N' Nite research node (`Grid_Bonus` 171) to every row but Land Plots (0),
/// Overgrowth (8) and Land Rank (13). Its most is 50 a level at its max level, times its best
/// shape (`Research[5]`), before the grid-wide multiplier.
///
/// Costs: N.js `MarketCostType` (the crop: col 2 + col 3 × level; for Land Plots the level counts
/// 2 × floor(lv/3) and floor(lv/4) extra) and `MarketCostQTY` (col 4 × col 5 ^ level, before the
/// Emperor and Exotic discounts). `MarketUpgUnlocked`: a row opens at col 6 crops found.
fn markets(save: &Save) -> Mechanic {
    let levels = save.numbers("FarmUpg", &[]);
    let node = rows("ResGridSquares", &[]).get(171).map_or(0.0, |r| cell(r, 1) * cell(r, 2));
    let shape = rows("Research", &[5]).iter().map(|v| num(v)).fold(0.0, f64::max);
    let raise = (node * (1.0 + shape / 100.0)).floor();
    let found = save.get("FarmCrop").and_then(Value::as_object).map_or(0, |o| o.len()) as f64;
    // Your own Day 'N' Nite today, before the grid-wide multiplier: 50 a level times your shape
    // (`Research[1][171]` indexes `Research[5]`, -1 for none). A floor on your real raise.
    let node_lv = save.at("Research", &[0, 171]).map_or(0.0, num);
    let shape_i = save.at("Research", &[1, 171]).map_or(-1.0, num);
    let own_shape = if shape_i >= 0.0 { rows("Research", &[5]).get(shape_i as usize).map_or(0.0, |v| num(v)) } else { 0.0 };
    let per = rows("ResGridSquares", &[]).get(171).map_or(0.0, |r| cell(r, 2));
    let own_raise = (per * node_lv * (1.0 + own_shape / 100.0)).floor();
    let items = rows("MarketInfo", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let max = if matches!(i, 0 | 8 | 13) { cell(r, 7) } else { cell(r, 7) + raise };
            let lv = at(&levels, i + 2);
            let mut it = Item::new(shout(text(r, 0)), None, lv, max);
            if found < cell(r, 6) {
                it.how = Some(format!("Opens at {} crops found. You have {}.", cell(r, 6), found));
                it.state = State::Locked;
            } else if !matches!(i, 0 | 8 | 13) && lv >= cell(r, 7) + own_raise && node_lv <= 0.0 {
                it.how = Some(format!("Capped at {} until you level the Day 'N' Nite research node.", cell(r, 7)));
                it.state = State::Locked;
            } else if lv < max {
                let step = if i == 0 { lv + 2.0 * (lv / 3.0).floor() + (lv / 4.0).floor() } else { lv };
                let crop = (cell(r, 2) + cell(r, 3) * step).floor().max(0.0) as usize;
                let cost = (cell(r, 4) * cell(r, 5).powf(lv)).floor();
                let held = crop_held(save, crop);
                it.how = Some(format!("Next level costs {} {} before discounts. You hold {}.", big(cost), crop_name(crop), big(held)));
                // At or past the floor of your own cap, only the game knows the rest of it.
                if cost.is_finite() && held >= cost && (matches!(i, 0 | 8 | 13) || lv < cell(r, 7) + own_raise) {
                    it.state = State::Ready;
                }
            }
            it
        })
        .collect();
    let mut m = mech("farm-market", 6, "Day and Night Market", None, Count::Levels, "levels", items);
    m.raises = vec![Raise {
        name: "Day 'N' Nite research node".into(),
        amount: format!("up to +{raise} on 13 of 16"),
        owned: save.at("Research", &[0, 171]).map_or(0.0, num) > 0.0,
    }];
    m.note = Some("Caps include the Day 'N' Nite research node at its max level in its best shape. Research grid multipliers can raise them further.".into());
    m
}

/// Exotic Market: `FarmUpg[20 + i]`. The bonus nears its top as levels climb, so no level is
/// the last one. `NAME_MAGNI` rows are unused. No "how": the shop offers a few at a time, and
/// which ones is kept outside the save.
fn exotic_market(save: &Save) -> Mechanic {
    let levels = save.numbers("FarmUpg", &[]);
    let items = rows("MarketExoticInfo", &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| text(r, 0) != "NAME_MAGNI")
        .map(|(i, r)| Item::new(shout(text(r, 0)), None, at(&levels, i + 20), 0.0))
        .collect();
    let mut m = mech("exotic-market", 6, "Exotic Market", None, Count::Open, "levels", items);
    m.note = Some("No max: each level adds less, toward the bonus's top.".into());
    m
}

/// Land Rank Database: `FarmRank[2][i]`, names in `NinjaInfo[34]`. Only the 5th column has a cap
/// (N.js `LandRank5thColumnMaxLV`): 1 + Grimoire upgrade 9 + ceil(Exotic DATADIGGING) + Legend
/// Talent 3, each at its most. DATADIGGING doubles when it is organic.
fn land_rank(save: &Save) -> Mechanic {
    let levels = save.numbers("FarmRank", &[2]);
    let grimoire = rows("GrimoireUpg", &[]).get(9).map_or(0.0, |r| cell(r, 4) * cell(r, 5));
    let digging = rows("MarketExoticInfo", &[]).get(15).map_or(0.0, |r| (2.0 * cell(r, 3)).ceil());
    let node = rows("ResGridSquares", &[]).get(130).map_or(0.0, |r| cell(r, 1));
    let legend = rows("LegendTalents", &[]).get(3).map_or(0.0, |r| (cell(r, 1) + node) * cell(r, 2));
    let max = (1.0 + grimoire + digging + legend).round();
    // N.js `LankRankPTSleft`: the 36 plot ranks added up, less every database level bought. A
    // level costs one point; an upgrade opens at the total rank in `NinjaInfo[37]`.
    let total: f64 = save.numbers("FarmRank", &[0]).iter().take(36).sum();
    let spent: f64 = levels.iter().take(20).sum();
    let left = (total - spent).max(0.0);
    let unlock = rows("NinjaInfo", &[37]);
    // The cap today, the same sum with your own levels (DATADIGGING: 5 × lv / (lv + 1000),
    // doubled when organic, `Spelunk[50]`).
    let g_lv = save.at("Grimoire", &[9]).map_or(0.0, num);
    let d_lv = at(&save.numbers("FarmUpg", &[]), 35);
    let organic = save.numbers("Spelunk", &[50]).contains(&15.0);
    let d_coef = rows("MarketExoticInfo", &[]).get(15).map_or(0.0, |r| cell(r, 3)) * if organic { 2.0 } else { 1.0 };
    let t_lv = save.at("Spelunk", &[18, 3]).map_or(0.0, num);
    let per_t = rows("LegendTalents", &[]).get(3).map_or(0.0, |r| cell(r, 2));
    let per_g = rows("GrimoireUpg", &[]).get(9).map_or(0.0, |r| cell(r, 5));
    let now = (1.0 + g_lv * per_g + (d_coef * d_lv / (d_lv + 1000.0)).ceil() + t_lv * per_t).round().min(max);
    let items: Vec<Item> = rows("NinjaInfo", &[34])
        .iter()
        .enumerate()
        .filter(|(i, _)| i % 5 == 4)
        .map(|(i, v)| {
            let lv = at(&levels, i);
            let mut it = Item::new(label(v.as_str().unwrap_or("")), None, lv, max);
            let need = unlock.get(i).map_or(0.0, |u| num(u));
            if total < need {
                it.how = Some(format!("Opens at a total land rank of {need}. Yours is {total}."));
                it.state = State::Locked;
            } else if lv >= now && lv < max {
                it.how = Some(format!("At your cap of {now}. The raises below lift it."));
                it.state = State::Locked;
            } else if lv < max {
                it.how = Some("Next level costs 1 land rank point.".into());
                if left >= 1.0 {
                    it.state = State::Ready;
                }
            }
            it
        })
        .collect();
    let mut m = mech("land-rank", 6, "Land Rank Database", None, Count::Levels, "levels", items);
    m.have = Some(Have {
        figure: format!("{} land rank points", big(left)),
        text: if left >= 1.0 { format!("Enough for {} levels across the database.", big(left)) } else { "No points left: plot ranks earn more.".into() },
    });
    m.raises = vec![
        Raise { name: "Grimoire: Land Rank Database Maxim".into(), amount: format!("+{grimoire}"), owned: g_lv * per_g >= grimoire && grimoire > 0.0 },
        Raise { name: "Exotic Market: DATADIGGING".into(), amount: format!("up to +{digging} when organic"), owned: d_lv > 0.0 },
        Raise { name: "Legend Talent: Extended Database".into(), amount: format!("+{legend}"), owned: t_lv * per_t >= legend && legend > 0.0 },
    ];
    m.note = Some(format!(
        "Only the 5th column has a cap, raised by a Grimoire upgrade, the DATADIGGING exotic (organic) and a Legend Talent. The rest level up without end. Your cap today is {now}."
    ));
    m
}

/// Jade Emporium: bought when `Ninja[102][9]` holds the letter for its index (`_`, then a to z,
/// then A to Z). N.js `AvailableEmporiumPurchases` is 48; later rows are `UNDER_CONSTRUCTION`.
///
/// Cost: N.js `EmporiumCost(p)`, where `p` is the upgrade's place in the shop order
/// (`NinjaInfo[24]`): (300 + 500p + p³) × 2.52^p × 3.07^(p-28) × 160^(p-38) × 300^(p-44), each
/// extra factor only past its threshold. Paid in jade coins, `Ninja[102][1]`.
fn jade_emporium(save: &Save) -> Mechanic {
    let bought = save.at("Ninja", &[102, 9]).and_then(Value::as_str).unwrap_or("");
    let jade = save.at("Ninja", &[102, 1]).map_or(0.0, num).max(0.0) + 0.0;
    let letters: Vec<char> = std::iter::once('_').chain('a'..='z').chain('A'..='Z').collect();
    let order: Vec<usize> = rows("NinjaInfo", &[24]).iter().map(|v| num(v) as usize).collect();
    let mut costs: Vec<(f64, String)> = Vec::new();
    let items: Vec<Item> = rows("JadeUpg", &[])
        .iter()
        .take(48)
        .enumerate()
        .filter(|(_, r)| text(r, 0) != "UNDER_CONSTRUCTION")
        .map(|(i, r)| {
            let have = letters.get(i).is_some_and(|c| bought.contains(*c));
            let mut it = Item::new(label(text(r, 0)), None, f64::from(u8::from(have)), 1.0);
            if !have && let Some(p) = order.iter().position(|&k| k == i) {
                let cost = emporium_cost(p as f64);
                it.how = Some(format!("Costs {} jade coins.", big(cost)));
                if cost <= jade {
                    it.state = State::Ready;
                    costs.push((cost, it.name.clone()));
                }
            }
            it
        })
        .collect();
    costs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let ready: Vec<(f64, &str)> = costs.iter().map(|(c, n)| (*c, n.as_str())).collect();
    let names: Vec<&str> = ready.iter().map(|r| r.1).collect();
    let text = match names.as_slice() {
        [] => "Not enough for anything left in the shop yet.".to_string(),
        [one] => format!("Enough for {one}."),
        [a, b] => format!("Enough for {a} and {b}."),
        [a, b, rest @ ..] => format!("Enough for {a}, {b} and {} more, each on its own.", rest.len()),
    };
    let mut m = mech("jade-emporium", 6, "Jade Emporium", None, Count::Done, "bought", items);
    m.have = Some(Have { figure: format!("{} jade coins", big(jade)), text });
    m
}

fn emporium_cost(p: f64) -> f64 {
    (300.0 + 500.0 * p + p.powi(3)) * 2.52f64.powf(p) * 3.07f64.powf((p - 28.0).max(0.0)) * 160f64.powf((p - 38.0).max(0.0)) * 300f64.powf((p - 44.0).max(0.0))
}

/// Ninja upgrades and Ninja Knowledge: `Ninja[103][i]`; `NinjaUpg` col 4 is the name. Costs
/// climb without a cap. Row 9 is a filler named "Name". No "how": N.js `JadeUpgCost` divides by
/// several discounts (Centurion Lineage per 100 levels, Way of Bargain, Emerald, a talent) that
/// a base figure would miss by many orders of magnitude.
fn ninja_upgrades(save: &Save) -> Mechanic {
    let levels = save.numbers("Ninja", &[103]);
    let items = rows("NinjaUpg", &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| text(r, 4) != "Name")
        .map(|(i, r)| Item::new(label(text(r, 4)), pic(format!("NjUpgI{i}")), at(&levels, i), 0.0))
        .collect();
    let mut m = mech("ninja-upgrades", 6, "Sneaking Upgrades", pic("NjUpgI0".into()), Count::Open, "levels", items);
    m.note = Some("No max: these level up without end.".into());
    m
}

/// Pristine Charms: `Ninja[107][i]` is 1 when found; names from `NjEQ` `NjTrP{i}`.
fn pristine_charms(save: &Save) -> Mechanic {
    let found = save.numbers("Ninja", &[107]);
    let items = extra_names("pristine")
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let mut it = Item::new(label(n), pic(format!("NjTrP{i}")), at(&found, i).min(1.0), 1.0);
            if !it.done() {
                it.how = Some("A rare find while Sneaking. Each of the 23 is as likely.".into());
            }
            it
        })
        .collect();
    mech("pristine-charms", 6, "Pristine Charms", pic("NjTrP0".into()), Count::Done, "found", items)
}

/// Gemstones: level in `OptLacc[233 + i]`. The bonus is base + scale × lv / (lv + 1000), so it
/// nears its top without a last level. No "how": the save doesn't say where each one drops.
fn gemstones(save: &Save) -> Mechanic {
    let items = extra_names("gems")
        .iter()
        .enumerate()
        .map(|(i, n)| Item::new(label(n), pic(format!("NjGem{i}")), save.option(233 + i), 0.0))
        .collect();
    let mut m = mech("gemstones", 6, "Gemstones", pic("NjGem0".into()), Count::Open, "levels", items);
    m.note = Some("No max: each level adds less, toward the gemstone's top.".into());
    m
}

/// How many of an item you hold: storage (`ChestOrder` / `ChestQuantity`) and every character's
/// inventory (`InventoryOrder_c` / `ItemQTY_c`).
fn item_held(save: &Save, id: &str) -> f64 {
    let count = |order: &str, qty: &str| -> f64 {
        let (Some(o), q) = (save.get(order).and_then(Value::as_array), save.numbers(qty, &[])) else { return 0.0 };
        o.iter().zip(q).filter(|(n, _)| n.as_str() == Some(id)).map(|(_, q)| q.max(0.0)).sum()
    };
    let chars = save.characters().min(12);
    let n = count("ChestOrder", "ChestQuantity") + (0..chars).map(|c| count(&format!("InventoryOrder_{c}"), &format!("ItemQTY_{c}"))).sum::<f64>();
    n.max(0.0) + 0.0
}

fn extra_names(key: &str) -> Vec<&'static str> {
    GAME.extra("w6").get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default()
}

/// Gold Food Beanstalk: `Ninja[104][i]` is 0 to 3 for the food at `NinjaInfo[29][2 + 3i]`. A
/// 10,000 stack gives 1, 100,000 gives 2 (Supersized Gold Beanstacking), 1,000,000 gives 3
/// (`OptLacc[475]`, the World 7 upgrade).
fn beanstalk(save: &Save) -> Mechanic {
    let tiers = save.numbers("Ninja", &[104]);
    let foods = rows("NinjaInfo", &[29]);
    let bought = save.at("Ninja", &[102, 9]).and_then(Value::as_str).unwrap_or("");
    // Jade Emporium 1 and 2 ("a", "b"): the beanstalk itself, then the 100,000 stack.
    let (stalk, supersized) = (bought.contains('a'), bought.contains('b'));
    let w7 = save.option(475) >= 1.0;
    let items = foods
        .iter()
        .skip(2)
        .step_by(3)
        .enumerate()
        .map(|(i, v)| {
            let id = v.as_str().unwrap_or("");
            let name = GAME.item_name(id).map_or_else(|| label(id), str::to_string);
            let tier = at(&tiers, i);
            let mut it = Item::new(name, pic(id.to_string()), tier, 3.0);
            let (need, gate) = match tier as u8 {
                0 => (1e4, (!stalk).then_some("Gold Food Beanstalk from the Jade Emporium")),
                1 => (1e5, (!supersized).then_some("Supersized Gold Beanstacking from the Jade Emporium")),
                2 => (1e6, (!w7).then_some("the beanstalk upgrade in World 7")),
                _ => return it,
            };
            let held = item_held(save, id);
            if let Some(g) = gate {
                it.how = Some(format!("The next stack needs {g}."));
                it.state = State::Locked;
            } else {
                it.how = Some(format!("Drop a stack of {} at the beanstalk. You hold {}.", big(need), big(held)));
                if held >= need {
                    it.state = State::Ready;
                }
            }
            it
        })
        .collect();
    let mut m = mech("beanstalk", 6, "Gold Food Beanstalk", pic("FoodG1".into()), Count::Levels, "stacks", items);
    m.raises = vec![
        Raise { name: "Supersized Gold Beanstacking".into(), amount: "+1 stack a food".into(), owned: supersized },
        Raise { name: "Beanstalk upgrade in World 7".into(), amount: "+1 stack a food".into(), owned: w7 },
    ];
    m.note = Some("Three stacks a food: 10,000, then 100,000 (Supersized Gold Beanstacking in the Jade Emporium), then 1,000,000 (the beanstalk upgrade in World 7).".into());
    m
}

/// Emperor Showdown: `OptLacc[369]` showdowns won; win `n` gives the bonus `EmperorBon[2][n % 48]`.
/// Bonuses grow with every win. The two "something World 7ish" rows aren't in the game yet.
fn emperor(save: &Save) -> Mechanic {
    let won = save.option(369).clamp(0.0, 1e12) as usize;
    let cycle = rows("EmperorBon", &[2]);
    let mut wins = vec![0.0; rows("EmperorBon", &[0]).len()];
    if !cycle.is_empty() {
        // Whole cycles, then the start of the next: counted, not looped, since the win count
        // comes from the paste.
        let (full, rest) = (won / cycle.len(), won % cycle.len());
        for (n, slot) in cycle.iter().enumerate() {
            if let Some(w) = wins.get_mut(num(slot) as usize) {
                *w += (full + usize::from(n < rest)) as f64;
            }
        }
    }
    let items = rows("EmperorBon", &[0])
        .iter()
        .enumerate()
        .filter_map(|(i, v)| {
            let raw = v.as_str().unwrap_or("");
            if raw.contains("something") {
                return None;
            }
            let name = label(raw);
            let name = name.strip_prefix("x ").unwrap_or(&name);
            let mut c = name.chars();
            let name: String = c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default();
            let mut it = Item::new(name, None, wins[i], 0.0);
            // The next showdown that pays this bonus: within one cycle of 48.
            if let Some(k) = (0..cycle.len()).find(|k| num(cycle[(won + k) % cycle.len()]) as usize == i) {
                it.how = Some(format!("Next one at showdown {}.", won + k + 1));
            }
            Some(it)
        })
        .collect();
    let mut m = mech("emperor", 6, "Emperor Showdown", pic("Boss6A".into()), Count::Open, "wins", items);
    m.note = Some("No max: every showdown won adds to one of these, in a fixed 48-fight cycle.".into());
    m
}

/// Summoning upgrades: `Summon[0][i]`; `SummonUPG` col 8 is the max, 9999 for no cap. Cost:
/// N.js Summoning `UpgCost`, col 4 × col 5 ^ level before the discounts, paid in the essence of
/// colour col 2 (`Summon[2][colour]`). Row 2, Familiar, is left out: the game sets
/// `Summon[0][2]` back to 0 every few days (when `Summon[3][3]` runs out), so it has no lasting level.
fn summoning(save: &Save) -> Mechanic {
    let levels = save.numbers("Summon", &[0]);
    let essence = save.numbers("Summon", &[2]);
    let items = rows("SummonUPG", &[])
        .iter()
        .enumerate()
        .filter(|(i, r)| *i != 2 && cell(r, 8) > 0.0 && cell(r, 8) < 999.0)
        .map(|(i, r)| {
            let lv = at(&levels, i);
            let mut it = Item::new(label(text(r, 3)), None, lv, cell(r, 8));
            let colour = cell(r, 2) as usize;
            if lv < cell(r, 8) && let Some(c) = COLOURS.get(colour) {
                let cost = cell(r, 4) * cell(r, 5).powf(lv);
                let held = at(&essence, colour).max(0.0) + 0.0;
                it.how = Some(format!("Next level costs {} {} essence before discounts. You hold {}.", big(cost), c.to_lowercase(), big(held)));
                if cost.is_finite() && held >= cost {
                    it.state = State::Ready;
                }
            }
            it
        })
        .collect();
    let mut m = mech("summoning", 6, "Summoning Upgrades", None, Count::Levels, "levels", items);
    m.note = Some("Only the upgrades with a cap, and not Familiar, whose level resets every few days. The rest level up without end.".into());
    m
}

const COLOURS: [&str; 8] = ["White", "Green", "Yellow", "Blue", "Purple", "Red", "Cyan", "Teal"];

/// Summoning battles won per colour: `Summon[1]` lists the opponents beaten. White is every
/// `Pet*` opponent; colour `w + 1` is the World `w + 1` Deathnote list (`DeathNoteMobs[w]`), as
/// far as its opponents have a battle in `SummonEnemies[0]` (World 7 stops at 15).
fn summoning_battles(save: &Save) -> Mechanic {
    let beaten: Vec<&str> = save.at("Summon", &[1]).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let enemies: Vec<&str> = rows("SummonEnemies", &[0]).iter().filter_map(|v| v.as_str()).collect();
    let mut lists: Vec<Vec<&str>> = vec![enemies.iter().copied().filter(|e| e.starts_with("Pet")).collect()];
    for world in rows("DeathNoteMobs", &[]) {
        let mobs = world.as_array().map(|a| a.iter().filter_map(Value::as_str).take_while(|m| enemies.contains(m)).collect()).unwrap_or_default();
        lists.push(mobs);
    }
    let opponents = rows("SummonEnemies", &[4]);
    let items = lists
        .iter()
        .take(COLOURS.len())
        .enumerate()
        .map(|(i, list)| {
            let won = list.iter().filter(|e| beaten.contains(e)).count() as f64;
            let name = if i == 0 { "White battles".to_string() } else { format!("{} battles (World {i})", COLOURS[i]) };
            let mut it = Item::new(name, pic(format!("SummC{}", i + 1)), won, list.len() as f64);
            // The first opponent of the colour not beaten yet, by its name in `SummonEnemies[4]`.
            if let Some(next) = list.iter().find(|e| !beaten.contains(e)) {
                let who = enemies.iter().position(|e| e == next).and_then(|k| opponents.get(k)).and_then(|v| v.as_str()).filter(|s| s.len() > 1);
                let who = who.map(label).or_else(|| GAME.monster_name(next).map(str::to_string));
                if let Some(w) = who {
                    it.how = Some(format!("Next opponent: {w}."));
                }
            }
            it
        })
        .collect();
    mech("summoning-battles", 6, "Summoning Battles", pic("SummC2".into()), Count::Levels, "won", items)
}

/// Summoning stones and Endless Summoning: stone wins in `KRbest["SummzTrz{i}"]` (N.js
/// `SumStoneTrialz`; seven stones, no save has an eighth), endless wins in `OptLacc[319]`.
/// Neither ends. No "how": each win makes the next fight stronger, and the check can't read
/// how strong your deck is.
fn summoning_stones(save: &Save) -> Mechanic {
    let mut items: Vec<Item> = (0..7)
        .map(|i| {
            let wins = save.get("KRbest").and_then(|k| k.get(format!("SummzTrz{i}"))).map_or(0.0, num);
            Item::new(format!("{} stone", COLOURS[i]), pic(format!("SummSt{i}")), wins, 0.0)
        })
        .collect();
    items.push(Item::new("Endless battles won", None, save.option(319), 0.0));
    let mut m = mech("summoning-stones", 6, "Summoning Stones and Endless", pic("SummSt0".into()), Count::Open, "wins", items);
    m.note = Some("No max: stones and endless battles keep getting harder.".into());
    m
}
