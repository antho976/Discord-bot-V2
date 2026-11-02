//! The queue's table, adding requests without duplicates, and the name matching.

use bot_core::anyhow::Result;
use bot_core::rusqlite::{Connection, OptionalExtension, Row, params};
use bot_core::{Ctx, Migration, now_ms};

pub const MIGRATIONS: &[Migration] = &[Migration {
    name: "001_reviews",
    // status: open, done or removed. A removed row stays so its thread or Twitch redemption
    // isn't added again.
    sql: "
        CREATE TABLE reviews (
            id INTEGER PRIMARY KEY, guild_id INTEGER NOT NULL, name TEXT NOT NULL,
            user_id INTEGER, thread_id INTEGER, twitch_user TEXT,
            reward_id TEXT, redemption_id TEXT, redeemed_at INTEGER,
            requested_at INTEGER NOT NULL, status TEXT NOT NULL DEFAULT 'open',
            notes TEXT NOT NULL DEFAULT '', source TEXT NOT NULL,
            pinged_at INTEGER, closed_at INTEGER, closed_by TEXT);
        CREATE INDEX reviews_by_status ON reviews(guild_id, status, requested_at);
        CREATE UNIQUE INDEX reviews_thread ON reviews(guild_id, thread_id) WHERE thread_id IS NOT NULL;
        CREATE UNIQUE INDEX reviews_redemption ON reviews(guild_id, redemption_id) WHERE redemption_id IS NOT NULL;
        CREATE TABLE reviews_twitch (
            guild_id INTEGER PRIMARY KEY, broadcaster_id TEXT NOT NULL, login TEXT NOT NULL,
            access_token TEXT, refresh_token TEXT, expires_at INTEGER NOT NULL DEFAULT 0,
            scopes TEXT NOT NULL DEFAULT '', synced_at INTEGER, error TEXT);
    ",
}, Migration {
    name: "002_reviews_sources",
    // Every post (kind 'thread') and redemption (kind 'redemption') that joined a request, so a
    // merged one is never added again and Complete reaches all of a request's redemptions.
    // twitch is the status the bot set on Twitch for a redemption (FULFILLED or CANCELED), NULL
    // while it's untouched; twitch_pending is 1 until Twitch accepted it.
    sql: "
        CREATE TABLE reviews_sources (
            guild_id INTEGER NOT NULL, kind TEXT NOT NULL, ext_id TEXT NOT NULL,
            review_id INTEGER NOT NULL, reward_id TEXT,
            twitch TEXT, twitch_pending INTEGER NOT NULL DEFAULT 0,
            UNIQUE (guild_id, kind, ext_id));
        CREATE INDEX reviews_sources_by_review ON reviews_sources(review_id);
        CREATE INDEX reviews_sources_pending ON reviews_sources(guild_id) WHERE twitch_pending = 1;
        INSERT INTO reviews_sources (guild_id, kind, ext_id, review_id)
            SELECT guild_id, 'thread', CAST(thread_id AS TEXT), id FROM reviews WHERE thread_id IS NOT NULL;
        INSERT INTO reviews_sources (guild_id, kind, ext_id, review_id, reward_id, twitch, twitch_pending)
            SELECT guild_id, 'redemption', redemption_id, id, reward_id, CASE status WHEN 'done' THEN 'FULFILLED' END, status = 'done'
            FROM reviews WHERE redemption_id IS NOT NULL;
    ",
}, Migration {
    name: "003_reviews_waitlist",
    // status can also be waiting: someone asked again soon after their last review, and joins
    // the queue at eligible_at.
    sql: "
        ALTER TABLE reviews ADD COLUMN eligible_at INTEGER;
        CREATE INDEX reviews_waiting ON reviews(eligible_at) WHERE status = 'waiting';
    ",
}, Migration {
    name: "004_reviews_thread_replies",
    // toolbox: the member's newest IdleonToolbox profile link in their post. nudged_at: when the
    // bot asked for one, so it asks once. pinged_by: the staff member who pressed Ping, told when
    // the member answers ready (ready_at). tags: the dashboard's tags, comma separated.
    // locked_at: when the bot locked a finished request's post (or claimed doing so);
    // lock_failed_at: when the timer last failed to, so it waits an hour before trying again.
    // reviews_marks 'v4': when this ran. The timer and the reminder only act on posts finished or
    // made after it, so an update doesn't lock or remind every post a server already has.
    sql: "
        ALTER TABLE reviews ADD COLUMN toolbox TEXT;
        ALTER TABLE reviews ADD COLUMN nudged_at INTEGER;
        ALTER TABLE reviews ADD COLUMN pinged_by INTEGER;
        ALTER TABLE reviews ADD COLUMN ready_at INTEGER;
        ALTER TABLE reviews ADD COLUMN tags TEXT NOT NULL DEFAULT '';
        ALTER TABLE reviews ADD COLUMN locked_at INTEGER;
        ALTER TABLE reviews ADD COLUMN lock_failed_at INTEGER;
        CREATE TABLE reviews_marks (name TEXT PRIMARY KEY, at INTEGER NOT NULL);
        INSERT INTO reviews_marks (name, at) VALUES ('v4', CAST(strftime('%s', 'now') AS INTEGER) * 1000);
    ",
}];

/// Names at least this alike are the same request, like the old bot.
pub const SAME_NAME: f64 = 0.85;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Review {
    pub id: i64,
    pub name: String,
    pub user_id: Option<u64>,
    pub thread_id: Option<u64>,
    pub twitch_user: Option<String>,
    pub reward_id: Option<String>,
    pub redemption_id: Option<String>,
    /// Set when it came with channel points: these go first.
    pub redeemed_at: Option<i64>,
    pub requested_at: i64,
    pub status: String,
    pub notes: String,
    pub source: String,
    pub pinged_at: Option<i64>,
    pub closed_at: Option<i64>,
    pub closed_by: Option<String>,
    /// On the waitlist: when it joins the queue.
    pub eligible_at: Option<i64>,
    /// The member's newest IdleonToolbox profile link in their post.
    pub toolbox: Option<String>,
    /// When the bot asked for a toolbox link.
    pub nudged_at: Option<i64>,
    /// Who pressed Ping last (a Discord user id).
    pub pinged_by: Option<u64>,
    /// When the member answered the ping with "ready".
    pub ready_at: Option<i64>,
    /// Tags from the post's words, in [`crate::words::tag_rank`] order.
    pub tags: Vec<String>,
    /// When the bot locked the post after the review.
    pub locked_at: Option<i64>,
}

impl Review {
    /// The toolbox link from the post, or else one staff put in the notes.
    pub fn toolbox_link(&self) -> Option<String> {
        self.toolbox.clone().or_else(|| crate::words::toolbox_link(&self.notes))
    }
}

const COLS: &str = "id, name, user_id, thread_id, twitch_user, reward_id, redemption_id, redeemed_at, requested_at, status, notes, source, pinged_at, closed_at, closed_by, eligible_at, toolbox, nudged_at, pinged_by, ready_at, tags, locked_at";

fn row(r: &Row) -> bot_core::rusqlite::Result<Review> {
    Ok(Review {
        id: r.get(0)?,
        name: r.get(1)?,
        user_id: r.get::<_, Option<i64>>(2)?.map(|u| u as u64),
        thread_id: r.get::<_, Option<i64>>(3)?.map(|u| u as u64),
        twitch_user: r.get(4)?,
        reward_id: r.get(5)?,
        redemption_id: r.get(6)?,
        redeemed_at: r.get(7)?,
        requested_at: r.get(8)?,
        status: r.get(9)?,
        notes: r.get(10)?,
        source: r.get(11)?,
        pinged_at: r.get(12)?,
        closed_at: r.get(13)?,
        closed_by: r.get(14)?,
        eligible_at: r.get(15)?,
        toolbox: r.get(16)?,
        nudged_at: r.get(17)?,
        pinged_by: r.get::<_, Option<i64>>(18)?.map(|u| u as u64),
        ready_at: r.get(19)?,
        tags: split_tags(&r.get::<_, String>(20)?),
        locked_at: r.get(21)?,
    })
}

