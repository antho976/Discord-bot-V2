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
/// Most embeds posted per channel per window: one message, with room for the line that sums up
/// the rest. Discord takes about one message a second in a channel, so a raid or a purge past this
/// is summed up instead of queued behind and posted minutes late.
pub const MAX_QUEUED: usize = MAX_PER_MESSAGE - 1;
/// Events past that which are still looked up in the audit log, so the dashboard names their
/// moderator.
const MAX_LOOKUPS: usize = 500;

/// An entry that a moderator may be behind, to look up in Discord's audit log before posting.
#[derive(Debug, Clone, Copy)]
pub struct Who {
    pub guild: u64,
    pub kind: Kind,
    pub target: u64,
    pub channel: Option<u64>,
    pub at: i64,
    /// The stored entry, to name the moderator on.
    pub row: Option<i64>,
}

/// One embed waiting to go out.
#[derive(Debug, Clone)]
pub struct Queued {
    pub embed: Embed,
    pub kind: Kind,
    pub who: Option<Who>,
    /// Taken back before it posted. Kept in place so the positions `waiting` hands out hold.
    pub retracted: bool,
    /// Past the window's limit: kept only to look up who did it, then summed up.
    pub over: bool,
}

#[derive(Default)]
struct Queue {
    items: Vec<Queued>,
    /// Items that will post, retracted ones included.
    shown: usize,
    /// Events summed up, by kind.
    dropped: Vec<(Kind, usize)>,
}

fn count(dropped: &mut Vec<(Kind, usize)>, kind: Kind) {
    match dropped.iter_mut().find(|(k, _)| *k == kind) {
        Some((_, n)) => *n += 1,
        None => dropped.push((kind, 1)),
    }
}

#[derive(Default)]
pub struct Batcher {
    queues: Mutex<HashMap<u64, Queue>>,
}

impl Batcher {
    /// Queue an embed. Returns true when it opened a new window, so the caller schedules the flush.
    pub fn push(&self, channel: u64, kind: Kind, embed: Embed, who: Option<Who>) -> bool {
        let mut queues = self.queues.lock().unwrap();
        let first = !queues.contains_key(&channel);
        let q = queues.entry(channel).or_default();
        if q.shown < MAX_QUEUED {
            q.shown += 1;
            q.items.push(Queued { embed, kind, who, retracted: false, over: false });
        } else if who.is_some() && q.items.len() < MAX_QUEUED + MAX_LOOKUPS {
            q.items.push(Queued { embed: EmbedBuilder::new().build(), kind, who, retracted: false, over: true });
        } else {
            count(&mut q.dropped, kind);
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

    /// Take back an entry that hasn't posted yet and still waits to learn who did it: a leave
    /// that turned out to be a ban. Returns whether one was found.
    pub fn retract(&self, guild: u64, kind: Kind, target: u64) -> bool {
        let mut queues = self.queues.lock().unwrap();
        let found = queues
            .values_mut()
            .flat_map(|q| q.items.iter_mut())
            .find(|item| item.who.is_some_and(|w| w.guild == guild && w.kind == kind && w.target == target));
        match found {
            Some(item) => {
                item.retracted = true;
                item.who = None;
                true
            }
            None => false,
        }
    }

    /// Take everything queued for a channel, in order, with a closing line for the events past the
    /// window's limit.
    pub fn drain(&self, channel: u64) -> Vec<Queued> {
        let Some(mut q) = self.queues.lock().unwrap().remove(&channel) else { return Vec::new() };
        q.items.retain(|item| !item.retracted);
        for item in q.items.iter().filter(|i| i.over) {
            count(&mut q.dropped, item.kind);
        }
        q.items.retain(|item| !item.over);
        if !q.dropped.is_empty() {
            let note = EmbedBuilder::new().color(0x8b8f98).description(more_line(&q.dropped)).build();
            q.items.push(Queued { embed: note, kind: Kind::Join, who: None, retracted: false, over: false });
        }
        q.items
    }
}

/// "…and 37 more joins, 2 more leaves. They're on the dashboard.", most first.
pub fn more_line(dropped: &[(Kind, usize)]) -> String {
    let mut sorted = dropped.to_vec();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.1));
    let parts: Vec<String> = sorted
        .iter()
        .map(|(kind, n)| {
            let (one, many) = nouns(*kind);
            format!("{} more {}", bot_core::text::num(*n as i64), if *n == 1 { one } else { many })
        })
        .collect();
    format!("…and {}. They're on the dashboard.", parts.join(", "))
}

