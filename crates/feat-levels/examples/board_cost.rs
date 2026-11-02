//! What drawing one /level top page costs: the SVG, the PNG render and its size, with ten
//! 128 px faces embedded the way the bot embeds downloaded avatars.
//!
//!     cargo run --release -p feat-levels --example board_cost [-- runs]

use std::time::Instant;

use feat_levels::card::{Board, Row, board_svg};

/// The face downloads for one page: ten Discord avatars, the first time and again straight
/// after, as a member pressing Next and then Previous would. Needs the network.
async fn faces(size: u32) {
    let ctx = bot_core::Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
    let urls: Vec<String> = (0..10).map(|i| format!("https://cdn.discordapp.com/embed/avatars/{}.png?size=128", i % 6)).collect();
    for pass in ["first", "again"] {
        let t = Instant::now();
        let got = feat_levels::card::avatars(&ctx, urls.clone(), size).await;
        println!("faces {pass:5} {} ms, {} of 10 arrived", t.elapsed().as_millis(), got.iter().flatten().count());
    }
}

fn main() -> bot_core::anyhow::Result<()> {
    if let Some(size) = std::env::var("FACES").ok().and_then(|s| s.parse().ok()) {
        bot_core::tokio::runtime::Runtime::new()?.block_on(faces(size));
        return Ok(());
    }
    let runs: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(30);
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../core/preview");
    let faces: Vec<String> = (1..=10).map(|i| bot_render::data_uri(&std::fs::read(format!("{dir}/av{i}.jpg")).unwrap())).collect();
    let rows = |faces: &[String]| -> Vec<Row> {
        (0..10)
            .map(|i| Row {
                place: i + 1,
                name: format!("Member number {i}"),
                avatar: if std::env::var_os("NO_FACES").is_some() { None } else { Some(faces[i as usize].clone()) },
                level: 85 - i as u32 * 5,
                figure: 2_687_776_484 / (i + 1).pow(3),
                prestige: 0,
                you: i == 5,
            })
            .collect()
    };
    let mut svg_us = Vec::new();
    let mut png_us = Vec::new();
    let mut bytes = 0;
    for _ in 0..runs {
        let t = Instant::now();
        let svg = board_svg(&Board { title: "Nephilheim's server".into(), week: false, page: 0, pages: 27, rows: rows(&faces) });
        svg_us.push(t.elapsed().as_micros());
        let t = Instant::now();
        let png = bot_render::png(&svg)?;
        png_us.push(t.elapsed().as_micros());
        bytes = png.len();
    }
    svg_us.sort();
    png_us.sort();
    let pct = |v: &[u128], p: f64| v[((v.len() - 1) as f64 * p) as usize];
    println!("runs {runs}");
    println!("svg  p50 {} us  p95 {} us", pct(&svg_us, 0.5), pct(&svg_us, 0.95));
    println!("png  p50 {} us  p95 {} us", pct(&png_us, 0.5), pct(&png_us, 0.95));
    println!("png size {} KB", bytes / 1024);
    if let Some(out) = std::env::var_os("BOARD_PNG") {
        let svg = board_svg(&Board { title: "Nephilheim's server".into(), week: false, page: 0, pages: 27, rows: rows(&faces) });
        std::fs::write(out, bot_render::png(&svg)?)?;
    }
    Ok(())
}
