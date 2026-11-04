//! Guilds: players band together. A guild grows from what its members do (never from buying
//! power), has a treasury that only buys guild upgrades, a weekly challenge, and a raid boss
//! its members fight together over a few days.

use bot_core::anyhow::Result;
use bot_core::rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::data;
use crate::emoji::get as e;
use crate::game::{Ids, ThenPrimary, back, thousands, title};
use crate::store::{Game, Player};
use crate::ui::{Btn, Opt, Screen};

pub const MAX_MEMBERS: i64 = 12;
pub const FOUND_GOLD: i64 = 500;
/// Asks and invites lapse after a week.
const PENDING_MS: i64 = 7 * 86_400_000;
/// A kicked player can't join or ask that guild again for three days (an invite still works).
const KICK_MS: i64 = 3 * 86_400_000;
/// How many guilds a player may be waiting on at once.
const MAX_ASKS: i64 = 3;
/// Weekly guild-XP minimums a leader can set (0 is none).
pub const MINIMUMS: [i64; 6] = [0, 50, 100, 250, 500, 1000];
/// Guilds per page in the search.
const PAGE: usize = 6;

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
    /// "open" (anyone joins) or "invite" (players ask, the leader accepts, or the leader invites).
    #[serde(default)]
    pub policy: String,
    /// Guild XP each member is asked to earn a week; 0 for none.
    #[serde(default)]
    pub min_week: i64,
}

impl Membership {
    pub fn invite_only(&self) -> bool {
        self.policy == "invite"
    }
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
    (
        "banner",
        "War Banner",
        "+5% damage against Titans and raids",
        6000,
    ),
    (
        "longship",
        "Longship",
        "+5% materials from expeditions",
        8000,
    ),
];

