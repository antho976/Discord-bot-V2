//! Account-wide: cards, achievements and the rest that belongs to no world.

use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![
        cards(save),
        achievements(save),
    ]
}

/// The most stars a card can have: 4, then 5 with the Rift unlock and 6 with Spelunking's.
const CARD_STARS: usize = 6;

/// Stars for `count` cards of a card whose first star costs `base` (the game's `CardLv`).
pub(super) fn card_stars(count: f64, base: f64, chizoar: bool) -> f64 {
    if count <= 0.0 {
        return 0.0;
    }
    let mut level = 1;
    for s in 0..CARD_STARS {
        let f = s as f64;
        let step = if chizoar {
            1.5 * (f + 1.0 + (f / 3.0).floor()).powi(2)
        } else {
            base * (f + 1.0 + (f / 3.0).floor() + 16.0 * (f / 4.0).floor() + 100.0 * (f / 5.0).floor()).powi(2)
        };
        if count > step {
            level = s + 2;
        }
    }
    (level - 1) as f64
}

/// Cards: `Cards0` maps a monster to the cards you have of it.
fn cards(save: &Save) -> Mechanic {
    let have = save.get("Cards0");
    let mut items = Vec::new();
    for group in GAME.rows("CardStuff") {
        for r in group.as_array().into_iter().flatten() {
            let mob = text(r, 0);
            if mob == "Blank" || mob.is_empty() {
                continue;
            }
            let count = have.and_then(|h| h.get(mob)).map(num).unwrap_or(0.0);
            let stars = card_stars(count, cell(r, 2), mob == "Boss3B");
            let name = GAME.monster_name(mob).map(str::to_string).unwrap_or_else(|| words(mob));
            let mut it = Item::new(name, pic(format!("Cards{}", text(r, 1))), stars, CARD_STARS as f64);
            it.level_name = Some(match (count > 0.0, stars as u8) {
                (false, _) => "Not found".into(),
                (true, 0) => "Found".into(),
                (true, 1) => "1 star".into(),
                (true, n) => format!("{n} stars"),
            });
            items.push(it);
        }
    }
    let mut m = mech("cards", 0, "Cards", pic("CardsA0".into()), Count::Levels, "stars", items);
    m.note = Some("4 stars at first, 5 with the Rift unlock, 6 with Spelunking's.".into());
    m
}

/// Achievements: `AchieveReg[i]` is -1 once done. Only the first 420 are ever saved.
fn achievements(save: &Save) -> Mechanic {
    let reg = save.numbers("AchieveReg", &[]);
    let items = rows("RegAchieves", &[])
        .iter()
        .enumerate()
        .take(420)
        .filter(|(_, r)| text(r, 0) != "FILLERZZZ_ACH")
        .map(|(i, r)| Item::new(label(text(r, 0)), None, f64::from(at(&reg, i) == -1.0), 1.0))
        .collect();
    mech("achievements", 0, "Achievements", None, Count::Done, "done", items)
}
