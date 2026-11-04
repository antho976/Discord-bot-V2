//! Jarl Sigrun's order: one job for the forge a day. Skagi lost its smith; now it's you. She
//! asks for supplies, or for a spare piece of gear you made, and pays in gold and smithing, so
//! there's always a reason to light the forge even when you need nothing yourself.

use bot_core::anyhow::Result;
use bot_core::rusqlite::params;

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

/// Today's order for a player: the same all day, different between players.
pub fn today(p: &Player) -> Order {
    let day = quests::day(p.now);
    let mut r = Rng::new(p.user.rotate_left(7) ^ (day as u64).wrapping_mul(0xA24B_AED4_963E_E407));
    let level = p.level as i64;
    // A piece she can use: one you can forge now, a tier or two under your best.
    let pieces: Vec<&'static Gear> = data::GEAR
        .iter()
        .filter(|g| {
            !g.after_boss
                && g.level <= p.level
                && g.smith <= p.smith_rank()
                && g.level + 8 >= p.level
                && g.level > 1
        })
        .collect();
    if !pieces.is_empty() && r.chance(0.35) {
        let g = *r.pick(&pieces);
        return Order {
            ask: Ask::Piece(g),
            gold: g.gold * 3 / 2 + 10 * level,
            smith: data::smith_xp_forge(g) * 3 / 2,
            threads: 1,
        };
    }
    let supplies: &[(&str, i64)] = if p.level >= 10 {
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
    Order {
        ask: Ask::Supply(id, n),
        gold: 40 + 12 * level,
        smith: 30 + 4 * level,
        threads: 0,
    }
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
    let o = today(&g.p);
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
