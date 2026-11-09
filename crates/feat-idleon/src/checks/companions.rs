//! Companions: the pets that follow a character and give the whole account a bonus. The game
//! keeps the ones you own on its server, not in the save; IdleonToolbox's copy has them
//! (`Save::companions`). `CompanionDB` is one row per companion: its monster ID and its bonus
//! text, size included.

use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![companions(save)]
}

/// Rows that aren't companions you can get: ones the game marks as not in it, the tournament
/// winners' prizes, and the one only 100 players have.
fn obtainable(desc: &str) -> bool {
    !["Not_officially_in_the_game", "Congratulations_on_winning_the_Tournament", "Only_100_of_these_exist"].iter().any(|p| desc.starts_with(p))
}

/// The bonus as the game writes it: "+5% All Skill Efficiency" (`{` is the game's "+").
fn bonus(row: &Value) -> String {
    let s = words(text(row, 1)).replace('{', "+");
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn companions(save: &Save) -> Mechanic {
    let owned = save.companions.as_deref().unwrap_or(&[]);
    let items: Vec<Item> = rows("CompanionDB", &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| obtainable(text(r, 1)))
        .map(|(i, r)| {
            let id = text(r, 0);
            let name = GAME.monster_name(id).map(str::to_string).unwrap_or_else(|| words(id));
            let has = owned.binary_search(&i).is_ok();
            let mut it = Item::new(name, None, f64::from(has), 1.0);
            it.how = Some(bonus(r));
            it
        })
        .collect();
    let n = items.iter().filter(|i| i.done()).count();
    let mut m = mech("companions", 0, "Companions", None, Count::Done, "owned", items);
    m.have = Some(match &save.companions {
        Some(_) => Have { figure: format!("{n} owned"), text: "Each one's bonus counts for the whole account while you own it.".into() },
        None => Have { figure: "Not in this paste".into(), text: "The game keeps companions off the save. Copy the save from IdleonToolbox to see yours.".into() },
    });
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_companions_count_and_prizes_are_left_out() {
        let s = Save::parse(r#"{"data": {"OptLacc": [0]}, "companion": {"l": ["1,0,0,1,0"]}}"#).unwrap();
        let m = companions(&s);
        assert_eq!(m.items.iter().filter(|i| i.done()).count(), 1);
        assert!(m.items.iter().all(|i| !i.how.as_deref().unwrap_or("").contains("Tournament")));
        let talents = m.items.iter().find(|i| i.how.as_deref() == Some("+25 Lv for all Talents")).unwrap();
        assert!(talents.done(), "row 1 is the +25 talent levels companion");
        assert!(m.items.iter().any(|i| i.how.as_deref() == Some("+100% Drop Rate and Class Exp")));
        assert!(m.items.iter().all(|i| !i.how.as_deref().unwrap_or("").starts_with("Only 100")));
    }
}