fn split_tags(s: &str) -> Vec<String> {
    s.split(',').map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned).collect()
}

/// Open requests: channel points first, then the longest waiting.
pub fn open(ctx: &Ctx, guild: u64) -> Result<Vec<Review>> {
    ctx.db.with(|c| open_in(c, guild))
}

fn open_in(c: &Connection, guild: u64) -> bot_core::rusqlite::Result<Vec<Review>> {
    let mut st = c.prepare(&format!(
        "SELECT {COLS} FROM reviews WHERE guild_id = ?1 AND status = 'open' ORDER BY redeemed_at IS NULL, COALESCE(eligible_at, requested_at), id"
    ))?;
    st.query_map([guild as i64], row)?.collect()
}

/// The waitlist: the soonest to join the queue first.
pub fn waiting(ctx: &Ctx, guild: u64) -> Result<Vec<Review>> {
    ctx.db.with(|c| {
        let mut st = c.prepare(&format!("SELECT {COLS} FROM reviews WHERE guild_id = ?1 AND status = 'waiting' ORDER BY eligible_at, requested_at, id"))?;
        st.query_map([guild as i64], row)?.collect()
    })
}

/// How many are in the queue and on the waitlist.
pub fn counts(ctx: &Ctx, guild: u64) -> Result<(usize, usize)> {
    ctx.db.with(|c| {
        c.query_row(
            "SELECT COALESCE(SUM(status = 'open'), 0), COALESCE(SUM(status = 'waiting'), 0) FROM reviews WHERE guild_id = ?1 AND status IN ('open', 'waiting')",
            [guild as i64],
            |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)? as usize)),
        )
    })
}

/// When this member or Twitch viewer last had a review completed.
pub fn last_done(c: &Connection, guild: u64, user: Option<u64>, viewer: Option<&str>) -> bot_core::rusqlite::Result<Option<i64>> {
    if user.is_none() && viewer.is_none() {
        return Ok(None);
    }
    c.query_row(
        "SELECT MAX(closed_at) FROM reviews WHERE guild_id = ?1 AND status = 'done'
         AND ((?2 IS NOT NULL AND user_id = ?2) OR (?3 IS NOT NULL AND twitch_user = ?3 COLLATE NOCASE))",
        params![guild as i64, user.map(|u| u as i64), viewer],
        |r| r.get(0),
    )
}

/// Move waitlisted requests whose time has come into the queue, keeping when they first asked.
/// Requests whose wait is over join the queue. They queue from the time they became eligible,
/// so the waitlist doesn't let a repeat asker pass people waiting for a first review.
pub fn promote_due(ctx: &Ctx, now: i64) -> Result<usize> {
    ctx.db.with(|c| c.execute("UPDATE reviews SET status = 'open' WHERE status = 'waiting' AND eligible_at <= ?1", [now]))
}

/// The last 50 finished or removed, newest first.
pub fn closed(ctx: &Ctx, guild: u64) -> Result<Vec<Review>> {
    ctx.db.with(|c| {
        let mut st = c.prepare(&format!("SELECT {COLS} FROM reviews WHERE guild_id = ?1 AND status IN ('done', 'removed') ORDER BY closed_at DESC, id DESC LIMIT 50"))?;
        st.query_map([guild as i64], row)?.collect()
    })
}

/// Whether removing this request would give channel points back: a redemption of it is still
/// untouched on Twitch, or its fulfil hasn't gone through yet.
pub fn refundable(ctx: &Ctx, guild: u64, review: i64) -> bool {
    ctx.db
        .with(|c| {
            c.query_row(
                "SELECT EXISTS (SELECT 1 FROM reviews_sources WHERE guild_id = ?1 AND review_id = ?2 AND kind = 'redemption' AND reward_id IS NOT NULL AND (twitch IS NULL OR twitch_pending = 1))",
                params![guild as i64, review],
                |r| r.get(0),
            )
        })
        .unwrap_or(false)
}

/// Whether this request's channel points were given back, so it can't return to the queue.
pub fn refunded(ctx: &Ctx, guild: u64, review: i64) -> bool {
    ctx.db
        .with(|c| {
            c.query_row(
                "SELECT EXISTS (SELECT 1 FROM reviews_sources WHERE guild_id = ?1 AND review_id = ?2 AND twitch = 'CANCELED')",
                params![guild as i64, review],
                |r| r.get(0),
            )
        })
        .unwrap_or(false)
}

pub fn get(ctx: &Ctx, guild: u64, id: i64) -> Result<Option<Review>> {
    ctx.db.with(|c| c.query_row(&format!("SELECT {COLS} FROM reviews WHERE guild_id = ?1 AND id = ?2"), params![guild as i64, id], row).optional())
}

/// Reviews finished in the last week.
pub fn done_since(ctx: &Ctx, guild: u64, since: i64) -> Result<i64> {
    ctx.db.with(|c| c.query_row("SELECT COUNT(*) FROM reviews WHERE guild_id = ?1 AND status = 'done' AND closed_at >= ?2", params![guild as i64, since], |r| r.get(0)))
}

/// A request as it arrives from a thread or a Twitch redemption.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub name: String,
    pub user_id: Option<u64>,
    pub thread_id: Option<u64>,
    pub twitch_user: Option<String>,
    pub reward_id: Option<String>,
    pub redemption_id: Option<String>,
    pub redeemed_at: Option<i64>,
    pub requested_at: i64,
    pub notes: String,
    pub source: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Added {
    /// A new open request with this id.
    New(i64),
    /// Someone asked again within the wait after their last review: on the waitlist with this
    /// id until the time given.
    Waiting(i64, i64),
    /// The same as this open or waitlisted request: what it lacked (thread, member, channel
    /// points) was added.
    Merged(i64),
    /// Its thread or redemption was seen before, whatever happened to it since.
    Known,
}

/// What a scan or a sync did with what it found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tally {
    pub added: usize,
    pub waiting: usize,
    pub merged: usize,
    pub known: usize,
}

impl Tally {
    pub fn count(&mut self, a: Added) {
        match a {
            Added::New(_) => self.added += 1,
            Added::Waiting(..) => self.waiting += 1,
            Added::Merged(_) => self.merged += 1,
            Added::Known => self.known += 1,
        }
    }
}

