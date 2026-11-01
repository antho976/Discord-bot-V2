//! Invite tracking: remember each invite's use count, and on a join find the one that went up.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bot_core::Ctx;
use bot_core::anyhow::Result;
use bot_core::rusqlite::{OptionalExtension, params};
use bot_core::twilight_http::error::ErrorType;
use bot_core::twilight_model::id::Id;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inv {
    pub code: String,
    pub uses: i64,
    /// 0 means unlimited.
    pub max_uses: i64,
    pub inviter: Option<u64>,
}

/// Which invite a join came through, from the counts before the join and the counts now.
///
/// The one invite whose use count went up, or else the one limited invite that vanished a use
/// from its limit (this join used it up). When more than one moved, several members joined
/// between the two readings (at the same moment, or while the bot was away) and nothing says who
/// used which, so the answer is `None` rather than a guess. Vanity links and widget joins match
/// nothing, which is `None` too.
pub fn find_used(old: &[Inv], new: &[Inv]) -> Option<Inv> {
    let before = |code: &str| old.iter().find(|o| o.code == code).map(|o| o.uses).unwrap_or(0);
    let risen: Vec<&Inv> = new.iter().filter(|n| n.uses > before(&n.code)).collect();
    let used_up: Vec<&Inv> = old.iter().filter(|o| o.max_uses > 0 && o.uses + 1 >= o.max_uses && !new.iter().any(|n| n.code == o.code)).collect();
    match (risen.as_slice(), used_up.as_slice()) {
        ([one], []) => Some((*one).clone()),
        ([], [one]) => Some(Inv { uses: one.uses + 1, ..(*one).clone() }),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct State {
    pub checked_at: i64,
    pub ok: bool,
    pub error: Option<String>,
    pub codes: i64,
}

pub fn state(ctx: &Ctx, guild: u64) -> Result<Option<State>> {
    ctx.db.with(|c| {
        c.query_row(
            "SELECT checked_at, ok, error, (SELECT COUNT(*) FROM welcome_invites WHERE guild_id = ?1)
             FROM welcome_invite_state WHERE guild_id = ?1",
            [guild as i64],
            |r| Ok(State { checked_at: r.get(0)?, ok: r.get(1)?, error: r.get(2)?, codes: r.get(3)? }),
        )
        .optional()
    })
}

/// The server's invites as last read, most used first.
pub fn load(ctx: &Ctx, guild: u64) -> Result<Vec<Inv>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached("SELECT code, uses, max_uses, inviter_id FROM welcome_invites WHERE guild_id = ?1 ORDER BY uses DESC, code")?;
        s
            .query_map([guild as i64], |r| {
                Ok(Inv { code: r.get(0)?, uses: r.get(1)?, max_uses: r.get(2)?, inviter: r.get::<_, Option<i64>>(3)?.map(|i| i as u64) })
            })?
            .collect()
    })
}

fn store(ctx: &Ctx, guild: u64, invites: &[Inv]) -> Result<()> {
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM welcome_invites WHERE guild_id = ?1", [guild as i64])?;
    for i in invites {
        tx.execute(
            "INSERT OR REPLACE INTO welcome_invites (guild_id, code, uses, max_uses, inviter_id) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![guild as i64, i.code, i.uses, i.max_uses, i.inviter.map(|u| u as i64)],
        )?;
    }
    tx.execute(
        "INSERT INTO welcome_invite_state (guild_id, checked_at, ok, error) VALUES (?1, ?2, 1, NULL)
         ON CONFLICT (guild_id) DO UPDATE SET checked_at = excluded.checked_at, ok = 1, error = NULL",
        params![guild as i64, bot_core::now_ms()],
    )?;
    tx.commit()?;
    Ok(())
}

fn store_error(ctx: &Ctx, guild: u64, error: &str) {
    let res = ctx.db.with(|c| {
        c.execute(
            "INSERT INTO welcome_invite_state (guild_id, checked_at, ok, error) VALUES (?1, ?2, 0, ?3)
             ON CONFLICT (guild_id) DO UPDATE SET checked_at = excluded.checked_at, ok = 0, error = excluded.error",
            params![guild as i64, bot_core::now_ms(), error],
        )
    });
    if let Err(e) = res {
        bot_core::tracing::warn!(guild, error = %e, "couldn't store the invite state");
    }
}

