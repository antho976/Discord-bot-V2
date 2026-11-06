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
        spelunk_caves(save),
        spelunk_chapters(save),
        spelunk_depth(save),
        advice_fish(save),
        clam_work(save),
        clam_upgrades(save),
        coral_kid(save),
        dancing_coral(save),
        gallery_trophies(save),
        gallery_podiums(save),
        gallery_nametags(save),
        menagerie_unlocks(save),
        menagerie_boosts(save),
        menagerie_missions(save),
        menagerie_jellyfish(save),
        observations(save),
        observation_levels(save),
        posty_notes(save),
        glimbo(save),
        the_button(save),
    ]
}

/// Whether the account owns Menagerie unlock `i` (the `Research` table's row 56 names them).
/// N.js `MenagerieStuff("UnlockOwned")`: `Menagerie[2][i] >= 1`. Other worlds' caps read this:
/// 9 Pillar coral +3, 11 Shiny +10, 14 two more Legend Talents, 16 an Atom, 17 Statue flair, 19
/// Legend Talents +3, 20 meals +10, 21 Tiny Cog T11.
pub(crate) fn menagerie_unlock(save: &Save, i: usize) -> bool {
    save.at("Menagerie", &[2, i]).map(num).unwrap_or(0.0) >= 1.0
}

/// Game text read like the game, with its icon glyphs and number stand-ins dropped:
/// "}x_Research_EXP_gains" is "Research EXP gains", "Go_Go_Secret_Owl_製" is "Go Go Secret Owl",
/// "+1_Legend_Talent_PTS" stays "+1 Legend Talent PTS". `潤` stands for "&".
fn clean(s: &str) -> String {
    let s: String = s
        .replace('潤', " & ")
        .replace("''", "'")
        .chars()
        .map(|c| if matches!(c, '_' | '|' | '@') { ' ' } else { c })
        .filter(|&c| (c as u32) < 0x2E80 && !matches!(c, '{' | '}' | '$' | '^'))
        .collect();
    let words: Vec<&str> = s.split_whitespace().filter(|w| !w.chars().all(|c| matches!(c, '+' | '-' | 'x' | '%'))).collect();
    let s = words.join(" ");
    let s = s.trim_end_matches(['!', '.', ',']);
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// The first sentence of a longer game text, cut at its first "!".
fn first_sentence(s: &str) -> String {
    clean(s.split('!').next().unwrap_or(s))
}

/// A row of the `Research` or `Spelunky` table that is a plain list of texts.
fn list(table: &str, row: usize) -> Vec<&'static str> {
    rows(table, &[row]).iter().map(|v| v.as_str().unwrap_or("")).collect()
}

/// A Done item: 1 of 1 when `have`.
fn found(name: impl Into<String>, picture: Option<String>, have: bool) -> Item {
    Item::new(name, picture, f64::from(u8::from(have)), 1.0)
}

/// Spelunking caves: `Spelunk[0][i]` is 1 once that cave's boss is beaten, which gives its lore
/// bonus (N.js `DoWeHaveLoreN1`). Caves are `Spelunky[6]`; "NameNN" rows are not in the game yet.
fn spelunk_caves(save: &Save) -> Mechanic {
    let beaten = save.numbers("Spelunk", &[0]);
    let items = caves()
        .map(|(i, name)| found(format!("{name} boss"), pic(format!("CaveBoss{i}")), at(&beaten, i) >= 1.0))
        .collect();
    let mut m = mech("spelunk-caves", 7, "Spelunking Cave Bosses", pic("CaveBoss0".into()), Count::Done, "beaten", items);
    m.note = Some("Each boss beaten gives a lore bonus somewhere else in the game.".into());
    m
}

/// The caves that exist: index and name.
fn caves() -> impl Iterator<Item = (usize, String)> {
    list("Spelunky", 6).into_iter().enumerate().filter(|(_, n)| !n.starts_with("Name") && !n.is_empty()).map(|(i, n)| (i, label(n)))
}

