//! Which mechanics leave items without a "how", over many saves:
//! `cargo run -p idleon-core --example coverage -- saves/*.json`.

use std::collections::BTreeMap;

use idleon_core::review::Count;
use idleon_core::save::Save;

fn main() {
    // mechanic -> (items open, items open with a how, saves)
    let mut seen: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    for path in std::env::args().skip(1) {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(save) = Save::parse(&text) else { continue };
        for m in idleon_core::run_checks(&save) {
            let open: Vec<_> = m.items.iter().filter(|i| !i.done()).collect();
            let e = seen.entry(format!("W{} {}{}", m.world, m.name, if m.count == Count::Open { " (no max)" } else { "" })).or_default();
            e.0 += open.len();
            e.1 += open.iter().filter(|i| i.how.is_some()).count();
            e.2 += 1;
        }
    }
    for (name, (open, how, _)) in &seen {
        if *open > 0 && how * 10 < open * 9 {
            println!("{:>4}% of {:>5} open items have a how  {name}", how * 100 / open, open);
        }
    }
}
