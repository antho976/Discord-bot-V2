//! World 7: Shimmerfin Deep.

use super::*;

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![
        research_grid(save),
        sushi(save),
        sushi_types(save),
        jelly(save),
        jelly_obstructions(save),
        minehead(save),
        minehead_opponents(save),
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

/// A Menagerie unlock named by its own text, `Research` row 56: `the Menagerie unlock "Unlock the
/// Silver Jellyfish within the Menagerie"`. Its index alone is ambiguous (the game shows no
/// numbers).
pub(crate) fn menagerie_unlock_name(i: usize) -> String {
    let text = list("Research", 56).get(i).map(|t| clean(t)).unwrap_or_default();
    if text.is_empty() { "a Menagerie unlock".into() } else { format!("the Menagerie unlock \"{text}\"") }
}

/// A whole number with thousands commas: "7,000".
fn whole(n: f64) -> String {
    crate::page::num(n.floor())
}

/// The first letter in capitals.
fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
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

use super::general::emporium_owned;

/// A Done item: 1 of 1 when `have`.
fn found(name: impl Into<String>, picture: Option<String>, have: bool) -> Item {
    Item::new(name, picture, f64::from(u8::from(have)), 1.0)
}

/// A number in the save, never negative or endless (a paste can claim anything).
fn held(save: &Save, key: &str, path: &[usize]) -> f64 {
    let v = save.at(key, path).map(num).unwrap_or(0.0);
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

/// How many of item `id` sit in storage: `ChestOrder[i]` is the item, `ChestQuantity[i]` the
/// count. The game also counts what characters carry, so this is a floor.
fn chest(save: &Save, id: &str) -> f64 {
    let Some(order) = save.get("ChestOrder").and_then(Value::as_array) else { return 0.0 };
    let qty = save.numbers("ChestQuantity", &[]);
    let n: f64 = order.iter().enumerate().filter(|(_, v)| v.as_str() == Some(id)).map(|(i, _)| at(&qty, i)).sum();
    if n.is_finite() { n.max(0.0) } else { 0.0 }
}

/// The account's Research level: the best `Lv0_c[20]` (the game reads the active character's,
/// and they all match).
fn research_level(save: &Save) -> f64 {
    (0..save.characters().clamp(1, 12)).map(|c| at(&save.numbers(&format!("Lv0_{c}"), &[]), 20)).filter(|v| v.is_finite()).fold(0.0, f64::max)
}

/// The cost line for one more level, and Ready when `have` covers it.
fn pay(it: &mut Item, cost: f64, have: f64, what: &str, discounts: bool) {
    if !cost.is_finite() {
        return;
    }
    let tail = if discounts { " before your discounts" } else { "" };
    // The game makes some first levels free (the first Jelly Operator upgrade, `0==t?0`).
    it.how = Some(if cost <= 0.0 { "The next level is free.".to_string() } else { format!("Next level costs {} {what}{tail}.", big(cost.ceil())) });
    if cost <= have {
        it.state = State::Ready;
    }
}

/// The level `have` takes an item to on its own from `level`, stopping at `max` (0 for none) and
/// after 400 levels.
fn reach(level: f64, max: f64, have: f64, cost: impl Fn(f64) -> f64) -> f64 {
    let (mut lv, mut left) = (level.max(0.0), have);
    for _ in 0..400 {
        if max > 0.0 && lv >= max {
            break;
        }
        let c = cost(lv);
        if !(c.is_finite() && c <= left) {
            break;
        }
        left -= c;
        lv += 1.0;
    }
    lv
}

/// The `have` line from what the two lowest Ready items reach: (item index, level reached).
fn have_line(figure: String, items: &[Item], reached: &[(usize, f64)], discounts: bool) -> Have {
    let mut open: Vec<&(usize, f64)> = reached.iter().filter(|(i, _)| items.get(*i).is_some_and(|it| it.state == State::Ready)).collect();
    open.sort_by(|a, b| items[a.0].level.total_cmp(&items[b.0].level));
    let reach: Vec<String> = open.iter().take(2).map(|(i, lv)| format!("{} to Lv {lv}", items[*i].name)).collect();
    let tail = if discounts { " Costs here are before your discounts, so you get a little further." } else { "" };
    Have {
        figure,
        text: match reach.as_slice() {
            [] => "Not enough for the next level of anything here yet.".into(),
            [one] => format!("Enough to take {one}.{tail}"),
            [a, b, ..] => format!("Enough to take {a}, or {b}.{tail}"),
        },
    }
}

/// Spelunking caves: `Spelunk[0][i]` is 1 once that cave's boss is beaten, which gives its lore
/// bonus (N.js `DoWeHaveLoreN1`). Caves are `Spelunky[6]`; "NameNN" rows are not in the game yet.
/// The game says where each boss is: "Lore Boss found at Depth" `Spelunky[7][i]` + 1.
fn spelunk_caves(save: &Save) -> Mechanic {
    let beaten = save.numbers("Spelunk", &[0]);
    let depth = list("Spelunky", 7);
    let lore = list("Spelunky", 14);
    let items = caves()
        .map(|(i, name)| {
            let mut it = found(format!("{name} boss"), pic(format!("CaveBoss{i}")), at(&beaten, i) >= 1.0);
            if let Some(d) = depth.get(i).and_then(|d| d.parse::<f64>().ok()).filter(|_| !it.done()) {
                let gives = lore.get(i).map(|l| clean(&l.replace("!_@_", ";_"))).filter(|l| !l.is_empty());
                it.how = Some(match gives {
                    Some(g) => format!("At depth {}. Beating it: {g}.", d + 1.0),
                    None => format!("The lore boss is at depth {} of {name}.", d + 1.0),
                });
            }
            it
        })
        .collect();
    let mut m = mech("spelunk-caves", 7, "Spelunking Cave Bosses", pic("CaveBoss0".into()), Count::Done, "beaten", items);
    m.note = Some("Each lore boss beaten unlocks the new content its cave's lore names, in another part of the game.".into());
    m
}

/// The caves that exist: index and name.
fn caves() -> impl Iterator<Item = (usize, String)> {
    list("Spelunky", 6).into_iter().enumerate().filter(|(_, n)| !n.starts_with("Name") && !n.is_empty()).map(|(i, n)| (i, label(n)))
}

/// Lore chapter bonuses: `Spelunk[8][chapter * 4 + k]` is the level of bonus k of a chapter
/// (`SpelunkChapters[chapter][k]`). Levels come from reading that chapter's pages
/// (`Spelunking{c}`, "Hold down to boost a random Lore Bonus") and have no cap; the bonus closes in
/// on its max (N.js `ChapterBonus`, "decay"), so this is Open.
fn spelunk_chapters(save: &Save) -> Mechanic {
    let levels = save.numbers("Spelunk", &[8]);
    let mut items = Vec::new();
    for c in 0..GAME.rows("SpelunkChapters").len().min(20) {
        let page = format!("Spelunking{c}");
        let title = GAME.item_name(&page).and_then(|n| n.split('\'').nth(1)).map(str::to_string).unwrap_or_else(|| format!("Chapter {}", c + 1));
        let pages = chest(save, &page);
        for (k, bonus) in rows("SpelunkChapters", &[c]).iter().enumerate() {
            let mut it = Item::new(format!("{title}: {}", clean(text(bonus, 0))), pic(page.clone()), at(&levels, c * 4 + k), 0.0);
            it.how = Some(if pages >= 1.0 {
                format!("Use '{title}' pages to level it. You have {} in storage.", big(pages))
            } else {
                format!("Use '{title}' pages to level it.")
            });
            items.push(it);
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
        let mut it = Item::new(name.clone(), pic(format!("CaveDoor{i}")), at(&normal, i), 0.0);
        it.how = Some(format!("Your deepest delve in {name}. Depths from every cave add up to Total Depths, a Menagerie mission."));
        items.push(it);
        let mut it = Item::new(format!("{name} (Manic)"), pic(format!("CaveDoor{i}")), at(&manic, i), 0.0);
        it.how = Some(format!("Your deepest Manic Mode delve in {name}. Manic Mode comes from the Spelunking Shop, and these depths count for Total Depths too."));
        items.push(it);
    }
    mech("spelunk-depth", 7, "Spelunking Depths", pic("CaveDoor0".into()), Count::Open, "depth", items)
}

/// Advice Fish: `Spelunk[11][i]`, upgrades in `Spelunky[18]` ("Name,text,max bonus,cost,"). No
/// level cap: N.js `BigFishBonuses` gives max × level / (level + 100). N.js `BigFishCost`:
/// 1.15^level × 10.01^col 3 coins, paid from the active character's money. Upgrade i opens
/// (N.js `isBigFishUpgUnlocked`) at once for the first, for the rest once the Class LVs of all
/// characters add up to more than `BigFishUpgLVREQ` = 4000 + (300 + 50 × max(0, i - 2)) × i.
/// Pictures: the fish is `W7_fish`; the seven advice icons are drawn into its sign
/// (`W7_fishUI`), so the items have none of their own.
fn advice_fish(save: &Save) -> Mechanic {
    let levels = save.numbers("Spelunk", &[11]);
    // The character you play pays, and can take coins out of the storage bank first.
    let coins = super::w1::spendable(save);
    let account: f64 = (0..save.characters()).map(|c| at(&save.numbers(&format!("Lv0_{c}"), &[]), 0)).filter(|v| v.is_finite()).map(|v| v.max(0.0)).sum();
    let items = list("Spelunky", 18)
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            let mut parts = r.split(',');
            let mut it = Item::new(label(parts.next().unwrap_or("")), None, at(&levels, i), 0.0);
            let f = i as f64;
            let need = 4000.0 + (300.0 + 50.0 * (f - 2.0).max(0.0)) * f;
            if i > 0 && account <= need {
                it.how = Some(format!("Needs an Account LV over {} (yours is {}).", whole(need), whole(account)));
                it.state = State::Locked;
            } else if let Some(coef) = parts.nth(2).and_then(|c| c.parse::<f64>().ok()) {
                let cost = 1.15f64.powf(it.level) * 10.01f64.powf(coef);
                if cost.is_finite() {
                    it.how = Some(format!("Next level costs {} coins.", big(cost.ceil())));
                    if cost <= coins {
                        it.state = State::Ready;
                    }
                }
            }
            it
        })
        .collect();
    let mut m = mech("advice-fish", 7, "Advice Fish", pic("W7_fish".into()), Count::Open, "levels", items);
    m.note = Some("No level cap. Level 100 gives half of each bonus, level 900 gives 90%. Paid from the coins of the character you play, which the storage bank can top up.".into());
    m
}

/// Clam Work compensations: one more each Worker Class level, `OptLacc[464]`. The twelve are
/// `Spelunky[30]`; the game shows compensation i once the level is above i. Promotions cost
/// pearls and can fail (see below), so nothing here is marked Ready. The game lists them as text,
/// so they have no pictures.
fn clam_work(save: &Save) -> Mechanic {
    let level = save.option(464);
    let items = list("Spelunky", 30)
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            let mut it = found(clean(r), None, level > i as f64);
            if !it.done() {
                it.how = Some(format!("Comes at Worker Class {}.", i + 1));
                if (i as f64 - level).abs() < 0.5 {
                    // N.js: a promotion try costs `Clam_Cost`(9) = 1e5 × 10^W pearls and works with
                    // chance `ClamPromochance` = 0.5 / (2 + W).
                    let w = level.clamp(0.0, 1000.0);
                    it.how = Some(format!("A promotion try costs {} pearls and works 1 time in {}.", big(1e5 * 10f64.powf(w)), big(4.0 + 2.0 * w)));
                }
            }
            it
        })
        .collect();
    mech("clam-work", 7, "Clam Work Compensations", pic("Clam".into()), Count::Done, "earned", items)
}

