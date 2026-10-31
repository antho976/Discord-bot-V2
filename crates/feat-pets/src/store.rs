//! Tables and queries: the per-server catalog, the collection, submissions waiting for review,
//! pet giveaways with their comments, and the giver ban list.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use bot_core::anyhow::Result;
use bot_core::rusqlite::{self, OptionalExtension, Row, params};
use bot_core::{Ctx, Migration, now_ms, serde_json};
use serde::Deserialize;

pub const MIGRATIONS: &[Migration] = &[Migration {
    name: "001_pets",
    sql: "
        CREATE TABLE pets_seeded (guild_id INTEGER PRIMARY KEY, at INTEGER NOT NULL);
        CREATE TABLE pets_categories (
            guild_id INTEGER NOT NULL, name TEXT NOT NULL, position INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (guild_id, name));
        CREATE TABLE pets_catalog (
            guild_id INTEGER NOT NULL, id TEXT NOT NULL, name TEXT NOT NULL, emoji TEXT NOT NULL DEFAULT '',
            category TEXT NOT NULL, rarity TEXT NOT NULL DEFAULT 'common', description TEXT NOT NULL DEFAULT '',
            bonus TEXT NOT NULL DEFAULT '', image TEXT NOT NULL DEFAULT '', position INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (guild_id, id));
        CREATE TABLE pets_owned (
            id INTEGER PRIMARY KEY, guild_id INTEGER NOT NULL, pet_id TEXT NOT NULL,
            added_by INTEGER NOT NULL, given_by INTEGER NOT NULL, added_at INTEGER NOT NULL,
            approved_by TEXT NOT NULL DEFAULT '');
        CREATE INDEX pets_owned_by_pet ON pets_owned(guild_id, pet_id);
        CREATE TABLE pets_pending (
            id INTEGER PRIMARY KEY, guild_id INTEGER NOT NULL, pet_id TEXT NOT NULL, user_id INTEGER NOT NULL,
            quantity INTEGER NOT NULL DEFAULT 1, requested_at INTEGER NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending', reviewed_by TEXT, reviewed_at INTEGER,
            reason TEXT NOT NULL DEFAULT '');
        CREATE INDEX pets_pending_by_status ON pets_pending(guild_id, status, requested_at);
        CREATE TABLE pets_giveaways (
            id INTEGER PRIMARY KEY, guild_id INTEGER NOT NULL, pet_id TEXT NOT NULL, pet_name TEXT NOT NULL,
            pet_emoji TEXT NOT NULL DEFAULT '', pet_rarity TEXT NOT NULL DEFAULT 'common',
            giver_id INTEGER NOT NULL, winner_id INTEGER NOT NULL, notes TEXT NOT NULL DEFAULT '',
            recorded_by TEXT NOT NULL, recorded_at INTEGER NOT NULL, expires_at INTEGER,
            warned INTEGER NOT NULL DEFAULT 0, status TEXT NOT NULL DEFAULT 'pending',
            closed_by TEXT, closed_at INTEGER);
        CREATE INDEX pets_giveaways_by_status ON pets_giveaways(guild_id, status, expires_at);
        CREATE TABLE pets_giveaway_comments (
            id INTEGER PRIMARY KEY, giveaway_id INTEGER NOT NULL, author TEXT NOT NULL,
            text TEXT NOT NULL, at INTEGER NOT NULL);
        CREATE INDEX pets_comments_by_giveaway ON pets_giveaway_comments(giveaway_id);
        CREATE TABLE pets_bans (
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, reason TEXT NOT NULL DEFAULT '',
            banned_by TEXT NOT NULL, banned_at INTEGER NOT NULL, PRIMARY KEY (guild_id, user_id));
    ",
}];

// ---- rarity ----

pub const RARITIES: [&str; 4] = ["common", "uncommon", "rare", "legendary"];

pub fn rarity_label(r: &str) -> &'static str {
    match r {
        "uncommon" => "Uncommon",
        "rare" => "Rare",
        "legendary" => "Legendary",
        _ => "Common",
    }
}

pub fn rarity_rank(r: &str) -> usize {
    RARITIES.iter().position(|x| *x == r).unwrap_or(0)
}

pub fn clean_rarity(r: &str) -> &'static str {
    RARITIES.iter().copied().find(|x| *x == r).unwrap_or("common")
}

// ---- catalog ----

#[derive(Debug, Clone, Deserialize)]
pub struct Pet {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub emoji: String,
    pub category: String,
    #[serde(default)]
    pub rarity: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub bonus: String,
    #[serde(default)]
    pub image: String,
}

