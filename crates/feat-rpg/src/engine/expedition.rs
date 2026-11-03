//! Expeditions: idle runs worked out when you come back, from what you chose when you left.

use serde::{Deserialize, Serialize};

use super::Rng;
use crate::data::{self, Danger, Region};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Plan {
    pub region: String,
    pub danger: Danger,
    pub hours: u32,
    pub supply: Option<String>,
}

impl Plan {
    pub fn region(&self) -> &'static Region {
        data::region(&self.region).unwrap_or(&data::REGIONS[0])
    }

    /// Power the run needs.
    pub fn needs(&self) -> i64 {
        (self.region().power as f64 * self.danger.hard()) as i64
    }

    /// Your Power with the supply's bonus.
    pub fn effective(&self, power: i64) -> i64 {
        let bonus = match self.supply.as_deref() {
            Some("rations") => 0.20,
            Some("tonic") if self.region == "barrow" => 0.40,
            _ => 0.0,
        };
        (power as f64 * (1.0 + bonus)) as i64
    }
}

/// Server-wide and personal multipliers on rewards, in percent (100 = none).
#[derive(Debug, Clone, Copy)]
pub struct Rates {
    pub xp: i64,
    pub loot: i64,
    pub gold: i64,
}

impl Default for Rates {
    fn default() -> Self {
        Self { xp: 100, loot: 100, gold: 100 }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Report {
    pub region: String,
    pub danger: String,
    pub hours: u32,
    /// Hours actually spent: less than planned when it went badly.
    pub spent: f64,
    pub wounded: bool,
    pub items: Vec<(String, i64)>,
    pub gold: i64,
    pub xp: i64,
    pub tribute: i64,
    /// A boss fact learned, if any.
    pub intel: Option<String>,
    pub story: Vec<(String, String)>,
    /// Hours since the region's rare drop, carried to the next run.
    pub pity: u32,
}

/// What a forecast shows before you send a run out.
pub struct Forecast {
    pub items: Vec<(String, i64)>,
    pub gold: i64,
    pub xp: i64,
    /// Power against what it needs.
    pub ratio: f64,
    pub intel_chance: f64,
    pub tribute: i64,
}

pub fn forecast(plan: &Plan, power: i64, scouting: i64, rates: Rates) -> Forecast {
    let r = plan.region();
    let h = plan.hours as f64;
    let ratio = plan.effective(power) as f64 / plan.needs().max(1) as f64;
    let done = if ratio >= 1.0 { 1.0 } else { ratio.powi(2).max(0.25) };
    let mult = plan.danger.loot() * done * h;
    let mut items: Vec<(String, i64)> = r.drops.iter().map(|(id, per)| (id.to_string(), (per * mult * rates.loot as f64 / 100.0).round() as i64)).collect();
    let rare = (r.rare.1 * h * done * rare_mult(plan)).max(h * done / r.rare.2 as f64);
    items.push((r.rare.0.to_string(), rare.round().max(0.0) as i64));
    Forecast {
        items,
        gold: (r.gold * mult * rates.gold as f64 / 100.0).round() as i64,
        xp: (r.xp * mult * rates.xp as f64 / 100.0).round() as i64,
        ratio,
        intel_chance: 1.0 - (1.0 - r.intel * (1.0 + scouting as f64 / 100.0)).powf(h * done),
        tribute: (data::TRIBUTE_PER_HOUR as f64 * h * done) as i64,
    }
}

fn rare_mult(plan: &Plan) -> f64 {
    if plan.supply.as_deref() == Some("bait") { 1.5 } else { 1.0 }
}

const QUIET: &[&str] = &[
    "Quiet going. Your pack got heavier.",
    "Nothing stirred. You worked steadily.",
    "Rain, then sun. Good hauling either way.",
    "You found an old cache, mostly picked over.",
];

fn trouble(region: &str) -> &'static [&'static str] {
    match region {
        "shore" => &["A nixie sang from the rocks. You covered your ears and pushed on.", "Harpies circled. You kept to the cliffs."],
        "wolfwood" => &["Wolves shadowed you through the pines.", "A satyr band tried an ambush. You drove them off."],
        _ => &["Draugr rose near the barrows. You held them off.", "A wight's cold breath chased you between the mounds."],
    }
}

/// Work out a run from its plan. `facts_known` are the boss facts the player already has, so
/// intel always finds something new. `pity` is hours since the region's rare drop.
pub fn run(plan: &Plan, power: i64, scouting: i64, facts_known: &[String], pity: u32, rates: Rates, seed: u64) -> Report {
    let mut rng = Rng::new(seed);
    let r = plan.region();
    let ratio = plan.effective(power) as f64 / plan.needs().max(1) as f64;
    let done = if ratio >= 1.0 { 1.0 } else { ratio.powi(2).max(0.25) };
    let spent = plan.hours as f64 * done;
    let full_hours = spent.ceil() as u32;
    let mut acc: Vec<(String, f64)> = r.drops.iter().map(|(id, _)| (id.to_string(), 0.0)).collect();
    let mut rare_n = 0i64;
    let mut pity = pity;
    let mut intel: Option<String> = None;
    let mut story = Vec::new();
    let loot = plan.danger.loot() * rates.loot as f64 / 100.0;
    let rare_word = data::item(r.rare.0).map(|i| i.name).unwrap_or("something rare");
    for hour in 0..full_hours {
        let part = (spent - hour as f64).min(1.0);
        for ((_, per), (_, total)) in r.drops.iter().zip(acc.iter_mut()) {
            *total += per * loot * part * rng.range(0.9, 1.1);
        }
        pity += 1;
        let mut line: Option<String> = None;
        if rng.chance(r.rare.1 * part * rare_mult(plan) * loot.min(2.0)) || pity >= r.rare.2 {
            rare_n += 1;
            pity = 0;
            line = Some(format!("Found {} {rare_word}.", if rng.chance(0.5) { "a fine" } else { "some" }));
        }
        let p_intel = r.intel * part * (1.0 + scouting as f64 / 100.0);
        if intel.is_none() && rng.chance(p_intel) {
            if let Some(f) = data::FACTS.iter().find(|f| !facts_known.iter().any(|k| k == *f)) {
                intel = Some(f.to_string());
                line = Some("Tracks of something huge near the old mounds. You watched it from a distance.".into());
            }
        }
        if line.is_none() && ratio < 1.4 && rng.chance(0.45) {
            line = Some(rng.pick(trouble(&r.id)).to_string());
        }
        story.push((format!("Hour {}", hour + 1), line.unwrap_or_else(|| rng.pick(QUIET).to_string())));
    }
    if done < 1.0 {
        story.push(("Back early".into(), "It was more than you could handle. You came back hurt with part of the haul.".into()));
    }
    // Long runs: keep the eventful hours and the last one.
    if story.len() > 5 {
        let last = story.len() - 1;
        let mut keep: Vec<(String, String)> = story
            .iter()
            .enumerate()
            .filter(|(i, (_, l))| *i == last || !QUIET.contains(&l.as_str()))
            .map(|(_, s)| s.clone())
            .collect();
        if keep.len() > 5 {
            keep.drain(0..keep.len() - 5);
        }
        story = keep;
    }
    let mut items: Vec<(String, i64)> = acc.into_iter().map(|(id, t)| (id, t.round() as i64)).filter(|(_, n)| *n > 0).collect();
    if rare_n > 0 {
        items.push((r.rare.0.to_string(), rare_n));
    }
    Report {
        region: r.id.into(),
        danger: plan.danger.id().into(),
        hours: plan.hours,
        spent,
        wounded: done < 1.0,
        items,
        gold: (r.gold * plan.danger.loot() * spent * rng.range(0.9, 1.1) * rates.gold as f64 / 100.0).round() as i64,
        xp: (r.xp * plan.danger.loot() * spent * rates.xp as f64 / 100.0).round() as i64,
        tribute: (data::TRIBUTE_PER_HOUR as f64 * spent) as i64,
        intel,
        story,
        pity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(region: &str, danger: Danger, hours: u32) -> Plan {
        Plan { region: region.into(), danger, hours, supply: None }
    }

    #[test]
    fn runs_replay_and_stay_close_to_the_forecast() {
        let p = plan("wolfwood", Danger::Hard, 8);
        let a = run(&p, 400, 0, &[], 0, Rates::default(), 42);
        let b = run(&p, 400, 0, &[], 0, Rates::default(), 42);
        assert_eq!(a, b);
        let f = forecast(&p, 400, 0, Rates::default());
        for (id, n) in &f.items {
            if id == "fang" {
                continue;
            }
            let got = a.items.iter().find(|(i, _)| i == id).map(|(_, n)| *n).unwrap_or(0);
            assert!((got - n).abs() as f64 <= (*n as f64 * 0.12).max(2.0), "{id}: forecast {n}, got {got}");
        }
    }

    #[test]
    fn under_powered_runs_come_back_early() {
        let r = run(&plan("barrow", Danger::Hard, 4), 150, 0, &[], 0, Rates::default(), 1);
        assert!(r.wounded);
        assert!(r.spent < 4.0);
        let r = run(&plan("barrow", Danger::Hard, 4), 400, 0, &[], 0, Rates::default(), 1);
        assert!(!r.wounded);
    }

    #[test]
    fn rare_drop_is_guaranteed_by_pity() {
        let p = plan("shore", Danger::Normal, 8);
        for seed in 0..50 {
            let r = run(&p, 500, 0, &[], 0, Rates::default(), seed);
            assert!(r.items.iter().any(|(i, n)| i == "amber" && *n >= 1), "seed {seed}: no amber in 8 hours");
        }
    }

    #[test]
    fn intel_never_repeats_a_known_fact() {
        let known: Vec<String> = data::FACTS[..3].iter().map(|s| s.to_string()).collect();
        for seed in 0..200 {
            let r = run(&plan("barrow", Danger::Normal, 8), 500, 50, &known, 0, Rates::default(), seed);
            if let Some(f) = r.intel {
                assert_eq!(f, "roar");
            }
        }
    }
}
