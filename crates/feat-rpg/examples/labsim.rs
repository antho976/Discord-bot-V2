//! How deep players get in the Labyrinth by Power: `cargo run --release -p feat-rpg --example labsim`.
use feat_rpg::data;
use feat_rpg::engine::combat::{End, Fight, autoplay};
use feat_rpg::engine::{Hero, Rng};

fn main() {
    println!("{:<8} {:<7} {:>8} {:>8} {:>8}", "Power", "patron", "median", "best 10%", "worst 10%");
    for power in [60, 120, 200, 300, 420, 520] {
        for main in data::GODS {
            let mut floors: Vec<u32> = (0..200u64)
                .map(|seed| {
                    let h = Hero { main, lesser: None, bonds: [2, 2, 2], power, scouting: 0, mods: Default::default() };
                    let (max, _) = h.stats();
                    let mut hp = max;
                    let mut potions = 3i64;
                    let mut r = Rng::new(seed * 31 + 7);
                    let mut floor = 1u32;
                    loop {
                        // Take a shrine when hurt and offered (half the time one is), else fight.
                        if hp * 2 < max && r.chance(0.5) {
                            hp = (hp + max * 3 / 10).min(max);
                            floor += 1;
                            continue;
                        }
                        let monster = if data::guardian_floor(floor) { data::monster("minotaur").unwrap() } else { &data::MONSTERS[(r.next_u64() % 12) as usize] };
                        let mut f = Fight::floor(h.clone(), hp, monster, data::floor_power(floor), 1.0, r.next_u64());
                        while f.end.is_none() && f.turn < 120 {
                            let a = autoplay(&f, potions, (0, 0));
                            f.act(a, potions, (0, 0)).unwrap();
                            potions -= f.used.drain(..).filter(|u| u == "potion").count() as i64;
                        }
                        if f.end != Some(End::Won) {
                            return floor;
                        }
                        hp = (f.hp + (max as f64 * data::LAB_WIN_HEAL) as i64).min(max);
                        // A sensible player leaves when low on HP and out of potions.
                        if hp * 4 < max && potions == 0 {
                            return floor;
                        }
                        floor += 1;
                        if floor > 60 {
                            return floor;
                        }
                    }
                })
                .collect();
            floors.sort_unstable();
            println!("{:<8} {:<7} {:>8} {:>8} {:>8}", power, main.name(), floors[100], floors[180], floors[20]);
        }
    }
}
