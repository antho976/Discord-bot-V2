//! Tiers from other players' saves. For each stage of the game (early, mid and late in each
//! world), the level that at least half the players at that stage have reached, item by item.
//! Players already follow what's good and what's cheap, so what most of them have done by a
//! stage is what that stage's tier asks for; what nobody has touched yet isn't asked for.
//!
//! The numbers live in `crowd.txt` next to this crate's `Cargo.toml`, one table per mechanic:
//! a row per item, a column per stage. `cargo run -p feat-idleon --example crowd -- saves/*.json`
//! writes it from a folder of saves, and keeps every number edited by hand (see that example).
//! A mechanic with a table gets its ladder from it, a tier per stage; one without keeps the
//! ladder `review.rs` builds from the game's maxes.
//!
//! A release build carries the file in itself. A debug build (`bot preview`) reads it again
//! for every check, so an edit shows on the next check with no rebuild.

use std::collections::HashMap;

use crate::review::Item;

/// Stage codes, in game order: world, then e(arly), m(id), l(ate); then the endgame past Late
/// World 7: two steps up, then the highest any save has (`max`).
pub const STAGES: [&str; 24] = ["1e", "1m", "1l", "2e", "2m", "2l", "3e", "3m", "3l", "4e", "4m", "4l", "5e", "5m", "5l", "6e", "6m", "6l", "7e", "7m", "7l", "end1", "end2", "max"];
/// The first endgame stage.
pub const ENDGAME: usize = 21;

/// The world a stage is in, 1 to 7; the endgame counts as World 7.
pub fn stage_world(stage: usize) -> u8 {
    (stage.min(ENDGAME - 1) / 3 + 1) as u8
}

/// Past Late World 7: goals, not what a stage expects.
pub fn endgame(stage: usize) -> bool {
    stage >= ENDGAME
}

/// "Early World 7", "Endgame 1", "Min-max".
pub fn stage_label(stage: usize) -> String {
    match stage.checked_sub(ENDGAME) {
        Some(0) => "Endgame 1".into(),
        Some(1) => "Endgame 2".into(),
        Some(_) => "Min-max".into(),
        None => format!("{} World {}", ["Early", "Mid", "Late"][stage % 3], stage_world(stage)),
    }
}

/// Mechanics whose tables come from the best level any save has, not from what half of each
/// stage has: every item rises a little at every stage, from the stage its world opens, toward
/// the highest any save has (see the crowd example's `best_curve`). An item that is hard to
/// level stays low even for the endgame, and its tiers follow. Their tiers are a stage each,
/// never split into parts.
pub const BEST: [&str; 1] = ["statue-levels"];

/// One mechanic's table.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Block {
    /// `off` after the key: the table is kept but not used (the mechanic keeps its own ladder).
    pub off: bool,
    /// A `BEST` mechanic's table: a tier per stage, not split into parts.
    pub whole: bool,
    /// Its columns, as indexes into `STAGES`, in game order.
    pub stages: Vec<usize>,
    /// Item name to its cells, one per column: `None` for `-` (not asked for at that stage).
    pub rows: Vec<(String, Vec<Option<f64>>)>,
    index: HashMap<String, usize>,
}

impl Block {
    pub fn new(stages: Vec<usize>) -> Block {
        Block { stages, ..Block::default() }
    }

    pub fn push(&mut self, name: String, cells: Vec<Option<f64>>) {
        self.index.entry(name.clone()).or_insert(self.rows.len());
        self.rows.push((name, cells));
    }

    pub fn row(&self, name: &str) -> Option<&[Option<f64>]> {
        self.index.get(name).map(|&i| self.rows[i].1.as_slice())
    }

    /// One cell: `None` when the row or the column isn't there, `Some(None)` for `-`.
    pub fn cell(&self, name: &str, stage: usize) -> Option<Option<f64>> {
        let col = self.stages.iter().position(|&s| s == stage)?;
        self.row(name).and_then(|r| r.get(col).copied())
    }
}

pub type Table = HashMap<String, Block>;

const FILE: &str = include_str!("../crowd.txt");

