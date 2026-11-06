//! How much memory the game data takes once loaded: `cargo run --release -p feat-idleon --example gamemem`.

fn rss() -> u64 {
    std::fs::read_to_string("/proc/self/status").ok().and_then(|s| s.lines().find(|l| l.starts_with("VmRSS")).and_then(|l| l.split_whitespace().nth(1)?.parse().ok())).unwrap_or(0)
}

fn main() {
    let before = rss();
    let s = feat_idleon::save::Save::parse(r#"{"OptLacc": []}"#).unwrap();
    let t = std::time::Instant::now();
    let _ = feat_idleon::run_checks(&s);
    println!("game data: +{} KB resident, {:?} to load", rss() - before, t.elapsed());
}
