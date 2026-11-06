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
/// climb without a cap. Row 9 is a filler named "Name".
///
/// Cost: N.js `JadeUpgCost`, worked in powers of ten since levels in the thousands overflow:
/// base (10^45 for row 0, 10^floor(2(t-15)) past row 16, 10^55 for row 31, else 1) × col 7 ×
/// col 8^lv × Centurion Lineage (1 / (1 + NLbonuses 0)^floor(lv/100)), divided by Gold Coupon
/// (`NinjaBonus` 11), Way of Bargain (`NLbonuses` 1), Emerald (`GemstoneBonus` 6) and talent 430
/// (its level × floor(all Ninja levels / 10)). Rows 3 and 4 also divide by `NLbonuses` 24 / 25;
/// rows 1 and 13 soften col 8 by `NLbonuses` 26 / 18. Left out, each of which only lowers the
/// cost: extra talent levels from other sources, and the bubble on row 8. So the figure is an
/// upper bound, and Ready (jade covers it) is certain.
///
/// Open: rows 17 on need the Coral Reef's Ninja Knowledge upgrade (`Spelunk[13][3]`) above
/// row - 17. Rows up to 16 need the row named in col 9 at level 1 (row 13 is always open); your
/// highest mastery (`OptLacc[232]`) repoints rows 6, 3, 1 and 0 at 1, 3 and 5.
fn ninja_upgrades(save: &Save) -> Mechanic {
    let levels = save.numbers("Ninja", &[103]);
    let table = rows("NinjaUpg", &[]);
    let jade = save.at("Ninja", &[102, 1]).map_or(0.0, num).max(0.0) + 0.0;
    let d = ninja_discounts(save, &levels);
    let items: Vec<Item> = table
        .iter()
        .enumerate()
        .filter(|(_, r)| text(r, 4) != "Name")
        .map(|(i, r)| {
            let lv = at(&levels, i);
            let mut it = Item::new(label(text(r, 4)), pic(format!("NjUpgI{i}")), lv, 0.0);
            let lg = jade_upg_cost_log10(&d, &levels, i, r);
            if let Some(why) = ninja_locked(save, &levels, i, r) {
                it.how = Some(why);
                it.state = State::Locked;
            } else if lg.is_finite() {
                it.how = Some(format!("Next level costs {} jade coins at most.", big_log10(lg)));
                if jade > 0.0 && lg <= jade.log10() {
                    it.state = State::Ready;
                }
            }
            it
        })
        .collect();
    let ready = items.iter().filter(|it| it.state == State::Ready).count();
    let mut m = mech("ninja-upgrades", 6, "Sneaking Upgrades", pic("NjUpgI0".into()), Count::Open, "levels", items);
    m.have = Some(Have {
        figure: format!("{} jade coins", big(jade)),
        text: if ready == 0 { "Not enough for the next level of any upgrade yet.".into() } else { format!("Enough for the next level of {ready} of these, each on its own.") },
    });
    m.note = Some("No max: these level up without end. Costs count your Ninja discounts, Gold Coupon, Emerald and the Ninja Knowledge talent; extra talent levels from elsewhere make them a little cheaper still.".into());
    m
}

/// Why a Sneaking upgrade can't be picked yet, if it can't (see `ninja_upgrades`).
fn ninja_locked(save: &Save, levels: &[f64], i: usize, row: &Value) -> Option<String> {
    if i >= 17 {
        let reef = save.at("Spelunk", &[13, 3]).map_or(0.0, num);
        let need = (i - 16) as f64;
        return (reef < need).then(|| format!("Needs the Coral Reef's Ninja Knowledge upgrade at level {need}. Yours is {reef}."));
    }
    if i == 13 {
        return None;
    }
    let mastery = save.option(232);
    let pre = match i {
        6 if mastery >= 1.0 => 10,
        3 if mastery >= 1.0 => 5,
        1 if mastery >= 3.0 => 4,
        0 if mastery >= 5.0 => 1,
        _ => cell(row, 9) as usize,
    };
    if at(levels, pre) >= 1.0 {
        return None;
    }
    if pre == i {
        return Some("Opens with a higher Sneaking mastery.".into());
    }
    let name = rows("NinjaUpg", &[]).get(pre).map(|r| label(text(r, 4))).unwrap_or_default();
    Some(format!("Opens once {name} is level 1."))
}

/// The discounts `JadeUpgCost` reads that don't depend on the row.
struct NinjaDiscounts {
    /// Ninja upgrade bonuses by index (`NLbonuses`), for the rows the cost uses.
    nl: [f64; 27],
    /// log10 of the shared divisor: Gold Coupon × Way of Bargain × Emerald × talent 430.
    shared_log: f64,
}

