//! What the checks find, and what to do next.
//!
//! Every check reads one mechanic and says how far along it is against the game's own maxes:
//! the items in it (a vial, a prayer, a grid node), each with its level and its max. The review
//! then picks what to do next with no other players to compare against: what is furthest
//! behind the rest of your account in the same world, and cheap finishes (one or two steps from
//! done).

use crate::save::Save;

/// One thing inside a mechanic: a vial, a statue, a research node.
#[derive(Debug, Clone)]
pub struct Item {
    pub name: String,
    /// The game's picture ID, when it has one.
    pub picture: Option<String>,
    pub level: f64,
    pub max: f64,
    /// What a level is called when it isn't a level: "Onyx", "Tier 3".
    pub level_name: Option<String>,
}

impl Item {
    pub fn new(name: impl Into<String>, picture: Option<String>, level: f64, max: f64) -> Item {
        Item { name: name.into(), picture, level: level.max(0.0), max: max.max(0.0), level_name: None }
    }

    pub fn done(&self) -> bool {
        self.max > 0.0 && self.level >= self.max
    }

    pub fn share(&self) -> f64 {
        if self.max <= 0.0 { 0.0 } else { (self.level / self.max).clamp(0.0, 1.0) }
    }
}

/// How a mechanic's progress adds up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Count {
    /// Levels: the sum of levels against the sum of maxes ("1,240 of 1,600 levels").
    Levels,
    /// Items done: how many are at their max ("31 of 41 bought").
    Done,
    /// No max anywhere (bubbles, refinery ranks): the levels are shown, lowest first, but the
    /// mechanic stays out of every share and out of Do next. Items keep `max` 0.
    Open,
}

#[derive(Debug, Clone)]
pub struct Mechanic {
    pub key: &'static str,
    /// 1 to 7; 0 for the account-wide ones (cards, achievements).
    pub world: u8,
    pub name: &'static str,
    pub picture: Option<String>,
    pub count: Count,
    /// What one counted thing is called: "levels", "bought", "found".
    pub unit: &'static str,
    pub items: Vec<Item>,
    /// A line on what raises the max or why it reads as it does, when that matters.
    pub note: Option<String>,
}

impl Mechanic {
    pub fn have(&self) -> f64 {
        match self.count {
            Count::Levels => self.items.iter().map(|i| i.level.min(i.max)).sum(),
            Count::Done => self.items.iter().filter(|i| i.done()).count() as f64,
            Count::Open => self.items.iter().map(|i| i.level).sum(),
        }
    }

    pub fn total(&self) -> f64 {
        match self.count {
            Count::Levels => self.items.iter().map(|i| i.max).sum(),
            Count::Done => self.items.len() as f64,
            Count::Open => 0.0,
        }
    }

    pub fn share(&self) -> f64 {
        let t = self.total();
        if t <= 0.0 { 0.0 } else { (self.have() / t).clamp(0.0, 1.0) }
    }

    pub fn done(&self) -> bool {
        self.total() > 0.0 && self.have() >= self.total()
    }

    /// Items not at their max, least done first (lowest level first when there's no max).
    pub fn open_items(&self) -> Vec<&Item> {
        let mut v: Vec<&Item> = self.items.iter().filter(|i| !i.done()).collect();
        v.sort_by(|a, b| a.share().total_cmp(&b.share()).then(a.level.total_cmp(&b.level)));
        v
    }

    /// Nothing at all yet: no level anywhere, no item found.
    pub fn untouched(&self) -> bool {
        !self.items.is_empty() && self.items.iter().all(|i| i.level <= 0.0)
    }
}

/// Why a mechanic made the do-next list.
#[derive(Debug, Clone, PartialEq)]
pub enum Why {
    /// It's this far under your average for its world (0.0 to 1.0).
    Behind { world_avg: f64 },
    /// Only this many items are left to max.
    NearlyDone { left: usize },
}

#[derive(Debug, Clone)]
pub struct Next {
    pub mechanic: usize,
    pub why: Why,
}

pub struct Review {
    pub name: String,
    pub characters: usize,
    /// The highest world the account has reached, 1 to 7.
    pub world: u8,
    pub mechanics: Vec<Mechanic>,
    pub next: Vec<Next>,
    /// Mechanics in worlds you've reached with no progress at all: new since you last played,
    /// or skipped. In game order.
    pub not_started: Vec<usize>,
}

impl Review {
    pub fn new(save: &Save, mechanics: Vec<Mechanic>) -> Review {
        let world = reached(&mechanics);
        let not_started: Vec<usize> = mechanics.iter().enumerate().filter(|(_, m)| m.world > 0 && m.world <= world && m.untouched()).map(|(i, _)| i).collect();
        let next = pick_next(&mechanics, world, &not_started);
        Review {
            name: save.names.first().cloned().unwrap_or_default(),
            characters: save.characters(),
            world,
            mechanics,
            next,
            not_started,
        }
    }

