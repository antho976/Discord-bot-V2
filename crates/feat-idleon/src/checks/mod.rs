//! One check per mechanic: read the save, compare with the game's maxes. One file per world.
//!
//! Where the save lives and which table column holds the max was worked out from the game's
//! code and checked against real saves (four from 2.43/2.45, three current ones with World 7).
//! A max is always the highest the game allows: where something raises a cap (a gaming
//! superbit, a Menagerie unlock), the check uses the raised cap and says what raises it, so
//! the share is the real distance to done. Mechanics with no max at all are `Count::Open`:
//! shown with their readings, left out of the shares.

mod caverns;
mod general;
mod tome;
mod companions;
mod guild;
mod w1;
mod w2;
mod w3;
mod w4;
mod w5;
mod w6;
mod w7;

pub(crate) use bot_core::serde_json::Value;

pub(crate) use crate::game::{GAME, num, words};
pub(crate) use crate::review::{Count, Have, Item, Mechanic, Raise, State};
pub(crate) use crate::save::Save;

/// The highest world reached: kills on that world's first map by any character (map 51 for
/// World 2, then every 50), or the account flags World 6 and 7 set when you arrive
/// (`OptLacc[194]`, `[408]`). Mechanics can't tell: some count things from every world.
pub fn world_reached(save: &Save) -> u8 {
    let details = rows("MapDetails", &[]);
    let chars = save.characters();
    for w in (2..=7u8).rev() {
        let flag = match w {
            7 => save.option(408) > 0.0,
            6 => save.option(194) > 0.0,
            _ => false,
        };
        let map = 50 * usize::from(w - 1) + 1;
        let req = details.get(map).and_then(|d| d.get(0)).and_then(|d| d.get(0)).map(num).unwrap_or(0.0);
        let killed = (0..chars).any(|c| save.at(&format!("KLA_{c}"), &[map, 0]).map(num).is_some_and(|left| left < req));
        if flag || killed {
            return w;
        }
    }
    1
}

pub fn all(save: &Save) -> Vec<Mechanic> {
    let mut v = Vec::new();
    v.extend(w1::all(save));
    v.extend(w2::all(save));
    v.extend(w3::all(save));
    v.extend(w4::all(save));
    v.extend(w5::all(save));
    v.extend(caverns::all(save));
    v.extend(w6::all(save));
    v.extend(w7::all(save));
    v.extend(general::all(save));
    v.extend(tome::all(save));
    v.extend(companions::all(save));
    v.extend(guild::all(save));
    v
}

// --- reading tables -------------------------------------------------------------------------

/// `table[path..]`: the rows of a table, or of a list inside it.
pub(crate) fn rows(table: &str, path: &[usize]) -> Vec<&'static Value> {
    let mut v: Vec<&'static Value> = GAME.rows(table).iter().collect();
    for &i in path {
        v = v.get(i).and_then(|x| x.as_array()).map(|a| a.iter().collect()).unwrap_or_default();
    }
    v
}

pub(crate) fn text(row: &Value, col: usize) -> &str {
    row.get(col).and_then(Value::as_str).unwrap_or("")
}

pub(crate) fn cell(row: &Value, col: usize) -> f64 {
    row.get(col).map(num).unwrap_or(0.0)
}

/// The game's text made readable: "+{%_Sample_Size" is "Sample Size", "COPPER_CORONA" is
/// "Copper Corona". `{`, `}`, `$` and `^` stand for numbers the game fills in; `@` is a line break.
pub(crate) fn label(s: &str) -> String {
    let s = words(s).replace('@', " ");
    let s: String = s.chars().filter(|c| !matches!(c, '{' | '}' | '$' | '^' | '|')).collect();
    let s = s.trim_start_matches(['+', '%', ' ']);
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.trim_end_matches(['!', '.']).to_string();
    if s.chars().any(char::is_lowercase) {
        return s;
    }
    s.split(' ')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c.flat_map(char::to_lowercase)).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn pic(p: String) -> Option<String> {
    Some(p)
}