/// `NLbonuses(n)`: (level + Menagerie[7][n]) × col 5 under row 17; rows 6, 7, 10 and 12 add
/// (row 3's level + Menagerie[7][n]) × `OptLacc[231]`; from row 17 on, level × col 5.
fn nl_bonus(save: &Save, levels: &[f64], n: usize) -> f64 {
    let per = rows("NinjaUpg", &[]).get(n).map_or(0.0, |r| cell(r, 5));
    let men = at(&save.numbers("Menagerie", &[7]), n);
    let lv = at(levels, n);
    match n {
        6 | 7 | 10 | 12 => (lv + men) * per + (at(levels, 3) + men) * save.option(231),
        _ if n < 17 => (lv + men) * per,
        _ => lv * per,
    }
}

/// `GemstoneBonus(t)`: base + scale × lv / (lv + 1000) once found; all but Moissanite (5) are
/// then × (1 + Moissanite / 100) × the best Generational Gemstones talent (432, decayMulti 3/300).
fn gem_bonus(save: &Save, t: usize) -> f64 {
    let lv = save.option(233 + t);
    if lv <= 0.5 {
        return 0.0;
    }
    let vals = GAME.extra("w6").get("gem_values").and_then(|v| v.get(t));
    let (base, scale) = (vals.and_then(|v| v.get(0)).map_or(0.0, num), vals.and_then(|v| v.get(1)).map_or(0.0, num));
    let v = base + scale * lv / (lv + 1000.0);
    if t == 5 {
        return v;
    }
    let t432 = best_talent(save, 432).map_or(1.0, |l| 1.0 + 3.0 * l / (l + 300.0));
    v * (1.0 + gem_bonus(save, 5) / 100.0) * t432.max(1.0)
}

/// A talent's level over your characters (`SL_c`): the highest, or with `lowest`, the lowest
/// among those that have it.
fn character_talent(save: &Save, talent: usize, lowest: bool) -> Option<f64> {
    let key = talent.to_string();
    let lv = (0..save.characters().min(12)).filter_map(|c| save.get(&format!("SL_{c}")).and_then(|m| m.get(&key)).map(num)).filter(|l| *l > 0.0);
    if lowest { lv.reduce(f64::min) } else { lv.reduce(f64::max) }
}

fn best_talent(save: &Save, talent: usize) -> Option<f64> {
    character_talent(save, talent, false)
}

/// Gold Coupon and the other Gold_ charms (N.js `NinjaBonus` with -1): over the 40 Sneaking
/// inventory slots (`Ninja[60 + e]`, [id, level]) in order, a Gold_ charm whose `ItemStat` beats
/// what its type holds sets it to that stat × (1 + Starite) × (1 + Legend Talent 6) × (1 + the
/// slot's symbol, 50 × `Spelunk[14][24 + e]`). Returns type 11.
fn gold_coupon(save: &Save) -> f64 {
    let gold = GAME.extra("w6").get("gold");
    let starite = gem_bonus(save, 3);
    let legend = rows("LegendTalents", &[]).get(6).map_or(0.0, |r| (save.at("Spelunk", &[18, 6]).map_or(0.0, num) * cell(r, 2)).round());
    let symbols = save.numbers("Spelunk", &[14]);
    let mut held: std::collections::HashMap<i64, f64> = std::collections::HashMap::new();
    for e in 0..40 {
        let Some(slot) = save.at("Ninja", &[60 + e]) else { continue };
        let Some(info) = slot.get(0).and_then(Value::as_str).and_then(|id| gold.and_then(|g| g.get(id))) else { continue };
        let f = |k: usize| info.get(k).map_or(0.0, num);
        let lv = slot.get(1).map_or(0.0, num).max(0.0);
        let (base, cap) = (f(2), f(3));
        let stat = (base + cap * lv / (lv + 50.0)).min(cap) + (cap * lv / (lv + 900.0)).min(cap);
        let kind = f(1) as i64;
        if f(0) == 2.0 && stat > held.get(&kind).copied().unwrap_or(0.0) {
            let mult = (1.0 + starite / 100.0) * (1.0 + legend / 100.0) * (1.0 + 50.0 * at(&symbols, 24 + e) / 100.0);
            held.insert(kind, stat * mult);
        }
    }
    held.get(&11).copied().unwrap_or(0.0)
}

fn ninja_discounts(save: &Save, levels: &[f64]) -> NinjaDiscounts {
    let mut nl = [0.0; 27];
    for (n, v) in nl.iter_mut().enumerate() {
        *v = nl_bonus(save, levels, n);
    }
    let total: f64 = levels.iter().take(64).sum();
    // The lowest level among characters with talent 430, so the discount is never overstated.
    let t430 = character_talent(save, 430, true).unwrap_or(0.0) * (total / 10.0).floor();
    let shared = (1.0 + gold_coupon(save) / 100.0) * (1.0 + nl[1] / 100.0) * (1.0 + gem_bonus(save, 6) / 100.0) * (1.0 + t430 / 100.0);
    NinjaDiscounts { nl, shared_log: shared.max(1.0).log10() }
}

