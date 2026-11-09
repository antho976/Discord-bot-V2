//! What the checks find, and what to do next.
//!
//! Every check reads one mechanic and says how far along it is against the game's own maxes:
//! the items in it (a vial, a prayer, a grid node), each with its level and its max. The review
//! then compares it with the ranking (`crowd.txt`): each mechanic's tiers, and everything the
//! ranking expects by your world that you don't have (`Review::should_have`).

use crate::crowd::Ladder;
use crate::expected::Pace;
use crate::save::Save;

/// One thing inside a mechanic: a vial, a statue, a research node.
#[derive(Debug, Clone)]
pub struct Item {
    pub name: String,
    /// The game's picture ID, when it has one.
    pub picture: Option<String>,
    pub level: f64,
    pub max: f64,
    /// What a level is called when it isn't a level: "Onyx", "Tier 3".
    pub level_name: Option<String>,
    /// How to move it on, in the player's own numbers: "Next level costs 400K particles".
    pub how: Option<String>,
    /// Whether the next step can be taken now, as far as the check can tell.
    pub state: State,
    /// The world this item opens in, when that's later than its mechanic's own world (a W1
    /// mechanic's item that only exists once you reach W7). Items in a world the account hasn't
    /// reached leave the ladder and the shares, and show as "Opens in World N".
    pub world: Option<u8>,
    /// The item's real level when `level` counts something else: a stamp is 1 once found
    /// (stamps have no cap), a bubble's `level` is how many of its bonus targets it has passed.
    /// Tiers from other players' saves (`crowd.txt`) compare this.
    pub raw: Option<f64>,
    /// Max levels that only a pack you don't own adds (the Potluck Pack's +10 on vault
    /// upgrades): taken off the max for a player who doesn't buy packs.
    pub pack_levels: f64,
    /// Only comes in a gem shop pack, and you don't have it: left out for a player who doesn't
    /// buy packs.
    pub pack_only: bool,
    /// The world its tiers start in, when that's later than its mechanic's and the item still
    /// counts before then (a liquid that opens at an Alchemy level, not a world): a ladder made
    /// from the maxes (`crowd::by_max`) starts its climb there.
    pub ladder_from: Option<u8>,
}

/// What the player spends real money on, chosen on the paste box: the tiers leave out what
/// they won't buy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Packs {
    /// Everything counts.
    #[default]
    Buy,
    /// Packs already owned count, no new ones: max levels and items only a pack you don't own
    /// gives are left out.
    NoMore,
    /// That, and the Gem Shop leaves Should have by now (gems are mostly bought).
    Free,
}

impl Packs {
    /// From the form's value ("buy", "no-more", "free"); anything else is `Buy`.
    pub fn from_code(s: &str) -> Packs {
        match s.trim() {
            "no-more" => Packs::NoMore,
            "free" => Packs::Free,
            _ => Packs::Buy,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Packs::Buy => "buy",
            Packs::NoMore => "no-more",
            Packs::Free => "free",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Packs::Buy => "I buy packs",
            Packs::NoMore => "No more packs",
            Packs::Free => "Free to play",
        }
    }
}

/// Whether an item's next step is open to the player right now.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum State {
    /// Nothing known either way.
    #[default]
    Unknown,
    /// They have what the next step costs.
    Ready,
    /// Something else has to happen first (`how` says what).
    Locked,
}

/// Something that raises a mechanic's cap, and whether the player has it.
#[derive(Debug, Clone)]
pub struct Raise {
    pub name: String,
    /// "+20", "+1 each".
    pub amount: String,
    pub owned: bool,
}

/// What the player holds to spend on a mechanic, and what it buys.
#[derive(Debug, Clone)]
pub struct Have {
    /// "96 Qa particles".
    pub figure: String,
    /// "Enough to take Phosphorus to Lv 61, or Sulfur to Lv 52."
    pub text: String,
}

