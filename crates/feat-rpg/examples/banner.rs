//! Draw the three patron banners to a folder: `cargo run -p feat-rpg --example banner -- <dir>`.
fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    for g in feat_rpg::data::GODS {
        for (tag, title) in [("", ""), ("-title", "Siren-deaf")] {
            let spec = feat_rpg::banner::Spec {
                patron: g,
                name: "Antho".into(),
                level: 14,
                power: 312,
                xp10: 6,
                title: title.into(),
            };
            let png = bot_render::png(&feat_rpg::banner::svg(&spec)).unwrap();
            std::fs::write(format!("{dir}/banner-{}{tag}.png", g.id()), png).unwrap();
        }
    }
}