/// Runs `f` with the tables as the file says them now.
pub fn with<R>(f: impl FnOnce(&Table) -> R) -> R {
    #[cfg(debug_assertions)]
    {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/crowd.txt"));
        f(&parse(text.as_deref().unwrap_or(FILE)).0)
    }
    #[cfg(not(debug_assertions))]
    {
        static TABLE: std::sync::LazyLock<Table> = std::sync::LazyLock::new(|| parse(FILE).0);
        f(&TABLE)
    }
}

/// Reads the file. A line at the margin names a mechanic by its key (`off` after it turns the
/// table off); under it, an indented `stages` line names the columns, and every other indented
/// line is an item: its name, then one cell per column, a number or `-`. Anything after `#` is
/// for people. Lines it can't read are skipped and listed in the second half of the answer.
pub fn parse(text: &str) -> (Table, Vec<String>) {
    let mut table = Table::new();
    let mut problems = Vec::new();
    let mut key: Option<String> = None;
    for (n, line) in text.lines().enumerate() {
        let n = n + 1;
        let line = line.split('#').next().unwrap_or("");
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some(&first) = words.first() else { continue };
        if !line.starts_with([' ', '\t']) {
            if table.contains_key(first) {
                problems.push(format!("line {n}: {first} is listed twice"));
            }
            let mut b = Block { whole: BEST.contains(&first), ..Block::default() };
            match words.get(1) {
                None => {}
                Some(&"off") => b.off = true,
                Some(w) => problems.push(format!("line {n}: {w:?} after {first} (only `off` goes there)")),
            }
            table.insert(first.to_string(), b);
            key = Some(first.to_string());
            continue;
        }
        let Some(block) = key.as_ref().and_then(|k| table.get_mut(k)) else {
            problems.push(format!("line {n}: a row before any mechanic"));
            continue;
        };
        if first == "stages" && words.len() > 1 && words[1..].iter().all(|w| STAGES.contains(w)) {
            if !block.stages.is_empty() || !block.rows.is_empty() {
                problems.push(format!("line {n}: a second stages line"));
                continue;
            }
            block.stages = words[1..].iter().filter_map(|w| STAGES.iter().position(|s| s == w)).collect();
            if !block.stages.windows(2).all(|p| p[0] < p[1]) {
                problems.push(format!("line {n}: stages out of game order"));
                block.stages.clear();
            }
            continue;
        }
        let cols = block.stages.len();
        if cols == 0 {
            problems.push(format!("line {n}: an item before the stages line"));
            continue;
        }
        if words.len() <= cols {
            problems.push(format!("line {n}: needs a name and {cols} cells"));
            continue;
        }
        let (name, cells) = words.split_at(words.len() - cols);
        let mut row = Vec::with_capacity(cols);
        for c in cells {
            match *c {
                "-" => row.push(None),
                c => match c.replace(',', "").parse::<f64>() {
                    Ok(v) if v.is_finite() && v >= 0.0 => row.push((v > 0.0).then_some(v)),
                    _ => {
                        problems.push(format!("line {n}: {c:?} isn't a level (a number, or - for none)"));
                        row.push(None);
                    }
                },
            }
        }
        let name = name.join(" ");
        if block.row(&name).is_some() {
            problems.push(format!("line {n}: {name} is listed twice"));
            continue;
        }
        block.push(name, row);
    }
    (table, problems)
}

/// How a cell is written: a whole number with thousands commas, or two decimals under 1.
pub fn cell_text(v: Option<f64>) -> String {
    match v {
        None => "-".to_string(),
        Some(v) if v < 1.0 => format!("{:.2}", v),
        Some(v) => {
            let s = format!("{:.0}", v.floor());
            let mut out = String::new();
            for (i, ch) in s.chars().enumerate() {
                if i > 0 && (s.len() - i).is_multiple_of(3) {
                    out.push(',');
                }
                out.push(ch);
            }
            out
        }
    }
}

