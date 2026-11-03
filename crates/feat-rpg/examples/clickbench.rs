//! How long the bot's own work takes per click (load the player, run the rules, save, build
//! the screen), on a file database like production. `cargo run --release -p feat-rpg --example clickbench`.
use std::time::Instant;

use bot_core::{Config, Ctx, Db};
use feat_rpg::data::{self, God};
use feat_rpg::engine::expedition::Rates;
use feat_rpg::game::{self, Route};
use feat_rpg::store;

fn main() {
    let rt = bot_core::tokio::runtime::Runtime::new().unwrap();
    let _e = rt.enter();
    let dir = std::env::temp_dir().join(format!("rpg-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(dir.join("bench.db").to_str().unwrap()).unwrap();
    db.migrate("rpg", store::MIGRATIONS).unwrap();
    let ctx = Ctx::new(Config::preview(1), db).unwrap();
    // 30 players with history, so queries run against real-sized tables.
    for u in 1..=30u64 {
        store::create(&ctx.db, 1, u, &format!("Player {u}"), data::GODS[(u % 3) as usize]).unwrap();
        store::run(&ctx.db, 1, u, None, |g| {
            g.xp(20_000)?;
            for _ in 0..200 {
                g.item("iron", 1, "bench")?;
                g.gold(5, "bench")?;
            }
            g.tribute(God::Thor, 900)
        })
        .unwrap();
    }
    let mut times: Vec<(String, Vec<u128>)> = Vec::new();
    for (label, screen, action, arg) in [("open the overview", "hub", "", ""), ("open the forge", "forge", "", ""), ("open the temple", "temple", "", ""), ("hunt turn", "hunt", "act", "strike")] {
        let mut t = Vec::new();
        for i in 0..400 {
            let u = 1 + (i % 30) as u64;
            if action == "act" && store::get(&ctx.db, 1, u).unwrap().unwrap().fight.is_none() {
                let v = store::get(&ctx.db, 1, u).unwrap().unwrap().version;
                let r = Route { owner: u, screen: "hunt".into(), action: "start".into(), arg: "shore".into(), version: v };
                store::run(&ctx.db, 1, u, None, |g| { g.set_hp(9999)?; game::handle(&ctx, g, &r, &[], Rates::default()) }).unwrap();
            }
            let v = store::get(&ctx.db, 1, u).unwrap().unwrap().version;
            let r = Route { owner: u, screen: screen.into(), action: action.into(), arg: arg.into(), version: v };
            let start = Instant::now();
            let out = store::run(&ctx.db, 1, u, Some("Player"), |g| game::handle(&ctx, g, &r, &[], Rates::default())).unwrap().unwrap();
            let _ = out.screen.components();
            t.push(start.elapsed().as_micros());
        }
        t.sort_unstable();
        times.push((label.into(), t));
    }
    println!("{:<20} {:>8} {:>8} {:>8}", "click", "p50 µs", "p95 µs", "max µs");
    for (l, t) in &times {
        println!("{:<20} {:>8} {:>8} {:>8}", l, t[t.len() / 2], t[t.len() * 95 / 100], t[t.len() - 1]);
    }
    // The banner is drawn when Discord first fetches its address, not during the click.
    let mut b = Vec::new();
    for i in 0..30u32 {
        let spec = feat_rpg::banner::Spec { patron: data::GODS[(i % 3) as usize], name: "Antho".into(), level: 14, power: 312 + i as i64, xp10: i % 10, title: String::new() };
        let start = Instant::now();
        let png = bot_render::png(&feat_rpg::banner::svg(&spec)).unwrap();
        b.push((start.elapsed().as_micros(), png.len()));
    }
    b.sort_unstable();
    println!("banner draw: p50 {:.1} ms, max {:.1} ms, {} KB", b[15].0 as f64 / 1000.0, b[29].0 as f64 / 1000.0, b[15].1 / 1024);
    let _ = std::fs::remove_dir_all(dir);
}
