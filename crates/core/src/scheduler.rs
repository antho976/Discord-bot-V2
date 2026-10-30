//! One timer for the whole bot: every minute each feature gets a `tick`, and the in-memory
//! counters are written out.

use std::time::Duration;

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
        if let Err(e) = ctx.flush_counters() {
            tracing::warn!(error = %e, "couldn't write counters");
        }
        prune(&ctx);
    }
    let _ = ctx.flush_counters();
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
