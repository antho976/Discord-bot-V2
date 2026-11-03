//! Draw the map: `cargo run -p feat-rpg --example mapimg -- <dir>`.
fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let png = bot_render::png(&feat_rpg::map::svg(&feat_rpg::map::Spec {
        open: 3,
        expedition: 2,
        titan: true,
    }))
    .unwrap();
    std::fs::write(format!("{dir}/map.png"), png).unwrap();
}
