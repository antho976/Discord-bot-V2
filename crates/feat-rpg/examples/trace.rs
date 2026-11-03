//! Print one boss fight turn by turn: `cargo run -p feat-rpg --example trace -- hades thor 380 3`.
use feat_rpg::data::{self, God};
use feat_rpg::engine::Hero;
use feat_rpg::engine::combat::{Fight, autoplay};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let main = God::from_id(&a[1]).unwrap();
    let lesser = God::from_id(a.get(2).map(String::as_str).unwrap_or("-"));
    let power: i64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(420);
    let seed: u64 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(1);
    let h = Hero { main, lesser, bonds: [2, 2, 2], power, scouting: 0 };
    let (max, _) = h.stats();
    let facts: Vec<String> = data::FACTS.iter().map(|s| s.to_string()).collect();
    let mut f = Fight::boss(h, max, seed as i64, facts, seed * 7 + 1);
    let (mut p, mut o) = (3i64, (2i64, 2i64));
    while f.end.is_none() && f.turn < 200 {
        let act = autoplay(&f, p, o);
        let (t, _) = f.telegraph();
        print!("t{:>2} hp {:>4} st {} boss {:>4} [{t}] {:?}: ", f.turn, f.hp, f.stamina, f.foe.hp, act);
        f.act(act, p, o).unwrap();
        println!("{}", f.log.join(" "));
        for u in f.used.drain(..) {
            match u.as_str() { "potion" => p -= 1, "fireoil" => o.0 -= 1, "frostoil" => o.1 -= 1, _ => {} }
        }
    }
    println!("{:?}", f.end);
}