/// The Clamworks pearl upgrades, `OptLacc[455 + i]`, and the Musselini page, `Menagerie[5][i]`
/// (Menagerie unlock 4). Names come from the page's own text (tools/idleon-extract/extra_w7.py).
/// No caps, except that Clam Comrades stops adding clams at 25 (N.js `Clam_Mobs`: min(25, 2 +
/// level)).
///
/// Pearl page (N.js `Clam_Cost`, before discounts, paid with pearls `OptLacc[454]`): slot 0 costs
/// `Spelunky[29][0]`^L + 3L + L^2.5; slot i costs `Clam_PearlUpgReq`(i) / 5 ×
/// `Spelunky[29][i]`^L + 2L + L^1.5, where `Clam_PearlUpgReq`(i) = 20 × (10 + 3W)^(i-1) and W is
/// the Worker Class (`OptLacc[464]`). Slot i opens once `OptLacc[465]` ≥ i, which happens when
/// you hold `Clam_PearlUpgReq`(i) pearls. Musselini page: each level costs 1 Scurge point; points
/// are the Scurge LV (`Menagerie[0][8]`) minus levels bought (`ScurgePTS_Left`), and slot i needs
/// Scurge LV 0, 5, 10, 25, 50, 100, 250, 500, 1000 (`ScurgeUpg_LvREQ`).
fn clam_upgrades(save: &Save) -> Mechanic {
    let names: Vec<&str> = GAME.extra("w7").get("clam_upgrades").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let mussel = save.numbers("Menagerie", &[5]);
    let page2 = menagerie_unlock(save, 4);
    let pearls = save.option(454);
    let pearls = if pearls.is_finite() { pearls.max(0.0) } else { 0.0 };
    let opened = save.option(465);
    let worker = save.option(464).clamp(0.0, 1000.0);
    let scurge = held(save, "Menagerie", &[0, 8]);
    let spent: f64 = mussel.iter().take(9).filter(|v| v.is_finite()).sum();
    let points = scurge - spent;
    const SCURGE_REQ: [f64; 9] = [0.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0];
    let scale = list("Spelunky", 29);
    let req = |i: usize| 20.0 * (10.0 + 3.0 * worker).powf(i as f64 - 1.0);
    let items: Vec<Item> = (0..18)
        .map(|i| {
            let name = names.get(i).map(|n| label(n)).unwrap_or_else(|| format!("Upgrade {}", i + 1));
            let level = if i < 9 { save.option(455 + i) } else { at(&mussel, i - 9) };
            let mut it = Item::new(name, pic(format!("ClamPearl{}", usize::from(i >= 9))), level, 0.0);
            if i < 9 {
                if i > 0 && (i as f64) > opened {
                    // N.js sets `OptLacc[465]` to the slot while the page is open and you hold
                    // its pearls, so holding them is all it takes.
                    if pearls >= req(i) {
                        it.how = Some(format!("You hold the {} pearls it needs: open the Clamworks page to unlock it.", big(req(i).ceil())));
                        it.state = State::Ready;
                    } else {
                        it.how = Some(format!("Opens once you hold {} pearls.", big(req(i).ceil())));
                        it.state = State::Locked;
                    }
                } else if i == 1 && it.level >= 23.0 {
                    it.how = Some("25 clams, the most the map holds. More levels add nothing.".into());
                } else {
                    let k = scale.get(i).and_then(|x| x.parse::<f64>().ok()).unwrap_or(1.0);
                    let l = it.level;
                    let cost = if i == 0 { k.powf(l) + 3.0 * l + l.powf(2.5) } else { req(i) / 5.0 * k.powf(l) + 2.0 * l + l.powf(1.5) };
                    pay(&mut it, cost, pearls, "pearls", true);
                    if i == 1 && it.how.is_some() {
                        it.how = it.how.map(|h| format!("{h} Each level adds a clam, up to 25 at level 23."));
                    }
                }
            } else if !page2 && it.level <= 0.0 {
                it.how = Some(format!("The Musselini page comes with {}.", menagerie_unlock_name(4)));
                it.state = State::Locked;
            } else if scurge < SCURGE_REQ[i - 9] {
                it.how = Some(format!("Needs Scurge LV {}.", SCURGE_REQ[i - 9]));
                it.state = State::Locked;
            } else {
                it.how = Some("Each level costs 1 Scurge point; you get one per Scurge LV.".into());
                if points >= 1.0 {
                    it.state = State::Ready;
                }
            }
            it
        })
        .collect();
    let mut m = mech("clam-upgrades", 7, "Clamworks Upgrades", pic("ClamPearl0".into()), Count::Open, "levels", items);
    m.have = Some(Have {
        figure: format!("{} pearls", big(pearls)),
        text: if page2 { format!("And {} Scurge points left for the Musselini page.", big(points.max(0.0))) } else { "For the pearl page.".into() },
    });
    m.note = Some(format!("No caps. The second nine are the Musselini page, opened by {}. Pearl costs are before your discounts.", menagerie_unlock_name(4)));
    m
}

/// Coral Kid: `OptLacc[427 + i]` for the first six, `OptLacc[616 + i - 6]` for the Singularity
/// page (Menagerie unlock 2). Texts in `Spelunky[25]`. N.js `CoralKidUpgCost`, no cap: 1e7 × 6^i
/// × 1.25^level for upgrade 2, 1e7 × 6^i × 1.1^level for the rest of page one, paid with Divinity
/// PTS (`Divinity[24]`); on page two (j = i - 6) (5 + 2j)^j × 1.85^level for the last two and
/// (7 + 2j)^j × 1.13^level otherwise, paid with Singularity Spirals (item `CKsing`).
/// The upgrades are text rows, each with the same up-arrow button (`CoralDivUpg`), so they have no
/// pictures of their own.
/// Before any of this the Kid has to be unlocked (`OptLacc[426]`): N.js takes 20M Divinity PTS a
/// try and unlocks him 1 time in 10.
/// The total Divinity LV each Coral Kid upgrade opens at, as N.js sets `_GenINFO[118]`.
const CK_DIVINITY: [u32; 12] = [0, 600, 900, 1200, 1500, 1750, 500, 3000, 5000, 8000, 10000, 12000];

fn coral_kid(save: &Save) -> Mechanic {
    let div = held(save, "Divinity", &[24]);
    let spirals = chest(save, "CKsing");
    let page2 = menagerie_unlock(save, 2);
    let met = save.option(426) >= 1.0;
    let cost = |i: usize, lv: f64| {
        let f = i as f64;
        match i {
            1 => 1e7 * 6f64.powf(f) * 1.25f64.powf(lv),
            0..=5 => 1e7 * 6f64.powf(f) * 1.1f64.powf(lv),
            10 | 11 => (5.0 + 2.0 * (f - 6.0)).powf(f - 6.0) * 1.85f64.powf(lv),
            _ => (7.0 + 2.0 * (f - 6.0)).powf(f - 6.0) * 1.13f64.powf(lv),
        }
    };
    let items = list("Spelunky", 25)
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            let mut it = Item::new(first_sentence(r.split(",_").next().unwrap_or(r)), None, save.option(if i < 6 { 427 + i } else { 616 + i - 6 }), 0.0);
            if !met {
                it.how = Some("Unlock the Coral Kid first: each try costs 20M Divinity PTS and works 1 time in 10.".into());
                it.state = State::Locked;
            } else if i >= 6 && !page2 {
                it.how = Some(format!("The Singularity page comes with {}.", menagerie_unlock_name(2)));
                it.state = State::Locked;
            } else {
                let c = cost(i, it.level);
                pay(&mut it, c, if i < 6 { div } else { spirals }, if i < 6 { "Divinity PTS" } else { "Singularity Spirals" }, false);
                // A new one also needs enough total Divinity levels across your characters (N.js
                // `_GenINFO[118]`), which isn't read here.
                if it.level < 1.0 && i != 0 {
                    it.state = State::Unknown;
                    if let Some(need) = CK_DIVINITY.get(i).filter(|&&n| n > 0) {
                        it.how = it.how.map(|h| format!("{h} Opens at {need} total Divinity LV across your characters."));
                    }
                }
            }
            it
        })
        .collect();
    let mut m = mech("coral-kid", 7, "Coral Kid", pic("CoralDivUpg".into()), Count::Open, "levels", items);
    m.have = Some(Have {
        figure: format!("{} Divinity PTS", big(div)),
        text: match (met, page2) {
            (false, _) if div >= 2e7 => "Enough for a try at unlocking the Coral Kid (20M each).".into(),
            (false, _) => "Not enough for a try at unlocking the Coral Kid yet (20M each).".into(),
            (true, true) => format!("And {} Singularity Spirals in storage for the second page.", big(spirals)),
            (true, false) => "For the first six upgrades.".into(),
        },
    });
    m.note = Some("No caps. Each upgrade opens at a total Divinity LV across your characters: 0, 600, 900, 1,200, 1,500 and 1,750 on page one, 500, 3,000, 5,000, 8,000, 10,000 and 12,000 on the Singularity page.".into());
    m
}