impl Pet {
    /// The emoji, or a paw when none is set.
    pub fn icon(&self) -> &str {
        if self.emoji.trim().is_empty() { "🐾" } else { &self.emoji }
    }
    /// An image URL that's safe to put in a page or an embed.
    pub fn image_url(&self) -> Option<&str> {
        let u = self.image.trim();
        u.starts_with("https://").then_some(u)
    }
}

#[derive(Deserialize)]
struct Seed {
    categories: Vec<String>,
    catalog: Vec<Pet>,
}

const SEED: &str = include_str!("../data/catalog.json");

static SEEDED: Mutex<Option<HashSet<u64>>> = Mutex::new(None);

/// Put the stock Idleon catalog in a server the first time the feature is used there.
pub fn ensure_seeded(ctx: &Ctx, guild: u64) -> Result<()> {
    if SEEDED.lock().unwrap().as_ref().is_some_and(|s| s.contains(&guild)) {
        return Ok(());
    }
    let seed: Seed = serde_json::from_str(SEED)?;
    {
        let mut conn = ctx.db.lock();
        let tx = conn.transaction()?;
        let fresh = tx.execute("INSERT OR IGNORE INTO pets_seeded (guild_id, at) VALUES (?1, ?2)", params![guild as i64, now_ms()])? == 1;
        if fresh {
            for (i, c) in seed.categories.iter().enumerate() {
                tx.execute("INSERT OR IGNORE INTO pets_categories (guild_id, name, position) VALUES (?1, ?2, ?3)", params![guild as i64, c, i as i64])?;
            }
            for (i, p) in seed.catalog.iter().enumerate() {
                tx.execute(
                    "INSERT OR IGNORE INTO pets_catalog (guild_id, id, name, emoji, category, rarity, description, bonus, image, position)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![guild as i64, p.id, p.name, p.emoji, p.category, clean_rarity(&p.rarity), p.description, p.bonus, p.image, i as i64],
                )?;
            }
        }
        tx.commit()?;
    }
    SEEDED.lock().unwrap().get_or_insert_with(HashSet::new).insert(guild);
    Ok(())
}

const PET_COLS: &str = "id, name, emoji, category, rarity, description, bonus, image";

fn pet_row(r: &Row) -> rusqlite::Result<Pet> {
    Ok(Pet {
        id: r.get(0)?,
        name: r.get(1)?,
        emoji: r.get(2)?,
        category: r.get(3)?,
        rarity: r.get(4)?,
        description: r.get(5)?,
        bonus: r.get(6)?,
        image: r.get(7)?,
    })
}

/// The whole catalog in display order.
pub fn catalog(ctx: &Ctx, guild: u64) -> Result<Vec<Pet>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(&format!("SELECT {PET_COLS} FROM pets_catalog WHERE guild_id = ?1 ORDER BY position, name"))?;
        s.query_map([guild as i64], pet_row)?.collect()
    })
}

pub fn pet(ctx: &Ctx, guild: u64, id: &str) -> Result<Option<Pet>> {
    ctx.db.with(|c| {
        c.query_row(&format!("SELECT {PET_COLS} FROM pets_catalog WHERE guild_id = ?1 AND id = ?2"), params![guild as i64, id], pet_row).optional()
    })
}

/// A pet by id, or by name when someone typed instead of picking a suggestion.
pub fn find_pet(ctx: &Ctx, guild: u64, input: &str) -> Result<Option<Pet>> {
    if let Some(p) = pet(ctx, guild, input)? {
        return Ok(Some(p));
    }
    let needle = input.trim().to_lowercase();
    Ok(catalog(ctx, guild)?.into_iter().find(|p| p.name.to_lowercase() == needle))
}

/// Category names in order, including any a pet uses that isn't in the list.
pub fn categories(ctx: &Ctx, guild: u64) -> Result<Vec<String>> {
    let mut out: Vec<String> = ctx.db.with(|c| {
        let mut s = c.prepare_cached("SELECT name FROM pets_categories WHERE guild_id = ?1 ORDER BY position, name")?;
        s.query_map([guild as i64], |r| r.get(0))?.collect()
    })?;
    for p in catalog(ctx, guild)? {
        if !out.contains(&p.category) {
            out.push(p.category);
        }
    }
    Ok(out)
}

pub fn add_category(ctx: &Ctx, guild: u64, name: &str) -> Result<bool> {
    ctx.db.with(|c| {
        let n = c.execute(
            "INSERT OR IGNORE INTO pets_categories (guild_id, name, position)
             VALUES (?1, ?2, (SELECT COALESCE(MAX(position), -1) + 1 FROM pets_categories WHERE guild_id = ?1))",
            params![guild as i64, name],
        )?;
        Ok(n > 0)
    })
}