/// One rung of a mechanic's ladder.
#[derive(Debug, Clone)]
pub struct Tier {
    pub label: String,
    /// The level each item needs, in item order; `None` for a count ("any 32 of 80"), where
    /// which items you have doesn't matter.
    pub targets: Option<Vec<f64>>,
    /// A count's level: an item counts at this level or more (`@any` rows in `crowd.txt`).
    /// `None` counts what's at its max.
    pub at: Option<f64>,
    /// How many of `need` are there.
    pub met: usize,
    pub need: usize,
}

impl Tier {
    pub fn complete(&self) -> bool {
        self.met >= self.need
    }
}

impl Item {
    pub fn new(name: impl Into<String>, picture: Option<String>, level: f64, max: f64) -> Item {
        Item { name: name.into(), picture, level: level.max(0.0), max: max.max(0.0), level_name: None, how: None, state: State::Unknown, world: None, raw: None, pack_levels: 0.0, pack_only: false, ladder_from: None }
    }

    pub fn done(&self) -> bool {
        self.max > 0.0 && self.level >= self.max
    }

    pub fn share(&self) -> f64 {
        if self.max <= 0.0 { 0.0 } else { (self.level / self.max).clamp(0.0, 1.0) }
    }

    /// The level other players' saves are compared on: `raw` when there is one.
    pub fn crowd_level(&self) -> f64 {
        self.raw.unwrap_or(self.level)
    }
}

/// A count ladder adds about this many at a time, in at most this many tiers.
const COUNT_STEP: usize = 5;
const MAX_COUNT_TIERS: usize = 12;
/// The most parts one milestone of a level ladder becomes.
const MAX_LEVEL_PARTS: usize = 4;

/// How a mechanic's progress adds up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Count {
    /// Levels: the sum of levels against the sum of maxes ("1,240 of 1,600 levels").
    Levels,
    /// Items done: how many are at their max ("31 of 41 bought").
    Done,
    /// No max anywhere (bubbles, refinery ranks): the levels are shown, lowest first, but the
    /// mechanic stays out of every share. Items keep `max` 0.
    Open,
}

#[derive(Debug, Clone)]
pub struct Mechanic {
    pub key: &'static str,
    /// 1 to 7; 0 for the account-wide ones (cards, achievements).
    pub world: u8,
    pub name: &'static str,
    pub picture: Option<String>,
    pub count: Count,
    /// What one counted thing is called: "levels", "bought", "found".
    pub unit: &'static str,
    pub items: Vec<Item>,
    /// A line on what raises the max or why it reads as it does, when that matters.
    pub note: Option<String>,
    /// The game's own tier names, for levels 1, 2, 3... ("Gold", "Onyx", "Zenith"). With them
    /// the ladder is the game's tiers; without, it's milestones on the way to each item's max.
    pub tier_names: Option<Vec<String>>,
    /// A line per named tier saying what it needs beyond levels ("Needs Ionized Sigils"),
    /// in `tier_names` order; empty strings and a short list mean nothing to add.
    pub tier_notes: Vec<String>,
    pub raises: Vec<Raise>,
    pub have: Option<Have>,
    /// Items from worlds the account hasn't reached (see `Item::world`), moved out of `items`
    /// by `Review::new` so they count nowhere until then.
    pub later: Vec<Item>,
    /// For a mechanic with no max: keep the check's own item order and show this heading
    /// ("Soonest rank-up first") instead of sorting lowest first.
    pub open_order: Option<&'static str>,
    /// Where it stands against the worlds its tiers belong to (`expected.txt`), set by
    /// `Review::new`; `None` until its tiers have worlds.
    pub pace: Option<Pace>,
    /// The ladder from other players' saves (`crowd.txt`), set by `Review::new` when the
    /// mechanic has a table there. With it, `tiers` is a tier per stage of the game.
    pub crowd: Option<Ladder>,
    /// A mechanic with both the game's own tiers (`tier_names`, on `level`) and a ladder from
    /// other players' saves on `raw` levels (statues: Gold to Zenith, and statue levels): the
    /// card shows a switch between Levels and Tiers. This is the note for the Levels side;
    /// `note` is the Tiers side's. See `levels_view` and `tiers_view`.
    pub levels_note: Option<String>,
    /// Game tiers drawn whole (Gold, Onyx, Zenith), not in parts of about 8.
    pub whole_tiers: bool,
    /// What the switch calls the side that isn't Levels: "Tiers" (statues), "Unlock" (stamps).
    pub other_side: &'static str,
}

