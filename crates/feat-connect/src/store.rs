//! The tables: requests to connect, the approval DMs each one sent, and the PCs that connected.

use bot_core::anyhow::Result;
use bot_core::rusqlite::{Connection, OptionalExtension, Row, params};
use bot_core::{Ctx, Migration};

use crate::domain::{self, MAX_PENDING};

pub const MIGRATIONS: &[Migration] = &[Migration {
    name: "001_connect",
    sql: "
        CREATE TABLE connect_requests (
            id TEXT PRIMARY KEY, kind TEXT NOT NULL, device_name TEXT NOT NULL,
            challenge TEXT NOT NULL, code TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'pending',
            ip TEXT, notified INTEGER NOT NULL DEFAULT 0, decided_by INTEGER, decided_on TEXT,
            claims INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL,
            decided_at INTEGER, claimed_at INTEGER, pc_link TEXT, host_id TEXT, linked_at INTEGER,
            dm_settled INTEGER NOT NULL DEFAULT 0);
        CREATE INDEX connect_requests_created ON connect_requests(created_at);
        CREATE INDEX connect_requests_status ON connect_requests(status, expires_at);
        CREATE TABLE connect_dms (
            request_id TEXT NOT NULL, user_id INTEGER NOT NULL, channel_id INTEGER NOT NULL,
            message_id INTEGER NOT NULL, PRIMARY KEY (request_id, user_id));
        CREATE TABLE connect_hosts (
            id TEXT PRIMARY KEY, request_id TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
            token_hash TEXT NOT NULL UNIQUE, created_at INTEGER NOT NULL, last_seen_at INTEGER,
            revoked_at INTEGER);
    ",
}, Migration {
    name: "002_dm_rounds",
    // 0: not asked yet, 1: approvers were DMed, 2: held back and rang the bell, 3: held back quietly.
    sql: "ALTER TABLE connect_requests ADD COLUMN dm_round INTEGER NOT NULL DEFAULT 0;
          CREATE INDEX connect_requests_ip ON connect_requests(ip, created_at);",
}];

#[derive(Debug, Clone)]
pub struct Request {
    pub id: String,
    pub kind: String,
    pub device: String,
    pub challenge: String,
    pub code: String,
    pub status: String,
    pub notified: bool,
    pub decided_by: Option<u64>,
    /// "discord" or "dashboard".
    pub decided_on: Option<String>,
    pub claims: i64,
    pub created_at: i64,
    pub expires_at: i64,
    pub decided_at: Option<i64>,
    pub pc_link: Option<String>,
}

impl Request {
    pub fn label(&self) -> String {
        domain::device_label(&self.kind, &self.device)
    }

    /// What the dashboard shows: the status, or "expired" for one that ran out unanswered or
    /// allowed but never collected.
    pub fn shown_state(&self, now: i64) -> &str {
        let state = domain::claim_state(&self.kind, &self.status, self.expires_at, self.claims, self.pc_link.is_some(), now);
        if matches!(self.status.as_str(), "pending" | "allowed") && state == domain::ClaimState::Expired {
            "expired"
        } else {
            &self.status
        }
    }
}

const COLS: &str = "id, kind, device_name, challenge, code, status, notified, decided_by, decided_on, claims, created_at, expires_at, decided_at, pc_link";

fn row(r: &Row) -> bot_core::rusqlite::Result<Request> {
    Ok(Request {
        id: r.get(0)?,
        kind: r.get(1)?,
        device: r.get(2)?,
        challenge: r.get(3)?,
        code: r.get(4)?,
        status: r.get(5)?,
        notified: r.get(6)?,
        decided_by: r.get::<_, Option<i64>>(7)?.map(|v| v as u64),
        decided_on: r.get(8)?,
        claims: r.get(9)?,
        created_at: r.get(10)?,
        expires_at: r.get(11)?,
        decided_at: r.get(12)?,
        pc_link: r.get(13)?,
    })
}

pub fn get(ctx: &Ctx, id: &str) -> Result<Option<Request>> {
    ctx.db.with(|c| c.query_row(&format!("SELECT {COLS} FROM connect_requests WHERE id = ?1"), [id], row).optional())
}

/// Why a new request was turned away, in the words the apps know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Busy,
    SlowDown,
}

pub struct NewRequest<'a> {
    pub kind: &'a str,
    pub device: &'a str,
    pub challenge: &'a str,
    pub ip: Option<&'a str>,
}