    /// The whole account's share of the maxes the checks read, each mechanic counting once.
    pub fn share(&self) -> f64 {
        let open: Vec<&Mechanic> = self.mechanics.iter().filter(|m| m.world <= self.world && m.total() > 0.0).collect();
        if open.is_empty() { 0.0 } else { open.iter().map(|m| m.share()).sum::<f64>() / open.len() as f64 }
    }

    /// One world's average share, `None` when it has no mechanics with a max.
    pub fn world_share(&self, world: u8) -> Option<f64> {
        world_avg(&self.mechanics, world)
    }
}

/// The highest world with any progress in it.
fn reached(mechanics: &[Mechanic]) -> u8 {
    mechanics.iter().filter(|m| m.world > 0 && m.have() > 0.0).map(|m| m.world).max().unwrap_or(1)
}

fn world_avg(mechanics: &[Mechanic], world: u8) -> Option<f64> {
    let ms: Vec<f64> = mechanics.iter().filter(|m| m.world == world && m.total() > 0.0).map(Mechanic::share).collect();
    (!ms.is_empty()).then(|| ms.iter().sum::<f64>() / ms.len() as f64)
}

/// How many the do-next list shows.
pub const NEXT: usize = 6;

/// Do next: first, mechanics furthest under their own world's average (the ones the rest of
/// the account has left behind), earlier worlds first when equal since they cost less; then
/// cheap finishes, mechanics with one to three items left. Worlds you haven't reached,
/// finished mechanics and ones not started (they have their own list) never show.
fn pick_next(mechanics: &[Mechanic], reached: u8, not_started: &[usize]) -> Vec<Next> {
    let mut behind: Vec<(f64, usize, f64)> = Vec::new();
    let mut nearly: Vec<(usize, usize)> = Vec::new();
    for (i, m) in mechanics.iter().enumerate() {
        if m.world > reached || m.done() || m.total() <= 0.0 || not_started.contains(&i) {
            continue;
        }
        let avg = world_avg(mechanics, m.world).unwrap_or(0.0);
        let gap = avg - m.share();
        // Earlier worlds weigh a little more: the same gap there is cheaper to close.
        let weight = 1.0 + 0.05 * f64::from(reached.saturating_sub(m.world.max(1)));
        if gap > 0.05 {
            behind.push((gap * weight, i, avg));
        }
        let left = m.items.iter().filter(|it| !it.done()).count();
        if (1..=3).contains(&left) && m.items.len() >= 5 {
            nearly.push((left, i));
        }
    }
    behind.sort_by(|a, b| b.0.total_cmp(&a.0));
    nearly.sort();
    let mut out: Vec<Next> = Vec::new();
    // Up to two thirds of the list from what's behind, the rest from cheap finishes.
    for &(_, i, avg) in behind.iter().take(NEXT * 2 / 3) {
        out.push(Next { mechanic: i, why: Why::Behind { world_avg: avg } });
    }
    for &(left, i) in &nearly {
        if out.len() >= NEXT {
            break;
        }
        if !out.iter().any(|n| n.mechanic == i) {
            out.push(Next { mechanic: i, why: Why::NearlyDone { left } });
        }
    }
    for &(_, i, avg) in behind.iter().skip(NEXT * 2 / 3) {
        if out.len() >= NEXT {
            break;
        }
        out.push(Next { mechanic: i, why: Why::Behind { world_avg: avg } });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mech(key: &'static str, world: u8, levels: &[(f64, f64)]) -> Mechanic {
        Mechanic {
            key,
            world,
            name: key,
            picture: None,
            count: Count::Levels,
            unit: "levels",
            items: levels.iter().map(|&(l, m)| Item::new("x", None, l, m)).collect(),
            note: None,
        }
    }

    #[test]
    fn behind_its_world_comes_first_and_done_never_shows() {
        let ms = vec![
            mech("a", 2, &[(10.0, 10.0), (10.0, 10.0)]),
            mech("b", 2, &[(9.0, 10.0), (9.0, 10.0)]),
            mech("c", 2, &[(1.0, 10.0), (1.0, 10.0)]),
            mech("d", 5, &[(0.0, 10.0)]),
        ];
        let next = pick_next(&ms, 2, &[]);
        assert_eq!(next.first().map(|n| ms[n.mechanic].key), Some("c"));
        assert!(next.iter().all(|n| ms[n.mechanic].key != "a" && ms[n.mechanic].key != "d"));
    }

    #[test]
    fn open_mechanics_stay_out_of_shares() {
        let mut m = mech("bubbles", 2, &[(5.0, 0.0), (9.0, 0.0)]);
        m.count = Count::Open;
        assert_eq!(m.have(), 14.0);
        assert_eq!(m.total(), 0.0);
        assert_eq!(m.share(), 0.0);
        assert!(!m.done());
        let next = pick_next(&[m], 2, &[]);
        assert!(next.is_empty());
    }

    #[test]
    fn counts() {
        let mut m = mech("a", 1, &[(5.0, 10.0), (12.0, 10.0)]);
        assert_eq!(m.have(), 15.0);
        assert_eq!(m.total(), 20.0);
        m.count = Count::Done;
        assert_eq!(m.have(), 1.0);
        assert_eq!(m.total(), 2.0);
    }
}
