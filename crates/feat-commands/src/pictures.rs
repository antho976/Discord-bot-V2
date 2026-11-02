//! Pictures sent with a command's or a trigger's reply. Staff add them on the edit page; the
//! browser shrinks a picture before sending it (a GIF goes as it is, so it keeps moving), and
//! the bot keeps the bytes and attaches them to the reply each time it answers.

use std::collections::HashMap;

use bot_core::anyhow::Result;
use bot_core::rusqlite::{OptionalExtension, params};
use bot_core::twilight_model::http::attachment::Attachment;
use bot_core::{Ctx, now_ms};

use crate::Kind;

/// The most pictures one reply carries.
pub const MAX_PER_REPLY: usize = 4;
/// What one reply's pictures may weigh together: under the 10 MB a message can carry to a
/// server without boosts, with room for the rest of the message.
pub const MAX_REPLY_BYTES: usize = 8 * 1024 * 1024;
/// What a server's pictures may weigh together, across every command and trigger.
pub const MAX_SERVER_BYTES: usize = 50 * 1024 * 1024;

/// Why a picture can't be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Full {
    /// The reply has [`MAX_PER_REPLY`] already.
    Count,
    /// With it, the reply's pictures would weigh more than [`MAX_REPLY_BYTES`].
    Reply,
    /// With it, the server's would weigh more than [`MAX_SERVER_BYTES`].
    Server,
}

impl Full {
    pub fn message(self) -> String {
        match self {
            Full::Count => format!("A reply can carry {MAX_PER_REPLY} pictures. Remove one first."),
            Full::Reply => format!("A reply's pictures can weigh {} MB together, and this one doesn't fit. Use a smaller picture or remove one.", MAX_REPLY_BYTES / (1024 * 1024)),
            Full::Server => format!("The server's command pictures can weigh {} MB together, and that's used up. Remove some first.", MAX_SERVER_BYTES / (1024 * 1024)),
        }
    }
}

/// Whether a picture of `size` bytes fits beside a reply's `sizes` and the server's `server`
/// bytes in all.
pub fn room(sizes: &[usize], server: usize, size: usize) -> Result<(), Full> {
    if sizes.len() >= MAX_PER_REPLY {
        return Err(Full::Count);
    }
    if sizes.iter().sum::<usize>() + size > MAX_REPLY_BYTES {
        return Err(Full::Reply);
    }
    if server + size > MAX_SERVER_BYTES {
        return Err(Full::Server);
    }
    Ok(())
}

/// A stored picture, without its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub id: i64,
    pub kind: Kind,
    pub item: u32,
}

fn kind_of(key: &str) -> Kind {
    if key == Kind::Trigger.key() { Kind::Trigger } else { Kind::Command }
}

/// The ids of one reply's pictures, in the order they were added (and are sent).
pub fn list(ctx: &Ctx, guild: u64, kind: Kind, item: u32) -> Result<Vec<i64>> {
    ctx.db.with(|c| {
        let mut st = c.prepare_cached("SELECT id FROM commands_pictures WHERE guild_id = ?1 AND kind = ?2 AND item_id = ?3 ORDER BY id")?;
        st.query_map(params![guild as i64, kind.key(), item], |r| r.get(0))?.collect()
    })
}