/// Check the limits and store a request in one go, so two devices asking at once can't both
/// slip under them.
pub fn create(ctx: &Ctx, new: &NewRequest, now: i64) -> Result<Result<Request, Refusal>> {
    let conn = ctx.db.lock();
    create_on(&conn, new, now)
}

fn create_on(c: &Connection, new: &NewRequest, now: i64) -> Result<Result<Request, Refusal>> {
    // Counted per app, so requests for one app can't shut the others out.
    let pending: i64 = c.query_row(
        "SELECT COUNT(*) FROM connect_requests WHERE status = 'pending' AND expires_at > ?1 AND kind = ?2",
        params![now, new.kind],
        |r| r.get(0),
    )?;
    if pending >= MAX_PENDING {
        return Ok(Err(Refusal::Busy));
    }
    if let Some(ip) = new.ip {
        // One open request per address: a loop on one machine can't keep the queue full.
        let waiting: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM connect_requests WHERE ip = ?1 AND status = 'pending' AND expires_at > ?2)",
            params![ip, now],
            |r| r.get(0),
        )?;
        if waiting {
            return Ok(Err(Refusal::SlowDown));
        }
        let recent: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM connect_requests WHERE ip = ?1 AND created_at >= ?2)",
            params![ip, now - domain::IP_COOLDOWN_MS],
            |r| r.get(0),
        )?;
        if recent {
            return Ok(Err(Refusal::SlowDown));
        }
    }
    let req = Request {
        id: domain::random_hex(16),
        kind: new.kind.to_owned(),
        device: domain::clean_device_name(new.device),
        challenge: new.challenge.to_owned(),
        code: domain::display_code(new.challenge),
        status: "pending".into(),
        notified: false,
        decided_by: None,
        decided_on: None,
        claims: 0,
        created_at: now,
        expires_at: now + domain::TTL_MS,
        decided_at: None,
        pc_link: None,
    };
    c.execute(
        "INSERT INTO connect_requests (id, kind, device_name, challenge, code, status, ip, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7, ?8)",
        params![req.id, req.kind, req.device, req.challenge, req.code, new.ip, req.created_at, req.expires_at],
    )?;
    Ok(Ok(req))
}

/// Whether a new request may DM the approvers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Round {
    Dm,
    /// Over the hourly allowance. `ring`: the first one held back this hour rings the bell.
    Quiet { ring: bool },
}

/// Count this request against the rolling hour of DM rounds and record which it got.
pub fn take_dm_round(ctx: &Ctx, id: &str, now: i64) -> Result<Round> {
    let conn = ctx.db.lock();
    let since = now - 3_600_000;
    let (rounds, rung): (i64, bool) = conn.query_row(
        "SELECT COUNT(*) FILTER (WHERE dm_round = 1), COALESCE(MAX(dm_round = 2), 0)
         FROM connect_requests WHERE created_at >= ?1 AND id != ?2",
        params![since, id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let round = if rounds < domain::MAX_DM_ROUNDS_PER_HOUR { Round::Dm } else { Round::Quiet { ring: !rung } };
    let mark = match round {
        Round::Dm => 1,
        Round::Quiet { ring: true } => 2,
        Round::Quiet { ring: false } => 3,
    };
    conn.execute("UPDATE connect_requests SET dm_round = ?2 WHERE id = ?1", params![id, mark])?;
    Ok(round)
}

pub fn set_notified(ctx: &Ctx, id: &str) -> Result<()> {
    ctx.db.with(|c| c.execute("UPDATE connect_requests SET notified = 1 WHERE id = ?1", [id]))?;
    Ok(())
}

pub fn add_dm(ctx: &Ctx, id: &str, user: u64, channel: u64, message: u64) -> Result<()> {
    ctx.db.with(|c| {
        c.execute(
            "INSERT OR REPLACE INTO connect_dms (request_id, user_id, channel_id, message_id) VALUES (?1, ?2, ?3, ?4)",
            params![id, user as i64, channel as i64, message as i64],
        )
    })?;
    Ok(())
}

/// `(user, channel, message)` for every DM a request sent.
pub fn dms(ctx: &Ctx, id: &str) -> Result<Vec<(u64, u64, u64)>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached("SELECT user_id, channel_id, message_id FROM connect_dms WHERE request_id = ?1")?;
        s.query_map([id], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)? as u64)))?.collect()
    })
}

