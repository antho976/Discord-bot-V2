//! The game's own data: the tables the checks read, as the game defines them, and item names.
//!
//! `data/game.json` is written by `tools/idleon-extract/extract.py` from the Steam install, so
//! after a game update the maxes and names change by running it again. The game keeps every
//! value as text (`"250"`), with underscores for spaces.

use std::collections::HashMap;
use std::sync::LazyLock;

use bot_core::serde_json::{self, Value};
use serde::Deserialize;

static RAW: &str = include_str!("../data/game.json");

#[derive(Deserialize)]
pub struct Game {
    /// When the game files the data came from were installed, "2026-11-02".
    pub updated: String,
    tables: HashMap<String, Value>,
    /// Item ID ("StampA1") to its English name ("Sword Stamp").
    items: HashMap<String, String>,
    /// Monster ID ("mushG") to its name ("Green Mushroom").
    monsters: HashMap<String, String>,
}

pub static GAME: LazyLock<Game> = LazyLock::new(|| serde_json::from_str(RAW).expect("data/game.json is written by the extractor"));

impl Game {
    /// A table's rows. Missing tables are empty, so a check reads as nothing found rather than
    /// failing the whole review.
    pub fn rows(&self, table: &str) -> &[Value] {
        self.tables.get(table).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn item_name(&self, id: &str) -> Option<&str> {
        self.items.get(id).map(String::as_str)
    }

    pub fn monster_name(&self, id: &str) -> Option<&str> {
        self.monsters.get(id).map(String::as_str)
    }
}

/// The game's text as people read it: `Big_Brain_Time` is "Big Brain Time".
pub fn words(s: &str) -> String {
    s.replace('_', " ").trim().to_string()
}

/// A number the game keeps as text. Anything unreadable is 0.
pub fn num(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.trim().parse().unwrap_or(0.0),
        Value::Bool(b) => f64::from(u8::from(*b)),
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_loads() {
        assert!(!GAME.rows("SaltLicks").is_empty());
        assert_eq!(GAME.item_name("StampA1"), Some("Sword Stamp"));
    }
}
