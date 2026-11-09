//! The Tome (World 4): one score for the whole account, made of 124 lines. Each line reads one
//! number from the account (N.js `_customEvent_TomeQTY` fills `TomeQTY[i]`), and turns it into
//! points with one of five curves against the line's target (`Tome` col 1 target, col 2 curve,
//! col 3 points; N.js `TomePCT`, `TomePTS`). A line counts once the account's total class levels
//! reach its requirement, which rises along the in-game order (`NinjaInfo[32]`, `TomeLvReq`).
//!
//! Only the lines a save can answer on its own are read: [`quantity`] says how, line by line,
//! as the game does. The rest need the game's own sums (talent points, ninja floors, sushi)
//! and are left out of the card, which says how many.

use super::general::card_stars;
use super::*;

/// The game's own ln 10 (N.js `getLOG`), kept as it rounds it so digits count the same.
#[allow(clippy::approx_constant)]
const LN_10: f64 = 2.30259;

/// Digits of a number as the game counts them: log10, 0 below 1.
fn log(x: f64) -> f64 {
    x.max(1.0).ln() / LN_10
}

pub fn all(save: &Save) -> Vec<Mechanic> {
    vec![tome(save)]
}

/// Lines worth `curve`'s share of their points: `q` the account's number, `m` the target.
/// 0 and 4 rise toward a ceiling (0 passes full points, up to 1.45 times), 1 the same on the
/// number's digits, 2 straight to the target, 3 lower is better (a time), nothing past 5x.
fn share(curve: u8, q: f64, m: f64) -> f64 {
    let s = match curve {
        0 if q < 0.0 => 0.0,
        0 => (1.7 * q / (q + m)).powf(0.7),
        1 => 2.4 * log(q) / (2.0 * log(q) + m),
        2 => (q / m).min(1.0),
        3 if q > 5.0 * m => 0.0,
        3 => (1.2 * (6.0 * m - q) / (7.0 * m - q)).powi(5),
        4 => {
            let q = q.min(m);
            (2.0 * q / (q + m)).powf(0.7)
        }
        _ => 0.0,
    };
    if s.is_finite() { s.max(0.0) } else { 0.0 }
}

/// The number where a line gives its full points (share 1).
fn full_at(curve: u8, m: f64) -> f64 {
    match curve {
        0 => m / 0.7,
        1 => 10f64.powf(2.5 * m),
        _ => m,
    }
}

/// Total class levels a line needs, by its place in the in-game order (N.js `TomeLvReq`).
fn level_needed(place: usize) -> f64 {
    let p = place as f64;
    let past = |n: f64| (p - n).max(0.0);
    40.0 * p + 5.0 * past(35.0) + 10.0 * past(60.0) + 10.0 * past(80.0) + 15.0 * past(100.0) + 350.0
}

fn sum(v: &[f64]) -> f64 {
    v.iter().sum()
}

fn max(v: &[f64]) -> f64 {
    v.iter().copied().fold(0.0, f64::max)
}

/// `OptLacc` keeps the account's best for many lines.
fn best(save: &Save, i: usize) -> f64 {
    save.option(i)
}