pub fn membership(c: &Connection, guild: u64, user: u64) -> Result<Option<Membership>> {
    Ok(c.query_row(
        "SELECT c.id, c.name, c.tag, c.leader, c.xp, c.treasury, c.upgrades, c.policy, c.min_week FROM rpg_clan_members m
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
                policy: r.get(7)?,
                min_week: r.get(8)?,
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
    /// Guild XP they earned last week.
    pub last_xp: i64,
    pub level: i64,
    pub joined: i64,
}

impl Member {
    pub fn label(&self) -> &str {
        if self.name.is_empty() {
            "Someone"
        } else {
            &self.name
        }
    }
    /// Short of the weekly minimum last week, counting only members who were in for all of it.
    pub fn under(&self, min: i64, week: i64) -> bool {
        min > 0 && self.joined < week_start(week - 1) + 86_400_000 && self.last_xp < min
    }
}

/// When a week (Monday 00:00 UTC) starts, in ms.
pub fn week_start(week: i64) -> i64 {
    (week * 7 - 3) * 86_400_000
}

pub fn members(c: &Connection, guild: u64, clan: i64, week: i64) -> Result<Vec<Member>> {
    let leader: i64 = c
        .query_row(
            "SELECT leader FROM rpg_clans WHERE id = ?1",
            params![clan],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let mut st = c.prepare_cached(
        "SELECT m.user_id, COALESCE(p.name, ''), CASE WHEN m.week = ?3 THEN m.week_xp ELSE 0 END, COALESCE(p.level, 1),
            CASE WHEN m.week = ?3 THEN m.last_xp WHEN m.week = ?3 - 1 THEN m.week_xp ELSE 0 END, m.joined
         FROM rpg_clan_members m LEFT JOIN rpg_players p ON p.guild_id = m.guild_id AND p.user_id = m.user_id
         WHERE m.guild_id = ?1 AND m.clan_id = ?2 ORDER BY 3 DESC, m.joined",
    )?;
    let rows = st
        .query_map(params![guild as i64, clan, week], |r| {
            let u = r.get::<_, i64>(0)?;
            Ok(Member {
                user: u as u64,
                name: r.get(1)?,
                leader: u == leader,
                week_xp: r.get(2)?,
                level: r.get(3)?,
                last_xp: r.get(4)?,
                joined: r.get(5)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

/// A guild in the search.
pub struct Listed {
    pub id: i64,
    pub name: String,
    pub tag: String,
    pub members: i64,
    pub level: u32,
    pub policy: String,
    pub min_week: i64,
}

impl Listed {
    /// "Open · no minimum", "Invite only · 250 guild XP a week".
    pub fn rules(&self) -> String {
        format!(
            "{} · {}",
            if self.policy == "invite" {
                "Invite only"
            } else {
                "Open"
            },
            if self.min_week > 0 {
                format!("{} guild XP a week", thousands(self.min_week))
            } else {
                "no minimum".into()
            }
        )
    }
}

/// Guilds, strongest first, matching a name or tag. Returns one page and whether more follow.
pub fn search(
    c: &Connection,
    guild: u64,
    query: &str,
    open_only: bool,
    page: usize,
) -> Result<(Vec<Listed>, bool)> {
    let mut st = c.prepare_cached(
        "SELECT c.id, c.name, c.tag, (SELECT COUNT(*) FROM rpg_clan_members m WHERE m.clan_id = c.id), c.xp, c.policy, c.min_week
         FROM rpg_clans c WHERE c.guild_id = ?1
           AND (?2 = '' OR instr(lower(c.name), lower(?2)) > 0 OR lower(c.tag) = lower(?2))
           AND (?3 = 0 OR c.policy = 'open')
         ORDER BY c.xp DESC, c.id LIMIT ?4 OFFSET ?5",
    )?;
    let mut rows = st
        .query_map(
            params![
                guild as i64,
                query,
                open_only as i64,
                PAGE as i64 + 1,
                (page * PAGE) as i64
            ],
            |r| {
                Ok(Listed {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    tag: r.get(2)?,
                    members: r.get(3)?,
                    level: level(r.get(4)?),
                    policy: r.get(5)?,
                    min_week: r.get(6)?,
                })
            },
        )?
        .collect::<Result<Vec<Listed>, _>>()?;
    let more = rows.len() > PAGE;
    rows.truncate(PAGE);
    Ok((rows, more))
}

fn listed(c: &Connection, guild: u64, clan: i64) -> Result<Option<Listed>> {
    Ok(c.query_row(
        "SELECT c.id, c.name, c.tag, (SELECT COUNT(*) FROM rpg_clan_members m WHERE m.clan_id = c.id), c.xp, c.policy, c.min_week
         FROM rpg_clans c WHERE c.id = ?1 AND c.guild_id = ?2",
        params![clan, guild as i64],
        |r| {
            Ok(Listed {
                id: r.get(0)?,
                name: r.get(1)?,
                tag: r.get(2)?,
                members: r.get(3)?,
                level: level(r.get(4)?),
                policy: r.get(5)?,
                min_week: r.get(6)?,
            })
        },
    )
    .optional()?)
}

/// A player's live asks, invites and kick cooldowns: (clan id, kind, at).
pub fn pending(c: &Connection, guild: u64, user: u64, now: i64) -> Result<Vec<(i64, String, i64)>> {
    let mut st = c.prepare_cached(
        "SELECT clan_id, kind, at FROM rpg_clan_invites WHERE guild_id = ?1 AND user_id = ?2
           AND at >= CASE kind WHEN 'kicked' THEN ?4 ELSE ?3 END ORDER BY at DESC",
    )?;
    let rows = st
        .query_map(
            params![guild as i64, user as i64, now - PENDING_MS, now - KICK_MS],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

/// Players asking to join a guild: (user, name, level, at).
pub fn asks(c: &Connection, clan: i64, now: i64) -> Result<Vec<(u64, String, i64, i64)>> {
    let mut st = c.prepare_cached(
        "SELECT i.user_id, COALESCE(p.name, ''), COALESCE(p.level, 1), i.at FROM rpg_clan_invites i
         LEFT JOIN rpg_players p ON p.guild_id = i.guild_id AND p.user_id = i.user_id
         WHERE i.clan_id = ?1 AND i.kind = 'ask' AND i.at >= ?2 ORDER BY i.at",
    )?;
    let rows = st
        .query_map(params![clan, now - PENDING_MS], |r| {
            Ok((r.get::<_, i64>(0)? as u64, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

/// Players seen in the last two weeks who aren't in a guild and haven't been invited by this
/// one yet, most recent first: (user, name, level).
fn free_players(
    c: &Connection,
    guild: u64,
    clan: i64,
    now: i64,
) -> Result<Vec<(u64, String, i64)>> {
    let mut st = c.prepare_cached(
        "SELECT p.user_id, p.name, p.level FROM rpg_players p
         WHERE p.guild_id = ?1 AND p.last_seen >= ?3
           AND NOT EXISTS (SELECT 1 FROM rpg_clan_members m WHERE m.guild_id = p.guild_id AND m.user_id = p.user_id)
           AND NOT EXISTS (SELECT 1 FROM rpg_clan_invites i WHERE i.clan_id = ?2 AND i.user_id = p.user_id
                             AND i.kind IN ('invite', 'ask') AND i.at >= ?4)
         ORDER BY p.last_seen DESC LIMIT 25",
    )?;
    let rows = st
        .query_map(
            params![guild as i64, clan, now - 2 * PENDING_MS, now - PENDING_MS],
            |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?, r.get(2)?)),
        )?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

fn count(c: &Connection, clan: i64) -> Result<i64> {
    Ok(c.query_row(
        "SELECT COUNT(*) FROM rpg_clan_members WHERE clan_id = ?1",
        params![clan],
        |r| r.get(0),
    )?)
}

/// Put a player in a guild and drop everything else they were waiting on.
fn add_member(g: &mut Game, clan: i64, user: u64) -> Result<()> {
    g.tx.execute(
        "INSERT INTO rpg_clan_members (guild_id, user_id, clan_id, role, joined) VALUES (?1, ?2, ?3, 'member', ?4)",
        params![g.p.guild as i64, user as i64, clan, g.p.now],
    )?;
    g.tx.execute(
        "DELETE FROM rpg_clan_invites WHERE guild_id = ?1 AND user_id = ?2 AND kind IN ('ask', 'invite')",
        params![g.p.guild as i64, user as i64],
    )?;
    Ok(())
}

fn in_a_guild(c: &Connection, guild: u64, user: u64) -> Result<bool> {
    Ok(c.query_row(
        "SELECT 1 FROM rpg_clan_members WHERE guild_id = ?1 AND user_id = ?2",
        params![guild as i64, user as i64],
        |_| Ok(true),
    )
    .optional()?
    .unwrap_or(false))
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
    let Some(m) = g.p.clan.clone() else {
        return Ok(());
    };
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
        "UPDATE rpg_clan_members SET
            last_xp = CASE WHEN week = ?3 THEN last_xp WHEN week = ?3 - 1 THEN week_xp ELSE 0 END,
            week_xp = CASE WHEN week = ?3 THEN week_xp + ?4 ELSE ?4 END, week = ?3
         WHERE guild_id = ?1 AND user_id = ?2",
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
    let taken: bool =
        g.tx.query_row(
            "SELECT 1 FROM rpg_clans WHERE guild_id = ?1 AND lower(name) = lower(?2)",
            params![g.p.guild as i64, name],
            |_| Ok(true),
        )
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

/// Join a guild: straight in if it's open, or with an invite from it.
pub fn join(g: &mut Game, clan: i64) -> Result<Option<String>> {
    if g.p.clan.is_some() {
        return Ok(Some("Leave your guild first.".into()));
    }
    let Some(c) = listed(&g.tx, g.p.guild, clan)? else {
        return Ok(Some("That guild is gone.".into()));
    };
    let mine = pending(&g.tx, g.p.guild, g.p.user, g.p.now)?;
    let invited = mine.iter().any(|x| x.0 == clan && x.1 == "invite");
    if !invited {
        if mine.iter().any(|x| x.0 == clan && x.1 == "kicked") {
            return Ok(Some(format!(
                "{} removed you lately. Try again in a few days.",
                c.name
            )));
        }
        if c.policy == "invite" {
            return Ok(Some(format!(
                "{} is invite only. Ask to join instead.",
                c.name
            )));
        }
    }
    if c.members >= MAX_MEMBERS {
        return Ok(Some(format!("{} is full.", c.name)));
    }
    let user = g.p.user;
    add_member(g, clan, user)?;
    g.p.clan = membership(&g.tx, g.p.guild, g.p.user)?;
    g.bump()?;
    Ok(Some(format!("You joined **{}**.", c.name)))
}

/// Ask an invite-only guild to let you in. An open guild just takes you.
pub fn ask(g: &mut Game, clan: i64) -> Result<Option<String>> {
    if g.p.clan.is_some() {
        return Ok(Some("Leave your guild first.".into()));
    }
    let Some(c) = listed(&g.tx, g.p.guild, clan)? else {
        return Ok(Some("That guild is gone.".into()));
    };
    let mine = pending(&g.tx, g.p.guild, g.p.user, g.p.now)?;
    if c.policy != "invite" || mine.iter().any(|x| x.0 == clan && x.1 == "invite") {
        return join(g, clan);
    }
    if mine.iter().any(|x| x.0 == clan && x.1 == "kicked") {
        return Ok(Some(format!(
            "{} removed you lately. Try again in a few days.",
            c.name
        )));
    }
    if mine.iter().any(|x| x.0 == clan && x.1 == "ask") {
        return Ok(Some(format!("You already asked {}.", c.name)));
    }
    if mine.iter().filter(|x| x.1 == "ask").count() as i64 >= MAX_ASKS {
        return Ok(Some(format!(
            "You can wait on {MAX_ASKS} guilds at once. Withdraw one first."
        )));
    }
    g.tx.execute(
        "INSERT OR REPLACE INTO rpg_clan_invites (guild_id, clan_id, user_id, kind, at) VALUES (?1, ?2, ?3, 'ask', ?4)",
        params![g.p.guild as i64, clan, g.p.user as i64, g.p.now],
    )?;
    g.bump()?;
    Ok(Some(format!(
        "You asked to join **{}**. Its leader sees it under Manage.",
        c.name
    )))
}

pub fn withdraw(g: &mut Game, clan: i64) -> Result<Option<String>> {
    g.tx.execute(
        "DELETE FROM rpg_clan_invites WHERE clan_id = ?1 AND user_id = ?2 AND kind = 'ask'",
        params![clan, g.p.user as i64],
    )?;
    g.bump()?;
    Ok(Some("Request withdrawn.".into()))
}

/// The leader lets someone in who asked.
pub fn accept(g: &mut Game, user: u64) -> Result<Option<String>> {
    let Some(m) = g.p.clan.clone().filter(|m| m.leader) else {
        return Ok(None);
    };
    let asked = asks(&g.tx, m.id, g.p.now)?;
    let Some((_, name, _, _)) = asked.iter().find(|x| x.0 == user) else {
        return Ok(Some("That request is gone.".into()));
    };
    let name = if name.is_empty() {
        "They".to_string()
    } else {
        name.clone()
    };
    if in_a_guild(&g.tx, g.p.guild, user)? {
        g.tx.execute(
            "DELETE FROM rpg_clan_invites WHERE clan_id = ?1 AND user_id = ?2",
            params![m.id, user as i64],
        )?;
        g.bump()?;
        return Ok(Some(format!("{name} joined another guild meanwhile.")));
    }
    if count(&g.tx, m.id)? >= MAX_MEMBERS {
        return Ok(Some("The guild is full.".into()));
    }
    add_member(g, m.id, user)?;
    g.bump()?;
    Ok(Some(format!("**{name}** is in the guild.")))
}

pub fn decline(g: &mut Game, user: u64) -> Result<Option<String>> {
    let Some(m) = g.p.clan.clone().filter(|m| m.leader) else {
        return Ok(None);
    };
    g.tx.execute(
        "DELETE FROM rpg_clan_invites WHERE clan_id = ?1 AND user_id = ?2 AND kind = 'ask'",
        params![m.id, user as i64],
    )?;
    g.bump()?;
    Ok(Some("Request declined.".into()))
}

/// The leader invites a player who isn't in a guild. It lasts a week.
pub fn invite(g: &mut Game, user: u64) -> Result<Option<String>> {
    let Some(m) = g.p.clan.clone().filter(|m| m.leader) else {
        return Ok(None);
    };
    let found: Option<(String, i64)> =
        g.tx.query_row(
            "SELECT name, level FROM rpg_players WHERE guild_id = ?1 AND user_id = ?2",
            params![g.p.guild as i64, user as i64],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((name, level)) = found else {
        return Ok(None);
    };
    let opens = crate::data::unlock(crate::data::Feature::Guild).level;
    if level < opens as i64 {
        return Ok(Some(format!("{name} can join a guild from level {opens}.")));
    }
    if in_a_guild(&g.tx, g.p.guild, user)? {
        return Ok(Some(format!("{name} is in a guild already.")));
    }
    // Someone who already asked is simply let in.
    if asks(&g.tx, m.id, g.p.now)?.iter().any(|x| x.0 == user) {
        return accept(g, user);
    }
    g.tx.execute(
        "INSERT OR REPLACE INTO rpg_clan_invites (guild_id, clan_id, user_id, kind, at) VALUES (?1, ?2, ?3, 'invite', ?4)",
        params![g.p.guild as i64, m.id, user as i64, g.p.now],
    )?;
    g.bump()?;
    Ok(Some(format!(
        "Invited **{name}**. They see it on their guild page for a week."
    )))
}

pub fn set_policy(g: &mut Game, policy: &str) -> Result<Option<String>> {
    let Some(m) = g.p.clan.clone().filter(|m| m.leader) else {
        return Ok(None);
    };
    let policy = if policy == "invite" { "invite" } else { "open" };
    g.tx.execute(
        "UPDATE rpg_clans SET policy = ?2 WHERE id = ?1",
        params![m.id, policy],
    )?;
    g.p.clan = membership(&g.tx, g.p.guild, g.p.user)?;
    g.bump()?;
    Ok(Some(
        if policy == "invite" {
            "The guild is invite only. Players ask, and you accept here."
        } else {
            "The guild is open: anyone can join while there's room."
        }
        .into(),
    ))
}

pub fn set_minimum(g: &mut Game, n: i64) -> Result<Option<String>> {
    let Some(m) = g.p.clan.clone().filter(|m| m.leader) else {
        return Ok(None);
    };
    let n = if MINIMUMS.contains(&n) { n } else { 0 };
    g.tx.execute(
        "UPDATE rpg_clans SET min_week = ?2 WHERE id = ?1",
        params![m.id, n],
    )?;
    g.p.clan = membership(&g.tx, g.p.guild, g.p.user)?;
    g.bump()?;
    Ok(Some(if n > 0 {
        format!(
            "Members are asked for {} guild XP a week. Who falls short shows here.",
            thousands(n)
        )
    } else {
        "No weekly minimum.".into()
    }))
}

pub fn leave(g: &mut Game) -> Result<Option<String>> {
    let Some(m) = g.p.clan.clone() else {
        return Ok(None);
    };
    g.tx.execute(
        "DELETE FROM rpg_clan_members WHERE guild_id = ?1 AND user_id = ?2",
        params![g.p.guild as i64, g.p.user as i64],
    )?;
    let next: Option<i64> =
        g.tx.query_row(
            "SELECT user_id FROM rpg_clan_members WHERE clan_id = ?1 ORDER BY joined LIMIT 1",
            params![m.id],
            |r| r.get(0),
        )
        .optional()?;
    match next {
        None => {
            g.tx.execute("DELETE FROM rpg_clans WHERE id = ?1", params![m.id])?;
            g.tx.execute(
                "DELETE FROM rpg_clan_invites WHERE clan_id = ?1",
                params![m.id],
            )?;
        }
        Some(u) if m.leader => {
            g.tx.execute(
                "UPDATE rpg_clans SET leader = ?2 WHERE id = ?1",
                params![m.id, u],
            )?;
            g.tx.execute(
                "UPDATE rpg_clan_members SET role = 'leader' WHERE clan_id = ?1 AND user_id = ?2",
                params![m.id, u],
            )?;
        }
        _ => {}
    }
    g.p.clan = None;
    g.bump()?;
    Ok(Some(format!("You left **{}**.", m.name)))
}

pub fn kick(g: &mut Game, user: u64) -> Result<Option<String>> {
    let Some(m) = g.p.clan.clone() else {
        return Ok(None);
    };
    if !m.leader || user == g.p.user {
        return Ok(None);
    }
    let gone = g.tx.execute(
        "DELETE FROM rpg_clan_members WHERE guild_id = ?1 AND user_id = ?2 AND clan_id = ?3",
        params![g.p.guild as i64, user as i64, m.id],
    )?;
    if gone == 0 {
        return Ok(Some("They already left.".into()));
    }
    // They can't walk straight back in or ask again for a few days.
    g.tx.execute(
        "INSERT OR REPLACE INTO rpg_clan_invites (guild_id, clan_id, user_id, kind, at) VALUES (?1, ?2, ?3, 'kicked', ?4)",
        params![g.p.guild as i64, m.id, user as i64, g.p.now],
    )?;
    g.bump()?;
    Ok(Some("They're no longer in the guild.".into()))
}

pub fn give(g: &mut Game, n: i64) -> Result<Option<String>> {
    let Some(m) = g.p.clan.clone() else {
        return Ok(None);
    };
    if g.p.gold < n {
        return Ok(Some("You don't have that much gold.".into()));
    }
    g.gold(-n, "guild treasury")?;
    g.tx.execute(
        "UPDATE rpg_clans SET treasury = treasury + ?2 WHERE id = ?1",
        params![m.id, n],
    )?;
    credit(g, n / 20, 0)?;
    if let Some(c) = g.p.clan.as_mut() {
        c.treasury += n;
    }
    g.bump()?;
    Ok(Some(format!(
        "{n} gold into the treasury. It only ever buys guild upgrades."
    )))
}

pub fn buy(g: &mut Game, id: &str) -> Result<Option<String>> {
    let Some(m) = g.p.clan.clone() else {
        return Ok(None);
    };
    let Some((_, name, _, cost)) = UPGRADES.iter().find(|u| u.0 == id) else {
        return Ok(None);
    };
    if !m.leader {
        return Ok(Some("Only the guild's leader buys upgrades.".into()));
    }
    if m.has(id) {
        return Ok(Some("The guild already has that.".into()));
    }
    if m.treasury < *cost {
        return Ok(Some(format!(
            "The treasury needs {} gold for the {name}.",
            thousands(*cost)
        )));
    }
    let mut ups = m.upgrades.clone();
    ups.push(id.to_owned());
    g.tx.execute(
        "UPDATE rpg_clans SET treasury = treasury - ?2, upgrades = ?3 WHERE id = ?1",
        params![m.id, cost, serde_json::to_string(&ups)?],
    )?;
    g.p.clan = membership(&g.tx, g.p.guild, g.p.user)?;
    g.bump()?;
    Ok(Some(format!("The guild built the **{name}**.")))
}

pub fn screen(g: &Game) -> Result<Screen> {
    let p = &g.p;
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    let Some(m) = &p.clan else {
        title(&mut s, "guild", "Guilds", "Band together");
        s.text(format!(
            "A guild grows from what its members do: hunts, expeditions, floors, dungeons. It levels up for small bonuses everyone shares, has a weekly challenge, and fights a raid boss together.\n-# Gold given to a guild only buys guild upgrades, never power for one member. Up to {MAX_MEMBERS} members."
        ));
        let mine = pending(&g.tx, p.guild, p.user, p.now)?;
        let mut invited = false;
        for (clan, _, _) in mine.iter().filter(|x| x.1 == "invite").take(3) {
            if let Some(c) = listed(&g.tx, p.guild, *clan)? {
                invited = true;
                s.section(
                    format!(
                        "{} **{} [{}]** invited you\n-# Level {} · {} of {MAX_MEMBERS} members · {}",
                        e("guild"),
                        c.name,
                        c.tag,
                        c.level,
                        c.members,
                        c.rules()
                    ),
                    Btn::new(ids.act("guild", "join", &c.id.to_string()), "Accept")
                        .emoji("guild")
                        .success()
                        .disabled(c.members >= MAX_MEMBERS),
                );
            }
        }
        let mut waiting = Vec::new();
        for (clan, _, _) in mine.iter().filter(|x| x.1 == "ask") {
            if let Some(c) = listed(&g.tx, p.guild, *clan)? {
                waiting.push(format!("{} [{}]", c.name, c.tag));
            }
        }
        if !waiting.is_empty() {
            s.text(format!("-# Waiting to hear from {}", waiting.join(", ")));
        }
        s.buttons(vec![
            back(&ids),
            Btn::new(ids.go("guildfind"), "Find a guild")
                .emoji("guild")
                .then_primary(!invited),
            Btn::new(
                ids.act("guild", "new", ""),
                format!("Found a guild · {FOUND_GOLD} gold"),
            )
            .emoji("gold")
            .disabled(p.gold < FOUND_GOLD),
        ]);
        return Ok(s);
    };
    let lvl = m.level();
    let week = p.week();
    title(
        &mut s,
        "guild",
        &format!("{} [{}]", m.name, m.tag),
        &format!(
            "Level {lvl}{}",
            if m.leader { " · you lead it" } else { "" }
        ),
    );
    let next = if lvl < 10 {
        format!(
            "{} / {} XP to level {}",
            thousands(m.xp),
            thousands(xp_for(lvl + 1)),
            lvl + 1
        )
    } else {
        "Highest level".into()
    };
    let perks = perks(lvl);
    // Level and what it gives everyone, on one line.
    s.text(format!(
        "{} {next}{}",
        e("guild"),
        if perks.is_empty() {
            String::new()
        } else {
            format!(" · {} for everyone", perks.join(", "))
        }
    ));
    let (done, target, claimed) = challenge(&g.tx, m.id, week)?;
    s.text(format!(
        "{} **Weekly challenge** · {} / {target} hunts{}",
        e("bounty"),
        done.min(target),
        if claimed {
            " · done"
        } else {
            "\n-# 800 treasury gold · 600 guild XP"
        }
    ));
    let raid = crate::shared::active(&g.tx, p.guild, "raid", m.id)?;
    match &raid {
        Some(r) => s.section(
            format!(
                "{} **Raid: {}**\n-# {}% left · ends in {}",
                e("boss"),
                data::monster(&r.boss).map(|x| x.name).unwrap_or("the wyrm"),
                r.hp * 100 / r.max_hp.max(1),
                crate::game::dur(r.ends - p.now)
            ),
            Btn::new(ids.go("raid"), "Raid").emoji("boss").primary(),
        ),
        None => s.section(
            format!(
                "{} **Raid: Fafnir**\n-# Three days, the whole guild · {} treasury gold to start",
                e("boss"),
                thousands(data::RAID_COST)
            ),
            Btn::new(ids.act("guild", "raid", ""), "Start the raid")
                .emoji("boss")
                .disabled(!m.leader || m.treasury < data::RAID_COST),
        ),
    };
    s.gap();
    s.text(format!(
        "{} **Treasury:** {} gold",
        e("gold"),
        thousands(m.treasury)
    ));
    let built: Vec<&str> = UPGRADES
        .iter()
        .filter(|u| m.has(u.0))
        .map(|u| u.1)
        .collect();
    if !built.is_empty() {
        s.text(format!("-# {} Built: {}", e("check"), built.join(", ")));
    }
    // Leaders see what's left, with costs, in the select below.
    if !m.leader {
        let left: Vec<String> = UPGRADES
            .iter()
            .filter(|u| !m.has(u.0))
            .map(|(_, name, what, cost)| format!("-# {name}: {what} · {} gold", thousands(*cost)))
            .collect();
        if !left.is_empty() {
            s.text(left.join("\n"));
        }
    }
    if m.leader {
        let opts: Vec<Opt> = UPGRADES
            .iter()
            .filter(|u| !m.has(u.0))
            .map(|(id, name, what, cost)| {
                Opt::new(format!("{name} · {} gold", thousands(*cost)), *id)
                    .emoji("guild")
                    .desc(*what)
            })
            .collect();
        s.inner_select(
            ids.act("guild", "buy", ""),
            "Build an upgrade from the treasury…",
            opts,
        );
    }
    let mem = members(&g.tx, p.guild, m.id, week)?;
    let list: Vec<String> = mem
        .iter()
        .take(12)
        .map(|x| {
            format!(
                "-# {}{} · level {} · {} XP this week",
                if x.leader { "👑 " } else { "" },
                if x.name.is_empty() {
                    "Someone"
                } else {
                    &x.name
                },
                x.level,
                thousands(x.week_xp)
            )
        })
        .collect();
    s.text(format!(
        "-# Members ({} of {MAX_MEMBERS})\n{}",
        mem.len(),
        list.join("\n")
    ));
    // Your own XP is already in the list; say it again only against the guild's minimum.
    if m.min_week > 0
        && let Some(me) = mem.iter().find(|x| x.user == p.user)
    {
        s.text(format!(
            "-# You: {} of the {} guild XP a week the guild asks for",
            thousands(me.week_xp),
            thousands(m.min_week)
        ));
    }
    let asking = if m.leader {
        asks(&g.tx, m.id, p.now)?.len()
    } else {
        0
    };
    if asking > 0 {
        s.text(format!(
            "{} **{asking} asking to join** · see Manage",
            e("guild")
        ));
    }
    s.buttons(vec![
        back(&ids),
        Btn::new(ids.act("guild", "give", "100"), "Give 100 gold")
            .emoji("gold")
            .disabled(p.gold < 100),
        Btn::new(ids.act("guild", "give", "1000"), "Give 1,000")
            .emoji("gold")
            .disabled(p.gold < 1000),
    ]);
    let mut row = vec![Btn::new(ids.go("guildfind"), "Other guilds").emoji("guild")];
    if m.leader {
        row.push(
            Btn::new(ids.go("guildmanage"), "Manage")
                .emoji("guild")
                .then_primary(asking > 0),
        );
    }
    row.push(Btn::new(ids.act("guild", "leave", ""), "Leave").danger());
    s.buttons(row);
    Ok(s)
}

/// The search arguments carried in a button: page, open-only, and the name being searched.
pub struct Find {
    pub page: usize,
    pub open_only: bool,
    pub query: String,
}

impl Find {
    pub fn parse(arg: &str) -> Find {
        let mut it = arg.splitn(3, '.');
        Find {
            page: it.next().and_then(|x| x.parse().ok()).unwrap_or(0).min(50),
            open_only: it.next() == Some("1"),
            query: clean(it.next().unwrap_or("")),
        }
    }
    pub fn arg(&self) -> String {
        format!("{}.{}.{}", self.page, self.open_only as u8, self.query)
    }
}

/// A search term that's safe to carry in a custom id.
pub fn clean(q: &str) -> String {
    q.chars()
        .filter(|c| *c != ':' && !c.is_control())
        .take(24)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Find a guild: the strongest first, or the ones matching a name or tag.
pub fn find_screen(g: &Game, f: &Find) -> Result<Screen> {
    let p = &g.p;
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let mut s = Screen::default();
    title(
        &mut s,
        "guild",
        "Find a guild",
        &match (f.query.is_empty(), f.open_only) {
            (true, false) => "Strongest first".to_string(),
            (true, true) => "Open guilds, strongest first".to_string(),
            (false, false) => format!("Matching “{}”", f.query),
            (false, true) => format!("Open guilds matching “{}”", f.query),
        },
    );
    let (rows, more) = search(&g.tx, p.guild, &f.query, f.open_only, f.page)?;
    let mine = pending(&g.tx, p.guild, p.user, p.now)?;
    let has = |clan: i64, kind: &str| mine.iter().any(|x| x.0 == clan && x.1 == kind);
    // Buttons that act on one guild carry it ahead of the search, so the page stays put.
    let here = |clan: i64| format!("{clan}.{}", f.arg());
    for c in &rows {
        let line = format!(
            "{} **{} [{}]** · level {}\n-# {} of {MAX_MEMBERS} members · {}",
            e("guild"),
            c.name,
            c.tag,
            c.level,
            c.members,
            c.rules()
        );
        let b = if p.clan.as_ref().is_some_and(|m| m.id == c.id) {
            Btn::new(ids.go("guild"), "Yours").emoji("guild")
        } else if p.clan.is_some() {
            Btn::new(ids.act("guild", "join", &c.id.to_string()), "Join").disabled(true)
        } else if has(c.id, "invite") {
            Btn::new(ids.act("guild", "join", &c.id.to_string()), "Accept invite")
                .success()
                .disabled(c.members >= MAX_MEMBERS)
        } else if c.members >= MAX_MEMBERS {
            Btn::new(ids.act("guild", "join", &c.id.to_string()), "Full").disabled(true)
        } else if has(c.id, "kicked") {
            Btn::new(ids.act("guild", "join", &c.id.to_string()), "Not yet").disabled(true)
        } else if c.policy == "invite" && has(c.id, "ask") {
            Btn::new(ids.act("guild", "withdraw", &here(c.id)), "Withdraw ask")
        } else if c.policy == "invite" {
            Btn::new(ids.act("guild", "ask", &here(c.id)), "Ask to join").emoji("guild")
        } else {
            Btn::new(ids.act("guild", "join", &c.id.to_string()), "Join")
                .emoji("guild")
                .primary()
        };
        s.section(line, b);
    }
    if rows.is_empty() {
        s.text(if f.query.is_empty() && !f.open_only && f.page == 0 {
            "No guilds yet. Found the first one.".to_string()
        } else if f.query.is_empty() {
            "No guilds here.".to_string()
        } else {
            format!("No guild matches “{}”.", f.query)
        });
    }
    let at = |page: usize, open_only: bool, query: &str| {
        Find {
            page,
            open_only,
            query: query.to_owned(),
        }
        .arg()
    };
    let mut row = vec![
        crate::game::back_to(&ids, "guild", "Guild"),
        Btn::new(ids.act("guild", "search", ""), "Search by name").emoji("guild"),
        Btn::new(
            ids.act("guild", "page", &at(0, !f.open_only, &f.query)),
            if f.open_only { "Show all" } else { "Open only" },
        ),
    ];
    if !f.query.is_empty() {
        row.push(Btn::new(
            ids.act("guild", "page", &at(0, f.open_only, "")),
            "Clear search",
        ));
    }
    s.buttons(row);
    if f.page > 0 || more {
        s.buttons(vec![
            Btn::new(
                ids.act(
                    "guild",
                    "page",
                    &at(f.page.saturating_sub(1), f.open_only, &f.query),
                ),
                "← Previous",
            )
            .disabled(f.page == 0),
            Btn::new(
                ids.act("guild", "page", &at(f.page + 1, f.open_only, &f.query)),
                "Next →",
            )
            .disabled(!more),
        ]);
    }
    Ok(s)
}

/// The leader's page: who can join, the weekly minimum, requests, invites and kicks.
pub fn manage_screen(g: &Game) -> Result<Screen> {
    let p = &g.p;
    let Some(m) = p.clan.as_ref().filter(|m| m.leader) else {
        return screen(g);
    };
    let ids = Ids {
        owner: p.user,
        version: p.version,
    };
    let week = p.week();
    let mut s = Screen::default();
    title(
        &mut s,
        "guild",
        &format!("Manage {} [{}]", m.name, m.tag),
        "Only the leader sees this",
    );
    s.inner_select(
        ids.act("guild", "policy", ""),
        "Who can join",
        vec![
            Opt::new("Open: anyone can join", "open")
                .emoji("guild")
                .desc("While there's room")
                .default(!m.invite_only()),
            Opt::new("Invite only", "invite")
                .emoji("lock")
                .desc("Players ask to join and you accept, or you invite them")
                .default(m.invite_only()),
        ],
    );
    s.inner_select(
        ids.act("guild", "minimum", ""),
        "Weekly minimum",
        MINIMUMS
            .iter()
            .map(|n| {
                if *n == 0 {
                    Opt::new("No weekly minimum", "0").default(m.min_week == 0)
                } else {
                    Opt::new(format!("{} guild XP a week", thousands(*n)), n.to_string())
                        .desc("Members short of it last week are marked")
                        .default(m.min_week == *n)
                }
            })
            .collect(),
    );
    let asked = asks(&g.tx, m.id, p.now)?;
    if !asked.is_empty() {
        s.text(format!("-# Asking to join ({})", asked.len()));
        for (user, name, lvl, at) in asked.iter().take(5) {
            s.section(
                format!(
                    "**{}** · level {lvl}\n-# Asked {} ago",
                    if name.is_empty() { "Someone" } else { name },
                    crate::game::dur(p.now - at)
                ),
                Btn::new(ids.act("guild", "accept", &user.to_string()), "Accept").success(),
            );
        }
        s.inner_select(
            ids.act("guild", "decline", ""),
            "Decline a request…",
            asked
                .iter()
                .map(|(user, name, lvl, _)| {
                    Opt::new(
                        format!("Decline {}", if name.is_empty() { "Someone" } else { name }),
                        user.to_string(),
                    )
                    .desc(format!("Level {lvl}"))
                })
                .collect(),
        );
    }
    let free = free_players(&g.tx, p.guild, m.id, p.now)?;
    s.inner_select(
        ids.act("guild", "invite", ""),
        "Invite a player…",
        free.iter()
            .map(|(user, name, lvl)| {
                Opt::new(
                    if name.is_empty() {
                        "Someone".to_string()
                    } else {
                        name.clone()
                    },
                    user.to_string(),
                )
                .desc(format!("Level {lvl} · not in a guild"))
            })
            .collect(),
    );
    let mem = members(&g.tx, p.guild, m.id, week)?;
    let lines: Vec<String> = mem
        .iter()
        .map(|x| {
            format!(
                "-# {}{} · level {} · {} this week · {} last week{}",
                if x.leader { "👑 " } else { "" },
                x.label(),
                x.level,
                thousands(x.week_xp),
                thousands(x.last_xp),
                if x.under(m.min_week, week) {
                    format!(" · {} under the minimum", e("warn"))
                } else {
                    String::new()
                }
            )
        })
        .collect();
    s.text(format!(
        "-# Guild XP by member{}\n{}",
        if m.min_week > 0 {
            format!(", {} asked a week", thousands(m.min_week))
        } else {
            String::new()
        },
        lines.join("\n")
    ));
    let mut kickable: Vec<&Member> = mem.iter().filter(|x| x.user != p.user).collect();
    kickable.sort_by_key(|x| (!x.under(m.min_week, week), x.last_xp));
    s.inner_select(
        ids.act("guild", "kick", ""),
        "Kick a member…",
        kickable
            .iter()
            .map(|x| {
                Opt::new(format!("Kick {}", x.label()), x.user.to_string()).desc(
                    if x.under(m.min_week, week) {
                        format!("Under the minimum last week ({})", thousands(x.last_xp))
                    } else {
                        format!(
                            "Level {} · {} guild XP this week",
                            x.level,
                            thousands(x.week_xp)
                        )
                    },
                )
            })
            .collect(),
    );
    s.text("-# A kicked player can't join or ask again for 3 days, unless you invite them.");
    s.buttons(vec![crate::game::back_to(&ids, "guild", "Guild")]);
    Ok(s)
}

/// XP bonus from the guild, in percent.
pub fn xp_bonus(p: &Player) -> i64 {
    p.clan
        .as_ref()
        .map(|c| if c.level() >= 2 { 2 } else { 0 })
        .unwrap_or(0)
}

pub fn loot_bonus(p: &Player) -> i64 {
    p.clan
        .as_ref()
        .map(|c| (if c.level() >= 4 { 3 } else { 0 }) + if c.has("longship") { 5 } else { 0 })
        .unwrap_or(0)
}

pub fn gold_bonus(p: &Player) -> i64 {
    p.clan
        .as_ref()
        .map(|c| if c.level() >= 8 { 5 } else { 0 })
        .unwrap_or(0)
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
