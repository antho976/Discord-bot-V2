//! Guilds: players band together. A guild grows from what its members do (never from buying
//! power), has a treasury that only buys guild upgrades, a weekly challenge, and a raid boss
//! its members fight together over a few days.

use bot_core::anyhow::Result;
use bot_core::rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::data;
use crate::emoji::get as e;
use crate::game::{Ids, back, thousands, title};
use crate::store::{Game, Player};
use crate::ui::{Btn, Opt, Screen};

pub const MAX_MEMBERS: i64 = 12;
pub const FOUND_GOLD: i64 = 500;

/// A player's guild, as loaded with them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Membership {
    pub id: i64,
    pub name: String,
    pub tag: String,
    pub leader: bool,
    pub xp: i64,
    pub treasury: i64,
    pub upgrades: Vec<String>,
}

impl Membership {
    pub fn level(&self) -> u32 {
        level(self.xp)
    }
    pub fn has(&self, upgrade: &str) -> bool {
        self.upgrades.iter().any(|u| u == upgrade)
    }
}

/// Guild level from its XP: 400, 1,600, 3,600… for levels 2, 3, 4…, up to 10.
pub fn level(xp: i64) -> u32 {
    (((xp.max(0) as f64) / 400.0).sqrt().floor() as u32 + 1).min(10)
}

pub fn xp_for(level: u32) -> i64 {
    400 * (level as i64 - 1).pow(2)
}

/// What a guild level gives every member.
pub fn perks(level: u32) -> Vec<&'static str> {
    let mut p = Vec::new();
    if level >= 2 {
        p.push("+2% XP");
    }
    if level >= 4 {
        p.push("+3% materials");
    }
    if level >= 6 {
        p.push("+1 raid attempt a day");
    }
    if level >= 8 {
        p.push("+5% gold");
    }
    if level >= 10 {
        p.push("the title Sworn of the Hall");
    }
    p
}

/// (id, name, what it does, treasury cost)
pub const UPGRADES: [(&str, &str, &str, i64); 3] = [
    ("mead", "Mead Hall", "+10% Vigor while away", 4000),
    ("banner", "War Banner", "+5% damage against Titans and raids", 6000),
    ("longship", "Longship", "+5% materials from expeditions", 8000),
];

pub fn membership(c: &Connection, guild: u64, user: u64) -> Result<Option<Membership>> {
    Ok(c.query_row(
        "SELECT c.id, c.name, c.tag, c.leader, c.xp, c.treasury, c.upgrades FROM rpg_clan_members m
         JOIN rpg_clans c ON c.id = m.clan_id WHERE m.guild_id = ?1 AND m.user_id = ?2",
        params![guild as i64, user as i64],
        |r| {
            Ok(Membership {
                id: r.get(0)?,
                name: r.get(1)?,
                tag: r.get(2)?,
                leader: r.get::<_, i64>(3)? as u64 == user,
                xp: r.get(4)?,
                treasury: r.get(5)?,
                upgrades: serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or_default(),
            })
        },
    )
    .optional()?)
}

pub struct Member {
    pub user: u64,
    pub name: String,
    pub leader: bool,
    pub week_xp: i64,
    pub level: i64,
}