/// log10 of `JadeUpgCost(t)` (see `ninja_upgrades`).
fn jade_upg_cost_log10(d: &NinjaDiscounts, levels: &[f64], t: usize, row: &Value) -> f64 {
    let lv = at(levels, t);
    let base = match t {
        31 => 55.0,
        0 => 45.0,
        _ if t > 16 => (2.0 * (t as f64 - 15.0)).floor(),
        _ => 0.0,
    };
    let growth = match t {
        13 => 1.0 + (cell(row, 8) - 1.0) / (1.0 + d.nl[18] / 1500.0),
        1 => 1.0 + (cell(row, 8) - 1.0) / (1.0 + d.nl[26] / 1500.0),
        _ => cell(row, 8),
    };
    let extra = match t {
        3 => (1.0 + d.nl[24] / 100.0).log10(),
        4 => (1.0 + d.nl[25] / 100.0).log10(),
        _ => 0.0,
    };
    let centurion = (lv / 100.0).floor() * (1.0 + d.nl[0] / 100.0).log10();
    let col7 = cell(row, 7);
    if col7 <= 0.0 || growth <= 0.0 {
        return f64::NAN;
    }
    base + col7.log10() + lv * growth.log10() - centurion - d.shared_log - extra
}

/// A number given as its log10, the way the game writes it: 400K, 96Qa, past that 1.2e345.
fn big_log10(lg: f64) -> String {
    if lg < 300.0 {
        return big(10f64.powf(lg));
    }
    let e = lg.floor();
    format!("{:.1}e{}", 10f64.powf(lg - e), e)
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
/// nears its top without a last level.
///
/// Where: the game adds each to a floor's find list (`NinjaInfo[12 + floor]`) once
/// `OptLacc[231]` (Sneaking mastery) reaches 1 or 2; Emerald and Firefrost also need a Wind
/// Walker (`hasClassTypeN` 29).
fn gemstones(save: &Save) -> Mechanic {
    // (floor shown in game, mastery needed, needs a Wind Walker), by gemstone index.
    const WHERE: [(u8, f64, bool); 8] = [(2, 1.0, false), (4, 1.0, false), (6, 1.0, false), (9, 1.0, false), (3, 2.0, false), (7, 2.0, false), (5, 1.0, true), (10, 1.0, true)];
    let mastery = save.option(231);
    let items = extra_names("gems")
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let mut it = Item::new(label(n), pic(format!("NjGem{i}")), save.option(233 + i), 0.0);
            if let Some(&(floor, need, ww)) = WHERE.get(i) {
                let ww = if ww { " with a Wind Walker" } else { "" };
                if mastery < need {
                    it.how = Some(format!("Found on Sneaking floor {floor}{ww}, from mastery {need}. Yours is {mastery}."));
                    it.state = State::Locked;
                } else {
                    it.how = Some(format!("Found on Sneaking floor {floor}{ww}. Each one adds a level."));
                }
            }
            it
        })
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
/// Neither ends. Opponents: a stone's is `SummonEnemies[4]` from `babaMummy` on; the next
/// endless one is `rift1` + min(4, floor(wins / 20)) (N.js, `_GenINFO[132]` for type 9).
fn summoning_stones(save: &Save) -> Mechanic {
    let ids = rows("SummonEnemies", &[0]);
    let names = rows("SummonEnemies", &[4]);
    let name_at = |k: usize| names.get(k).and_then(|v| v.as_str()).filter(|s| s.len() > 1).map(label);
    let first = ids.iter().position(|v| v.as_str() == Some("babaMummy"));
    let mut items: Vec<Item> = (0..7)
        .map(|i| {
            let wins = save.get("KRbest").and_then(|k| k.get(format!("SummzTrz{i}"))).map_or(0.0, num);
            let mut it = Item::new(format!("{} stone", COLOURS[i]), pic(format!("SummSt{i}")), wins, 0.0);
            if let Some(who) = first.and_then(|f| name_at(f + i)) {
                it.how = Some(format!("Opponent: {who}. Win {} is next.", wins.max(0.0) + 1.0));
            }
            it
        })
        .collect();
    let endless = save.option(319).max(0.0);
    let mut it = Item::new("Endless battles won", None, endless, 0.0);
    if let Some(who) = ids.iter().position(|v| v.as_str() == Some("rift1")).and_then(|f| name_at(f + (endless / 20.0).floor().min(4.0) as usize)) {
        it.how = Some(format!("Next opponent: {who}."));
    }
    items.push(it);
    let mut m = mech("summoning-stones", 6, "Summoning Stones and Endless", pic("SummSt0".into()), Count::Open, "wins", items);
    m.note = Some("No max: stones and endless battles keep getting harder.".into());
    m
}