/// What's stored when Discord refuses to show the invites: the bot lacks Manage Server.
const REFUSED: &str = "Discord refused the request. The bot needs the Manage Server permission to read invites.";

/// Whether the last reading was refused for want of Manage Server. Joins don't ask again then:
/// each would be one more refusal, and enough of those in a join raid get the bot shut out of
/// Discord for a while. The hourly reading in the tick finds out when the permission is back.
pub fn refused(ctx: &Ctx, guild: u64) -> bool {
    state(ctx, guild).ok().flatten().is_some_and(|s| !s.ok && s.error.as_deref() == Some(REFUSED))
}

fn describe(e: &bot_core::twilight_http::Error) -> String {
    match e.kind() {
        ErrorType::Response { status, .. } if status.get() == 403 => REFUSED.into(),
        _ => format!("Discord returned an error: {e}"),
    }
}

/// Every invite of the server right now, or the reason it couldn't be read.
async fn fetch(ctx: &Ctx, guild: u64) -> Result<Vec<Inv>, String> {
    let guild = Id::new_checked(guild).ok_or("That isn't a server.")?;
    let res = ctx.http.guild_invites(guild).await.map_err(|e| describe(&e))?;
    let list = res.models().await.map_err(|e| format!("Couldn't read Discord's answer: {e}"))?;
    Ok(list
        .into_iter()
        .map(|i| Inv { code: i.code, uses: i.uses.unwrap_or(0) as i64, max_uses: i.max_uses.unwrap_or(0) as i64, inviter: i.inviter.map(|u| u.id.get()) })
        .collect())
}

/// A reading stored: the invites before it, when there was a good reading to compare with, and now.
type Stored = (Option<Vec<Inv>>, Vec<Inv>);

/// The last reading of a server made through [`Readings::read`]: its number and what it stored.
type Flight = Arc<bot_core::tokio::sync::Mutex<(u64, Option<Stored>)>>;

/// The invite readings in flight. Each server has at most one at a time: joins that arrive while
/// one is out wait for it, then share the next reading between them, so a join raid sends a
/// handful of requests instead of one per join. A slow answer holds up only that server's
/// readings, never another server or the rest of a join. Readings are numbered as they start;
/// one that answers after a later one was stored is older than the baseline and isn't stored
/// over it.
#[derive(Default)]
pub struct Readings {
    started: AtomicU64,
    /// Per server, the number of the newest reading stored. Locked only to compare and store,
    /// never across a call to Discord.
    newest: Mutex<HashMap<u64, u64>>,
    /// Per server, the reading in flight or the last one made. One entry per server.
    flights: Mutex<HashMap<u64, Flight>>,
}