/// Remove an empty category. Returns false when pets still use it.
pub fn delete_category(ctx: &Ctx, guild: u64, name: &str) -> Result<bool> {
    ctx.db.with(|c| {
        let used: i64 = c.query_row("SELECT COUNT(*) FROM pets_catalog WHERE guild_id = ?1 AND category = ?2", params![guild as i64, name], |r| r.get(0))?;
        if used > 0 {
            return Ok(false);
        }
        c.execute("DELETE FROM pets_categories WHERE guild_id = ?1 AND name = ?2", params![guild as i64, name])?;
        Ok(true)
    })
}

/// A url-safe id from a name, unique in the server's catalog.
pub fn new_pet_id(ctx: &Ctx, guild: u64, name: &str) -> Result<String> {
    let base = slug(name);
    let base = if base.is_empty() { "pet".to_owned() } else { base };
    let mut id = base.clone();
    let mut n = 2;
    while pet(ctx, guild, &id)?.is_some() {
        id = format!("{base}-{n}");
        n += 1;
    }
    Ok(id)
}

pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(48).collect()
}

/// Insert or replace a catalog entry. New entries go to the end.
pub fn save_pet(ctx: &Ctx, guild: u64, p: &Pet) -> Result<()> {
    add_category(ctx, guild, &p.category)?;
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO pets_catalog (guild_id, id, name, emoji, category, rarity, description, bonus, image, position)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, (SELECT COALESCE(MAX(position), -1) + 1 FROM pets_catalog WHERE guild_id = ?1))
             ON CONFLICT (guild_id, id) DO UPDATE SET name = excluded.name, emoji = excluded.emoji, category = excluded.category,
               rarity = excluded.rarity, description = excluded.description, bonus = excluded.bonus, image = excluded.image",
            params![guild as i64, p.id, p.name, p.emoji, p.category, clean_rarity(&p.rarity), p.description, p.bonus, p.image],
        )
    })?;
    Ok(())
}

/// Remove a pet from the catalog and every copy of it from the collection. Returns the copies
/// removed.
pub fn delete_pet(ctx: &Ctx, guild: u64, id: &str) -> Result<usize> {
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    let owned = tx.execute("DELETE FROM pets_owned WHERE guild_id = ?1 AND pet_id = ?2", params![guild as i64, id])?;
    tx.execute("DELETE FROM pets_catalog WHERE guild_id = ?1 AND id = ?2", params![guild as i64, id])?;
    tx.execute("DELETE FROM pets_pending WHERE guild_id = ?1 AND pet_id = ?2 AND status = 'pending'", params![guild as i64, id])?;
    tx.commit()?;
    Ok(owned)
}

// ---- collection ----

#[derive(Debug, Clone)]
pub struct Owned {
    pub pet_id: String,
    pub added_by: u64,
    pub given_by: u64,
    pub added_at: i64,
}

/// How many of each pet the server owns.
pub fn owned_counts(ctx: &Ctx, guild: u64) -> Result<HashMap<String, i64>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached("SELECT pet_id, COUNT(*) FROM pets_owned WHERE guild_id = ?1 GROUP BY pet_id")?;
        s.query_map([guild as i64], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
    })
}

/// Companions whose first copy joined the collection since `since`.
pub fn first_added_since(ctx: &Ctx, guild: u64, since: i64) -> Result<i64> {
    ctx.db.with(|c| {
        c.query_row(
            "SELECT COUNT(*) FROM (SELECT pet_id FROM pets_owned WHERE guild_id = ?1 GROUP BY pet_id HAVING MIN(added_at) >= ?2)",
            [guild as i64, since],
            |r| r.get(0),
        )
    })
}

pub fn owned_total(ctx: &Ctx, guild: u64) -> Result<i64> {
    ctx.db.with(|c| c.query_row("SELECT COUNT(*) FROM pets_owned WHERE guild_id = ?1", [guild as i64], |r| r.get(0)))
}

/// Copies a member added or gave, newest first. The old bot counted both.
pub fn contributions(ctx: &Ctx, guild: u64, user: u64) -> Result<Vec<Owned>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(
            "SELECT pet_id, added_by, given_by, added_at FROM pets_owned
             WHERE guild_id = ?1 AND (added_by = ?2 OR given_by = ?2) ORDER BY added_at DESC",
        )?;
        s
            .query_map(params![guild as i64, user as i64], |r| {
                Ok(Owned { pet_id: r.get(0)?, added_by: r.get::<_, i64>(1)? as u64, given_by: r.get::<_, i64>(2)? as u64, added_at: r.get(3)? })
            })?
            .collect()
    })
}