/// Dancing Coral: `Spelunk[4][6]` is how many of its bonuses are bought, one at a time in order.
/// Bonus i is `Spelunky[23][i]` and also raises shrine i (`ShrineInfo`) past level 200. The
/// game's last two rows read "who knows" and are left out. N.js `DancingCoralCOST`: `Spelunky[22]
/// [i]` / (1 + (10L + 1.05^L) / 100), L = `Spelunk[4][7]`, rounded up. The first is paid with Reef
/// Coral (`Spelunk[4][5]`), the rest with coral item `Coral{i}`; names and sources are the page's
/// own text (extra_w7.py).
fn dancing_coral(save: &Save) -> Mechanic {
    let bought = held(save, "Spelunk", &[4, 6]);
    let cheaper = held(save, "Spelunk", &[4, 7]).min(1000.0);
    let shrines = rows("ShrineInfo", &[]);
    let costs = list("Spelunky", 22);
    let extra = GAME.extra("w7");
    let texts = |k: &str| -> Vec<String> { extra.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(words).collect()).unwrap_or_default() };
    let (paid, from) = (texts("dancing_coral_paid"), texts("dancing_coral_from"));
    let items = list("Spelunky", 23)
        .into_iter()
        .enumerate()
        .filter(|(_, r)| !r.contains("who_knows"))
        .map(|(i, r)| {
            let shrine = shrines.get(i).map(|s| label(text(s, 0))).unwrap_or_default();
            let mut it = found(format!("{} ({shrine})", clean(r)), pic(format!("Coral{i}")), bought > i as f64);
            if it.done() {
                return it;
            }
            if (i as f64) > bought {
                it.how = Some("Buy the bonus before it first.".into());
                it.state = State::Locked;
                return it;
            }
            let base = costs.get(i).and_then(|c| c.parse::<f64>().ok()).unwrap_or(0.0);
            let cost = (base / (1.0 + (10.0 * cheaper + 1.05f64.powf(cheaper)) / 100.0)).ceil();
            let have = if i == 0 { held(save, "Spelunk", &[4, 5]) } else { chest(save, &format!("Coral{i}")) };
            let what = paid.get(i).cloned().unwrap_or_else(|| "coral".into());
            let source = from.get(i).map(|f| {
                let mut c = f.chars();
                // "Generated Daily in Town" reads as "generated daily in town"; monster names keep
                // their capitals ("dropped by Shellslugs").
                if f.starts_with("Generated") {
                    return format!(" ({})", f.to_lowercase());
                }
                c.next().map(|x| format!(" ({}{})", x.to_lowercase(), c.as_str())).unwrap_or_default()
            }).unwrap_or_default();
            it.how = Some(format!("Costs {} {what}{source}.", big(cost)));
            if cost <= have {
                it.state = State::Ready;
            }
            it
        })
        .collect();
    mech("dancing-coral", 7, "Dancing Coral", pic("Coral0".into()), Count::Done, "bought", items)
}

/// Trophy numbers in the Gallery: `Spelunk[16]`, 48 storage slots then the podiums; -1 is empty.
fn gallery_slots(save: &Save) -> Vec<f64> {
    save.numbers("Spelunk", &[16])
}

/// Where a trophy or nametag comes from, when the code gives it plainly (extra_w7.py): a quest
/// reward, a drop-table row, a gem shop pack, a box, the Darts minigame or the dice buff.
/// Where a Gallery item comes from first ("quest", "drop", "pack", ...).
fn gallery_kind(id: &str) -> Option<&'static str> {
    GAME.extra("w7").get("gallery_sources")?.get(id)?.as_array()?.first()?.as_array()?.first()?.as_str()
}

fn gallery_source(id: &str) -> Option<String> {
    let first = GAME.extra("w7").get("gallery_sources")?.get(id)?.as_array()?.first()?.as_array()?;
    let (kind, detail) = (first.first()?.as_str()?, first.get(1)?.as_str()?);
    Some(match kind {
        "quest" => format!("A quest reward from {}.", words(detail.trim_end_matches(|c: char| c.is_ascii_digit()))),
        "drop" if detail.contains("Event") => {
            let event = match detail.split("Event").next().unwrap_or("") {
                "xmas" => "the holiday event",
                "Summer" => "the summer event",
                "cropfall" => "the Cropfall event",
                "luck" => "the luck event",
                _ => "an event",
            };
            format!("A rare drop during {event}.")
        }
        "drop" => "A rare drop from monsters.".into(),
        "pack" => "Comes with a gem shop pack.".into(),
        "open" => format!("Can come out of a {} when you open it.", GAME.item_name(detail).unwrap_or("box")),
        "darts" => "Comes from the Darts minigame.".into(),
        // N.js: the dice buff rolls 1 to 10,000 and drops it on a 1.
        "dice" => "Drops on a roll of 1 from the dice buff, 1 in 10,000.".into(),
        _ => return None,
    })
}

/// The how for a Gallery item not there yet: where it comes from, and a copy in storage (Ready).
fn gallery_how(it: &mut Item, save: &Save, id: &str, more: &str) {
    let held = chest(save, id);
    let mut how = gallery_source(id).unwrap_or_default();
    if !more.is_empty() {
        how = if how.is_empty() { more.to_string() } else { format!("{how} {more}") };
    }
    if held >= 1.0 {
        how = format!("{} in storage: bring it to the Gallery. {how}", big(held)).trim_end().to_string();
        it.state = State::Ready;
    } else if gallery_kind(id) == Some("pack") {
        it.pack_only = true;
    }
    if !how.is_empty() {
        it.how = Some(how);
    }
}

/// Gallery trophies: every `TrophyN` the game has, found once it sits anywhere in the Gallery.
/// "I Made This Game" (Trophy4) is the developer's own and only counts when you have it.
fn gallery_trophies(save: &Save) -> Mechanic {
    let slots = gallery_slots(save);
    let items = (1..100)
        .filter_map(|n| GAME.item_name(&format!("Trophy{n}")).map(|name| (n, name)))
        .filter_map(|(n, name)| {
            let have = slots.contains(&(n as f64));
            (n != 4 || have).then(|| {
                let mut it = found(name, pic(format!("Trophy{n}")), have);
                if !have {
                    gallery_how(&mut it, save, &format!("Trophy{n}"), "");
                }
                it
            })
        })
        .collect();
    let mut m = mech("gallery-trophies", 7, "Gallery Trophies", pic("Trophy1".into()), Count::Done, "in the Gallery", items);
    m.note = Some("A trophy counts once it's in the Gallery, on a podium or in storage.".into());
    m
}

/// Gallery podiums: the slots after the first 48 of `Spelunk[16]`. N.js `PodiumsOwned` is the
/// smaller of 19 and: 1, plus ceil(Gallery coral `Spelunk[13][4]` / 4), Emporium 42, gem shop
/// `GemItemsPurchased[40]`, 2 for the Lunarheim boss `Spelunk[0][5]`, min(2, Deathskull
/// `Sailing[3][33]`) and event shop 26.
/// Pictures: the Gallery draws a podium as `GalleryPod{tier}`, the tier rising with podium
/// upgrades (N.js `PodiumsOwned_Lv2`..`Lv4`: companions, Killroy, legend talents and more, not all
/// in the save), so a podium's own picture can't be told and they have none.
fn gallery_podiums(save: &Save) -> Mechanic {
    let slots = gallery_slots(save);
    let bought = |yes: bool| f64::from(u8::from(yes));
    let owned = (1.0
        + (held(save, "Spelunk", &[13, 4]) / 4.0).ceil()
        + bought(emporium_owned(save, 42))
        + held(save, "GemItemsPurchased", &[40]).floor()
        + 2.0 * bought(held(save, "Spelunk", &[0, 5]) >= 1.0)
        + held(save, "Sailing", &[3, 33]).round().min(2.0)
        + bought(super::general::event_shop_owned(save, 26)))
    .min(19.0);
    let spare = slots.iter().take(48).any(|&v| v >= 1.0);
    let items = (0..19)
        .map(|i| {
            let mut it = found(format!("Podium {}", i + 1), None, at(&slots, 48 + i) >= 1.0);
            if !it.done() {
                if (i as f64) < owned {
                    it.how = Some("Owned and empty: put a trophy on it.".into());
                    if spare {
                        it.state = State::Ready;
                    }
                } else {
                    it.how = Some("Not owned yet. The note lists where podiums come from.".into());
                }
            }
            it
        })
        .collect();
    let mut m = mech("gallery-podiums", 7, "Gallery Podiums", pic("GalleryPodiumA0".into()), Count::Done, "filled", items);
    m.note = Some("19 podiums is the most the game allows. You start with 1, then get +1 per 4 levels of the Gallery coral in the Coral Reef, +2 for beating the Lunarheim lore boss, +1 per Deathskull artifact tier up to +2, +1 per gem shop podium, +1 from the Jade Emporium and +1 from the event shop.".into());
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
            (i != 2 || level >= 1.0).then(|| {
                let mut it = Item::new(name, pic(id.clone()), level.min(5.0), 5.0);
                if level < 5.0 {
                    // N.js adds a level for every copy (not a replica) brought to the Gallery.
                    let more = "Each copy brought to the Gallery adds a level.";
                    if level >= 1.0 {
                        gallery_how(&mut it, save, "", more);
                        if chest(save, &id) >= 1.0 {
                            it.how = Some(format!("{} in storage. {more}", big(chest(save, &id))));
                            it.state = State::Ready;
                        }
                    } else {
                        gallery_how(&mut it, save, &id, more);
                    }
                }
                it
            })
        })
        .collect();
    let mut m = mech("gallery-nametags", 7, "Gallery Nametags", pic("EquipmentNametag1".into()), Count::Levels, "levels", items);
    m.note = Some("A nametag's bonus stops growing at level 5, so levels past 5 aren't counted.".into());
    m
}