impl Readings {
    fn start(&self) -> u64 {
        self.started.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Store reading `n` of `guild` as the new baseline, unless a reading that started later was
    /// stored first. `None` when nothing was stored: an older reading, or Discord refused.
    fn settle(&self, ctx: &Ctx, guild: u64, n: u64, fetched: Result<Vec<Inv>, String>) -> Option<Stored> {
        let mut newest = self.newest.lock().unwrap();
        let newest = newest.entry(guild).or_default();
        if n < *newest {
            return None;
        }
        match fetched {
            Ok(now) => {
                let baseline = state(ctx, guild).ok().flatten().is_some_and(|s| s.ok);
                let before = baseline.then(|| load(ctx, guild).unwrap_or_default());
                if let Err(e) = store(ctx, guild, &now) {
                    bot_core::tracing::warn!(guild, error = %e, "couldn't store invites");
                    return None;
                }
                *newest = n;
                Some((before, now))
            }
            Err(msg) => {
                store_error(ctx, guild, &msg);
                None
            }
        }
    }

    /// Read the server's invites, or share a reading that started after this call did: that one
    /// already sees whatever happened before the call, such as the join it's for.
    async fn read(&self, ctx: &Ctx, guild: u64) -> Option<Stored> {
        let arrived = self.started.load(Ordering::SeqCst);
        let flight = self.flights.lock().unwrap().entry(guild).or_default().clone();
        let mut last = flight.lock().await;
        if last.0 > arrived {
            return last.1.clone();
        }
        let n = self.start();
        let fetched = fetch(ctx, guild).await;
        let stored = self.settle(ctx, guild, n, fetched);
        *last = (n, stored.clone());
        stored
    }
}

/// Read the server's invites and store them as the new baseline. Returns whether it was stored.
pub async fn refresh(ctx: &Ctx, readings: &Readings, guild: u64) -> bool {
    readings.read(ctx, guild).await.is_some()
}

/// Servers whose invites are due: never read, or last read over an hour ago.
pub fn due(ctx: &Ctx, guild: u64) -> bool {
    match state(ctx, guild) {
        Ok(Some(s)) => bot_core::now_ms() - s.checked_at > 3_600_000,
        Ok(None) => true,
        Err(_) => false,
    }
}

pub fn on_invite_create(ctx: &Ctx, guild: u64, code: &str, max_uses: i64, inviter: Option<u64>) {
    let res = ctx.db.with(|c| {
        c.execute(
            "INSERT OR REPLACE INTO welcome_invites (guild_id, code, uses, max_uses, inviter_id) VALUES (?1, ?2, 0, ?3, ?4)",
            params![guild as i64, code, max_uses, inviter.map(|u| u as i64)],
        )
    });
    if let Err(e) = res {
        bot_core::tracing::warn!(guild, error = %e, "couldn't store a new invite");
    }
}

/// Forget a deleted invite, unless it was one use from its limit: that join may be about to be
/// counted and needs the row to find it.
pub fn on_invite_delete(ctx: &Ctx, guild: u64, code: &str) {
    let res = ctx.db.with(|c| {
        c.execute(
            "DELETE FROM welcome_invites WHERE guild_id = ?1 AND code = ?2 AND NOT (max_uses > 0 AND uses + 1 >= max_uses)",
            params![guild as i64, code],
        )
    });
    if let Err(e) = res {
        bot_core::tracing::warn!(guild, error = %e, "couldn't forget an invite");
    }
}

/// Work out which invite a new member used and record the join. Joins at the same moment share
/// a reading and compare it with the one stored before it (see [`Readings`]). Nothing is asked
/// while Discord refuses to show the invites, and a join waits at most 15 seconds for a reading;
/// either way it's recorded without an invite.
pub async fn record_join(ctx: &Ctx, readings: &Readings, guild: u64, user: u64, bot: bool, track: bool) {
    let mut used: Option<Inv> = None;
    if track
        && !bot
        && !refused(ctx, guild)
        && let Ok(Some((Some(before), now))) = bot_core::tokio::time::timeout(Duration::from_secs(15), readings.read(ctx, guild)).await
    {
        used = find_used(&before, &now);
    }
    let res = ctx.db.with(|c| {
        c.execute(
            "INSERT INTO welcome_joins (guild_id, user_id, inviter_id, code, joined_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                guild as i64,
                user as i64,
                used.as_ref().and_then(|i| i.inviter).map(|u| u as i64),
                used.as_ref().map(|i| i.code.clone()),
                bot_core::now_ms()
            ],
        )
    });
    if let Err(e) = res {
        bot_core::tracing::warn!(guild, error = %e, "couldn't record a join");
    }
}

/// Invite names by code.
pub fn names(ctx: &Ctx, guild: u64) -> Result<HashMap<String, String>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached("SELECT code, name FROM welcome_invite_names WHERE guild_id = ?1")?;
        s.query_map([guild as i64], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
    })
}

