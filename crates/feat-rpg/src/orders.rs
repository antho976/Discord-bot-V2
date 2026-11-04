//! Jarl Sigrun's order: one job for the forge a day. Skagi lost its smith; now it's you. She
//! asks for supplies, or for a spare piece of gear you made, and pays in gold and smithing, so
//! there's always a reason to light the forge even when you need nothing yourself.

use bot_core::anyhow::Result;
use bot_core::rusqlite::{OptionalExtension, params};

use crate::data::{self, Gear};
use crate::emoji::get as e;
use crate::engine::Rng;
use crate::quests;
use crate::store::{Game, Player};

#[derive(Debug, Clone, Copy)]
pub enum Ask {
    /// This many of a supply made at the forge.
    Supply(&'static str, i64),
    /// A spare piece of this gear (any rarity, not worn).
    Piece(&'static Gear),
}

#[derive(Debug, Clone, Copy)]
pub struct Order {
    pub ask: Ask,
    pub gold: i64,
    pub smith: i64,
    pub threads: i64,
}

impl Order {
    pub fn describe(&self) -> String {
        match self.ask {
            Ask::Supply(id, n) => data::count(id, n),
            Ask::Piece(g) => format!("a spare {}", g.name),
        }
    }
    pub fn emoji(&self) -> &'static str {
        match self.ask {
            Ask::Supply(id, _) => data::item(id).map(|i| i.emoji).unwrap_or("forge"),
            Ask::Piece(_) => "forge",
        }
    }
    /// How much of it the player has ready to hand over, and how much it takes.
    pub fn have(&self, p: &Player) -> (i64, i64) {
        match self.ask {
            Ask::Supply(id, n) => (p.have(id), n),
            Ask::Piece(g) => (spare(p, g).is_some() as i64, 1),
        }
    }
    pub fn pays(&self) -> String {
        let mut parts = vec![
            format!("{} {} gold", e("gold"), self.gold),
            format!("{} smithing", self.smith),
        ];
        if self.threads > 0 {
            parts.push(format!(
                "{} {}",
                e("thread"),
                data::count("thread", self.threads)
            ));
        }
        parts.join(" · ")
    }
}

/// A spare (not worn) piece of `g`, the lowest rarity first: she doesn't need your best.
fn spare<'a>(p: &'a Player, g: &Gear) -> Option<&'a crate::store::GearRow> {
    p.gear
        .iter()
        .filter(|x| !x.equipped && x.gear.id == g.id)
        .min_by_key(|x| (x.rarity, x.upgrade))
}