/// An answer; only a pending request that hasn't run out takes one. The row as it now stands.
pub fn decide(ctx: &Ctx, id: &str, allow: bool, by: u64, on: &str, now: i64) -> Result<Option<Request>> {
    let changed = ctx.db.with(|c| {
        c.execute(
            "UPDATE connect_requests SET status = ?2, decided_by = ?3, decided_on = ?4, decided_at = ?5, dm_settled = 1
             WHERE id = ?1 AND status = 'pending' AND expires_at > ?5",
            params![id, if allow { "allowed" } else { "denied" }, by as i64, on, now],
        )
    })?;
    if changed == 0 {
        return Ok(None);
    }
    get(ctx, id)
}

/// Count one pairing link against an allowed request before handing it out, in one statement so
/// two claims at once can't both slip under the limit. False when the request is past it, ran
/// out, or (for a Relay phone) has no link from a PC yet.
pub fn reserve_claim(ctx: &Ctx, id: &str, now: i64) -> Result<bool> {
    let n = ctx.db.with(|c| {
        c.execute(
            "UPDATE connect_requests SET status = 'claimed', claimed_at = ?2, claims = claims + 1
             WHERE id = ?1 AND status IN ('allowed', 'claimed') AND claims < ?3 AND expires_at > ?2
               AND (kind != 'relay' OR pc_link IS NOT NULL)",
            params![id, now, domain::MAX_CLAIMS],
        )
    })?;
    Ok(n > 0)
}

/// Give back a claim whose link couldn't be made, so the device can try again.
pub fn release_claim(ctx: &Ctx, id: &str) -> Result<()> {
    ctx.db.with(|c| {
        c.execute(
            "UPDATE connect_requests SET claims = claims - 1, status = CASE WHEN claims = 1 THEN 'allowed' ELSE status END
             WHERE id = ?1 AND claims > 0",
            [id],
        )
    })?;
    Ok(())
}

/// Requests that ran out unanswered and still have DMs showing buttons.
pub fn expired_unsettled(ctx: &Ctx, now: i64) -> Result<Vec<Request>> {
    ctx.db.with(|c| {
        let mut s = c.prepare(&format!("SELECT {COLS} FROM connect_requests WHERE status = 'pending' AND expires_at <= ?1 AND dm_settled = 0"))?;
        s.query_map([now], row)?.collect()
    })
}

pub fn mark_settled(ctx: &Ctx, id: &str) -> Result<()> {
    ctx.db.with(|c| c.execute("UPDATE connect_requests SET dm_settled = 1 WHERE id = ?1", [id]))?;
    Ok(())
}

pub fn prune(ctx: &Ctx, now: i64) -> Result<()> {
    let before = now - domain::KEEP_MS;
    ctx.db.with(|c| {
        c.execute("DELETE FROM connect_dms WHERE request_id IN (SELECT id FROM connect_requests WHERE created_at < ?1)", [before])?;
        c.execute("DELETE FROM connect_requests WHERE created_at < ?1", [before])
    })?;
    Ok(())
}

pub fn pending(ctx: &Ctx, now: i64) -> Result<Vec<Request>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(&format!(
            "SELECT {COLS} FROM connect_requests WHERE status = 'pending' AND expires_at > ?1 ORDER BY created_at"
        ))?;
        s.query_map([now], row)?.collect()
    })
}

/// Answered or run-out requests, newest first.
pub fn history(ctx: &Ctx, now: i64, limit: i64) -> Result<Vec<Request>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(&format!(
            "SELECT {COLS} FROM connect_requests WHERE status != 'pending' OR expires_at <= ?1
             ORDER BY COALESCE(decided_at, expires_at) DESC LIMIT ?2"
        ))?;
        s.query_map(params![now, limit], row)?.collect()
    })
}

/// Relay phones an approver allowed that no PC has opened a window for yet.
pub fn open_relay_requests(ctx: &Ctx, now: i64) -> Result<Vec<Request>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(&format!(
            "SELECT {COLS} FROM connect_requests
             WHERE kind = 'relay' AND status = 'allowed' AND pc_link IS NULL AND expires_at > ?1
             ORDER BY created_at LIMIT 5"
        ))?;
        s.query_map([now], row)?.collect()
    })
}

/// A PC's pairing link for an allowed Relay phone; only the first PC to answer sets it.
pub fn set_pc_link(ctx: &Ctx, id: &str, host: &str, link: &str, now: i64) -> Result<bool> {
    let n = ctx.db.with(|c| {
        c.execute(
            "UPDATE connect_requests SET pc_link = ?3, host_id = ?2, linked_at = ?4
             WHERE id = ?1 AND kind = 'relay' AND status = 'allowed' AND pc_link IS NULL AND expires_at > ?4",
            params![id, host, link, now],
        )
    })?;
    Ok(n > 0)
}

