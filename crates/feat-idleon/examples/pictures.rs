//! Which items and mechanics show no picture, over many saves:
//! `cargo run -p feat-idleon --example pictures -- saves/*.json`. A picture ID the crate doesn't
//! have is listed with its ID; an item with no ID at all as "(no id)".

use std::collections::{BTreeMap, BTreeSet};

use feat_idleon::save::Save;

fn main() {
    // mechanic -> items without a picture (name, the ID it asked for)
    let mut missing: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
    let mut items: BTreeMap<String, usize> = BTreeMap::new();
    for path in std::env::args().skip(1) {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(save) = Save::parse(&text) else { continue };
        for m in feat_idleon::run_checks(&save) {
            let key = format!("W{} {} ({})", m.world, m.name, m.key);
            let n = items.entry(key.clone()).or_default();
            *n = (*n).max(m.items.len());
            if !m.picture.as_deref().is_some_and(feat_idleon::has_picture) {
                missing.entry(key.clone()).or_default().insert(("[card]".into(), m.picture.clone().unwrap_or_else(|| "(no id)".into())));
            }
            for i in &m.items {
                if !i.picture.as_deref().is_some_and(feat_idleon::has_picture) {
                    missing.entry(key.clone()).or_default().insert((i.name.clone(), i.picture.clone().unwrap_or_else(|| "(no id)".into())));
                }
            }
        }
    }
    for (key, set) in &missing {
        let n = items.get(key).copied().unwrap_or(0);
        println!("{key}: {} of {n} without a picture", set.iter().filter(|(name, _)| name != "[card]").count());
        for (name, id) in set {
            println!("    {name:<36} {id}");
        }
    }
}
