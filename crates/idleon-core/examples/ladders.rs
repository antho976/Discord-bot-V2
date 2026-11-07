//! Prints every ladder an empty account sees in a given world: `--example ladders -- 5`.
use idleon_core::review::Review;
use idleon_core::save::Save;
fn main() {
    let w: u8 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(7);
    let s = Save::parse(r#"{"OptLacc": []}"#).unwrap();
    let r = Review::new(&s, idleon_core::run_checks(&s), w);
    let (mut crowd, mut generic, mut none) = (0, 0, 0);
    for m in &r.mechanics {
        let t = m.tiers();
        if t.is_empty() { none += 1; continue; }
        if m.crowd.is_some() { crowd += 1 } else { generic += 1 }
        let labels: Vec<String> = t.iter().map(|t| format!("{} ({})", t.label, t.need)).collect();
        println!("{:<6} W{} {:<22} {}", if m.crowd.is_some() { "crowd" } else { "game" }, m.world, m.key, labels.join(" | "));
    }
    println!("crowd {crowd}, game-max {generic}, no ladder {none}; pace: {:?}", r.overdue().iter().map(|(w, v)| (*w, v.len())).collect::<Vec<_>>());
}
