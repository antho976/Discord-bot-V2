//! Writes `crowd.txt` (tiers from other players' saves, see `src/crowd.rs`) from a folder of
//! saves.
//!
//! `cargo run --release -p feat-idleon --example crowd -- ~/saves/*.json`
//!
//! Each save is put at a stage: the world it has reached, then early, mid or late in it by its
//! total class levels against the other saves in that world (the lowest third is early). Only
//! the newest saves of each stage count (by file date), so the tiers follow how people play
//! after updates. A stage needs `MIN_SAVES` saves before it gets a column.
//!
//! For each item and stage, the cell is the level at least half that stage's saves have,
//! rounded down to a number a person would pick, and never lower than the stage before.
//! `-` when fewer than half have started it: not asked for yet.
//!
//! Hand edits are kept. Next to `crowd.txt` it writes `crowd.gen.txt`, exactly what it
//! generated; on the next run, any cell in `crowd.txt` that differs from `crowd.gen.txt` was
//! edited by hand, and stays (its row says `# hand`). Every other cell is rewritten from the
//! saves. `off` after a mechanic's key is kept too. Items the last run didn't have (a game
//! update) say `# new since the last run`, and are listed at the end.

use std::collections::HashMap;
use std::fmt::Write as _;

use feat_idleon::crowd::{self, Block, STAGES};
use feat_idleon::review::{Count, Review};
use feat_idleon::save::Save;

/// Saves a stage needs before it gets a column.
const MIN_SAVES: usize = 5;
/// The newest this many saves of a stage count.
const NEWEST: usize = 300;
/// Mechanics whose every column comes from the AutoReview base (crowd-base.txt), changed there,
/// none from the saves (owner, 2026-11-08: bubbles "with an order like scoli's autoreview but
/// tweaked a bit with the levels").
const BASE_ONLY: [&str; 1] = ["bubbles"];
/// Mechanics tiered on how many items are at a level, any of them, not on each item's own
/// level: an `@any L` row for each level here (see `crowd::any_level`). What you can level in
/// them depends on what you happen to have (a vial's item, the Arcade's shop rotation), so a
/// tier that names items makes you chase them; a count lets you take the cheapest.
const COUNTS: [(&str, &[f64]); 2] = [("vials", &[1.0, 4.0, 7.0, 10.0, 13.0]), ("arcade", &[10.0, 25.0, 50.0, 100.0])];
/// In a `COUNTS` mechanic, the items still asked for by name, to get as soon as you can (owner,
/// 2026-11-08: Gold Ball Gain, Refinery Speed; Drop Rate is the next most maxed in the saves).
/// Any other named row typed into `crowd.txt` by hand stays too.
const FIRST: [(&str, &[&str]); 1] = [("arcade", &["Gold Ball Gain", "Drop Rate", "Refinery Speed"])];

/// Per mechanic key, per item name (interned in `names`): its level.
type Levels = HashMap<&'static str, HashMap<u32, f32>>;

struct Seen {
    stage: usize,
    levels: Levels,
}