/// Lore chapter bonuses: `Spelunk[8][chapter * 4 + k]` is the level of bonus k of a chapter
/// (`SpelunkChapters[chapter][k]`). Levels come from reading pages and have no cap; the bonus
/// closes in on its max (N.js `ChapterBonus`, "decay"), so this is Open.
fn spelunk_chapters(save: &Save) -> Mechanic {
    let levels = save.numbers("Spelunk", &[8]);
    let mut items = Vec::new();
    for c in 0..GAME.rows("SpelunkChapters").len() {
        let page = format!("Spelunking{c}");
        let title = GAME.item_name(&page).and_then(|n| n.split('\'').nth(1)).map(str::to_string).unwrap_or_else(|| format!("Chapter {}", c + 1));
        for (k, bonus) in rows("SpelunkChapters", &[c]).iter().enumerate() {
            items.push(Item::new(format!("{title}: {}", clean(text(bonus, 0))), pic(page.clone()), at(&levels, c * 4 + k), 0.0));
        }
    }
    let mut m = mech("spelunk-chapters", 7, "Spelunking Lore Chapters", pic("Spelunking0".into()), Count::Open, "levels", items);
    m.note = Some("Chapter levels have no cap. Each bonus gets closer to its max the more levels it has.".into());
    m
}

/// Deepest depth per cave: `Spelunk[1][i]`, and in Manic mode `Spelunk[47][i]` (N.js
/// `AllDepthTOT` adds both). No cap.
fn spelunk_depth(save: &Save) -> Mechanic {
    let normal = save.numbers("Spelunk", &[1]);
    let manic = save.numbers("Spelunk", &[47]);
    let mut items = Vec::new();
    for (i, name) in caves() {
        items.push(Item::new(name.clone(), pic(format!("CaveDoor{i}")), at(&normal, i), 0.0));
        items.push(Item::new(format!("{name} (Manic)"), pic(format!("CaveDoor{i}")), at(&manic, i), 0.0));
    }
    mech("spelunk-depth", 7, "Spelunking Depths", pic("CaveDoor0".into()), Count::Open, "depth", items)
}

/// Advice Fish: `Spelunk[11][i]`, upgrades in `Spelunky[18]` ("Name,text,max bonus,cost,"). No
/// level cap: N.js `BigFishBonuses` gives max × level / (level + 100).
fn advice_fish(save: &Save) -> Mechanic {
    let levels = save.numbers("Spelunk", &[11]);
    let items = list("Spelunky", 18)
        .into_iter()
        .enumerate()
        .map(|(i, r)| Item::new(label(r.split(',').next().unwrap_or("")), None, at(&levels, i), 0.0))
        .collect();
    let mut m = mech("advice-fish", 7, "Advice Fish", None, Count::Open, "levels", items);
    m.note = Some("No level cap. Level 100 gives half of each bonus, level 900 gives 90%.".into());
    m
}

/// Clam Work compensations: one more each Worker Class level, `OptLacc[464]`. The twelve are
/// `Spelunky[30]`; the game shows compensation i once the level is above i.
fn clam_work(save: &Save) -> Mechanic {
    let level = save.option(464);
    let items = list("Spelunky", 30).into_iter().enumerate().map(|(i, r)| found(clean(r), None, level > i as f64)).collect();
    mech("clam-work", 7, "Clam Work Compensations", pic("ClamW".into()), Count::Done, "earned", items)
}

/// The Clamworks pearl upgrades, `OptLacc[455 + i]`, and the Musselini page, `Menagerie[5][i]`.
/// Names come from the page's own text (tools/idleon-extract/extra_w7.py). No caps.
fn clam_upgrades(save: &Save) -> Mechanic {
    let names: Vec<&str> = GAME.extra("w7").get("clam_upgrades").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let mussel = save.numbers("Menagerie", &[5]);
    let items: Vec<Item> = (0..18)
        .map(|i| {
            let name = names.get(i).map(|n| label(n)).unwrap_or_else(|| format!("Upgrade {}", i + 1));
            let level = if i < 9 { save.option(455 + i) } else { at(&mussel, i - 9) };
            Item::new(name, pic(format!("ClamPearl{}", usize::from(i >= 9))), level, 0.0)
        })
        .collect();
    let mut m = mech("clam-upgrades", 7, "Clamworks Upgrades", pic("ClamPearl0".into()), Count::Open, "levels", items);
    m.note = Some("No caps. The second nine are the Musselini page, opened by a Menagerie unlock.".into());
    m
}

