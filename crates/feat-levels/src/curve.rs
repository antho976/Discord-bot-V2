//! How much XP each level takes. Matches antho-s-bot's curve so imported totals land on the
//! same levels: level n -> n+1 costs `base * factor^n`, unless a custom table says otherwise.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    pub level: u32,
    /// XP earned inside the current level.
    pub into: i64,
    /// XP the current level takes in total.
    pub needed: i64,
}

/// The most XP a member can hold. Steep curves run past what i64 can add up within a few hundred
/// levels; nobody earns this much, and capping here keeps every sum below safe.
pub const MAX_XP: i64 = 1_000_000_000_000_000;

pub fn xp_for_level(level: u32, base: i64, factor: f64, custom: &BTreeMap<u32, i64>) -> i64 {
    if let Some(v) = custom.get(&(level + 1)) {
        return (*v).max(1);
    }
    ((base as f64) * factor.powi(level as i32)).round().max(1.0) as i64
}

pub fn progress(total: i64, base: i64, factor: f64, custom: &BTreeMap<u32, i64>) -> Progress {
    let (mut level, mut acc) = (0u32, 0i64);
    let mut needed = xp_for_level(0, base, factor, custom);
    while total >= acc.saturating_add(needed) && level < 1000 {
        acc = acc.saturating_add(needed);
        level += 1;
        needed = xp_for_level(level, base, factor, custom);
    }
    Progress { level, into: total - acc, needed }
}

/// Total XP needed to sit at the start of `level`. Saturates rather than overflowing; anything past
/// [`MAX_XP`] is out of reach.
pub fn xp_to_reach(level: u32, base: i64, factor: f64, custom: &BTreeMap<u32, i64>) -> i64 {
    (0..level).fold(0i64, |sum, l| sum.saturating_add(xp_for_level(l, base, factor, custom)))
}

/// `███████░░░░░░░░░` for a fraction.
pub fn bar(into: i64, needed: i64, width: usize) -> String {
    let filled = if needed <= 0 { 0 } else { ((into as f64 / needed as f64) * width as f64).round() as usize };
    let filled = filled.min(width);
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_old_bot() {
        let none = BTreeMap::new();
        // base 100, factor 1.2: 100, 120, 144, ...
        assert_eq!(xp_for_level(0, 100, 1.2, &none), 100);
        assert_eq!(xp_for_level(2, 100, 1.2, &none), 144);
        assert_eq!(progress(0, 100, 1.2, &none).level, 0);
        assert_eq!(progress(100, 100, 1.2, &none), Progress { level: 1, into: 0, needed: 120 });
        assert_eq!(progress(219, 100, 1.2, &none).level, 1);
        assert_eq!(progress(220, 100, 1.2, &none).level, 2);
        assert_eq!(xp_to_reach(2, 100, 1.2, &none), 220);
    }

    #[test]
    fn reaching_a_level_lands_exactly_on_it() {
        let none = BTreeMap::new();
        for factor in [1.0, 1.2, 2.5, 5.0] {
            for level in 1..=40u32 {
                let at = xp_to_reach(level, 100, factor, &none);
                if at > MAX_XP {
                    break;
                }
                assert_eq!(progress(at, 100, factor, &none), Progress { level, into: 0, needed: xp_for_level(level, 100, factor, &none) });
                assert_eq!(progress(at - 1, 100, factor, &none).level, level - 1, "factor {factor}, level {level}");
            }
        }
    }

    #[test]
    fn steep_curves_saturate_instead_of_overflowing() {
        let none = BTreeMap::new();
        // 100 * 1.2^n passes i64 once summed past level ~215; 5^n far sooner.
        assert_eq!(xp_to_reach(500, 100, 1.2, &none), i64::MAX);
        assert_eq!(xp_to_reach(500, 100_000, 5.0, &none), i64::MAX);
        assert!(xp_to_reach(500, 100, 1.2, &none) > MAX_XP);
        assert!(progress(MAX_XP, 100, 5.0, &none).level < 30);
        assert!(progress(i64::MAX, 100, 1.2, &none).level <= 1000);
    }

    #[test]
    fn bars() {
        assert_eq!(bar(0, 100, 4), "░░░░");
        assert_eq!(bar(50, 100, 4), "██░░");
        assert_eq!(bar(150, 100, 4), "████");
        assert_eq!(bar(5, 0, 4), "░░░░");
        assert_eq!(bar(-5, 100, 4), "░░░░");
    }

    #[test]
    fn custom_table_wins() {
        let custom = BTreeMap::from([(1, 50), (2, 50)]);
        assert_eq!(progress(100, 100, 1.2, &custom).level, 2);
    }
}
