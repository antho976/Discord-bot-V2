//! Draw the map in a few states: `cargo run -p feat-rpg --example mapimg -- <dir>`.
fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    for (name, open, expedition, titan) in [
        ("map-new", 1, 0, false),
        ("map-mid", 3, 2, false),
        ("map-all", 7, 3, true),
    ] {
        let spec = feat_rpg::map::Spec {
            open,
            expedition,
            titan,
        };
        let t = std::time::Instant::now();
        let png = bot_render::png(&feat_rpg::map::svg(&spec)).unwrap();
        println!("{name}: {} KB in {:?}", png.len() / 1024, t.elapsed());
        std::fs::write(format!("{dir}/{name}.png"), png).unwrap();
    }
}
