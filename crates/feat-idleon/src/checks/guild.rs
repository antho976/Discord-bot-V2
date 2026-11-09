//! Guild bonuses: `Guild[0][i]` is bonus i's level, the guild's and not the player's (only the
//! guild's owner spends guild points). `GuildBonuses` col 0 its name, col 3 its bonus text, col 7
//! its max level.

use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![guild(save)]
}

/// Bonuses left out: one with no bonus yet (max 0), and the one for small guilds, which a
/// guild past its size has no reason to level.
fn counted(row: &Value) -> bool {
    cell(row, 7) > 0.0 && !text(row, 0).starts_with("Bonus_GP_for_small_guilds")
}

fn guild(save: &Save) -> Mechanic {
    let levels = save.numbers("Guild", &[0]);
    let items = rows("GuildBonuses", &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| counted(r))
        .map(|(i, r)| {
            let mut it = Item::new(label(text(r, 0)), None, at(&levels, i), cell(r, 7));
            if !it.done() {
                it.how = Some(label(text(r, 3)));
            }
            it
        })
        .collect();
    let mut m = mech("guild", 0, "Guild Bonuses", None, Count::Levels, "levels", items);
    m.note = Some("These are your guild's levels: only its owner spends guild points on them. Bonus GP for small guilds is left out.".into());
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_come_from_the_guild_and_small_guild_gp_is_left_out() {
        let s = Save::parse(r#"{"OptLacc": [0], "Guild": [[100, 25]]}"#).unwrap();
        let m = guild(&s);
        assert!(m.items[0].done(), "Guild Gifts maxes at 100");
        assert_eq!(m.items[1].level, 25.0);
        assert!(m.items.iter().all(|i| !i.name.contains("Small Guilds") && i.max > 0.0));
    }
}