/// Name invites, or clear a name with an empty one.
pub fn set_names(ctx: &Ctx, guild: u64, names: &[(String, String)]) -> Result<()> {
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    for (code, name) in names {
        if name.is_empty() {
            tx.execute("DELETE FROM welcome_invite_names WHERE guild_id = ?1 AND code = ?2", params![guild as i64, code])?;
        } else {
            tx.execute(
                "INSERT INTO welcome_invite_names (guild_id, code, name) VALUES (?1, ?2, ?3)
                 ON CONFLICT (guild_id, code) DO UPDATE SET name = excluded.name",
                params![guild as i64, code, name],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Join {
    pub user: u64,
    pub inviter: Option<u64>,
    pub code: Option<String>,
    pub at: i64,
}

pub fn recent_joins(ctx: &Ctx, guild: u64, limit: i64) -> Result<Vec<Join>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(
            "SELECT user_id, inviter_id, code, joined_at FROM welcome_joins WHERE guild_id = ?1 ORDER BY joined_at DESC, id DESC LIMIT ?2",
        )?;
        s
            .query_map(params![guild as i64, limit], |r| {
                Ok(Join { user: r.get::<_, i64>(0)? as u64, inviter: r.get::<_, Option<i64>>(1)?.map(|i| i as u64), code: r.get(2)?, at: r.get(3)? })
            })?
            .collect()
    })
}

/// `(inviter, joins since `since`, joins ever)`, busiest in the period first.
pub fn top_inviters(ctx: &Ctx, guild: u64, since: i64, limit: i64) -> Result<Vec<(u64, i64, i64)>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(
            "SELECT inviter_id, SUM(joined_at >= ?2), COUNT(*) FROM welcome_joins
             WHERE guild_id = ?1 AND inviter_id IS NOT NULL
             GROUP BY inviter_id ORDER BY 2 DESC, 3 DESC LIMIT ?3",
        )?;
        s
            .query_map(params![guild as i64, since, limit], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?, r.get(2)?)))?
            .collect()
    })
}

/// Joins since `since`, newest first. Each comes with whether the member had joined before, so
/// a rejoin can read as "came back".
pub fn joins_since(ctx: &Ctx, guild: u64, since: i64, limit: i64) -> Result<Vec<(Join, bool)>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(
            "SELECT j.user_id, j.inviter_id, j.code, j.joined_at,
                    EXISTS (SELECT 1 FROM welcome_joins p WHERE p.guild_id = j.guild_id AND p.user_id = j.user_id AND p.joined_at < j.joined_at)
             FROM welcome_joins j WHERE j.guild_id = ?1 AND j.joined_at >= ?2
             ORDER BY j.joined_at DESC, j.id DESC LIMIT ?3",
        )?;
        s
            .query_map(params![guild as i64, since, limit], |r| {
                let join = Join { user: r.get::<_, i64>(0)? as u64, inviter: r.get::<_, Option<i64>>(1)?.map(|i| i as u64), code: r.get(2)?, at: r.get(3)? };
                Ok((join, r.get::<_, bool>(4)?))
            })?
            .collect()
    })
}

