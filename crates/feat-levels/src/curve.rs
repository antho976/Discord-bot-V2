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

pub fn xp_for_level(level: u32, base: i64, factor: f64, custom: &BTreeMap<u32, i64>) -> i64 {
    if let Some(v) = custom.get(&(level + 1)) {
        return (*v).max(1);
    }
    ((base as f64) * factor.powi(level as i32)).round().max(1.0) as i64
}

pub fn progress(total: i64, base: i64, factor: f64, custom: &BTreeMap<u32, i64>) -> Progress {
    let (mut level, mut acc) = (0u32, 0i64);
    let mut needed = xp_for_level(0, base, factor, custom);
    while total >= acc + needed && level < 1000 {
        acc += needed;
        level += 1;
        needed = xp_for_level(level, base, factor, custom);
    }
    Progress { level, into: total - acc, needed }
}

/// Total XP needed to sit at the start of `level`.
pub fn xp_to_reach(level: u32, base: i64, factor: f64, custom: &BTreeMap<u32, i64>) -> i64 {
    (0..level).map(|l| xp_for_level(l, base, factor, custom)).sum()
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
    fn custom_table_wins() {
        let custom = BTreeMap::from([(1, 50), (2, 50)]);
        assert_eq!(progress(100, 100, 1.2, &custom).level, 2);
    }
}