impl Mechanic {
    pub fn have(&self) -> f64 {
        match self.count {
            Count::Levels => self.items.iter().map(|i| i.level.min(i.max)).sum(),
            Count::Done => self.items.iter().filter(|i| i.done()).count() as f64,
            Count::Open => self.items.iter().map(|i| i.level).sum(),
        }
    }

    pub fn total(&self) -> f64 {
        match self.count {
            Count::Levels => self.items.iter().map(|i| i.max).sum(),
            Count::Done => self.items.len() as f64,
            Count::Open => 0.0,
        }
    }

    pub fn share(&self) -> f64 {
        let t = self.total();
        if t <= 0.0 { 0.0 } else { (self.have() / t).clamp(0.0, 1.0) }
    }

    pub fn done(&self) -> bool {
        self.total() > 0.0 && self.have() >= self.total()
    }

    /// Items not at their max, least done first (lowest level first when there's no max).
    pub fn open_items(&self) -> Vec<&Item> {
        let mut v: Vec<&Item> = self.items.iter().filter(|i| !i.done()).collect();
        if self.open_order.is_some() {
            return v;
        }
        v.sort_by(|a, b| a.share().total_cmp(&b.share()).then(a.level.total_cmp(&b.level)));
        v
    }

    /// The ladder, easiest rung first. Empty for mechanics with no max. With a table from other
    /// players' saves (`crowd`), a tier per stage of the game. Without, three kinds, each in
    /// small steps so a tier is a plan, not a wall:
    /// - things you buy or find once (`Count::Done`, or every max 1): about five more at a time
    ///   (at most 12 tiers), any of them, so one rare item never holds every later tier back;
    /// - the game's own tiers (`tier_names`): Gold, then Onyx..., each in parts of about 8
    ///   items in game order (roughly the order the game unlocks them);
    /// - levels: started, a quarter, half, three quarters of each item's own max, then maxed,
    ///   each in parts the same way. Per item, so one expensive item holds back only its share.
    pub fn tiers(&self) -> Vec<Tier> {
        if let Some(l) = &self.crowd {
            return self.crowd_tiers(l).into_iter().map(|(t, _)| t).collect();
        }
        if self.count == Count::Open || self.items.is_empty() {
            return Vec::new();
        }
        let n = self.items.len();
        if self.tier_names.is_none() && (self.count == Count::Done || self.items.iter().all(|i| i.max <= 1.0)) {
            let done = self.items.iter().filter(|i| i.done()).count();
            let steps = n.div_ceil(COUNT_STEP).clamp(n.min(5), MAX_COUNT_TIERS);
            let mut out: Vec<Tier> = Vec::new();
            for k in 1..=steps {
                let need = (n * k).div_ceil(steps);
                if out.last().is_some_and(|t| t.need == need) {
                    continue;
                }
                let label = if need == n { format!("All {n}") } else { format!("Any {need} of {n}") };
                out.push(Tier { label, targets: None, at: None, met: done.min(need), need });
            }
            return out;
        }
        // Each milestone: (its name, every item's target at it).
        let mut steps: Vec<(String, Vec<f64>)> = Vec::new();
        if let Some(names) = &self.tier_names {
            for (k, name) in names.iter().enumerate() {
                let lv = (k + 1) as f64;
                steps.push((name.clone(), self.items.iter().map(|i| lv.min(i.max)).collect()));
            }
        } else {
            let same = self.items.iter().all(|i| i.max == self.items[0].max);
            for (f, word) in [(0.0, "Started"), (0.25, "A quarter of the max"), (0.5, "Half the max"), (0.75, "Three quarters of the max"), (1.0, "Maxed")] {
                let targets: Vec<f64> = self.items.iter().map(|i| if f == 0.0 { i.max.min(1.0) } else { (i.max * f).ceil().max(1.0) }).collect();
                let label = match (n, same) {
                    (1, _) if f == 0.0 => "Started".to_string(),
                    (1, _) => format!("{} {}", crate::page::num(targets[0]), self.unit),
                    (_, true) if f == 1.0 => format!("Max, Lv {}", crate::page::num(targets[0])),
                    (_, true) if f > 0.0 => format!("Lv {}", crate::page::num(targets[0])),
                    _ => word.to_string(),
                };
                steps.push((label, targets));
            }
        }
        steps.dedup_by(|b, a| a.1 == b.1);
        let order: Vec<usize> = (0..n).collect();
        let mut rungs: Vec<(String, Vec<f64>)> = Vec::new();
        let mut prev = vec![0.0; n];
        for (name, targets) in steps {
            let parts = if self.whole_tiers { vec![targets.clone()] } else { crate::crowd::split(&prev, &targets, &order, crate::crowd::PART, MAX_LEVEL_PARTS) };
            let k = parts.len();
            for (p, t) in parts.into_iter().enumerate() {
                rungs.push((if k == 1 { name.clone() } else { format!("{name} · {} of {k}", p + 1) }, t));
            }
            prev = targets;
        }
        rungs
            .into_iter()
            .map(|(label, targets)| {
                let met = self.items.iter().zip(&targets).filter(|(i, t)| i.level >= **t).count();
                Tier { label, need: targets.len(), targets: Some(targets), at: None, met }
            })
            .collect()
    }

