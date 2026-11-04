//! Gear rolls: how good a piece comes out, and which traits it carries.

use super::Rng;
use crate::data::{self, Gear, Rarity, Trait};

pub type Traits = Vec<(Trait, i64)>;

pub fn roll_rarity(source: &str, rng: &mut Rng) -> Rarity {
    pick_rarity(data::rarity_odds(source), rng)
}

fn pick_rarity(odds: [u32; 4], rng: &mut Rng) -> Rarity {
    let mut x = (rng.next_u64() % 100) as u32;
    for (i, o) in odds.iter().enumerate() {
        if x < *o {
            return Rarity::from_index(i as i64);
        }
        x -= o;
    }
    Rarity::Common
}

/// `n` different traits that fit the piece's slot.
pub fn roll_traits(g: &Gear, n: usize, rng: &mut Rng) -> Traits {
    let mut pool: Vec<Trait> = data::TRAITS
        .iter()
        .copied()
        .filter(|t| t.fits(g.slot))
        .collect();
    let mut out = Vec::new();
    while out.len() < n && !pool.is_empty() {
        let i = (rng.next_u64() % pool.len() as u64) as usize;
        let t = pool.swap_remove(i);
        let (lo, hi) = t.range();
        out.push((t, lo + (rng.next_u64() % (hi - lo + 1) as u64) as i64));
    }
    out
}

/// A whole piece from a source.
pub fn roll(g: &Gear, source: &str, rng: &mut Rng) -> (Rarity, Traits) {
    let r = roll_rarity(source, rng);
    (r, roll_traits(g, r.index() as usize, rng))
}

/// A piece forged at a smith rank.
pub fn forge(g: &Gear, rank: u32, rng: &mut Rng) -> (Rarity, Traits) {
    let r = pick_rarity(data::forge_odds(rank), rng);
    (r, roll_traits(g, r.index() as usize, rng))
}

/// A piece forged and tempered: at least Fine, the chosen trait first and rolled in the top
/// half of its range, then the rest at random.
pub fn temper(g: &Gear, rank: u32, chosen: Trait, rng: &mut Rng) -> (Rarity, Traits) {
    let r = pick_rarity(data::forge_odds(rank), rng).max(Rarity::Fine);
    let (lo, hi) = chosen.range();
    let mid = (lo + hi + 1) / 2;
    let mut out = vec![(
        chosen,
        mid + (rng.next_u64() % (hi - mid + 1) as u64) as i64,
    )];
    let mut rest = roll_traits(g, r.index() as usize + 1, rng);
    rest.retain(|(t, _)| *t != chosen);
    out.extend(rest.into_iter().take(r.index() as usize - 1));
    (r, out)
}

/// A piece of gear to drop: one the player could wear soon, from any slot.
pub fn drop_piece(level: u32, boss_won: bool, rng: &mut Rng) -> &'static Gear {
    let fits: Vec<&Gear> = data::GEAR
        .iter()
        .filter(|g| g.level <= level + 2 && (!g.after_boss || boss_won))
        .collect();
    // Lean towards the newest pieces you can wear.
    let top = fits.iter().map(|g| g.level).max().unwrap_or(1);
    let near: Vec<&Gear> = fits
        .iter()
        .copied()
        .filter(|g| g.level + 8 >= top)
        .collect();
    near[(rng.next_u64() % near.len() as u64) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traits_fit_and_match_rarity() {
        let mut rng = Rng::new(3);
        for g in data::GEAR {
            for _ in 0..50 {
                let (r, t) = roll(g, "elite", &mut rng);
                assert_eq!(t.len(), r.index() as usize);
                assert!(
                    t.iter()
                        .all(|(x, v)| x.fits(g.slot) && *v >= x.range().0 && *v <= x.range().1)
                );
                let kinds: std::collections::HashSet<_> = t.iter().map(|x| x.0 as u8).collect();
                assert_eq!(kinds.len(), t.len());
            }
        }
    }

    #[test]
    fn tempering_keeps_the_chosen_trait() {
        let mut rng = Rng::new(9);
        for g in data::GEAR {
            for t in data::TRAITS.iter().filter(|t| t.fits(g.slot)) {
                for _ in 0..20 {
                    let (r, traits) = temper(g, 1, *t, &mut rng);
                    assert!(r >= Rarity::Fine);
                    assert_eq!(traits.len(), r.index() as usize);
                    assert_eq!(traits[0].0, *t);
                    assert!(traits[0].1 >= (t.range().0 + t.range().1 + 1) / 2);
                    let kinds: std::collections::HashSet<_> =
                        traits.iter().map(|x| x.0 as u8).collect();
                    assert_eq!(kinds.len(), traits.len());
                }
            }
        }
    }

    #[test]
    fn odds_add_up() {
        for s in ["forge", "hunt", "elite", "guardian", "boss"] {
            assert_eq!(data::rarity_odds(s).iter().sum::<u32>(), 100);
        }
    }
}