/// The Menagerie's rank, `Menagerie[0][2]`, and Lumi, `Menagerie[0][0]`.
fn menagerie_rank_lumi(save: &Save) -> (f64, f64) {
    (held(save, "Menagerie", &[0, 2]).min(1e6), held(save, "Menagerie", &[0, 0]))
}

/// The rank that shows slot `i` when `first` slots come one per rank and the rest one per two
/// ranks (N.js `UpgUnlocked` 1 and 2: min(first, rank) + (rank - first) / 2).
fn rank_for(i: usize, first: usize) -> usize {
    if i < first { i + 1 } else { first + 2 * (i + 1 - first) }
}

fn slots_at(rank: f64, first: f64) -> f64 {
    rank.min(first) + ((rank - first).max(0.0) / 2.0).floor()
}

/// Menagerie unlocks: `Menagerie[2][i]`, texts in `Research` row 56. N.js `MenagerieStuff("Cost",
/// 1)`: (10 + i²) × max(1.6, 2.5 - 0.15 i/4)^i Lumi, no discounts. Slot i shows at the rank
/// `rank_for(i, 7)`.
fn menagerie_unlocks(save: &Save) -> Mechanic {
    let (rank, lumi) = menagerie_rank_lumi(save);
    let open = slots_at(rank, 7.0);
    let mut costs: Vec<f64> = Vec::new();
    // Unlocks your rank shows that you don't own yet, and the rank that shows the next one.
    let (mut shown_left, mut next_rank) = (0, None);
    let items: Vec<Item> = list("Research", 56)
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            let mut it = found(clean(r), pic(format!("MenaIconO{i}")), menagerie_unlock(save, i));
            if !it.done() {
                let f = i as f64;
                let cost = (10.0 + f * f) * (2.5 - 0.15 * f / 4.0).max(1.6).powf(f);
                if (i as f64) >= open {
                    next_rank = next_rank.or(Some(rank_for(i, 7)));
                    it.how = Some(format!("Shows at Menagerie rank {}. Costs {} Lumi.", rank_for(i, 7), big(cost.ceil())));
                    it.state = State::Locked;
                } else {
                    shown_left += 1;
                    it.how = Some(format!("Costs {} Lumi.", big(cost.ceil())));
                    if cost <= lumi {
                        it.state = State::Ready;
                        costs.push(cost);
                    }
                }
            }
            it
        })
        .collect();
    costs.sort_by(f64::total_cmp);
    let mut left = lumi;
    let afford = costs.iter().take_while(|&&c| {
        let ok = c <= left;
        left -= c;
        ok
    }).count();
    let mut m = mech("menagerie-unlocks", 7, "Menagerie Unlocks", pic("MenaIconO0".into()), Count::Done, "owned", items);
    m.have = Some(Have {
        figure: format!("{} Lumi", big(lumi)),
        text: match afford {
            0 if shown_left > 0 => "Not enough for any unlock your rank shows yet.".into(),
            0 => match next_rank {
                Some(r) => format!("You own every unlock your rank shows. The next shows at Menagerie rank {r}."),
                None => "You own every unlock.".into(),
            },
            1 => "Enough for one of the unlocks your rank shows.".into(),
            n => format!("Enough for {n} of the unlocks your rank shows, cheapest first."),
        },
    });
    m.note = Some("Some raise caps elsewhere: +3 max levels on the Pillar coral, +3 on nine Legend Talents, +10 max Cooking LV for every meal, +10 max Shiny LV for every pet, a 17th Atom, a 4th statue flair level (Elegant) and Tiny Cogs up to T11.".into());
    m
}

/// Menagerie boosts: `Menagerie[3][i]`, texts in `Research` row 57. No cap. N.js
/// `MenagerieStuff("Cost", 2)`: 3L + L^1.5 + (5 + i^1.5) × max(1.45, 2.25 - 0.15 i/4)^i × E^L Lumi,
/// with E from `CostExponent` (35 for 8, 10, 12; 22.8 for 2, 5, 17; 3.5 for 6; 2.1 for 0, 1, 7;
/// 1.3 otherwise). Slot i shows at rank `rank_for(i, 6)`.
fn menagerie_boosts(save: &Save) -> Mechanic {
    let levels = save.numbers("Menagerie", &[3]);
    let (rank, lumi) = menagerie_rank_lumi(save);
    let open = slots_at(rank, 6.0);
    let cost = |i: usize, lv: f64| {
        let e: f64 = match i {
            8 | 10 | 12 => 35.0,
            2 | 5 | 17 => 22.8,
            6 => 3.5,
            0 | 1 | 7 => 2.1,
            _ => 1.3,
        };
        let f = i as f64;
        3.0 * lv + lv.powf(1.5) + (5.0 + f.powf(1.5)) * (2.25 - 0.15 * f / 4.0).max(1.45).powf(f) * e.powf(lv)
    };
    let mut reached = Vec::new();
    let items: Vec<Item> = list("Research", 57)
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            let mut it = Item::new(clean(r), pic(format!("MenaIconP{i}")), at(&levels, i), 0.0);
            if (i as f64) >= open {
                it.how = Some(format!("Shows at Menagerie rank {}.", rank_for(i, 6)));
                it.state = State::Locked;
            } else {
                let c = cost(i, it.level);
                pay(&mut it, c, lumi, "Lumi", false);
                reached.push((i, reach(it.level, 0.0, lumi, |lv| cost(i, lv))));
            }
            it
        })
        .collect();
    let mut m = mech("menagerie-boosts", 7, "Menagerie Boosts", pic("MenaIconP0".into()), Count::Open, "levels", items);
    m.have = Some(have_line(format!("{} Lumi", big(lumi)), &m.items, &reached, false));
    m
}

/// Menagerie missions: completions in `Menagerie[1][i]`; what each counts is `Research` row 55.
/// Every completion raises the next goal, with no end. N.js `MenagerieStuff("Cost", 0)`: the goal
/// is `Research[54][i]` + `Research[53][i]` × completions, rounded down; a goal whose text has "$"
/// is shown as 10^goal.
fn menagerie_missions(save: &Save) -> Mechanic {
    let done = save.numbers("Menagerie", &[1]);
    let (inc, base, text_) = (list("Research", 53), list("Research", 54), list("Research", 52));
    let n = |v: &[&str], i: usize| v.get(i).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
    let items = list("Research", 55)
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            let mut it = Item::new(clean(r), pic(format!("MenaIconY{i}")), at(&done, i), 0.0);
            let goal = (n(&base, i) + n(&inc, i) * it.level).floor();
            let shown = if text_.get(i).is_some_and(|t| t.contains('$')) { big(10f64.powf(goal)) } else { big(goal) };
            if goal.is_finite() {
                it.how = Some(format!("Next goal: {shown} {}.", clean(r)));
            }
            it
        })
        .collect();
    mech("menagerie-missions", 7, "Menagerie Missions", pic("MenaIconY0".into()), Count::Open, "completions", items)
}

/// The Silver Jellyfish's two upgrades: `Menagerie[0][9]` (Obol bonuses, N.js `ObolMISCmulti`)
/// stops at 100 and `Menagerie[0][10]` (auto-trash, `ObolAutoTrashRate`) at 190: the game only
/// sells them below those. N.js `SilverJellyCosts`: 2 × (100 + 25L + L³) × 1.065^L and (25 + 10L +
/// L²) × 1.056^L Obol Fragments (`CYObolFragments`). The Silver Jellyfish is Menagerie unlock 10.
/// It is jellyfish 4 in town (N.js: jelly n shows once `SideJellyUnlocked`(n - 1), which for 3 is
/// unlock 10, and jelly 4 draws `ObolMISCmulti`), so its picture is `MenaJelly4`.
fn menagerie_jellyfish(save: &Save) -> Mechanic {
    let silver = || pic("MenaJelly4".into());
    let v = save.numbers("Menagerie", &[0]);
    let frags = held(save, "CYObolFragments", &[]);
    let open = menagerie_unlock(save, 10);
    let cost0 = |l: f64| 2.0 * (100.0 + 25.0 * l + l.powi(3)) * 1.065f64.powf(l);
    let cost1 = |l: f64| (25.0 + 10.0 * l + l * l) * 1.056f64.powf(l);
    let mut items = vec![
        Item::new("Silver Jellyfish: Obol bonuses", silver(), at(&v, 9), 100.0),
        Item::new("Silver Jellyfish: Obol auto-trash", silver(), at(&v, 10), 190.0),
    ];
    let mut reached = Vec::new();
    for (n, it) in items.iter_mut().enumerate() {
        if it.level >= it.max {
            continue;
        }
        if !open {
            it.how = Some(format!("Needs {}.", menagerie_unlock_name(10)));
            it.state = State::Locked;
            continue;
        }
        let f: &dyn Fn(f64) -> f64 = if n == 0 { &cost0 } else { &cost1 };
        pay(it, f(it.level), frags, "Obol Fragments", false);
        reached.push((n, reach(it.level, it.max, frags, f)));
    }
    let mut m = mech("menagerie-jellyfish", 7, "Menagerie Jellyfish", pic("MenaJelly0".into()), Count::Levels, "levels", items);
    if open {
        m.have = Some(have_line(format!("{} Obol Fragments", big(frags)), &m.items, &reached, false));
    }
    m
}

/// N.js `OccurrencesToBeFound` at Research level `lv`: none below 1, only the first until it is
/// found (`first`), then min(43, 5 × floor((lv + 10) / 10) - lv/20 - lv/30 - lv/50), each part
/// rounded down.
fn observations_open(lv: f64, first: bool) -> f64 {
    if lv < 1.0 {
        return 0.0;
    }
    if !first {
        return 1.0;
    }
    (5.0 * ((lv + 10.0) / 10.0).floor() - (lv / 20.0).floor() - (lv / 30.0).floor() - (lv / 50.0).floor()).min(43.0)
}

