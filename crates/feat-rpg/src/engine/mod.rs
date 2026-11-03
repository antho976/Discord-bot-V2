//! The game's rules as plain functions: no Discord, no database. Everything random takes a
//! seeded [`Rng`], so a fight or an expedition replays exactly from its seed.

pub mod combat;
pub mod expedition;
pub mod loot;
pub mod progress;
pub mod sim;

use crate::data::{self, God, Skill};

/// SplitMix64: small, fast and the same on every machine and build.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// In `[0, 1)`.
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// In `[lo, hi)`.
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.f64()
    }
    pub fn chance(&mut self, p: f64) -> bool {
        self.f64() < p
    }
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next_u64() % items.len() as u64) as usize]
    }
}

/// What the rules need to know about a player.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Hero {
    pub main: God,
    pub lesser: Option<God>,
    /// Bond rank with each god, in [`data::GODS`] order.
    pub bonds: [u32; 3],
    pub power: i64,
    /// Extra scouting chance from gear, in percent.
    pub scouting: i64,
    /// What gear traits add.
    #[serde(default)]
    pub mods: data::Mods,
}

impl Hero {
    pub fn bond(&self, g: God) -> u32 {
        self.bonds[g as usize]
    }

    pub fn stats(&self) -> (i64, i64) {
        let (hp, atk) = data::stats(self.power, self.main);
        ((hp as f64 * (1.0 + self.mods.hp)).round() as i64, atk)
    }

    /// Skills this hero can use: the patron's first, the second at bond 2, the pair skill
    /// with a lesser vow and bond 1 with that god.
    pub fn skills(&self) -> Vec<Skill> {
        let [first, second] = self.main.skills();
        let mut out = vec![first];
        if self.bond(self.main) >= 2 {
            out.push(second);
        }
        if let Some(l) = self.lesser
            && self.bond(l) >= 1
            && let Some(s) = data::pair_skill(self.main, l)
        {
            out.push(s);
        }
        out
    }

    /// How much stronger skills are from the main bond: 5% per rank past 1.
    pub fn skill_mult(&self) -> f64 {
        1.0 + 0.05 * self.bond(self.main).saturating_sub(1) as f64
    }
}