/// Line `i`'s number, read from the save the way the game reads it; `None` for lines that need
/// the game's own sums.
fn quantity(save: &Save, i: usize) -> Option<f64> {
    let chars = save.characters();
    let n = |key: &str, path: &[usize]| save.numbers(key, path);
    let one = |key: &str, path: &[usize]| save.at(key, path).map(num).unwrap_or(0.0);
    let lv = |c: usize| n(&format!("Lv0_{c}"), &[]);
    let list_len = |key: &str, path: &[usize]| save.at(key, path).and_then(Value::as_array).map_or(0, Vec::len) as f64;
    let found = |prefix: &str| save.get("Cards1").and_then(Value::as_array).map_or(0, |a| a.iter().filter(|v| v.as_str().is_some_and(|s| s.starts_with(prefix))).count()) as f64;
    let q = match i {
        0 => (0..3).map(|k| sum(&n("StampLv", &[k]))).sum(),
        1 => {
            let all: Vec<Vec<f64>> = (0..chars).map(|c| save.at(&format!("StatueLevels_{c}"), &[]).and_then(Value::as_array).map(|a| a.iter().map(|r| crate::save::index(r, 0).map(num).unwrap_or(0.0)).collect()).unwrap_or_default()).collect();
            let len = all.iter().map(Vec::len).max().unwrap_or(0);
            (0..len).map(|s| all.iter().map(|v| at(v, s)).fold(0.0, f64::max)).sum()
        }
        2 => {
            let have = save.get("Cards0");
            GAME.rows("CardStuff")
                .iter()
                .flat_map(|g| g.as_array().into_iter().flatten())
                .filter_map(|r| {
                    let mob = text(r, 0);
                    let count = have.and_then(|h| h.get(mob)).map(num).unwrap_or(0.0);
                    (count > 0.0 && mob != "Blank").then(|| card_stars(count, cell(r, 2), mob == "Boss3B") + 1.0)
                })
                .sum()
        }
        3 => {
            // Each talent's best max level on any character.
            let all: Vec<Vec<f64>> = (0..chars).map(|c| n(&format!("SM_{c}"), &[])).collect();
            let len = all.iter().map(Vec::len).max().unwrap_or(0);
            (0..len).map(|t| all.iter().map(|v| at(v, t)).fold(0.0, f64::max)).sum()
        }
        4 => {
            let mut done = std::collections::HashSet::new();
            for c in 0..chars {
                if let Some(o) = save.get(&format!("QuestComplete_{c}")).and_then(Value::as_object) {
                    done.extend(o.iter().filter(|(_, v)| num(v) == 1.0).map(|(k, _)| k.as_str()));
                }
            }
            done.len() as f64
        }
        5 => (0..chars).map(|c| at(&lv(c), 0)).sum(),
        6 => save.at("TaskZZ1", &[]).and_then(Value::as_array).map_or(0.0, |w| w.iter().map(|r| sum(&crate::save::numbers(r).into_iter().take(8).collect::<Vec<_>>())).sum()),
        7 => n("AchieveReg", &[]).iter().filter(|&&v| v == -1.0).count() as f64,
        8 => best(save, 198),
        9 => best(save, 208),
        10 => found("Trophy"),
        11 => (0..chars).map(|c| lv(c).iter().skip(1).take(21).map(|v| v.max(0.0)).sum::<f64>()).sum(),
        12 => best(save, 201),
        13 => one("TaskZZ0", &[0, 2]),
        14 => best(save, 172),
        16 => {
            let v = best(save, 202);
            if v > 0.0 { 1.0 / v } else { 0.0 }
        }
        18 => best(save, 200),
        19 => save.get("SSprog").and_then(Value::as_array).map_or(0, |a| a.iter().filter(|r| crate::save::index(r, 1).map(num) == Some(1.0)).count()) as f64,
        20 => best(save, 203),
        21 => found("Obol"),
        22 => (0..4).map(|k| sum(&n("CauldronInfo", &[k]))).sum(),
        23 => sum(&n("CauldronInfo", &[4])),
        24 => {
            let v = n("CauldronP2W", &[4]);
            (0..v.len().div_ceil(2)).map(|r| at(&v, 1 + 2 * r) + 1.0).sum()
        }
        25 => best(save, 199),
        26 => ["CYDeliveryBoxComplete", "CYDeliveryBoxStreak", "CYDeliveryBoxMisc"].iter().map(|k| one(k, &[]).round()).sum(),
        27 => best(save, 204),
        28 => best(save, 205),
        29 => best(save, 206),
        30 => 1000.0 - best(save, 207),
        31..=35 => best(save, 211 + (i - 31)),
        36 => best(save, 209),
        37 => sum(&n("TotemInfo", &[0])),
        39 => save.get("WeeklyBoss").and_then(Value::as_object).map_or(0, |o| o.iter().filter(|(k, v)| k.starts_with("d_") && num(v) == -1.0).count()) as f64,
        40 => (3..9).map(|k| one("Refinery", &[k, 1])).sum(),
        41 => sum(&n("Atoms", &[])),
        43 => {
            let order = save.get("ChestOrder").and_then(Value::as_array);
            order.and_then(|o| o.iter().position(|v| v.as_str() == Some("Critter11A"))).map_or(0.0, |p| at(&n("ChestQuantity", &[]), p))
        }
        44 => best(save, 224),
        45 => one("Rift", &[0]),
        46 => ["Pets", "PetsStored"].iter().flat_map(|k| save.get(k).and_then(Value::as_array).into_iter().flatten()).map(|p| crate::save::index(p, 2).map(num).unwrap_or(0.0)).fold(0.0, f64::max),
        47 => 1000.0 - best(save, 220),
        48 => (0..10).map(|r| (6..9).map(|k| one("Cooking", &[r, k])).sum::<f64>()).sum(),
        50 => sum(&n("Meals", &[0])),
        52 => n("Lab", &[15]).iter().map(|v| v.max(0.0)).sum(),
        53 => sum(&n("FamValColosseumHighscores", &[])),
        54 => best(save, 217),
        56 => 1000.0 - best(save, 218),
        57 => save.get("Boats").and_then(Value::as_array).map_or(0.0, |b| b.iter().map(|r| crate::save::index(r, 3).map(num).unwrap_or(0.0) + crate::save::index(r, 5).map(num).unwrap_or(0.0)).sum()),
        58 => (one("Divinity", &[25]) - 10.0).max(0.0),
        59 => one("GamingSprout", &[28, 1]),
        60 => sum(&n("Sailing", &[3])),
        61 => one("Sailing", &[1, 0]),
        62 => (0..20).map(|r| one("Captains", &[r, 3])).fold(0.0, f64::max),
        63 => one("GamingSprout", &[32, 1]).max(best(save, 210)),
        64 => one("Gaming", &[8]),
        65 => list_len("Cards1", &[]),
        66 => one("Gaming", &[0]),
        67 => 2f64.powf(best(save, 219)),
        68 => save.get("FarmCrop").and_then(Value::as_object).map_or(0, |o| o.len()) as f64,
        69 => sum(&n("Ninja", &[104])),
        70 => sum(&n("Summon", &[0])),
        73 => {
            // Familiars in Grey Slime units: each kind is worth 3, 4, 5... times the one before.
            let v = n("Summon", &[4]);
            let (mut total, mut unit) = (0.0, 1.0);
            for r in 0..9 {
                total += unit * at(&v, r);
                unit *= (r + 3) as f64;
            }
            total
        }
        74 => save.at("Ninja", &[102, 9]).and_then(Value::as_str).map_or(0, |s| s.chars().count()) as f64,
        75 => (0..4).map(|k| one("FamValMinigameHiscores", &[k])).sum::<f64>() + best(save, 99),
        76 => sum(&n("PrayOwned", &[])),
        77 => sum(&n("FarmRank", &[0])),
        78 => best(save, 221),
        79 => best(save, 222),
        80 => sum(&n("ArcadeUpg", &[])),
        82 => (65..71).map(|k| one("Holes", &[11, k])).sum(),
        83 => n("Holes", &[9]).iter().map(|v| log(*v).ceil()).sum(),
        84 => n("Holes", &[1]).iter().map(|v| v.max(0.0).round()).sum(),
        85 => best(save, 262),
        86 => best(save, 279),
        87..=89 => one("Holes", &[11, 73 + (i - 87)]),
        90 => best(save, 356),
        91 => one("Holes", &[11, 8]),
        92 => [1, 3, 5, 7].iter().map(|&k| one("Holes", &[11, k]).max(0.0).round()).sum(),
        93 => n("Holes", &[7]).iter().map(|v| v.max(0.0).round()).sum(),
        94 => best(save, 353).min(12.0).round() + 1.0,
        95 => best(save, 369).round(),
        97 => (0..6).map(|k| one("Spelunk", &[13, k])).sum(),
        98 => max(&n("Spelunk", &[1])),
        99 => sum(&n("Ninja", &[103])),
        100 => best(save, 445),
        101 => best(save, 446),
        102 => max(&n("Spelunk", &[2])),
        103 => n("Spelunk", &[5]).iter().map(|v| v.max(0.0)).sum(),
        104 => list_len("Spelunk", &[6]),
        105 => (0..chars).map(|c| at(&lv(c), 19)).fold(0.0, f64::max),
        106 => best(save, 443),
        107 => found("EquipmentNametag"),
        108 => one("Bubba", &[1, 8]),
        109 => list_len("Spelunk", &[46]),
        110 => one("Research", &[7, 4]),
        111 => list_len("Research", &[11]),
        113 => best(save, 498),
        115 => n("Research", &[12]).iter().map(|v| v.max(0.0).round()).sum(),
        117 => best(save, 594),
        118 => n("RoyalG", &[0]).iter().map(|v| v.max(0.0)).sum(),
        121 => one("Research", &[7, 9]),
        _ => return None,
    };
    q.is_finite().then_some(q)
}