/// Research observations: `Research[2][i]` is 1 once found. `Occurrences` names them and col 7
/// says where each is. More are on offer as the Research level rises, up to 43.
fn observations(save: &Save) -> Mechanic {
    let have = save.numbers("Research", &[2]);
    let lv = research_level(save);
    let first = at(&have, 0) >= 1.0;
    let open = observations_open(lv, first);
    let table = rows("Occurrences", &[]);
    let first_name = table.first().map(|r| label(text(r, 0))).unwrap_or_default();
    let items = observation_rows()
        .map(|(i, name)| {
            let mut it = found(name, pic(format!("ResObj{i}")), at(&have, i) >= 1.0);
            if !it.done() {
                let place = table.get(i).map(|r| clean(text(r, 7))).unwrap_or_default();
                if (i as f64) >= open {
                    let needed = (1..=400).find(|&l| observations_open(f64::from(l), true) > i as f64);
                    it.how = Some(match needed {
                        Some(_) if !first && (i as f64) < observations_open(lv, true) => format!("Offered once you find {first_name}. {place}."),
                        Some(l) => format!("Offered from Research LV {l}. {place}."),
                        None => format!("{place}."),
                    });
                    it.state = State::Locked;
                } else {
                    it.how = Some(format!("{place}."));
                }
            }
            it
        })
        .collect();
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
    let found = save.numbers("Research", &[2]);
    let items = observation_rows()
        .map(|(i, name)| {
            let mut it = Item::new(name, pic(format!("ResObj{i}")), at(&levels, i), 0.0);
            it.how = Some(if at(&found, i) >= 1.0 { "Put an Optical Monocle on it to give it insight.".into() } else { "Find this observation first, then put an Optical Monocle on it.".into() });
            it
        })
        .collect();
    mech("observation-levels", 7, "Observation Insight", pic("ResObj0".into()), Count::Open, "levels", items)
}

/// Posty Notes: N.js `PostyNotesOwned` is min(14, Research level / 10). Texts in `Research` row 6.
fn posty_notes(save: &Save) -> Mechanic {
    let owned = (research_level(save) / 10.0).floor().min(14.0);
    let items = list("Research", 6)
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            let mut it = found(format!("Posty Note {}: {}", i + 1, first_sentence(r)), pic(format!("ResPosty{i}")), (i as f64) < owned);
            if !it.done() {
                it.how = Some(format!("Comes at Research LV {}.", 10 * (i + 1)));
                it.state = State::Locked;
            }
            it
        })
        .collect();
    let mut m = mech("posty-notes", 7, "Posty Notes", pic("ResPosty0".into()), Count::Done, "unlocked", items);
    m.note = Some("One more every 10 Research levels.".into());
    m
}

/// Glimbo trades: `Research[12][t]` trades for slot t, which pays `Research` row 27's item and
/// raises the max level of vault upgrade `Research` row 26 (`UpgradeVault`). No cap. N.js
/// `Glimbo_Cost`, before discounts: (1 + 2.5n) × `Research[28][t]`^n items for trade n + 1.
/// Glimbo himself is a town sprite, not one of the game's pictures, so the card has none.
fn glimbo(save: &Save) -> Mechanic {
    let trades = save.numbers("Research", &[12]);
    let vault = rows("UpgradeVault", &[]);
    let pay_with = list("Research", 27);
    let scale = list("Research", 28);
    let items = list("Research", 26)
        .into_iter()
        .enumerate()
        .map(|(t, v)| {
            let upgrade = v.parse::<usize>().ok();
            let name = upgrade.and_then(|v| vault.get(v)).map(|r| clean(text(r, 0))).unwrap_or_default();
            let id = pay_with.get(t).copied().unwrap_or("");
            // The row is named for the vault upgrade a trade raises, so it wears that upgrade's
            // picture; the item you pay with is named in the how.
            let mut it = Item::new(name, upgrade.map(|v| format!("VaultUpg{v}")), at(&trades, t), 0.0);
            let k = scale.get(t).and_then(|x| x.parse::<f64>().ok()).unwrap_or(1.0);
            let cost = ((1.0 + 2.5 * it.level) * k.powf(it.level)).floor().max(1.0);
            if cost.is_finite() && !id.is_empty() {
                let item = GAME.item_name(id).map(str::to_string).unwrap_or_else(|| id.to_string());
                it.how = Some(format!("Next trade costs {} {item} before your discounts.", big(cost)));
                if cost <= chest(save, id) {
                    it.state = State::Ready;
                }
            }
            it
        })
        .collect();
    let mut m = mech("glimbo", 7, "Glimbo Trades", None, Count::Open, "trades", items);
    let boosted = list("Research", 29).iter().filter(|v| **v == "1").count();
    let node = rows("ResGridSquares", &[]).get(169).map(|r| label(text(r, 0))).unwrap_or_default();
    m.note = Some(format!("Each trade adds 1 max level to its vault upgrade. On {boosted} of the {} slots, each level of the {node} grid node and the event shop's Glimbo bonus add 1 more per trade. Ready counts only what's in storage.", m.items.len()));
    m
}

/// The Button: `OptLacc[594]` counts presses (N.js `Button_Bonuses`), with no end. The button is a
/// town sprite (animation "ButtonG"), not one of the game's pictures, so it has none.
fn the_button(save: &Save) -> Mechanic {
    let mut press = Item::new("Presses", None, save.option(594), 0.0);
    press.how = Some("Every press counts toward its next bonus.".into());
    let items = vec![press];
    let mut m = mech("the-button", 7, "The Button", None, Count::Open, "presses", items);
    m.note = Some("Every five presses feed the next of its nine bonuses, round and round.".into());
    m
}

/// Research grid: `Research[0][i]`; `ResGridSquares` col 1 is the max. Rows named "Name" are
/// empty squares. Each level costs one Grid Point: N.js `Grid_PTSspent` is the sum of levels.
/// `Grid_PTSearned` is Research LV + LV/10 × (1 + [LV ≥ 60] + node 50) plus companions, sushi,
/// jelly, sailing, tasks and shop bonuses; only the first part is read here, so the points shown
/// are a floor.
/// Pictures: the grid draws every node as the same square (`ResSq{max}_{level}`), so nodes have
/// none; the card has the Research skill's own icon (N.js draws skill `Lv0[1 + t]` as
/// `ClassIcons{42 + t}`; Research is `Lv0[20]`, so `ClassIcons61`).
fn research_grid(save: &Save) -> Mechanic {
    let levels = save.numbers("Research", &[0]);
    let mut items: Vec<Item> = rows("ResGridSquares", &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| text(r, 0) != "Name" && cell(r, 1) > 0.0)
        .map(|(i, r)| Item::new(label(text(r, 0)), None, at(&levels, i), cell(r, 1)))
        .collect();
    for it in items.iter_mut().filter(|it| it.level < it.max) {
        it.how = Some("Each level costs 1 Grid Point.".into());
    }
    let lv = research_level(save);
    let earned = (lv + (lv / 10.0).floor() * (1.0 + (lv / 60.0).floor().min(1.0) + at(&levels, 50))).floor();
    let spent: f64 = levels.iter().filter(|v| v.is_finite()).sum();
    let mut m = mech("research", 7, "Research Grid", pic("ClassIcons61".into()), Count::Levels, "levels", items);
    let free = earned - spent;
    if free >= 1.0 && free.is_finite() {
        m.have = Some(Have {
            figure: format!("{} Grid Points or more", big(free)),
            text: "At least that many levels to place. Bonuses from companions, sushi and shops add more.".into(),
        });
    }
    m
}

/// The upgrades of a shop kept the same way: levels at `key[path][i]`, max in col 1 (0 for a row
/// that isn't bought, 999 and over for no cap). Only capped upgrades count.
fn capped_items(table: &str, levels: &[f64], picture: &str) -> Vec<(usize, Item)> {
    rows(table, &[])
        .iter()
        .enumerate()
        .filter(|(_, r)| cell(r, 1) > 0.0 && cell(r, 1) < 999.0)
        .map(|(i, r)| (i, Item::new(label(text(r, 0)), pic(format!("{picture}{i}")), at(levels, i), cell(r, 1))))
        .collect()
}

/// The display order of a shop (`Research` row 32 for sushi, 44 for jelly): slot k shows
/// upgrade `order[k]`, and the slot number drives the cost.
fn slots(row: usize) -> Vec<usize> {
    list("Research", row).iter().filter_map(|v| v.parse().ok()).collect()
}

/// What sets the research minigame shops apart (Sushi Station, Jelly Operator).
struct Shop {
    key: &'static str,
    name: &'static str,
    table: &'static str,
    picture: &'static str,
    /// The research grid node that opens it (see [`node_missing`]).
    node: usize,
    /// The currency, "Bloodcells".
    what: &'static str,
}

/// The research grid node that opens a W7 minigame, named, when the account has no level in
/// it. N.js: Mr Minehead (129) only opens his menu once `Grid_Bonus(129)` ≥ 1, the W7 quick menu
/// hides Sushi Station (189) and Jelly Operator (187) until those nodes have a level, and each
/// node's text reads "You can now speak to ... to play".
fn node_missing(save: &Save, node: usize) -> Option<String> {
    (held(save, "Research", &[0, node]) < 1.0).then(|| rows("ResGridSquares", &[]).get(node).map(|r| label(text(r, 0))).unwrap_or_else(|| format!("#{node}")))
}

/// Why the next level can't be bought whatever it costs: the shop's node isn't levelled, or
/// Research LV is under what the upgrade's slot needs (N.js `UpgLvREQ`, checked against `Lv0[20]`
/// on every buy).
fn shop_lock(closed: Option<&str>, research: f64, need: f64) -> Option<String> {
    if let Some(node) = closed {
        return Some(format!("Opens with the {node} research node."));
    }
    (research < need).then(|| format!("Needs Research LV {} (yours is {}).", whole(need), whole(research)))
}