/// Copies added since `since`, one row per pet, member and time (a submission lands together):
/// `(pet name, added by, copies, added at)`, newest first.
pub fn recent_additions(ctx: &Ctx, guild: u64, since: i64, limit: i64) -> Result<Vec<(String, u64, i64, i64)>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(
            "SELECT COALESCE(p.name, o.pet_id), o.added_by, COUNT(*), o.added_at FROM pets_owned o
             LEFT JOIN pets_catalog p ON p.guild_id = o.guild_id AND p.id = o.pet_id
             WHERE o.guild_id = ?1 AND o.added_at >= ?2
             GROUP BY o.pet_id, p.name, o.added_by, o.added_at ORDER BY o.added_at DESC LIMIT ?3",
        )?;
        s.query_map(params![guild as i64, since, limit], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64, r.get(2)?, r.get(3)?)))?
            .collect()
    })
}

pub fn add_owned(ctx: &Ctx, guild: u64, pet_id: &str, added_by: u64, given_by: u64, qty: i64, approved_by: &str) -> Result<()> {
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    insert_owned(&tx, guild, pet_id, added_by, given_by, qty, approved_by)?;
    tx.commit()?;
    Ok(())
}

fn insert_owned(c: &rusqlite::Connection, guild: u64, pet_id: &str, added_by: u64, given_by: u64, qty: i64, approved_by: &str) -> rusqlite::Result<()> {
    let now = now_ms();
    for _ in 0..qty.max(0) {
        c.execute(
            "INSERT INTO pets_owned (guild_id, pet_id, added_by, given_by, added_at, approved_by) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![guild as i64, pet_id, added_by as i64, given_by as i64, now, approved_by],
        )?;
    }
    Ok(())
}

/// Remove up to `qty` copies, oldest first, preferring copies `prefer` gave. Returns how many
/// went.
pub fn remove_owned(ctx: &Ctx, guild: u64, pet_id: &str, qty: i64, prefer: Option<u64>) -> Result<i64> {
    ctx.db.with(|c| {
        let n = c.execute(
            "DELETE FROM pets_owned WHERE id IN (
               SELECT id FROM pets_owned WHERE guild_id = ?1 AND pet_id = ?2
               ORDER BY (given_by = ?3) DESC, added_at, id LIMIT ?4)",
            params![guild as i64, pet_id, prefer.map(|p| p as i64).unwrap_or(-1), qty.max(0)],
        )?;
        Ok(n as i64)
    })
}

// ---- submissions ----

#[derive(Debug, Clone)]
pub struct Pending {
    pub id: i64,
    pub pet_id: String,
    pub user_id: u64,
    pub quantity: i64,
    pub requested_at: i64,
    pub status: String,
    pub reviewed_by: Option<String>,
    pub reviewed_at: Option<i64>,
    pub reason: String,
}

const PENDING_COLS: &str = "id, pet_id, user_id, quantity, requested_at, status, reviewed_by, reviewed_at, reason";

fn pending_row(r: &Row) -> rusqlite::Result<Pending> {
    Ok(Pending {
        id: r.get(0)?,
        pet_id: r.get(1)?,
        user_id: r.get::<_, i64>(2)? as u64,
        quantity: r.get(3)?,
        requested_at: r.get(4)?,
        status: r.get(5)?,
        reviewed_by: r.get(6)?,
        reviewed_at: r.get(7)?,
        reason: r.get(8)?,
    })
}

pub fn submit(ctx: &Ctx, guild: u64, pet_id: &str, user: u64, qty: i64) -> Result<i64> {
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO pets_pending (guild_id, pet_id, user_id, quantity, requested_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![guild as i64, pet_id, user as i64, qty, now_ms()],
        )?;
        Ok(c.last_insert_rowid())
    })
}

/// Submissions still waiting, oldest first.
pub fn pending(ctx: &Ctx, guild: u64) -> Result<Vec<Pending>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(&format!("SELECT {PENDING_COLS} FROM pets_pending WHERE guild_id = ?1 AND status = 'pending' ORDER BY requested_at, id"))?;
        s.query_map([guild as i64], pending_row)?.collect()
    })
}

pub fn pending_for(ctx: &Ctx, guild: u64, user: u64) -> Result<Vec<Pending>> {
    Ok(pending(ctx, guild)?.into_iter().filter(|p| p.user_id == user).collect())
}