/// A level rounded down to a number a person would pick: whole under 20, fives under 100,
/// then two significant figures (1,234 reads 1,200). Down, so a tier never asks for more than
/// the players it came from have.
pub fn nice(x: f64) -> f64 {
    if !x.is_finite() || x <= 0.0 {
        0.0
    } else if x < 1.0 {
        (x * 100.0).floor() / 100.0
    } else if x < 20.0 {
        x.floor()
    } else if x < 100.0 {
        (x / 5.0).floor() * 5.0
    } else {
        let p = 10f64.powi(x.log10().floor() as i32 - 1);
        (x / p).floor() * p
    }
}

/// One tier of a ladder from a table.
#[derive(Debug, Clone, PartialEq)]
pub struct Rung {
    /// "Early World 7", or "Early World 7 · 2 of 4" for a part of a big stage.
    pub label: String,
    pub world: u8,
    /// Index into `STAGES`.
    pub stage: usize,
    /// The level each item needs, in item order; 0 for not asked.
    pub targets: Vec<f64>,
}

/// A mechanic's ladder from its table: a tier per stage, a stage that asks for many new things
/// split into parts of about `PART` (see `split`).
#[derive(Debug, Clone, PartialEq)]
pub struct Ladder {
    pub rungs: Vec<Rung>,
}

/// Items a tier adds at most before it's split into parts, and the most parts a step becomes.
pub const PART: usize = 8;
pub const MAX_PARTS: usize = 10;

/// The parts of one step from `prev` to `next` (targets in item order): the items it raises,
/// taken in `order` (best first), a part at a time, each part keeping the ones before it. One
/// part (just `next`) when it raises `part` items or fewer.
pub fn split(prev: &[f64], next: &[f64], order: &[usize], part: usize, max_parts: usize) -> Vec<Vec<f64>> {
    let raised: Vec<usize> = order.iter().copied().filter(|&i| next.get(i).copied().unwrap_or(0.0) > prev.get(i).copied().unwrap_or(0.0)).collect();
    let n = raised.len();
    if n <= part {
        return vec![next.to_vec()];
    }
    let parts = n.div_ceil(part).min(max_parts.max(1));
    let size = n.div_ceil(parts);
    let mut out = Vec::with_capacity(parts);
    let mut cur = prev.to_vec();
    cur.resize(next.len(), 0.0);
    for chunk in raised.chunks(size) {
        for &i in chunk {
            cur[i] = next[i];
        }
        out.push(cur.clone());
    }
    // Items the order left out (none, when it covers every item) arrive with the last part.
    if let Some(last) = out.last_mut() {
        *last = next.to_vec();
    }
    out
}