    /// The ladder from other players' saves, each tier with the world it belongs to: a tier per
    /// stage (only the items asked for count toward it), then everything at its max, which
    /// belongs to no world.
    fn crowd_tiers(&self, l: &Ladder) -> Vec<(Tier, Option<u8>)> {
        let tier = |label: String, targets: Vec<f64>| {
            let asked = || self.items.iter().zip(&targets).filter(|(_, t)| **t > 0.0);
            let need = asked().count();
            let met = asked().filter(|(i, t)| i.crowd_level() >= **t).count();
            Tier { label, targets: Some(targets), at: None, met, need }
        };
        // A count rung: any `need` items at the level.
        let count = |label: String, need: usize, level: f64| {
            let met = self.items.iter().filter(|i| i.crowd_level() >= level).count().min(need);
            Tier { label, targets: None, at: Some(level), met, need }
        };
        // Endgame tiers belong to no world: nobody is behind for not having them.
        let mut out: Vec<(Tier, Option<u8>)> = l
            .rungs
            .iter()
            .map(|r| {
                let t = match r.count {
                    Some((n, level)) => count(r.label.clone(), n, level),
                    None => tier(r.label.clone(), r.targets.clone()),
                };
                (t, (!crate::crowd::endgame(r.stage)).then_some(r.world))
            })
            .collect();
        // Items with a real level of their own have no max in those units (stamps, bubbles).
        // Not on the Levels side of a switch: the maxes there are the other side's (the owl).
        if self.count != Count::Open && self.levels_note.is_none() && self.items.iter().any(|i| i.max > 0.0) && self.items.iter().all(|i| i.raw.is_none()) {
            let max: Vec<f64> = self.items.iter().map(|i| i.max).collect();
            if l.rungs.iter().rev().find(|r| r.count.is_none()).is_none_or(|r| r.targets != max) {
                out.push((tier("Everything maxed".to_string(), max), None));
            }
        }
        out
    }

    /// Both ladders (`levels_note` set and a crowd ladder on `raw` levels): the Levels side. Its
    /// tasks are levels, so the items' next steps on the other side (`how`, `state`: a statue's
    /// next tier) are left off, except for what you don't have at all yet (a stamp's where to
    /// find it); the levels note stands in for the other one.
    pub fn levels_view(&self) -> Option<Mechanic> {
        let note = self.levels_note.clone().filter(|_| self.crowd.is_some())?;
        let mut m = self.clone();
        m.note = Some(note);
        // Items with no max (the owl's lasting bonuses) live on this side: theirs stay.
        for i in m.items.iter_mut().filter(|i| i.level > 0.0 && i.max > 0.0) {
            i.how = None;
            i.state = State::Unknown;
        }
        Some(m)
    }