/// Submissions waiting for review. A submission of 3 counts once: it's one decision.
pub fn pending_count(ctx: &Ctx, guild: u64) -> Result<i64> {
    ctx.db.with(|c| c.query_row("SELECT COUNT(*) FROM pets_pending WHERE guild_id = ?1 AND status = 'pending'", [guild as i64], |r| r.get(0)))
}

pub fn reviewed(ctx: &Ctx, guild: u64, limit: i64) -> Result<Vec<Pending>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(&format!(
            "SELECT {PENDING_COLS} FROM pets_pending WHERE guild_id = ?1 AND status != 'pending' ORDER BY reviewed_at DESC, id DESC LIMIT ?2"
        ))?;
        s.query_map(params![guild as i64, limit], pending_row)?.collect()
    })
}

/// Approve or reject submissions that are still pending. Approved ones join the collection.
/// Returns the submissions that changed.
pub fn review(ctx: &Ctx, guild: u64, ids: &[i64], approve: bool, reviewer: &str, reason: &str) -> Result<Vec<Pending>> {
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    let mut changed = Vec::new();
    let now = now_ms();
    for id in ids {
        let row = tx
            .query_row(
                &format!("SELECT {PENDING_COLS} FROM pets_pending WHERE guild_id = ?1 AND id = ?2 AND status = 'pending'"),
                params![guild as i64, id],
                pending_row,
            )
            .optional()?;
        let Some(mut p) = row else { continue };
        let status = if approve { "approved" } else { "rejected" };
        tx.execute(
            "UPDATE pets_pending SET status = ?3, reviewed_by = ?4, reviewed_at = ?5, reason = ?6 WHERE guild_id = ?1 AND id = ?2",
            params![guild as i64, id, status, reviewer, now, reason],
        )?;
        if approve {
            insert_owned(&tx, guild, &p.pet_id, p.user_id, p.user_id, p.quantity, reviewer)?;
        }
        p.status = status.into();
        p.reviewed_by = Some(reviewer.into());
        p.reviewed_at = Some(now);
        p.reason = reason.into();
        changed.push(p);
    }
    tx.commit()?;
    Ok(changed)
}

// ---- giveaways ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pending,
    Confirmed,
    Expired,
    Cancelled,
}

impl Status {
    pub fn parse(s: &str) -> Self {
        match s {
            "confirmed" => Status::Confirmed,
            "expired" => Status::Expired,
            "cancelled" => Status::Cancelled,
            _ => Status::Pending,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Confirmed => "confirmed",
            Status::Expired => "expired",
            Status::Cancelled => "cancelled",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Status::Pending => "Waiting",
            Status::Confirmed => "Confirmed",
            Status::Expired => "Expired",
            Status::Cancelled => "Cancelled",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Giveaway {
    pub id: i64,
    pub pet_id: String,
    pub pet_name: String,
    pub pet_emoji: String,
    pub pet_rarity: String,
    pub giver_id: u64,
    pub winner_id: u64,
    pub notes: String,
    pub recorded_by: String,
    pub recorded_at: i64,
    pub expires_at: Option<i64>,
    pub warned: bool,
    pub status: Status,
    pub closed_by: Option<String>,
    pub closed_at: Option<i64>,
}

impl Giveaway {
    pub fn icon(&self) -> &str {
        if self.pet_emoji.trim().is_empty() { "🐾" } else { &self.pet_emoji }
    }
}

const GIVEAWAY_COLS: &str =
    "id, pet_id, pet_name, pet_emoji, pet_rarity, giver_id, winner_id, notes, recorded_by, recorded_at, expires_at, warned, status, closed_by, closed_at";

fn giveaway_row(r: &Row) -> rusqlite::Result<Giveaway> {
    Ok(Giveaway {
        id: r.get(0)?,
        pet_id: r.get(1)?,
        pet_name: r.get(2)?,
        pet_emoji: r.get(3)?,
        pet_rarity: r.get(4)?,
        giver_id: r.get::<_, i64>(5)? as u64,
        winner_id: r.get::<_, i64>(6)? as u64,
        notes: r.get(7)?,
        recorded_by: r.get(8)?,
        recorded_at: r.get(9)?,
        expires_at: r.get(10)?,
        warned: r.get(11)?,
        status: Status::parse(&r.get::<_, String>(12)?),
        closed_by: r.get(13)?,
        closed_at: r.get(14)?,
    })
}

/// Waiting giveaways, soonest expiry first.
pub fn active_giveaways(ctx: &Ctx, guild: u64) -> Result<Vec<Giveaway>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(&format!(
            "SELECT {GIVEAWAY_COLS} FROM pets_giveaways WHERE guild_id = ?1 AND status = 'pending'
             ORDER BY expires_at IS NULL, expires_at, recorded_at"
        ))?;
        s.query_map([guild as i64], giveaway_row)?.collect()
    })
}

