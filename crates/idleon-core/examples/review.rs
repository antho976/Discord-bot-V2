//! Read a save and print what each check finds: `cargo run -p idleon-core --example review -- save.json`.
//! Flags any level above its max and any picture the crate doesn't have, and lists what's
//! overdue for the account's world (from expected.txt).

use idleon_core::review::Review;
use idleon_core::save::Save;

fn main() {
    for path in std::env::args().skip(1) {
        let text = std::fs::read_to_string(&path).expect("read the save");
        let t = std::time::Instant::now();
        let save = Save::parse(&text).unwrap_or_else(|e| panic!("{path}: {}", e.message()));
        let r = Review::new(&save, idleon_core::run_checks(&save), idleon_core::world_reached(&save));
        let took = t.elapsed();
        println!("== {path}: {} · world {} · {} characters · {:.0}% · {took:?}", r.name, r.world, r.characters, r.share() * 100.0);
        for m in &r.mechanics {
            let over: Vec<String> = m.items.iter().filter(|i| i.max > 0.0 && i.level > i.max).map(|i| format!("{} {}>{}", i.name, i.level, i.max)).collect();
            let missing = m.items.iter().filter(|i| i.picture.as_deref().is_some_and(|p| !idleon_core::has_picture(p))).count();
            println!(
                "  W{} {:<20} {:>7.0} / {:<7.0} {:>4.0}%  items {:>3}{}{}",
                m.world, m.name, m.have(), m.total(), m.share() * 100.0, m.items.len(),
                if over.is_empty() { String::new() } else { format!("  OVER: {}", over.join(", ")) },
                if missing == 0 { String::new() } else { format!("  no picture: {missing}") },
            );
        }
        if std::env::var("HOW").is_ok() {
            // What the card's "what to do" shows: the current tier and its first tasks.
            for m in &r.mechanics {
                let tiers = m.tiers();
                let Some(k) = tiers.iter().position(|t| !t.complete()) else { continue };
                println!("  {} · tier {} of {}: {}", m.name, k + 1, tiers.len(), tiers[k].label);
                for (it, target) in m.tasks(&tiers[k]).iter().take(3) {
                    println!("      {} {} -> {} [{:?}] {}", it.name, m.tier_level(it), target, it.state, it.how.as_deref().unwrap_or("-"));
                }
                if let Some(h) = &m.have {
                    println!("      have: {} · {}", h.figure, h.text);
                }
                for x in &m.raises {
                    println!("      raise: {} {} owned={}", x.name, x.amount, x.owned);
                }
            }
        }
        // From expected.txt: what's overdue for the account's world, most overdue world first.
        let overdue = r.overdue();
        if !overdue.is_empty() {
            println!("  overdue for World {}:", r.world);
            for (w, ms) in overdue {
                let names: Vec<String> = ms
                    .iter()
                    .map(|&i| {
                        let m = &r.mechanics[i];
                        let p = m.pace.as_ref().expect("overdue has a pace");
                        format!("{} (tier {}, at {})", m.name, p.due.map_or(0, |k| k + 1), p.at.map_or("none".into(), |a| format!("W{a}")))
                    })
                    .collect();
                println!("    W{w}: {}", names.join(", "));
            }
        }
    }
}
