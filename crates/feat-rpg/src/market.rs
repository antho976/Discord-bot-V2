//! Skagi's market: a stall that changes every day. Three pieces of gear picked for you (Fine
//! or better), and supplies at fixed prices. Somewhere for gold to go besides the forge.

use bot_core::anyhow::Result;
use bot_core::rusqlite::params;

use crate::data::{self, Gear, Rarity};
use crate::emoji::get as e;
use crate::engine::Rng;
use crate::engine::loot::{self, Traits};
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

/// Today's three pieces for a player.
pub fn stock(
    user: u64,
    day: i64,
    level: u32,
    boss: bool,
) -> Vec<(&'static Gear, Rarity, Traits, i64)> {
    let mut r = Rng::new(user.rotate_left(13) ^ (day as u64).wrapping_mul(0x9E37_79B9));
    let mut picked: Vec<&'static Gear> = Vec::new();
    for _ in 0..12 {
        let g = loot::drop_piece(level, boss, &mut r);
        if !picked.iter().any(|x| x.id == g.id) {
            picked.push(g);
        }
        if picked.len() == 3 {
            break;
        }
    }
    picked
        .into_iter()
        .map(|g| {
            let (mut rarity, mut traits) = loot::roll(g, "hunt", &mut r);
            if rarity == Rarity::Common {
                rarity = Rarity::Fine;
                traits = loot::roll_traits(g, 1, &mut r);
            }
            let price = g.gold * 2 + 150 * rarity.index().pow(2) + 10 * level as i64;
            (g, rarity, traits, price)
        })
        .collect()
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
    for (i, (gear, rarity, traits, price)) in stock(p.user, quests::day(p.now), p.level, p.boss_won)
        .into_iter()
        .enumerate()
    {
        let sold = bought(g, &format!("g{i}"))? > 0;
        let worn = p.equipped(gear.slot).map(|x| x.power()).unwrap_or(0);
        let power =
            data::gear_power(gear, 0) + data::gear_power(gear, 0) * rarity.power_pct() / 100;
        let t: Vec<String> = traits
            .iter()
            .map(|(t, v)| format!("{}: {}", t.name(), t.describe(*v)))
            .collect();
        let line = format!(
            "{} **{} {}** · {}{} Power vs worn{}\n-# {} · {} gold{}",
            e(rarity.emoji()),
            rarity.name(),
            gear.name,
            if power >= worn { "+" } else { "" },
            power - worn,
            if gear.level > p.level {
                format!(" · {} level {}", e("lock"), gear.level)
            } else {
                String::new()
            },
            t.join(" · "),
            thousands(price),
            if sold { " · sold" } else { "" }
        );
        s.section(
            line,
            Btn::new(ids.act("market", "gear", &i.to_string()), "Buy")
                .emoji("gold")
                .disabled(sold || p.gold < price || gear.level > p.level),
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
    let Some((gear, rarity, traits, price)) =
        stock(g.p.user, quests::day(g.p.now), g.p.level, g.p.boss_won)
            .into_iter()
            .nth(i)
    else {
        return Ok(None);
    };
    if bought(g, &format!("g{i}"))? > 0 {
        return Ok(Some("Already sold.".into()));
    }
    if g.p.gold < price || gear.level > g.p.level {
        return Ok(Some("You can't buy that yet.".into()));
    }
    g.gold(-price, "market")?;
    mark(g, &format!("g{i}"))?;
    let worn = g.p.equipped(gear.slot).map(|x| x.power()).unwrap_or(0);
    let power = data::gear_power(gear, 0) + data::gear_power(gear, 0) * rarity.power_pct() / 100;
    g.add_piece(gear, rarity, traits, power > worn, "market")?;
    g.bump()?;
    Ok(Some(format!(
        "Bought the {} {}{}.",
        rarity.name(),
        gear.name,
        if power > worn {
            ": you're wearing it"
        } else {
            ": it's in your pack"
        }
    )))
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
