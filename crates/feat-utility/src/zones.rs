//! Timezone helpers: the IANA name list for autocomplete, parsing what people type, and the
//! local time and offset of a zone. The tz database is bundled with `jiff`, so none of this
//! depends on the host.

use std::sync::OnceLock;

use bot_core::jiff::Timestamp;
use bot_core::jiff::tz::{self, TimeZone};

/// Region names people would pick, sorted: `Europe/Warsaw`, not the legacy `US/Eastern` or
/// the `posix/` and `right/` copies. `UTC` is kept.
pub fn names() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut all: Vec<String> = tz::db()
            .available()
            .map(|n| n.as_str().to_owned())
            .filter(|n| {
                n == "UTC"
                    || (n.contains('/')
                        && !["posix/", "right/", "Etc/", "SystemV/", "US/", "Brazil/", "Canada/", "Chile/", "Mexico/"]
                            .iter()
                            .any(|p| n.starts_with(p)))
            })
            .collect();
        all.sort();
        all.dedup();
        all
    })
}

/// The canonical IANA name for what someone typed, or `None` when it isn't a zone.
/// Case and spaces don't matter: "europe/warsaw" and "America/New York" both work.
pub fn normalize(input: &str) -> Option<String> {
    let typed = input.trim().replace(' ', "_");
    if typed.is_empty() {
        return None;
    }
    if let Some(hit) = names().iter().find(|n| n.eq_ignore_ascii_case(&typed)) {
        return Some(hit.clone());
    }
    // Names outside the picker list (`Etc/GMT+2`, `US/Eastern`) still work when typed in full.
    let zone = TimeZone::get(&typed).ok()?;
    zone.iana_name().map(str::to_owned)
}

/// Up to `limit` zone names matching what's been typed so far, best first. A match on the
/// city ("warsaw", "new york") beats a match on the region.
pub fn search(query: &str, limit: usize) -> Vec<&'static str> {
    let q = query.trim().to_lowercase().replace(' ', "_");
    let mut scored: Vec<(u8, &'static str)> = names()
        .iter()
        .filter_map(|name| {
            let lower = name.to_lowercase();
            let city = lower.rsplit('/').next().unwrap_or(&lower);
            let score = if q.is_empty() {
                3
            } else if city.starts_with(&q) {
                0
            } else if lower.starts_with(&q) {
                1
            } else if lower.contains(&q) {
                2
            } else {
                return None;
            };
            Some((score, name.as_str()))
        })
        .collect();
    scored.sort();
    scored.into_iter().take(limit).map(|(_, n)| n).collect()
}

/// "UTC", "UTC+2", "UTC-3:30".
pub fn offset_label(secs: i32) -> String {
    if secs == 0 {
        return "UTC".into();
    }
    let sign = if secs < 0 { '-' } else { '+' };
    let secs = secs.abs();
    let (h, m) = (secs / 3600, (secs % 3600) / 60);
    if m == 0 { format!("UTC{sign}{h}") } else { format!("UTC{sign}{h}:{m:02}") }
}

/// What the clock reads in a zone right now.
#[derive(Debug, Clone, PartialEq)]
pub struct Local {
    /// "Thu 14:32"
    pub time: String,
    /// "UTC+2"
    pub offset: String,
    pub offset_secs: i32,
}

pub fn local(zone: &str, now: Timestamp) -> Option<Local> {
    let tz = TimeZone::get(zone).ok()?;
    let z = now.to_zoned(tz);
    let offset_secs = z.offset().seconds();
    Some(Local { time: z.strftime("%a %H:%M").to_string(), offset: offset_label(offset_secs), offset_secs })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn list_has_regions_and_no_noise() {
        let n = names();
        assert!(n.iter().any(|z| z == "Europe/Warsaw"));
        assert!(n.iter().any(|z| z == "UTC"));
        assert!(!n.iter().any(|z| z.starts_with("posix/") || z.starts_with("Etc/")));
    }

    #[test]
    fn normalize_accepts_case_and_spaces() {
        assert_eq!(normalize("europe/warsaw").as_deref(), Some("Europe/Warsaw"));
        assert_eq!(normalize("  America/New York ").as_deref(), Some("America/New_York"));
        assert_eq!(normalize("utc").as_deref(), Some("UTC"));
        assert_eq!(normalize("Mars/Olympus"), None);
        assert_eq!(normalize(""), None);
    }

    #[test]
    fn search_prefers_the_city() {
        let hits = search("new york", 5);
        assert_eq!(hits.first(), Some(&"America/New_York"));
        let hits = search("war", 5);
        assert_eq!(hits.first(), Some(&"Europe/Warsaw"));
        assert!(search("zzzz", 5).is_empty());
        assert_eq!(search("", 25).len(), 25);
    }

    #[test]
    fn offsets_read_naturally() {
        assert_eq!(offset_label(0), "UTC");
        assert_eq!(offset_label(7200), "UTC+2");
        assert_eq!(offset_label(-10_800), "UTC-3");
        assert_eq!(offset_label(19_800), "UTC+5:30");
        assert_eq!(offset_label(-12_600), "UTC-3:30");
    }

    #[test]
    fn local_time_follows_the_zone_and_dst() {
        // 12:00 UTC on a January Thursday and a July Thursday.
        let winter = local("Europe/Warsaw", at("2026-01-15T12:00:00Z")).unwrap();
        assert_eq!((winter.time.as_str(), winter.offset.as_str()), ("Thu 13:00", "UTC+1"));
        let summer = local("Europe/Warsaw", at("2026-07-16T12:00:00Z")).unwrap();
        assert_eq!((summer.time.as_str(), summer.offset.as_str()), ("Thu 14:00", "UTC+2"));
        let india = local("Asia/Kolkata", at("2026-01-15T12:00:00Z")).unwrap();
        assert_eq!((india.time.as_str(), india.offset.as_str()), ("Thu 17:30", "UTC+5:30"));
        assert!(local("Nope/Nope", at("2026-01-15T12:00:00Z")).is_none());
    }
}
