//! Bring connected PCs over from antho-s-bot, so a PC that ran `relay cloud connect` against the
//! old bot keeps its token here. Tokens are stored the same way in both (sha256, hex), so the
//! hashes copy as they are.

use std::path::Path;

use bot_core::anyhow::{Context, Result, bail};
use bot_core::rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use bot_core::Ctx;

/// `(id, request_id, name, token_hash, created_at, last_seen_at)` as antho-s-bot stored them.
type OldHost = (String, String, String, String, Option<i64>, Option<i64>);

/// How many PCs came over from a SQLite file's bytes.
pub fn import_bytes(ctx: &Ctx, bytes: &[u8]) -> Result<usize> {
    if !bytes.starts_with(b"SQLite format 3\0") {
        bail!("That isn't a SQLite database. Upload antho-s-bot's bot.db or a backup of it.");
    }
    let dir = std::env::temp_dir().join(format!("bot-connect-import-{}", crate::domain::random_hex(6)));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("import.db");
    std::fs::write(&path, bytes)?;
    let res = import_db(ctx, &path);
    let _ = std::fs::remove_dir_all(&dir);
    res
}

pub fn import_db(ctx: &Ctx, path: &Path) -> Result<usize> {
    let src = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).with_context(|| format!("opening {}", path.display()))?;
    let has = src
        .query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'connect_hosts'", [], |_| Ok(()))
        .optional()?
        .is_some();
    if !has {
        bail!("This database has no connected PCs. Is it antho-s-bot's?");
    }
    let rows: Vec<OldHost> = {
        let mut s = src.prepare("SELECT id, request_id, name, token_hash, created_at, last_seen_at FROM connect_hosts WHERE revoked_at IS NULL")?;
        let it = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?;
        it.collect::<Result<_, _>>()?
    };
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    let mut n = 0;
    for (id, request_id, name, hash, created, seen) in rows {
        if !crate::domain::is_hex(&hash, 64) {
            continue;
        }
        n += tx.execute(
            "INSERT OR IGNORE INTO connect_hosts (id, request_id, name, token_hash, created_at, last_seen_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, request_id, name, hash, created.unwrap_or_else(bot_core::now_ms), seen],
        )?;
    }
    tx.commit()?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn pcs_come_over_with_their_token_hashes() {
        let ctx = crate::testing::ctx();
        let dir = std::env::temp_dir().join(format!("connect-import-test-{}", crate::domain::random_hex(4)));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.db");
        {
            let old = Connection::open(&path).unwrap();
            old.execute_batch(
                "CREATE TABLE connect_hosts (id text PRIMARY KEY NOT NULL, request_id text NOT NULL, name text NOT NULL,
                   token_hash text NOT NULL, created_at integer, last_seen_at integer, revoked_at integer);",
            )
            .unwrap();
            let (_, hash) = crate::domain::new_host_token();
            old.execute("INSERT INTO connect_hosts VALUES ('h1', 'r1', 'desk', ?1, 1, 2, NULL)", [&hash]).unwrap();
            old.execute("INSERT INTO connect_hosts VALUES ('h2', 'r2', 'old', ?1, 1, 2, 3)", [&"b".repeat(64)]).unwrap();
        }
        assert_eq!(import_bytes(&ctx, &std::fs::read(&path).unwrap()).unwrap(), 1);
        // Importing twice adds nothing.
        assert_eq!(import_db(&ctx, &path).unwrap(), 0);
        assert_eq!(crate::store::count_hosts(&ctx).unwrap(), 1);
        assert!(import_bytes(&ctx, b"not a db").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
