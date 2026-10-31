//! Auto-purge: delete messages older than a channel's limit. Runs from the minute tick in a task
//! of its own, visiting each channel every quarter of its limit (10 to 60 minutes apart).

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use bot_core::discord::{snowflake_ms, why_refused};
use bot_core::twilight_http::request::AuditLogReason;
use bot_core::twilight_model::id::Id;
use bot_core::twilight_model::id::marker::MessageMarker;
use bot_core::{Ctx, now_ms};

use crate::{KEY, Settings};

const MIN_MS: i64 = 60_000;
const DAY_MS: i64 = 86_400_000;
/// Discord bulk-deletes only messages younger than 14 days; keep a margin.
const BULK_MAX_AGE_MS: i64 = 14 * DAY_MS - 10 * MIN_MS;
/// Pages of 100 read per channel and run.
const MAX_PAGES: usize = 5;
/// Older messages go one request each, so only this many per channel and run.
const MAX_SINGLE: usize = 20;

#[derive(Default)]
pub struct Runs {
    running: AtomicBool,
    /// When each channel was last purged.
    last: Mutex<HashMap<u64, i64>>,
}

/// How often a channel is visited: a quarter of its limit, 10 to 60 minutes.
pub fn every_ms(after_mins: i64) -> i64 {
    (after_mins * MIN_MS / 4).clamp(10 * MIN_MS, 60 * MIN_MS)
}

/// The smallest message id sent at `ms`.
pub fn snowflake_at(ms: i64) -> u64 {
    ((ms - 1_420_070_400_000).max(1) as u64) << 22
}

/// Start a purge run in the background if a channel is due and no run is going.
pub fn tick(runs: &std::sync::Arc<Runs>, ctx: &Ctx) {
    if runs.running.swap(true, Ordering::AcqRel) {
        return;
    }
    let now = now_ms();
    let mut due = Vec::new();
    {
        let mut last = runs.last.lock().unwrap();
        for guild in ctx.enabled_guilds(KEY) {
            let Ok(s) = ctx.settings::<Settings>(guild, KEY) else { continue };
            for r in s.channels.iter().filter(|r| r.purge_after_mins > 0) {
                if last.get(&r.channel).is_none_or(|t| now - t >= every_ms(r.purge_after_mins)) {
                    last.insert(r.channel, now);
                    due.push((guild, r.channel, r.purge_after_mins));
                }
            }
        }
    }
    if due.is_empty() {
        runs.running.store(false, Ordering::Release);
        return;
    }
    let (runs, ctx) = (runs.clone(), ctx.clone());
    bot_core::tokio::spawn(async move {
        for (guild, channel, after) in due {
            match purge(&ctx, channel, after).await {
                Ok(n) if n > 0 => ctx.count(guild, "rules_purged", n as i64),
                Ok(_) => {}
                Err(why) => {
                    bot_core::tracing::warn!(guild, channel, error = %why, "auto-purge failed");
                    ctx.notify_once(guild, KEY, "purge", format!("Auto-purge couldn't clear a channel. {why}"), Some(format!("/g/{guild}/rules")), Duration::from_secs(86_400));
                }
            }
        }
        runs.running.store(false, Ordering::Release);
    });
}

/// Of a page of messages `(id, pinned)`, newest first, which to bulk-delete and which to delete
/// one by one. Pinned messages and the sticky copy stay. `single` stops at `room`.
pub fn sort_out(page: &[(u64, bool)], keep: Option<u64>, bulk_floor: i64, room: usize, bulk: &mut Vec<u64>, single: &mut Vec<u64>) {
    for (id, pinned) in page {
        if *pinned || Some(*id) == keep {
            continue;
        }
        if snowflake_ms(*id) > bulk_floor {
            bulk.push(*id);
        } else if single.len() < room {
            single.push(*id);
        }
    }
}

async fn purge(ctx: &Ctx, channel: u64, after_mins: i64) -> Result<usize, String> {
    let now = now_ms();
    let keep = ctx.db.with(|c| crate::sticky::current(c, channel)).ok().flatten();
    let bulk_floor = now - BULK_MAX_AGE_MS;
    let mut before = snowflake_at(now - after_mins * MIN_MS);
    let (mut bulk, mut single) = (Vec::new(), Vec::new());
    let ch = Id::new(channel);
    for _ in 0..MAX_PAGES {
        let page = match ctx.http.channel_messages(ch).before(Id::new(before)).limit(100).await {
            Ok(r) => r.models().await.map_err(|e| e.to_string())?,
            Err(e) => return Err(why_refused(&e)),
        };
        let Some(oldest) = page.last() else { break };
        before = oldest.id.get();
        let ids: Vec<(u64, bool)> = page.iter().map(|m| (m.id.get(), m.pinned)).collect();
        sort_out(&ids, keep, bulk_floor, MAX_SINGLE, &mut bulk, &mut single);
        // Everything further back is older still, so once the one-by-one room is full, stop.
        if page.len() < 100 || single.len() >= MAX_SINGLE {
            break;
        }
    }
    let mut deleted = 0;
    for chunk in bulk.chunks(100) {
        let ids: Vec<Id<MessageMarker>> = chunk.iter().map(|m| Id::new(*m)).collect();
        let res = if let [one] = ids.as_slice() {
            ctx.http.delete_message(ch, *one).await.map(|_| ())
        } else {
            ctx.http.delete_messages(ch, &ids).reason("Auto-purge").await.map(|_| ())
        };
        res.map_err(|e| why_refused(&e))?;
        deleted += chunk.len();
    }
    for id in single {
        if ctx.http.delete_message(ch, Id::new(id)).await.is_ok() {
            deleted += 1;
        }
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_are_visited_every_quarter_of_their_limit() {
        assert_eq!(every_ms(20), 10 * MIN_MS);
        assert_eq!(every_ms(120), 30 * MIN_MS);
        assert_eq!(every_ms(7 * 24 * 60), 60 * MIN_MS);
    }

    #[test]
    fn snowflakes_from_times() {
        let at = 1_700_000_000_000;
        assert_eq!(snowflake_ms(snowflake_at(at)), at);
        assert!(snowflake_at(at + 1) > snowflake_at(at));
    }

    #[test]
    fn pinned_and_sticky_stay_and_old_ones_are_capped() {
        let now = 1_800_000_000_000;
        let recent = |n: i64| snowflake_at(now - n * MIN_MS);
        let old = |n: i64| snowflake_at(now - 20 * DAY_MS - n * MIN_MS);
        let page = vec![(recent(1), false), (recent(2), true), (recent(3), false), (old(1), false), (old(2), false), (old(3), false)];
        let (mut bulk, mut single) = (Vec::new(), Vec::new());
        sort_out(&page, Some(recent(3)), now - BULK_MAX_AGE_MS, 2, &mut bulk, &mut single);
        assert_eq!(bulk, vec![recent(1)]);
        assert_eq!(single, vec![old(1), old(2)]);
    }
}
