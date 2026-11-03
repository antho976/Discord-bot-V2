//! Shared bosses: one huge HP pool that many players chip at, each in their own fight.
//!
//! - **Titans** belong to the whole server. One rises every few days (or when staff start one
//!   from the dashboard) and stays for two days. Everyone gets three attempts a day.
//! - **Raids** belong to a guild: its leader starts one from the treasury, and members get two
//!   attempts a day for three days.
//!
//! An attempt lasts ten turns; the damage dealt comes off the shared pool in one guarded
//! statement. Whoever helped can claim a reward when it falls, or a small one if it gets away.

use bot_core::anyhow::Result;
use bot_core::rusqlite::{Connection, OptionalExtension, params};

use crate::data;
use crate::emoji::get as e;
use crate::engine::Rng;
use crate::engine::combat::Fight;
use crate::engine::loot;
use crate::game::{Ids, back, dur, seed, thousands, title};
use crate::quests;
use crate::store::{Game, Player};
use crate::ui::{Btn, Screen};

#[derive(Debug, Clone)]
pub struct Shared {
    pub id: i64,
    pub kind: String,
    pub clan: i64,
    pub boss: String,
    pub power: i64,
    pub hp: i64,
    pub max_hp: i64,
    pub starts: i64,
    pub ends: i64,
    pub killed_at: Option<i64>,
}

impl Shared {
    pub fn name(&self) -> &'static str {
        data::monster(&self.boss)
            .map(|m| m.name)
            .unwrap_or("The Titan")
    }
    pub fn over(&self, now: i64) -> bool {
        self.killed_at.is_some() || now >= self.ends
    }
}

fn row(r: &bot_core::rusqlite::Row) -> bot_core::rusqlite::Result<Shared> {
    Ok(Shared {
        id: r.get(0)?,
        kind: r.get(1)?,
        clan: r.get(2)?,
        boss: r.get(3)?,
        power: r.get(4)?,
        hp: r.get(5)?,
        max_hp: r.get(6)?,
        starts: r.get(7)?,
        ends: r.get(8)?,
        killed_at: r.get(9)?,
    })
}

const COLS: &str = "id, kind, clan_id, boss, power, hp, max_hp, starts, ends, killed_at";

/// The one still going, or the latest one if it ended in the last day (so rewards can be
/// claimed from it).
pub fn active(c: &Connection, guild: u64, kind: &str, clan: i64) -> Result<Option<Shared>> {
    let now = bot_core::now_ms();
    Ok(c.query_row(
        &format!("SELECT {COLS} FROM rpg_shared WHERE guild_id = ?1 AND kind = ?2 AND clan_id = ?3 AND ends > ?4 - 86400000 ORDER BY id DESC LIMIT 1"),
        params![guild as i64, kind, clan, now],
        row,
    )
    .optional()?)
}

pub fn by_id(c: &Connection, id: i64) -> Result<Option<Shared>> {
    Ok(c.query_row(
        &format!("SELECT {COLS} FROM rpg_shared WHERE id = ?1"),
        params![id],
        row,
    )
    .optional()?)
}

/// (damage, attempts, attempts today, claimed)
pub fn mine(c: &Connection, id: i64, user: u64, day: i64) -> Result<(i64, i64, i64, bool)> {
    Ok(c.query_row(
        "SELECT damage, attempts, CASE WHEN day = ?3 THEN day_attempts ELSE 0 END, claimed FROM rpg_shared_dmg WHERE shared_id = ?1 AND user_id = ?2",
        params![id, user as i64, day],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, i64>(3)? != 0)),
    )
    .optional()?
    .unwrap_or((0, 0, 0, false)))
}

