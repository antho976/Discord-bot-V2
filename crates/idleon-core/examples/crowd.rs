//! Writes `crowd.txt` (tiers from other players' saves, see `src/crowd.rs`) from a folder of
//! saves.
//!
//! `cargo run --release -p idleon-core --example crowd -- ~/saves/*.json`
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

use idleon_core::crowd::{self, Block, STAGES};
use idleon_core::review::{Count, Review};
use idleon_core::save::Save;

/// Saves a stage needs before it gets a column.
const MIN_SAVES: usize = 5;
/// The newest this many saves of a stage count.
const NEWEST: usize = 300;

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
    let full = Review::new(&empty, idleon_core::run_checks(&empty), 7);

    // Read every save once, keeping only levels.
    let mut names: Vec<String> = Vec::new();
    let mut ids: HashMap<String, u32> = HashMap::new();
    let mut read: Vec<(u8, f64, std::time::SystemTime, Levels)> = Vec::new();
    let mut skipped = 0;
    for p in std::env::args().skip(1) {
        let Ok(text) = std::fs::read_to_string(&p) else { skipped += 1; continue };
        let Ok(save) = Save::parse(&text) else { skipped += 1; continue };
        let when = std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        let world = idleon_core::world_reached(&save);
        let levels: f64 = (0..save.characters()).map(|i| save.numbers(&format!("Lv0_{i}"), &[]).first().copied().unwrap_or(0.0).max(0.0)).sum();
        let r = Review::new(&save, idleon_core::run_checks(&save), world);
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
        if rows.is_empty() {
            continue;
        }
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
        // The base's columns for stages short of saves, matched to these rows.
        let maxes: HashMap<&str, f64> = m.items.iter().chain(&m.later).filter(|i| i.raw.is_none() && i.max > 0.0).map(|i| (i.name.as_str(), i.max)).collect();
        let from_base: HashMap<usize, (Vec<Option<f64>>, Vec<f64>)> = base
            .get(m.key)
            .map(|b| {
                b.stages
                    .iter()
                    .filter(|&&s| counts[s] < MIN_SAVES)
                    .map(|&s| (s, base_column(b, s, &rows, &maxes, m.key, &mut unmatched, &mut fuzzy)))
                    .collect()
            })
            .unwrap_or_default();
        // Columns: stages with enough saves, the base's for stages without, and any stage with a
        // hand edit in it.
        let mut cols = stages.clone();
        cols.extend(from_base.keys().copied());
        if let Some(b) = had {
            cols.extend(b.stages.iter().copied());
        }
        cols.sort_unstable();
        cols.dedup();
        if cols.is_empty() {
            continue;
        }

        let mut gen_block = Block::new(cols.clone());
        let mut block = Block::new(cols.clone());
        block.off = had.is_some_and(|b| b.off);
        let row_ids: Vec<Option<u32>> = rows.iter().map(|n| ids.get(n).copied()).collect();
        let level = |x: &Seen, r: usize| row_ids[r].and_then(|id| x.levels.get(m.key)?.get(&id).copied()).unwrap_or(0.0);
        let anyone: Vec<bool> = (0..rows.len()).map(|r| seen.iter().any(|x| level(x, r) > 0.0)).collect();
        let mut table: Vec<Vec<Option<f64>>> = vec![vec![None; cols.len()]; rows.len()];
        // How much each row matters at each stage, higher first: the share of the stage's saves
        // that have it, or for a base column, how early AutoReview's tiers ask for it.
        let mut prio: Vec<Vec<f64>> = vec![vec![0.0; cols.len()]; rows.len()];
        let mut floor = vec![0.0f64; rows.len()];
        for (k, &s) in cols.iter().enumerate() {
            let here: Vec<&Seen> = match s.checked_sub(crowd::ENDGAME) {
                Some(0 | 1) => seen.iter().filter(|x| x.stage == late7).collect(),
                Some(_) => seen.iter().collect(),
                None => seen.iter().filter(|x| x.stage == s).collect(),
            };
            let n = here.len();
            if n < MIN_SAVES {
                if let Some((col, rank)) = from_base.get(&s) {
                    for r in 0..rows.len() {
                        table[r][k] = col[r];
                        prio[r][k] = -rank[r];
                    }
                }
                continue;
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
        // Rows most important first: by the first stage that asks for them, then by how much
        // they matter there, then the game's order. Each stage's parts follow this order.
        let mut by_prio: Vec<usize> = (0..rows.len()).collect();
        let first = |r: usize| table[r].iter().position(Option::is_some).unwrap_or(usize::MAX);
        by_prio.sort_by(|&a, &b| first(a).cmp(&first(b)).then_with(|| {
            let k = first(a);
            if k == usize::MAX { std::cmp::Ordering::Equal } else { prio[b][k].total_cmp(&prio[a][k]) }
        }).then(a.cmp(&b)));
        let mut lines = Vec::new();
        for r in by_prio {
            let name = &rows[r];
            let anyone = anyone[r];
            let generated = table[r].clone();
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
                if !rows.contains(name) {
                    eprintln!("{}: {name} isn't in the game or any save any more; dropped", m.key);
                }
            }
        }
        let count = match (m.count, from_base.is_empty()) {
            (Count::Open, true) => "no max",
            (Count::Open, false) => "no max; World 1 to 5 columns from AutoReview's tiers, changed",
            (_, false) => "World 1 to 5 columns from AutoReview's tiers, changed",
            _ => "",
        };
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
# Rewrite from saves (keeps your edits):
#   cargo run --release -p idleon-core --example crowd -- <folder of saves>/*.json
# `bot preview` (a debug build) reads this file on every check: edit, save, check again.
";