/// The line's name as the Tome writes it, without the "tap for more" mark.
fn line_name(raw: &str) -> String {
    label(&raw.replace('膛', "").replace("(Tap_for_more_info)", ""))
}

fn tome(save: &Save) -> Mechanic {
    let lines = rows("Tome", &[]);
    let order: Vec<usize> = rows("NinjaInfo", &[32]).iter().filter_map(|v| v.as_str().and_then(|s| s.parse().ok())).collect();
    let levels: f64 = (0..save.characters()).map(|c| at(&save.numbers(&format!("Lv0_{c}"), &[]), 0)).sum();
    let (mut score, mut left_out) = (0.0, 0);
    let mut items = Vec::new();
    for (place, &i) in order.iter().enumerate() {
        let Some(r) = lines.get(i) else { continue };
        let (target, curve, points) = (cell(r, 1), cell(r, 2) as u8, cell(r, 3));
        let Some(q) = quantity(save, i) else {
            left_out += 1;
            continue;
        };
        let need = level_needed(place);
        let open = levels >= need;
        let pts = if open { (share(curve, q, target) * points).ceil() } else { 0.0 };
        score += pts;
        let mut it = Item::new(line_name(text(r, 0)), None, pts.min(points), points);
        // The Tome is a World 4 building.
        it.world = Some(4);
        it.level_name = Some(format!("{} of {} pts", big(pts), big(points)));
        if !open {
            it.state = State::Locked;
            it.how = Some(format!("Counts once your class levels add up to {}; they're at {} now.", big(need), big(levels)));
        } else if pts < points {
            let full = full_at(curve, target);
            it.how = Some(match curve {
                3 => format!("You're at {}. Full points at {} or lower.", big(q), big(full)),
                _ => format!("You're at {}. Full points at {}.", big(q), big(full)),
            });
        }
        items.push(it);
    }
    let mut m = mech("tome", 0, "The Tome", None, Count::Levels, "points", items);
    m.note = Some(format!(
        "{left_out} of the Tome's lines need sums only the game makes (talent points, ninja floors, sushi and the like), so they're left out. The others are scored as the game scores them."
    ));
    m.have = Some(Have {
        figure: format!("{} points", big(score)),
        text: "From the lines read here, so the game's own total is higher. Past 4,000 points the Tome adds skill efficiency; past 8,000, drop rate.".into(),
    });
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_match_the_game() {
        // Straight to the target.
        assert_eq!(share(2, 50.0, 100.0), 0.5);
        assert_eq!(share(2, 500.0, 100.0), 1.0);
        // Rising: full points at the target / 0.7, and more past it.
        assert!((share(0, 100.0 / 0.7, 100.0) - 1.0).abs() < 1e-9);
        assert!(share(0, 1e9, 100.0) > 1.4);
        // Lower is better: full at the target, nothing past five times it.
        assert!((share(3, 10.0, 10.0) - 1.0).abs() < 1e-9);
        assert_eq!(share(3, 51.0, 10.0), 0.0);
        // Capped at the target.
        assert!((share(4, 2e6, 1e6) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn lines_open_with_class_levels() {
        assert_eq!(level_needed(0), 350.0);
        assert_eq!(level_needed(1), 390.0);
        assert_eq!(level_needed(36), 40.0 * 36.0 + 5.0 + 350.0);
    }

    #[test]
    fn an_achievement_counts_and_scores() {
        let s = Save::parse(r#"{"OptLacc": [0], "Lv0_0": [1000], "AchieveReg": [-1, -1, 0]}"#).unwrap();
        let m = tome(&s);
        let a = m.items.iter().find(|i| i.name == "Total Achievements Completed").unwrap();
        assert!(a.how.as_deref().is_some_and(|h| h.starts_with("You're at 2.")), "{:?}", a.how);
        assert!(a.level > 0.0);
    }
}