/// Top damage: (user, name, damage).
pub fn board(c: &Connection, id: i64, limit: i64) -> Result<Vec<(u64, String, i64)>> {
    let mut st = c.prepare_cached(
        "SELECT d.user_id, COALESCE(p.name, ''), d.damage FROM rpg_shared_dmg d
         JOIN rpg_shared s ON s.id = d.shared_id
         LEFT JOIN rpg_players p ON p.guild_id = s.guild_id AND p.user_id = d.user_id
         WHERE d.shared_id = ?1 ORDER BY d.damage DESC LIMIT ?2",
    )?;
    let rows = st
        .query_map(params![id, limit], |r| {
            Ok((r.get::<_, i64>(0)? as u64, r.get(1)?, r.get(2)?))
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

fn total_damage(c: &Connection, id: i64) -> Result<i64> {
    Ok(c.query_row(
        "SELECT COALESCE(SUM(damage), 0) FROM rpg_shared_dmg WHERE shared_id = ?1",
        params![id],
        |r| r.get(0),
    )?)
}

/// Start a shared boss. Returns its id.
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    c: &Connection,
    guild: u64,
    kind: &str,
    clan: i64,
    boss: &str,
    power: i64,
    hp: i64,
    hours: i64,
) -> Result<i64> {
    let now = bot_core::now_ms();
    c.execute(
        "INSERT INTO rpg_shared (guild_id, kind, clan_id, boss, power, hp, max_hp, starts, ends) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8)",
        params![guild as i64, kind, clan, boss, power, hp, now, now + hours * 3_600_000],
    )?;
    Ok(c.last_insert_rowid())
}

/// The Power a Titan fights at: around the server's typical player, never trivial.
pub fn titan_power(c: &Connection, guild: u64, since: i64) -> Result<i64> {
    let avg: Option<f64> = c.query_row(
        "SELECT AVG(level) FROM rpg_players WHERE guild_id = ?1 AND last_seen >= ?2",
        params![guild as i64, since],
        |r| r.get(0),
    )?;
    let lvl = avg.unwrap_or(5.0);
    Ok((60.0 + lvl * 14.0).max(100.0) as i64)
}

/// Start a Titan now if none is up. Returns the announcement, if one rose.
pub fn rise_titan(c: &Connection, guild: u64, hours: i64) -> Result<Option<String>> {
    if let Some(t) = active(c, guild, "titan", 0)?
        && !t.over(bot_core::now_ms())
    {
        return Ok(None);
    }
    let now = bot_core::now_ms();
    let week_ago = now - 7 * 86_400_000;
    let players: i64 = c.query_row(
        "SELECT COUNT(*) FROM rpg_players WHERE guild_id = ?1 AND last_seen >= ?2",
        params![guild as i64, week_ago],
        |r| r.get(0),
    )?;
    let n: i64 = c.query_row(
        "SELECT COUNT(*) FROM rpg_shared WHERE guild_id = ?1 AND kind = 'titan'",
        params![guild as i64],
        |r| r.get(0),
    )?;
    let boss = data::TITANS[(n as usize) % data::TITANS.len()];
    let hp = data::TITAN_HP_PER_PLAYER * players.max(3);
    let power = titan_power(c, guild, week_ago)?;
    spawn(c, guild, "titan", 0, boss, power, hp, hours)?;
    let name = data::monster(boss).map(|m| m.name).unwrap_or("A Titan");
    Ok(Some(format!(
        "{} **{name} rises over Midgard.** Everyone can fight it from `/rpg`: {} attempts a day for the next {hours} hours. When it falls, everyone who fought shares the spoils.",
        e("boss"),
        data::TITAN_ATTEMPTS
    )))
}

/// Whether a Titan is due: none for `every` hours since the last one ended.
pub fn titan_due(c: &Connection, guild: u64, every_hours: i64) -> Result<bool> {
    if every_hours <= 0 {
        return Ok(false);
    }
    let last: Option<i64> = c.query_row(
        "SELECT MAX(COALESCE(killed_at, ends)) FROM rpg_shared WHERE guild_id = ?1 AND kind = 'titan'",
        params![guild as i64],
        |r| r.get(0),
    )?;
    let now = bot_core::now_ms();
    // The first Titan comes a day after the RPG starts being played.
    let since = match last {
        Some(t) => t,
        None => c
            .query_row(
                "SELECT MIN(created_at) FROM rpg_players WHERE guild_id = ?1",
                params![guild as i64],
                |r| r.get::<_, Option<i64>>(0),
            )?
            .map(|t| t - (every_hours - 24) * 3_600_000)
            .unwrap_or(now),
    };
    Ok(now - since >= every_hours * 3_600_000)
}

/// Titans that ended without falling and haven't been announced: (id, text).
pub fn unannounced_endings(c: &Connection, guild: u64) -> Result<Vec<(i64, String)>> {
    let now = bot_core::now_ms();
    let mut st = c.prepare_cached(&format!("SELECT {COLS} FROM rpg_shared WHERE guild_id = ?1 AND kind = 'titan' AND killed_at IS NULL AND ends <= ?2 AND announced = 0"))?;
    let list: Vec<Shared> = st
        .query_map(params![guild as i64, now], row)?
        .collect::<Result<_, _>>()?;
    Ok(list
        .into_iter()
        .map(|t| {
            let pct = 100 - t.hp * 100 / t.max_hp.max(1);
            (t.id, format!("{} **{}** sinks back into the deep, wounded {pct}%. Everyone who fought it can still claim something in `/rpg`.", e("boss"), t.name()))
        })
        .collect())
}

pub fn mark_announced(c: &Connection, id: i64) -> Result<()> {
    c.execute(
        "UPDATE rpg_shared SET announced = 1 WHERE id = ?1",
        params![id],
    )?;
    Ok(())
}

fn attempts_allowed(p: &Player, kind: &str) -> i64 {
    match kind {
        "raid" => {
            data::RAID_ATTEMPTS
                + if p.clan.as_ref().is_some_and(|c| c.level() >= 6) {
                    1
                } else {
                    0
                }
        }
        _ => data::TITAN_ATTEMPTS,
    }
}

/// Begin an attempt on a shared boss.
pub fn attempt(g: &mut Game, kind: &str) -> Result<Option<String>> {
    let clan = if kind == "raid" {
        g.p.clan.as_ref().map(|c| c.id).unwrap_or(-1)
    } else {
        0
    };
    let Some(sb) = active(&g.tx, g.p.guild, kind, clan)? else {
        return Ok(Some("There's nothing to fight right now.".into()));
    };
    if sb.over(g.p.now) {
        return Ok(Some("It's over. Claim your share below.".into()));
    }
    if g.p.fight.is_some() {
        return Ok(Some("Finish the fight you're in first.".into()));
    }
    if g.p.hp() * 10 < g.p.max_hp() * 5 {
        return Ok(Some("Go in with at least half your HP.".into()));
    }
    let day = quests::day(g.p.now);
    let (_, _, today, _) = mine(&g.tx, sb.id, g.p.user, day)?;
    if today >= attempts_allowed(&g.p, kind) {
        return Ok(Some("You've used today's attempts. Back tomorrow.".into()));
    }
    let Some(m) = data::monster(&sb.boss) else {
        return Ok(None);
    };
    let mut hero = g.p.hero();
    if g.p.clan.as_ref().is_some_and(|c| c.has("banner")) {
        hero.mods.damage += 0.05;
    }
    let mut f = Fight::shared(
        hero,
        g.p.hp(),
        m,
        sb.power,
        data::SHARED_TURNS,
        sb.id,
        seed(&g.p, 113),
    );
    f.region = format!("{kind}:{}", sb.id);
    g.save_fight(Some(f))?;
    g.bump()?;
    Ok(None)
}

/// An attempt ended: its damage comes off the pool. Returns what to say and, when this
/// attempt felled it, the public line.
pub fn settle(g: &mut Game, f: &Fight) -> Result<(String, Option<String>)> {
    let id: i64 = f
        .region
        .rsplit(':')
        .next()
        .and_then(|x| x.parse().ok())
        .unwrap_or(-1);
    let Some(sb) = by_id(&g.tx, id)? else {
        return Ok((String::new(), None));
    };
    let dealt = f.dealt();
    let day = quests::day(g.p.now);
    g.tx.execute(
        "INSERT INTO rpg_shared_dmg (shared_id, user_id, damage, attempts, day, day_attempts) VALUES (?1, ?2, ?3, 1, ?4, 1)
         ON CONFLICT (shared_id, user_id) DO UPDATE SET damage = damage + excluded.damage, attempts = attempts + 1,
            day_attempts = CASE WHEN day = excluded.day THEN day_attempts + 1 ELSE 1 END, day = excluded.day",
        params![id, g.p.user as i64, dealt, day],
    )?;
    let mut announce = None;
    if sb.killed_at.is_none() {
        // One guarded statement: the pool never goes below zero, and only one attempt fells it.
        g.tx.execute(
            "UPDATE rpg_shared SET hp = MAX(0, hp - ?2) WHERE id = ?1 AND killed_at IS NULL",
            params![id, dealt],
        )?;
        let fell = g.tx.execute(
            "UPDATE rpg_shared SET killed_at = ?2 WHERE id = ?1 AND hp = 0 AND killed_at IS NULL",
            params![id, g.p.now],
        )? == 1;
        if fell {
            if sb.kind == "raid" {
                g.tx.execute(
                    "UPDATE rpg_clans SET xp = xp + 1000 WHERE id = ?1",
                    params![sb.clan],
                )?;
            }
            announce = Some(if sb.kind == "titan" {
                format!(
                    "{} **{}** falls! {} landed the last blow. Everyone who fought it can claim their share in `/rpg`.",
                    e("boss"),
                    sb.name(),
                    g.p.name
                )
            } else {
                format!(
                    "{} **{}** brought down **{}** for their guild.",
                    e("boss"),
                    g.p.name,
                    sb.name()
                )
            });
        }
    }
    crate::guild::credit(g, 20, 0)?;
    let left = by_id(&g.tx, id)?.map(|x| x.hp).unwrap_or(0);
    let said = format!(
        "{} You dealt **{}** damage. {} has {} HP left ({}%).",
        e("boss"),
        thousands(dealt),
        sb.name(),
        thousands(left),
        left * 100 / sb.max_hp.max(1)
    );
    Ok((said, announce))
}

/// Claim the reward from a shared boss that's over.
pub fn claim(g: &mut Game, kind: &str) -> Result<Option<String>> {
    let clan = if kind == "raid" {
        g.p.clan.as_ref().map(|c| c.id).unwrap_or(-1)
    } else {
        0
    };
    let Some(sb) = active(&g.tx, g.p.guild, kind, clan)? else {
        return Ok(None);
    };
    if !sb.over(g.p.now) {
        return Ok(Some("It's not over yet.".into()));
    }
    let (dmg, attempts, _, claimed) = mine(&g.tx, sb.id, g.p.user, 0)?;
    if attempts == 0 || claimed {
        return Ok(None);
    }
    g.tx.execute(
        "UPDATE rpg_shared_dmg SET claimed = 1 WHERE shared_id = ?1 AND user_id = ?2",
        params![sb.id, g.p.user as i64],
    )?;
    let total = total_damage(&g.tx, sb.id)?.max(1);
    let share = dmg as f64 / total as f64;
    let mut rng = Rng::new(seed(&g.p, 131));
    let why = if kind == "raid" {
        "the guild raid"
    } else {
        "the Titan"
    };
    let said = if sb.killed_at.is_some() {
        let gold = if kind == "raid" {
            300
        } else {
            150 + (900.0 * share.min(1.0)) as i64
        };
        let threads = if kind == "raid" {
            4
        } else {
            2 + if share >= 0.2 { 2 } else { 0 }
        };
        g.gold(gold, why)?;
        g.item("thread", threads, why)?;
        let piece = loot::drop_piece(g.p.level, g.p.boss_won, &mut rng);
        let (rarity, traits) = loot::roll(
            piece,
            if kind == "raid" { "guardian" } else { "elite" },
            &mut rng,
        );
        g.add_piece(piece, rarity, traits, false, why)?;
        let top = board(&g.tx, sb.id, 1)?.first().map(|x| x.0) == Some(g.p.user);
        let title = if kind == "raid" {
            Some("Wyrmbane")
        } else if top {
            Some("Titan-slayer")
        } else {
            None
        };
        if let Some(t) = title {
            g.add_title(t)?;
        }
        format!(
            "Your share of {}: {gold} gold, {}, and a **{} {}** in your pack.{}",
            sb.name(),
            data::count("thread", threads),
            rarity.name(),
            piece.name,
            title
                .map(|t| format!(" And the title **{t}**."))
                .unwrap_or_default()
        )
    } else {
        let gold = 40 * attempts;
        g.gold(gold, why)?;
        g.item("thread", 1, why)?;
        format!(
            "{} got away, but you made it bleed: {gold} gold and a loose thread.",
            sb.name()
        )
    };
    g.bump()?;
    Ok(Some(said))
}

pub fn screen(g: &Game, kind: &str) -> Result<Screen> {
    let p = &g.p;
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    let clan = if kind == "raid" {
        p.clan.as_ref().map(|c| c.id).unwrap_or(-1)
    } else {
        0
    };
    let Some(sb) = active(&g.tx, p.guild, kind, clan)? else {
        if kind == "raid" {
            title(&mut s, "boss", "Guild raid", "Nothing to fight");
            s.text("Your guild's leader starts a raid from the guild screen.");
            s.buttons(vec![
                back(&ids),
                Btn::new(ids.go("guild"), "Guild").emoji("guild"),
            ]);
        } else {
            title(&mut s, "boss", "Titans", "Server-wide");
            s.text("Every few days a Titan rises over Midgard. Everyone fights it, three attempts a day, and everyone who did shares the spoils when it falls. None is up right now.");
            s.buttons(vec![back(&ids)]);
        }
        return Ok(s);
    };
    let day = quests::day(p.now);
    let (dmg, attempts, today, claimed) = mine(&g.tx, sb.id, p.user, day)?;
    let allowed = attempts_allowed(p, kind);
    let over = sb.over(p.now);
    let label = if kind == "raid" {
        "Guild raid"
    } else {
        "Titan"
    };
    let meta = if over {
        if sb.killed_at.is_some() {
            format!("{label} · fallen")
        } else {
            format!("{label} · got away")
        }
    } else {
        format!("{label} · {} left", dur(sb.ends - p.now))
    };
    title(&mut s, "boss", sb.name(), &meta);
    let pct = sb.hp * 100 / sb.max_hp.max(1);
    let filled = (pct / 10) as usize;
    let bar = format!(
        "{}{}",
        e("pip").repeat(filled),
        e("pipoff").repeat(10 - filled.min(10))
    );
    s.text(format!(
        "{bar}\n**{}** / {} HP · {pct}% left",
        thousands(sb.hp),
        thousands(sb.max_hp)
    ));
    if let Some(m) = data::monster(&sb.boss) {
        s.text(format!(
            "-# Fights at {} Power · weak to {} · each attempt lasts {} turns",
            sb.power,
            m.weak.unwrap_or("nothing"),
            data::SHARED_TURNS
        ));
    }
    s.text(format!(
        "{} Your damage: **{}** in {attempts} attempt{} · today {today} of {allowed}",
        e("hunt"),
        thousands(dmg),
        if attempts == 1 { "" } else { "s" }
    ));
    let top = board(&g.tx, sb.id, 5)?;
    if !top.is_empty() {
        let list: Vec<String> = top
            .iter()
            .enumerate()
            .map(|(i, (u, n, d))| {
                format!(
                    "-# {}. {}{} · {}",
                    i + 1,
                    if n.is_empty() { "Someone" } else { n },
                    if *u == p.user { " (you)" } else { "" },
                    thousands(*d)
                )
            })
            .collect();
        s.text(format!("-# Most damage\n{}", list.join("\n")));
    }
    let mut b = vec![back(&ids)];
    if over {
        b.push(
            Btn::new(ids.act(kind, "claim", ""), "Claim your share")
                .emoji("gold")
                .primary()
                .disabled(attempts == 0 || claimed),
        );
        if claimed {
            s.text(format!("-# {} Claimed.", e("check")));
        }
    } else {
        b.push(
            Btn::new(ids.act(kind, "fight", ""), "Attack")
                .emoji("boss")
                .danger()
                .disabled(today >= allowed || p.fight.is_some()),
        );
    }
    s.buttons(b);
    Ok(s)
}