/// Coral Kid: `OptLacc[427 + i]` for the first six, `OptLacc[616 + i - 6]` for the Singularity
/// page (Menagerie unlock 2). Texts in `Spelunky[25]`. N.js `CoralKidUpgCost` grows without a cap.
fn coral_kid(save: &Save) -> Mechanic {
    let items = list("Spelunky", 25)
        .into_iter()
        .enumerate()
        .map(|(i, r)| Item::new(first_sentence(r.split(",_").next().unwrap_or(r)), None, save.option(if i < 6 { 427 + i } else { 616 + i - 6 }), 0.0))
        .collect();
    let mut m = mech("coral-kid", 7, "Coral Kid", pic("CoralDivUpg".into()), Count::Open, "levels", items);
    m.note = Some(if menagerie_unlock(save, 2) {
        "No caps. Upgrades 7 to 12 are the Singularity page.".into()
    } else {
        "No caps. Upgrades 7 to 12 are the Singularity page, opened by a Menagerie unlock you don't have yet.".into()
    });
    m
}

/// Dancing Coral: `Spelunk[4][6]` is how many of its bonuses are bought, in order. Bonus i is
/// `Spelunky[23][i]` and also raises shrine i (`ShrineInfo`) past level 200. The game's last two
/// rows read "who knows" and are left out.
fn dancing_coral(save: &Save) -> Mechanic {
    let bought = save.at("Spelunk", &[4, 6]).map(num).unwrap_or(0.0);
    let shrines = rows("ShrineInfo", &[]);
    let items = list("Spelunky", 23)
        .into_iter()
        .enumerate()
        .filter(|(_, r)| !r.contains("who_knows"))
        .map(|(i, r)| {
            let shrine = shrines.get(i).map(|s| label(text(s, 0))).unwrap_or_default();
            found(format!("{} ({shrine})", clean(r)), pic(format!("Coral{i}")), bought > i as f64)
        })
        .collect();
    mech("dancing-coral", 7, "Dancing Coral", pic("Coral0".into()), Count::Done, "bought", items)
}

/// Trophy numbers in the Gallery: `Spelunk[16]`, 48 storage slots then the podiums; -1 is empty.
fn gallery_slots(save: &Save) -> Vec<f64> {
    save.numbers("Spelunk", &[16])
}

/// Gallery trophies: every `TrophyN` the game has, found once it sits anywhere in the Gallery.
/// "I Made This Game" (Trophy4) is the developer's own and only counts when you have it.
fn gallery_trophies(save: &Save) -> Mechanic {
    let slots = gallery_slots(save);
    let items = (1..100)
        .filter_map(|n| GAME.item_name(&format!("Trophy{n}")).map(|name| (n, name)))
        .filter_map(|(n, name)| {
            let have = slots.contains(&(n as f64));
            (n != 4 || have).then(|| found(name, pic(format!("Trophy{n}")), have))
        })
        .collect();
    let mut m = mech("gallery-trophies", 7, "Gallery Trophies", pic("Trophy1".into()), Count::Done, "in the Gallery", items);
    m.note = Some("A trophy counts once it's in the Gallery, on a podium or in storage.".into());
    m
}

/// Gallery podiums: the slots after the first 48 of `Spelunk[16]`. N.js `PodiumsOwned` caps
/// them at 19.
fn gallery_podiums(save: &Save) -> Mechanic {
    let slots = gallery_slots(save);
    let items = (0..19).map(|i| found(format!("Podium {}", i + 1), None, at(&slots, 48 + i) >= 1.0)).collect();
    let mut m = mech("gallery-podiums", 7, "Gallery Podiums", pic("GalleryPodiumA0".into()), Count::Done, "filled", items);
    m.note = Some("19 podiums is the most the game allows. An empty one is either not owned yet or has no trophy on it.".into());
    m
}

/// Gallery nametags: `Spelunk[17][i]` is the level of `EquipmentNametag{i}` (6 is "6b"). The
/// bonus stops growing at level 5 (N.js reads "1,1.6,2,2.3,2.5"), so 5 is the max here. Lava's
/// own nametag (2) only counts when you have it.
fn gallery_nametags(save: &Save) -> Mechanic {
    let levels = save.numbers("Spelunk", &[17]);
    let items = (0..levels.len().max(70))
        .filter_map(|i| {
            let id = if i == 6 { "EquipmentNametag6b".to_string() } else { format!("EquipmentNametag{i}") };
            let name = GAME.item_name(&id)?;
            let level = at(&levels, i);
            (i != 2 || level >= 1.0).then(|| Item::new(name, pic(id), level.min(5.0), 5.0))
        })
        .collect();
    let mut m = mech("gallery-nametags", 7, "Gallery Nametags", pic("EquipmentNametag1".into()), Count::Levels, "levels", items);
    m.note = Some("A nametag's bonus stops growing at level 5, so levels past 5 aren't counted.".into());
    m
}

