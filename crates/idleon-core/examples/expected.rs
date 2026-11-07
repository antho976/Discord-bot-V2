//! Writes `expected.txt` (the world each tier belongs to, see `src/expected.rs`): every mechanic
//! with a ladder, one line per tier, keeping every world already set, with how many of the saves
//! given have each tier done.
//!
//! `cargo run -p idleon-core --example expected -- ~/saves/*.json`
//!
//! Run it after the ladders change (a game update, a new mechanic, a new tier). It rewrites the
//! file in place and says on stderr what moved.

use std::collections::HashMap;
use std::fmt::Write as _;

use idleon_core::expected;
use idleon_core::review::Review;
use idleon_core::save::Save;

const WORLD_NAMES: [&str; 8] = ["Account", "Blunder Hills", "Yum-Yum Desert", "Frostbite Tundra", "Hyperion Nebula", "Smolderin' Plateau", "Spirited Valley", "Shimmerfin Deep"];

fn main() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/expected.txt");
    let old = std::fs::read_to_string(path).unwrap_or_default();
    let (kept, problems) = expected::parse(&old);
    for p in &problems {
        eprintln!("expected.txt {p} (left out)");
    }

    // The ladders as a World 7 account sees them: every item in, nothing done.
    let empty = Save::parse(r#"{"OptLacc": []}"#).expect("an empty save");
    let full = Review::new(&empty, idleon_core::run_checks(&empty), 7);

    // Per mechanic, per tier: how many saves have it done. And how many saves read a ladder
    // of another length (an account whose own items change the ladder).
    let mut done: HashMap<&str, Vec<usize>> = HashMap::new();
    // Tier names from the save with the most items in each mechanic: a real account holds
    // things an empty save can't (stamps from past events), so its names are the ones you see.
    let mut names: HashMap<&str, (usize, Vec<String>)> = HashMap::new();
    let mut odd: HashMap<&str, usize> = HashMap::new();
    let mut saves = 0;
    for p in std::env::args().skip(1) {
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let Ok(save) = Save::parse(&text) else { continue };
        saves += 1;
        let r = Review::new(&save, idleon_core::run_checks(&save), idleon_core::world_reached(&save));
        for m in &r.mechanics {
            let Some(fm) = full.mechanics.iter().find(|f| f.key == m.key) else { continue };
            let tiers = m.tiers();
            if tiers.len() != fm.tiers().len() {
                *odd.entry(fm.key).or_default() += 1;
            }
            if tiers.len() == fm.tiers().len() && names.get(fm.key).is_none_or(|(n, _)| m.items.len() > *n) {
                names.insert(fm.key, (m.items.len(), tiers.iter().map(|t| t.label.clone()).collect()));
            }
            let row = done.entry(fm.key).or_insert_with(|| vec![0; fm.tiers().len()]);
            for (k, t) in tiers.iter().enumerate().take(row.len()) {
                row[k] += usize::from(t.complete());
            }
        }
    }

    // By world, as the page's tabs show them (account-wide last), the page's order inside each.
    let mut order: Vec<&idleon_core::review::Mechanic> = full.mechanics.iter().collect();
    order.sort_by_key(|m| if m.world == 0 { 8 } else { m.world });
    let mut out = String::from(HEAD);
    let mut world = u8::MAX;
    let mut written = 0;
    for m in order {
        let tiers = m.tiers();
        // A mechanic with a crowd.txt table takes its tiers' worlds from there.
        if tiers.is_empty() || m.crowd.is_some() {
            continue;
        }
        if m.world != world {
            world = m.world;
            let title = if world == 0 { "Account-wide".to_string() } else { format!("World {world}, {}", WORLD_NAMES[usize::from(world)]) };
            let _ = write!(out, "\n# ---- {title} {}\n", "-".repeat(80usize.saturating_sub(title.len() + 7)));
        }
        let _ = writeln!(out, "\n{:<40}# {}", m.key, m.name);
        let set = kept.get(m.key);
        if let Some(ws) = set.filter(|ws| ws.len() != tiers.len() && ws.iter().any(Option::is_some)) {
            eprintln!("{}: the ladder has {} tiers, the file had {}; worlds kept by position", m.key, tiers.len(), ws.len());
        }
        for (k, t) in tiers.iter().enumerate() {
            let w = set.and_then(|ws| ws.get(k).copied().flatten()).map_or("-".to_string(), |w| w.to_string());
            let label = names.get(m.key).and_then(|(_, l)| l.get(k)).unwrap_or(&t.label);
            let line = format!("  {w}  {label}");
            let count = done.get(m.key).map(|row| format!(" {} of {saves} saves", row[k])).unwrap_or_default();
            let _ = if count.is_empty() { writeln!(out, "{line}") } else { writeln!(out, "{line:<40}#{count}") };
        }
        written += 1;
    }
    for key in kept.keys() {
        if !full.mechanics.iter().any(|m| m.key == key && !m.tiers().is_empty()) {
            eprintln!("{key}: no such mechanic with a ladder any more; dropped");
        }
    }
    let mut odd: Vec<_> = odd.into_iter().collect();
    odd.sort();
    for (key, n) in odd {
        eprintln!("{key}: {n} of {saves} saves read a ladder of another length than the file's");
    }
    std::fs::write(path, out).expect("write expected.txt");
    eprintln!("wrote {written} mechanics to {path}");
}

const HEAD: &str = "\
# The world each tier belongs to: by which world a player is expected to have it done.
#
# One block per mechanic, grouped by world as the page's tabs show them. A block starts with
# the mechanic's key (keep it as it is), then one line per tier of its ladder, as a World 7
# account sees it. A tier line starts with its world, 1 to 7, or - for not set yet. Only that
# is read: the tier's name and how many of the test saves have it done are there for you.
#
# A card says nothing about worlds until its mechanic has one set, and worlds can't go down a
# ladder (a harder tier can't be expected sooner).
#
# `bot preview` (a debug build) reads this file on every check: edit, save, check again.
# After the ladders change (a game update, a new mechanic), run
#   cargo run -p idleon-core --example expected -- <folder of saves>/*.json
# to rewrite it with the new tiers, keeping every world already set.
";