fn main() {
    let dir = env!("CARGO_MANIFEST_DIR");
    let path = format!("{dir}/crowd.txt");
    let gen_path = format!("{dir}/crowd.gen.txt");
    let (current, problems) = crowd::parse(&std::fs::read_to_string(&path).unwrap_or_default());
    for p in &problems {
        eprintln!("crowd.txt {p} (left out)");
    }
    let gen_text = std::fs::read_to_string(&gen_path).ok();
    // Early-world base tiers (tools/autoreview-base): a stage without enough saves takes its
    // column from here.
    let (base, problems) = crowd::parse(&std::fs::read_to_string(format!("{dir}/crowd-base.txt")).unwrap_or_default());
    for p in &problems {
        eprintln!("crowd-base.txt {p} (left out)");
    }
    let mut unmatched: Vec<String> = Vec::new();
    let mut fuzzy: Vec<String> = Vec::new();
    let (last_gen, _) = crowd::parse(gen_text.as_deref().unwrap_or(""));

    // Every item as a World 7 account sees it, for the row order (the game's order).
    let empty = Save::parse(r#"{"OptLacc": []}"#).expect("an empty save");
    let full = Review::new(&empty, feat_idleon::run_checks(&empty), 7);

    // Read every save once, keeping only levels.
    let mut names: Vec<String> = Vec::new();
    let mut ids: HashMap<String, u32> = HashMap::new();
    let mut read: Vec<(u8, f64, std::time::SystemTime, Levels)> = Vec::new();
    let mut skipped = 0;
    // Items some save has greenstacked or holds 1M of, for greenstacks.txt.
    let mut stacked: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for p in std::env::args().skip(1) {
        let Ok(text) = std::fs::read_to_string(&p) else { skipped += 1; continue };
        let Ok(save) = Save::parse(&text) else { skipped += 1; continue };
        let list = |key: &str| save.get(key).and_then(|v| v.as_array()).map(|a| a.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect::<Vec<_>>()).unwrap_or_default();
        stacked.extend(list("GreenStacks").into_iter().filter(|id| !id.is_empty()));
        let qty = save.numbers("ChestQuantity", &[]);
        let mut held: HashMap<String, f64> = HashMap::new();
        for (k, id) in list("ChestOrder").into_iter().enumerate() {
            *held.entry(id).or_default() += qty.get(k).copied().unwrap_or(0.0);
        }
        stacked.extend(held.into_iter().filter(|(id, n)| !id.is_empty() && *n >= 1e6).map(|(id, _)| id));
        let when = std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        let world = feat_idleon::world_reached(&save);
        let levels: f64 = (0..save.characters()).map(|i| save.numbers(&format!("Lv0_{i}"), &[]).first().copied().unwrap_or(0.0).max(0.0)).sum();
        let r = Review::new(&save, feat_idleon::run_checks(&save), world);
        let mut per: HashMap<&'static str, HashMap<u32, f32>> = HashMap::new();
        for m in &r.mechanics {
            let row = per.entry(m.key).or_default();
            for i in m.items.iter().chain(&m.later) {
                let id = *ids.entry(i.name.clone()).or_insert_with(|| {
                    names.push(i.name.clone());
                    (names.len() - 1) as u32
                });
                let v = row.entry(id).or_insert(0.0);
                *v = v.max(i.crowd_level() as f32);
            }
        }
        read.push((world, levels, when, per));
    }
    if skipped > 0 {
        eprintln!("{skipped} files weren't saves; left out");
    }

    // Stages: by world, then thirds by total class levels.
    let mut seen: Vec<Seen> = Vec::new();
    let mut counts = [0usize; STAGES.len()];
    for w in 1..=7u8 {
        let mut here: Vec<_> = read.iter().filter(|r| r.0.clamp(1, 7) == w).collect();
        here.sort_by(|a, b| a.1.total_cmp(&b.1));
        let n = here.len();
        let mut by_stage: [Vec<_>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for (k, r) in here.into_iter().enumerate() {
            by_stage[(k * 3 / n.max(1)).min(2)].push(r);
        }
        for (part, mut list) in by_stage.into_iter().enumerate() {
            list.sort_by_key(|r| std::cmp::Reverse(r.2));
            list.truncate(NEWEST);
            let stage = usize::from(w - 1) * 3 + part;
            counts[stage] = list.len();
            for r in list {
                seen.push(Seen { stage, levels: r.3.clone() });
            }
        }
    }
    let total: usize = counts.iter().sum();
    // The endgame: two steps from Late World 7's saves, then the highest of every save.
    let late7 = crowd::ENDGAME - 1;
    counts[crowd::ENDGAME] = counts[late7];
    counts[crowd::ENDGAME + 1] = counts[late7];
    counts[crowd::ENDGAME + 2] = total;
    let stages: Vec<usize> = (0..STAGES.len()).filter(|&s| counts[s] >= MIN_SAVES).collect();
    // For `COUNTS` mechanics, a world whose early, mid and late are each short of saves but that
    // has enough between them gets one column at its mid stage from all of them: a count is one
    // number per level, steady on a handful of saves where a row per item isn't.
    let whole_world: Vec<usize> = (0..6)
        .map(|w| w * 3)
        .filter(|&s0| (s0..s0 + 3).all(|s| counts[s] < MIN_SAVES) && (s0..s0 + 3).map(|s| counts[s]).sum::<usize>() >= MIN_SAVES)
        .map(|s0| s0 + 1)
        .collect();

    let mut out = String::from(HEAD);
    let _ = writeln!(out, "# Saves per stage: {}", (0..crowd::ENDGAME).filter(|&s| counts[s] > 0).map(|s| format!("{} {}", STAGES[s], counts[s])).collect::<Vec<_>>().join(", "));
    let _ = writeln!(out, "# ({total} saves; a stage needs {MIN_SAVES} to get a column)");
    let mut gen_out = out.clone();
    let mut written = 0;
    let mut hand_kept = 0;
    let mut per_account = Vec::new();
    let mut new_items: Vec<String> = Vec::new();
    for m in &full.mechanics {
        // Row order: the game's, then names only real saves have (event stamps), by first seen.
        let mut rows: Vec<String> = Vec::new();
        for i in m.items.iter().chain(&m.later) {
            if !rows.contains(&i.name) {
                rows.push(i.name.clone());
            }
        }
        let mut extra: Vec<u32> = seen.iter().flat_map(|s| s.levels.get(m.key).into_iter().flat_map(|l| l.keys().copied())).collect();
        extra.sort_unstable();
        extra.dedup();
        for id in extra {
            let name = &names[id as usize];
            if !rows.contains(name) {
                rows.push(name.clone());
            }
        }
        // Made per account from each item's max (`crowd::by_max`): no table.
        if rows.is_empty() || crowd::BY_MAX.contains(&m.key) {
            continue;
        }
        let only_base = BASE_ONLY.contains(&m.key) && base.contains_key(m.key);
        let had = current.get(m.key);
        // Items that differ from account to account (characters): no crowd tiers.
        if !seen.is_empty() && had.is_none() {
            let common = rows
                .iter()
                .filter(|n| ids.get(*n).is_some_and(|id| seen.iter().filter(|s| s.levels.get(m.key).is_some_and(|l| l.contains_key(id))).count() * 2 >= seen.len()))
                .count();
            if common * 2 < rows.len() {
                per_account.push(m.key);
                continue;
            }
        }
        // Columns, every row's cell per column, how much each row matters there, whether any
        // save has it, and the base columns used.
        let (cols, table, prio, anyone, from_base, keep, levels, count_table) = if crowd::BEST.contains(&m.key) {
            let (cols, table) = best_table(m, &rows, &ids, &seen);
            let anyone = rows.iter().map(|n| ids.get(n).is_some_and(|id| seen.iter().any(|x| x.levels.get(m.key).and_then(|l| l.get(id)).is_some_and(|v| *v > 0.0)))).collect();
            let prio = vec![vec![0.0; cols.len()]; rows.len()];
            let keep = vec![true; rows.len()];
            (cols, table, prio, anyone, HashMap::new(), keep, &[][..], Vec::new())
        } else {
            // The base's columns for stages short of saves, matched to these rows.
            let maxes: HashMap<&str, f64> = m.items.iter().chain(&m.later).filter(|i| i.raw.is_none() && i.max > 0.0).map(|i| (i.name.as_str(), i.max)).collect();
            let from_base: HashMap<usize, (Vec<Option<f64>>, Vec<f64>)> = base
                .get(m.key)
                .map(|b| {
                    b.stages
                        .iter()
                        .filter(|&&s| only_base || counts[s] < MIN_SAVES)
                        .map(|&s| (s, base_column(b, s, &rows, &maxes, m.key, &mut unmatched, &mut fuzzy)))
                        .collect()
                })
                .unwrap_or_default();
            // A `COUNTS` mechanic names only its `FIRST` items and rows typed in by hand (a row
            // that isn't what the last run wrote); the rest are asked for by the counts.
            let counted = COUNTS.iter().find(|c| c.0 == m.key).map(|c| c.1);
            let first = FIRST.iter().find(|f| f.0 == m.key).map_or(&[][..], |f| f.1);
            // The stage the mechanic opens at.
            let open = usize::from(m.world.clamp(1, 7) - 1) * 3;
            // Columns: stages with enough saves, the base's for stages without, and any stage with a
            // hand edit in it. A `COUNTS` mechanic also gets its whole-world columns, and a column
            // at each world's mid stage from where it opens up to its first saves, rising to them
            // (see the ramp below).
            let mut cols = if only_base { Vec::new() } else { stages.clone() };
            cols.extend(from_base.keys().copied());
            // (A base-only table keeps the base's columns: the old ones were the saves'.)
            if let Some(b) = had.filter(|_| !only_base) {
                cols.extend(b.stages.iter().copied());
            }
            if counted.is_some() {
                cols.extend(whole_world.iter().copied());
                let data = stages.iter().chain(&whole_world).copied().min().unwrap_or(crowd::ENDGAME);
                cols.extend((open..data).filter(|s| s % 3 == 1));
            }
            cols.sort_unstable();
            cols.dedup();
            if cols.is_empty() {
                continue;
            }

            let row_ids: Vec<Option<u32>> = rows.iter().map(|n| ids.get(n).copied()).collect();
            let full_level = |x: &Seen, r: usize| row_ids[r].and_then(|id| x.levels.get(m.key)?.get(&id).copied()).unwrap_or(0.0);
            let keep: Vec<bool> = rows
                .iter()
                .map(|name| {
                    counted.is_none()
                        || first.contains(&name.as_str())
                        || (gen_text.is_some() && had.and_then(|b| b.row(name)).is_some_and(|cells| last_gen.get(m.key).and_then(|g| g.row(name)) != Some(cells)))
                })
                .collect();
            let level = |x: &Seen, r: usize| if keep[r] { full_level(x, r) } else { 0.0 };
            let anyone: Vec<bool> = (0..rows.len()).map(|r| seen.iter().any(|x| full_level(x, r) > 0.0)).collect();
            // The `@any` rows: per level, a cell per column, and what the last column asked.
            let levels: &[f64] = counted.unwrap_or(&[]);
            let mut count_table: Vec<Vec<Option<f64>>> = vec![vec![None; cols.len()]; levels.len()];
            let mut count_floor = vec![0.0f64; levels.len()];
            let mut table: Vec<Vec<Option<f64>>> = vec![vec![None; cols.len()]; rows.len()];
            // How much each row matters at each stage, higher first: the share of the stage's saves
            // that have it, or for a base column, how early AutoReview's tiers ask for it.
            let mut prio: Vec<Vec<f64>> = vec![vec![0.0; cols.len()]; rows.len()];
            let mut floor = vec![0.0f64; rows.len()];
            // The first column with enough saves.
            let mut first_data: Option<usize> = None;
            for (k, &s) in cols.iter().enumerate() {
                let here: Vec<&Seen> = match s.checked_sub(crowd::ENDGAME) {
                    Some(0 | 1) => seen.iter().filter(|x| x.stage == late7).collect(),
                    Some(_) => seen.iter().collect(),
                    None if counted.is_some() && whole_world.contains(&s) => seen.iter().filter(|x| x.stage / 3 == s / 3).collect(),
                    None => seen.iter().filter(|x| x.stage == s).collect(),
                };
                let n = here.len();
                if n >= MIN_SAVES && first_data.is_none() && !only_base {
                    first_data = Some(k);
                }
                if n < MIN_SAVES || only_base {
                    if let Some((col, rank)) = from_base.get(&s) {
                        for r in 0..rows.len() {
                            table[r][k] = if keep[r] { col[r] } else { None };
                            prio[r][k] = -rank[r];
                        }
                        // The base's counts: its own `@any` rows, and `@first N` as N unlocked.
                        let b = &base[m.key];
                        for (c, &l) in levels.iter().enumerate() {
                            let named = b.cell(&format!("@any {}", crowd::cell_text(Some(l))), s).flatten();
                            let v = named.or_else(|| (l <= 1.0).then(|| b.cell("@first", s).flatten()).flatten());
                            if let Some(v) = v {
                                count_floor[c] = count_floor[c].max(v.min(rows.len() as f64));
                                count_table[c][k] = Some(count_floor[c]);
                            }
                        }
                    }
                    continue;
                }
                // Each count: how many items the stage's saves have at the level; the count half of
                // them reach (the endgame: the top quarter, tenth, then the most of any save).
                for (c, &l) in levels.iter().enumerate() {
                    let mut have: Vec<usize> = here.iter().map(|x| (0..rows.len()).filter(|&r| f64::from(full_level(x, r)) >= l).count()).collect();
                    have.sort_unstable_by(|a, b| b.cmp(a));
                    let q = s.checked_sub(crowd::ENDGAME).map_or(0.5, |e| [0.25, 0.10, 0.0][e.min(2)]);
                    let at = ((q * n as f64).ceil() as usize).clamp(1, n) - 1;
                    let v = crowd::nice(have[at] as f64).max(count_floor[c]);
                    count_floor[c] = v;
                    count_table[c][k] = (v > 0.0).then_some(v);
                }
                // lv[r][j]: row r's level in save j; sorted[r]: highest first.
                let lv: Vec<Vec<f32>> = (0..rows.len()).map(|r| here.iter().map(|x| level(x, r)).collect()).collect();
                let sorted: Vec<Vec<f32>> = lv.iter().map(|v| { let mut v = v.clone(); v.sort_by(|a, b| b.total_cmp(a)); v }).collect();
                // Each item asks for the level a share `q` of the stage has. Starting from half,
                // `q` rises (asking less, and dropping what fewer players have started) until at
                // least half the stage has the whole tier done: a tier is what a typical player at
                // that stage has all of, not each item's own median.
                let mut targets = floor.clone();
                let mut chose = 1.0;
                // The endgame: per item, the level the top quarter of Late World 7 has, then the top
                // tenth, then the highest any save has; never under the tier before, so nothing
                // there is already asked for by World 7.
                if let Some(e) = s.checked_sub(crowd::ENDGAME) {
                    let top = [0.25, 0.10, 0.0][e.min(2)];
                    let at = ((top * n as f64).ceil() as usize).clamp(1, n) - 1;
                    targets = (0..rows.len()).map(|r| crowd::nice(f64::from(sorted[r][at])).max(floor[r]).min(maxes.get(rows[r].as_str()).copied().unwrap_or(f64::MAX))).collect();
                }
                for q in (10..=20).map(|t| f64::from(t) * 0.05).filter(|_| !crowd::endgame(s)) {
                    chose = q;
                    let at = ((q * n as f64).ceil() as usize).clamp(1, n) - 1;
                    targets = (0..rows.len()).map(|r| crowd::nice(f64::from(sorted[r][at])).max(floor[r]).min(maxes.get(rows[r].as_str()).copied().unwrap_or(f64::MAX))).collect();
                    let met = (0..n).filter(|&j| (0..rows.len()).all(|r| targets[r] <= 0.0 || f64::from(lv[r][j]) >= targets[r])).count();
                    if met * 2 >= n {
                        break;
                    }
                }
                if std::env::var("VERBOSE").is_ok() {
                    let asked = targets.iter().filter(|t| **t > 0.0).count();
                    eprintln!("{:<28} {}  q {:.2}  asks {asked} of {}", m.key, STAGES[s], chose, rows.len());
                }
                for r in 0..rows.len() {
                    table[r][k] = (targets[r] > 0.0).then_some(targets[r]);
                    prio[r][k] = (0..n).filter(|&j| f64::from(lv[r][j]) >= targets[r]).count() as f64 / n as f64;
                }
                floor = targets;
            }
            // The ramp (`COUNTS` mechanics): from where the mechanic opens to the first column with
            // saves that asks for it, each count and each named row rises with the square of the
            // stages since it opened toward what that column asks, so the climb starts small (a base
            // column that asks more keeps its number); whole numbers, never down from one column to
            // the next. A count climbs to what its first saves say, nothing when they say none. A
            // named row is to get as soon as you can: Lv 1 from where the mechanic opens, and one
            // the first saves don't ask for yet (Refinery Speed in World 6) climbs toward the first
            // column that does.
            if let Some(kd) = first_data.filter(|_| counted.is_some()) {
                let ramp = |cells: &mut Vec<Option<f64>>, most: f64, named: bool| {
                    let to = if named { (kd..cells.len()).find(|&k| cells[k].is_some()) } else { Some(kd) };
                    let Some(to) = to else { return };
                    let top = cells[to].unwrap_or(0.0);
                    let share = |s: usize| if s < open { 0.0 } else { ((s - open + 1) as f64 / (cols[to] - open + 1) as f64).powi(2) };
                    let start = |s: usize| if named && s >= open { 1.0 } else { 0.0 };
                    let mut run = 0.0f64;
                    for k in 0..to {
                        let v = crowd::nice(top * share(cols[k])).floor().max(start(cols[k])).max(cells[k].unwrap_or(0.0)).max(run).min(most);
                        run = v;
                        cells[k] = (v > 0.0).then_some(v);
                    }
                };
                for cells in &mut count_table {
                    ramp(cells, rows.len() as f64, false);
                }
                for (r, cells) in table.iter_mut().enumerate().filter(|(r, _)| keep[*r]) {
                    ramp(cells, maxes.get(rows[r].as_str()).copied().unwrap_or(f64::MAX), true);
                }
            }
            (cols, table, prio, anyone, from_base, keep, levels, count_table)
        };
        let mut gen_block = Block::new(cols.clone());
        let mut block = Block::new(cols.clone());
        block.off = had.is_some_and(|b| b.off);
        // Rows most important first: by the first stage that asks for them, then by how much
        // they matter there, then the game's order. Each stage's parts follow this order.
        let mut by_prio: Vec<usize> = (0..rows.len()).collect();
        let first = |r: usize| table[r].iter().position(Option::is_some).unwrap_or(usize::MAX);
        by_prio.sort_by(|&a, &b| first(a).cmp(&first(b)).then_with(|| {
            let k = first(a);
            if k == usize::MAX { std::cmp::Ordering::Equal } else { prio[b][k].total_cmp(&prio[a][k]) }
        }).then(a.cmp(&b)));
        let mut lines = Vec::new();
        // Named rows, then the counts (`@any` rows), lowest level first.
        let count_names: Vec<String> = levels.iter().map(|&l| format!("@any {}", crowd::cell_text(Some(l)))).collect();
        let all: Vec<(String, Vec<Option<f64>>, bool)> = by_prio
            .into_iter()
            .filter(|&r| keep[r])
            .map(|r| (rows[r].clone(), table[r].clone(), anyone[r]))
            .chain(count_names.iter().cloned().zip(count_table.iter().cloned()).map(|(n, c)| (n, c, true)))
            .collect();
        for (name, generated, anyone) in &all {
            let (name, anyone) = (name, *anyone);
            let generated = generated.clone();
            let mut cells = generated.clone();
            let mut hand = false;
            if let Some(b) = had {
                for (k, &s) in cols.iter().enumerate() {
                    let Some(now) = b.cell(name, s) else { continue };
                    let before = last_gen.get(m.key).and_then(|g| g.cell(name, s));
                    if gen_text.is_some() && before != Some(now) {
                        hand |= now != cells[k];
                        cells[k] = now;
                    }
                }
            }
            hand_kept += usize::from(hand);
            // New in the game since the last run (an update): there's no crowd for it yet.
            let fresh = gen_text.is_some() && last_gen.get(m.key).is_none_or(|g| g.row(name).is_none());
            if fresh {
                new_items.push(format!("{}: {name}", m.key));
            }
            let note = match (hand, fresh, anyone) {
                (true, _, _) => "  # hand",
                (_, true, _) => "  # new since the last run",
                (_, _, false) if !seen.is_empty() => "  # no save has it yet",
                _ => "",
            };
            lines.push((name.clone(), cells.clone(), note));
            gen_block.push(name.clone(), generated);
            block.push(name.clone(), cells);
        }
        if let Some(b) = had {
            for (name, _) in &b.rows {
                if !rows.contains(name) && crowd::any_level(name).is_none() {
                    eprintln!("{}: {name} isn't in the game or any save any more; dropped", m.key);
                }
            }
        }
        let count = match (m.count, from_base.is_empty()) {
            _ if only_base => "every column from AutoReview's tiers, changed",
            (Count::Open, true) => "no max",
            (Count::Open, false) => "no max; World 1 to 5 columns from AutoReview's tiers, changed",
            (_, false) => "World 1 to 5 columns from AutoReview's tiers, changed",
            _ => "",
        };
        let count = if levels.is_empty() { count.to_string() } else if count.is_empty() { "counts".to_string() } else { format!("counts; {count}") };
        let count = count.as_str();
        write_block(&mut out, m.key, m.name, count, &block, &lines);
        let gen_lines: Vec<_> = gen_block.rows.iter().map(|(n, c)| (n.clone(), c.clone(), "")).collect();
        write_block(&mut gen_out, m.key, m.name, count, &gen_block, &gen_lines);
        written += 1;
    }
    for key in current.keys() {
        if !full.mechanics.iter().any(|m| m.key == key) {
            eprintln!("{key}: no such mechanic any more; dropped");
        }
    }
    if !new_items.is_empty() {
        eprintln!("new since the last run ({}): {}", new_items.len(), new_items.join(", "));
    }
    if !fuzzy.is_empty() {
        eprintln!("base names matched loosely ({}): {}", fuzzy.len(), fuzzy.join("; "));
    }
    if !unmatched.is_empty() {
        eprintln!("base names with no item here ({}): {}", unmatched.len(), unmatched.join(", "));
    }
    if !per_account.is_empty() {
        eprintln!("left out, their items differ per account: {}", per_account.join(", "));
    }
    add_greenstacks(&format!("{dir}/greenstacks.txt"), &stacked);
    std::fs::write(&path, out).expect("write crowd.txt");
    std::fs::write(&gen_path, gen_out).expect("write crowd.gen.txt");
    eprintln!("wrote {written} mechanics from {total} saves to {path}; {hand_kept} rows keep hand edits");
}

/// One base column for these rows: `@first N` is the first N rows (found, level 1),
/// `@first-max N` the first N at their max, neither counting a `!Name`; named rows match by name, letters
/// and digits only, else by the most words in common. A cell of 1e15 is the item's max.
fn base_column(b: &Block, stage: usize, rows: &[String], maxes: &HashMap<&str, f64>, key: &str, unmatched: &mut Vec<String>, fuzzy: &mut Vec<String>) -> (Vec<Option<f64>>, Vec<f64>) {
    let mut col: Vec<Option<f64>> = vec![None; rows.len()];
    // Where each row comes in AutoReview's tiers (lower first): their row order, then counts in
    // game order after them.
    let mut rank: Vec<f64> = (0..rows.len()).map(|r| (b.rows.len() + r) as f64).collect();
    let norm = |s: &str| s.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_lowercase();
    let words = |s: &str| s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| w.len() >= 3).map(str::to_lowercase).collect::<Vec<_>>();
    let at_max = |r: usize| maxes.get(rows[r].as_str()).copied();
    let get = |name: &str| b.cell(name, stage).flatten();
    // `!Name` rows leave an item out of the counts (it doesn't count toward N either).
    let skip: Vec<String> = b.rows.iter().filter(|(name, _)| name.starts_with('!') && get(name).is_some()).map(|(name, _)| norm(&name[1..])).collect();
    for (special, max) in [("@first", false), ("@first-max", true)] {
        let Some(n) = get(special) else { continue };
        for r in (0..rows.len()).filter(|&r| !skip.contains(&norm(&rows[r]))).take(n as usize) {
            col[r] = if max { at_max(r).or(Some(1.0)) } else { Some(col[r].unwrap_or(0.0).max(1.0)) };
        }
    }
    for (pos, (name, _)) in b.rows.iter().enumerate() {
        if name.starts_with(['@', '!']) {
            continue;
        }
        let Some(v) = get(name) else { continue };
        let r = rows.iter().position(|row| norm(row) == norm(name)).or_else(|| {
            let theirs = words(name);
            let score = |row: &String| words(row).iter().filter(|w| theirs.iter().any(|t| w.starts_with(t.as_str()) || t.starts_with(w.as_str()))).count();
            let best = rows.iter().map(score).max().unwrap_or(0);
            let found: Vec<usize> = (0..rows.len()).filter(|&r| best > 0 && score(&rows[r]) == best).collect();
            let r = (found.len() == 1).then(|| found[0])?;
            let note = format!("{key}: {name} = {}", rows[r]);
            if !fuzzy.contains(&note) {
                fuzzy.push(note);
            }
            Some(r)
        });
        match r {
            Some(r) => {
                let v = if v >= 1e15 { at_max(r).unwrap_or(1.0) } else { v.min(at_max(r).unwrap_or(f64::MAX)) };
                col[r] = Some(col[r].unwrap_or(0.0).max(v));
                rank[r] = rank[r].min(pos as f64);
            }
            None => {
                let note = format!("{key}: {name}");
                if !unmatched.contains(&note) {
                    unmatched.push(note);
                }
            }
        }
    }
    (col, rank)
}

/// A `crowd::BEST` mechanic's table: every stage a column, each cell its share (`best_curve`)
/// of the highest level any save has for that row.
fn best_table(m: &feat_idleon::review::Mechanic, rows: &[String], ids: &HashMap<String, u32>, seen: &[Seen]) -> (Vec<usize>, Vec<Vec<Option<f64>>>) {
    let world: HashMap<&str, u8> = m.items.iter().chain(&m.later).map(|i| (i.name.as_str(), i.world.unwrap_or(m.world))).collect();
    // On a card with a switch, found-once items with no level of their own (the owl's
    // Megafeathers) belong to the other side: no level ladder for them.
    let once: Vec<&str> = m.items.iter().chain(&m.later).filter(|i| m.levels_note.is_some() && i.max > 0.0 && i.max <= 1.0 && i.raw.is_none()).map(|i| i.name.as_str()).collect();
    let cols: Vec<usize> = (0..STAGES.len()).collect();
    let best: Vec<f64> = rows.iter().map(|name| f64::from(ids.get(name).map_or(0.0, |id| seen.iter().filter_map(|x| x.levels.get(m.key)?.get(id).copied()).fold(0.0f32, f32::max)))).collect();
    // The stage each row opens at: its world's first, or for the Upgrade Vault, the first stage
    // where the whole vault on the curve (every upgrade's best at that stage's share) reaches the
    // total vault levels that open the upgrade, so the tiers open upgrades in the game's order.
    let mut open: Vec<Option<usize>> = rows.iter().map(|name| (m.key != "upgrade-vault").then(|| usize::from(world.get(name.as_str()).copied().unwrap_or(m.world).clamp(1, 7) - 1) * 3)).collect();
    let mut table: Vec<Vec<Option<f64>>> = vec![vec![None; cols.len()]; rows.len()];
    let opens: HashMap<String, f64> = if m.key == "upgrade-vault" { feat_idleon::vault_opens().into_iter().collect() } else { HashMap::new() };
    for &s in &cols {
        if m.key == "upgrade-vault" {
            // Where the vault as a whole stands at this stage: every upgrade's best on the curve.
            let asked: f64 = best.iter().map(|b| b * crowd::curve_from(0, s)).sum();
            for r in 0..rows.len() {
                if open[r].is_none() && opens.get(&rows[r]).is_some_and(|&need| asked >= need) {
                    open[r] = Some(s);
                }
            }
        }
        for r in 0..rows.len() {
            let Some(o) = open[r] else { continue };
            let f = crowd::curve_from(o, s);
            if f > 0.0 && best[r] > 0.0 && !once.contains(&rows[r].as_str()) {
                table[r][s] = Some(crowd::nice(best[r] * f).max(1.0));
            }
        }
    }
    (cols, table)
}

/// Adds to `greenstacks.txt` the stackable items some save has greenstacked or holds 1M of
/// that it doesn't list yet; a `-ID` line keeps an item out.
fn add_greenstacks(path: &str, stacked: &std::collections::BTreeSet<String>) {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let named: std::collections::HashSet<&str> = text.lines().map(|l| l.split('#').next().unwrap_or("").trim().trim_start_matches('-').trim()).filter(|l| !l.is_empty()).collect();
    let stackable: std::collections::HashSet<&str> = feat_idleon::stackable_items().into_iter().collect();
    let new: Vec<&String> = stacked.iter().filter(|id| stackable.contains(id.as_str()) && !named.contains(id.as_str())).collect();
    if new.is_empty() {
        return;
    }
    let mut out = text.clone();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    for id in &new {
        let _ = writeln!(out, "{id:<24}# {}", feat_idleon::item_name(id).unwrap_or(id));
    }
    std::fs::write(path, out).expect("write greenstacks.txt");
    eprintln!("greenstacks.txt: added {} ({})", new.len(), new.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "));
}

fn write_block(out: &mut String, key: &str, name: &str, note: &str, b: &Block, lines: &[(String, Vec<Option<f64>>, &str)]) {
    let width = lines.iter().map(|l| l.0.chars().count()).max().unwrap_or(0).max(6) + 2;
    let cells: Vec<Vec<String>> = lines.iter().map(|l| l.1.iter().map(|&c| crowd::cell_text(c)).collect()).collect();
    let cw: Vec<usize> = (0..b.stages.len()).map(|k| cells.iter().map(|r| r[k].len()).max().unwrap_or(1).max(2)).collect();
    let head = if b.off { format!("{key} off") } else { key.to_string() };
    let extra = if note.is_empty() { String::new() } else { format!(", {note}") };
    let _ = write!(out, "\n{head:<40}# {name}{extra}\n");
    let _ = write!(out, "  {:<width$}", "stages");
    for (k, &s) in b.stages.iter().enumerate() {
        let _ = write!(out, "  {:>w$}", STAGES[s], w = cw[k]);
    }
    out.push('\n');
    for (l, row) in lines.iter().zip(&cells) {
        let _ = write!(out, "  {:<width$}", l.0);
        for (k, c) in row.iter().enumerate() {
            let _ = write!(out, "  {:>w$}", c, w = cw[k]);
        }
        let _ = writeln!(out, "{}", l.2);
    }
}

const HEAD: &str = "\
# Tiers from other players' saves (see src/crowd.rs). One table per mechanic: a row per item,
# a column per stage of the game (7e, 7m, 7l: early, mid and late World 7). A cell is the
# level that at least half the players at that stage have; - means not asked for yet.
# Each column is one tier on the page; a last \"Everything maxed\" tier follows.
#
# Edit any cell: the next run of the generator keeps it (its row then says # hand).
#   - to stop asking for something everyone levels but shouldn't, write -
#   - to ask for something sooner, write a level in an earlier column
#   - `off` after a mechanic's key keeps the page on its old ladder for it
# A table marked `counts` (vials, the Arcade) names only what to get first; its `@any L` rows
# say how many items, any of them, a stage asks at Lv L or more (`@any 1`: how many unlocked).
# Each count that rises is its own tier, after the stage's named items. Type an item's row in
# to ask for it by name too.
# Rewrite from saves (keeps your edits):
#   cargo run --release -p feat-idleon --example crowd -- <folder of saves>/*.json
# `bot preview` (a debug build) reads this file on every check: edit, save, check again.
";
