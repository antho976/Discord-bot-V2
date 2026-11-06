//! World 1: Blunder Hills.

use bot_core::serde_json::Value;

use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![
        statues(save),
        bribes(save),
        star_signs(save),
        stamps(save),
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
