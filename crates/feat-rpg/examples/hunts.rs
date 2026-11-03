//! Early-game hunt numbers: turns per fight and HP lost, per patron and region.
use feat_rpg::data;
use feat_rpg::engine::Hero;
use feat_rpg::engine::combat::{End, Fight, autoplay};

fn main() {
    for (power, label) in [(45, "new player, level 1"), (90, "level 5 with starter gear"), (160, "level 8, warhammer + mail")] {
        for region in data::REGIONS {
            for main in data::GODS {
                let h = Hero { main, lesser: None, bonds: [0, 0, 0], power, scouting: 0, mods: Default::default() };
                let (max, atk) = h.stats();
                let (mut turns, mut lost, mut wins, n) = (0u32, 0i64, 0, 200u64);
                for seed in 0..n {
                    let mut f = Fight::hunt(h.clone(), max, region, seed);
                    while f.end.is_none() && f.turn < 100 {
                        let a = autoplay(&f, 0, (0, 0));
                        f.act(a, 0, (0, 0)).unwrap();
                    }
                    turns += f.turn;
                    lost += max - f.hp;
                    if f.end == Some(End::Won) { wins += 1; }
                }
                println!("{label:<28} {:<14} {:<7} hp {max:>3} atk {atk:>3} | win {:>3}% | {:>4.1} turns | loses {:>3.0}% HP per fight",
                    region.name, main.name(), wins * 100 / n, turns as f64 / n as f64, lost as f64 / n as f64 * 100.0 / max as f64);
            }
        }
    }
}
