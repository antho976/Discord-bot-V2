//! What the review says, as text and numbers, for every screen that shows it: the web page and
//! the desktop app read the same words from here, so the two never drift apart.

use crate::expected::Pace;
use crate::review::{Count, Item, Mechanic, Owed, Review, State as Step, Tier};

pub const WORLD_NAMES: [&str; 8] = ["Account", "Blunder Hills", "Yum-Yum Desert", "Frostbite Tundra", "Hyperion Nebula", "Smolderin' Plateau", "Spirited Valley", "Shimmerfin Deep"];

pub fn world_tag(w: u8) -> String {
    if w == 0 { "All".into() } else { format!("W{w}") }
}

/// A world tab's name: "World 3", or "Account" for what isn't in a world.
pub fn world_name(w: u8) -> String {
    if w == 0 { "Account".to_string() } else { format!("World {w}") }
}

/// The worlds that have mechanics, in tab order (1 to 7, then Account), and the one to open on:
/// the highest reached.
pub fn worlds(r: &Review) -> (Vec<u8>, u8) {
    let worlds: Vec<u8> = (1..=7).chain([0]).filter(|w| r.mechanics.iter().any(|m| m.world == *w)).collect();
    let start = worlds.iter().copied().filter(|w| (1..=r.world).contains(w)).max().unwrap_or(1);
    (worlds, start)
}

/// Mechanics the lead's count covers: in a reached world, with something to count.
pub fn counted(r: &Review) -> usize {
    r.mechanics.iter().filter(|m| m.world <= r.world && m.total() > 0.0).count()
}

/// Not started chips shown before "Show N more".
pub const FRESH: usize = 8;

/// Tiers a card's ladder shows at once, and later tiers its "After that" lists.
pub const LADDER: usize = 6;
pub const LATER: usize = 5;

/// Tasks shown under the current tier before "and N more".
pub const TASKS: usize = 8;

/// Boxes Should have by now shows, oldest first: two rows of four on a wide screen, never a
/// third (narrower screens show two rows of what fits).
pub const OWED_BOXES: usize = 8;

/// The narrowest a Should have by now box gets before the boxes take one column fewer, and the
/// gap between them: the web page's breakpoints (692, 1044, 1396 px) are these.
pub const OWE_MIN: f32 = 340.0;
pub const OWE_GAP: f32 = 12.0;

/// Columns of Should have by now boxes in a column of content this wide, and how many boxes
/// that shows (two rows at most; a single column shows them all).
pub fn owe_columns(width: f32) -> (usize, usize) {
    let cols = (((width + OWE_GAP) / (OWE_MIN + OWE_GAP)).floor() as usize).clamp(1, 4);
    (cols, if cols == 1 { OWED_BOXES } else { cols * 2 })
}

/// Which tiers a ladder shows: `LADDER` of them, starting one before the current tier (the last
/// done one, for context), all of them when they fit. `(from, to)`, `to` exclusive.
pub fn window(len: usize, current: Option<usize>) -> (usize, usize) {
    if len <= LADDER {
        return (0, len);
    }
    let from = current.map_or(len - LADDER, |c| c.saturating_sub(1)).min(len - LADDER);
    (from, from + LADDER)
}

/// Mechanics whose tiers aren't in a real order yet (owner, 2026-11-06): their cards say so.
pub const ON_HOLD: &[&str] = &[
    "arcade", "sigils", "post-office", "equinox-upgrades", "sampling", "meals", "lab-chips", "breeding-upgrades", "shiny",
    "breeding-pets", "artifacts", "slab",
];

pub fn on_hold(m: &Mechanic) -> bool {
    ON_HOLD.contains(&m.key)
}

/// Pictures that are a portrait at the left end of a wide name plate (the cavern villagers):
/// drawn as their left square, which is the portrait.
pub const LEFT_SQUARE: &[&str] = &["HoleUIvillager"];

pub fn left_square(id: &str) -> bool {
    LEFT_SQUARE.iter().any(|p| id.starts_with(p))
}

/// The lead's line under the share: "World 7 · 10 characters" and the packs choice.
pub fn packs_line(r: &Review) -> Option<&'static str> {
    (r.packs != crate::review::Packs::Buy).then(|| r.packs.label())
}

/// What a link to the review says above the page: "Mira's account", "Test account".
pub fn title(r: &Review) -> String {
    if r.test {
        "Test account".into()
    } else if !r.name.is_empty() {
        format!("{}'s account", r.name)
    } else {
        "Account check".into()
    }
}

