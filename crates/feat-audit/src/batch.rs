//! Collects log embeds per channel for a short window so a burst goes out as a few messages
//! instead of one message each. In-process only: embeds still waiting when the bot stops are lost.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use bot_core::twilight_model::channel::message::Embed;
use bot_core::twilight_util::builder::embed::EmbedBuilder;

use crate::Kind;

/// How long the first embed of a burst waits for company.
pub const WINDOW: Duration = Duration::from_secs(2);
/// Discord allows 10 embeds in a message.
pub const MAX_PER_MESSAGE: usize = 10;
/// Discord allows 6000 characters across a message's embeds; stay under it.
pub const MAX_CHARS: usize = 5500;
/// Most embeds held per channel per window. A raid or a purge past this is summed up in one line.
pub const MAX_QUEUED: usize = 50;

/// An entry that a moderator may be behind, to look up in Discord's audit log before posting.
#[derive(Debug, Clone, Copy)]
pub struct Who {
    pub guild: u64,
    pub kind: Kind,
    pub target: u64,
    pub channel: Option<u64>,
    pub at: i64,
}

/// One embed waiting to go out.
#[derive(Debug, Clone)]
pub struct Queued {
    pub embed: Embed,
    pub who: Option<Who>,
}

#[derive(Default)]
struct Queue {
    items: Vec<Queued>,
    dropped: usize,
}

#[derive(Default)]
pub struct Batcher {
    queues: Mutex<HashMap<u64, Queue>>,
}

impl Batcher {
    /// Queue an embed. Returns true when it opened a new window, so the caller schedules the flush.
    pub fn push(&self, channel: u64, embed: Embed, who: Option<Who>) -> bool {
        let mut queues = self.queues.lock().unwrap();
        let first = !queues.contains_key(&channel);
        let q = queues.entry(channel).or_default();
        if q.items.len() >= MAX_QUEUED {
            q.dropped += 1;
        } else {
            q.items.push(Queued { embed, who });
        }
        first
    }

    /// The embeds queued for a channel that still wait to learn who did it, by position. Items are
    /// only ever appended until the queue is drained, so the positions hold.
    pub fn waiting(&self, channel: u64) -> Vec<(usize, Who)> {
        let queues = self.queues.lock().unwrap();
        let Some(q) = queues.get(&channel) else { return Vec::new() };
        q.items.iter().enumerate().filter_map(|(i, item)| item.who.map(|w| (i, w))).collect()
    }

    /// Change a queued embed in place and stop it waiting.
    pub fn settle(&self, channel: u64, index: usize, f: impl FnOnce(&mut Embed)) {
        if let Some(item) = self.queues.lock().unwrap().get_mut(&channel).and_then(|q| q.items.get_mut(index)) {
            f(&mut item.embed);
            item.who = None;
        }
    }

    /// Take everything queued for a channel, in order, with a closing note when some were dropped.
    pub fn drain(&self, channel: u64) -> Vec<Queued> {
        let Some(mut q) = self.queues.lock().unwrap().remove(&channel) else { return Vec::new() };
        if q.dropped > 0 {
            let note = EmbedBuilder::new()
                .color(0x8b8f98)
                .description(format!("{} more events were not posted here. They are on the dashboard.", q.dropped))
                .build();
            q.items.push(Queued { embed: note, who: None });
        }
        q.items
    }
}

/// Characters Discord counts toward the 6000 limit.
pub fn embed_len(e: &Embed) -> usize {
    let n = |s: &Option<String>| s.as_ref().map_or(0, |s| s.chars().count());
    n(&e.title)
        + n(&e.description)
        + e.footer.as_ref().map_or(0, |f| f.text.chars().count())
        + e.author.as_ref().map_or(0, |a| a.name.chars().count())
        + e.fields.iter().map(|f| f.name.chars().count() + f.value.chars().count()).sum::<usize>()
}

/// Group embeds into messages of at most 10 embeds and about 5500 characters, keeping order.
pub fn chunk(embeds: Vec<Embed>) -> Vec<Vec<Embed>> {
    let mut out: Vec<Vec<Embed>> = Vec::new();
    let mut size = 0;
    for e in embeds {
        let len = embed_len(&e);
        match out.last_mut() {
            Some(last) if last.len() < MAX_PER_MESSAGE && size + len <= MAX_CHARS => {
                last.push(e);
                size += len;
            }
            _ => {
                out.push(vec![e]);
                size = len;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embed(text: &str) -> Embed {
        EmbedBuilder::new().description(text).build()
    }

    fn embeds(q: Vec<Queued>) -> Vec<Embed> {
        q.into_iter().map(|q| q.embed).collect()
    }

    #[test]
    fn only_the_first_push_opens_a_window() {
        let b = Batcher::default();
        assert!(b.push(1, embed("a"), None));
        assert!(!b.push(1, embed("b"), None));
        assert!(b.push(2, embed("c"), None));
        assert_eq!(b.drain(1).len(), 2);
        // Once drained, the next push opens a new window.
        assert!(b.push(1, embed("d"), None));
    }

    #[test]
    fn a_burst_goes_out_ten_at_a_time() {
        let b = Batcher::default();
        for i in 0..25 {
            b.push(7, embed(&format!("event {i}")), None);
        }
        let sizes: Vec<usize> = chunk(embeds(b.drain(7))).iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![10, 10, 5]);
        assert!(b.drain(7).is_empty());
    }

    #[test]
    fn long_embeds_split_before_the_character_limit() {
        let long = "x".repeat(2000);
        let messages = chunk((0..5).map(|_| embed(&long)).collect());
        assert_eq!(messages.iter().map(Vec::len).collect::<Vec<_>>(), vec![2, 2, 1]);
        for m in &messages {
            assert!(m.iter().map(embed_len).sum::<usize>() <= MAX_CHARS);
        }
    }

    #[test]
    fn a_flood_is_summed_up() {
        let b = Batcher::default();
        for i in 0..(MAX_QUEUED + 7) {
            b.push(3, embed(&format!("event {i}")), None);
        }
        let all = embeds(b.drain(3));
        assert_eq!(all.len(), MAX_QUEUED + 1);
        assert!(all.last().unwrap().description.as_deref().unwrap().starts_with("7 more events"));
    }
}