// ---- PCs ----

#[derive(Debug, Clone)]
pub struct Host {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub last_seen_at: Option<i64>,
}

/// The PC that connected through request `request_id`, with a new token hash. A PC that claims
/// its request again (its first try failed half way) replaces the token it was given before. A
/// PC someone forgot stays forgotten: false, and no token is stored.
pub fn upsert_host(ctx: &Ctx, request_id: &str, name: &str, token_hash: &str, now: i64) -> Result<bool> {
    let n = ctx.db.with(|c| {
        c.execute(
            "INSERT INTO connect_hosts (id, request_id, name, token_hash, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (request_id) DO UPDATE SET token_hash = excluded.token_hash, name = excluded.name
             WHERE connect_hosts.revoked_at IS NULL",
            params![domain::random_hex(8), request_id, name, token_hash, now],
        )
    })?;
    Ok(n > 0)
}

/// The PC holding the token behind `token_hash`, unless it was forgotten. Marks it seen.
pub fn host_by_token(ctx: &Ctx, token_hash: &str, now: i64) -> Result<Option<Host>> {
    let host = ctx.db.with(|c| {
        c.query_row(
            "SELECT id, name, created_at, last_seen_at FROM connect_hosts WHERE token_hash = ?1 AND revoked_at IS NULL",
            [token_hash],
            |r| Ok(Host { id: r.get(0)?, name: r.get(1)?, created_at: r.get(2)?, last_seen_at: r.get(3)? }),
        )
        .optional()
    })?;
    if let Some(h) = &host {
        ctx.db.with(|c| c.execute("UPDATE connect_hosts SET last_seen_at = ?2 WHERE id = ?1", params![h.id, now]))?;
    }
    Ok(host)
}

pub fn count_hosts(ctx: &Ctx) -> Result<i64> {
    ctx.db.with(|c| c.query_row("SELECT COUNT(*) FROM connect_hosts WHERE revoked_at IS NULL", [], |r| r.get(0)))
}

pub fn hosts(ctx: &Ctx) -> Result<Vec<Host>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(
            "SELECT id, name, created_at, last_seen_at FROM connect_hosts WHERE revoked_at IS NULL ORDER BY created_at DESC",
        )?;
        s.query_map([], |r| Ok(Host { id: r.get(0)?, name: r.get(1)?, created_at: r.get(2)?, last_seen_at: r.get(3)? }))?.collect()
    })
}

/// Forget a PC here: its token stops working, and the request it came through can't hand out
/// another, so a PC forgotten minutes after it was allowed can't claim its way back. Its cloud
/// link is revoked on the engine apart.
pub fn revoke_host(ctx: &Ctx, id: &str, now: i64) -> Result<Option<String>> {
    ctx.db.with(|c| {
        let gone: Option<(String, String)> = c
            .query_row(
                "UPDATE connect_hosts SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL RETURNING name, request_id",
                params![id, now],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((_, request)) = &gone {
            c.execute("UPDATE connect_requests SET claims = MAX(claims, ?2) WHERE id = ?1", params![request, domain::MAX_CLAIMS])?;
        }
        Ok(gone.map(|(name, _)| name))
    })
}

/// Whether the feature is on in any server. Read from the table rather than the gateway cache,
/// so a device that wakes a sleeping service gets a real answer before the gateway is ready.
pub fn enabled_anywhere(ctx: &Ctx) -> bool {
    ctx.db
        .with(|c| {
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM guild_features WHERE feature = ?1 AND enabled = 1)",
                [crate::KEY],
                |r| r.get(0),
            )
        })
        .unwrap_or(false)
}

/// A server whose dashboard page to link from a notice.
pub fn some_enabled_guild(ctx: &Ctx) -> Option<u64> {
    ctx.enabled_guilds(crate::KEY).first().copied().or_else(|| {
        ctx.db
            .with(|c| {
                c.query_row(
                    "SELECT guild_id FROM guild_features WHERE feature = ?1 AND enabled = 1 ORDER BY guild_id LIMIT 1",
                    [crate::KEY],
                    |r| r.get::<_, i64>(0),
                )
                .optional()
            })
            .ok()
            .flatten()
            .map(|g| g as u64)
    })
}