/// A world column's tooltip and its number: "Frostbite Tundra: 64%", "64"; a world not reached
/// says so.
pub fn world_col(r: &Review, w: u8) -> (Option<f64>, String, String) {
    let reached = w == 0 || w <= r.world;
    let share = r.world_share(w).filter(|_| reached);
    let tip = format!("{}: {}", WORLD_NAMES[w as usize], share.map_or("not reached".into(), pct));
    let n = share.map_or("·".into(), |s| format!("{:.0}", s * 100.0));
    (share, tip, n)
}

/// Should have by now, ready to draw.
pub struct Owing {
    /// Things owed in all, not counting the endgame goals.
    pub total: usize,
    /// There are endgame boxes (goals from the best saves).
    pub goals: bool,
    pub boxes: Vec<OweBox>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chip {
    Goal,
    Missed,
    NextUp,
}

impl Chip {
    pub fn label(self) -> &'static str {
        match self {
            Chip::Goal => "Goal",
            Chip::Missed => "Missed",
            Chip::NextUp => "Next up",
        }
    }
}

pub struct OweBox {
    pub stage: usize,
    pub label: String,
    /// The stage's world, for its colour.
    pub world: u8,
    pub chip: Chip,
    /// "12 things".
    pub count: String,
    pub lines: Vec<OweLine>,
}

/// One mechanic's line in a box: the tier you're on and the one the stage asks for.
pub struct OweLine {
    /// Index into `Review::mechanics`.
    pub mechanic: usize,
    /// The one item left, when there's only one (" · Copper Stamp").
    pub item: Option<String>,
    /// "T 3 ▸ T 5", or "T 5".
    pub tiers: String,
    /// What it waits for when none of it can move yet.
    pub need: Option<String>,
    /// "3 left: Sword Stamp, ...".
    pub title: String,
}

pub fn owing(r: &Review) -> Option<Owing> {
    let stages = r.should_have();
    if stages.is_empty() {
        return None;
    }
    let total = stages.iter().filter(|(s, _)| !crate::crowd::endgame(*s)).map(|(_, v)| v.len()).sum();
    let goals = stages.iter().any(|(s, _)| crate::crowd::endgame(*s));
    let boxes = stages
        .iter()
        .take(OWED_BOXES)
        .map(|(stage, v)| {
            let w = crate::crowd::stage_world(*stage);
            // A mechanic's items at this stage, in the order the list ranks them.
            let mut rows: Vec<Vec<&Owed>> = Vec::new();
            for o in v {
                match rows.iter_mut().find(|g| g[0].mechanic == o.mechanic) {
                    Some(g) => g.push(o),
                    None => rows.push(vec![o]),
                }
            }
            let chip = if crate::crowd::endgame(*stage) {
                Chip::Goal
            } else if w < r.world {
                Chip::Missed
            } else {
                Chip::NextUp
            };
            OweBox {
                stage: *stage,
                label: crate::crowd::stage_label(*stage),
                world: w,
                chip,
                count: format!("{}{}", v.len(), if v.len() == 1 { " thing" } else { " things" }),
                lines: rows.iter().map(|g| owe_line(r, g)).collect(),
            }
        })
        .collect();
    Some(Owing { total, goals, boxes })
}

/// The sentence under Should have by now's heading.
pub fn owing_about(o: &Owing) -> String {
    let mut s = format!("{} things players have by your world and you don't, oldest first. ", o.total);
    if o.goals {
        s.push_str("Then the endgame: steps toward the best of every save we have, and Min-max, the best itself. ");
    }
    s.push_str("Click one to open its card.");
    s
}

fn owe_line(r: &Review, g: &[&Owed]) -> OweLine {
    let m = &r.mechanics[g[0].mechanic];
    let here = m.tiers().iter().position(|t| !t.complete()).unwrap_or(0) + 1;
    let want = g.iter().map(|o| o.tier).max().unwrap_or(0) + 1;
    let items: Vec<&Item> = g.iter().map(|o| &m.items[o.item]).collect();
    let need = items.iter().all(|i| i.state == Step::Locked).then(|| items[0].how.as_deref()).flatten().map(|h| match items.len() {
        1 => h.to_string(),
        _ => format!("{}: {h}", items[0].name),
    });
    let names = items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(", ");
    OweLine {
        mechanic: g[0].mechanic,
        item: match items.as_slice() {
            [i] => Some(i.name.clone()),
            _ => None,
        },
        tiers: if want > here { format!("T {here} ▸ T {want}") } else { format!("T {want}") },
        need,
        title: format!("{} left: {names}", items.len()),
    }
}

