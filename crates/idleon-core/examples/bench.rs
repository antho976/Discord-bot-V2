//! Where a review's time goes, without the web server: read the save, run every check, build
//! the review. `cargo run --release -p idleon-core --example bench -- saves/*.json`.

use std::time::Instant;

use idleon_core::review::Review;
use idleon_core::save::Save;

fn main() {
    let texts: Vec<String> = std::env::args().skip(1).filter_map(|p| std::fs::read_to_string(p).ok()).collect();
    // Load the game data once, as the bot does on its first review.
    let t = Instant::now();
    let warm = Save::parse(&texts[0]).unwrap();
    let _ = idleon_core::run_checks(&warm);
    println!("first review (loads the game data): {:?}", t.elapsed());
    let (mut parse, mut checks, mut review) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..5 {
        for text in &texts {
            let t = Instant::now();
            let s = Save::parse(text).unwrap();
            let a = t.elapsed();
            let ms = idleon_core::run_checks(&s);
            let b = t.elapsed();
            let r = Review::new(&s, ms, idleon_core::world_reached(&s));
            std::hint::black_box(r.share());
            let c = t.elapsed();
            parse.push(a.as_secs_f64() * 1e3);
            checks.push((b - a).as_secs_f64() * 1e3);
            review.push((c - b).as_secs_f64() * 1e3);
        }
    }
    for (name, mut v) in [("parse the save", parse), ("run the checks", checks), ("build the review", review)] {
        v.sort_by(f64::total_cmp);
        println!("{name:>18}: p50 {:.2} ms, p95 {:.2} ms ({} runs)", v[v.len() / 2], v[v.len() * 95 / 100], v.len());
    }
}