/// `(joins since `since`, of those with a known invite)`.
pub fn join_counts(ctx: &Ctx, guild: u64, since: i64) -> Result<(i64, i64)> {
    ctx.db.with(|c| {
        c.query_row(
            "SELECT COUNT(*), COALESCE(SUM(inviter_id IS NOT NULL), 0) FROM welcome_joins WHERE guild_id = ?1 AND joined_at >= ?2",
            params![guild as i64, since],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inv(code: &str, uses: i64, max: i64) -> Inv {
        Inv { code: code.into(), uses, max_uses: max, inviter: Some(7) }
    }

    #[test]
    fn finds_the_invite_that_went_up() {
        let old = [inv("a", 3, 0), inv("b", 1, 0)];
        let new = [inv("a", 3, 0), inv("b", 2, 0)];
        assert_eq!(find_used(&old, &new).unwrap().code, "b");
    }

    #[test]
    fn a_new_invite_with_a_use_counts() {
        let old = [inv("a", 3, 0)];
        let new = [inv("a", 3, 0), inv("fresh", 1, 0)];
        assert_eq!(find_used(&old, &new).unwrap().code, "fresh");
    }

    #[test]
    fn a_used_up_invite_is_found_by_its_absence() {
        let old = [inv("a", 3, 0), inv("once", 0, 1)];
        let new = [inv("a", 3, 0)];
        let used = find_used(&old, &new).unwrap();
        assert_eq!((used.code.as_str(), used.uses), ("once", 1));
    }

    #[test]
    fn an_expired_unlimited_invite_is_not_a_match() {
        let old = [inv("a", 3, 0), inv("old", 9, 0)];
        let new = [inv("a", 3, 0)];
        assert!(find_used(&old, &new).is_none());
    }

    #[test]
    fn no_change_means_unknown() {
        let both = [inv("a", 3, 0)];
        assert!(find_used(&both, &both).is_none());
    }

    #[test]
    fn two_invites_ticking_at_once_name_neither() {
        // Two members joined between readings, one through each: a guess would be wrong half the time.
        let old = [inv("a", 1, 0), inv("b", 1, 0)];
        let new = [inv("a", 2, 0), inv("b", 2, 0)];
        assert!(find_used(&old, &new).is_none());
        // The same when one of them was a single-use invite that's now gone.
        let old = [inv("a", 1, 0), inv("once", 0, 1)];
        let new = [inv("a", 2, 0)];
        assert!(find_used(&old, &new).is_none());
        // And for whoever's join is handled second: the first reading already took both rises.
        assert!(find_used(&new, &new).is_none());
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn an_older_reading_never_replaces_a_newer_one() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.install_features(vec![std::sync::Arc::new(crate::Welcome::default())]).unwrap();
        let r = Readings::default();
        let (first, second) = (r.start(), r.start());
        // The later reading answers first and becomes the baseline; there was none to compare with.
        assert_eq!(r.settle(&ctx, 1, second, Ok(vec![inv("a", 3, 0)])), Some((None, vec![inv("a", 3, 0)])));
        // The earlier one answers after it: older than the baseline, so it's dropped.
        assert_eq!(r.settle(&ctx, 1, first, Ok(vec![inv("a", 2, 0)])), None);
        assert_eq!(load(&ctx, 1).unwrap(), vec![inv("a", 3, 0)]);
        // The next one compares with the baseline.
        let third = r.start();
        let (before, now) = r.settle(&ctx, 1, third, Ok(vec![inv("a", 4, 0)])).unwrap();
        assert_eq!(find_used(&before.unwrap(), &now).unwrap().code, "a");
        // Each server keeps its own order: a reading of server 2 isn't older than server 1's.
        assert!(r.settle(&ctx, 2, first, Ok(vec![inv("b", 1, 0)])).is_some());
        // A refusal is noted, unless a newer reading is already stored.
        assert_eq!(r.settle(&ctx, 1, second, Err("refused".into())), None);
        assert!(state(&ctx, 1).unwrap().unwrap().ok);
        let fourth = r.start();
        assert_eq!(r.settle(&ctx, 1, fourth, Err("refused".into())), None);
        assert!(!state(&ctx, 1).unwrap().unwrap().ok);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn joins_dont_ask_while_discord_refuses() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.install_features(vec![std::sync::Arc::new(crate::Welcome::default())]).unwrap();
        assert!(!refused(&ctx, 1), "never read");
        store_error(&ctx, 1, "Discord returned an error: 500");
        assert!(!refused(&ctx, 1), "another failure is tried again on the next join");
        store_error(&ctx, 1, REFUSED);
        assert!(refused(&ctx, 1));
        // A join is still recorded, without asking Discord.
        let r = Readings::default();
        record_join(&ctx, &r, 1, 10, false, true).await;
        assert_eq!(r.started.load(Ordering::SeqCst), 0);
        assert_eq!(recent_joins(&ctx, 1, 10).unwrap().len(), 1);
        // A good reading clears it.
        store(&ctx, 1, &[inv("a", 1, 0)]).unwrap();
        assert!(!refused(&ctx, 1));
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn joins_during_a_reading_share_the_next_one() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.install_features(vec![std::sync::Arc::new(crate::Welcome::default())]).unwrap();
        let r = Arc::new(Readings::default());
        // A reading of server 1 is out when a join arrives.
        let flight = r.flights.lock().unwrap().entry(1).or_default().clone();
        let mut out = flight.lock().await;
        let join = bot_core::tokio::spawn({
            let (r, ctx) = (r.clone(), ctx.clone());
            async move { r.read(&ctx, 1).await }
        });
        bot_core::tokio::task::yield_now().await;
        // The next reading starts after the join arrived, and the join shares it.
        let n = r.start();
        let stored = r.settle(&ctx, 1, n, Ok(vec![inv("a", 1, 0)]));
        *out = (n, stored.clone());
        drop(out);
        assert_eq!(join.await.unwrap(), stored);
        assert_eq!(r.started.load(Ordering::SeqCst), n, "the join didn't ask Discord again");
        assert_eq!(r.flights.lock().unwrap().len(), 1);
    }

    #[test]
    fn one_invite_rising_by_more_than_one_still_names_it() {
        let old = [inv("a", 1, 0), inv("b", 5, 0)];
        let new = [inv("a", 1, 0), inv("b", 7, 0)];
        assert_eq!(find_used(&old, &new).unwrap().code, "b");
    }
}