/// Add a request unless it's already here. A thread or redemption is never added twice, even
/// after it was finished or removed, or merged into another request. Among open and waitlisted
/// requests, the same member, the same Twitch viewer or a name at least [`SAME_NAME`] alike is
/// the same request. Someone whose last review was completed less than `wait_ms` ago goes on the
/// waitlist until then; 0 turns the waitlist off.
pub fn add(ctx: &Ctx, guild: u64, r: &Request, wait_ms: i64) -> Result<Added> {
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    let sources: Vec<(&str, String)> = r.thread_id.map(|t| ("thread", t.to_string())).into_iter().chain(r.redemption_id.clone().map(|d| ("redemption", d))).collect();
    for (kind, ext) in &sources {
        let seen = tx
            .query_row("SELECT 1 FROM reviews_sources WHERE guild_id = ?1 AND kind = ?2 AND ext_id = ?3", params![guild as i64, kind, ext], |_| Ok(()))
            .optional()?
            .is_some();
        if seen {
            return Ok(Added::Known);
        }
    }
    let mut active = open_in(&tx, guild)?;
    {
        let mut st = tx.prepare(&format!("SELECT {COLS} FROM reviews WHERE guild_id = ?1 AND status = 'waiting' ORDER BY requested_at, id"))?;
        active.extend(st.query_map([guild as i64], row)?.collect::<bot_core::rusqlite::Result<Vec<_>>>()?);
    }
    let (id, added) = match duplicate_of(&active, r) {
        Some(d) => {
            // The request keeps its first post and redemption for links; the rest are in
            // reviews_sources.
            tx.execute(
                "UPDATE reviews SET thread_id = COALESCE(thread_id, ?3), user_id = COALESCE(user_id, ?4),
                 twitch_user = COALESCE(twitch_user, ?5), reward_id = COALESCE(reward_id, ?6),
                 redemption_id = COALESCE(redemption_id, ?7), redeemed_at = COALESCE(redeemed_at, ?8),
                 notes = CASE WHEN notes = '' THEN ?9 ELSE notes END
                 WHERE guild_id = ?1 AND id = ?2",
                params![
                    guild as i64,
                    d.id,
                    r.thread_id.map(|t| t as i64),
                    r.user_id.map(|u| u as i64),
                    r.twitch_user,
                    r.redemption_id.as_ref().filter(|_| d.redemption_id.is_none()).and(r.reward_id.clone()),
                    r.redemption_id.as_ref().filter(|_| d.redemption_id.is_none()),
                    r.redeemed_at,
                    r.notes,
                ],
            )?;
            (d.id, Added::Merged(d.id))
        }
        None => {
            let last = last_done(&tx, guild, r.user_id, r.twitch_user.as_deref())?;
            let eligible = waitlist_until(last, wait_ms, now_ms());
            tx.execute(
                "INSERT INTO reviews (guild_id, name, user_id, thread_id, twitch_user, reward_id, redemption_id, redeemed_at, requested_at, notes, source, status, eligible_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    guild as i64,
                    r.name,
                    r.user_id.map(|u| u as i64),
                    r.thread_id.map(|t| t as i64),
                    r.twitch_user,
                    r.reward_id,
                    r.redemption_id,
                    r.redeemed_at,
                    r.requested_at,
                    r.notes,
                    r.source,
                    if eligible.is_some() { "waiting" } else { "open" },
                    eligible,
                ],
            )?;
            let id = tx.last_insert_rowid();
            (id, eligible.map_or(Added::New(id), |at| Added::Waiting(id, at)))
        }
    };
    for (kind, ext) in &sources {
        add_source(&tx, guild, kind, ext, id, r.reward_id.as_deref().filter(|_| *kind == "redemption"))?;
    }
    tx.commit()?;
    Ok(added)
}

/// When a new request from someone whose last review was completed at `last` joins the queue,
/// if it has to wait at all.
pub fn waitlist_until(last: Option<i64>, wait_ms: i64, now: i64) -> Option<i64> {
    let at = last? + wait_ms;
    (wait_ms > 0 && at > now).then_some(at)
}

/// Record a post or redemption as part of a request.
pub fn add_source(c: &Connection, guild: u64, kind: &str, ext_id: &str, review: i64, reward_id: Option<&str>) -> bot_core::rusqlite::Result<()> {
    c.execute(
        "INSERT OR IGNORE INTO reviews_sources (guild_id, kind, ext_id, review_id, reward_id) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![guild as i64, kind, ext_id, review, reward_id],
    )?;
    Ok(())
}

/// A channel points redemption the bot is setting on Twitch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redemption {
    pub id: String,
    pub reward_id: String,
    /// FULFILLED or CANCELED.
    pub status: String,
}

/// Set every redemption of a request that's still untouched on Twitch, or waiting for Twitch,
/// to `status` (FULFILLED or CANCELED), pending until [`twitch_settled`]. Returns them.
pub fn set_twitch(ctx: &Ctx, guild: u64, review: i64, status: &str) -> Result<Vec<Redemption>> {
    ctx.db.with(|c| {
        c.execute(
            "UPDATE reviews_sources SET twitch = ?3, twitch_pending = 1
             WHERE guild_id = ?1 AND review_id = ?2 AND kind = 'redemption' AND reward_id IS NOT NULL AND (twitch IS NULL OR twitch_pending = 1)",
            params![guild as i64, review, status],
        )?;
        let mut st = c.prepare("SELECT ext_id, reward_id, twitch FROM reviews_sources WHERE guild_id = ?1 AND review_id = ?2 AND kind = 'redemption' AND twitch_pending = 1")?;
        st.query_map(params![guild as i64, review], |r| Ok(Redemption { id: r.get(0)?, reward_id: r.get(1)?, status: r.get(2)? }))?.collect()
    })
}

/// Redemptions whose status Twitch hasn't taken yet, oldest first, for the next sync to retry.
pub fn pending_twitch(ctx: &Ctx, guild: u64) -> Result<Vec<Redemption>> {
    ctx.db.with(|c| {
        let mut st = c.prepare("SELECT ext_id, reward_id, twitch FROM reviews_sources WHERE guild_id = ?1 AND twitch_pending = 1 AND reward_id IS NOT NULL AND twitch IS NOT NULL ORDER BY rowid LIMIT 50")?;
        st.query_map([guild as i64], |r| Ok(Redemption { id: r.get(0)?, reward_id: r.get(1)?, status: r.get(2)? }))?.collect()
    })
}

/// Twitch took the status, or the redemption is gone from its queue.
pub fn twitch_settled(ctx: &Ctx, guild: u64, redemption: &str) -> Result<()> {
    ctx.db.with(|c| c.execute("UPDATE reviews_sources SET twitch_pending = 0 WHERE guild_id = ?1 AND kind = 'redemption' AND ext_id = ?2", params![guild as i64, redemption]))?;
    Ok(())
}

/// The open or waitlisted request this one repeats, if any. Two similar names from two different known
/// members, or two different Twitch viewers, are two requests (two people titling their posts
/// "Account review", or two viewers typing "main").
pub fn duplicate_of<'a>(open: &'a [Review], r: &Request) -> Option<&'a Review> {
    open.iter().find(|o| {
        let same_member = o.user_id.is_some() && o.user_id == r.user_id;
        let same_viewer = matches!((&o.twitch_user, &r.twitch_user), (Some(a), Some(b)) if a.eq_ignore_ascii_case(b));
        let other_member = matches!((o.user_id, r.user_id), (Some(a), Some(b)) if a != b);
        let other_viewer = matches!((&o.twitch_user, &r.twitch_user), (Some(a), Some(b)) if !a.eq_ignore_ascii_case(b));
        same_member || same_viewer || (!other_member && !other_viewer && similarity(&o.name, &r.name) >= SAME_NAME)
    })
}

