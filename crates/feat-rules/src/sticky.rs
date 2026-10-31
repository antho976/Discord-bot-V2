//! Sticky messages: after enough new messages (or a quiet spell), post the message again at the
//! bottom and delete the previous copy. Reposting waits for a short lull so a busy chat doesn't
//! get a copy between every two messages.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bot_core::rusqlite::{Connection, OptionalExtension, params};
use bot_core::twilight_model::id::Id;
use bot_core::{Ctx, now_ms};

use crate::{KEY, Rule, Settings};

/// Wait for this long without a message before reposting...
pub const LULL_MS: i64 = 3_000;
/// ...but never more than this after the repost became due.
pub const MAX_WAIT_MS: i64 = 30_000;

/// One channel's count since the sticky message was last posted.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    /// New messages since the last copy.
    pub new: i64,
    /// When the latest of them arrived.
    pub last: i64,
    /// When `new` reached the repost count.
    pub due_since: Option<i64>,
    /// A task is already waiting to repost.
    pub waiting: bool,
}

#[derive(Debug, PartialEq)]
pub enum Next {
    Post,
    Wait(i64),
    Idle,
}

impl State {
    /// A new message arrived at `now`.
    pub fn note(&mut self, now: i64, every: i64) {
        self.new += 1;
        self.last = now;
        if self.due_since.is_none() && self.new >= every.max(1) {
            self.due_since = Some(now);
        }
    }

    /// What to do at `now`. `quiet_ms` reposts after a quiet spell even below the count; 0 is
    /// off.
    pub fn next(&self, now: i64, quiet_ms: i64) -> Next {
        if self.new == 0 {
            return Next::Idle;
        }
        let at = match self.due_since {
            Some(due) => (self.last + LULL_MS).min(due + MAX_WAIT_MS),
            None if quiet_ms > 0 => self.last + quiet_ms,
            None => return Next::Idle,
        };
        if now >= at { Next::Post } else { Next::Wait(at - now) }
    }

    pub fn posted(&mut self) {
        self.new = 0;
        self.due_since = None;
    }
}

pub type States = Arc<Mutex<HashMap<u64, State>>>;

/// Count a message in a channel with a sticky message, and start a waiter if a repost is coming.
pub fn on_message(states: &States, ctx: &Ctx, guild: u64, rule: &Rule) {
    let channel = rule.channel;
    let start = {
        let mut all = states.lock().unwrap();
        let st = all.entry(channel).or_default();
        st.note(now_ms(), rule.sticky_every);
        let start = !st.waiting && st.next(now_ms(), rule.sticky_quiet_mins * 60_000) != Next::Idle;
        if start {
            st.waiting = true;
        }
        start
    };
    if start {
        let (states, ctx) = (states.clone(), ctx.clone());
        bot_core::tokio::spawn(async move { wait_and_post(states, ctx, guild, channel).await });
    }
}

/// Sleep until the repost is due, post, and keep going while messages keep it due. Settings are
/// read each time round, so a removed rule or a changed count takes effect.
async fn wait_and_post(states: States, ctx: Ctx, guild: u64, channel: u64) {
    loop {
        let rule = ctx.settings::<Settings>(guild, KEY).ok().and_then(|s| s.rule(channel).cloned()).filter(Rule::has_sticky);
        let next = {
            let mut all = states.lock().unwrap();
            let st = all.entry(channel).or_default();
            let next = match &rule {
                Some(r) if ctx.enabled(guild, KEY) => st.next(now_ms(), r.sticky_quiet_mins * 60_000),
                _ => Next::Idle,
            };
            match next {
                Next::Idle => *st = State::default(),
                Next::Post => st.posted(),
                Next::Wait(_) => {}
            }
            next
        };
        match (next, rule) {
            (Next::Wait(ms), _) => bot_core::tokio::time::sleep(Duration::from_millis(ms.max(50) as u64)).await,
            (Next::Post, Some(r)) => post(&ctx, guild, channel, &r.sticky).await,
            _ => return,
        }
    }
}

/// The current copy of a channel's sticky message.
pub fn current(conn: &Connection, channel: u64) -> bot_core::rusqlite::Result<Option<u64>> {
    conn.query_row("SELECT message_id FROM rules_sticky WHERE channel_id = ?1", [channel as i64], |r| r.get::<_, i64>(0).map(|m| m as u64)).optional()
}

