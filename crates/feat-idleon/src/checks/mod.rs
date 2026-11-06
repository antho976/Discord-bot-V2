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
mod w1;
mod w2;
mod w3;
mod w4;
mod w5;
mod w6;
mod w7;

pub(crate) use bot_core::serde_json::Value;

pub(crate) use crate::game::{GAME, num, words};
pub(crate) use crate::review::{Count, Item, Mechanic};
pub(crate) use crate::save::Save;

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
    Mechanic { key, world, name, picture, count, unit, items, note: None }
}

pub(crate) fn at(v: &[f64], i: usize) -> f64 {
    v.get(i).copied().unwrap_or(0.0)
}


#[cfg(test)]
mod tests {
    use super::*;
    use super::general::card_stars;

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
        for m in &ms {
            assert!(!m.items.is_empty(), "{} has no items", m.key);
            assert_eq!(m.have(), 0.0, "{} reads progress from nothing", m.key);
        }
    }
}