/// Menagerie unlocks: `Menagerie[2][i]`, texts in `Research` row 56.
fn menagerie_unlocks(save: &Save) -> Mechanic {
    let items = list("Research", 56).into_iter().enumerate().map(|(i, r)| found(clean(r), pic(format!("MenaIconO{i}")), menagerie_unlock(save, i))).collect();
    let mut m = mech("menagerie-unlocks", 7, "Menagerie Unlocks", pic("MenaIconO0".into()), Count::Done, "owned", items);
    m.note = Some("Several of these raise caps elsewhere: Pillar coral, Legend Talents, meals, Shiny levels, Atoms.".into());
    m
}

/// Menagerie boosts: `Menagerie[3][i]`, texts in `Research` row 57. Bought with Lumi, no cap.
fn menagerie_boosts(save: &Save) -> Mechanic {
    let levels = save.numbers("Menagerie", &[3]);
    let items = list("Research", 57).into_iter().enumerate().map(|(i, r)| Item::new(clean(r), pic(format!("MenaIconP{i}")), at(&levels, i), 0.0)).collect();
    mech("menagerie-boosts", 7, "Menagerie Boosts", pic("MenaIconP0".into()), Count::Open, "levels", items)
}

/// Menagerie missions: completions in `Menagerie[1][i]`; what each counts is `Research` row 55.
/// Every completion raises the next goal, with no end.
fn menagerie_missions(save: &Save) -> Mechanic {
    let done = save.numbers("Menagerie", &[1]);
    let items = list("Research", 55).into_iter().enumerate().map(|(i, r)| Item::new(clean(r), pic(format!("MenaIconY{i}")), at(&done, i), 0.0)).collect();
    mech("menagerie-missions", 7, "Menagerie Missions", pic("MenaIconY0".into()), Count::Open, "completions", items)
}

/// The Silver Jellyfish's two upgrades: `Menagerie[0][9]` (Obol bonuses, N.js `ObolMISCmulti`)
/// stops at 100 and `Menagerie[0][10]` (auto-trash, `ObolAutoTrashRate`) at 190: the game only
/// sells them below those.
fn menagerie_jellyfish(save: &Save) -> Mechanic {
    let v = save.numbers("Menagerie", &[0]);
    let items = vec![
        Item::new("Silver Jellyfish: Obol bonuses", None, at(&v, 9), 100.0),
        Item::new("Silver Jellyfish: Obol auto-trash", None, at(&v, 10), 190.0),
    ];
    mech("menagerie-jellyfish", 7, "Menagerie Jellyfish", pic("MenaJelly0".into()), Count::Levels, "levels", items)
}

/// Research observations: `Research[2][i]` is 1 once found. `Occurrences` names them; N.js
/// `OccurrencesToBeFound` offers more as Research levels rise, up to 43.
fn observations(save: &Save) -> Mechanic {
    let have = save.numbers("Research", &[2]);
    let items = observation_rows().map(|(i, name)| found(name, pic(format!("ResObj{i}")), at(&have, i) >= 1.0)).collect();
    let mut m = mech("observations", 7, "Research Observations", pic("ResObj0".into()), Count::Done, "found", items);
    m.note = Some("New observations appear as your Research level rises, 43 in all.".into());
    m
}

fn observation_rows() -> impl Iterator<Item = (usize, String)> {
    rows("Occurrences", &[]).into_iter().enumerate().filter(|(_, r)| text(r, 0) != "Name").take(43).map(|(i, r)| (i, label(text(r, 0))))
}

