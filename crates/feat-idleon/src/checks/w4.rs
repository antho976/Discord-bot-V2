//! World 4: Hyperion Nebula.

use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![
        jewels(save),
    ]
}

/// Lab jewels: `Lab[14][i]` is 1 once owned.
fn jewels(save: &Save) -> Mechanic {
    let owned = save.numbers("Lab", &[14]);
    let items = rows("JewelDesc", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| Item::new(label(text(r, 11)), pic(format!("ConsoleJwl{i}")), f64::from(at(&owned, i) >= 1.0), 1.0))
        .collect();
    mech("jewels", 4, "Lab Jewels", pic("ConsoleJwl0".into()), Count::Done, "owned", items)
}
