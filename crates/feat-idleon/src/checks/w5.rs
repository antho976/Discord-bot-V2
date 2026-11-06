//! World 5: Smolderin' Plateau.

use bot_core::serde_json::Value;

use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![
        artifacts(save),
        gods(save),
        slab(save),
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

/// The Slab: items ever looted (`Cards1`) against every item it lists.
fn slab(save: &Save) -> Mechanic {
    let found = save.get("Cards1").and_then(Value::as_array).map_or(0, Vec::len) as f64;
    let total = rows("SlabItemSort", &[]).len() as f64;
    let items = vec![Item::new("Items looted", None, found.min(total), total)];
    mech("slab", 5, "The Slab", None, Count::Levels, "items", items)
}