/// Observation insight levels: `Research[4][i]`. N.js `ObservationInsightExpREQ` grows without a cap.
fn observation_levels(save: &Save) -> Mechanic {
    let levels = save.numbers("Research", &[4]);
    let items = observation_rows().map(|(i, name)| Item::new(name, pic(format!("ResObj{i}")), at(&levels, i), 0.0)).collect();
    mech("observation-levels", 7, "Observation Insight", pic("ResObj0".into()), Count::Open, "levels", items)
}

/// Posty Notes: N.js `PostyNotesOwned` is min(14, Research level / 10). Research level is the
/// account's best `Lv0_c[20]`. Texts in `Research` row 6.
fn posty_notes(save: &Save) -> Mechanic {
    let level = (0..save.characters().max(12)).map(|c| at(&save.numbers(&format!("Lv0_{c}"), &[]), 20)).fold(0.0, f64::max);
    let owned = (level / 10.0).floor().min(14.0);
    let items = list("Research", 6)
        .into_iter()
        .enumerate()
        .map(|(i, r)| found(format!("Posty Note {}: {}", i + 1, first_sentence(r)), pic(format!("ResPosty{i}")), (i as f64) < owned))
        .collect();
    let mut m = mech("posty-notes", 7, "Posty Notes", pic("ResPosty0".into()), Count::Done, "unlocked", items);
    m.note = Some("One more every 10 Research levels.".into());
    m
}

/// Glimbo trades: `Research[12][t]` trades for slot t, which pays `Research` row 27's item and
/// raises the max level of vault upgrade `Research` row 26 (`UpgradeVault`). No cap.
fn glimbo(save: &Save) -> Mechanic {
    let trades = save.numbers("Research", &[12]);
    let vault = rows("UpgradeVault", &[]);
    let pay = list("Research", 27);
    let items = list("Research", 26)
        .into_iter()
        .enumerate()
        .map(|(t, v)| {
            let name = v.parse::<usize>().ok().and_then(|v| vault.get(v)).map(|r| clean(text(r, 0))).unwrap_or_default();
            Item::new(name, pay.get(t).map(|p| p.to_string()), at(&trades, t), 0.0)
        })
        .collect();
    let mut m = mech("glimbo", 7, "Glimbo Trades", None, Count::Open, "trades", items);
    m.note = Some("Each trade raises the max level of that vault upgrade.".into());
    m
}

/// The Button: `OptLacc[594]` counts presses (N.js `Button_Bonuses`), with no end.
fn the_button(save: &Save) -> Mechanic {
    let items = vec![Item::new("Presses", None, save.option(594), 0.0)];
    let mut m = mech("the-button", 7, "The Button", None, Count::Open, "presses", items);
    m.note = Some("Every five presses feed the next of its nine bonuses, round and round.".into());
    m
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

/// Coral Reef: `Spelunk[13][i]`; col 1 is the max. Row 3's cap is raised from 12 to 15 by
/// Menagerie unlock 9 (N.js rewrites `CoralReef[3][1]`); the last row is a filler.
fn coral_reef(save: &Save) -> Mechanic {
    let levels = save.numbers("Spelunk", &[13]);
    let letters = ['z', 'A', 'B', 'C', 'D'];
    let items = rows("CoralReef", &[])
        .iter()
        .enumerate()
        .take(5)
        .map(|(i, r)| {
            let max = if i == 3 { 15.0 } else { cell(r, 1) };
            Item::new(clean(text(r, 0)), pic(format!("Reef{}1", letters[i])), at(&levels, i), max)
        })
        .collect();
    let mut m = mech("coral-reef", 7, "Coral Reef", pic("Reefz1".into()), Count::Levels, "levels", items);
    m.note = Some(if menagerie_unlock(save, 9) {
        "Pillar's cap of 15 includes your Menagerie unlock.".into()
    } else {
        "Pillar's cap counts the Menagerie unlock that raises it from 12 to 15, which you don't have yet.".into()
    });
    m
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
/// five talents, by its own max) and by 3 for nine talents with Menagerie unlock 19. Talents 40
/// and 41 are named at runtime and only show with Menagerie unlock 14.
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
    let mut note = String::from("Caps include what research nodes add, and the Menagerie unlock worth +3 on nine talents");
    note += if menagerie_unlock(save, 19) { ", which you have." } else { ", which you don't have yet." };
    if !menagerie_unlock(save, 14) {
        note += " The last two talents come with another Menagerie unlock.";
    }
    m.note = Some(note);
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
