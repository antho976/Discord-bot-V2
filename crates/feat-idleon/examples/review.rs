//! Read a save and print what each check finds: `cargo run -p feat-idleon --example review -- save.json`.
//! Flags any level above its max and any picture the crate doesn't have.

use feat_idleon::review::Review;
use feat_idleon::save::Save;

fn main() {
    for path in std::env::args().skip(1) {
        let text = std::fs::read_to_string(&path).expect("read the save");
        let t = std::time::Instant::now();
        let save = Save::parse(&text).unwrap_or_else(|e| panic!("{path}: {}", e.message()));
        let r = Review::new(&save, feat_idleon::run_checks(&save), feat_idleon::world_reached(&save));
        let took = t.elapsed();
        println!("== {path}: {} · world {} · {} characters · {:.0}% · {took:?}", r.name, r.world, r.characters, r.share() * 100.0);
        for m in &r.mechanics {
            let over: Vec<String> = m.items.iter().filter(|i| i.max > 0.0 && i.level > i.max).map(|i| format!("{} {}>{}", i.name, i.level, i.max)).collect();
            let missing = m.items.iter().filter(|i| i.picture.as_deref().is_some_and(|p| !feat_idleon::has_picture(p))).count();
            println!(
                "  W{} {:<20} {:>7.0} / {:<7.0} {:>4.0}%  items {:>3}{}{}",
                m.world, m.name, m.have(), m.total(), m.share() * 100.0, m.items.len(),
                if over.is_empty() { String::new() } else { format!("  OVER: {}", over.join(", ")) },
                if missing == 0 { String::new() } else { format!("  no picture: {missing}") },
            );
        }
        println!("  do next:");
        for n in &r.next {
            println!("    {} ({:?})", r.mechanics[n.mechanic].name, n.why);
        }
    }
}