/// Today's order for a player: picked the first time it's asked for and kept for the day, so
/// ranking up or levelling mid-day doesn't change what Sigrun wants.
pub fn today(g: &Game) -> Result<Order> {
    let day = quests::day(g.p.now);
    let kept: Option<String> = g
        .tx
        .query_row(
            "SELECT slot FROM rpg_bought WHERE guild_id = ?1 AND user_id = ?2 AND day = ?3 AND slot LIKE 'ask:%'",
            params![g.p.guild as i64, g.p.user as i64, day],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(o) = kept.as_deref().and_then(|k| decode(k, g.p.level)) {
        return Ok(o);
    }
    let o = pick(&g.p);
    g.tx.execute(
        "INSERT OR IGNORE INTO rpg_bought (guild_id, user_id, day, slot) VALUES (?1, ?2, ?3, ?4)",
        params![g.p.guild as i64, g.p.user as i64, day, encode(&o)],
    )?;
    Ok(o)
}

fn encode(o: &Order) -> String {
    match o.ask {
        Ask::Supply(id, n) => format!("ask:s:{id}:{n}"),
        Ask::Piece(g) => format!("ask:p:{}", g.id),
    }
}

fn decode(k: &str, level: u32) -> Option<Order> {
    let mut parts = k.split(':').skip(1);
    match (parts.next()?, parts.next()?) {
        ("p", id) => data::gear(id).map(|g| piece_order(g, level)),
        ("s", id) => {
            let n: i64 = parts.next()?.parse().ok()?;
            let id = data::item(id)?.id;
            Some(supply_order(id, n, level))
        }
        _ => None,
    }
}

fn piece_order(g: &'static Gear, level: u32) -> Order {
    Order {
        ask: Ask::Piece(g),
        gold: g.gold * 3 / 2 + 10 * level as i64,
        smith: data::smith_xp_forge(g) * 3 / 2,
        threads: 1,
    }
}

fn supply_order(id: &'static str, n: i64, level: u32) -> Order {
    Order {
        ask: Ask::Supply(id, n),
        gold: 40 + 12 * level as i64,
        smith: 30 + 4 * level as i64,
        threads: 0,
    }
}

/// A fresh pick from the day and the player.
fn pick(p: &Player) -> Order {
    let day = quests::day(p.now);
    let mut r = Rng::new(p.user.rotate_left(7) ^ (day as u64).wrapping_mul(0xA24B_AED4_963E_E407));
    // A piece she can use: one you can forge now, under what you wear in that slot (so making
    // it doesn't put it on you), and not too far behind.
    let pieces: Vec<&'static Gear> = data::GEAR
        .iter()
        .filter(|g| {
            let worn = p.equipped(g.slot).map(|x| x.gear.level).unwrap_or(0);
            !g.after_boss
                && g.level < worn
                && g.smith <= p.smith_rank()
                && g.level + 8 >= p.level
                && g.level > 1
        })
        .collect();
    if !pieces.is_empty() && r.chance(0.35) {
        let g = pieces[(r.next_u64() % pieces.len() as u64) as usize];
        return piece_order(g, p.level);
    }
    let supplies: &[(&'static str, i64)] = if p.level >= 10 {
        &[
            ("potion", 4),
            ("rations", 4),
            ("fireoil", 2),
            ("frostoil", 2),
            ("bait", 2),
        ]
    } else {
        &[("potion", 4), ("rations", 4), ("bait", 2)]
    };
    let (id, n) = *r.pick(supplies);
    supply_order(id, n, p.level)
}

pub fn done(g: &Game) -> Result<bool> {
    Ok(g.tx.query_row(
        "SELECT COUNT(*) FROM rpg_bought WHERE guild_id = ?1 AND user_id = ?2 AND day = ?3 AND slot = 'order'",
        params![g.p.guild as i64, g.p.user as i64, quests::day(g.p.now)],
        |r| r.get::<_, i64>(0),
    )? > 0)
}

/// Hand the order over. Returns what to say.
pub fn fill(g: &mut Game) -> Result<Option<String>> {
    if done(g)? {
        return Ok(Some(
            "Sigrun has what she needs today. Come back tomorrow.".into(),
        ));
    }
    let o = today(g)?;
    let (have, need) = o.have(&g.p);
    if have < need {
        return Ok(Some(format!("She asked for {}.", o.describe())));
    }
    match o.ask {
        Ask::Supply(id, n) => g.item(id, -n, "Sigrun's order")?,
        Ask::Piece(gear) => {
            let id = spare(&g.p, gear).map(|x| x.id).unwrap_or(-1);
            g.remove_gear(id)?;
        }
    }
    g.tx.execute(
        "INSERT INTO rpg_bought (guild_id, user_id, day, slot) VALUES (?1, ?2, ?3, 'order')",
        params![g.p.guild as i64, g.p.user as i64, quests::day(g.p.now)],
    )?;
    g.gold(o.gold, "Sigrun's order")?;
    if o.threads > 0 {
        g.item("thread", o.threads, "Sigrun's order")?;
    }
    let up = g.smith(o.smith)?;
    g.bump()?;
    Ok(Some(format!(
        "Sigrun weighs it in her hands and nods. {}{}",
        o.pays(),
        crate::game::rank_line(up)
    )))
}