/// A research minigame shop's `have` line: what the currency buys, or that nothing can be
/// bought until its node has a level.
fn shop_have(figure: String, items: &[Item], reached: &[(usize, f64)], closed: Option<&str>) -> Have {
    match closed {
        Some(node) => Have { figure, text: format!("Nothing here can be bought until you level the {node} research node.") },
        None => have_line(figure, items, reached, true),
    }
}

/// A slot-ordered shop: the next level's cost, Locked until the shop's node has a level,
/// Research LV reaches the slot's `lv_req` and the slot before has a level (N.js `CanWeBuyUpg`
/// and the buy check), Ready when the currency covers it, and what the currency buys.
fn slot_shop(save: &Save, shop: &Shop, levels: &[f64], order: &[usize], have: f64, lv_req: impl Fn(usize) -> f64, cost: impl Fn(usize, &Value, f64) -> f64) -> Mechanic {
    let data = rows(shop.table, &[]);
    let pairs = capped_items(shop.table, levels, shop.picture);
    let closed = node_missing(save, shop.node);
    let research = research_level(save);
    let mut reached = Vec::new();
    let mut items = Vec::new();
    for (n, (u, mut it)) in pairs.into_iter().enumerate() {
        let slot = order.iter().position(|&o| o == u);
        if it.level < it.max {
            if let Some(why) = shop_lock(closed.as_deref(), research, slot.map_or(0.0, &lv_req)) {
                it.how = Some(why);
                it.state = State::Locked;
            } else if let (Some(k), Some(row)) = (slot, data.get(u)) {
                let before = k.checked_sub(1).and_then(|p| order.get(p)).copied();
                if let Some(b) = before.filter(|&b| at(levels, b) < 1.0) {
                    let prev = data.get(b).map(|r| label(text(r, 0))).unwrap_or_default();
                    it.how = Some(format!("Buy a level of {prev} first."));
                    it.state = State::Locked;
                } else {
                    let c = cost(k, row, it.level);
                    pay(&mut it, c, have, shop.what, true);
                    reached.push((n, reach(it.level, it.max, have, |lv| cost(k, row, lv))));
                }
            }
        }
        items.push(it);
    }
    let mut m = mech(shop.key, 7, shop.name, pic(format!("{}0", shop.picture)), Count::Levels, "levels", items);
    m.have = Some(shop_have(format!("{} {}", big(have), shop.what), &m.items, &reached, closed.as_deref()));
    m.note = Some("Only the upgrades with a cap. The rest level up without end.".into());
    m
}

/// Sushi Station: `Sushi[2][u]`, paid with Bucks (`Sushi[4][3]`). N.js `SushiStuff("UpgCost")`
/// for slot k: max(0.1, col 4 or 1) × (5 + k + max(0, k-1)²) × (1.5 + max(0, k-3)/16)^max(0, k-4)
/// × 1.3^max(0, k-20) × col 2^level, then discounts. `SushiStuff("UpgLvREQ")`: slot k needs
/// Research LV 1 + min(2, k) + min(4, k) + 3k - max(0, k-4) - max(0, k-8) + k/6 + k/17.
fn sushi(save: &Save) -> Mechanic {
    let levels = save.numbers("Sushi", &[2]);
    let cost = |k: usize, r: &Value, lv: f64| {
        let k = k as f64;
        let base = if cell(r, 4) != 0.0 { cell(r, 4).max(0.1) } else { 1.0 };
        base * (5.0 + k + (k - 1.0).max(0.0).powi(2)) * (1.5 + (k - 3.0).max(0.0) / 16.0).powf((k - 4.0).max(0.0)) * 1.3f64.powf((k - 20.0).max(0.0)) * cell(r, 2).powf(lv)
    };
    let lv_req = |k: usize| {
        let k = k as i64;
        (1 + k.min(2) + k.min(4) + 3 * k - (k - 4).max(0) - (k - 8).max(0) + k / 6 + k / 17) as f64
    };
    let shop = Shop { key: "sushi", name: "Sushi Station", table: "SushiUPG", picture: "SushiUpg", node: 189, what: "Bucks" };
    slot_shop(save, &shop, &levels, &slots(32), held(save, "Sushi", &[4, 3]), lv_req, cost)
}

/// Jelly Operator: `Research[17][u]`, paid with Bloodcells (`Research[7][11]`). N.js
/// `JellyOperation("UpgCost")` for slot k: 0 for the first, else max(0.1, col 4 or 1) × (1 + k/7)
/// × (6 + 5k + k²) × (1.4 + max(0, k-3)/30)^max(0, k-4) × 1.3^max(0, k-20) × col 2^level.
/// `JellyOperation("UpgLvREQ")`: slot k needs Research LV 15 + 2k + k/15 - k/11.
fn jelly(save: &Save) -> Mechanic {
    let levels = save.numbers("Research", &[17]);
    let cost = |k: usize, r: &Value, lv: f64| {
        if k == 0 {
            return 0.0;
        }
        let k = k as f64;
        let base = if cell(r, 4) != 0.0 { cell(r, 4).max(0.1) } else { 1.0 };
        base * (1.0 + k / 7.0) * (6.0 + 5.0 * k + k * k) * (1.4 + (k - 3.0).max(0.0) / 30.0).powf((k - 4.0).max(0.0)) * 1.3f64.powf((k - 20.0).max(0.0)) * cell(r, 2).powf(lv)
    };
    let lv_req = |k: usize| (15 + 2 * k + k / 15 - k / 11) as f64;
    let shop = Shop { key: "jelly", name: "Jelly Operator", table: "JellyUPG", picture: "JellyUpg", node: 187, what: "Bloodcells" };
    slot_shop(save, &shop, &levels, &slots(44), held(save, "Research", &[7, 11]), lv_req, cost)
}

/// Minehead: `Research[8][t]`, paid with Minehead Currency (`Research[7][5]`). N.js
/// `Minehead("UpgCost")`: (5 + t + max(0, t-2)^1.3) × 2^max(0, t-4) × A_MineCost^max(0, t-9) ×
/// col 2^level, then discounts. A_MineCost is a server value: IdleonToolbox pastes carry it
/// (`Save::server`); without it, from the 11th upgrade on only the rest of the cost is shown and
/// nothing there is marked Ready. `Minehead("UpgLvREQ")`: upgrade t needs Research LV 1 + 3t +
/// t/3 + t/11, and Mr Minehead talks once node 129 has a level.
fn minehead(save: &Save) -> Mechanic {
    let levels = save.numbers("Research", &[8]);
    let have = held(save, "Research", &[7, 5]);
    let data = rows("MineheadUPG", &[]);
    let closed = node_missing(save, 129);
    let research = research_level(save);
    // IdleonToolbox pastes carry the server values; other sources don't.
    let server = save.server("A_MineCost").map(|v| v.clamp(1.0, 1e6));
    let cost = |t: usize, lv: f64| {
        let r = data.get(t).copied().unwrap_or(&Value::Null);
        let f = t as f64;
        let mut c = (5.0 + f + (f - 2.0).max(0.0).powf(1.3)) * 2f64.powf((f - 4.0).max(0.0)) * cell(r, 2).powf(lv);
        if let Some(k) = server {
            c *= k.powf((f - 9.0).max(0.0));
        }
        c
    };
    let mut reached = Vec::new();
    let items: Vec<Item> = capped_items("MineheadUPG", &levels, "MineUpg")
        .into_iter()
        .enumerate()
        .map(|(n, (t, mut it))| {
            if it.level < it.max {
                let c = cost(t, it.level);
                if let Some(why) = shop_lock(closed.as_deref(), research, (1 + 3 * t + t / 3 + t / 11) as f64) {
                    it.how = Some(why);
                    it.state = State::Locked;
                } else if t <= 9 || server.is_some() {
                    pay(&mut it, c, have, "Minehead Currency", true);
                    reached.push((n, reach(it.level, it.max, have, |lv| cost(t, lv))));
                } else if c.is_finite() {
                    it.how = Some(format!("Next level costs {} Minehead Currency times the server's cost multiplier.", big(c.ceil())));
                }
            }
            it
        })
        .collect();
    let mut m = mech("minehead", 7, "Minehead", pic("MineUpg0".into()), Count::Levels, "levels", items);
    m.have = Some(shop_have(format!("{} Minehead Currency", big(have)), &m.items, &reached, closed.as_deref()));
    m.note = Some("Only the upgrades with a cap. The rest level up without end.".into());
    m
}

/// A bonus text from the game with its number put in: `{` is the number, `}` the multiplier
/// 1 + number/100 ("}x_Drop_Rate" at 10 is "1.1x Drop Rate"). Each `@` line is a sentence of its
/// own, cut at its "!"; notes in brackets and "Current ..." readouts are left out.
fn bonus_text(text: &str, v: f64) -> String {
    let short = |n: f64| {
        let s = format!("{:.2}", n);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    let filled = text.replace('{', &short(v)).replace('}', &short(1.0 + v / 100.0));
    filled
        .split('@')
        .map(|part| clean(part.split('!').next().unwrap_or(part)))
        .filter(|p| !p.is_empty() && !p.starts_with('(') && !p.starts_with("Current"))
        .collect::<Vec<_>>()
        .join(". ")
}

/// A list the game works through in order, one at a time: `done` of them so far. Each item is
/// `names[i]`, with what it gives (`texts[i]` filled with `values[i]`) after `verb`.
fn in_order(done: f64, names: &[String], texts: &[&str], values: &[&str], verb: &str) -> Vec<Item> {
    names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let mut it = found(name.clone(), None, done > i as f64);
            let v = values.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
            if let Some(t) = texts.get(i).map(|t| bonus_text(t, v)).filter(|t| !t.is_empty()) {
                it.how = Some(format!("{verb}: {t}."));
            }
            it
        })
        .collect()
}

