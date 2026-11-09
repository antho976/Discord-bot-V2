//! Obols: each character's own page (`ObolEqO0_i`, 21 slots) and the family page every
//! character shares (`ObolEqO1`, 24 slots). `ObolXYcoords[0]` and `[1]` are those pages'
//! slots, col 2 the slot's shape and col 3 what opens it: a class level on a character's page,
//! the account's total class levels on the family page (N.js compares `LvTotal0`). An empty
//! slot is "Blank"; a family slot not open yet is "ObolLocked" + its shape.
//!
//! Players fill the family page first, then each character's page as it levels (see
//! docs/idleon-research.md); which obol goes where is a choice, so this counts slots filled.

use super::w1::{char_name, characters, class_icon};
use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![obols(save)]
}

/// A page's slots: what opens each one.
fn opens_at(page: usize) -> Vec<f64> {
    rows("ObolXYcoords", &[page]).iter().map(|r| cell(r, 3)).collect()
}

fn filled(v: &Value) -> bool {
    v.as_str().is_some_and(|s| s.starts_with("Obol") && !s.starts_with("ObolLocked"))
}

/// One page as an item: slots filled out of all of them, and what to do next.
fn page(name: String, picture: Option<String>, slots: &[Value], opens: &[f64], level: f64, what: &str) -> Item {
    let n = filled_count(slots, opens.len());
    let mut it = Item::new(name, picture, n as f64, opens.len() as f64);
    let empty_open = (0..opens.len()).filter(|&s| opens[s] <= level && !slots.get(s).is_some_and(filled)).count();
    let next = opens.iter().copied().filter(|&o| o > level).fold(f64::INFINITY, f64::min);
    it.how = if empty_open > 0 {
        it.state = State::Ready;
        Some(if empty_open == 1 { "1 open slot is empty.".into() } else { format!("{empty_open} open slots are empty.") })
    } else if next.is_finite() {
        it.state = State::Locked;
        Some(format!("The next slot opens at {what} {}.", big(next)))
    } else {
        None
    };
    it
}

fn filled_count(slots: &[Value], len: usize) -> usize {
    slots.iter().take(len).filter(|v| filled(v)).count()
}

fn list(save: &Save, key: &str) -> Vec<Value> {
    save.get(key).and_then(Value::as_array).cloned().unwrap_or_default()
}

fn obols(save: &Save) -> Mechanic {
    let total: f64 = (0..save.characters()).map(|c| at(&save.numbers(&format!("Lv0_{c}"), &[]), 0)).sum();
    let mut items = vec![page("Family page".into(), None, &list(save, "ObolEqO1"), &opens_at(1), total, "a total of class levels of")];
    let own = opens_at(0);
    for c in 0..characters(save) {
        let level = at(&save.numbers(&format!("Lv0_{c}"), &[]), 0);
        items.push(page(format!("{}: own page", char_name(save, c)), class_icon(save, c), &list(save, &format!("ObolEqO0_{c}")), &own, level, "class Lv"));
    }
    let mut m = mech("obols", 0, "Obols", None, Count::Levels, "slots filled", items);
    m.note = Some("Slots with an obol in them. Fill the family page first: every character gets it. Drop rate obols in every slot is the simple plan.".into());
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_filled_slots_and_says_what_is_open() {
        let s = Save::parse(r#"{"OptLacc": [0], "Lv0_0": [100], "ObolEqO1": ["ObolSilverPop", "Blank", "ObolLocked1"], "ObolEqO0_0": ["ObolSilverPop", "Blank"]}"#).unwrap();
        let m = obols(&s);
        let family = &m.items[0];
        assert_eq!(family.level, 1.0);
        assert_eq!(family.max, 24.0);
        // Total class levels 100: the slots that open at 60, 80 and 100 are open, two empty.
        assert_eq!(family.how.as_deref(), Some("2 open slots are empty."));
        let own = &m.items[1];
        assert_eq!(own.level, 1.0);
        assert!(own.how.as_deref().is_some_and(|h| h.contains("empty")), "{:?}", own.how);
    }
}
