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
    let items = vec![Item::new("Crops discovered", pic("FarmCrop0".into()), found.min(total), total)];
    mech("crops", 6, "Farming Crops", pic("FarmCrop0".into()), Count::Levels, "crops", items)
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
fn markets(save: &Save) -> Mechanic {
    let levels = save.numbers("FarmUpg", &[]);
    let node = rows("ResGridSquares", &[]).get(171).map_or(0.0, |r| cell(r, 1) * cell(r, 2));
    let shape = rows("Research", &[5]).iter().map(|v| num(v)).fold(0.0, f64::max);
    let raise = (node * (1.0 + shape / 100.0)).floor();
    let items = rows("MarketInfo", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let max = if matches!(i, 0 | 8 | 13) { cell(r, 7) } else { cell(r, 7) + raise };
            Item::new(shout(text(r, 0)), None, at(&levels, i + 2), max)
        })
        .collect();
    let mut m = mech("farm-market", 6, "Day and Night Market", None, Count::Levels, "levels", items);
    m.note = Some("Caps include the Day 'N' Nite research node at its max level in its best shape. Research grid multipliers can raise them further.".into());
    m
}

/// Exotic Market: `FarmUpg[20 + i]`. The bonus nears its top as levels climb, so no level is
/// the last one. `NAME_MAGNI` rows are unused.
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
    let items = rows("NinjaInfo", &[34])
        .iter()
        .enumerate()
        .filter(|(i, _)| i % 5 == 4)
        .map(|(i, v)| Item::new(label(v.as_str().unwrap_or("")), None, at(&levels, i), max))
        .collect();
    let mut m = mech("land-rank", 6, "Land Rank Database", None, Count::Levels, "levels", items);
    m.note = Some("Only the 5th column has a cap, raised by a Grimoire upgrade, the DATADIGGING exotic (organic) and a Legend Talent. The rest level up without end.".into());
    m
}

/// Jade Emporium: bought when `Ninja[102][9]` holds the letter for its index (`_`, then a to z,
/// then A to Z). N.js `AvailableEmporiumPurchases` is 48; later rows are `UNDER_CONSTRUCTION`.
fn jade_emporium(save: &Save) -> Mechanic {
    let bought = save.at("Ninja", &[102, 9]).and_then(Value::as_str).unwrap_or("");
    let letters: Vec<char> = std::iter::once('_').chain('a'..='z').chain('A'..='Z').collect();
    let items = rows("JadeUpg", &[])
        .iter()
        .take(48)
        .enumerate()
        .filter(|(_, r)| text(r, 0) != "UNDER_CONSTRUCTION")
        .map(|(i, r)| {
            let have = letters.get(i).is_some_and(|c| bought.contains(*c));
            Item::new(label(text(r, 0)), None, f64::from(u8::from(have)), 1.0)
        })
        .collect();
    mech("jade-emporium", 6, "Jade Emporium", None, Count::Done, "bought", items)
}

/// Ninja upgrades and Ninja Knowledge: `Ninja[103][i]`; `NinjaUpg` col 4 is the name. Costs
/// climb without a cap. Row 9 is a filler named "Name".
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
        .map(|(i, n)| Item::new(label(n), pic(format!("NjTrP{i}")), at(&found, i).min(1.0), 1.0))
        .collect();
    mech("pristine-charms", 6, "Pristine Charms", pic("NjTrP0".into()), Count::Done, "found", items)
}

/// Gemstones: level in `OptLacc[233 + i]`. The bonus is base + scale × lv / (lv + 1000), so it
/// nears its top without a last level.
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

fn extra_names(key: &str) -> Vec<&'static str> {
    GAME.extra("w6").get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default()
}

/// Gold Food Beanstalk: `Ninja[104][i]` is 0 to 3 for the food at `NinjaInfo[29][2 + 3i]`. A
/// 10,000 stack gives 1, 100,000 gives 2 (Supersized Gold Beanstacking), 1,000,000 gives 3
/// (`OptLacc[475]`, the World 7 upgrade).
fn beanstalk(save: &Save) -> Mechanic {
    let tiers = save.numbers("Ninja", &[104]);
    let foods = rows("NinjaInfo", &[29]);
    let items = foods
        .iter()
        .skip(2)
        .step_by(3)
        .enumerate()
        .map(|(i, v)| {
            let id = v.as_str().unwrap_or("");
            let name = GAME.item_name(id).map_or_else(|| label(id), str::to_string);
            Item::new(name, pic(id.to_string()), at(&tiers, i), 3.0)
        })
        .collect();
    let mut m = mech("beanstalk", 6, "Gold Food Beanstalk", pic("FoodG1".into()), Count::Levels, "stacks", items);
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
            Some(Item::new(name, None, wins[i], 0.0))
        })
        .collect();
    let mut m = mech("emperor", 6, "Emperor Showdown", pic("Boss6A".into()), Count::Open, "wins", items);
    m.note = Some("No max: every showdown won adds to one of these, in a fixed 48-fight cycle.".into());
    m
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
    let items = lists
        .iter()
        .take(COLOURS.len())
        .enumerate()
        .map(|(i, list)| {
            let won = list.iter().filter(|e| beaten.contains(e)).count() as f64;
            let name = if i == 0 { "White battles".to_string() } else { format!("{} battles (World {i})", COLOURS[i]) };
            Item::new(name, pic(format!("SummC{}", i + 1)), won, list.len() as f64)
        })
        .collect();
    mech("summoning-battles", 6, "Summoning Battles", pic("SummC2".into()), Count::Levels, "won", items)
}

/// Summoning stones and Endless Summoning: stone wins in `KRbest["SummzTrz{i}"]` (N.js
/// `SumStoneTrialz`; seven stones, no save has an eighth), endless wins in `OptLacc[319]`.
/// Neither ends.
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
