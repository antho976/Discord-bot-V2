//! The checks in `feat_idleon::invariants`, on real saves and their rendered results:
//! `cargo run -p feat-idleon --example invariants -- saves/*.json`. Exits 1 on any problem,
//! so a hook can run it. The unit test runs the same checks on an empty save.

use feat_idleon::review::Review;
use feat_idleon::save::Save;

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: invariants save.json...");
        std::process::exit(2);
    }
    let mut problems = 0;
    for path in &paths {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let save = match Save::parse(&text) {
            Ok(s) => s,
            Err(e) => {
                println!("{path}: skipped, not a save: {}", e.message());
                continue;
            }
        };
        let r = Review::new(&save, feat_idleon::run_checks(&save), feat_idleon::world_reached(&save));
        let mut bad = feat_idleon::invariants::review(&r);
        bad.extend(feat_idleon::invariants::page(&feat_idleon::results_html(&r)));
        println!("{path}: {} mechanics, {} problems", r.mechanics.len(), bad.len());
        for b in &bad {
            println!("  {b}");
        }
        problems += bad.len();
    }
    if problems > 0 {
        std::process::exit(1);
    }
}
