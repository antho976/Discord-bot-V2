//! World 3: Frostbite Tundra.

use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![
        salt_lick(save),
        prayers(save),
        atoms(save),
    ]
}

/// Salt Lick: `SaltLick[i]`; `SaltLicks` col 4 is the max, col 0 the salt.
fn salt_lick(save: &Save) -> Mechanic {
    let levels = save.numbers("SaltLick", &[]);
    let items = rows("SaltLicks", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| Item::new(label(text(r, 1)), pic(text(r, 0).to_string()), at(&levels, i), cell(r, 4)))
        .collect();
    mech("salt-lick", 3, "Salt Lick", pic("Refinery1".into()), Count::Levels, "levels", items)
}

/// Prayers: `PrayOwned[i]`; `PrayerInfo` col 9 is the max. Rows named "Some Prayer Name" are fillers.
fn prayers(save: &Save) -> Mechanic {
    let levels = save.numbers("PrayOwned", &[]);
    let items = rows("PrayerInfo", &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| !text(r, 0).starts_with("Some_Prayer"))
        .map(|(i, r)| Item::new(label(text(r, 0)), pic(format!("Prayer{i}")), at(&levels, i), cell(r, 9)))
        .collect();
    mech("prayers", 3, "Prayers", pic("Prayer0".into()), Count::Levels, "levels", items)
}

/// The atom cap, fully raised: 20, +10 (gaming superbit), +20 (compass), +20 (event shop), +1
/// (Menagerie).
const ATOM_MAX: f64 = 71.0;

/// Atom Collider: `Atoms[i]`, one cap for every atom.
fn atoms(save: &Save) -> Mechanic {
    let levels = save.numbers("Atoms", &[]);
    let items = rows("AtomInfo", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let name = text(r, 0).split("_-_").next().unwrap_or("");
            Item::new(label(name), pic(format!("Atom{i}")), at(&levels, i), ATOM_MAX)
        })
        .collect();
    let mut m = mech("atoms", 3, "Atom Collider", pic("Atom0".into()), Count::Levels, "levels", items);
    m.note = Some("The cap starts at 20. A gaming superbit, a compass upgrade, an event shop item and a Menagerie boost raise it to 71.".into());
    m
}