pub fn members(c: &Connection, guild: u64, clan: i64, week: i64) -> Result<Vec<Member>> {
    let leader: i64 = c.query_row("SELECT leader FROM rpg_clans WHERE id = ?1", params![clan], |r| r.get(0)).unwrap_or(0);
    let mut st = c.prepare_cached(
        "SELECT m.user_id, COALESCE(p.name, ''), CASE WHEN m.week = ?3 THEN m.week_xp ELSE 0 END, COALESCE(p.level, 1)
         FROM rpg_clan_members m LEFT JOIN rpg_players p ON p.guild_id = m.guild_id AND p.user_id = m.user_id
         WHERE m.guild_id = ?1 AND m.clan_id = ?2 ORDER BY 3 DESC, m.joined",
    )?;
    let rows = st
        .query_map(params![guild as i64, clan, week], |r| {
            let u = r.get::<_, i64>(0)?;
            Ok(Member { user: u as u64, name: r.get(1)?, leader: u == leader, week_xp: r.get(2)?, level: r.get(3)? })
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

/// A guild in the list to join: (id, name, tag, members, level).
pub type Listed = (i64, String, String, i64, u32);

/// Guilds, strongest first, for joining.
pub fn open_guilds(c: &Connection, guild: u64) -> Result<Vec<Listed>> {
    let mut st = c.prepare_cached(
        "SELECT c.id, c.name, c.tag, (SELECT COUNT(*) FROM rpg_clan_members m WHERE m.clan_id = c.id), c.xp
         FROM rpg_clans c WHERE c.guild_id = ?1 ORDER BY c.xp DESC LIMIT 25",
    )?;
    let rows = st
        .query_map(params![guild as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, level(r.get(4)?))))?
        .collect::<Result<Vec<Listed>, _>>()?;
    Ok(rows)
}

/// The weekly challenge: wins as a guild, 12 per member.
pub fn challenge(c: &Connection, clan: i64, week: i64) -> Result<(i64, i64, bool)> {
    let (n, w, wins, claimed): (i64, i64, i64, i64) = c.query_row(
        "SELECT (SELECT COUNT(*) FROM rpg_clan_members m WHERE m.clan_id = c.id), c.week, c.week_wins, c.week_claimed FROM rpg_clans c WHERE c.id = ?1",
        params![clan],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let target = 12 * n.max(1);
    if w != week {
        return Ok((0, target, false));
    }
    Ok((wins, target, claimed != 0))
}

/// A member did something: guild XP, their weekly share, and hunt wins for the challenge.
pub fn credit(g: &mut Game, xp: i64, wins: i64) -> Result<()> {
    let Some(m) = g.p.clan.clone() else { return Ok(()) };
    if xp <= 0 && wins <= 0 {
        return Ok(());
    }
    let week = g.p.week();
    let before = m.level();
    g.tx.execute(
        "UPDATE rpg_clans SET xp = xp + ?2,
            week_wins = CASE WHEN week = ?3 THEN week_wins + ?4 ELSE ?4 END,
            week_claimed = CASE WHEN week = ?3 THEN week_claimed ELSE 0 END,
            week = ?3
         WHERE id = ?1",
        params![m.id, xp, week, wins],
    )?;
    g.tx.execute(
        "UPDATE rpg_clan_members SET week_xp = CASE WHEN week = ?3 THEN week_xp + ?4 ELSE ?4 END, week = ?3 WHERE guild_id = ?1 AND user_id = ?2",
        params![g.p.guild as i64, g.p.user as i64, week, xp],
    )?;
    let after = level(m.xp + xp);
    if let Some(c) = g.p.clan.as_mut() {
        c.xp += xp;
    }
    // The weekly challenge pays the treasury the moment it's done.
    let (done, target, claimed) = challenge(&g.tx, m.id, week)?;
    if !claimed && done >= target {
        g.tx.execute("UPDATE rpg_clans SET week_claimed = 1, treasury = treasury + 800, xp = xp + 600 WHERE id = ?1", params![m.id])?;
    }
    if after >= 10 && before < 10 {
        g.add_title("Sworn of the Hall")?;
    }
    Ok(())
}

pub fn found(g: &mut Game, name: &str, tag: &str) -> Result<Option<String>> {
    let name = name.trim();
    let tag = tag.trim().to_uppercase();
    if g.p.clan.is_some() {
        return Ok(Some("Leave your guild first.".into()));
    }
    if !(3..=24).contains(&name.chars().count()) {
        return Ok(Some("A guild name is 3 to 24 characters.".into()));
    }
    if !(2..=4).contains(&tag.chars().count()) || !tag.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Ok(Some("A tag is 2 to 4 letters or numbers.".into()));
    }
    if g.p.gold < FOUND_GOLD {
        return Ok(Some(format!("Founding a guild costs {FOUND_GOLD} gold.")));
    }
    let taken: bool = g
        .tx
        .query_row("SELECT 1 FROM rpg_clans WHERE guild_id = ?1 AND lower(name) = lower(?2)", params![g.p.guild as i64, name], |_| Ok(true))
        .optional()?
        .unwrap_or(false);
    if taken {
        return Ok(Some("A guild with that name already exists.".into()));
    }
    g.gold(-FOUND_GOLD, "founded a guild")?;
    g.tx.execute(
        "INSERT INTO rpg_clans (guild_id, name, tag, leader, created) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![g.p.guild as i64, name, tag, g.p.user as i64, g.p.now],
    )?;
    let id = g.tx.last_insert_rowid();
    g.tx.execute(
        "INSERT INTO rpg_clan_members (guild_id, user_id, clan_id, role, joined) VALUES (?1, ?2, ?3, 'leader', ?4)",
        params![g.p.guild as i64, g.p.user as i64, id, g.p.now],
    )?;
    g.p.clan = crate::guild::membership(&g.tx, g.p.guild, g.p.user)?;
    g.bump()?;
    Ok(Some(format!("You founded **{name}** [{tag}].")))
}

pub fn join(g: &mut Game, clan: i64) -> Result<Option<String>> {
    if g.p.clan.is_some() {
        return Ok(Some("Leave your guild first.".into()));
    }
    let row: Option<(String, i64)> = g
        .tx
        .query_row(
            "SELECT c.name, (SELECT COUNT(*) FROM rpg_clan_members m WHERE m.clan_id = c.id) FROM rpg_clans c WHERE c.id = ?1 AND c.guild_id = ?2",
            params![clan, g.p.guild as i64],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((name, n)) = row else { return Ok(Some("That guild is gone.".into())) };
    if n >= MAX_MEMBERS {
        return Ok(Some(format!("{name} is full.")));
    }
    g.tx.execute(
        "INSERT INTO rpg_clan_members (guild_id, user_id, clan_id, role, joined) VALUES (?1, ?2, ?3, 'member', ?4)",
        params![g.p.guild as i64, g.p.user as i64, clan, g.p.now],
    )?;
    g.p.clan = membership(&g.tx, g.p.guild, g.p.user)?;
    g.bump()?;
    Ok(Some(format!("You joined **{name}**.")))
}

pub fn leave(g: &mut Game) -> Result<Option<String>> {
    let Some(m) = g.p.clan.clone() else { return Ok(None) };
    g.tx.execute("DELETE FROM rpg_clan_members WHERE guild_id = ?1 AND user_id = ?2", params![g.p.guild as i64, g.p.user as i64])?;
    let next: Option<i64> = g
        .tx
        .query_row("SELECT user_id FROM rpg_clan_members WHERE clan_id = ?1 ORDER BY joined LIMIT 1", params![m.id], |r| r.get(0))
        .optional()?;
    match next {
        None => {
            g.tx.execute("DELETE FROM rpg_clans WHERE id = ?1", params![m.id])?;
        }
        Some(u) if m.leader => {
            g.tx.execute("UPDATE rpg_clans SET leader = ?2 WHERE id = ?1", params![m.id, u])?;
            g.tx.execute("UPDATE rpg_clan_members SET role = 'leader' WHERE clan_id = ?1 AND user_id = ?2", params![m.id, u])?;
        }
        _ => {}
    }
    g.p.clan = None;
    g.bump()?;
    Ok(Some(format!("You left **{}**.", m.name)))
}

pub fn kick(g: &mut Game, user: u64) -> Result<Option<String>> {
    let Some(m) = g.p.clan.clone() else { return Ok(None) };
    if !m.leader || user == g.p.user {
        return Ok(None);
    }
    g.tx.execute("DELETE FROM rpg_clan_members WHERE guild_id = ?1 AND user_id = ?2 AND clan_id = ?3", params![g.p.guild as i64, user as i64, m.id])?;
    g.bump()?;
    Ok(Some("They're no longer in the guild.".into()))
}

pub fn give(g: &mut Game, n: i64) -> Result<Option<String>> {
    let Some(m) = g.p.clan.clone() else { return Ok(None) };
    if g.p.gold < n {
        return Ok(Some("You don't have that much gold.".into()));
    }
    g.gold(-n, "guild treasury")?;
    g.tx.execute("UPDATE rpg_clans SET treasury = treasury + ?2 WHERE id = ?1", params![m.id, n])?;
    credit(g, n / 20, 0)?;
    if let Some(c) = g.p.clan.as_mut() {
        c.treasury += n;
    }
    g.bump()?;
    Ok(Some(format!("{n} gold into the treasury. It only ever buys guild upgrades.")))
}

pub fn buy(g: &mut Game, id: &str) -> Result<Option<String>> {
    let Some(m) = g.p.clan.clone() else { return Ok(None) };
    let Some((_, name, _, cost)) = UPGRADES.iter().find(|u| u.0 == id) else { return Ok(None) };
    if !m.leader {
        return Ok(Some("Only the guild's leader buys upgrades.".into()));
    }
    if m.has(id) {
        return Ok(Some("The guild already has that.".into()));
    }
    if m.treasury < *cost {
        return Ok(Some(format!("The treasury needs {} gold for the {name}.", thousands(*cost))));
    }
    let mut ups = m.upgrades.clone();
    ups.push(id.to_owned());
    g.tx.execute("UPDATE rpg_clans SET treasury = treasury - ?2, upgrades = ?3 WHERE id = ?1", params![m.id, cost, serde_json::to_string(&ups)?])?;
    g.p.clan = membership(&g.tx, g.p.guild, g.p.user)?;
    g.bump()?;
    Ok(Some(format!("The guild built the **{name}**.")))
}

pub fn screen(g: &Game) -> Result<Screen> {
    let p = &g.p;
    let ids = Ids { owner: p.user, version: p.version };
    let mut s = Screen::default();
    let Some(m) = &p.clan else {
        title(&mut s, "guild", "Guilds", "Band together");
        s.text(format!(
            "A guild grows from what its members do: hunts, expeditions, floors, dungeons. It levels up for small bonuses everyone shares, has a weekly challenge, and fights a raid boss together.\n-# Gold given to a guild only buys guild upgrades, never power for one member. Up to {MAX_MEMBERS} members."
        ));
        let open = open_guilds(&g.tx, p.guild)?;
        let opts: Vec<Opt> = open
            .iter()
            .filter(|x| x.3 < MAX_MEMBERS)
            .map(|(id, name, tag, n, lvl)| Opt::new(format!("{name} [{tag}]"), id.to_string()).emoji("guild").desc(format!("Level {lvl} · {n} of {MAX_MEMBERS} members")))
            .collect();
        if opts.is_empty() {
            s.text("-# No guilds yet. Found the first one.");
        } else {
            s.inner_select(ids.act("guild", "join", ""), "Join a guild…", opts);
        }
        s.buttons(vec![back(&ids), Btn::new(ids.act("guild", "new", ""), format!("Found a guild · {FOUND_GOLD} gold")).emoji("guild").primary().disabled(p.gold < FOUND_GOLD)]);
        return Ok(s);
    };
    let lvl = m.level();
    let week = p.week();
    title(&mut s, "guild", &format!("{} [{}]", m.name, m.tag), &format!("Guild level {lvl}{}", if m.leader { " · you lead it" } else { "" }));
    let next = if lvl < 10 { format!("{} / {} XP to level {}", thousands(m.xp), thousands(xp_for(lvl + 1)), lvl + 1) } else { "Highest level".into() };
    let perks = perks(lvl);
    s.text(format!(
        "{} {next}\n-# Everyone in the guild gets: {}",
        e("guild"),
        if perks.is_empty() { "nothing yet, from level 2 on +2% XP".into() } else { perks.join(", ") }
    ));
    let (done, target, claimed) = challenge(&g.tx, m.id, week)?;
    s.text(format!(
        "{} **Weekly challenge:** {} / {target} hunts won as a guild{}",
        e("bounty"),
        done.min(target),
        if claimed { "  ·  done: +800 treasury, +600 guild XP" } else { "\n-# Pays 800 gold into the treasury and 600 guild XP" }
    ));
    let raid = crate::shared::active(&g.tx, p.guild, "raid", m.id)?;
    match &raid {
        Some(r) => s.section(
            format!("{} **Raid: {}**\n-# {}% left · ends in {}", e("boss"), data::monster(&r.boss).map(|x| x.name).unwrap_or("the wyrm"), r.hp * 100 / r.max_hp.max(1), crate::game::dur(r.ends - p.now)),
            Btn::new(ids.go("raid"), "Raid").emoji("boss").primary(),
        ),
        None => s.section(
            format!("{} **Raid**\n-# Fafnir the Hoard-Wyrm, fought by the whole guild over three days. Starting it costs {} from the treasury.", e("boss"), thousands(data::RAID_COST)),
            Btn::new(ids.act("guild", "raid", ""), "Start the raid").emoji("boss").disabled(!m.leader || m.treasury < data::RAID_COST),
        ),
    };
    s.gap();
    s.text(format!("{} **Treasury:** {} gold", e("gold"), thousands(m.treasury)));
    let ups: Vec<String> = UPGRADES.iter().map(|(id, name, what, cost)| if m.has(id) { format!("{} {name}: {what}", e("check")) } else { format!("{} {name}: {what} · {} gold", e("lock"), thousands(*cost)) }).collect();
    s.text(format!("-# {}", ups.join("\n-# ")));
    if m.leader {
        let opts: Vec<Opt> = UPGRADES.iter().filter(|u| !m.has(u.0)).map(|(id, name, what, cost)| Opt::new(format!("{name} · {} gold", thousands(*cost)), *id).emoji("guild").desc(*what)).collect();
        s.inner_select(ids.act("guild", "buy", ""), "Build an upgrade from the treasury…", opts);
    }
    let mem = members(&g.tx, p.guild, m.id, week)?;
    let list: Vec<String> = mem.iter().take(12).map(|x| format!("-# {}{} · level {} · {} guild XP this week", if x.leader { "👑 " } else { "" }, if x.name.is_empty() { "Someone" } else { &x.name }, x.level, thousands(x.week_xp))).collect();
    s.text(format!("-# Members ({} of {MAX_MEMBERS})\n{}", mem.len(), list.join("\n")));
    if m.leader && mem.len() > 1 {
        let opts: Vec<Opt> = mem.iter().filter(|x| x.user != p.user).map(|x| Opt::new(format!("Remove {}", if x.name.is_empty() { "this member" } else { &x.name }), x.user.to_string())).collect();
        s.inner_select(ids.act("guild", "kick", ""), "Remove a member…", opts);
    }
    s.buttons(vec![
        back(&ids),
        Btn::new(ids.act("guild", "give", "100"), "Give 100 gold").emoji("gold").disabled(p.gold < 100),
        Btn::new(ids.act("guild", "give", "1000"), "Give 1,000").emoji("gold").disabled(p.gold < 1000),
        Btn::new(ids.act("guild", "leave", ""), "Leave").danger(),
    ]);
    Ok(s)
}

/// XP bonus from the guild, in percent.
pub fn xp_bonus(p: &Player) -> i64 {
    p.clan.as_ref().map(|c| if c.level() >= 2 { 2 } else { 0 }).unwrap_or(0)
}

pub fn loot_bonus(p: &Player) -> i64 {
    p.clan.as_ref().map(|c| (if c.level() >= 4 { 3 } else { 0 }) + if c.has("longship") { 5 } else { 0 }).unwrap_or(0)
}

pub fn gold_bonus(p: &Player) -> i64 {
    p.clan.as_ref().map(|c| if c.level() >= 8 { 5 } else { 0 }).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_climb_and_cap() {
        assert_eq!(level(0), 1);
        assert_eq!(level(399), 1);
        assert_eq!(level(400), 2);
        assert_eq!(level(1600), 3);
        assert_eq!(level(10_000_000), 10);
        for l in 2..=10 {
            assert_eq!(level(xp_for(l)), l);
        }
    }
}
