//! A player's save, as pasted from IdleonToolbox, Idleon Efficiency or a save tool.
//!
//! Three shapes reach us: the game's own save (`{"StampLv": "[...]", ...}`), IdleonToolbox's
//! copy (`{"data": {...the save...}, "charNames": [...]}`) and Idleon Efficiency's (the save with
//! `playerNames`). Many values inside are themselves JSON written as text, so [`Save::get`]
//! reads those too.

use bot_core::serde_json::{self, Map, Value};

use crate::game::num;

/// More characters than the game allows, so nothing real is cut.
pub const MAX_CHARACTERS: usize = 16;

/// Biggest paste accepted. The biggest real saves seen are 1.4 MB (late World 7). A paste costs
/// up to ~17 times its size in memory while it's read (an 8 MB one of numbers peaked at 136 MB),
/// so the cap keeps about three times the headroom, not more.
pub const MAX_BYTES: usize = 4 * 1024 * 1024;

pub struct Save {
    data: Map<String, Value>,
    /// The game server's live values that IdleonToolbox copies alongside the save
    /// (`serverVars`: cost multipliers like `A_MineCost`). Empty for other sources.
    server: Map<String, Value>,
    /// Character names in save order.
    pub names: Vec<String>,
    /// The companions owned, as `CompanionDB` rows, from IdleonToolbox's copy (`companion.l`,
    /// "row,..." per companion; the game keeps them on its server, not in the save). `None`
    /// when the paste has no companion list.
    pub companions: Option<Vec<usize>>,
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
            SaveError::NotJson => "That isn't JSON. Copy your save again (Copy for support in IdleonToolbox, or Copy Raw JSON in Idleon Efficiency) and paste it as it is.",
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
        let companions = companions(root.remove("companion"));
        let server = match root.remove("serverVars") {
            Some(Value::Object(m)) => m,
            _ => Map::new(),
        };
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
        // The game allows 12 characters; a paste claiming thousands would make every
        // per-character check loop over them.
        names.truncate(MAX_CHARACTERS);
        // Tidy the save's own JSON-in-text once, so every read after is plain.
        for v in data.values_mut() {
            if let Value::String(s) = v
                && matches!(s.trim_start().as_bytes().first(), Some(b'[' | b'{'))
                && let Ok(parsed) = serde_json::from_str::<Value>(s)
            {
                *v = parsed;
            }
        }
        Ok(Save { data, server, names, companions })
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

    /// A server value from the copy (`serverVars`), when it's there and a number.
    pub fn server(&self, key: &str) -> Option<f64> {
        self.server.get(key).map(num).filter(|v| v.is_finite())
    }

    /// A server list from the copy (`serverVars`, like `ArcadeBonuses`: the upgrades the Arcade
    /// sells this update), as a list or as JSON written as text. Read as numbers, at most
    /// [`MAX_SERVER_LIST`] of them, since it comes from a paste.
    pub fn server_list(&self, key: &str) -> Option<Vec<f64>> {
        let parsed;
        let list = match self.server.get(key)? {
            Value::String(s) if s.len() <= MAX_SERVER_TEXT => {
                parsed = serde_json::from_str::<Value>(s).ok()?;
                &parsed
            }
            v => v,
        };
        Some(list.as_array()?.iter().take(MAX_SERVER_LIST).map(num).collect())
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
        (0..MAX_CHARACTERS).take_while(|i| self.data.contains_key(&format!("Lv0_{i}"))).count()
    }
}

pub fn index(v: &Value, i: usize) -> Option<&Value> {
    match v {
        Value::Array(a) => a.get(i),
        Value::Object(o) => o.get(&i.to_string()),
        _ => None,
    }
}

/// Highest index a numbered object is read up to. Real saves stop under 1,000; a paste with a key
/// like "4000000000" would otherwise ask for a list billions long and take the bot down.
const MAX_INDEX: usize = 100_000;

/// Most entries a server list is read to. The game's are short (the Arcade's has 9).
pub const MAX_SERVER_LIST: usize = 100;

/// Longest server list written as text that's parsed; a real one is a few dozen characters.
const MAX_SERVER_TEXT: usize = 10_000;

