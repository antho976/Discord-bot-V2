//! Roles saved when a member leaves and given back when they rejoin.

use bot_core::Ctx;
use bot_core::anyhow::Result;
use bot_core::rusqlite::{OptionalExtension, params};
use bot_core::twilight_model::id::Id;

fn encode(ids: &[u64]) -> String {
    ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",")
}

fn decode(s: &str) -> Vec<u64> {
    s.split(',').filter_map(|p| p.parse().ok()).collect()
}

/// Remember the roles a member had when they left. Replaces any earlier save.
pub fn save(ctx: &Ctx, guild: u64, user: u64, roles: &[u64]) -> Result<()> {
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO welcome_saved_roles (guild_id, user_id, roles, left_at, restored_at, restored_roles)
             VALUES (?1, ?2, ?3, ?4, NULL, NULL)
             ON CONFLICT (guild_id, user_id) DO UPDATE SET roles = excluded.roles, left_at = excluded.left_at,
               restored_at = NULL, restored_roles = NULL",
            params![guild as i64, user as i64, encode(roles), bot_core::now_ms()],
        )
    })?;
    Ok(())
}

/// Drop a save still waiting for a member, as when they leave again with no roles. A save that
/// was already given back stays, for the dashboard's list of restores.
pub fn forget(ctx: &Ctx, guild: u64, user: u64) -> Result<()> {
    ctx.db.with(|c| {
        c.execute(
            "DELETE FROM welcome_saved_roles WHERE guild_id = ?1 AND user_id = ?2 AND restored_at IS NULL",
            params![guild as i64, user as i64],
        )
    })?;
    Ok(())
}

/// The roles waiting for a member who left no more than `days` ago and hasn't been restored.
pub fn waiting(ctx: &Ctx, guild: u64, user: u64, days: i64) -> Result<Vec<u64>> {
    let cutoff = bot_core::now_ms() - days * 86_400_000;
    let roles: Option<String> = ctx.db.with(|c| {
        c.query_row(
            "SELECT roles FROM welcome_saved_roles WHERE guild_id = ?1 AND user_id = ?2 AND left_at >= ?3 AND restored_at IS NULL",
            params![guild as i64, user as i64, cutoff],
            |r| r.get(0),
        )
        .optional()
    })?;
    Ok(roles.map(|r| decode(&r)).unwrap_or_default())
}

pub fn mark_restored(ctx: &Ctx, guild: u64, user: u64, given: &[u64]) -> Result<()> {
    ctx.db.with(|c| {
        c.execute(
            "UPDATE welcome_saved_roles SET restored_at = ?3, restored_roles = ?4 WHERE guild_id = ?1 AND user_id = ?2",
            params![guild as i64, user as i64, bot_core::now_ms(), encode(given)],
        )
    })?;
    Ok(())
}

/// Drop saves older than the keep time, and restored ones after a month.
pub fn prune(ctx: &Ctx, guild: u64, days: i64) -> Result<()> {
    let now = bot_core::now_ms();
    ctx.db.with(|c| {
        c.execute(
            "DELETE FROM welcome_saved_roles WHERE guild_id = ?1
             AND ((restored_at IS NULL AND left_at < ?2) OR (restored_at IS NOT NULL AND restored_at < ?3))",
            params![guild as i64, now - days * 86_400_000, now - 30 * 86_400_000],
        )
    })?;
    Ok(())
}

pub struct Restored {
    pub user: u64,
    pub roles: Vec<u64>,
    pub at: i64,
}

pub fn recently_restored(ctx: &Ctx, guild: u64, limit: i64) -> Result<Vec<Restored>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(
            "SELECT user_id, restored_roles, restored_at FROM welcome_saved_roles
             WHERE guild_id = ?1 AND restored_at IS NOT NULL ORDER BY restored_at DESC LIMIT ?2",
        )?;
        s
            .query_map(params![guild as i64, limit], |r| {
                Ok(Restored { user: r.get::<_, i64>(0)? as u64, roles: decode(&r.get::<_, String>(1)?), at: r.get(2)? })
            })?
            .collect()
    })
}

/// How many departed members have roles saved and waiting.
pub fn waiting_count(ctx: &Ctx, guild: u64, days: i64) -> Result<i64> {
    let cutoff = bot_core::now_ms() - days * 86_400_000;
    ctx.db.with(|c| {
        c.query_row(
            "SELECT COUNT(*) FROM welcome_saved_roles WHERE guild_id = ?1 AND restored_at IS NULL AND left_at >= ?2",
            params![guild as i64, cutoff],
            |r| r.get(0),
        )
    })
}

/// Departed members with roles saved and waiting, `(user, left at)`, most recent first.
pub fn waiting_members(ctx: &Ctx, guild: u64, days: i64, limit: i64) -> Result<Vec<(u64, i64)>> {
    let cutoff = bot_core::now_ms() - days * 86_400_000;
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(
            "SELECT user_id, left_at FROM welcome_saved_roles WHERE guild_id = ?1 AND restored_at IS NULL AND left_at >= ?2
             ORDER BY left_at DESC LIMIT ?3",
        )?;
        s.query_map(params![guild as i64, cutoff, limit], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?)))?.collect()
    })
}

/// Can the bot give this role? It must exist, not be managed (integration or booster roles)
/// and sit below the bot's own highest role. Without a cached bot member we let it try.
pub fn assignable(ctx: &Ctx, guild: u64, role: u64) -> bool {
    let Some(r) = Id::new_checked(role).and_then(|id| ctx.cache.role(id)) else { return false };
    if r.managed || role == guild {
        return false;
    }
    let Some(bot) = ctx.bot_id() else { return true };
    let (Some(guild_id), Some(bot_id)) = (Id::new_checked(guild), Id::new_checked(bot)) else { return false };
    let Some(me) = ctx.cache.member(guild_id, bot_id) else { return true };
    let top = me.roles().iter().filter_map(|id| ctx.cache.role(*id).map(|r| r.position)).max().unwrap_or(0);
    r.position < top
}

#[cfg(test)]
mod tests {
    use super::*;

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn leaving_with_no_roles_drops_the_old_save() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.install_features(vec![std::sync::Arc::new(crate::Welcome::default())]).unwrap();
        save(&ctx, 1, 10, &[100, 101]).unwrap();
        assert_eq!(waiting(&ctx, 1, 10, 90).unwrap(), vec![100, 101]);
        forget(&ctx, 1, 10).unwrap();
        assert!(waiting(&ctx, 1, 10, 90).unwrap().is_empty());
        // A restore already made stays listed.
        save(&ctx, 1, 11, &[100]).unwrap();
        mark_restored(&ctx, 1, 11, &[100]).unwrap();
        forget(&ctx, 1, 11).unwrap();
        assert_eq!(recently_restored(&ctx, 1, 10).unwrap().len(), 1);
    }
}