/// Minehead opponents: `Research[7][4]` is how many are beaten, in order (N.js
/// `Minehead("BonusQTY")` gives bonus t once it's above t). Opponent t is the head
/// `Research[11][Research[10][t]]` (`Minehead("MineheadID")`); its bonus is `Research[19][t]`
/// with the number `Research[20][t]`, and it counts for the whole game.
fn minehead_opponents(save: &Save) -> Mechanic {
    let ids = list("Research", 10);
    let heads = list("Research", 11);
    let texts = list("Research", 19);
    let names: Vec<String> = (0..texts.len())
        .map(|t| ids.get(t).and_then(|i| i.parse::<usize>().ok()).and_then(|i| heads.get(i)).map(|h| label(h)).unwrap_or_else(|| format!("Opponent {}", t + 1)))
        .collect();
    let items = in_order(held(save, "Research", &[7, 4]), &names, &texts, &list("Research", 20), "Beating it");
    let mut m = mech("minehead-opponents", 7, "Minehead Opponents", pic("MineUpg0".into()), Count::Done, "beaten", items);
    m.note = Some("Opponents come one after another. What each one gives counts for the whole game, not just Minehead.".into());
    m
}

/// Sushi types: N.js `SushiStuff("UniqueSushi")` counts the types from the first whose
/// `Sushi[5][n]` is 0 or more, stopping at the first one never cooked; each type below that count
/// gives its bonus `Research[36][t]` with the number `Research[37][t]` (`SushiStuff("RoG_BonusQTY")`).
/// Names are `Research[30]`.
fn sushi_types(save: &Save) -> Mechanic {
    let cooked = save.numbers("Sushi", &[5]);
    let names: Vec<String> = list("Research", 30).iter().map(|n| label(n)).collect();
    let unique = (0..names.len()).take_while(|&n| cooked.get(n).is_some_and(|v| v.is_finite() && *v >= 0.0)).count();
    let items = in_order(unique as f64, &names, &list("Research", 36), &list("Research", 37), "Cooking it the first time");
    let mut m = mech("sushi-types", 7, "Sushi Types", pic("SushiUpg0".into()), Count::Done, "cooked", items);
    m.note = Some("Each new type of sushi you cook gives a bonus for good. They come in order: a higher tier opens after the one before.".into());
    m
}

/// Jelly Operator obstructions: `Research[7][9]` is how many are cleared, in order (N.js
/// `JellyOperation("RoG_BonusQTY")`). Obstruction t is `Research[45][t]`, its bonus
/// `Research[46][t]` with the number `Research[47][t]`.
fn jelly_obstructions(save: &Save) -> Mechanic {
    let names: Vec<String> = list("Research", 45).iter().map(|n| label(n)).collect();
    let items = in_order(held(save, "Research", &[7, 9]), &names, &list("Research", 46), &list("Research", 47), "Clearing it");
    let mut m = mech("jelly-obstructions", 7, "Jelly Operator Obstructions", pic("JellyUpg0".into()), Count::Done, "cleared", items);
    m.note = Some("A successful operation clears the next obstruction, and its bonus counts for the whole game.".into());
    m
}

/// Coral Reef: `Spelunk[13][i]`, unlocked when `Spelunk[12][i]` is 1; col 1 is the max. Row 3's
/// cap is raised from 12 to 15 by Menagerie unlock 9 (N.js rewrites `CoralReef[3][1]`); the last
/// row is a filler. N.js `ReefCost`: col 2 × col 3^level, paid with Reef Coral (`Spelunk[4][5]`).
fn coral_reef(save: &Save) -> Mechanic {
    let levels = save.numbers("Spelunk", &[13]);
    let open = save.numbers("Spelunk", &[12]);
    let have = held(save, "Spelunk", &[4, 5]);
    let letters = ['z', 'A', 'B', 'C', 'D'];
    let table = rows("CoralReef", &[]);
    let cost = |r: &Value, lv: f64| cell(r, 2) * cell(r, 3).powf(lv);
    let mut reached = Vec::new();
    let items: Vec<Item> = table
        .iter()
        .enumerate()
        .take(5)
        .map(|(i, r)| {
            let max = if i == 3 { 15.0 } else { cell(r, 1) };
            let level = at(&levels, i);
            let mut it = Item::new(clean(text(r, 0)), pic(format!("Reef{}1", letters[i])), level, max);
            if level <= 0.0 && at(&open, i) < 1.0 {
                it.how = Some("Not unlocked in the Coral Reef yet.".into());
                it.state = State::Locked;
            } else if level < max {
                if i == 3 && level >= 12.0 && !menagerie_unlock(save, 9) {
                    it.how = Some(format!("Levels 13 to 15 need {}.", menagerie_unlock_name(9)));
                    it.state = State::Locked;
                } else {
                    pay(&mut it, cost(r, level), have, "Reef Coral", false);
                    reached.push((i, reach(level, max, have, |lv| cost(r, lv))));
                }
            }
            it
        })
        .collect();
    let mut m = mech("coral-reef", 7, "Coral Reef", pic("Reefz1".into()), Count::Levels, "levels", items);
    m.have = Some(have_line(format!("{} Reef Coral", big(have)), &m.items, &reached, false));
    m.raises = vec![Raise { name: capital(&menagerie_unlock_name(9)), amount: "+3 max levels on the Pillar coral, 12 to 15".into(), owned: menagerie_unlock(save, 9) }];
    m
}

/// Zenith Market: `Spelunk[45][i]`; col 3 is the max. N.js `ZenithMarketCost`: level + col 1 ×
/// col 2^level (whole below 1M), paid with Zenith Clusters (`OptLacc[486]`), no discounts. An
/// upgrade can be bought once the one before it has a level.
/// Pictures: the market lists its upgrades as text, with no icons; the card has the Zenith
/// Cluster (`Quest110`), shown in the market's corner.
fn zenith_market(save: &Save) -> Mechanic {
    let levels = save.numbers("Spelunk", &[45]);
    let have = save.option(486).max(0.0);
    let have = if have.is_finite() { have } else { 0.0 };
    let cost = |r: &Value, lv: f64| {
        let c = lv + cell(r, 1) * cell(r, 2).powf(lv);
        if c < 1e6 { c.floor() } else { c }
    };
    let table = rows("ZenithMarket", &[]);
    let mut reached = Vec::new();
    let items: Vec<Item> = table
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut it = Item::new(label(text(r, 0)), None, at(&levels, i), cell(r, 3));
            if it.level < it.max {
                // N.js buys a level only once the upgrade before has one.
                if i > 0 && at(&levels, i - 1) < 1.0 {
                    let prev = table.get(i - 1).map(|p| label(text(p, 0))).unwrap_or_default();
                    it.how = Some(format!("Buy a level of {prev} first."));
                    it.state = State::Locked;
                } else {
                    let c = cost(r, it.level);
                    pay(&mut it, c, have, "Zenith Clusters", false);
                    reached.push((i, reach(it.level, it.max, have, |lv| cost(r, lv))));
                }
            }
            it
        })
        .collect();
    let mut m = mech("zenith", 7, "Zenith Market", pic("Quest110".into()), Count::Levels, "levels", items);
    m.have = Some(have_line(format!("{} Zenith Clusters", big(have)), &m.items, &reached, false));
    m
}

/// Legend Talents: `Spelunk[18][i]`. The max (N.js `LegendTalent_MaxLV`) is col 1, plus the level
/// of four research nodes (each for five talents), plus 3 for nine talents with Menagerie unlock
/// 19. The max here counts each node at its own max and the unlock as owned; a talent at the cap
/// the game allows today (the nodes' placed levels, the unlock only when owned) is Locked and says
/// what lifts it. Talents 40 and 41 are named at runtime and only show with Menagerie unlock 14.
fn legend_talents(save: &Save) -> Mechanic {
    let levels = save.numbers("Spelunk", &[18]);
    let grid = rows("ResGridSquares", &[]);
    let grid_levels = save.numbers("Research", &[0]);
    let node_max = |n: usize| grid.get(n).map_or(0.0, |r| cell(r, 1));
    let node_placed = |n: usize| at(&grid_levels, n).clamp(0.0, node_max(n));
    let node_name = |n: usize| grid.get(n).map(|r| label(text(r, 0))).unwrap_or_default();
    let raises: [(usize, [usize; 5]); 4] = [(130, [2, 13, 3, 18, 5]), (131, [9, 23, 33, 1, 16]), (132, [20, 31, 19, 27, 8]), (152, [34, 30, 29, 6, 26])];
    let menagerie = [35, 33, 36, 0, 20, 31, 15, 32, 6];
    let unlock19 = menagerie_unlock(save, 19);
    // The talent shown in each icon slot: `Spelunky[26][k]` is the talent index.
    let order = rows("Spelunky", &[26]);
    let icon_for = |i: usize| order.iter().position(|v| num(v) as usize == i).map(|k| format!("LegendTalentIcon{k}"));
    let runtime_names = ["Minehead My Beloved", "Two Time Jelly Operation"];
    let items = rows("LegendTalents", &[])
        .iter()
        .enumerate()
        .take(42)
        .map(|(i, r)| {
            // `max` is the most it can ever reach, `cap` what the game allows now, `lifts` what
            // closes the gap.
            let (mut max, mut cap) = (cell(r, 1), cell(r, 1));
            let mut lifts = Vec::new();
            for (node, talents) in &raises {
                if talents.contains(&i) {
                    let (placed, most) = (node_placed(*node), node_max(*node));
                    max += most;
                    cap += placed;
                    if placed < most {
                        lifts.push(format!("each level of the {} research node adds 1 ({placed} of {most} placed)", node_name(*node)));
                    }
                }
            }
            if menagerie.contains(&i) {
                max += 3.0;
                if unlock19 {
                    cap += 3.0;
                } else {
                    lifts.push(format!("{} adds 3", menagerie_unlock_name(19)));
                }
            }
            let name = if i >= 40 { runtime_names[i - 40].to_string() } else { label(text(r, 0)) };
            let mut it = Item::new(name, icon_for(i), at(&levels, i), max);
            if i >= 40 && it.level <= 0.0 && !menagerie_unlock(save, 14) {
                it.how = Some(format!("Added by {}.", menagerie_unlock_name(14)));
                it.state = State::Locked;
            } else if it.level < it.max && it.level >= cap && !lifts.is_empty() {
                it.how = Some(format!("At its max of {} for now: {}.", whole(cap), lifts.join(", and ")));
                it.state = State::Locked;
            } else if it.level < it.max {
                // N.js levels a talent by 1 while available points are above 0 and it's under
                // `LegendTalent_MaxLV`: one point a level.
                it.how = Some("Each level costs 1 Legend Talent PTS.".into());
            }
            it
        })
        .collect();
    let mut m = mech("legend-talents", 7, "Legend Talents", pic("LegendTalentIcon0".into()), Count::Levels, "levels", items);
    let talent_names = |ids: &[usize]| -> String {
        let names: Vec<String> = ids.iter().filter_map(|&t| m.items.get(t).map(|it| it.name.clone())).collect();
        names.join(", ")
    };
    let mut raised: Vec<Raise> = raises
        .iter()
        .map(|(n, talents)| {
            let name = node_name(*n);
            let (have, max) = (node_placed(*n), node_max(*n));
            Raise {
                name: format!("Research node {name} ({have} of {max} levels)"),
                amount: format!("+1 max level per node level, up to +{max}, on {}", talent_names(talents)),
                owned: have >= 1.0,
            }
        })
        .collect();
    raised.push(Raise { name: capital(&menagerie_unlock_name(19)), amount: format!("+3 max levels on {}", talent_names(&menagerie)), owned: unlock19 });
    raised.push(Raise { name: capital(&menagerie_unlock_name(14)), amount: format!("Adds two talents: {}", runtime_names.join(" and ")), owned: menagerie_unlock(save, 14) });
    m.raises = raised;
    // Each point earned: one per 100 Class LVs past 400 on each character (N.js
    // `LegendPTS_owned`), plus bonuses from Clam Work, sushi, a companion, the gem shop, sailing and
    // the event shop. The next one from levels comes on the character closest to their next 100.
    let next = (0..save.characters().clamp(1, 12))
        .filter_map(|c| {
            let lv = at(&save.numbers(&format!("Lv0_{c}"), &[]), 0);
            let goal = (100.0 * (lv / 100.0).floor() + 100.0).max(500.0);
            (lv.is_finite() && lv > 0.0).then_some((goal - lv, goal, c))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, goal, c)) = next {
        let who = save.names.get(c).cloned().unwrap_or_else(|| format!("character {}", c + 1));
        m.note = Some(format!("Points come from Class LVs: one per 100 past 400 on each character. Your next is at Class LV {goal} on {who}."));
    }
    m
}

