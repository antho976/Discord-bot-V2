//! The world each tier belongs to: by which world a player is expected to have it done. It's
//! the owner's judgement, kept in `expected.txt` next to this crate's `Cargo.toml`, one block
//! per mechanic. With it, a card can say where its mechanic is ("where a World 2 player would
//! be") and which tier is overdue for the world you're in.
//!
//! A release build carries the file in itself. A debug build (`bot preview`) reads it again
//! for every check, so an edit shows on the next check with no rebuild.

use std::collections::HashMap;

use crate::review::Tier;

/// Tier worlds per mechanic key, in ladder order: `None` for a tier with no world set yet.
pub type Worlds = HashMap<String, Vec<Option<u8>>>;

const FILE: &str = include_str!("../expected.txt");

/// Runs `f` with the worlds as the file says them now.
pub fn with<R>(f: impl FnOnce(&Worlds) -> R) -> R {
    #[cfg(debug_assertions)]
    {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/expected.txt"));
        f(&parse(text.as_deref().unwrap_or(FILE)).0)
    }
    #[cfg(not(debug_assertions))]
    {
        static WORLDS: std::sync::LazyLock<Worlds> = std::sync::LazyLock::new(|| parse(FILE).0);
        f(&WORLDS)
    }
}

/// Reads the file: a line starting at the margin names a mechanic by its key, and each
/// indented line under it is one tier, in ladder order, starting with the tier's world (1 to 7)
/// or `-` for not set. The rest of a tier line, and anything after `#`, is for people. Lines
/// it can't read are skipped and listed in the second half of the answer (the tests fail on any).
pub fn parse(text: &str) -> (Worlds, Vec<String>) {
    let mut worlds = Worlds::new();
    let mut problems = Vec::new();
    let mut key: Option<String> = None;
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("");
        let Some(first) = line.split_whitespace().next() else { continue };
        if !line.starts_with([' ', '\t']) {
            if worlds.contains_key(first) {
                problems.push(format!("line {}: {first} is listed twice", n + 1));
            }
            worlds.insert(first.to_string(), Vec::new());
            key = Some(first.to_string());
            continue;
        }
        let Some(tiers) = key.as_ref().and_then(|k| worlds.get_mut(k)) else {
            problems.push(format!("line {}: a tier before any mechanic", n + 1));
            continue;
        };
        match first {
            "-" => tiers.push(None),
            w => match w.parse::<u8>() {
                Ok(w @ 1..=7) => tiers.push(Some(w)),
                _ => {
                    problems.push(format!("line {}: {w:?} isn't a world (1 to 7, or - for not set)", n + 1));
                    tiers.push(None);
                }
            },
        }
    }
    (worlds, problems)
}

/// Where a mechanic stands against the worlds its tiers belong to.
#[derive(Debug, Clone, PartialEq)]
pub struct Pace {
    /// Each tier's world, in ladder order (`None`: not set).
    pub worlds: Vec<Option<u8>>,
    /// The highest world, up to the one you're in, all of whose tiers are done: "where a
    /// World N player would be". `None` when even the first world's tiers aren't. Capped at
    /// your world: an account short of World 7 has a shorter ladder (later worlds' items
    /// aren't on it), so having all of it done says nothing about later worlds' tiers.
    pub at: Option<u8>,
    /// The first tier not done whose world you've reached: what's overdue. Its world is
    /// `worlds[due]`.
    pub due: Option<usize>,
}