/// Finished giveaways, newest first.
pub fn past_giveaways(ctx: &Ctx, guild: u64, limit: i64) -> Result<Vec<Giveaway>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(&format!(
            "SELECT {GIVEAWAY_COLS} FROM pets_giveaways WHERE guild_id = ?1 AND status != 'pending'
             ORDER BY COALESCE(closed_at, recorded_at) DESC LIMIT ?2"
        ))?;
        s.query_map(params![guild as i64, limit], giveaway_row)?.collect()
    })
}

pub fn all_giveaways(ctx: &Ctx, guild: u64) -> Result<Vec<Giveaway>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(&format!("SELECT {GIVEAWAY_COLS} FROM pets_giveaways WHERE guild_id = ?1 ORDER BY recorded_at"))?;
        s.query_map([guild as i64], giveaway_row)?.collect()
    })
}

pub fn giveaway(ctx: &Ctx, guild: u64, id: i64) -> Result<Option<Giveaway>> {
    ctx.db.with(|c| {
        c.query_row(&format!("SELECT {GIVEAWAY_COLS} FROM pets_giveaways WHERE guild_id = ?1 AND id = ?2"), params![guild as i64, id], giveaway_row)
            .optional()
    })
}

pub struct NewGiveaway<'a> {
    pub pet: &'a Pet,
    pub giver: u64,
    pub winner: u64,
    pub notes: &'a str,
    pub recorded_by: &'a str,
    pub expires_at: Option<i64>,
}

pub fn record_giveaway(ctx: &Ctx, guild: u64, g: &NewGiveaway) -> Result<i64> {
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO pets_giveaways (guild_id, pet_id, pet_name, pet_emoji, pet_rarity, giver_id, winner_id, notes, recorded_by, recorded_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                guild as i64,
                g.pet.id,
                g.pet.name,
                g.pet.emoji,
                clean_rarity(&g.pet.rarity),
                g.giver as i64,
                g.winner as i64,
                g.notes,
                g.recorded_by,
                now_ms(),
                g.expires_at
            ],
        )?;
        Ok(c.last_insert_rowid())
    })
}

/// Close a waiting giveaway. Returns false when it wasn't waiting any more.
pub fn close_giveaway(ctx: &Ctx, guild: u64, id: i64, to: Status, by: &str) -> Result<bool> {
    ctx.db.with(|c| {
        let n = c.execute(
            "UPDATE pets_giveaways SET status = ?3, closed_by = ?4, closed_at = ?5 WHERE guild_id = ?1 AND id = ?2 AND status = 'pending'",
            params![guild as i64, id, to.as_str(), by, now_ms()],
        )?;
        Ok(n > 0)
    })
}

pub fn mark_warned(ctx: &Ctx, guild: u64, id: i64) -> Result<()> {
    ctx.db.with(|c| c.execute("UPDATE pets_giveaways SET warned = 1 WHERE guild_id = ?1 AND id = ?2", params![guild as i64, id]))?;
    Ok(())
}

pub fn delete_giveaway(ctx: &Ctx, guild: u64, id: i64) -> Result<()> {
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    let n = tx.execute("DELETE FROM pets_giveaways WHERE guild_id = ?1 AND id = ?2", params![guild as i64, id])?;
    if n > 0 {
        tx.execute("DELETE FROM pets_giveaway_comments WHERE giveaway_id = ?1", [id])?;
    }
    tx.commit()?;
    Ok(())
}

/// Waiting giveaways that expire within `within_ms`, or already have.
pub fn expiring(ctx: &Ctx, guild: u64, within_ms: i64) -> Result<Vec<Giveaway>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(&format!(
            "SELECT {GIVEAWAY_COLS} FROM pets_giveaways WHERE guild_id = ?1 AND status = 'pending'
             AND expires_at IS NOT NULL AND expires_at <= ?2"
        ))?;
        s.query_map(params![guild as i64, now_ms() + within_ms], giveaway_row)?.collect()
    })
}

#[derive(Debug, Clone)]
pub struct Comment {
    pub id: i64,
    pub author: String,
    pub text: String,
    pub at: i64,
}