/// How alike two names are, from 0 to 1: one minus the edit distance over the longer length,
/// ignoring case and outer spaces. Names whose lengths differ by more than a fifth score 0.
pub fn similarity(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.trim().to_lowercase().chars().collect();
    let b: Vec<char> = b.trim().to_lowercase().chars().collect();
    if a == b {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let longer = a.len().max(b.len());
    if a.len().abs_diff(b.len()) as f64 > longer as f64 * 0.2 {
        return 0.0;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    1.0 - prev[b.len()] as f64 / longer as f64
}

pub fn set_notes(ctx: &Ctx, guild: u64, id: i64, notes: &str) -> Result<()> {
    ctx.db.with(|c| c.execute("UPDATE reviews SET notes = ?3 WHERE guild_id = ?1 AND id = ?2", params![guild as i64, id, notes]))?;
    Ok(())
}

/// Ping pressed by `by`: a new ping starts over, so an earlier "ready" no longer counts.
pub fn mark_pinged(ctx: &Ctx, guild: u64, id: i64, by: u64) -> Result<()> {
    ctx.db.with(|c| {
        c.execute("UPDATE reviews SET pinged_at = ?3, pinged_by = ?4, ready_at = NULL WHERE guild_id = ?1 AND id = ?2", params![guild as i64, id, now_ms(), by as i64])
    })?;
    Ok(())
}

/// The open or waitlisted request a post belongs to, its first post or one merged into it.
pub fn by_thread(ctx: &Ctx, guild: u64, thread: u64) -> Result<Option<Review>> {
    ctx.db.with(|c| {
        c.query_row(
            &format!(
                "SELECT {COLS} FROM reviews WHERE guild_id = ?1 AND status IN ('open', 'waiting') AND id = (
                   SELECT review_id FROM reviews_sources WHERE guild_id = ?1 AND kind = 'thread' AND ext_id = ?2)"
            ),
            params![guild as i64, thread.to_string()],
            row,
        )
        .optional()
    })
}

/// The member's newest toolbox link; it replaces an older one.
pub fn set_toolbox(ctx: &Ctx, guild: u64, id: i64, url: &str) -> Result<()> {
    ctx.db.with(|c| c.execute("UPDATE reviews SET toolbox = ?3 WHERE guild_id = ?1 AND id = ?2", params![guild as i64, id, url]))?;
    Ok(())
}

/// Add tags a request doesn't have yet, keeping them in their order. Says whether any were new.
pub fn add_tags(ctx: &Ctx, guild: u64, id: i64, tags: &[String]) -> Result<bool> {
    if tags.is_empty() {
        return Ok(false);
    }
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    let Some(have) = tx.query_row("SELECT tags FROM reviews WHERE guild_id = ?1 AND id = ?2", params![guild as i64, id], |r| r.get::<_, String>(0)).optional()? else {
        return Ok(false);
    };
    let mut all = split_tags(&have);
    let before = all.len();
    for t in tags {
        if !all.contains(t) {
            all.push(t.clone());
        }
    }
    if all.len() == before {
        return Ok(false);
    }
    all.sort_by_key(|t| crate::words::tag_rank(t));
    tx.execute("UPDATE reviews SET tags = ?3 WHERE guild_id = ?1 AND id = ?2", params![guild as i64, id, all.join(",")])?;
    tx.commit()?;
    Ok(true)
}

/// Mark a pinged request ready, once per ping: only for an answer `said_at` after the ping, and
/// only if it isn't ready already. Says whether this answer was the one.
pub fn claim_ready(ctx: &Ctx, guild: u64, id: i64, said_at: i64) -> Result<bool> {
    let n = ctx.db.with(|c| {
        c.execute(
            // 10 s of slack: the ping is stamped after it posted, by this machine's clock, and a
            // quick answer is stamped by Discord's.
            "UPDATE reviews SET ready_at = ?3 WHERE guild_id = ?1 AND id = ?2 AND status = 'open' AND pinged_at IS NOT NULL AND pinged_at <= ?4 + 10000 AND ready_at IS NULL",
            params![guild as i64, id, now_ms(), said_at],
        )
    })?;
    Ok(n > 0)
}

/// How long after a post the bot waits before asking for a toolbox link, so the member can add
/// it in their first messages.
pub const NUDGE_AFTER_MS: i64 = 3 * 60_000;
/// Posts older than this aren't asked: a scan finding last week's posts shouldn't ping them all.
pub const NUDGE_WITHIN_MS: i64 = 86_400_000;

/// Requests still waiting their turn from a post made since [`since_v4`], between
/// [`NUDGE_WITHIN_MS`] and [`NUDGE_AFTER_MS`] ago, with a member, no toolbox link, no ping and no
/// ask yet.
pub fn nudge_due(ctx: &Ctx, guild: u64, now: i64) -> Result<Vec<Review>> {
    let since = since_v4(ctx).max(now - NUDGE_WITHIN_MS);
    let list: Vec<Review> = ctx.db.with(|c| {
        let mut st = c.prepare(&format!(
            "SELECT {COLS} FROM reviews WHERE guild_id = ?1 AND status IN ('open', 'waiting') AND thread_id IS NOT NULL AND user_id IS NOT NULL
             AND toolbox IS NULL AND nudged_at IS NULL AND pinged_at IS NULL AND requested_at BETWEEN ?2 AND ?3 ORDER BY requested_at LIMIT 20"
        ))?;
        st.query_map(params![guild as i64, since, now - NUDGE_AFTER_MS], row)?.collect()
    })?;
    // A link staff put in the notes counts. Those are taken off the list for good, so they
    // don't fill every run's 20.
    let (noted, due): (Vec<Review>, Vec<Review>) = list.into_iter().partition(|r| r.toolbox_link().is_some());
    ctx.db.with(|c| {
        for r in &noted {
            c.execute("UPDATE reviews SET toolbox = ?3 WHERE guild_id = ?1 AND id = ?2", params![guild as i64, r.id, r.toolbox_link()])?;
        }
        Ok(())
    })?;
    Ok(due)
}

/// Take the lock of a finished request's post, so the button, the dashboard and the timer
/// don't all lock it. Says whether this caller should lock it.
pub fn claim_lock(ctx: &Ctx, guild: u64, id: i64) -> Result<bool> {
    let n = ctx.db.with(|c| {
        c.execute(
            "UPDATE reviews SET locked_at = ?3 WHERE guild_id = ?1 AND id = ?2 AND status = 'done' AND thread_id IS NOT NULL AND locked_at IS NULL",
            params![guild as i64, id, now_ms()],
        )
    })?;
    Ok(n > 0)
}

/// The post isn't locked (any more): locking failed, or the request went back to the queue.
pub fn release_lock(ctx: &Ctx, guild: u64, id: i64) -> Result<()> {
    ctx.db.with(|c| c.execute("UPDATE reviews SET locked_at = NULL WHERE guild_id = ?1 AND id = ?2", params![guild as i64, id]))?;
    Ok(())
}

/// When this version's columns were added: the timer and the reminder leave alone what came
/// before.
pub fn since_v4(ctx: &Ctx) -> i64 {
    ctx.db.with(|c| c.query_row("SELECT at FROM reviews_marks WHERE name = 'v4'", [], |r| r.get(0))).unwrap_or(i64::MAX)
}

/// How long the timer waits after failing to lock a post before trying it again.
pub const LOCK_RETRY_MS: i64 = 3_600_000;

/// Finished requests whose post is still open `after_ms` after the review: finished since
/// [`since_v4`] and in the last week, and not failed in the last [`LOCK_RETRY_MS`], so one post
/// the bot can't lock doesn't hold up the others.
pub fn lock_due(ctx: &Ctx, guild: u64, now: i64, after_ms: i64) -> Result<Vec<Review>> {
    let since = since_v4(ctx).max(now - after_ms - 7 * 86_400_000);
    ctx.db.with(|c| {
        let mut st = c.prepare(&format!(
            "SELECT {COLS} FROM reviews WHERE guild_id = ?1 AND status = 'done' AND thread_id IS NOT NULL AND locked_at IS NULL
             AND closed_at BETWEEN ?2 AND ?3 AND (lock_failed_at IS NULL OR lock_failed_at <= ?4) ORDER BY closed_at LIMIT 20"
        ))?;
        st.query_map(params![guild as i64, since, now - after_ms, now - LOCK_RETRY_MS], row)?.collect()
    })
}

/// The timer couldn't lock this post: let the lock go and wait [`LOCK_RETRY_MS`].
pub fn lock_failed(ctx: &Ctx, guild: u64, id: i64) -> Result<()> {
    ctx.db.with(|c| c.execute("UPDATE reviews SET locked_at = NULL, lock_failed_at = ?3 WHERE guild_id = ?1 AND id = ?2", params![guild as i64, id, now_ms()]))?;
    Ok(())
}

/// Take the one ask for a toolbox link, before asking, so two runs can't both ask.
pub fn claim_nudge(ctx: &Ctx, guild: u64, id: i64) -> Result<bool> {
    let n = ctx.db.with(|c| c.execute("UPDATE reviews SET nudged_at = ?3 WHERE guild_id = ?1 AND id = ?2 AND nudged_at IS NULL", params![guild as i64, id, now_ms()]))?;
    Ok(n > 0)
}

/// What staff do to a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    /// Open to done.
    Complete,
    /// Open or waitlisted to removed.
    Remove,
    /// Done, or removed without channel points (those were refunded), back to open.
    Reopen,
    /// Waitlisted to open now.
    Promote,
}

