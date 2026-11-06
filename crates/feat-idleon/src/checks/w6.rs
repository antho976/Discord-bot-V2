//! World 6: Spirited Valley.

use bot_core::serde_json::Value;

use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![
        crops(save),
        summoning(save),
    ]
}

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