impl Pace {
    /// `None` for a mechanic with no tier world set at all.
    pub fn new(tiers: &[Tier], worlds: &[Option<u8>], reached: u8) -> Option<Pace> {
        // By position: an account that hasn't reached a world has fewer items, but the same
        // rungs in the same order. A ladder with fewer rungs than the file leaves the rest out.
        let worlds: Vec<Option<u8>> = (0..tiers.len()).map(|k| worlds.get(k).copied().flatten()).collect();
        if worlds.iter().all(Option::is_none) {
            return None;
        }
        let set = || tiers.iter().zip(&worlds).filter_map(|(t, w)| w.map(|w| (t, w)));
        // The highest world with every tier up to it done, among the worlds tiers belong to.
        let at = set().map(|(_, w)| w).filter(|&w| w <= reached && set().all(|(t, tw)| tw > w || t.complete())).max();
        let due = (0..tiers.len()).find(|&k| worlds[k].is_some_and(|w| w <= reached) && !tiers[k].complete());
        Some(Pace { worlds, at, due })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks;
    use crate::save::Save;

    fn tier(done: bool) -> Tier {
        Tier { label: String::new(), targets: None, met: usize::from(done), need: 1 }
    }

    #[test]
    fn reads_worlds_and_says_what_it_cant() {
        let (w, problems) = parse("# a comment\nstamps   # Stamps\n  1  Any 5\n  -  Any 10\n\tx  bad\nvials\n  3\n  9\n");
        assert_eq!(w["stamps"], vec![Some(1), None, None]);
        assert_eq!(w["vials"], vec![Some(3), None]);
        assert_eq!(problems.len(), 2, "{problems:?}");
    }

    #[test]
    fn pace_is_the_highest_world_done_through() {
        let worlds = [Some(1), Some(2), Some(2), Some(4), None];
        let tiers = [tier(true), tier(true), tier(false), tier(true), tier(false)];
        let p = Pace::new(&tiers, &worlds, 7).unwrap();
        assert_eq!(p.at, Some(1), "a World 2 tier isn't done");
        assert_eq!(p.due, Some(2));
        // Nothing overdue for a World 1 account, and the unset tier never counts.
        assert_eq!(Pace::new(&tiers, &worlds, 1).unwrap().due, None);
        let all = [tier(true), tier(true), tier(true), tier(true), tier(false)];
        assert_eq!(Pace::new(&all, &worlds, 7).unwrap(), Pace { worlds: worlds.to_vec(), at: Some(4), due: None });
        // A World 2 account with its whole (shorter) ladder done is at World 2, not 4.
        assert_eq!(Pace::new(&all, &worlds, 2).unwrap().at, Some(2));
        let none = [tier(false), tier(false), tier(false), tier(false), tier(false)];
        assert_eq!(Pace::new(&none, &worlds, 7).unwrap().at, None);
        assert_eq!(Pace::new(&none, &[None, None], 7), None);
        // Fewer rungs than the file: the extra worlds are left out.
        assert_eq!(Pace::new(&tiers[..2], &worlds, 7).unwrap().worlds, vec![Some(1), Some(2)]);
    }

    /// The file reads clean, names only real mechanics, never lists more tiers than a full
    /// ladder has, and its worlds never go down a ladder (a harder tier can't be due sooner).
    #[test]
    fn the_file_matches_the_ladders() {
        let (worlds, problems) = parse(FILE);
        assert!(problems.is_empty(), "expected.txt: {problems:#?}");
        let save = Save::parse(r#"{"OptLacc": []}"#).unwrap();
        let review = crate::review::Review::new(&save, checks::all(&save), 7);
        for (key, ws) in &worlds {
            let m = review.mechanics.iter().find(|m| m.key == key).unwrap_or_else(|| panic!("expected.txt: no mechanic {key}"));
            let rungs = m.tiers().len();
            assert!(ws.len() <= rungs, "expected.txt: {key} lists {} tiers, its ladder has {rungs}", ws.len());
            let set: Vec<u8> = ws.iter().flatten().copied().collect();
            assert!(set.windows(2).all(|p| p[0] <= p[1]), "expected.txt: {key}'s worlds go down: {ws:?}");
        }
        for m in &review.mechanics {
            assert!(m.tiers().is_empty() || worlds.contains_key(m.key), "expected.txt has no {}: run the expected example", m.key);
        }
    }
}