/// Every reply's picture ids in a server, for the lists.
pub fn by_item(ctx: &Ctx, guild: u64) -> Result<HashMap<(Kind, u32), Vec<i64>>> {
    let rows: Vec<(String, u32, i64)> = ctx.db.with(|c| {
        let mut st = c.prepare_cached("SELECT kind, item_id, id FROM commands_pictures WHERE guild_id = ?1 ORDER BY id")?;
        st.query_map([guild as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect()
    })?;
    let mut map: HashMap<(Kind, u32), Vec<i64>> = HashMap::new();
    for (kind, item, id) in rows {
        map.entry((kind_of(&kind), item)).or_default().push(id);
    }
    Ok(map)
}

/// Add a picture to a reply, when it fits ([`room`]). The check and the write happen under one
/// lock, so two uploads at once can't both squeeze past a limit.
pub fn add(ctx: &Ctx, guild: u64, kind: Kind, item: u32, mime: &str, data: &[u8]) -> Result<Result<i64, Full>> {
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    let sizes: Vec<usize> = {
        let mut st = tx.prepare_cached("SELECT length(data) FROM commands_pictures WHERE guild_id = ?1 AND kind = ?2 AND item_id = ?3")?;
        st.query_map(params![guild as i64, kind.key(), item], |r| r.get::<_, i64>(0))?.map(|n| n.map(|n| n as usize)).collect::<Result<_, _>>()?
    };
    let server: i64 = tx.query_row("SELECT COALESCE(SUM(length(data)), 0) FROM commands_pictures WHERE guild_id = ?1", [guild as i64], |r| r.get(0))?;
    if let Err(full) = room(&sizes, server as usize, data.len()) {
        return Ok(Err(full));
    }
    tx.execute(
        "INSERT INTO commands_pictures (guild_id, kind, item_id, mime, data, added_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![guild as i64, kind.key(), item, mime, data, now_ms()],
    )?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    Ok(Ok(id))
}

/// Which reply a picture belongs to, when it's this server's.
pub fn find(ctx: &Ctx, guild: u64, id: i64) -> Result<Option<Picture>> {
    ctx.db.with(|c| {
        c.query_row("SELECT kind, item_id FROM commands_pictures WHERE guild_id = ?1 AND id = ?2", params![guild as i64, id], |r| {
            Ok(Picture { id, kind: kind_of(&r.get::<_, String>(0)?), item: r.get(1)? })
        })
        .optional()
    })
}

/// Remove a picture. Whether there was one in this server.
pub fn remove(ctx: &Ctx, guild: u64, id: i64) -> Result<bool> {
    ctx.db.with(|c| Ok(c.execute("DELETE FROM commands_pictures WHERE guild_id = ?1 AND id = ?2", params![guild as i64, id])? > 0))
}

/// Remove a deleted command's or trigger's pictures.
pub fn forget(ctx: &Ctx, guild: u64, kind: Kind, item: u32) -> Result<()> {
    ctx.db.with(|c| c.execute("DELETE FROM commands_pictures WHERE guild_id = ?1 AND kind = ?2 AND item_id = ?3", params![guild as i64, kind.key(), item]))?;
    Ok(())
}

/// A picture's type and bytes.
pub fn get(ctx: &Ctx, guild: u64, id: i64) -> Result<Option<(String, Vec<u8>)>> {
    ctx.db.with(|c| {
        c.query_row("SELECT mime, data FROM commands_pictures WHERE guild_id = ?1 AND id = ?2", params![guild as i64, id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
    })
}

/// A file name Discord shows the picture under, with the extension its type needs.
fn file_name(n: usize, mime: &str) -> String {
    let ext = match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        _ => "webp",
    };
    format!("picture-{n}.{ext}")
}

/// One reply's pictures as files for Discord, in order. Empty when it has none.
pub fn files(ctx: &Ctx, guild: u64, kind: Kind, item: u32) -> Result<Vec<Attachment>> {
    let rows: Vec<(String, Vec<u8>)> = ctx.db.with(|c| {
        let mut st = c.prepare_cached("SELECT mime, data FROM commands_pictures WHERE guild_id = ?1 AND kind = ?2 AND item_id = ?3 ORDER BY id")?;
        st.query_map(params![guild as i64, kind.key(), item], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
    })?;
    Ok(rows.into_iter().enumerate().map(|(i, (mime, data))| Attachment::from_bytes(file_name(i + 1, &mime), data, i as u64 + 1)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: usize = 1024 * 1024;

    #[test]
    fn a_reply_takes_four_pictures_within_its_weight() {
        assert_eq!(room(&[], 0, MB), Ok(()));
        assert_eq!(room(&[MB; 3], 3 * MB, MB), Ok(()));
        assert_eq!(room(&[MB; 4], 4 * MB, 1), Err(Full::Count));
        // Two of 3 MB leave room for 2 MB more, not 3.
        assert_eq!(room(&[3 * MB, 3 * MB], 6 * MB, 2 * MB), Ok(()));
        assert_eq!(room(&[3 * MB, 3 * MB], 6 * MB, 3 * MB), Err(Full::Reply));
        assert_eq!(room(&[], 0, MAX_REPLY_BYTES + 1), Err(Full::Reply));
        // The server's own limit, even for a reply with none yet.
        assert_eq!(room(&[], MAX_SERVER_BYTES - MB, MB), Ok(()));
        assert_eq!(room(&[], MAX_SERVER_BYTES - MB, MB + 1), Err(Full::Server));
        for full in [Full::Count, Full::Reply, Full::Server] {
            assert!(!full.message().is_empty());
        }
    }

    #[test]
    fn files_are_named_for_their_type() {
        assert_eq!(file_name(1, "image/png"), "picture-1.png");
        assert_eq!(file_name(2, "image/jpeg"), "picture-2.jpg");
        assert_eq!(file_name(3, "image/gif"), "picture-3.gif");
        assert_eq!(file_name(4, "image/webp"), "picture-4.webp");
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn pictures_stay_with_their_reply_and_server() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate(crate::KEY, crate::MIGRATIONS).unwrap();
        let a = add(&ctx, 1, Kind::Command, 1, "image/webp", b"a").unwrap().unwrap();
        let b = add(&ctx, 1, Kind::Command, 1, "image/png", b"b").unwrap().unwrap();
        // A trigger with the same id has its own.
        let t = add(&ctx, 1, Kind::Trigger, 1, "image/gif", b"t").unwrap().unwrap();
        assert_eq!(list(&ctx, 1, Kind::Command, 1).unwrap(), vec![a, b]);
        assert_eq!(list(&ctx, 1, Kind::Trigger, 1).unwrap(), vec![t]);
        let map = by_item(&ctx, 1).unwrap();
        assert_eq!(map.get(&(Kind::Command, 1)), Some(&vec![a, b]));
        assert_eq!(map.get(&(Kind::Trigger, 1)), Some(&vec![t]));
        assert_eq!(find(&ctx, 1, t).unwrap(), Some(Picture { id: t, kind: Kind::Trigger, item: 1 }));
        // Sent in the order they were added, named for their type.
        let sent = files(&ctx, 1, Kind::Command, 1).unwrap();
        assert_eq!(sent.iter().map(|f| f.filename.as_str()).collect::<Vec<_>>(), ["picture-1.webp", "picture-2.png"]);
        assert_eq!(sent[1].file, b"b");
        assert!(files(&ctx, 1, Kind::Command, 2).unwrap().is_empty());
        // Another server can't see or remove them.
        assert!(get(&ctx, 2, a).unwrap().is_none());
        assert!(find(&ctx, 2, a).unwrap().is_none());
        assert!(!remove(&ctx, 2, a).unwrap());
        assert!(remove(&ctx, 1, a).unwrap());
        assert_eq!(get(&ctx, 1, b).unwrap(), Some(("image/png".to_owned(), b"b".to_vec())));
        // The fifth is refused.
        for _ in 0..3 {
            add(&ctx, 1, Kind::Command, 1, "image/webp", b"x").unwrap().unwrap();
        }
        assert_eq!(add(&ctx, 1, Kind::Command, 1, "image/webp", b"x").unwrap(), Err(Full::Count));
        // Deleting the command takes its pictures, and only its own.
        forget(&ctx, 1, Kind::Command, 1).unwrap();
        assert!(list(&ctx, 1, Kind::Command, 1).unwrap().is_empty());
        assert_eq!(list(&ctx, 1, Kind::Trigger, 1).unwrap(), vec![t]);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn heavy_pictures_hit_the_reply_limit() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate(crate::KEY, crate::MIGRATIONS).unwrap();
        let five = vec![0u8; 5 * MB];
        add(&ctx, 1, Kind::Command, 1, "image/gif", &five).unwrap().unwrap();
        assert_eq!(add(&ctx, 1, Kind::Command, 1, "image/gif", &five).unwrap(), Err(Full::Reply));
        // Another command still has room.
        assert!(add(&ctx, 1, Kind::Command, 2, "image/gif", &five).unwrap().is_ok());
    }
}