/// Comments for a server's giveaways, keyed by giveaway, oldest first.
pub fn comments(ctx: &Ctx, guild: u64) -> Result<HashMap<i64, Vec<Comment>>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached(
            "SELECT c.giveaway_id, c.id, c.author, c.text, c.at FROM pets_giveaway_comments c
             JOIN pets_giveaways g ON g.id = c.giveaway_id WHERE g.guild_id = ?1 ORDER BY c.at, c.id",
        )?;
        let mut out: HashMap<i64, Vec<Comment>> = HashMap::new();
        let rows = s.query_map([guild as i64], |r| Ok((r.get::<_, i64>(0)?, Comment { id: r.get(1)?, author: r.get(2)?, text: r.get(3)?, at: r.get(4)? })))?;
        for row in rows {
            let (g, c) = row?;
            out.entry(g).or_default().push(c);
        }
        Ok(out)
    })
}

pub fn add_comment(ctx: &Ctx, giveaway: i64, author: &str, text: &str) -> Result<()> {
    ctx.db.with(|c| {
        c.execute("INSERT INTO pets_giveaway_comments (giveaway_id, author, text, at) VALUES (?1, ?2, ?3, ?4)", params![giveaway, author, text, now_ms()])
    })?;
    Ok(())
}

pub fn delete_comment(ctx: &Ctx, giveaway: i64, id: i64) -> Result<()> {
    ctx.db.with(|c| c.execute("DELETE FROM pets_giveaway_comments WHERE giveaway_id = ?1 AND id = ?2", params![giveaway, id]))?;
    Ok(())
}

// ---- giver bans ----

#[derive(Debug, Clone)]
pub struct Ban {
    pub user_id: u64,
    pub reason: String,
    pub banned_by: String,
    pub banned_at: i64,
}

pub fn bans(ctx: &Ctx, guild: u64) -> Result<Vec<Ban>> {
    ctx.db.with(|c| {
        let mut s = c.prepare_cached("SELECT user_id, reason, banned_by, banned_at FROM pets_bans WHERE guild_id = ?1 ORDER BY banned_at DESC")?;
        s.query_map([guild as i64], |r| Ok(Ban { user_id: r.get::<_, i64>(0)? as u64, reason: r.get(1)?, banned_by: r.get(2)?, banned_at: r.get(3)? }))?
            .collect()
    })
}

pub fn ban(ctx: &Ctx, guild: u64, user: u64) -> Result<Option<Ban>> {
    Ok(bans(ctx, guild)?.into_iter().find(|b| b.user_id == user))
}

/// Returns false when they were already on the list.
pub fn add_ban(ctx: &Ctx, guild: u64, user: u64, reason: &str, by: &str) -> Result<bool> {
    ctx.db.with(|c| {
        let n = c.execute(
            "INSERT OR IGNORE INTO pets_bans (guild_id, user_id, reason, banned_by, banned_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![guild as i64, user as i64, reason, by, now_ms()],
        )?;
        Ok(n > 0)
    })
}

pub fn remove_ban(ctx: &Ctx, guild: u64, user: u64) -> Result<()> {
    ctx.db.with(|c| c.execute("DELETE FROM pets_bans WHERE guild_id = ?1 AND user_id = ?2", params![guild as i64, user as i64]))?;
    Ok(())
}

// ---- stats ----

#[derive(Debug, Default)]
pub struct Stats {
    pub total: i64,
    pub confirmed: i64,
    pub expired: i64,
    pub cancelled: i64,
    pub active: i64,
    /// Confirmed giveaways per giver, most first.
    pub top_givers: Vec<(u64, i64)>,
    /// Confirmed giveaways per pet name, most first: `(name, emoji, count)`.
    pub top_pets: Vec<(String, String, i64)>,
    /// Confirmed giveaways per rarity, rarest first.
    pub rarities: Vec<(String, i64)>,
}

impl Stats {
    /// Share of finished giveaways that were confirmed, 0 to 100.
    pub fn confirmation_rate(&self) -> Option<i64> {
        let finished = self.confirmed + self.expired + self.cancelled;
        (finished > 0).then(|| (self.confirmed * 100 + finished / 2) / finished)
    }
}