impl Ladder {
    /// `None` when the table is off or asks nothing of these items. Items are matched by name;
    /// an item the table doesn't name isn't asked for. A target above an item's own max (a cap
    /// this account hasn't raised yet) asks for the max; items with a `raw` level have no max
    /// in those units.
    pub fn new(block: &Block, items: &[Item]) -> Option<Ladder> {
        if block.off {
            return None;
        }
        let rows: Vec<Option<&[Option<f64>]>> = items.iter().map(|i| block.row(&i.name)).collect();
        // Priority: the table's row order (the generator writes rows most important first).
        let mut order: Vec<usize> = (0..items.len()).collect();
        order.sort_by_key(|&i| block.index.get(&items[i].name).copied().unwrap_or(usize::MAX));
        let mut rungs: Vec<Rung> = Vec::new();
        let mut prev = vec![0.0; items.len()];
        for (col, &stage) in block.stages.iter().enumerate() {
            let targets: Vec<f64> = items
                .iter()
                .zip(&rows)
                .map(|(i, r)| {
                    let t = r.and_then(|r| r.get(col).copied().flatten()).unwrap_or(0.0);
                    if i.max > 0.0 && i.raw.is_none() { t.min(i.max) } else { t }
                })
                .collect();
            if targets.iter().all(|&t| t <= 0.0) || rungs.last().is_some_and(|r| r.targets == targets) {
                continue;
            }
            let parts = if block.whole { vec![targets.clone()] } else { split(&prev, &targets, &order, PART, MAX_PARTS) };
            let n = parts.len();
            for (p, t) in parts.into_iter().enumerate() {
                let label = if n == 1 { stage_label(stage) } else { format!("{} · {} of {n}", stage_label(stage), p + 1) };
                rungs.push(Rung { label, world: stage_world(stage), stage, targets: t });
            }
            prev = targets;
        }
        (!rungs.is_empty()).then_some(Ladder { rungs })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_endgame_follows_late_world_7() {
        assert_eq!(STAGES[ENDGAME - 1], "7l");
        assert_eq!((stage_label(20), stage_label(21), stage_label(22), stage_label(23)), ("Late World 7".into(), "Endgame 1".into(), "Endgame 2".into(), "Min-max".into()));
        assert!((ENDGAME..STAGES.len()).all(|s| stage_world(s) == 7 && endgame(s)));
        assert!(!endgame(20));
    }

    const TEXT: &str = "\
# a comment
stamps                       # Stamps
  stages            7e   7m   7l
  Pickaxe Stamp     65   75   85
  Clover Stamp       -    -  1,200   # hand
vials off
  stages  2e
  Copper Corona   3
bad
  Too Few  1
";

    #[test]
    fn reads_tables_and_says_what_it_cant() {
        let (t, problems) = parse(TEXT);
        let s = &t["stamps"];
        assert_eq!(s.stages, vec![18, 19, 20]);
        assert_eq!(s.row("Pickaxe Stamp"), Some(&[Some(65.0), Some(75.0), Some(85.0)][..]));
        assert_eq!(s.cell("Clover Stamp", 20), Some(Some(1200.0)));
        assert_eq!(s.cell("Clover Stamp", 18), Some(None));
        assert_eq!(s.cell("Clover Stamp", 5), None);
        assert!(t["vials"].off);
        assert_eq!(problems.len(), 1, "{problems:?}");
    }

    #[test]
    fn nice_rounds_down_to_numbers_people_pick() {
        assert_eq!(nice(0.0), 0.0);
        assert_eq!(nice(0.567), 0.56);
        assert_eq!(nice(1.0), 1.0);
        assert_eq!(nice(17.9), 17.0);
        assert_eq!(nice(67.0), 65.0);
        assert_eq!(nice(1234.0), 1200.0);
        assert_eq!(nice(98_765.0), 98_000.0);
        assert_eq!(cell_text(Some(1_234_567.0)), "1,234,567");
        assert_eq!(cell_text(Some(0.5)), "0.50");
    }

    #[test]
    fn a_ladder_skips_empty_and_repeated_stages_and_keeps_to_the_cap() {
        let (t, _) = parse("m\n  stages 6l 7e 7m 7l\n  A  -  5  5  9\n  B  -  -  -  20\n");
        let items = vec![Item::new("A", None, 5.0, 10.0), Item::new("B", None, 0.0, 15.0), Item::new("C", None, 0.0, 10.0)];
        let l = Ladder::new(&t["m"], &items).unwrap();
        assert_eq!(l.rungs.len(), 2);
        assert_eq!(l.rungs[0], Rung { label: "Early World 7".into(), world: 7, stage: 18, targets: vec![5.0, 0.0, 0.0] });
        assert_eq!(l.rungs[1].targets, vec![9.0, 15.0, 0.0]);
        // A stage that raises many items comes in parts, in the table's row order.
        let mut text = String::from("m\n  stages 7e\n");
        for k in (0..20).rev() {
            text.push_str(&format!("  I{k}  5\n"));
        }
        let (big, _) = parse(&text);
        let items: Vec<Item> = (0..20).map(|k| Item::new(format!("I{k}"), None, 0.0, 10.0)).collect();
        let l = Ladder::new(&big["m"], &items).unwrap();
        assert_eq!(l.rungs.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["Early World 7 · 1 of 3", "Early World 7 · 2 of 3", "Early World 7 · 3 of 3"]);
        // Rows run I19 down to I0, so the first part asks for the last items.
        assert!(l.rungs[0].targets[19] > 0.0 && l.rungs[0].targets[0] == 0.0);
        assert!(l.rungs[2].targets.iter().all(|&t| t == 5.0));
        let mut off = t["m"].clone();
        off.off = true;
        assert_eq!(Ladder::new(&off, &items), None);
    }
}