    /// Both ladders: the other side (`other_side`), the game's own ladder (statue tiers whole,
    /// stamps found), with each item's next step. Its pace and side box belong to the levels.
    pub fn tiers_view(&self) -> Option<Mechanic> {
        self.levels_note.as_ref().filter(|_| self.crowd.is_some())?;
        let mut m = self.clone();
        m.crowd = None;
        m.pace = None;
        m.have = None;
        m.whole_tiers = true;
        // Only what this side counts: the owl's lasting bonuses (no max) are the Levels side's.
        m.items.retain(|i| i.max > 0.0);
        Some(m)
    }

    /// The level a tier compares an item on: its real level under a crowd ladder.
    pub fn tier_level(&self, i: &Item) -> f64 {
        if self.crowd.is_some() { i.crowd_level() } else { i.level }
    }

    /// What tier `k` of `tiers` itself asks for that you don't have yet. For a mechanic whose
    /// tiers fit on the card (`crowd::FITS`), only what it adds to the tier before it, so a
    /// tier picked ahead never lists more than it asks; otherwise everything it waits on
    /// (`tasks`).
    pub fn tier_tasks<'a>(&'a self, tiers: &[Tier], k: usize) -> Vec<(&'a Item, f64)> {
        let v = self.tasks(&tiers[k]);
        if self.crowd.is_none() || !crate::crowd::FITS.contains(&self.key) {
            return v;
        }
        let before = k.checked_sub(1).and_then(|p| tiers[p].targets.as_ref());
        let pos = |i: &Item| self.items.iter().position(|x| std::ptr::eq(x, i));
        v.into_iter().filter(|(i, t)| before.and_then(|b| pos(i).map(|p| b[p])).is_none_or(|was| *t > was)).collect()
    }

    /// The items a tier still waits on, the ones you can take now first, then the furthest off.
    /// For a count tier, every item you don't have yet, in the game's order after the ready ones;
    /// for a count at a level (`at`), every item under it that can reach it, the ones closest to
    /// it first after the ready ones, so the cheapest way through comes first.
    pub fn tasks<'a>(&'a self, tier: &Tier) -> Vec<(&'a Item, f64)> {
        let rank = |s: State| match s {
            State::Ready => 0,
            State::Unknown => 1,
            State::Locked => 2,
        };
        let Some(targets) = &tier.targets else {
            if let Some(at) = tier.at {
                let mut v: Vec<(&Item, f64)> = self.items.iter().filter(|i| self.tier_level(i) < at && (i.max <= 0.0 || i.max >= at)).map(|i| (i, at)).collect();
                v.sort_by(|a, b| rank(a.0.state).cmp(&rank(b.0.state)).then(self.tier_level(b.0).total_cmp(&self.tier_level(a.0))));
                return v;
            }
            let mut v: Vec<(&Item, f64)> = self.items.iter().filter(|i| !i.done()).map(|i| (i, i.max.max(1.0))).collect();
            v.sort_by_key(|(i, _)| rank(i.state));
            return v;
        };
        let mut v: Vec<(&Item, f64)> = self.items.iter().zip(targets).filter(|(i, t)| self.tier_level(i) < **t).map(|(i, t)| (i, *t)).collect();
        v.sort_by(|a, b| rank(a.0.state).cmp(&rank(b.0.state)).then((self.tier_level(a.0) / a.1).total_cmp(&(self.tier_level(b.0) / b.1))));
        v
    }

    /// For a mechanic with no max: average, median, highest and lowest level.
    pub fn spread(&self) -> Option<[f64; 4]> {
        let mut v: Vec<f64> = self.items.iter().map(|i| i.level).collect();
        if v.is_empty() {
            return None;
        }
        v.sort_by(f64::total_cmp);
        let avg = v.iter().sum::<f64>() / v.len() as f64;
        let mid = v.len() / 2;
        let median = if v.len().is_multiple_of(2) { (v[mid - 1] + v[mid]) / 2.0 } else { v[mid] };
        Some([avg, median, v[v.len() - 1], v[0]])
    }

    /// Nothing at all yet: no level anywhere, no item found.
    pub fn untouched(&self) -> bool {
        !self.items.is_empty() && self.items.iter().all(|i| i.level <= 0.0)
    }
}

