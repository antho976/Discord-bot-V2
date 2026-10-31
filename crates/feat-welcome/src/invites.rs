//! Invite tracking: remember each invite's use count, and on a join find the one that went up.

use std::collections::HashMap;

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
/// An invite whose use count went up wins (the biggest rise if several did). Otherwise a limited
/// invite that is gone and was one use from its limit was used up by this join. Vanity links and
/// widget joins match nothing, which returns `None`.
pub fn find_used(old: &[Inv], new: &[Inv]) -> Option<Inv> {
    let mut best: Option<(i64, &Inv)> = None;
    for n in new {
        let before = old.iter().find(|o| o.code == n.code).map(|o| o.uses).unwrap_or(0);
        let rise = n.uses - before;
        if rise > 0 && best.is_none_or(|(r, _)| rise > r) {
            best = Some((rise, n));
        }
    }
    if let Some((_, inv)) = best {
        return Some(inv.clone());
    }
    old.iter()
        .find(|o| o.max_uses > 0 && o.uses + 1 >= o.max_uses && !new.iter().any(|n| n.code == o.code))
        .map(|o| Inv { uses: o.uses + 1, ..o.clone() })
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

fn describe(e: &bot_core::twilight_http::Error) -> String {
    match e.kind() {
        ErrorType::Response { status, .. } if status.get() == 403 => {
            "Discord refused the request. The bot needs the Manage Server permission to read invites.".into()
        }
        _ => format!("Discord returned an error: {e}"),
    }
}

/// Every invite of the server right now, or the reason it couldn't be read.
async fn fetch(ctx: &Ctx, guild: u64) -> Result<Vec<Inv>, String> {
    let res = ctx.http.guild_invites(Id::new(guild)).await.map_err(|e| describe(&e))?;
    let list = res.models().await.map_err(|e| format!("Couldn't read Discord's answer: {e}"))?;
    Ok(list
        .into_iter()
        .map(|i| Inv { code: i.code, uses: i.uses.unwrap_or(0) as i64, max_uses: i.max_uses.unwrap_or(0) as i64, inviter: i.inviter.map(|u| u.id.get()) })
        .collect())
}

/// Read the server's invites and store them as the new baseline. Returns whether it worked.
pub async fn refresh(ctx: &Ctx, guild: u64) -> bool {
    match fetch(ctx, guild).await {
        Ok(list) => match store(ctx, guild, &list) {
            Ok(()) => true,
            Err(e) => {
                bot_core::tracing::warn!(guild, error = %e, "couldn't store invites");
                false
            }
        },
        Err(msg) => {
            store_error(ctx, guild, &msg);
            false
        }
    }
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

/// Work out which invite a new member used and record the join. `lock` keeps two joins at the
/// same moment from reading each other's counts.
pub async fn record_join(ctx: &Ctx, lock: &bot_core::tokio::sync::Mutex<()>, guild: u64, user: u64, bot: bool, track: bool) {
    let _guard = lock.lock().await;
    let mut used: Option<Inv> = None;
    if track && !bot {
        let baseline = state(ctx, guild).ok().flatten().is_some_and(|s| s.ok);
        let old = if baseline { load(ctx, guild).unwrap_or_default() } else { Vec::new() };
        match fetch(ctx, guild).await {
            Ok(new) => {
                if baseline {
                    used = find_used(&old, &new);
                }
                if let Err(e) = store(ctx, guild, &new) {
                    bot_core::tracing::warn!(guild, error = %e, "couldn't store invites");
                }
            }
            Err(msg) => store_error(ctx, guild, &msg),
        }
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
    fn the_biggest_rise_wins() {
        let old = [inv("a", 1, 0), inv("b", 1, 0)];
        let new = [inv("a", 2, 0), inv("b", 4, 0)];
        assert_eq!(find_used(&old, &new).unwrap().code, "b");
    }
}