/// What an item's row says on the right: its named level, "17 / 40", or "Lv 17".
pub fn item_fig(i: &Item) -> String {
    match &i.level_name {
        Some(n) => n.clone(),
        None if i.max > 0.0 => format!("{} / {}", num(i.level), num(i.max)),
        None => format!("Lv {}", num(i.level)),
    }
}

/// Where a mechanic stands against its tiers' worlds, in a line: what world it's at, and the
/// first tier overdue for the world you're in.
pub fn pace_line(p: &Pace, reached: u8) -> Option<String> {
    let at = p.at.map(|w| format!("Where a World {w} player would be."));
    match (at, p.due.and_then(|k| p.worlds[k].map(|w| (k, w)))) {
        (at, Some((k, w))) => Some(format!("{}Tier {} is expected by World {w}, and you're in World {reached}.", at.map(|a| a + " ").unwrap_or_default(), k + 1)),
        (Some(at), None) => Some(format!("{at} Nothing here is overdue for World {reached}.")),
        (None, None) => p.worlds.iter().flatten().min().map(|w| format!("Its first tier is expected by World {w}.")),
    }
}

/// A card's line under its name: the figure, the tier you're on ("Tier 3/8"), and the world
/// level it's at.
pub fn card_line(m: &Mechanic, tiers: &[Tier], reached: u8) -> String {
    let mut s = fig(m);
    match tiers.iter().position(|t| !t.complete()) {
        Some(k) => s.push_str(&format!(" · Tier {}/{}", k + 1, tiers.len())),
        None if !tiers.is_empty() => s.push_str(" · All tiers"),
        None => {}
    }
    if let Some(p) = &m.pace {
        match (p.at, p.worlds.iter().flatten().min()) {
            (Some(w), _) => s.push_str(&format!(" · World {w} level")),
            (None, Some(f)) if *f <= reached => s.push_str(&format!(" · Under World {f} level")),
            _ => {}
        }
    }
    s
}

/// A level as the mechanic names it: "Gold" for a statue, "Lv 17" otherwise.
pub fn level_text(m: &Mechanic, level: f64) -> String {
    match &m.tier_names {
        Some(names) if level >= 1.0 => names.get(level as usize - 1).cloned().unwrap_or_else(|| format!("Lv {}", num(level))),
        Some(_) => "None".into(),
        None => format!("Lv {}", num(level)),
    }
}

/// A task's figure: "Lv 3 → Lv 10", a named level, or nothing for a one-time thing.
pub fn task_fig(m: &Mechanic, i: &Item, target: f64) -> String {
    let once = i.max <= 1.0 && m.tier_names.is_none();
    // A one-time thing has no level to show; the chip or the line says the rest.
    if m.crowd.is_some() && !(once && i.raw.is_none()) {
        format!("Lv {} → Lv {}", num(m.tier_level(i)), num(target))
    } else if once {
        i.level_name.clone().unwrap_or_default()
    } else {
        format!("{} → {}", level_text(m, i.level), level_text(m, target))
    }
}

/// A task's chip: "Do now", "Locked", or none.
pub fn chip(i: &Item) -> Option<&'static str> {
    match i.state {
        Step::Ready => Some("Do now"),
        Step::Locked => Some("Locked"),
        Step::Unknown => None,
    }
}

/// The note under "To reach Tier N": how many are left and how many can be done now.
pub fn tasks_note(t: &Tier, tasks: &[(&Item, f64)]) -> String {
    let ready = tasks.iter().filter(|(i, _)| i.state == Step::Ready).count();
    let mut s = String::new();
    if t.targets.is_none() {
        s.push_str(&format!("{} more, any of these. ", t.need - t.met));
    }
    s.push_str(&format!("{} left.", tasks.len()));
    if ready == tasks.len() && ready > 0 {
        s.push_str(" You can do all of them now.");
    } else if ready > 0 {
        s.push_str(&format!(" {ready} you can do right now."));
    }
    s
}