pub(crate) fn mech(key: &'static str, world: u8, name: &'static str, picture: Option<String>, count: Count, unit: &'static str, items: Vec<Item>) -> Mechanic {
    Mechanic { key, world, name, picture, count, unit, items, note: None, tier_names: None, tier_notes: Vec::new(), raises: Vec::new(), have: None, later: Vec::new(), open_order: None, pace: None, crowd: None, levels_note: None, whole_tiers: false, other_side: "Tiers" }
}

/// A big number the way the game writes it: 400K, 1.5M, 96 Qa. Past the names, 1.2e45.
pub(crate) fn big(n: f64) -> String {
    const NAMES: [&str; 11] = ["", "K", "M", "B", "T", "Qa", "Qi", "Sx", "Sp", "O", "N"];
    if n.is_nan() {
        return "0".into();
    }
    if n.is_infinite() {
        return "∞".into();
    }
    if n.abs() < 1000.0 {
        // + 0.0 turns -0 into 0 ("You have -0.").
        return format!("{}", n.round() + 0.0);
    }
    let mut k = ((n.abs().log10() / 3.0).floor() as usize).min(NAMES.len());
    let mut v = n / 1000f64.powi(k as i32);
    // 999,960 rounds to 1000K: that's 1M.
    if v.abs().round() >= 1000.0 {
        k += 1;
        v /= 1000.0;
    }
    if k >= NAMES.len() {
        return format!("{n:.1e}");
    }
    let digits = if v.abs() >= 10.0 { 0 } else { 1 };
    let s = format!("{v:.digits$}");
    let s = s.trim_end_matches(".0");
    format!("{s}{}", NAMES[k])
}

pub(crate) fn at(v: &[f64], i: usize) -> f64 {
    v.get(i).copied().unwrap_or(0.0)
}


/// Each Upgrade Vault upgrade's name and the total vault levels that open it (`UpgradeVault`
/// col 6), named as the check names them.
pub fn vault_opens() -> Vec<(String, f64)> {
    rows("UpgradeVault", &[]).iter().map(|r| (w1::vault_name(text(r, 0)), cell(r, 6))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::general::card_stars;

    #[test]
    fn big_numbers_read_like_the_game() {
        assert_eq!(big(400_000.0), "400K");
        assert_eq!(big(1_500_000.0), "1.5M");
        assert_eq!(big(9.64e16), "96Qa");
        assert_eq!(big(512.0), "512");
        assert_eq!(big(999_960.0), "1M");
        assert_eq!(big(999.6), "1000");
    }

    #[test]
    fn labels_read_like_the_game() {
        assert_eq!(label("+{%_Sample_Size"), "Sample Size");
        assert_eq!(label("COPPER_CORONA"), "Copper Corona");
        assert_eq!(label("Big_Brain_Time"), "Big Brain Time");
    }

    #[test]
    fn card_stars_follow_the_game() {
        // Green Mushroom: 5 cards for the first star.
        assert_eq!(card_stars(0.0, 5.0, false), 0.0);
        assert_eq!(card_stars(1.0, 5.0, false), 0.0);
        assert_eq!(card_stars(6.0, 5.0, false), 1.0);
        assert_eq!(card_stars(21.0, 5.0, false), 2.0);
    }

    #[test]
    fn every_check_runs_on_an_empty_save() {
        let s = Save::parse(r#"{"OptLacc": []}"#).unwrap();
        let ms = all(&s);
        assert!(ms.len() >= 25);
        // The key is the card's id and its Show all data: two cards can't share one.
        let mut keys: Vec<&str> = ms.iter().map(|m| m.key).collect();
        keys.sort_unstable();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n, "two mechanics share a key");
        for m in &ms {
            assert!(!m.items.is_empty(), "{} has no items", m.key);
            assert_eq!(m.have(), 0.0, "{} reads progress from nothing", m.key);
        }
    }
}
