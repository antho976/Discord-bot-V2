//! Pictures staff upload for the welcome and goodbye cards. One is drawn behind the card each
//! time, at random or in turn. The browser shrinks a picture before sending it, so a row is a
//! few hundred kilobytes at most.

use bot_core::anyhow::{Result, bail};
use bot_core::rusqlite::{OptionalExtension, params};
use bot_core::{Ctx, now_ms};

/// The most pictures a card can have.
pub const MAX: usize = 10;

/// Which card a picture is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Welcome,
    Goodbye,
}

impl Kind {
    pub fn key(self) -> &'static str {
        match self {
            Kind::Welcome => "welcome",
            Kind::Goodbye => "goodbye",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "welcome" => Some(Kind::Welcome),
            "goodbye" => Some(Kind::Goodbye),
            _ => None,
        }
    }
}

/// The ids of a card's pictures, oldest first.
pub fn list(ctx: &Ctx, guild: u64, kind: Kind) -> Result<Vec<i64>> {
    ctx.db.with(|c| {
        let mut st = c.prepare_cached("SELECT id FROM welcome_banners WHERE guild_id = ?1 AND kind = ?2 ORDER BY id")?;
        let ids = st.query_map(params![guild as i64, kind.key()], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
        Ok(ids)
    })
}

/// Add a picture. Refused once the card has [`MAX`].
pub fn add(ctx: &Ctx, guild: u64, kind: Kind, mime: &str, data: &[u8]) -> Result<i64> {
    if list(ctx, guild, kind)?.len() >= MAX {
        bail!("A card can have {MAX} pictures. Remove one first.");
    }
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO welcome_banners (guild_id, kind, mime, data, added_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![guild as i64, kind.key(), mime, data, now_ms()],
        )?;
        Ok(c.last_insert_rowid())
    })
}

/// Remove a picture. Whether there was one.
pub fn remove(ctx: &Ctx, guild: u64, id: i64) -> Result<bool> {
    ctx.db.with(|c| Ok(c.execute("DELETE FROM welcome_banners WHERE guild_id = ?1 AND id = ?2", params![guild as i64, id])? > 0))
}

/// A picture's type and bytes.
pub fn get(ctx: &Ctx, guild: u64, id: i64) -> Result<Option<(String, Vec<u8>)>> {
    ctx.db.with(|c| {
        c.query_row("SELECT mime, data FROM welcome_banners WHERE guild_id = ?1 AND id = ?2", params![guild as i64, id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
    })
}

/// The picture after `last` in `ids`, going back to the first after the end. A `last` that was
/// removed since is passed over to the next one still there.
pub fn next_in_order(ids: &[i64], last: Option<i64>) -> Option<i64> {
    let last = last.unwrap_or(i64::MIN);
    ids.iter().copied().find(|id| *id > last).or_else(|| ids.first().copied())
}

/// A picture at random, never `last` again when there's another: `roll` is any random number.
pub fn at_random(ids: &[i64], last: Option<i64>, roll: usize) -> Option<i64> {
    let others: Vec<i64> = ids.iter().copied().filter(|id| Some(*id) != last).collect();
    let from = if others.is_empty() { ids } else { &others };
    (!from.is_empty()).then(|| from[roll % from.len()])
}

/// Choose the picture for the next card and note it, so the next one differs. None when the
/// card has no pictures.
pub fn pick(ctx: &Ctx, guild: u64, kind: Kind, in_order: bool) -> Result<Option<Vec<u8>>> {
    let ids = list(ctx, guild, kind)?;
    let last: Option<i64> = ctx.db.with(|c| {
        c.query_row("SELECT last_id FROM welcome_banner_turn WHERE guild_id = ?1 AND kind = ?2", params![guild as i64, kind.key()], |r| r.get(0))
            .optional()
    })?;
    let chosen = if in_order { next_in_order(&ids, last) } else { at_random(&ids, last, rand::random::<u32>() as usize) };
    let Some(id) = chosen else { return Ok(None) };
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO welcome_banner_turn (guild_id, kind, last_id) VALUES (?1, ?2, ?3)
             ON CONFLICT (guild_id, kind) DO UPDATE SET last_id = excluded.last_id",
            params![guild as i64, kind.key(), id],
        )?;
        Ok(())
    })?;
    Ok(get(ctx, guild, id)?.map(|(_, data)| data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_order_goes_round_and_skips_removed_ones() {
        let ids = [3, 5, 9];
        assert_eq!(next_in_order(&ids, None), Some(3));
        assert_eq!(next_in_order(&ids, Some(3)), Some(5));
        assert_eq!(next_in_order(&ids, Some(9)), Some(3));
        // 6 was removed: the next one still there.
        assert_eq!(next_in_order(&ids, Some(6)), Some(9));
        assert_eq!(next_in_order(&[], Some(6)), None);
    }

    #[test]
    fn at_random_never_repeats_when_there_is_another() {
        for roll in 0..20 {
            assert_ne!(at_random(&[3, 5], Some(3), roll), Some(3));
        }
        assert_eq!(at_random(&[3], Some(3), 7), Some(3));
        assert_eq!(at_random(&[], None, 7), None);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn pictures_are_kept_per_card_and_capped() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate("welcome", crate::MIGRATIONS).unwrap();
        let a = add(&ctx, 1, Kind::Welcome, "image/webp", b"a").unwrap();
        let b = add(&ctx, 1, Kind::Welcome, "image/webp", b"b").unwrap();
        add(&ctx, 1, Kind::Goodbye, "image/webp", b"g").unwrap();
        assert_eq!(list(&ctx, 1, Kind::Welcome).unwrap(), vec![a, b]);
        // In turn: a, b, a.
        let got: Vec<Vec<u8>> = (0..3).map(|_| pick(&ctx, 1, Kind::Welcome, true).unwrap().unwrap()).collect();
        assert_eq!(got, vec![b"a".to_vec(), b"b".to_vec(), b"a".to_vec()]);
        // At random never shows the same one twice running.
        let mut last = pick(&ctx, 1, Kind::Welcome, false).unwrap();
        for _ in 0..10 {
            let next = pick(&ctx, 1, Kind::Welcome, false).unwrap();
            assert_ne!(next, last);
            last = next;
        }
        assert_eq!(pick(&ctx, 1, Kind::Goodbye, false).unwrap(), Some(b"g".to_vec()));
        assert_eq!(pick(&ctx, 2, Kind::Goodbye, false).unwrap(), None);
        // Another server can't remove this one's.
        assert!(!remove(&ctx, 2, a).unwrap());
        assert!(remove(&ctx, 1, a).unwrap());
        for _ in 1..MAX {
            add(&ctx, 1, Kind::Welcome, "image/webp", b"x").unwrap();
        }
        assert!(add(&ctx, 1, Kind::Welcome, "image/webp", b"x").is_err());
    }
}