/// Move a request if it may go there from where it is, and say whether it moved, so a second
/// click or a stale page does nothing. Back in the queue, a fulfil Twitch hasn't taken yet is
/// dropped.
pub fn claim(ctx: &Ctx, guild: u64, id: i64, to: Move, by: &str) -> Result<bool> {
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    let moved = match to {
        Move::Complete => tx.execute(
            "UPDATE reviews SET status = 'done', closed_at = ?3, closed_by = ?4 WHERE guild_id = ?1 AND id = ?2 AND status = 'open'",
            params![guild as i64, id, now_ms(), by],
        )?,
        Move::Remove => tx.execute(
            "UPDATE reviews SET status = 'removed', closed_at = ?3, closed_by = ?4 WHERE guild_id = ?1 AND id = ?2 AND status IN ('open', 'waiting')",
            params![guild as i64, id, now_ms(), by],
        )?,
        // Moved early, they queue from now, like everyone else from the waitlist.
        Move::Promote => tx.execute(
            "UPDATE reviews SET status = 'open', eligible_at = ?3 WHERE guild_id = ?1 AND id = ?2 AND status = 'waiting'",
            params![guild as i64, id, now_ms()],
        )?,
        Move::Reopen => {
            let n = tx.execute(
                "UPDATE reviews SET status = 'open', closed_at = NULL, closed_by = NULL
                 WHERE guild_id = ?1 AND id = ?2 AND (status = 'done' OR (status = 'removed' AND NOT EXISTS (
                   SELECT 1 FROM reviews_sources x WHERE x.guild_id = ?1 AND x.review_id = ?2 AND x.twitch = 'CANCELED')))",
                params![guild as i64, id],
            )?;
            if n > 0 {
                tx.execute("UPDATE reviews_sources SET twitch = NULL, twitch_pending = 0 WHERE guild_id = ?1 AND review_id = ?2 AND twitch_pending = 1", params![guild as i64, id])?;
            }
            n
        }
    };
    tx.commit()?;
    Ok(moved > 0)
}