/// A later tier's right side: "4 more", or who holds it back ("Sword Stamp Lv 3, ... and 2 more").
pub fn later_who(m: &Mechanic, t: &Tier) -> String {
    let Some(targets) = &t.targets else {
        return format!("{} more", t.need - t.met);
    };
    let behind: Vec<String> = m
        .items
        .iter()
        .zip(targets)
        .filter(|(i, tg)| m.tier_level(i) < **tg)
        .map(|(i, _)| match () {
            _ if m.crowd.is_some() && i.raw.is_some() => format!("{} Lv {}", i.name, num(m.tier_level(i))),
            _ if i.max <= 1.0 && m.tier_names.is_none() => i.name.clone(),
            _ if m.crowd.is_some() => format!("{} Lv {}", i.name, num(i.level)),
            _ => format!("{} {}", i.name, level_text(m, i.level)),
        })
        .collect();
    let mut s = behind.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    if behind.len() > 3 {
        s.push_str(&format!(" and {} more", behind.len() - 3));
    }
    s
}

/// "Not counted until you get there: X (World 5), ... and 3 more."
pub fn later_worlds(m: &Mechanic) -> Option<String> {
    if m.later.is_empty() {
        return None;
    }
    let mut s = String::from("Not counted until you get there: ");
    for (n, i) in m.later.iter().take(6).enumerate() {
        if n > 0 {
            s.push_str(", ");
        }
        s.push_str(&format!("{} (World {})", i.name, i.world.unwrap_or(0)));
    }
    if m.later.len() > 6 {
        s.push_str(&format!(" and {} more", m.later.len() - 6));
    }
    s.push('.');
    Some(s)
}

pub fn fig(m: &Mechanic) -> String {
    match m.count {
        Count::Levels => format!("{} / {} {}", num(m.have()), num(m.total()), m.unit),
        Count::Done => format!("{} of {} {}", num(m.have()), num(m.total()), m.unit),
        // A sum of levels with no max says little: the middle one says more.
        Count::Open => match m.spread() {
            Some([_, median, top, _]) => format!("median {} · highest {}", num(median.round()), num(top)),
            None => format!("{} {}", num(m.have()), m.unit),
        },
    }
}

/// A share for a bar's width, to three places: the bar can't show more.
pub fn share3(s: f64) -> String {
    format!("{:.3}", s.clamp(0.0, 1.0)).trim_end_matches('0').trim_end_matches('.').to_string()
}

pub fn pct(s: f64) -> String {
    let p = s * 100.0;
    // 99.6% shouldn't read as done.
    if (99.5..100.0).contains(&p) { "99%".into() } else { format!("{:.0}%", p) }
}

/// Whole numbers with thousands separators; anything else with one decimal. From a quadrillion
/// on, the game's short form ("1.2Qa", "3.4e21"): past 2^53 the digits aren't real, and past
/// 2^64 a plain cast reads 18,446,744,073,709,551,615.
pub fn num(n: f64) -> String {
    if n.is_nan() {
        return "0".into();
    }
    if n.is_infinite() || n.abs() >= 1e15 {
        return crate::checks::big(n);
    }
    if n.fract().abs() > 1e-9 {
        return format!("{n:.1}");
    }
    let s = format!("{}", n.abs() as u64);
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0.0 { format!("-{out}") } else { out }
}

/// The longest problem report text: the suggestion box's limit (1,500) less the longest header a
/// report gets ("Account check report (40 characters), 60 characters:").
pub const REPORT_TEXT: usize = 1370;

/// A report as `/suggest` text, for someone who isn't signed in: the same header budget as one
/// sent from the page (account 40, card 60).
pub fn report_text(account: &str, card: &str, text: &str) -> String {
    let acc: String = account.chars().take(40).collect();
    let card: String = card.chars().take(60).collect();
    let mut s = String::from("Account check report");
    if !acc.is_empty() {
        s.push_str(&format!(" ({acc})"));
    }
    if !card.is_empty() {
        s.push_str(&format!(", {card}"));
    }
    s.push_str(": ");
    s.push_str(text.trim());
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ladder_shows_six_tiers_around_where_you_are() {
        assert_eq!(window(4, Some(2)), (0, 4));
        assert_eq!(window(20, Some(0)), (0, 6));
        assert_eq!(window(20, Some(9)), (8, 14));
        assert_eq!(window(20, Some(19)), (14, 20));
        assert_eq!(window(20, None), (14, 20));
    }

    #[test]
    fn owe_columns_match_the_pages_breakpoints() {
        assert_eq!(owe_columns(691.0), (1, 8));
        assert_eq!(owe_columns(692.0), (2, 4));
        assert_eq!(owe_columns(1043.0), (2, 4));
        assert_eq!(owe_columns(1044.0), (3, 6));
        assert_eq!(owe_columns(1396.0), (4, 8));
        assert_eq!(owe_columns(3000.0), (4, 8));
    }
}
