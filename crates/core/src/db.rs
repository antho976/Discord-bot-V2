//! One SQLite connection behind a mutex. Every query here is small, so callers lock, run and
//! release; nothing holds the lock across an await.

use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};

/// A feature's schema change. Applied once, in order, keyed by `(feature, name)`.
pub struct Migration {
    pub name: &'static str,
    pub sql: &'static str,
}

#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

impl Db {
    pub fn open(path: &str) -> Result<Self> {
        if path == ":memory:" {
            return Self::memory();
        }
        if let Some(dir) = std::path::Path::new(path).parent()
            && !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            }
        let conn = Connection::open(path).with_context(|| format!("opening {path}"))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA busy_timeout=5000;
             PRAGMA foreign_keys=ON;
             PRAGMA cache_size=-2000;
             PRAGMA temp_store=MEMORY;",
        )?;
        conn.set_prepared_statement_cache_capacity(64);
        let db = Self(Arc::new(Mutex::new(conn)));
        db.migrate("core", CORE_MIGRATIONS)?;
        Ok(db)
    }

    /// In-memory database for tests.
    pub fn memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self(Arc::new(Mutex::new(conn)));
        db.migrate("core", CORE_MIGRATIONS)?;
        Ok(db)
    }

    pub fn lock(&self) -> MutexGuard<'_, Connection> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Run `f` with the connection. Keep it short: no awaits, no network.
    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
        Ok(f(&self.lock())?)
    }

    pub fn migrate(&self, feature: &str, migrations: &[Migration]) -> Result<()> {
        let mut conn = self.lock();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                feature TEXT NOT NULL, name TEXT NOT NULL, applied_at INTEGER NOT NULL,
                PRIMARY KEY (feature, name))",
        )?;
        for m in migrations {
            let done: bool = conn
                .query_row(
                    "SELECT 1 FROM schema_migrations WHERE feature = ?1 AND name = ?2",
                    params![feature, m.name],
                    |_| Ok(true),
                )
                .optional()?
                .unwrap_or(false);
            if done {
                continue;
            }
            let tx = conn.transaction()?;
            tx.execute_batch(m.sql).with_context(|| format!("migration {feature}/{}", m.name))?;
            tx.execute(
                "INSERT INTO schema_migrations (feature, name, applied_at) VALUES (?1, ?2, ?3)",
                params![feature, m.name, crate::now_ms()],
            )?;
            tx.commit()?;
            tracing::info!(feature, name = m.name, "applied migration");
        }
        Ok(())
    }

    /// A feature's settings for one server, or the defaults when none are saved yet.
    pub fn settings<T: DeserializeOwned + Default>(&self, guild_id: u64, feature: &str) -> Result<T> {
        let raw: Option<String> = self.with(|c| {
            c.query_row(
                "SELECT json FROM guild_settings WHERE guild_id = ?1 AND feature = ?2",
                params![guild_id as i64, feature],
                |r| r.get(0),
            )
            .optional()
        })?;
        Ok(match raw {
            Some(json) => serde_json::from_str(&json).unwrap_or_else(|e| {
                tracing::warn!(feature, guild_id, error = %e, "settings didn't parse, using defaults");
                T::default()
            }),
            None => T::default(),
        })
    }

    pub fn save_settings<T: Serialize>(&self, guild_id: u64, feature: &str, value: &T) -> Result<()> {
        let json = serde_json::to_string(value)?;
        self.with(|c| {
            c.execute(
                "INSERT INTO guild_settings (guild_id, feature, json, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (guild_id, feature) DO UPDATE SET json = excluded.json, updated_at = excluded.updated_at",
                params![guild_id as i64, feature, json, crate::now_ms()],
            )
        })?;
        Ok(())
    }
}

const CORE_MIGRATIONS: &[Migration] = &[Migration {
    name: "001_core",
    sql: "
        CREATE TABLE guild_settings (
            guild_id INTEGER NOT NULL, feature TEXT NOT NULL, json TEXT NOT NULL,
            updated_at INTEGER NOT NULL, PRIMARY KEY (guild_id, feature));
        CREATE TABLE guild_features (
            guild_id INTEGER NOT NULL, feature TEXT NOT NULL, enabled INTEGER NOT NULL,
            PRIMARY KEY (guild_id, feature));
        CREATE TABLE dash_users (
            id INTEGER PRIMARY KEY, name TEXT NOT NULL DEFAULT '', avatar TEXT,
            admin INTEGER NOT NULL DEFAULT 0, added_by INTEGER, created_at INTEGER NOT NULL,
            notif_seen INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE dash_perms (
            user_id INTEGER NOT NULL REFERENCES dash_users(id) ON DELETE CASCADE,
            guild_id INTEGER NOT NULL, page TEXT NOT NULL, level INTEGER NOT NULL,
            PRIMARY KEY (user_id, guild_id, page));
        CREATE TABLE dash_sessions (
            token_hash TEXT PRIMARY KEY, user_id INTEGER NOT NULL, created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL);
        CREATE TABLE dash_audit (
            id INTEGER PRIMARY KEY, at INTEGER NOT NULL, user_id INTEGER NOT NULL, user_name TEXT NOT NULL,
            guild_id INTEGER, feature TEXT NOT NULL, action TEXT NOT NULL);
        CREATE INDEX dash_audit_at ON dash_audit(at);
        CREATE TABLE notifications (
            id INTEGER PRIMARY KEY, at INTEGER NOT NULL, guild_id INTEGER, feature TEXT NOT NULL,
            text TEXT NOT NULL, link TEXT);
        CREATE INDEX notifications_at ON notifications(at);
        CREATE TABLE daily_stats (
            guild_id INTEGER NOT NULL, day TEXT NOT NULL, key TEXT NOT NULL, value INTEGER NOT NULL,
            PRIMARY KEY (guild_id, key, day));
    ",
}];
