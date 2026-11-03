//! Levels, bonds, Vigor, HP regen and weeks.

use crate::data::{self, BOND_AT, LEVEL_CAP, xp_to_next};

/// Add `gain` XP. Returns the new (level, xp into that level) and how many levels were gained.
/// At the realm's level cap XP stops just short of the next level.
pub fn add_xp(level: u32, xp: i64, gain: i64) -> (u32, i64, u32) {
    let (mut level, mut xp) = (level.max(1), xp + gain.max(0));
    let start = level;
    while level < LEVEL_CAP && xp >= xp_to_next(level) {
        xp -= xp_to_next(level);
        level += 1;
    }
    if level >= LEVEL_CAP {
        xp = xp.min(xp_to_next(level) - 1);
    }
    (level, xp, level - start)
}

/// Bond rank from personal tribute to a god, capped by the realm.
pub fn bond_rank(tribute: i64, cap: u32) -> u32 {
    let mut r = 0;
    while (r as usize) + 1 < BOND_AT.len() && tribute >= BOND_AT[r as usize + 1] {
        r += 1;
    }
    r.min(cap)
}

/// Tribute still needed for the next bond rank, if it's under the cap.
pub fn to_next_bond(tribute: i64, cap: u32) -> Option<i64> {
    let r = bond_rank(tribute, cap);
    (r < cap).then(|| BOND_AT[r as usize + 1] - tribute)
}

/// Vigor now, from the stored value and when it was stored. In percent, 0 to 100.
pub fn vigor(stored: f64, at_ms: i64, now_ms: i64) -> f64 {
    let hours = (now_ms - at_ms).max(0) as f64 / 3_600_000.0;
    (stored + hours * data::VIGOR_PER_HOUR).min(100.0)
}

/// HP now, healing out of fights from the stored value.
pub fn hp_now(stored: i64, at_ms: i64, max: i64, now_ms: i64) -> i64 {
    let mins = (now_ms - at_ms).max(0) as f64 / 60_000.0;
    (stored as f64 + max as f64 * data::REGEN_PER_MIN * mins).min(max as f64) as i64
}

/// Minutes until HP is full.
pub fn minutes_to_full(hp: i64, max: i64) -> i64 {
    if hp >= max {
        return 0;
    }
    (((max - hp) as f64) / (max as f64 * data::REGEN_PER_MIN)).ceil() as i64
}

/// The week number, starting on Mondays (UTC). Offerings reset and the boss changes with it.
pub fn week(now_ms: i64) -> i64 {
    // 1970-01-01 was a Thursday; shift so weeks start on Monday.
    (now_ms.div_euclid(86_400_000) + 3).div_euclid(7)
}

/// When the week after `w` starts, in ms.
pub fn week_start(w: i64) -> i64 {
    (w * 7 - 3) * 86_400_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xp_levels_and_caps() {
        let (l, x, g) = add_xp(1, 0, xp_to_next(1));
        assert_eq!((l, x, g), (2, 0, 1));
        let (l, x, _) = add_xp(1, 0, 10_000_000);
        assert_eq!(l, LEVEL_CAP);
        assert_eq!(x, xp_to_next(LEVEL_CAP) - 1);
    }

    #[test]
    fn bonds_rank_up_and_cap() {
        assert_eq!(bond_rank(0, 2), 0);
        assert_eq!(bond_rank(300, 2), 1);
        assert_eq!(bond_rank(1499, 2), 1);
        assert_eq!(bond_rank(1500, 2), 2);
        assert_eq!(bond_rank(100_000, 2), 2);
        assert_eq!(bond_rank(100_000, 5), 5);
        assert_eq!(to_next_bond(1000, 2), Some(500));
        assert_eq!(to_next_bond(2000, 2), None);
    }

    #[test]
    fn weeks_start_on_monday() {
        // 2026-11-02 is a Monday.
        let monday = 1_793_577_600_000; // 2026-11-02T00:00:00Z
        assert_eq!(week(monday), week(monday + 6 * 86_400_000 + 86_399_000));
        assert_eq!(week(monday) - 1, week(monday - 1));
        assert_eq!(week_start(week(monday)), monday);
    }

    #[test]
    fn vigor_and_hp_regen() {
        assert_eq!(vigor(0.0, 0, 10 * 3_600_000), 15.0);
        assert_eq!(vigor(90.0, 0, 100 * 3_600_000), 100.0);
        assert_eq!(hp_now(0, 0, 450, 45 * 60_000), 450);
    }
}
