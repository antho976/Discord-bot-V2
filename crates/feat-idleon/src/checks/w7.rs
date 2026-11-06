//! World 7: Shimmerfin Deep.

use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![
        research_grid(save),
        capped(save, "sushi", 7, "Sushi Station", "SushiUPG", "Sushi", &[2], "SushiUpg"),
        capped(save, "jelly", 7, "Jelly Operator", "JellyUPG", "Research", &[17], "JellyUpg"),
        capped(save, "minehead", 7, "Minehead", "MineheadUPG", "Research", &[8], "MineUpg"),
        coral_reef(save),
        zenith_market(save),
        legend_talents(save),
        spelunk_upgrades(save),
    ]
}

/// Research grid: `Research[0][i]`; `ResGridSquares` col 1 is the max. Rows named "Name" are
/// empty squares.
fn research_grid(save: &Save) -> Mechanic {
    let levels = save.numbers("Research", &[0]);
    let items = rows("ResGridSquares", &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| text(r, 0) != "Name" && cell(r, 1) > 0.0)
        .map(|(i, r)| Item::new(label(text(r, 0)), None, at(&levels, i), cell(r, 1)))
        .collect();
    mech("research", 7, "Research Grid", None, Count::Levels, "levels", items)
}

/// Upgrade shops kept the same way: levels at `key[path][i]`, max in col 1 (0 for a row that
/// isn't bought, 999 and over for no cap). Only capped upgrades count.
#[allow(clippy::too_many_arguments)]
fn capped(save: &Save, key: &'static str, world: u8, name: &'static str, table: &str, save_key: &str, path: &[usize], picture: &str) -> Mechanic {
    let levels = save.numbers(save_key, path);
    let items: Vec<Item> = rows(table, &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| cell(r, 1) > 0.0 && cell(r, 1) < 999.0)
        .map(|(i, r)| Item::new(label(text(r, 0)), pic(format!("{picture}{i}")), at(&levels, i), cell(r, 1)))
        .collect();
    let mut m = mech(key, world, name, pic(format!("{picture}0")), Count::Levels, "levels", items);
    m.note = Some("Only the upgrades with a cap. The rest level up without end.".into());
    m
}

/// Coral Reef: `Spelunk[13][i]`; col 1 is the max. Row 3's cap is raised from 12 to 15 by a
/// Menagerie unlock; the last row is a filler.
fn coral_reef(save: &Save) -> Mechanic {
    let levels = save.numbers("Spelunk", &[13]);
    let letters = ['z', 'A', 'B', 'C', 'D'];
    let items = rows("CoralReef", &[])
        .iter()
        .enumerate()
        .take(5)
        .map(|(i, r)| {
            let max = if i == 3 { 15.0 } else { cell(r, 1) };
            Item::new(label(text(r, 0)), pic(format!("Reef{}1", letters[i])), at(&levels, i), max)
        })
        .collect();
    mech("coral-reef", 7, "Coral Reef", pic("Reefz1".into()), Count::Levels, "levels", items)
}

/// Zenith Market: `Spelunk[45][i]`; col 3 is the max.
fn zenith_market(save: &Save) -> Mechanic {
    let levels = save.numbers("Spelunk", &[45]);
    let items = rows("ZenithMarket", &[])
        .iter()
        .enumerate()
        .map(|(i, r)| Item::new(label(text(r, 0)), None, at(&levels, i), cell(r, 3)))
        .collect();
    mech("zenith", 7, "Zenith Market", None, Count::Levels, "levels", items)
}

/// Legend Talents: `Spelunk[18][i]`. The max is col 1, raised by four research nodes (each for
/// five talents, by its own max) and by 3 for nine talents with a Menagerie unlock.
fn legend_talents(save: &Save) -> Mechanic {
    let levels = save.numbers("Spelunk", &[18]);
    let grid = rows("ResGridSquares", &[]);
    let node_max = |n: usize| grid.get(n).map_or(0.0, |r| cell(r, 1));
    let raises: [(usize, [usize; 5]); 4] = [(130, [2, 13, 3, 18, 5]), (131, [9, 23, 33, 1, 16]), (132, [20, 31, 19, 27, 8]), (152, [34, 30, 29, 6, 26])];
    let menagerie = [35, 33, 36, 0, 20, 31, 15, 32, 6];
    // The talent shown in each icon slot: `Spelunky[26][k]` is the talent index.
    let order = rows("Spelunky", &[26]);
    let icon_for = |i: usize| order.iter().position(|v| num(v) as usize == i).map(|k| format!("LegendTalentIcon{k}"));
    let runtime_names = ["Minehead My Beloved", "Two Time Jelly Operation"];
    let items = rows("LegendTalents", &[])
        .iter()
        .enumerate()
        .take(42)
        .map(|(i, r)| {
            let mut max = cell(r, 1);
            for (node, talents) in &raises {
                if talents.contains(&i) {
                    max += node_max(*node);
                }
            }
            if menagerie.contains(&i) {
                max += 3.0;
            }
            let name = if i >= 40 { runtime_names[i - 40].to_string() } else { label(text(r, 0)) };
            Item::new(name, icon_for(i), at(&levels, i), max)
        })
        .collect();
    let mut m = mech("legend-talents", 7, "Legend Talents", pic("LegendTalentIcon0".into()), Count::Levels, "levels", items);
    m.note = Some("Caps include what research nodes and the Menagerie add.".into());
    m
}

/// Spelunking shop: `Spelunk[5][i]` (-1 is locked); `SpelunkUpg` col 3 is the max, 99999 for no cap.
fn spelunk_upgrades(save: &Save) -> Mechanic {
    let levels = save.numbers("Spelunk", &[5]);
    let items = rows("SpelunkUpg", &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| cell(r, 3) > 0.0 && cell(r, 3) < 999.0)
        .map(|(i, r)| Item::new(label(text(r, 0)), pic(format!("CaveShopUpg{i}")), at(&levels, i).max(0.0), cell(r, 3)))
        .collect();
    let mut m = mech("spelunk-shop", 7, "Spelunking Shop", pic("CaveShopUpg0".into()), Count::Levels, "levels", items);
    m.note = Some("Only the upgrades with a cap. The rest level up without end.".into());
    m
}