/// Something the ranking expects by your world that you don't have yet.
#[derive(Debug, Clone, PartialEq)]
pub struct Owed {
    pub mechanic: usize,
    /// Index into the mechanic's `items`.
    pub item: usize,
    /// The level that stage asks for.
    pub target: f64,
    /// The tier of the mechanic's ladder that first asks for it (index into `Mechanic::tiers`,
    /// numbered from 1 on its card).
    pub tier: usize,
}

pub struct Review {
    pub name: String,
    pub characters: usize,
    /// The highest world the account has reached, 1 to 7.
    pub world: u8,
    pub mechanics: Vec<Mechanic>,
    /// Mechanics in worlds you've reached with no progress at all: new since you last played,
    /// or skipped. In game order.
    pub not_started: Vec<usize>,
    /// A made-up account with nothing in it, for staff to see how the page reads (the page's
    /// Test account links), not a paste.
    pub test: bool,
    /// What the player buys, as chosen on the paste box.
    pub packs: Packs,
}

impl Review {
    /// `world` is the highest world the account has reached (`checks::world_reached`).
    pub fn new(save: &Save, mechanics: Vec<Mechanic>, world: u8) -> Review {
        Review::with_packs(save, mechanics, world, Packs::Buy)
    }

    /// `new`, for a player who buys what `packs` says: without packs, what only a pack you
    /// don't own gives leaves the items before any tier is built (so the tiers stop at the cap
    /// you can reach), and free to play also takes the Gem Shop off its ranking.
    pub fn with_packs(save: &Save, mechanics: Vec<Mechanic>, world: u8, packs: Packs) -> Review {
        let world = world.clamp(1, 7);
        let mut mechanics = mechanics;
        if packs != Packs::Buy {
            for m in &mut mechanics {
                m.items.retain(|i| !i.pack_only);
                for i in &mut m.items {
                    if i.pack_levels > 0.0 {
                        i.max = (i.max - i.pack_levels).max(0.0);
                        i.level = i.level.min(i.max);
                    }
                }
            }
        }
        for m in &mut mechanics {
            let (now, later): (Vec<Item>, Vec<Item>) = std::mem::take(&mut m.items).into_iter().partition(|i| i.world.is_none_or(|w| w <= world));
            m.items = now;
            m.later = later;
        }
        crate::crowd::with(|table| {
            for m in &mut mechanics {
                // Only when the table has a tier for your world or an earlier one: until there are
                // saves from earlier worlds, an account short of World 7 keeps the game-max ladder.
                let made = crate::crowd::BY_MAX.contains(&m.key).then(|| crate::crowd::by_max(m.key, &m.items, m.world));
                m.crowd = table.get(m.key).or(made.as_ref()).and_then(|b| Ladder::new(b, &m.items)).filter(|l| l.rungs.iter().any(|r| r.world <= world));
            }
        });
        if packs == Packs::Free {
            for m in mechanics.iter_mut().filter(|m| m.key == "gem-shop") {
                m.crowd = None;
            }
        }
        crate::expected::with(|worlds| {
            for m in &mut mechanics {
                m.pace = match &m.crowd {
                    Some(l) => {
                        let (tiers, ws): (Vec<Tier>, Vec<Option<u8>>) = m.crowd_tiers(l).into_iter().unzip();
                        Pace::new(&tiers, &ws, world)
                    }
                    None => worlds.get(m.key).and_then(|ws| Pace::new(&m.tiers(), ws, world)),
                };
            }
        });
        let not_started: Vec<usize> = mechanics.iter().enumerate().filter(|(_, m)| m.world > 0 && m.world <= world && m.untouched()).map(|(i, _)| i).collect();
        Review {
            name: save.names.first().cloned().unwrap_or_default(),
            characters: save.characters(),
            world,
            mechanics,
            not_started,
            test: false,
            packs,
        }
    }

