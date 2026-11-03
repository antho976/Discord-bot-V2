//! Balance simulator. `cargo run -p feat-rpg --example sim -- boss` prints boss win rates for
//! every pairing; `-- days` plays 30 days as a light and a heavy player for each patron and
//! reports the pace and the economy.

use feat_rpg::data::{self, God};
use feat_rpg::engine::combat::{End, Fight, autoplay};
use feat_rpg::engine::{Hero, sim};

fn boss_rate(main: God, lesser: Option<God>, power: i64, facts: &[String], n: u64) -> (u32, f64) {
    let mut wins = 0;
    let mut turns = 0u64;
    for seed in 0..n {
        let h = Hero {
            main,
            lesser,
            bonds: [2, 2, 2],
            power,
            scouting: 0,
            mods: Default::default(),
        };
        let (max, _) = h.stats();
        let mut f = Fight::boss(h, max, seed as i64, facts.to_vec(), seed * 7 + 1);
        let (mut p, mut o) = (3i64, (2i64, 2i64));
        for _ in 0..300 {
            if f.end.is_some() {
                break;
            }
            let a = autoplay(&f, p, o);
            f.act(a, p, o).unwrap();
            for u in f.used.drain(..) {
                match u.as_str() {
                    "potion" => p -= 1,
                    "fireoil" => o.0 -= 1,
                    "frostoil" => o.1 -= 1,
                    _ => {}
                }
            }
        }
        if f.end == Some(End::Won) {
            wins += 1;
        }
        turns += f.turn as u64;
    }
    (wins, turns as f64 / n as f64)
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "days".into());
    let all: Vec<String> = data::FACTS.iter().map(|s| s.to_string()).collect();
    if mode == "boss" {
        println!("Boss win rate over 100 fights (turns on average)");
        println!(
            "{:<16} {:>14} {:>14} {:>14} {:>14}",
            "pairing", "420 all intel", "420 no intel", "340 all intel", "250 no intel"
        );
        for main in data::GODS {
            for lesser in [None, Some(God::Thor), Some(God::Athena), Some(God::Hades)] {
                if lesser == Some(main) {
                    continue;
                }
                let name = format!(
                    "{}{}",
                    main.name(),
                    lesser.map(|l| format!("+{}", l.name())).unwrap_or_default()
                );
                let cells: Vec<String> = [(420, true), (420, false), (340, true), (250, false)]
                    .iter()
                    .map(|(p, intel)| {
                        let (w, t) =
                            boss_rate(main, lesser, *p, if *intel { &all } else { &[] }, 100);
                        format!("{w:>3}% ({t:>4.1})")
                    })
                    .collect();
                println!(
                    "{:<16} {:>14} {:>14} {:>14} {:>14}",
                    name, cells[0], cells[1], cells[2], cells[3]
                );
            }
        }
        return;
    }
    println!(
        "30 days, per patron. Light: 3 short visits a day. Heavy: about 6 hours of play a day."
    );
    for style in [sim::Style::Light, sim::Style::Heavy] {
        for main in data::GODS {
            let r = sim::play(main, style, 30, 11);
            println!("\n{:?} {}: {}", style, main.name(), r.summary());
            for line in r.days.iter().take(12) {
                println!("  {line}");
            }
        }
    }
}
