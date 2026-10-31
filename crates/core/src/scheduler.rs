//! One timer for the whole bot: every minute each feature gets a `tick`, voice time is counted,
//! and the in-memory counters are written out.

use std::collections::HashMap;
use std::time::Duration;

use twilight_model::id::Id;

use crate::Ctx;

pub async fn run(ctx: Ctx, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let mut every = tokio::time::interval(Duration::from_secs(60));
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    every.tick().await;
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = every.tick() => {}
        }
        for f in ctx.features() {
            if let Err(e) = f.tick(&ctx).await {
                tracing::warn!(feature = f.meta().key, error = %e, "tick failed");
            }
        }
        count_voice(&ctx);
        if let Err(e) = ctx.flush_counters() {
            tracing::warn!(error = %e, "couldn't write counters");
        }
        prune(&ctx);
    }
    let _ = ctx.flush_counters();
}

/// Add a minute for everyone in a voice channel with someone else in it.
fn count_voice(ctx: &Ctx) {
    for g in ctx.guild_ids() {
        let Some(users) = ctx.cache.guild_voice_states(Id::new(g)) else { continue };
        let mut per_channel: HashMap<u64, i64> = HashMap::new();
        for u in users.iter() {
            if ctx.cache.user(*u).is_some_and(|x| x.bot) {
                continue;
            }
            if let Some(v) = ctx.cache.voice_state(*u, Id::new(g)) {
                *per_channel.entry(v.channel_id().get()).or_default() += 1;
            }
        }
        let minutes: i64 = per_channel.values().filter(|n| **n >= 2).sum();
        if minutes > 0 {
            ctx.count(g, "voice_minutes", minutes);
        }
    }
}

/// Drop expired sessions and old notices once in a while.
fn prune(ctx: &Ctx) {
    let now = crate::now_ms();
    let month = 30 * 86_400_000i64;
    let _ = ctx.db.with(|c| {
        c.execute("DELETE FROM dash_sessions WHERE expires_at < ?1", [now])?;
        c.execute("DELETE FROM notifications WHERE at < ?1", [now - month])?;
        c.execute("DELETE FROM dash_audit WHERE at < ?1", [now - 6 * month])?;
        Ok(())
    });
}
