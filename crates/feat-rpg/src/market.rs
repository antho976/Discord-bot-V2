//! Skagi's market: a stall that changes every day. Three lots of materials picked for what
//! you'll forge next, and supplies at fixed prices. It sells no finished gear: that's the
//! forge's work, and the market feeds it.

use bot_core::anyhow::Result;
use bot_core::rusqlite::params;

use crate::data::{self, Gear};
use crate::emoji::get as e;
use crate::engine::Rng;
use crate::engine::loot;
use crate::game::{Ids, back_to, thousands, title};
use crate::quests;
use crate::store::Game;
use crate::ui::{Btn, Opt, Screen};

/// (item, gold each, a day's limit)
pub const SUPPLIES: [(&str, i64, i64); 5] = [
    ("potion", 35, 6),
    ("rations", 20, 6),
    ("fireoil", 60, 3),
    ("frostoil", 60, 3),
    ("thread", 180, 2),
];

/// One lot on the stall: (material, how many, the piece it's for, gold).
pub type Lot = (&'static str, i64, &'static Gear, i64);

/// Gold per unit of a material: three times what it's worth at an altar.
pub fn unit_price(item: &str) -> i64 {
    data::item(item).map(|i| i.tribute * 3).unwrap_or(10)
}

/// Today's three lots for a player: materials for pieces they could forge soon, never the
/// same material twice, never the Barrow King's shards.
pub fn stock(user: u64, day: i64, level: u32, boss: bool) -> Vec<Lot> {
    let mut r = Rng::new(user.rotate_left(13) ^ (day as u64).wrapping_mul(0x9E37_79B9));
    let mut out: Vec<Lot> = Vec::new();
    for _ in 0..24 {
        let g = loot::drop_piece(level, boss, &mut r);
        let mats: Vec<(&str, i64)> = g
            .cost
            .iter()
            .copied()
            .filter(|(id, _)| *id != "shard")
            .collect();
        if mats.is_empty() {
            continue;
        }
        let (id, n) = *r.pick(&mats);
        if out.iter().any(|x| x.0 == id) {
            continue;
        }
        // Rare materials come a few at a time; common ones by the half-recipe.
        let qty = if data::item(id).is_some_and(|i| i.tribute >= 10) {
            n.clamp(1, 3)
        } else {
            (n / 2).max(3)
        };
        out.push((id, qty, g, unit_price(id) * qty));
        if out.len() == 3 {
            break;
        }
    }
    out
}

fn bought(g: &Game, slot: &str) -> Result<i64> {
    Ok(g.tx.query_row(
        "SELECT COUNT(*) FROM rpg_bought WHERE guild_id = ?1 AND user_id = ?2 AND day = ?3 AND (slot = ?4 OR slot LIKE ?4 || ':%')",
        params![g.p.guild as i64, g.p.user as i64, quests::day(g.p.now), slot],
        |r| r.get(0),
    )?)
}

fn mark(g: &Game, slot: &str) -> Result<()> {
    let n = bought(g, slot)?;
    g.tx.execute(
        "INSERT INTO rpg_bought (guild_id, user_id, day, slot) VALUES (?1, ?2, ?3, ?4)",
        params![
            g.p.guild as i64,
            g.p.user as i64,
            quests::day(g.p.now),
            format!("{slot}:{n}")
        ],
    )?;
    Ok(())
}

pub fn screen(g: &Game) -> Result<Screen> {
    let p = &g.p;
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    title(
        &mut s,
        "market",
        "Skagi market",
        "New stock every day at midnight UTC",
    );
    s.text(format!(
        "{} You have {} gold.",
        e("gold"),
        thousands(p.gold)
    ));
    for (i, (item, qty, gear, price)) in stock(p.user, quests::day(p.now), p.level, p.boss_won)
        .into_iter()
        .enumerate()
    {
        let sold = bought(g, &format!("g{i}"))? > 0;
        s.section(
            format!(
                "{} **{}** · {} gold{}\n-# For a {} · you have {}",
                e(item),
                data::count(item, qty),
                thousands(price),
                if sold { " · sold" } else { "" },
                gear.name,
                p.have(item)
            ),
            Btn::new(ids.act("market", "gear", &i.to_string()), "Buy")
                .emoji("gold")
                .disabled(sold || p.gold < price),
        );
    }
    let mut opts = Vec::new();
    for (id, price, limit) in SUPPLIES {
        let left = limit - bought(g, id)?;
        if let Some(it) = data::item(id) {
            opts.push(
                Opt::new(format!("{} · {price} gold", it.name), id)
                    .emoji(it.emoji)
                    .desc(format!(
                        "{} · {left} left today · you have {}",
                        it.used_for,
                        p.have(id)
                    )),
            );
        }
    }
    s.inner_select(ids.act("market", "supply", ""), "Buy supplies…", opts);
    s.buttons(vec![back_to(&ids, "skagi", "Skagi")]);
    Ok(s)
}

pub fn buy_gear(g: &mut Game, i: usize) -> Result<Option<String>> {
    let Some((item, qty, _, price)) =
        stock(g.p.user, quests::day(g.p.now), g.p.level, g.p.boss_won)
            .into_iter()
            .nth(i)
    else {
        return Ok(None);
    };
    if bought(g, &format!("g{i}"))? > 0 {
        return Ok(Some("Already sold.".into()));
    }
    if g.p.gold < price {
        return Ok(Some(format!("That's {price} gold.")));
    }
    g.gold(-price, "market")?;
    g.item(item, qty, "market")?;
    mark(g, &format!("g{i}"))?;
    g.bump()?;
    Ok(Some(format!("Bought {}.", data::count(item, qty))))
}

pub fn buy_supply(g: &mut Game, id: &str) -> Result<Option<String>> {
    let Some((_, price, limit)) = SUPPLIES.iter().find(|x| x.0 == id).copied() else {
        return Ok(None);
    };
    if bought(g, id)? >= limit {
        return Ok(Some("Sold out for today.".into()));
    }
    if g.p.gold < price {
        return Ok(Some(format!("That's {price} gold.")));
    }
    g.gold(-price, "market")?;
    g.item(id, 1, "market")?;
    mark(g, id)?;
    g.bump()?;
    Ok(Some(format!("Bought {}.", data::count(id, 1))))
}