pub fn stats(list: &[Giveaway]) -> Stats {
    let mut s = Stats { total: list.len() as i64, ..Default::default() };
    let mut givers: HashMap<u64, i64> = HashMap::new();
    let mut pets: HashMap<String, (String, i64)> = HashMap::new();
    let mut rarities: HashMap<String, i64> = HashMap::new();
    for g in list {
        match g.status {
            Status::Pending => s.active += 1,
            Status::Expired => s.expired += 1,
            Status::Cancelled => s.cancelled += 1,
            Status::Confirmed => {
                s.confirmed += 1;
                *givers.entry(g.giver_id).or_default() += 1;
                pets.entry(g.pet_name.clone()).or_insert_with(|| (g.icon().to_owned(), 0)).1 += 1;
                *rarities.entry(clean_rarity(&g.pet_rarity).to_owned()).or_default() += 1;
            }
        }
    }
    s.top_givers = givers.into_iter().collect();
    s.top_givers.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    s.top_givers.truncate(10);
    s.top_pets = pets.into_iter().map(|(n, (e, c))| (n, e, c)).collect();
    s.top_pets.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    s.top_pets.truncate(10);
    s.rarities = rarities.into_iter().collect();
    s.rarities.sort_by_key(|(r, _)| std::cmp::Reverse(rarity_rank(r)));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_parses() {
        let seed: Seed = serde_json::from_str(SEED).unwrap();
        assert_eq!(seed.categories.len(), 4);
        assert_eq!(seed.catalog.len(), 61);
        for p in &seed.catalog {
            assert!(seed.categories.contains(&p.category), "{} has an unknown category", p.id);
            assert_eq!(clean_rarity(&p.rarity), p.rarity, "{} has an odd rarity", p.id);
            assert!(!p.id.contains(':'), "ids go in custom ids");
        }
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("King Doot"), "king-doot");
        assert_eq!(slug("  Santa's Little Helper!! "), "santa-s-little-helper");
        assert_eq!(slug("???"), "");
    }

    #[test]
    fn rate_counts_finished_only() {
        let s = Stats { confirmed: 3, expired: 1, active: 5, ..Default::default() };
        assert_eq!(s.confirmation_rate(), Some(75));
        assert_eq!(Stats::default().confirmation_rate(), None);
    }
}

#[cfg(test)]
mod flow {
    use super::*;
    use bot_core::Feature as _;

    fn ctx() -> Ctx {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.install_features(vec![std::sync::Arc::new(crate::Pets)]).unwrap();
        ctx
    }

    #[test]
    fn submit_review_give() {
        let rt = bot_core::tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let _in = rt.enter();
        let ctx = ctx();
        let g = 42;
        ensure_seeded(&ctx, g).unwrap();
        ensure_seeded(&ctx, g).unwrap();
        assert_eq!(catalog(&ctx, g).unwrap().len(), 61);
        assert_eq!(categories(&ctx, g).unwrap().len(), 4);

        let a = submit(&ctx, g, "king-doot", 7, 3).unwrap();
        let b = submit(&ctx, g, "slime", 8, 1).unwrap();
        assert_eq!(pending_count(&ctx, g).unwrap(), 2);
        let done = review(&ctx, g, &[a, a], true, "Antho", "").unwrap();
        assert_eq!(done.len(), 1, "a submission is reviewed once");
        review(&ctx, g, &[b], false, "Antho", "dupe").unwrap();
        assert_eq!(owned_counts(&ctx, g).unwrap().get("king-doot"), Some(&3));
        assert_eq!(owned_total(&ctx, g).unwrap(), 3);
        assert_eq!(contributions(&ctx, g, 7).unwrap().len(), 3);
        assert!(pending(&ctx, g).unwrap().is_empty());

        let pet = pet(&ctx, g, "king-doot").unwrap().unwrap();
        let id = record_giveaway(&ctx, g, &NewGiveaway { pet: &pet, giver: 7, winner: 9, notes: "", recorded_by: "Antho", expires_at: Some(now_ms() - 1) }).unwrap();
        assert_eq!(expiring(&ctx, g, 0).unwrap().len(), 1);
        assert!(close_giveaway(&ctx, g, id, Status::Confirmed, "Antho").unwrap());
        assert!(!close_giveaway(&ctx, g, id, Status::Expired, "Expired").unwrap(), "closed giveaways stay closed");
        assert_eq!(remove_owned(&ctx, g, "king-doot", 1, Some(7)).unwrap(), 1);
        let s = stats(&all_giveaways(&ctx, g).unwrap());
        assert_eq!((s.confirmed, s.top_givers[0]), (1, (7, 1)));

        assert!(add_ban(&ctx, g, 7, "", "Antho").unwrap());
        assert!(!add_ban(&ctx, g, 7, "", "Antho").unwrap());
        assert!(ban(&ctx, g, 7).unwrap().is_some());

        assert!(!delete_category(&ctx, g, "Legacy Companions").unwrap());
        assert_eq!(delete_pet(&ctx, g, "king-doot").unwrap(), 2);
        let id = new_pet_id(&ctx, g, "Slime").unwrap();
        assert_eq!(id, "slime-2");
        let _ = crate::Pets.meta();
    }
}