pub fn numbers(v: &Value) -> Vec<f64> {
    match v {
        Value::Array(a) => a.iter().map(num).collect(),
        Value::Object(o) => {
            // From the entries, so the work is the object's size, not its highest key.
            let entries: Vec<(usize, f64)> = o.iter().filter_map(|(k, v)| k.parse::<usize>().ok().filter(|&i| i < MAX_INDEX).map(|i| (i, num(v)))).collect();
            let mut out = vec![0.0; entries.iter().map(|(i, _)| i + 1).max().unwrap_or(0)];
            for (i, v) in entries {
                out[i] = v;
            }
            out
        }
        _ => Vec::new(),
    }
}

/// Most companions read from a paste; the game has under 200.
const MAX_COMPANIONS: usize = 1_000;

/// The `CompanionDB` rows in IdleonToolbox's `companion.l` ("24,0,0,793,1": row 24), each once.
fn companions(v: Option<Value>) -> Option<Vec<usize>> {
    let Value::Array(list) = v?.as_object_mut()?.remove("l")? else { return None };
    let mut rows: Vec<usize> = list.iter().take(MAX_COMPANIONS).filter_map(|e| e.as_str()?.split(',').next()?.trim().parse().ok()).filter(|&r| r < MAX_COMPANIONS).collect();
    rows.sort_unstable();
    rows.dedup();
    Some(rows)
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

        let toolbox = r#"{"data": {"OptLacc": [0], "CauldronInfo": [{}, {}, {}, {}, {"0": 13, "2": 4}]}, "charNames": ["x"], "serverVars": {"A_MineCost": 1.31}}"#;
        let s = Save::parse(toolbox).unwrap();
        assert_eq!(s.server("A_MineCost"), Some(1.31));
        assert_eq!(s.server("Nope"), None);
        assert_eq!(s.numbers("CauldronInfo", &[4]), vec![13.0, 0.0, 4.0]);
        assert_eq!(s.names, vec!["x"]);
    }

    #[test]
    fn server_lists_read_as_lists_or_text_and_stay_short() {
        let raw = r#"{"data": {"OptLacc": [0]}, "serverVars": {"ArcadeBonuses": [73, 71, "25"], "AsText": "[5, 7]", "Num": 3, "Bad": "[1,", "Long": [PLACEHOLDER]}}"#;
        let long = vec!["1"; 500].join(",");
        let s = Save::parse(&raw.replace("PLACEHOLDER", &long)).unwrap();
        assert_eq!(s.server_list("ArcadeBonuses"), Some(vec![73.0, 71.0, 25.0]));
        assert_eq!(s.server_list("AsText"), Some(vec![5.0, 7.0]));
        assert_eq!(s.server_list("Num"), None);
        assert_eq!(s.server_list("Bad"), None);
        assert_eq!(s.server_list("Nope"), None);
        assert_eq!(s.server_list("Long").map(|v| v.len()), Some(MAX_SERVER_LIST));
        assert_eq!(Save::parse(r#"{"OptLacc": [0]}"#).unwrap().server_list("ArcadeBonuses"), None);
    }

    #[test]
    fn companions_come_from_the_toolbox_copy() {
        let s = Save::parse(r#"{"data": {"OptLacc": [0]}, "companion": {"l": ["24,0,0,793,1", "3,0,0,1,0", "24,1,0,5,0", "x", 7]}}"#).unwrap();
        assert_eq!(s.companions, Some(vec![3, 24]));
        assert_eq!(Save::parse(r#"{"OptLacc": [0]}"#).unwrap().companions, None);
    }

    #[test]
    fn a_huge_numbered_key_is_ignored() {
        let s = Save::parse(r#"{"OptLacc": [0], "X": {"0": 1, "4000000000": 2, "18446744073709551615": 3}}"#).unwrap();
        assert_eq!(s.numbers("X", &[]), vec![1.0]);
    }

    #[test]
    fn refuses_what_isnt_a_save() {
        assert_eq!(Save::parse("hello").err(), Some(SaveError::NotJson));
        assert_eq!(Save::parse("[1]").err(), Some(SaveError::NotJson));
        assert_eq!(Save::parse(r#"{"a": 1}"#).err(), Some(SaveError::NotASave));
        assert_eq!(Save::parse(r#"{"data": 3}"#).err(), Some(SaveError::NotASave));
    }
}