    /// The whole account's share of the maxes the checks read, each mechanic counting once.
    pub fn share(&self) -> f64 {
        let open: Vec<&Mechanic> = self.mechanics.iter().filter(|m| m.world <= self.world && m.total() > 0.0).collect();
        if open.is_empty() { 0.0 } else { open.iter().map(|m| m.share()).sum::<f64>() / open.len() as f64 }
    }

    /// One world's average share, `None` when it has no mechanics with a max.
    pub fn world_share(&self, world: u8) -> Option<f64> {
        world_avg(&self.mechanics, world)
    }

    /// What the ranking (`crowd.txt`) expects by your world that you don't have, from every
    /// mechanic and every world, grouped by the stage that first asks for it (index into
    /// `crowd::STAGES`), earliest stage first. Each item shows once, under the earliest stage
    /// it falls short of, with that stage's level. Inside a stage: what you can do now first,
    /// then the rest, the locked ones last, in game order.
    pub fn should_have(&self) -> Vec<(usize, Vec<Owed>)> {
        let mut by: std::collections::BTreeMap<usize, Vec<Owed>> = std::collections::BTreeMap::new();
        for (mi, m) in self.mechanics.iter().enumerate() {
            let Some(l) = &m.crowd else { continue };
            let mut seen = vec![false; m.items.len()];
            for (k, r) in l.rungs.iter().enumerate().filter(|(_, r)| r.world <= self.world) {
                for (ii, (it, &t)) in m.items.iter().zip(&r.targets).enumerate() {
                    if t > 0.0 && !seen[ii] && m.tier_level(it) < t {
                        seen[ii] = true;
                        by.entry(r.stage).or_default().push(Owed { mechanic: mi, item: ii, target: t, tier: k });
                    }
                }
            }
        }
        let rank = |o: &Owed| match self.mechanics[o.mechanic].items[o.item].state {
            State::Ready => 0,
            State::Unknown => 1,
            State::Locked => 2,
        };
        by.into_iter()
            .map(|(stage, mut v)| {
                v.sort_by_key(|o| (rank(o), self.mechanics[o.mechanic].world.wrapping_sub(1), o.mechanic));
                (stage, v)
            })
            .collect()
    }

    /// Mechanics with a tier overdue for the world you're in, grouped by the world that tier
    /// belongs to, earliest world first (the most overdue), in game order inside each.
    pub fn overdue(&self) -> Vec<(u8, Vec<usize>)> {
        let mut out: Vec<(u8, Vec<usize>)> = Vec::new();
        for (i, m) in self.mechanics.iter().enumerate() {
            let Some(w) = m.pace.as_ref().and_then(|p| p.due.and_then(|k| p.worlds[k])) else { continue };
            match out.iter_mut().find(|(ow, _)| *ow == w) {
                Some((_, v)) => v.push(i),
                None => out.push((w, vec![i])),
            }
        }
        out.sort_by_key(|(w, _)| *w);
        out
    }
}

