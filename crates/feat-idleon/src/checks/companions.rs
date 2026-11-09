//! Companions: the pets that follow a character and give the whole account a bonus. The game
//! keeps the ones you own on its server, not in the save; IdleonToolbox's copy has them
//! (`Save::companions`). `CompanionDB` is one row per companion: its monster ID, its bonus text
//! ("{" stands for col 2, the bonus's size).

use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![companions(save)]
}

/// Rows that aren't companions you can get: ones the game marks as not in it, and the
/// tournament winners' prizes.
fn obtainable(desc: &str) -> bool {
    !desc.starts_with("Not_officially_in_the_game") && !desc.starts_with("Congratulations_on_winning_the_Tournament")
}

/// The bonus as the game writes it, its size filled in: "+5% All Skill Efficiency".
fn bonus(row: &Value) -> String {
    let size = num(row.get(2).unwrap_or(&Value::Null));
    let s = words(text(row, 1)).replace('{', &format!("+{}", big(size)));
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
    }
}
