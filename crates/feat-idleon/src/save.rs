//! A player's save, as pasted from IdleonToolbox, Idleon Efficiency or a save tool.
//!
//! Three shapes reach us: the game's own save (`{"StampLv": "[...]", ...}`), IdleonToolbox's
//! copy (`{"data": {...the save...}, "charNames": [...]}`) and Idleon Efficiency's (the save with
//! `playerNames`). Many values inside are themselves JSON written as text, so [`Save::get`]
//! reads those too.

use bot_core::serde_json::{self, Map, Value};

use crate::game::num;

/// Biggest paste accepted. Late-game saves are about 1 MB.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

pub struct Save {
    data: Map<String, Value>,
    /// Character names in save order.
    pub names: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub enum SaveError {
    TooBig,
    NotJson,
    /// JSON, but no Idleon save in it.
    NotASave,
}

impl SaveError {
    pub fn message(&self) -> &'static str {
        match self {
            SaveError::TooBig => "That's bigger than any Idleon save. Paste only the JSON from IdleonToolbox or Idleon Efficiency.",
            SaveError::NotJson => "That isn't JSON. Copy the whole save from IdleonToolbox (Raw data, Copy) or Idleon Efficiency and paste it as it is.",
            SaveError::NotASave => "That's JSON, but not an Idleon save: it has no account options in it.",
        }
    }
}

impl Save {
    pub fn parse(text: &str) -> Result<Save, SaveError> {
        if text.len() > MAX_BYTES {
            return Err(SaveError::TooBig);
        }
        let Ok(Value::Object(mut root)) = serde_json::from_str::<Value>(text.trim()) else {
            return Err(SaveError::NotJson);
        };
        let mut names = string_list(root.get("charNames")).or_else(|| string_list(root.get("playerNames"))).unwrap_or_default();
        let mut data = match root.remove("data") {
            Some(Value::Object(d)) => d,
            Some(_) => return Err(SaveError::NotASave),
            None => root,
        };
        if !data.contains_key("OptLacc") {
            return Err(SaveError::NotASave);
        }
        if names.is_empty() {
            names = string_list(data.get("playerNames")).or_else(|| string_list(data.get("PlayerNames"))).unwrap_or_default();
        }
        // Tidy the save's own JSON-in-text once, so every read after is plain.
        for v in data.values_mut() {
            if let Value::String(s) = v
                && matches!(s.trim_start().as_bytes().first(), Some(b'[' | b'{'))
                && let Ok(parsed) = serde_json::from_str::<Value>(s)
            {
                *v = parsed;
            }
        }
        Ok(Save { data, names })
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.data.get(key)
    }

    /// `key[i]`, `key[i][j]` and so on; objects with numeric keys (`{"0": 13}`) read like lists.
    pub fn at(&self, key: &str, path: &[usize]) -> Option<&Value> {
        let mut v = self.get(key)?;
        for &i in path {
            v = index(v, i)?;
        }
        Some(v)
    }

    /// The numbers in a list or a numbered object, in order. Gaps in a numbered object are 0.
    pub fn numbers(&self, key: &str, path: &[usize]) -> Vec<f64> {
        self.at(key, path).map(numbers).unwrap_or_default()
    }

    /// A value in the account options list, `OptLacc[i]`.
    pub fn option(&self, i: usize) -> f64 {
        self.at("OptLacc", &[i]).map(num).unwrap_or(0.0)
    }

    /// Characters, from the names or, failing that, the per-character keys (`Lv0_3`).
    pub fn characters(&self) -> usize {
        if !self.names.is_empty() {
            return self.names.len();
        }
        (0..12).take_while(|i| self.data.contains_key(&format!("Lv0_{i}"))).count()
    }
}

pub fn index(v: &Value, i: usize) -> Option<&Value> {
    match v {
        Value::Array(a) => a.get(i),
        Value::Object(o) => o.get(&i.to_string()),
        _ => None,
    }
}

pub fn numbers(v: &Value) -> Vec<f64> {
    match v {
        Value::Array(a) => a.iter().map(num).collect(),
        Value::Object(o) => {
            let len = o.keys().filter_map(|k| k.parse::<usize>().ok()).max().map_or(0, |m| m + 1);
            (0..len).map(|i| o.get(&i.to_string()).map(num).unwrap_or(0.0)).collect()
        }
        _ => Vec::new(),
    }
}

fn string_list(v: Option<&Value>) -> Option<Vec<String>> {
    let a = v?.as_array()?;
    let out: Vec<String> = a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_all_three_shapes() {
        let raw = r#"{"OptLacc": "[1, 2, 3]", "SaltLick": "[20, 8]", "playerNames": ["a", "b"]}"#;
        let s = Save::parse(raw).unwrap();
        assert_eq!(s.numbers("SaltLick", &[]), vec![20.0, 8.0]);
        assert_eq!(s.option(2), 3.0);
        assert_eq!(s.names, vec!["a", "b"]);

        let toolbox = r#"{"data": {"OptLacc": [0], "CauldronInfo": [{}, {}, {}, {}, {"0": 13, "2": 4}]}, "charNames": ["x"]}"#;
        let s = Save::parse(toolbox).unwrap();
        assert_eq!(s.numbers("CauldronInfo", &[4]), vec![13.0, 0.0, 4.0]);
        assert_eq!(s.names, vec!["x"]);
    }

    #[test]
    fn refuses_what_isnt_a_save() {
        assert_eq!(Save::parse("hello").err(), Some(SaveError::NotJson));
        assert_eq!(Save::parse("[1]").err(), Some(SaveError::NotJson));
        assert_eq!(Save::parse(r#"{"a": 1}"#).err(), Some(SaveError::NotASave));
        assert_eq!(Save::parse(r#"{"data": 3}"#).err(), Some(SaveError::NotASave));
    }
}