/// The highest world with any progress in it.
fn world_avg(mechanics: &[Mechanic], world: u8) -> Option<f64> {
    let ms: Vec<f64> = mechanics.iter().filter(|m| m.world == world && m.total() > 0.0).map(Mechanic::share).collect();
    (!ms.is_empty()).then(|| ms.iter().sum::<f64>() / ms.len() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mech(key: &'static str, world: u8, levels: &[(f64, f64)]) -> Mechanic {
        Mechanic {
            key,
            world,
            name: key,
            picture: None,
            count: Count::Levels,
            unit: "levels",
            items: levels.iter().map(|&(l, m)| Item::new("x", None, l, m)).collect(),
            note: None,
            tier_names: None,
            tier_notes: Vec::new(),
            raises: Vec::new(),
            have: None,
            later: Vec::new(),
            open_order: None,
            levels_note: None,
            whole_tiers: false,
            other_side: "Tiers",
            pace: None,
            crowd: None,
        }
    }

    #[test]
    fn without_packs_what_only_a_pack_gives_leaves() {
        let s = Save::parse(r#"{"OptLacc": []}"#).expect("an empty save");
        let mut m = mech("upgrade-vault", 1, &[(505.0, 510.0), (0.0, 1.0)]);
        m.items[0].pack_levels = 10.0;
        m.items[1].pack_only = true;
        let r = Review::with_packs(&s, vec![m.clone()], 7, Packs::NoMore);
        assert_eq!(r.mechanics[0].items.iter().map(|i| (i.level, i.max)).collect::<Vec<_>>(), [(500.0, 500.0)]);
        let r = Review::with_packs(&s, vec![m], 7, Packs::Buy);
        assert_eq!((r.mechanics[0].items.len(), r.mechanics[0].items[0].max), (2, 510.0));
        assert_eq!((Packs::from_code("no-more\r"), Packs::from_code("free"), Packs::from_code("x")), (Packs::NoMore, Packs::Free, Packs::Buy));
    }

    #[test]
    fn open_mechanics_stay_out_of_shares() {
        let mut m = mech("bubbles", 2, &[(5.0, 0.0), (9.0, 0.0)]);
        m.count = Count::Open;
        assert_eq!(m.have(), 14.0);
        assert_eq!(m.total(), 0.0);
        assert_eq!(m.share(), 0.0);
        assert!(!m.done());
    }

    #[test]
    fn tiers_are_per_item_and_skip_repeats() {
        let m = mech("atoms", 3, &[(70.0, 70.0), (17.0, 70.0), (0.0, 70.0)]);
        let t = m.tiers();
        assert_eq!(t.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(), ["Started", "Lv 18", "Lv 35", "Lv 53", "Max, Lv 70"]);
        assert_eq!(t[0].need, 3);
        assert_eq!(t[0].met, 2);
        assert_eq!(t[1].met, 1);
        assert_eq!(m.tasks(&t[1]).len(), 2);
        // Found-once things count how many, whichever: one missing early item doesn't hold
        // the later tiers back, and a tier reads "31 of 32", not "79 of 80".
        let mut d = mech("star-signs", 1, &[(1.0, 1.0); 10]);
        d.count = Count::Done;
        d.items[0].level = 0.0;
        let t = d.tiers();
        assert_eq!(t.len(), 5);
        assert!(t[3].complete() && !t[4].complete());
        assert_eq!((t[4].met, t[4].need), (9, 10));
        assert_eq!(d.tasks(&t[4]).len(), 1);
        // Many items at one milestone come in parts of about 8, in game order, no "All" step.
        let big = mech("research", 7, &[(0.0, 20.0); 20]);
        let t = big.tiers();
        assert_eq!(t[0].label, "Started · 1 of 3");
        assert_eq!(t.len(), 15);
        assert!(t.iter().all(|t| !t.label.starts_with("All")));
        assert_eq!(big.tasks(&t[0]).len(), 7);
        // Count ladders: about five more at a time, at most 12 tiers.
        let mut c = mech("bribes", 1, &[(0.0, 1.0); 40]);
        c.count = Count::Done;
        assert_eq!(c.tiers().len(), 8);
        let mut g = mech("greenstacks", 0, &[(0.0, 1.0); 867]);
        g.count = Count::Done;
        assert_eq!(g.tiers().len(), 12);
    }

    #[test]
    fn counts() {
        let mut m = mech("a", 1, &[(5.0, 10.0), (12.0, 10.0)]);
        assert_eq!(m.have(), 15.0);
        assert_eq!(m.total(), 20.0);
        m.count = Count::Done;
        assert_eq!(m.have(), 1.0);
        assert_eq!(m.total(), 2.0);
    }
}