/// Spelunking shop: `Spelunk[5][i]` (-1 is not bought yet); `SpelunkUpg` col 3 is the max, 99999
/// for no cap. An upgrade shows once the one in col 6 is bought (N.js `ShopUpgVisible`). N.js
/// `ShopUpgCost`, before discounts: B = (10 + level) × col 1 × 9.5^col 7 × 6.3^col 8, ×1e7 from
/// row 52 and ×1e9 more from row 66; the first purchase costs B, later ones 0.25 × B × col
/// 2^level + level² + 5 × level. Paid with Amber (`Spelunk[4][0]`), which the game shows in the
/// tier of the last contract bought (see [`amber_unit`]); costs and the figure here do the same.
fn spelunk_upgrades(save: &Save) -> Mechanic {
    let levels = save.numbers("Spelunk", &[5]);
    let have = held(save, "Spelunk", &[4, 0]);
    let (per, amber) = amber_unit(&levels);
    let table = rows("SpelunkUpg", &[]);
    let cost = |t: usize, r: &Value, lv: f64| {
        let mut b = (10.0 + lv) * cell(r, 1) * 9.5f64.powf(cell(r, 7)) * 6.3f64.powf(cell(r, 8));
        if t >= 52 {
            b *= 1e7;
        }
        if t >= 66 {
            b *= 1e9;
        }
        if lv < 0.0 { b } else { 0.25 * b * cell(r, 2).powf(lv) + lv * lv + 5.0 * lv }
    };
    let mut reached = Vec::new();
    let mut items = Vec::new();
    for (i, r) in table.iter().enumerate().filter(|(_, r)| cell(r, 3) > 0.0 && cell(r, 3) < 999.0) {
        let raw = at(&levels, i);
        let mut it = Item::new(label(text(r, 0)), pic(format!("CaveShopUpg{i}")), raw.max(0.0), cell(r, 3));
        let before = cell(r, 6) as usize;
        if it.level < it.max {
            if i != 0 && at(&levels, before) < 0.0 {
                let prev = table.get(before).map(|p| label(text(p, 0))).unwrap_or_default();
                it.how = Some(format!("Shows once you buy {prev}."));
                it.state = State::Locked;
            } else {
                pay(&mut it, cost(i, r, raw.max(-1.0)) / per, have / per, amber, true);
                if raw >= 0.0 {
                    reached.push((items.len(), reach(raw, it.max, have, |lv| cost(i, r, lv))));
                }
            }
        }
        items.push(it);
    }
    let mut m = mech("spelunk-shop", 7, "Spelunking Shop", pic("CaveShopUpg0".into()), Count::Levels, "levels", items);
    m.have = Some(have_line(format!("{} {amber}", big(have / per)), &m.items, &reached, true));
    m.note = Some("Only the upgrades with a cap. The rest level up without end.".into());
    m
}

/// How the game counts amber: N.js `AmberDenominator` divides every amount by 1e36, 1e21, 1e9 or
/// 1e3 once the Pink, Red, Green or Blue Amber contract (shop rows 66, 51, 41, 20) has a level,
/// and colours it to match. Before any contract it's plain amber.
fn amber_unit(levels: &[f64]) -> (f64, &'static str) {
    [(66, 1e36, "Pink Amber"), (51, 1e21, "Red Amber"), (41, 1e9, "Green Amber"), (20, 1e3, "Blue Amber")]
        .into_iter()
        .find(|(row, ..)| at(levels, *row) >= 1.0)
        .map_or((1.0, "Amber"), |(_, per, name)| (per, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Save {
        Save::parse(text).unwrap()
    }

    /// A list of `n` zeros with `set` filled in, as save JSON.
    fn list_of(n: usize, set: &[(usize, &str)]) -> String {
        let mut v = vec!["0".to_string(); n];
        for &(i, x) in set {
            v[i] = x.to_string();
        }
        format!("[{}]", v.join(","))
    }

    #[test]
    fn minigame_shops_wait_for_their_node_and_research_level() {
        let closed = jelly(&parse(r#"{"OptLacc": []}"#));
        assert!(closed.items.iter().all(|i| i.state == State::Locked && i.how.as_deref() == Some("Opens with the Jelly Operator Linguistics research node.")));
        assert!(closed.have.as_ref().is_some_and(|h| h.text.starts_with("Nothing here can be bought until you level the Jelly Operator Linguistics")));

        let grid = list_of(240, &[(187, "1")]);
        let lv = list_of(21, &[(20, "20")]);
        let open = jelly(&parse(&format!(r#"{{"OptLacc": [], "Lv0_0": {lv}, "Research": [{grid}]}}"#)));
        // Slot 0 is free and needs Research LV 15; slot 3 needs 15 + 6.
        let order = slots(44);
        let at_slot = |k: usize| open.items.iter().find(|i| i.picture.as_deref() == Some(format!("JellyUpg{}", order[k]).as_str())).unwrap();
        assert_eq!(at_slot(0).state, State::Ready);
        assert_eq!(at_slot(3).state, State::Locked);
        assert_eq!(at_slot(3).how.as_deref(), Some("Needs Research LV 21 (yours is 20)."));
    }

    #[test]
    fn legend_talents_stop_at_the_cap_the_game_allows_today() {
        let spelunk = format!("[{}, [2, 3]]", vec!["[]"; 18].join(","));
        let m = legend_talents(&parse(&format!(r#"{{"OptLacc": [], "Spelunk": {spelunk}}}"#)));
        // Coral Restoration: 2 base, +3 with Menagerie unlock 19, which this account lacks.
        assert_eq!(m.items[0].state, State::Locked);
        assert!(m.items[0].how.as_deref().is_some_and(|h| h.starts_with("At its max of 2 for now: the Menagerie unlock")));
        // Greatest Drop Party Ever: 4 base, raised by a research node with no level placed.
        assert_eq!(m.items[1].how.as_deref(), Some("Each level costs 1 Legend Talent PTS."));
    }

    #[test]
    fn amber_reads_in_the_last_contract_tier() {
        assert_eq!(amber_unit(&[]), (1.0, "Amber"));
        let mut levels = vec![-1.0; 68];
        levels[20] = 4.0;
        assert_eq!(amber_unit(&levels), (1e3, "Blue Amber"));
        levels[51] = 1.0;
        assert_eq!(amber_unit(&levels), (1e21, "Red Amber"));
    }

    #[test]
    fn advice_fish_opens_by_account_level() {
        let lv = list_of(21, &[(0, "4500")]);
        let m = advice_fish(&parse(&format!(r#"{{"OptLacc": [], "Lv0_0": {lv}, "Money_0": 1e60}}"#)));
        assert_eq!(m.items[0].state, State::Ready);
        // Upgrade 1 opens past 4,300, upgrade 2 past 4,600.
        assert_eq!(m.items[1].state, State::Ready);
        assert_eq!(m.items[2].state, State::Locked);
        assert_eq!(m.items[2].how.as_deref(), Some("Needs an Account LV over 4,600 (yours is 4,500)."));
    }

    #[test]
    fn podiums_count_the_emporium_and_event_shop() {
        let ninja = format!("[{}, [0,0,0,0,0,0,0,0,0,\"P\"]]", vec!["[]"; 102].join(","));
        let opt = list_of(312, &[(311, "\"z\"")]);
        let m = gallery_podiums(&parse(&format!(r#"{{"OptLacc": {opt}, "Ninja": {ninja}}}"#)));
        // 1 to start, +1 Emporium 42, +1 event shop 26.
        assert_eq!(m.items[2].how.as_deref(), Some("Owned and empty: put a trophy on it."));
        assert!(m.items[3].how.as_deref().is_some_and(|h| h.starts_with("Not owned yet")));
    }
}
