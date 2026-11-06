//! World 2: Yum-Yum Desert.

use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![
        vials(save),
        sigils(save),
        arcade(save),
    ]
}

/// Vials: `CauldronInfo[4][i]`, max level 13.
fn vials(save: &Save) -> Mechanic {
    let levels = save.numbers("CauldronInfo", &[4]);
    let items = rows("AlchemyDescription", &[4])
        .iter()
        .enumerate()
        .map(|(i, r)| Item::new(label(text(r, 0)), pic(text(r, 5).to_string()), at(&levels, i), 13.0))
        .collect();
    mech("vials", 2, "Vials", pic("aVials13".into()), Count::Levels, "levels", items)
}

const SIGIL_TIERS: [&str; 6] = ["Locked", "Blue", "Yellow", "Red", "Purple", "Green"];

/// Sigils: `CauldronP2W[4]` is [progress, tier] pairs; tier -1 locked to 4 green.
fn sigils(save: &Save) -> Mechanic {
    let p2w = save.numbers("CauldronP2W", &[4]);
    let items = rows("SigilDesc", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let tier = (p2w.get(2 * i + 1).copied().unwrap_or(-1.0) + 1.0).clamp(0.0, 5.0);
            let mut it = Item::new(label(text(r, 0)), pic(format!("aSiga{i}")), tier, 5.0);
            it.level_name = Some(SIGIL_TIERS[tier as usize].into());
            it
        })
        .collect();
    let mut m = mech("sigils", 2, "Sigils", pic("aSiga0".into()), Count::Levels, "tiers", items);
    m.note = Some("Blue, Yellow, Red, Purple, then Green. Red needs Ionized Sigils in the Emporium, Purple a Spelunking lore boss, Green the Eclectic Sigils research.".into());
    m
}

/// Arcade: `ArcadeUpg[i]`, max 102 (101 Cosmic, 102 Omega). The last six rows are fillers.
fn arcade(save: &Save) -> Mechanic {
    let levels = save.numbers("ArcadeUpg", &[]);
    let items = rows("ArcadeShopInfo", &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| !text(r, 0).contains("who_knows"))
        .map(|(i, r)| Item::new(label(text(r, 0)), pic(format!("PachiShopICON{i}")), at(&levels, i), 102.0))
        .collect();
    let mut m = mech("arcade", 2, "Arcade", pic("PachiShopICON0".into()), Count::Levels, "levels", items);
    m.note = Some("100 with gold balls, then 101 (Cosmic, 2x) and 102 (Omega, 6x) with their own balls.".into());
    m
}