fn set_current(conn: &Connection, guild: u64, channel: u64, message: u64, now: i64) -> bot_core::rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO rules_sticky (channel_id, guild_id, message_id, posted_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (channel_id) DO UPDATE SET message_id = excluded.message_id, posted_at = excluded.posted_at",
        params![channel as i64, guild as i64, message as i64, now],
    )?;
    Ok(())
}

/// Post the sticky message and delete the copy before it.
pub async fn post(ctx: &Ctx, guild: u64, channel: u64, text: &str) {
    let old = ctx.db.with(|c| current(c, channel)).ok().flatten();
    match ctx.say(channel, text).await {
        Ok(id) => {
            if let Err(e) = ctx.db.with(|c| set_current(c, guild, channel, id, now_ms())) {
                bot_core::tracing::warn!(guild, channel, error = %e, "couldn't store the sticky message");
            }
            ctx.count(guild, "rules_sticky", 1);
            if let Some(old) = old {
                let _ = ctx.http.delete_message(Id::new(channel), Id::new(old)).await;
            }
        }
        Err(e) => {
            bot_core::tracing::warn!(guild, channel, error = %e, "couldn't post a sticky message");
            ctx.notify_once(guild, KEY, "sticky", format!("A sticky message couldn't be posted. {}", bot_core::discord::why_refused_any(&e)), Some(format!("/g/{guild}/rules")), Duration::from_secs(86_400));
        }
    }
}

/// Take a channel's sticky message down, when its rule loses it.
pub async fn remove(ctx: &Ctx, channel: u64) {
    let old = ctx.db.with(|c| {
        let old = current(c, channel)?;
        c.execute("DELETE FROM rules_sticky WHERE channel_id = ?1", [channel as i64])?;
        Ok(old)
    });
    if let Ok(Some(old)) = old {
        let _ = ctx.http.delete_message(Id::new(channel), Id::new(old)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_the_count_then_a_lull() {
        let mut st = State::default();
        assert_eq!(st.next(0, 0), Next::Idle);
        st.note(1_000, 3);
        st.note(2_000, 3);
        // Below the count and no quiet repost: nothing to do.
        assert_eq!(st.next(60_000, 0), Next::Idle);
        st.note(3_000, 3);
        assert_eq!(st.due_since, Some(3_000));
        assert_eq!(st.next(4_000, 0), Next::Wait(2_000));
        assert_eq!(st.next(6_000, 0), Next::Post);
        // Another message pushes the lull back.
        st.note(5_500, 3);
        assert_eq!(st.next(6_000, 0), Next::Wait(2_500));
        st.posted();
        assert_eq!(st.next(9_000, 0), Next::Idle);
    }

    #[test]
    fn a_busy_channel_gets_its_copy_after_thirty_seconds() {
        let mut st = State::default();
        st.note(0, 1);
        // A message every two seconds never leaves a three-second lull.
        for t in (2_000..30_000).step_by(2_000) {
            st.note(t, 1);
            assert!(matches!(st.next(t, 0), Next::Wait(_)), "at {t}");
        }
        assert_eq!(st.next(30_000, 0), Next::Post);
    }

    #[test]
    fn a_quiet_spell_reposts_below_the_count() {
        let mut st = State::default();
        st.note(10_000, 50);
        assert_eq!(st.next(20_000, 60_000), Next::Wait(50_000));
        assert_eq!(st.next(70_000, 60_000), Next::Post);
        // Reaching the count still uses the short lull.
        let mut st = State::default();
        for t in 0..50 {
            st.note(t * 100, 50);
        }
        assert_eq!(st.next(4_900 + LULL_MS, 60_000), Next::Post);
    }

    #[test]
    fn the_current_copy_is_replaced() {
        let db = bot_core::Db::memory().unwrap();
        db.migrate(KEY, crate::MIGRATIONS).unwrap();
        let c = db.lock();
        assert_eq!(current(&c, 5).unwrap(), None);
        set_current(&c, 1, 5, 100, 0).unwrap();
        set_current(&c, 1, 5, 101, 1).unwrap();
        assert_eq!(current(&c, 5).unwrap(), Some(101));
    }
}
