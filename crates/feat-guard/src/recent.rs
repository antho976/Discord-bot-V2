//! A small in-memory window of each member's recent messages: to spot the same message posted
//! in several channels at once, and to know what to delete when someone is caught.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};

/// How long message ids are kept, so a catch can delete the member's recent messages.
pub const KEEP_MS: i64 = 10 * 60 * 1000;
/// The window for "same message in several channels".
pub const REPEAT_MS: i64 = 30 * 1000;
const PER_USER: usize = 60;

#[derive(Debug, Clone, Copy)]
struct Seen {
    at: i64,
    channel: u64,
    message: u64,
    print: Option<u64>,
}

#[derive(Default)]
pub struct Recent {
    users: HashMap<(u64, u64), VecDeque<Seen>>,
    /// When each member was last caught, so a burst only raises one alert.
    caught: HashMap<(u64, u64), i64>,
}

/// What makes two messages "the same": the text with case and spacing evened out, plus the
/// attachments' names and sizes. None for messages too short to mean anything ("hi", "lol").
pub fn fingerprint(text: &str, attachments: &[(String, u64)]) -> Option<u64> {
    let norm: String = text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    if norm.chars().count() < 12 && attachments.is_empty() {
        return None;
    }
    let mut h = DefaultHasher::new();
    norm.hash(&mut h);
    for (name, size) in attachments {
        name.hash(&mut h);
        size.hash(&mut h);
    }
    Some(h.finish())
}

impl Recent {
    /// Note a message. Returns how many distinct channels got the same message within the
    /// repeat window, this one included (1 when it's new or has no fingerprint).
    pub fn record(&mut self, guild: u64, user: u64, channel: u64, message: u64, print: Option<u64>, now: i64) -> usize {
        let list = self.users.entry((guild, user)).or_default();
        while list.front().is_some_and(|s| now - s.at > KEEP_MS) || list.len() >= PER_USER {
            list.pop_front();
        }
        list.push_back(Seen { at: now, channel, message, print });
        let Some(print) = print else { return 1 };
        let mut channels: Vec<u64> = list
            .iter()
            .filter(|s| s.print == Some(print) && now - s.at <= REPEAT_MS)
            .map(|s| s.channel)
            .collect();
        channels.sort_unstable();
        channels.dedup();
        channels.len()
    }

    /// Every message the member sent in the last ten minutes, as `(channel, message)`, forgotten
    /// once taken.
    pub fn take(&mut self, guild: u64, user: u64, now: i64) -> Vec<(u64, u64)> {
        self.users
            .remove(&(guild, user))
            .unwrap_or_default()
            .into_iter()
            .filter(|s| now - s.at <= KEEP_MS)
            .map(|s| (s.channel, s.message))
            .collect()
    }

    /// True when the member was caught in the last two minutes; otherwise notes this catch.
    pub fn already_caught(&mut self, guild: u64, user: u64, now: i64) -> bool {
        let key = (guild, user);
        if self.caught.get(&key).is_some_and(|t| now - t < 120_000) {
            return true;
        }
        self.caught.insert(key, now);
        false
    }

    /// Drop everything older than the window. Called once a minute.
    pub fn prune(&mut self, now: i64) {
        self.users.retain(|_, list| {
            list.retain(|s| now - s.at <= KEEP_MS);
            !list.is_empty()
        });
        self.caught.retain(|_, t| now - *t < 120_000);
    }

    #[cfg(test)]
    pub fn tracked(&self) -> usize {
        self.users.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: u64 = 1;
    const U: u64 = 2;

    #[test]
    fn same_message_in_three_channels_counts() {
        let mut r = Recent::default();
        let p = fingerprint("Check this out https://x.y/z", &[]);
        assert_eq!(r.record(G, U, 10, 100, p, 0), 1);
        assert_eq!(r.record(G, U, 10, 101, p, 1_000), 1, "same channel twice is still one channel");
        assert_eq!(r.record(G, U, 11, 102, p, 5_000), 2);
        assert_eq!(r.record(G, U, 12, 103, p, 9_000), 3);
        // Another member posting the same text doesn't count toward this one.
        assert_eq!(r.record(G, 3, 13, 104, p, 9_500), 1);
    }

    #[test]
    fn repeats_outside_the_window_dont_count() {
        let mut r = Recent::default();
        let p = fingerprint("a long enough message", &[]);
        r.record(G, U, 10, 100, p, 0);
        r.record(G, U, 11, 101, p, 20_000);
        assert_eq!(r.record(G, U, 12, 102, p, 45_000), 2);
    }

    #[test]
    fn case_and_spacing_dont_matter_but_short_messages_do() {
        assert_eq!(fingerprint("Free  stuff here guys", &[]), fingerprint("free stuff here GUYS", &[]));
        assert_eq!(fingerprint("hi", &[]), None);
        assert!(fingerprint("", &[("image0.png".into(), 1234)]).is_some());
        let mut r = Recent::default();
        for (i, ch) in [10, 11, 12].into_iter().enumerate() {
            assert_eq!(r.record(G, U, ch, i as u64, None, i as i64), 1);
        }
    }

    #[test]
    fn take_returns_recent_ids_and_prune_forgets() {
        let mut r = Recent::default();
        r.record(G, U, 10, 100, None, 0);
        r.record(G, U, 11, 101, None, 60_000);
        r.record(G, 9, 11, 102, None, 60_000);
        assert_eq!(r.take(G, U, KEEP_MS + 1_000), vec![(11, 101)]);
        assert!(r.take(G, U, KEEP_MS + 1_000).is_empty());
        r.prune(60_000 + KEEP_MS + 1);
        assert_eq!(r.tracked(), 0);
    }

    #[test]
    fn one_alert_per_burst() {
        let mut r = Recent::default();
        assert!(!r.already_caught(G, U, 0));
        assert!(r.already_caught(G, U, 30_000));
        assert!(!r.already_caught(G, U, 200_000));
    }
}
