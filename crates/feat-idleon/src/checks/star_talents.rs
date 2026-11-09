//! Star talents: the talent tab every class shares, levelled per character with star talent
//! points. `SL_i[id]` is character i's level in talent `id`, `SM_i[id]` the most it can have
//! now (books and bonuses raise it). Talent `id` is named `TalentIconNames[id]`.
//!
//! Only the ones players level first (docs/idleon-research.md, in that order), on the character
//! furthest behind: every character needs them.

use super::w1::{char_name, characters};
use super::*;

/// The star talents to level first, by the game's name, with what each does. Stonks! stops at
/// Lv 30: past it, points cost more than it gives back.
const FIRST: [(&str, &str, Option<f64>); 10] = [
    ("TICK_TOCK", "More AFK gains, fighting and skilling.", None),
    ("CRYSTALS_4_DAYYS", "Crystal monsters spawn more often.", None),
    ("STONKS!", "Gives star talent points back; Lv 30 is enough.", Some(30.0)),
    ("SUPERSOURCE", "Base efficiency for every skill.", None),
    ("ACTION_FRENZY", "Faster skilling.", None),
    ("ANCIENT_MULTITOOL", "Efficiency for every skill.", None),
    ("STATUE_METALLURGY", "A chance for statues to drop twice.", None),
    ("STUDIOUS_QUESTER", "Skill efficiency for each quest done.", None),
    ("FROTHY_MALK", "Stronger food bonuses.", None),
    ("PRINTER_SAMPLING", "Printer samples get some of your AFK gains; 1 point does most of it.", Some(1.0)),
];

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![star_talents(save)]
}

fn star_talents(save: &Save) -> Mechanic {
    let names = rows("TalentIconNames", &[]);
    let chars = characters(save);
    let levels: Vec<Vec<f64>> = (0..chars).map(|c| save.numbers(&format!("SL_{c}"), &[])).collect();
    let maxes: Vec<Vec<f64>> = (0..chars).map(|c| save.numbers(&format!("SM_{c}"), &[])).collect();
    let items = FIRST
        .iter()
        .filter_map(|&(game_name, what, cap)| {
            let id = names.iter().position(|n| n.as_str() == Some(game_name))?;
            // The most any character can have now, or the stop for the ones worth only so much.
            let top = (0..chars).map(|c| at(&maxes[c], id)).fold(0.0, f64::max);
            let max = cap.map_or(top, |c| c.min(top.max(c)));
            let (low, who) = (0..chars).map(|c| (at(&levels[c], id), c)).fold((f64::INFINITY, 0), |a, b| if b.0 < a.0 { b } else { a });
            let low = if low.is_finite() { low } else { 0.0 };
            let mut it = Item::new(label(game_name), None, low.min(max), max);
            it.how = Some(if it.done() || chars < 2 { what.to_string() } else { format!("{what} Lowest on {}, at Lv {}.", char_name(save, who), big(low)) });
            Some(it)
        })
        .collect();
    let mut m = mech("star-talents", 0, "Star Talents", None, Count::Levels, "levels", items);
    m.note = Some("The ones players level first, on your character furthest behind: each character levels its own. A max is what your characters can reach now.".into());
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lowest_character_counts_and_stonks_stops_at_30() {
        let s = Save::parse(r#"{"OptLacc": [0], "Lv0_0": [100], "Lv0_1": [100],
            "SL_0": {"621": 50, "622": 80}, "SM_0": {"621": 100, "622": 100},
            "SL_1": {"621": 20, "622": 30}, "SM_1": {"621": 100, "622": 100}}"#).unwrap();
        let m = star_talents(&s);
        let tick = m.items.iter().find(|i| i.name == "Tick Tock").unwrap();
        assert_eq!((tick.level, tick.max), (20.0, 100.0));
        assert!(tick.how.as_deref().is_some_and(|h| h.ends_with("Lowest on Character 2, at Lv 20.")), "{:?}", tick.how);
        let stonks = m.items.iter().find(|i| i.name.starts_with("Stonks")).unwrap();
        assert!(stonks.done(), "Lv 30 is all it asks");
    }
}