/// A kind of event as a noun, one and many.
fn nouns(kind: Kind) -> (&'static str, &'static str) {
    match kind {
        Kind::MessageDelete => ("deleted message", "deleted messages"),
        Kind::MessageEdit => ("edit", "edits"),
        Kind::BulkDelete => ("bulk delete", "bulk deletes"),
        Kind::Join => ("join", "joins"),
        Kind::Leave => ("leave", "leaves"),
        Kind::Nickname => ("nickname change", "nickname changes"),
        Kind::Roles => ("role change", "role changes"),
        Kind::Ban => ("ban", "bans"),
        Kind::Unban => ("unban", "unbans"),
        Kind::Timeout => ("timeout", "timeouts"),
        Kind::TimeoutEnd => ("timeout removed", "timeouts removed"),
        Kind::Boost => ("boost", "boosts"),
        Kind::VoiceJoin => ("voice join", "voice joins"),
        Kind::VoiceLeave => ("voice leave", "voice leaves"),
        Kind::VoiceMove => ("voice move", "voice moves"),
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
        assert!(b.push(1, Kind::Join, embed("a"), None));
        assert!(!b.push(1, Kind::Join, embed("b"), None));
        assert!(b.push(2, Kind::Join, embed("c"), None));
        assert_eq!(b.drain(1).len(), 2);
        // Once drained, the next push opens a new window.
        assert!(b.push(1, Kind::Join, embed("d"), None));
    }

    #[test]
    fn a_burst_goes_out_ten_at_a_time() {
        let sizes: Vec<usize> = chunk((0..25).map(|i| embed(&format!("event {i}"))).collect()).iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![10, 10, 5]);
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
    fn a_leave_is_taken_back_when_its_ban_comes_late() {
        let b = Batcher::default();
        let who = |kind, target| Some(Who { guild: 1, kind, target, channel: None, at: 0, row: None });
        b.push(5, Kind::Timeout, embed("timeout"), who(Kind::Timeout, 8));
        b.push(5, Kind::Leave, embed("left"), who(Kind::Leave, 9));
        b.push(5, Kind::Join, embed("joined"), None);
        assert!(!b.retract(2, Kind::Leave, 9), "another server");
        assert!(!b.retract(1, Kind::Leave, 8), "another member");
        assert!(b.retract(1, Kind::Leave, 9));
        assert!(!b.retract(1, Kind::Leave, 9), "only once");
        // Positions hold for the moderator lookup still under way.
        assert_eq!(b.waiting(5).iter().map(|(i, w)| (*i, w.target)).collect::<Vec<_>>(), vec![(0, 8)]);
        let left: Vec<Option<String>> = b.drain(5).into_iter().map(|q| q.embed.description).collect();
        assert_eq!(left, vec![Some("timeout".to_owned()), Some("joined".to_owned())]);
    }

    #[test]
    fn a_flood_is_one_message_and_a_line() {
        let b = Batcher::default();
        for i in 0..(MAX_QUEUED + 37) {
            b.push(3, Kind::Join, embed(&format!("join {i}")), None);
        }
        // Bans past the limit are still looked up for the dashboard, then summed up too.
        let who = |target| Some(Who { guild: 1, kind: Kind::Ban, target, channel: None, at: 0, row: None });
        b.push(3, Kind::Ban, embed("ban"), who(8));
        b.push(3, Kind::Ban, embed("ban"), who(9));
        assert_eq!(b.waiting(3).len(), 2);
        let all = embeds(b.drain(3));
        assert_eq!(chunk(all.clone()).len(), 1, "one message per window");
        assert_eq!(all.len(), MAX_QUEUED + 1);
        assert_eq!(all.last().unwrap().description.as_deref(), Some("…and 37 more joins, 2 more bans. They're on the dashboard."));
        assert_eq!(more_line(&[(Kind::Leave, 1)]), "…and 1 more leave. They're on the dashboard.");
    }
}