/// What a viewer typed when redeeming: who the review is for, and anything after it.
/// "review for Mira - check alchemy" is ("Mira", "check alchemy"); "Mira (main)" is
/// ("Mira", "main").
pub fn parse_input(input: &str) -> (String, String) {
    let mut s = input.trim();
    for prefix in ["reviewing for ", "review for ", "for ", "reviewing ", "review "] {
        // `get`, not slicing: the byte after the prefix's length may fall inside a character.
        if s.len() > prefix.len() && s.get(..prefix.len()).is_some_and(|p| p.eq_ignore_ascii_case(prefix)) {
            s = s[prefix.len()..].trim_start();
            break;
        }
    }
    if let Some(open) = s.find('(')
        && s.ends_with(')')
        && open > 0
    {
        return (s[..open].trim().to_owned(), s[open + 1..s.len() - 1].trim().to_owned());
    }
    for sep in [", ", " - ", " – "] {
        if let Some(i) = s.find(sep)
            && i > 0
        {
            return (s[..i].trim().to_owned(), s[i + sep.len()..].trim().to_owned());
        }
    }
    (s.to_owned(), String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review(id: i64, name: &str, user: Option<u64>, twitch: Option<&str>) -> Review {
        Review { id, name: name.into(), user_id: user, twitch_user: twitch.map(Into::into), status: "open".into(), source: "thread".into(), ..Review::default() }
    }

    #[test]
    fn names_compare_like_the_old_bot() {
        assert_eq!(similarity("Mira", " mira "), 1.0);
        assert!(similarity("AnthoMain", "AnthoMainn") >= SAME_NAME);
        assert!(similarity("bramblepaw", "bramblepow") >= SAME_NAME);
        assert!(similarity("Teo", "Leo") < SAME_NAME);
        // Lengths a fifth apart never match.
        assert_eq!(similarity("Kasia", "Kasia the Great"), 0.0);
        assert_eq!(similarity("", "x"), 0.0);
    }

    #[test]
    fn duplicates_by_member_viewer_or_name() {
        let open = vec![review(1, "AnthoMain", Some(10), None), review(2, "Pavel", None, Some("pavel_tv"))];
        let req = |name: &str, user: Option<u64>, twitch: Option<&str>| Request { name: name.into(), user_id: user, twitch_user: twitch.map(Into::into), ..Request::default() };
        assert_eq!(duplicate_of(&open, &req("Something else", Some(10), None)).map(|r| r.id), Some(1));
        assert_eq!(duplicate_of(&open, &req("Main acct", None, Some("PAVEL_TV"))).map(|r| r.id), Some(2));
        assert_eq!(duplicate_of(&open, &req("anthomainn", None, None)).map(|r| r.id), Some(1));
        // A similar name from someone else is their own request.
        assert_eq!(duplicate_of(&open, &req("AnthoMainn", Some(11), None)), None);
        assert_eq!(duplicate_of(&open, &req("Noor", Some(12), None)), None);
        // The same for two viewers on Twitch.
        assert_eq!(duplicate_of(&open, &req("Pavel", None, Some("noor_tv"))), None);
        // A post and a redemption with the same name are one request.
        assert_eq!(duplicate_of(&open, &req("Pavel", Some(13), None)).map(|r| r.id), Some(2));
    }

    #[test]
    fn redemption_input_names_who_it_is_for() {
        assert_eq!(parse_input("Mira"), ("Mira".into(), String::new()));
        assert_eq!(parse_input("review for Mira - check alchemy"), ("Mira".into(), "check alchemy".into()));
        assert_eq!(parse_input("For John Doe, main account"), ("John Doe".into(), "main account".into()));
        assert_eq!(parse_input("Mira (second account)"), ("Mira".into(), "second account".into()));
        assert_eq!(parse_input("  "), (String::new(), String::new()));
        assert_eq!(parse_input("review"), ("review".into(), String::new()));
        // A character straddling a prefix's length used to panic (and abort the bot).
        assert_eq!(parse_input("ミラ"), ("ミラ".into(), String::new()));
        assert_eq!(parse_input("Café au lait"), ("Café au lait".into(), String::new()));
        assert_eq!(parse_input("review for ミラ"), ("ミラ".into(), String::new()));
        assert_eq!(parse_input("Ash 🐸"), ("Ash 🐸".into(), String::new()));
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn requests_are_added_once() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate("reviews", MIGRATIONS).unwrap();
        let thread = Request { name: "AnthoMain".into(), user_id: Some(10), thread_id: Some(500), requested_at: 1, source: "thread", ..Request::default() };
        let Added::New(id) = add(&ctx, 1, &thread, 0).unwrap() else { panic!("first add is new") };
        assert_eq!(add(&ctx, 1, &thread, 0).unwrap(), Added::Known);
        // The same thread in another server is another request.
        assert!(matches!(add(&ctx, 2, &thread, 0).unwrap(), Added::New(_)));
        // A redemption for the same account moves it up instead of adding a second one.
        let points = Request { name: "anthomain".into(), twitch_user: Some("antho".into()), reward_id: Some("rw".into()), redemption_id: Some("rd1".into()), redeemed_at: Some(5), requested_at: 5, source: "twitch", ..Request::default() };
        assert_eq!(add(&ctx, 1, &points, 0).unwrap(), Added::Merged(id));
        let r = get(&ctx, 1, id).unwrap().unwrap();
        assert_eq!((r.redeemed_at, r.redemption_id.as_deref(), r.reward_id.as_deref(), r.thread_id), (Some(5), Some("rd1"), Some("rw"), Some(500)));
        // Removed stays as a marker: neither the thread nor the redemption comes back.
        assert!(claim(&ctx, 1, id, Move::Remove, "Antho").unwrap());
        assert_eq!(add(&ctx, 1, &thread, 0).unwrap(), Added::Known);
        assert_eq!(add(&ctx, 1, &points, 0).unwrap(), Added::Known);
        assert!(open(&ctx, 1).unwrap().is_empty());
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn merged_posts_and_redemptions_are_remembered() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate("reviews", MIGRATIONS).unwrap();
        // A second post from the same member merges, and stays known after Complete.
        let post = |thread: u64| Request { name: "AnthoMain".into(), user_id: Some(10), thread_id: Some(thread), requested_at: 1, source: "thread", ..Request::default() };
        let Added::New(id) = add(&ctx, 1, &post(500), 0).unwrap() else { panic!("first post is new") };
        assert_eq!(add(&ctx, 1, &post(501), 0).unwrap(), Added::Merged(id));
        assert!(claim(&ctx, 1, id, Move::Complete, "Antho").unwrap());
        assert_eq!(add(&ctx, 1, &post(501), 0).unwrap(), Added::Known);
        assert!(open(&ctx, 1).unwrap().is_empty());
        // A second redemption from the same viewer merges, is known on the next sync, and is
        // fulfilled with the first.
        let points = |rd: &str| Request { name: "Moonfrost".into(), twitch_user: Some("pavel_tv".into()), reward_id: Some("rw".into()), redemption_id: Some(rd.into()), redeemed_at: Some(5), requested_at: 5, source: "twitch", ..Request::default() };
        let Added::New(id) = add(&ctx, 1, &points("rd1"), 0).unwrap() else { panic!("first redemption is new") };
        assert_eq!(add(&ctx, 1, &points("rd2"), 0).unwrap(), Added::Merged(id));
        assert_eq!(add(&ctx, 1, &points("rd2"), 0).unwrap(), Added::Known);
        let ids = |list: Vec<Redemption>| list.into_iter().map(|r| (r.id, r.status)).collect::<Vec<_>>();
        let f = |id: &str| (id.to_owned(), "FULFILLED".to_owned());
        assert_eq!(ids(set_twitch(&ctx, 1, id, "FULFILLED").unwrap()), [f("rd1"), f("rd2")]);
        // One Twitch took stays done; the other is still pending.
        twitch_settled(&ctx, 1, "rd1").unwrap();
        assert_eq!(ids(set_twitch(&ctx, 1, id, "FULFILLED").unwrap()), [f("rd2")]);
        assert_eq!(ids(pending_twitch(&ctx, 1).unwrap()), [f("rd2")]);
        twitch_settled(&ctx, 1, "rd2").unwrap();
        assert!(set_twitch(&ctx, 1, id, "FULFILLED").unwrap().is_empty());
        assert!(pending_twitch(&ctx, 1).unwrap().is_empty());
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn each_action_claims_its_request_once() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate("reviews", MIGRATIONS).unwrap();
        let post = Request { name: "Teo".into(), user_id: Some(10), thread_id: Some(500), requested_at: 1, source: "thread", ..Request::default() };
        let Added::New(id) = add(&ctx, 1, &post, 0).unwrap() else { panic!("new") };
        assert!(claim(&ctx, 1, id, Move::Complete, "Antho").unwrap());
        // A double click completes once, and a done request can't be removed.
        assert!(!claim(&ctx, 1, id, Move::Complete, "Antho").unwrap());
        assert!(!claim(&ctx, 1, id, Move::Remove, "Antho").unwrap());
        assert!(claim(&ctx, 1, id, Move::Reopen, "Antho").unwrap());
        assert!(!claim(&ctx, 1, id, Move::Reopen, "Antho").unwrap());
        // A removed post can come back; a removed redemption was refunded and can't.
        assert!(claim(&ctx, 1, id, Move::Remove, "Antho").unwrap());
        assert!(claim(&ctx, 1, id, Move::Reopen, "Antho").unwrap());
        let points = Request { name: "Moonfrost".into(), twitch_user: Some("pavel_tv".into()), reward_id: Some("rw".into()), redemption_id: Some("rd1".into()), redeemed_at: Some(5), requested_at: 5, source: "twitch", ..Request::default() };
        let Added::New(paid) = add(&ctx, 1, &points, 0).unwrap() else { panic!("new") };
        assert!(claim(&ctx, 1, paid, Move::Remove, "Antho").unwrap());
        // Removing cancels the redemption, as the page does: refunded, so it can't come back.
        assert!(refundable(&ctx, 1, paid));
        set_twitch(&ctx, 1, paid, "CANCELED").unwrap();
        assert!(refunded(&ctx, 1, paid));
        assert!(!claim(&ctx, 1, paid, Move::Reopen, "Antho").unwrap());
        // Completed and fulfilled, then back in the queue: removing it refunds nothing, and it
        // can come back again.
        let later = Request { name: "Kasia".into(), twitch_user: Some("kasia_plays".into()), reward_id: Some("rw".into()), redemption_id: Some("rd9".into()), redeemed_at: Some(6), requested_at: 6, source: "twitch", ..Request::default() };
        let Added::New(done) = add(&ctx, 1, &later, 0).unwrap() else { panic!("new") };
        assert!(claim(&ctx, 1, done, Move::Complete, "Antho").unwrap());
        set_twitch(&ctx, 1, done, "FULFILLED").unwrap();
        twitch_settled(&ctx, 1, "rd9").unwrap();
        assert!(claim(&ctx, 1, done, Move::Reopen, "Antho").unwrap());
        assert!(!refundable(&ctx, 1, done));
        assert!(claim(&ctx, 1, done, Move::Remove, "Antho").unwrap());
        assert!(claim(&ctx, 1, done, Move::Reopen, "Antho").unwrap());
    }

    #[test]
    fn the_waitlist_runs_from_the_last_review() {
        let day = 86_400_000;
        // Reviewed 2 days ago with a 7-day wait: joins 5 days from now.
        assert_eq!(waitlist_until(Some(10 * day), 7 * day, 12 * day), Some(17 * day));
        // The wait is over, there was no review, or the waitlist is off.
        assert_eq!(waitlist_until(Some(10 * day), 7 * day, 17 * day), None);
        assert_eq!(waitlist_until(None, 7 * day, 12 * day), None);
        assert_eq!(waitlist_until(Some(10 * day), 0, 12 * day), None);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn asking_again_soon_goes_on_the_waitlist() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate("reviews", MIGRATIONS).unwrap();
        let week = 7 * 86_400_000;
        let post = |thread: u64, name: &str| Request { name: name.into(), user_id: Some(10), thread_id: Some(thread), requested_at: thread as i64, source: "thread", ..Request::default() };
        let Added::New(first) = add(&ctx, 1, &post(500, "AnthoMain"), week).unwrap() else { panic!("first is new") };
        assert!(claim(&ctx, 1, first, Move::Complete, "Antho").unwrap());
        let done_at = get(&ctx, 1, first).unwrap().unwrap().closed_at.unwrap();
        // A new post from the same member is a re-request, waiting until a week after the review.
        let Added::Waiting(again, at) = add(&ctx, 1, &post(501, "AnthoMain"), week).unwrap() else { panic!("a re-request waits") };
        assert_eq!(at, done_at + week);
        // The old post never comes back, and a second re-request merges into the waiting one.
        assert_eq!(add(&ctx, 1, &post(500, "AnthoMain"), week).unwrap(), Added::Known);
        assert_eq!(add(&ctx, 1, &post(502, "Alt account"), week).unwrap(), Added::Merged(again));
        assert!(open(&ctx, 1).unwrap().is_empty());
        assert_eq!(counts(&ctx, 1).unwrap(), (0, 1));
        // Someone else goes straight in.
        let other = Request { name: "Noor".into(), user_id: Some(11), thread_id: Some(600), requested_at: 600, source: "thread", ..Request::default() };
        let Added::New(noor) = add(&ctx, 1, &other, week).unwrap() else { panic!("someone else is new") };
        // A Twitch viewer reviewed before waits too, with their channel points.
        let points = |rd: &str| Request { name: "Moonfrost".into(), twitch_user: Some("Pavel_TV".into()), reward_id: Some("rw".into()), redemption_id: Some(rd.into()), redeemed_at: Some(700), requested_at: 700, source: "twitch", ..Request::default() };
        let Added::New(paid) = add(&ctx, 1, &points("rd1"), week).unwrap() else { panic!("new") };
        assert!(claim(&ctx, 1, paid, Move::Complete, "Antho").unwrap());
        let Added::Waiting(paid_again, _) = add(&ctx, 1, &points("rd2"), week).unwrap() else { panic!("a viewer's re-request waits") };
        // Nothing is due yet; once it is, both join the queue with when they asked, points first.
        assert_eq!(promote_due(&ctx, done_at + 1000).unwrap(), 0);
        assert_eq!(promote_due(&ctx, now_ms() + week).unwrap(), 2);
        let queue: Vec<(i64, i64)> = open(&ctx, 1).unwrap().into_iter().map(|r| (r.id, r.requested_at)).collect();
        assert_eq!(queue[0], (paid_again, 700));
        assert!(queue.contains(&(again, 501)));
        // Staff can move one to the queue early, once.
        assert!(claim(&ctx, 1, noor, Move::Complete, "Antho").unwrap());
        let Added::Waiting(early, _) = add(&ctx, 1, &Request { thread_id: Some(601), ..other }, week).unwrap() else { panic!("waits") };
        assert!(claim(&ctx, 1, early, Move::Promote, "Antho").unwrap());
        assert!(!claim(&ctx, 1, early, Move::Promote, "Antho").unwrap());
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn existing_requests_fill_the_sources() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate("reviews", &MIGRATIONS[..1]).unwrap();
        ctx.db
            .with(|c| {
                c.execute_batch(
                    "INSERT INTO reviews (guild_id, name, thread_id, reward_id, redemption_id, requested_at, status, source) VALUES
                     (1, 'Kasia', 500, 'rw', 'rd1', 1, 'done', 'thread'), (1, 'Teo', 501, NULL, NULL, 2, 'open', 'thread')",
                )
            })
            .unwrap();
        ctx.db.migrate("reviews", MIGRATIONS).unwrap();
        let req = |thread: Option<u64>, rd: Option<&str>| Request { name: "Someone new".into(), thread_id: thread, redemption_id: rd.map(Into::into), source: "thread", ..Request::default() };
        assert_eq!(add(&ctx, 1, &req(Some(500), None), 0).unwrap(), Added::Known);
        assert_eq!(add(&ctx, 1, &req(Some(501), None), 0).unwrap(), Added::Known);
        assert_eq!(add(&ctx, 1, &req(None, Some("rd1")), 0).unwrap(), Added::Known);
        // A done request's redemption is retried once in case its fulfil failed.
        let pending: i64 = ctx.db.with(|c| c.query_row("SELECT twitch_pending FROM reviews_sources WHERE ext_id = 'rd1'", [], |r| r.get(0))).unwrap();
        assert_eq!(pending, 1);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn channel_points_go_first_then_the_oldest() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate("reviews", MIGRATIONS).unwrap();
        for (name, user, at, redeemed) in [("Old", 1, 10, None), ("Newer", 2, 20, None), ("Paid", 3, 30, Some(30))] {
            add(&ctx, 1, &Request { name: name.into(), user_id: Some(user), requested_at: at, redeemed_at: redeemed, source: "thread", ..Request::default() }, 0).unwrap();
        }
        let names: Vec<String> = open(&ctx, 1).unwrap().into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["Paid", "Old", "Newer"]);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn a_ping_is_answered_ready_once() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate("reviews", MIGRATIONS).unwrap();
        let post = Request { name: "Mira".into(), user_id: Some(10), thread_id: Some(500), requested_at: 1, source: "thread", ..Request::default() };
        let Added::New(id) = add(&ctx, 1, &post, 0).unwrap() else { panic!("new") };
        // Not pinged yet: "ready" means nothing.
        assert!(!claim_ready(&ctx, 1, id, now_ms()).unwrap());
        mark_pinged(&ctx, 1, id, 77).unwrap();
        let r = get(&ctx, 1, id).unwrap().unwrap();
        assert_eq!((r.pinged_by, r.ready_at), (Some(77), None));
        let pinged = r.pinged_at.unwrap();
        // An answer from well before the ping doesn't count; one after it does, once.
        assert!(!claim_ready(&ctx, 1, id, pinged - 60_000).unwrap());
        assert!(claim_ready(&ctx, 1, id, pinged + 1).unwrap());
        assert!(!claim_ready(&ctx, 1, id, pinged + 2).unwrap());
        assert!(get(&ctx, 1, id).unwrap().unwrap().ready_at.is_some());
        // Pinging again starts over.
        mark_pinged(&ctx, 1, id, 78).unwrap();
        let r = get(&ctx, 1, id).unwrap().unwrap();
        assert_eq!((r.pinged_by, r.ready_at), (Some(78), None));
        assert!(claim_ready(&ctx, 1, id, now_ms()).unwrap());
        // Done requests don't turn ready.
        mark_pinged(&ctx, 1, id, 78).unwrap();
        assert!(claim(&ctx, 1, id, Move::Complete, "Antho").unwrap());
        assert!(!claim_ready(&ctx, 1, id, now_ms()).unwrap());
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn posts_find_their_request_with_links_and_tags() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate("reviews", MIGRATIONS).unwrap();
        let post = |thread: u64| Request { name: "Mira".into(), user_id: Some(10), thread_id: Some(thread), requested_at: 1, source: "thread", ..Request::default() };
        let Added::New(id) = add(&ctx, 1, &post(500), 0).unwrap() else { panic!("new") };
        assert_eq!(add(&ctx, 1, &post(501), 0).unwrap(), Added::Merged(id));
        // Both posts lead to the request; another server's post of the same id doesn't.
        assert_eq!(by_thread(&ctx, 1, 500).unwrap().map(|r| r.id), Some(id));
        assert_eq!(by_thread(&ctx, 1, 501).unwrap().map(|r| r.id), Some(id));
        assert_eq!(by_thread(&ctx, 2, 500).unwrap(), None);
        assert_eq!(by_thread(&ctx, 1, 999).unwrap(), None);
        // The newest link wins.
        set_toolbox(&ctx, 1, id, "https://idleontoolbox.com/?profile=Old").unwrap();
        set_toolbox(&ctx, 1, id, "https://idleontoolbox.com/?profile=Mira").unwrap();
        assert_eq!(get(&ctx, 1, id).unwrap().unwrap().toolbox.as_deref(), Some("https://idleontoolbox.com/?profile=Mira"));
        // Tags gather without repeats, stages first.
        let t = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(add_tags(&ctx, 1, id, &t(&["W5"])).unwrap());
        assert!(add_tags(&ctx, 1, id, &t(&["Late game", "W5"])).unwrap());
        assert!(!add_tags(&ctx, 1, id, &t(&["W5"])).unwrap());
        assert!(!add_tags(&ctx, 1, id, &[]).unwrap());
        assert_eq!(get(&ctx, 1, id).unwrap().unwrap().tags, ["Late game", "W5"]);
        // A finished request is no longer found by its post.
        assert!(claim(&ctx, 1, id, Move::Complete, "Antho").unwrap());
        assert_eq!(by_thread(&ctx, 1, 500).unwrap(), None);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn posts_without_a_link_are_asked_once() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate("reviews", MIGRATIONS).unwrap();
        // As if this version had been running for a while.
        ctx.db.with(|c| c.execute("UPDATE reviews_marks SET at = 0", [])).unwrap();
        let now = now_ms();
        let min = 60_000;
        let post = |name: &str, user: Option<u64>, thread: Option<u64>, ago: i64, notes: &str| Request {
            name: name.into(),
            user_id: user,
            thread_id: thread,
            requested_at: now - ago,
            notes: notes.into(),
            source: "thread",
            ..Request::default()
        };
        let Added::New(due) = add(&ctx, 1, &post("Due", Some(10), Some(500), 10 * min, ""), 0).unwrap() else { panic!("new") };
        // Too new, too old, no member, no post, or a link in the notes or from the post.
        add(&ctx, 1, &post("Fresh", Some(11), Some(501), min, ""), 0).unwrap();
        add(&ctx, 1, &post("Old", Some(12), Some(502), 2 * NUDGE_WITHIN_MS, ""), 0).unwrap();
        add(&ctx, 1, &post("Twitch", None, Some(503), 10 * min, ""), 0).unwrap();
        add(&ctx, 1, &post("Nopost", Some(14), None, 10 * min, ""), 0).unwrap();
        add(&ctx, 1, &post("Noted", Some(15), Some(505), 10 * min, "idleontoolbox.com/?profile=Noted"), 0).unwrap();
        let Added::New(linked) = add(&ctx, 1, &post("Linked", Some(16), Some(506), 10 * min, ""), 0).unwrap() else { panic!("new") };
        set_toolbox(&ctx, 1, linked, "https://idleontoolbox.com/?profile=Linked").unwrap();
        // Already pinged: past the point of asking.
        let Added::New(pinged) = add(&ctx, 1, &post("Pinged", Some(17), Some(507), 10 * min, ""), 0).unwrap() else { panic!("new") };
        mark_pinged(&ctx, 1, pinged, 77).unwrap();
        let ids = |list: Vec<Review>| list.into_iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(ids(nudge_due(&ctx, 1, now).unwrap()), [due]);
        // The notes link became the request's link, so it isn't looked at again.
        let noted: i64 = ctx.db.with(|c| c.query_row("SELECT COUNT(*) FROM reviews WHERE name = 'Noted' AND toolbox IS NOT NULL", [], |r| r.get(0))).unwrap();
        assert_eq!(noted, 1);
        assert!(claim_nudge(&ctx, 1, due).unwrap());
        assert!(!claim_nudge(&ctx, 1, due).unwrap());
        assert!(nudge_due(&ctx, 1, now).unwrap().is_empty());
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn older_rows_take_the_new_columns() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate("reviews", &MIGRATIONS[..3]).unwrap();
        ctx.db.with(|c| c.execute_batch("INSERT INTO reviews (guild_id, name, requested_at, source, pinged_at) VALUES (1, 'Kasia', 1, 'thread', 5)")).unwrap();
        ctx.db.migrate("reviews", MIGRATIONS).unwrap();
        let r = open(&ctx, 1).unwrap().remove(0);
        assert_eq!((r.pinged_at, r.pinged_by, r.ready_at, r.toolbox, r.nudged_at, r.locked_at), (Some(5), None, None, None, None, None));
        assert!(r.tags.is_empty());
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn finished_posts_are_locked_once() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate("reviews", MIGRATIONS).unwrap();
        ctx.db.with(|c| c.execute("UPDATE reviews_marks SET at = 0", [])).unwrap();
        let day = 86_400_000;
        let post = |name: &str, user: u64, thread: Option<u64>| Request { name: name.into(), user_id: Some(user), thread_id: thread, requested_at: 1, source: "thread", ..Request::default() };
        let Added::New(id) = add(&ctx, 1, &post("Mira", 10, Some(500)), 0).unwrap() else { panic!("new") };
        let Added::New(twitch) = add(&ctx, 1, &post("Teo", 11, None), 0).unwrap() else { panic!("new") };
        // Open requests aren't locked.
        assert!(!claim_lock(&ctx, 1, id).unwrap());
        assert!(claim(&ctx, 1, id, Move::Complete, "Antho").unwrap());
        assert!(claim(&ctx, 1, twitch, Move::Complete, "Antho").unwrap());
        let now = now_ms();
        // Not due within the day; due after it, and only the one with a post.
        assert!(lock_due(&ctx, 1, now, day).unwrap().is_empty());
        let ids = |list: Vec<Review>| list.into_iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(ids(lock_due(&ctx, 1, now + day + 1000, day).unwrap()), [id]);
        // Older than a week past the timer: left alone.
        assert!(lock_due(&ctx, 1, now + 9 * day, day).unwrap().is_empty());
        assert!(claim_lock(&ctx, 1, id).unwrap());
        assert!(!claim_lock(&ctx, 1, id).unwrap());
        assert!(lock_due(&ctx, 1, now + day + 1000, day).unwrap().is_empty());
        // Back in the queue, the lock is let go, and it can be locked again once done.
        release_lock(&ctx, 1, id).unwrap();
        assert!(claim(&ctx, 1, id, Move::Reopen, "Antho").unwrap());
        assert!(!claim_lock(&ctx, 1, id).unwrap());
        assert!(claim(&ctx, 1, id, Move::Complete, "Antho").unwrap());
        assert!(claim_lock(&ctx, 1, id).unwrap());
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn a_failed_lock_waits_an_hour() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate("reviews", MIGRATIONS).unwrap();
        ctx.db.with(|c| c.execute("UPDATE reviews_marks SET at = 0", [])).unwrap();
        let day = 86_400_000;
        let post = |name: &str, user: u64, thread: u64| Request { name: name.into(), user_id: Some(user), thread_id: Some(thread), requested_at: 1, source: "thread", ..Request::default() };
        let Added::New(a) = add(&ctx, 1, &post("Mira", 10, 500), 0).unwrap() else { panic!("new") };
        let Added::New(b) = add(&ctx, 1, &post("Teo", 11, 501), 0).unwrap() else { panic!("new") };
        assert!(claim(&ctx, 1, a, Move::Complete, "Antho").unwrap());
        assert!(claim(&ctx, 1, b, Move::Complete, "Antho").unwrap());
        let later = now_ms() + day + 1000;
        let ids = |list: Vec<Review>| list.into_iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(ids(lock_due(&ctx, 1, later, day).unwrap()), [a, b]);
        // The bot couldn't lock the first: the other is next, and the first returns an hour on.
        assert!(claim_lock(&ctx, 1, a).unwrap());
        lock_failed(&ctx, 1, a).unwrap();
        assert_eq!(get(&ctx, 1, a).unwrap().unwrap().locked_at, None);
        // (With the timer at 0, so "due" is right away and only the retry wait matters.)
        assert_eq!(ids(lock_due(&ctx, 1, now_ms() + 1000, 0).unwrap()), [b]);
        assert_eq!(ids(lock_due(&ctx, 1, now_ms() + LOCK_RETRY_MS + 1000, 0).unwrap()), [a, b]);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn an_update_leaves_existing_posts_alone() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate("reviews", &MIGRATIONS[..3]).unwrap();
        let now = now_ms();
        let (h, min) = (3_600_000, 60_000);
        // Finished two days ago and asked ten minutes ago, both before this version.
        ctx.db
            .with(|c| {
                c.execute(
                    "INSERT INTO reviews (guild_id, name, user_id, thread_id, requested_at, status, source, closed_at) VALUES
                     (1, 'Old done', 10, 500, 1, 'done', 'thread', ?1), (1, 'Old ask', 11, 501, ?2, 'open', 'thread', NULL)",
                    params![now - 48 * h, now - 10 * min],
                )
            })
            .unwrap();
        ctx.db.migrate("reviews", MIGRATIONS).unwrap();
        assert!(since_v4(&ctx) <= now_ms() && since_v4(&ctx) > now - h);
        assert!(lock_due(&ctx, 1, now_ms(), 24 * h).unwrap().is_empty());
        assert!(nudge_due(&ctx, 1, now_ms()).unwrap().is_empty());
        // Nothing about them looks locked or asked.
        let old = closed(&ctx, 1).unwrap().remove(0);
        assert_eq!((old.locked_at, old.nudged_at), (None, None));
        // A post made after the update is asked as usual.
        let Added::New(new) = add(&ctx, 1, &Request { name: "New".into(), user_id: Some(12), thread_id: Some(502), requested_at: now_ms(), source: "thread", ..Request::default() }, 0).unwrap() else { panic!("new") };
        assert_eq!(nudge_due(&ctx, 1, now_ms() + NUDGE_AFTER_MS + 1000).unwrap().into_iter().map(|r| r.id).collect::<Vec<_>>(), [new]);
    }
}
