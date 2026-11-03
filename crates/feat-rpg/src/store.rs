//! The RPG's tables and the one way to change them: [`Game::run`] loads a player inside a
//! transaction, the action changes it through methods that log every gold and item movement,
//! and the transaction commits or nothing happens.

use std::collections::BTreeMap;

use bot_core::anyhow::{Result, anyhow};
use bot_core::rusqlite::{Connection, OptionalExtension, Transaction, params};
use bot_core::{Db, Migration, now_ms};

use crate::data::{self, God, Slot};
use crate::engine::combat::Fight;
use crate::engine::expedition::{Plan, Report};
use crate::engine::{Hero, progress};

pub const MIGRATIONS: &[Migration] = &[MIGRATION_001, MIGRATION_002, MIGRATION_003, Migration {
    name: "004_world",
    sql: "
        CREATE TABLE rpg_clans (
            id INTEGER PRIMARY KEY, guild_id INTEGER NOT NULL, name TEXT NOT NULL, tag TEXT NOT NULL,
            leader INTEGER NOT NULL, xp INTEGER NOT NULL DEFAULT 0, treasury INTEGER NOT NULL DEFAULT 0,
            upgrades TEXT NOT NULL DEFAULT '[]', created INTEGER NOT NULL,
            week INTEGER NOT NULL DEFAULT 0, week_wins INTEGER NOT NULL DEFAULT 0, week_claimed INTEGER NOT NULL DEFAULT 0);
        CREATE UNIQUE INDEX rpg_clans_name ON rpg_clans (guild_id, name);
        CREATE TABLE rpg_clan_members (
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, clan_id INTEGER NOT NULL, role TEXT NOT NULL,
            joined INTEGER NOT NULL, week INTEGER NOT NULL DEFAULT 0, week_xp INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (guild_id, user_id));
        CREATE INDEX rpg_clan_members_clan ON rpg_clan_members (clan_id);
        CREATE TABLE rpg_shared (
            id INTEGER PRIMARY KEY, guild_id INTEGER NOT NULL, kind TEXT NOT NULL, clan_id INTEGER NOT NULL DEFAULT 0,
            boss TEXT NOT NULL, power INTEGER NOT NULL, hp INTEGER NOT NULL, max_hp INTEGER NOT NULL,
            starts INTEGER NOT NULL, ends INTEGER NOT NULL, killed_at INTEGER, announced INTEGER NOT NULL DEFAULT 0);
        CREATE INDEX rpg_shared_guild ON rpg_shared (guild_id, kind, ends);
        CREATE TABLE rpg_shared_dmg (
            shared_id INTEGER NOT NULL, user_id INTEGER NOT NULL, damage INTEGER NOT NULL DEFAULT 0,
            attempts INTEGER NOT NULL DEFAULT 0, day INTEGER NOT NULL DEFAULT 0, day_attempts INTEGER NOT NULL DEFAULT 0,
            claimed INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (shared_id, user_id));
        CREATE TABLE rpg_druns (guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, state TEXT NOT NULL, PRIMARY KEY (guild_id, user_id));
        CREATE TABLE rpg_dclears (
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, dungeon TEXT NOT NULL, day INTEGER NOT NULL, clears INTEGER NOT NULL,
            PRIMARY KEY (guild_id, user_id, dungeon, day));
        CREATE TABLE rpg_side (
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, quest TEXT NOT NULL, step INTEGER NOT NULL DEFAULT 0,
            progress INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (guild_id, user_id, quest));
        CREATE TABLE rpg_bought (
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, day INTEGER NOT NULL, slot TEXT NOT NULL,
            PRIMARY KEY (guild_id, user_id, day, slot));
    ",
}, Migration {
    name: "005_posts",
    sql: "
        CREATE TABLE rpg_posts (
            channel_id INTEGER NOT NULL, message_id INTEGER NOT NULL, delete_at INTEGER NOT NULL,
            PRIMARY KEY (channel_id, message_id));
        CREATE INDEX rpg_posts_due ON rpg_posts (delete_at);
    ",
}, MIGRATION_006];

/// Guild controls: who may join, a weekly minimum, asks and invites, and a cooldown after a kick.
const MIGRATION_006: Migration = Migration {
    name: "006_guild_controls",
    sql: "
        ALTER TABLE rpg_clans ADD COLUMN policy TEXT NOT NULL DEFAULT 'open';
        ALTER TABLE rpg_clans ADD COLUMN min_week INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE rpg_clan_members ADD COLUMN last_xp INTEGER NOT NULL DEFAULT 0;
        CREATE TABLE rpg_clan_invites (
            guild_id INTEGER NOT NULL, clan_id INTEGER NOT NULL, user_id INTEGER NOT NULL,
            kind TEXT NOT NULL, at INTEGER NOT NULL, PRIMARY KEY (clan_id, user_id));
        CREATE INDEX rpg_clan_invites_user ON rpg_clan_invites (guild_id, user_id);
    ",
};

const MIGRATION_003: Migration = Migration {
    name: "003_gear_and_labyrinth",
    sql: "
        ALTER TABLE rpg_gear ADD COLUMN rarity INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE rpg_gear ADD COLUMN traits TEXT NOT NULL DEFAULT '[]';
        ALTER TABLE rpg_players ADD COLUMN tutorial INTEGER NOT NULL DEFAULT 0;
        CREATE TABLE rpg_runs (
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, state TEXT NOT NULL,
            PRIMARY KEY (guild_id, user_id));
        CREATE TABLE rpg_lab (
            guild_id INTEGER NOT NULL, week INTEGER NOT NULL, user_id INTEGER NOT NULL, best INTEGER NOT NULL,
            PRIMARY KEY (guild_id, week, user_id));
    ",
};

const MIGRATION_002: Migration = Migration {
    name: "002_saga",
    sql: "
        ALTER TABLE rpg_players ADD COLUMN title TEXT;
        ALTER TABLE rpg_players ADD COLUMN event TEXT;
        ALTER TABLE rpg_players ADD COLUMN saga INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE rpg_players ADD COLUMN saga_progress TEXT NOT NULL DEFAULT '[]';
        ALTER TABLE rpg_players ADD COLUMN saga_choices TEXT NOT NULL DEFAULT '{}';
        CREATE TABLE rpg_titles (
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, title TEXT NOT NULL, at INTEGER NOT NULL,
            PRIMARY KEY (guild_id, user_id, title));
        CREATE TABLE rpg_bounties (
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, day INTEGER NOT NULL, idx INTEGER NOT NULL,
            progress INTEGER NOT NULL DEFAULT 0, claimed INTEGER NOT NULL DEFAULT 0, level INTEGER NOT NULL,
            PRIMARY KEY (guild_id, user_id, day, idx));
    ",
};

const MIGRATION_001: Migration = Migration {
    name: "001_rpg",
    sql: "
        CREATE TABLE rpg_players (
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL,
            name TEXT NOT NULL DEFAULT '',
            patron TEXT, lesser TEXT,
            level INTEGER NOT NULL DEFAULT 1, xp INTEGER NOT NULL DEFAULT 0,
            gold INTEGER NOT NULL DEFAULT 0,
            hp INTEGER NOT NULL DEFAULT -1, hp_at INTEGER NOT NULL DEFAULT 0,
            vigor REAL NOT NULL DEFAULT 0, vigor_at INTEGER NOT NULL DEFAULT 0,
            version INTEGER NOT NULL DEFAULT 0,
            boss_won INTEGER NOT NULL DEFAULT 0,
            boss_reward_week INTEGER NOT NULL DEFAULT -1,
            hub_channel INTEGER, hub_message INTEGER,
            report TEXT,
            created_at INTEGER NOT NULL, last_seen INTEGER NOT NULL,
            PRIMARY KEY (guild_id, user_id));
        CREATE TABLE rpg_items (
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, item TEXT NOT NULL, qty INTEGER NOT NULL,
            PRIMARY KEY (guild_id, user_id, item));
        CREATE TABLE rpg_gear (
            id INTEGER PRIMARY KEY, guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL,
            gear TEXT NOT NULL, upgrade INTEGER NOT NULL DEFAULT 0, equipped INTEGER NOT NULL DEFAULT 0,
            made_at INTEGER NOT NULL);
        CREATE INDEX rpg_gear_owner ON rpg_gear (guild_id, user_id);
        CREATE TABLE rpg_bonds (
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, god TEXT NOT NULL, tribute INTEGER NOT NULL,
            PRIMARY KEY (guild_id, user_id, god));
        CREATE TABLE rpg_expeditions (
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, plan TEXT NOT NULL,
            power INTEGER NOT NULL, scouting INTEGER NOT NULL,
            started_at INTEGER NOT NULL, ends_at INTEGER NOT NULL, seed INTEGER NOT NULL,
            PRIMARY KEY (guild_id, user_id));
        CREATE TABLE rpg_fights (
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, state TEXT NOT NULL, started_at INTEGER NOT NULL,
            PRIMARY KEY (guild_id, user_id));
        CREATE TABLE rpg_intel (
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, week INTEGER NOT NULL, fact TEXT NOT NULL,
            PRIMARY KEY (guild_id, user_id, week, fact));
        CREATE TABLE rpg_pity (
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, region TEXT NOT NULL, hours INTEGER NOT NULL,
            PRIMARY KEY (guild_id, user_id, region));
        CREATE TABLE rpg_offerings (
            guild_id INTEGER NOT NULL, week INTEGER NOT NULL, god TEXT NOT NULL, tribute INTEGER NOT NULL,
            PRIMARY KEY (guild_id, week, god));
        CREATE TABLE rpg_givers (
            guild_id INTEGER NOT NULL, week INTEGER NOT NULL, user_id INTEGER NOT NULL, tribute INTEGER NOT NULL,
            PRIMARY KEY (guild_id, week, user_id));
        CREATE TABLE rpg_ledger (
            id INTEGER PRIMARY KEY, guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, at INTEGER NOT NULL,
            what TEXT NOT NULL, delta INTEGER NOT NULL, reason TEXT NOT NULL);
        CREATE INDEX rpg_ledger_guild_at ON rpg_ledger (guild_id, at);
        CREATE INDEX rpg_ledger_user ON rpg_ledger (guild_id, user_id, at);
        CREATE TABLE rpg_kills (
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, boss TEXT NOT NULL, at INTEGER NOT NULL, turns INTEGER NOT NULL,
            PRIMARY KEY (guild_id, user_id, boss));
    ",
};

#[derive(Debug, Clone)]
pub struct GearRow {
    pub id: i64,
    pub gear: &'static data::Gear,
    pub upgrade: i64,
    pub equipped: bool,
    pub rarity: data::Rarity,
    pub traits: crate::engine::loot::Traits,
}

impl GearRow {
    pub fn power(&self) -> i64 {
        let base = data::gear_power(self.gear, self.upgrade);
        base + base * self.rarity.power_pct() / 100
    }
    pub fn label(&self) -> String {
        if self.upgrade > 0 {
            format!("{} +{}", self.gear.name, self.upgrade)
        } else {
            self.gear.name.to_owned()
        }
    }
    pub fn traits_text(&self) -> String {
        self.traits
            .iter()
            .map(|(t, v)| format!("{}: {}", t.name(), t.describe(*v)))
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

#[derive(Debug, Clone)]
pub struct Expedition {
    pub plan: Plan,
    pub power: i64,
    pub scouting: i64,
    pub started_at: i64,
    pub ends_at: i64,
    pub seed: u64,
}

/// A player as loaded for one click.
#[derive(Debug, Clone)]
pub struct Player {
    pub guild: u64,
    pub user: u64,
    pub name: String,
    pub patron: Option<God>,
    pub lesser: Option<God>,
    pub level: u32,
    pub xp: i64,
    pub gold: i64,
    hp_stored: i64,
    hp_at: i64,
    vigor_stored: f64,
    vigor_at: i64,
    pub version: i64,
    pub boss_won: bool,
    pub boss_reward_week: i64,
    pub hub: Option<(u64, u64)>,
    pub report: Option<Report>,
    pub created_at: i64,
    pub items: BTreeMap<String, i64>,
    pub gear: Vec<GearRow>,
    /// Personal tribute per god, in [`data::GODS`] order.
    pub tribute: [i64; 3],
    pub expedition: Option<Expedition>,
    pub fight: Option<Fight>,
    /// Boss facts known now (the weakness only for this week).
    pub facts: Vec<String>,
    pub now: i64,
    pub title: Option<String>,
    pub titles: Vec<String>,
    /// A choice waiting after a hunt: a merchant, a shrine.
    pub event: Option<String>,
    pub saga: usize,
    pub saga_progress: Vec<i64>,
    pub saga_choices: BTreeMap<String, String>,
    /// Today's bounties: (progress, claimed) by index.
    pub bounty: [(i64, bool); 3],
    /// The level today's bounties were set at, once they've been seen.
    pub bounty_level: Option<u32>,
    /// Tutorial step; past the last step means done.
    pub tutorial: i64,
    /// A Labyrinth run in progress.
    pub run: Option<crate::labyrinth::Run>,
    /// A dungeon delve in progress.
    pub delve: Option<crate::dungeons::Delve>,
    pub clan: Option<crate::guild::Membership>,
    /// Side quests: (quest, step, progress).
    pub side: Vec<(String, usize, i64)>,
}

impl Player {
    pub fn main(&self) -> God {
        self.patron.unwrap_or(God::Thor)
    }
    pub fn have(&self, item: &str) -> i64 {
        self.items.get(item).copied().unwrap_or(0)
    }
    pub fn equipped(&self, slot: Slot) -> Option<&GearRow> {
        self.gear.iter().find(|g| g.equipped && g.gear.slot == slot)
    }
    pub fn power(&self) -> i64 {
        data::level_power(self.level)
            + data::SLOTS
                .iter()
                .filter_map(|s| self.equipped(*s))
                .map(GearRow::power)
                .sum::<i64>()
    }
    pub fn mods(&self) -> data::Mods {
        let mut m = data::Mods::default();
        for g in data::SLOTS.iter().filter_map(|s| self.equipped(*s)) {
            for (t, v) in &g.traits {
                m.add(*t, *v);
            }
        }
        m
    }
    pub fn scouting(&self) -> i64 {
        data::SLOTS
            .iter()
            .filter_map(|s| self.equipped(*s))
            .map(|g| g.gear.scouting)
            .sum::<i64>()
            + (self.mods().scouting * 100.0) as i64
    }
    pub fn bond(&self, g: God) -> u32 {
        progress::bond_rank(self.tribute[g as usize], data::BOND_CAP)
    }
    pub fn hero(&self) -> Hero {
        Hero {
            main: self.main(),
            lesser: self.lesser,
            bonds: [0, 1, 2].map(|i| progress::bond_rank(self.tribute[i], data::BOND_CAP)),
            power: self.power(),
            scouting: self.scouting(),
            mods: self.mods(),
        }
    }
    pub fn max_hp(&self) -> i64 {
        self.hero().stats().0
    }
    /// Game time since `at`, sped up by the server's test speed.
    fn fast(&self, at: i64) -> i64 {
        at + (self.now - at).max(0) * speed(self.guild)
    }
    pub fn hp(&self) -> i64 {
        if self.hp_stored < 0 {
            return self.max_hp();
        }
        progress::hp_now(
            self.hp_stored,
            self.hp_at,
            self.max_hp(),
            self.fast(self.hp_at),
        )
    }
    pub fn vigor(&self) -> f64 {
        let mead = if self
            .clan
            .as_ref()
            .is_some_and(|c| c.upgrades.iter().any(|u| u == "mead"))
        {
            1.1
        } else {
            1.0
        };
        let at = self.vigor_at;
        let fast = at + ((self.fast(at) - at) as f64 * mead) as i64;
        progress::vigor(self.vigor_stored, at, fast)
    }
    /// Real minutes until HP is full.
    pub fn minutes_to_full(&self) -> i64 {
        (progress::minutes_to_full(self.hp(), self.max_hp()) + speed(self.guild) - 1)
            / speed(self.guild)
    }
    pub fn week(&self) -> i64 {
        progress::week(self.now)
    }
    pub fn xp_needed(&self) -> i64 {
        data::xp_to_next(self.level)
    }
}

fn speeds() -> &'static std::sync::RwLock<std::collections::HashMap<u64, i64>> {
    static S: std::sync::OnceLock<std::sync::RwLock<std::collections::HashMap<u64, i64>>> =
        std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

/// The test speed for a server: expeditions, healing and Vigor run this many times faster.
pub fn set_speed(guild: u64, speed: i64) {
    speeds().write().unwrap().insert(guild, speed.clamp(1, 600));
}

/// Parts of the game a server can switch off.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(default)]
pub struct Modules {
    pub labyrinth: bool,
    pub dungeons: bool,
    pub guilds: bool,
    pub raids: bool,
    pub titans: bool,
    pub map: bool,
    pub saga: bool,
    pub quests: bool,
    pub bounties: bool,
    pub market: bool,
    pub encounters: bool,
    pub elites: bool,
}

impl Default for Modules {
    fn default() -> Self {
        Self {
            labyrinth: true,
            dungeons: true,
            guilds: true,
            raids: true,
            titans: true,
            map: true,
            saga: true,
            quests: true,
            bounties: true,
            market: true,
            encounters: true,
            elites: true,
        }
    }
}

impl Modules {
    /// (field, label, what it is) for the dashboard.
    pub const LIST: [(&'static str, &'static str, &'static str); 12] = [
        (
            "labyrinth",
            "Labyrinth",
            "Floor-by-floor runs, loose threads, the weekly board",
        ),
        (
            "dungeons",
            "Dungeons",
            "Fixed delves with a boss and a daily chest",
        ),
        (
            "guilds",
            "Guilds",
            "Player guilds, treasury, upgrades, weekly challenge",
        ),
        (
            "raids",
            "Guild raids",
            "A guild fights one boss together over a few days",
        ),
        (
            "titans",
            "Titans",
            "Server-wide boss events everyone chips away at",
        ),
        ("map", "Map", "The drawn map of Midgard and its places"),
        ("saga", "Saga", "The Loom story and its chapters"),
        (
            "quests",
            "Side quests",
            "Skagi's townsfolk and their quest lines",
        ),
        ("bounties", "Bounties", "Three small jobs a day"),
        (
            "market",
            "Market",
            "A daily stall of gear and supplies for gold",
        ),
        ("encounters", "Encounters", "Small choices after hunts"),
        ("elites", "Elites", "Rare named monsters in hunts"),
    ];
    pub fn get(&self, key: &str) -> bool {
        match key {
            "labyrinth" => self.labyrinth,
            "dungeons" => self.dungeons,
            "guilds" => self.guilds,
            "raids" => self.raids,
            "titans" => self.titans,
            "map" => self.map,
            "saga" => self.saga,
            "quests" => self.quests,
            "bounties" => self.bounties,
            "market" => self.market,
            "encounters" => self.encounters,
            "elites" => self.elites,
            _ => true,
        }
    }
    pub fn set(&mut self, key: &str, on: bool) {
        match key {
            "labyrinth" => self.labyrinth = on,
            "dungeons" => self.dungeons = on,
            "guilds" => self.guilds = on,
            "raids" => self.raids = on,
            "titans" => self.titans = on,
            "map" => self.map = on,
            "saga" => self.saga = on,
            "quests" => self.quests = on,
            "bounties" => self.bounties = on,
            "market" => self.market = on,
            "encounters" => self.encounters = on,
            "elites" => self.elites = on,
            _ => {}
        }
    }
}

fn module_map() -> &'static std::sync::RwLock<std::collections::HashMap<u64, Modules>> {
    static M: std::sync::OnceLock<std::sync::RwLock<std::collections::HashMap<u64, Modules>>> =
        std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

pub fn set_modules(guild: u64, m: Modules) {
    module_map().write().unwrap().insert(guild, m);
}

/// The server's switches (all on until settings are read).
pub fn modules(guild: u64) -> Modules {
    module_map()
        .read()
        .unwrap()
        .get(&guild)
        .copied()
        .unwrap_or_default()
}

pub fn speed(guild: u64) -> i64 {
    speeds().read().unwrap().get(&guild).copied().unwrap_or(1)
}

pub fn week_now() -> i64 {
    progress::week(now_ms())
}

pub(crate) fn load(c: &Connection, guild: u64, user: u64, now: i64) -> Result<Option<Player>> {
    let row = c
        .query_row(
            "SELECT name, patron, lesser, level, xp, gold, hp, hp_at, vigor, vigor_at, version, boss_won,
                    boss_reward_week, hub_channel, hub_message, report, created_at,
                    title, event, saga, saga_progress, saga_choices, tutorial
             FROM rpg_players WHERE guild_id = ?1 AND user_id = ?2",
            params![guild as i64, user as i64],
            |r| {
                Ok(Player {
                    guild,
                    user,
                    name: r.get(0)?,
                    patron: r.get::<_, Option<String>>(1)?.as_deref().and_then(God::from_id),
                    lesser: r.get::<_, Option<String>>(2)?.as_deref().and_then(God::from_id),
                    level: r.get::<_, i64>(3)? as u32,
                    xp: r.get(4)?,
                    gold: r.get(5)?,
                    hp_stored: r.get(6)?,
                    hp_at: r.get(7)?,
                    vigor_stored: r.get(8)?,
                    vigor_at: r.get(9)?,
                    version: r.get(10)?,
                    boss_won: r.get::<_, i64>(11)? != 0,
                    boss_reward_week: r.get(12)?,
                    hub: match (r.get::<_, Option<i64>>(13)?, r.get::<_, Option<i64>>(14)?) {
                        (Some(ch), Some(m)) => Some((ch as u64, m as u64)),
                        _ => None,
                    },
                    report: r.get::<_, Option<String>>(15)?.and_then(|s| serde_json::from_str(&s).ok()),
                    created_at: r.get(16)?,
                    items: BTreeMap::new(),
                    gear: Vec::new(),
                    tribute: [0; 3],
                    expedition: None,
                    fight: None,
                    facts: Vec::new(),
                    now,
                    title: r.get(17)?,
                    titles: Vec::new(),
                    event: r.get(18)?,
                    saga: r.get::<_, i64>(19)? as usize,
                    saga_progress: serde_json::from_str(&r.get::<_, String>(20)?).unwrap_or_default(),
                    saga_choices: serde_json::from_str(&r.get::<_, String>(21)?).unwrap_or_default(),
                    bounty: [(0, false); 3],
                    bounty_level: None,
                    tutorial: r.get(22)?,
                    run: None,
                    delve: None,
                    clan: None,
                    side: Vec::new(),
                })
            },
        )
        .optional()?;
    let Some(mut p) = row else { return Ok(None) };
    let (g, u) = (guild as i64, user as i64);
    let mut st = c.prepare_cached(
        "SELECT item, qty FROM rpg_items WHERE guild_id = ?1 AND user_id = ?2 AND qty != 0",
    )?;
    p.items = st
        .query_map(params![g, u], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let mut st = c.prepare_cached("SELECT id, gear, upgrade, equipped, rarity, traits FROM rpg_gear WHERE guild_id = ?1 AND user_id = ?2 ORDER BY id")?;
    p.gear = st
        .query_map(params![g, u], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)? != 0,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .filter_map(|(id, gear, upgrade, equipped, rarity, traits)| {
            data::gear(&gear).map(|gear| GearRow {
                id,
                gear,
                upgrade,
                equipped,
                rarity: data::Rarity::from_index(rarity),
                traits: serde_json::from_str(&traits).unwrap_or_default(),
            })
        })
        .collect();
    let mut st = c.prepare_cached(
        "SELECT god, tribute FROM rpg_bonds WHERE guild_id = ?1 AND user_id = ?2",
    )?;
    for row in st.query_map(params![g, u], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })? {
        let (god, t) = row?;
        if let Some(god) = God::from_id(&god) {
            p.tribute[god as usize] = t;
        }
    }
    p.expedition = c
        .query_row(
            "SELECT plan, power, scouting, started_at, ends_at, seed FROM rpg_expeditions WHERE guild_id = ?1 AND user_id = ?2",
            params![g, u],
            |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get::<_, i64>(5)?)),
        )
        .optional()?
        .and_then(|(plan, power, scouting, started_at, ends_at, seed)| {
            serde_json::from_str(&plan).ok().map(|plan| Expedition { plan, power, scouting, started_at, ends_at, seed: seed as u64 })
        });
    p.fight = c
        .query_row(
            "SELECT state FROM rpg_fights WHERE guild_id = ?1 AND user_id = ?2",
            params![g, u],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|s| serde_json::from_str(&s).ok());
    let week = progress::week(now);
    let mut st =
        c.prepare_cached("SELECT fact, week FROM rpg_intel WHERE guild_id = ?1 AND user_id = ?2")?;
    for row in st.query_map(params![g, u], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })? {
        let (fact, w) = row?;
        if (fact != "weakness" || w == week) && !p.facts.contains(&fact) {
            p.facts.push(fact);
        }
    }
    p.facts
        .sort_by_key(|f| data::FACTS.iter().position(|x| x == f));
    p.run = c
        .query_row(
            "SELECT state FROM rpg_runs WHERE guild_id = ?1 AND user_id = ?2",
            params![g, u],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|s| serde_json::from_str(&s).ok());
    p.delve = c
        .query_row(
            "SELECT state FROM rpg_druns WHERE guild_id = ?1 AND user_id = ?2",
            params![g, u],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|s| serde_json::from_str(&s).ok());
    p.clan = crate::guild::membership(c, guild, user)?;
    let mut st = c.prepare_cached(
        "SELECT quest, step, progress FROM rpg_side WHERE guild_id = ?1 AND user_id = ?2",
    )?;
    p.side = st
        .query_map(params![g, u], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)? as usize,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<Result<_, _>>()?;
    let mut st = c.prepare_cached(
        "SELECT title FROM rpg_titles WHERE guild_id = ?1 AND user_id = ?2 ORDER BY at",
    )?;
    p.titles = st
        .query_map(params![g, u], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut st = c.prepare_cached("SELECT idx, progress, claimed, level FROM rpg_bounties WHERE guild_id = ?1 AND user_id = ?2 AND day = ?3")?;
    for row in st.query_map(params![g, u, crate::quests::day(now)], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)? != 0,
            r.get::<_, i64>(3)?,
        ))
    })? {
        let (i, pr, cl, lvl) = row?;
        if let Some(b) = p.bounty.get_mut(i as usize) {
            *b = (pr, cl);
        }
        p.bounty_level = Some(lvl as u32);
    }
    Ok(Some(p))
}

/// One click's worth of changes to one player, inside a transaction.
pub struct Game<'a> {
    pub tx: Transaction<'a>,
    pub p: Player,
}

impl Game<'_> {
    fn ids(&self) -> (i64, i64) {
        (self.p.guild as i64, self.p.user as i64)
    }

    fn ledger(&self, what: &str, delta: i64, reason: &str) -> Result<()> {
        let (g, u) = self.ids();
        self.tx.execute(
            "INSERT INTO rpg_ledger (guild_id, user_id, at, what, delta, reason) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![g, u, self.p.now, what, delta, reason],
        )?;
        Ok(())
    }

    /// Move gold in or out. Refuses to go below zero.
    pub fn gold(&mut self, delta: i64, reason: &str) -> Result<()> {
        if delta == 0 {
            return Ok(());
        }
        if self.p.gold + delta < 0 {
            return Err(anyhow!("not enough gold"));
        }
        self.p.gold += delta;
        let (g, u) = self.ids();
        self.tx.execute(
            "UPDATE rpg_players SET gold = gold + ?3 WHERE guild_id = ?1 AND user_id = ?2",
            params![g, u, delta],
        )?;
        self.ledger("gold", delta, reason)
    }

    /// Move items in or out. Refuses to go below zero.
    pub fn item(&mut self, item: &str, delta: i64, reason: &str) -> Result<()> {
        if delta == 0 {
            return Ok(());
        }
        let have = self.p.have(item);
        if have + delta < 0 {
            return Err(anyhow!("not enough {item}"));
        }
        self.p.items.insert(item.to_owned(), have + delta);
        let (g, u) = self.ids();
        self.tx.execute(
            "INSERT INTO rpg_items (guild_id, user_id, item, qty) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (guild_id, user_id, item) DO UPDATE SET qty = qty + excluded.qty",
            params![g, u, item, delta],
        )?;
        self.ledger(item, delta, reason)
    }

    pub fn can_pay(&self, cost: &[(&str, i64)], gold: i64) -> bool {
        self.p.gold >= gold && cost.iter().all(|(id, n)| self.p.have(id) >= *n)
    }

    pub fn pay(&mut self, cost: &[(&str, i64)], gold: i64, reason: &str) -> Result<()> {
        for (id, n) in cost {
            self.item(id, -n, reason)?;
        }
        self.gold(-gold, reason)
    }

    /// Add XP; returns levels gained.
    pub fn xp(&mut self, gain: i64) -> Result<u32> {
        let (l, x, up) = progress::add_xp(self.p.level, self.p.xp, gain);
        self.p.level = l;
        self.p.xp = x;
        let (g, u) = self.ids();
        self.tx.execute(
            "UPDATE rpg_players SET level = ?3, xp = ?4 WHERE guild_id = ?1 AND user_id = ?2",
            params![g, u, l as i64, x],
        )?;
        Ok(up)
    }

    pub fn set_hp(&mut self, hp: i64) -> Result<()> {
        let max = self.p.max_hp();
        let hp = hp.clamp(0, max);
        self.p.hp_stored = hp;
        self.p.hp_at = self.p.now;
        let (g, u) = self.ids();
        self.tx.execute(
            "UPDATE rpg_players SET hp = ?3, hp_at = ?4 WHERE guild_id = ?1 AND user_id = ?2",
            params![g, u, hp, self.p.now],
        )?;
        Ok(())
    }

    /// Spend (negative) or add Vigor, in percent.
    pub fn vigor(&mut self, delta: f64) -> Result<()> {
        let v = (self.p.vigor() + delta).clamp(0.0, 100.0);
        self.p.vigor_stored = v;
        self.p.vigor_at = self.p.now;
        let (g, u) = self.ids();
        self.tx.execute(
            "UPDATE rpg_players SET vigor = ?3, vigor_at = ?4 WHERE guild_id = ?1 AND user_id = ?2",
            params![g, u, v, self.p.now],
        )?;
        Ok(())
    }

    /// Tribute to a god's altar: the server's weekly offering and this player's bond.
    pub fn tribute(&mut self, god: God, n: i64) -> Result<()> {
        if n <= 0 {
            return Ok(());
        }
        self.p.tribute[god as usize] += n;
        let (g, u) = self.ids();
        let week = self.p.week();
        self.tx.execute(
            "INSERT INTO rpg_bonds (guild_id, user_id, god, tribute) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (guild_id, user_id, god) DO UPDATE SET tribute = tribute + excluded.tribute",
            params![g, u, god.id(), n],
        )?;
        self.tx.execute(
            "INSERT INTO rpg_offerings (guild_id, week, god, tribute) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (guild_id, week, god) DO UPDATE SET tribute = tribute + excluded.tribute",
            params![g, week, god.id(), n],
        )?;
        self.tx.execute(
            "INSERT INTO rpg_givers (guild_id, week, user_id, tribute) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (guild_id, week, user_id) DO UPDATE SET tribute = tribute + excluded.tribute",
            params![g, week, u, n],
        )?;
        Ok(())
    }

    pub fn learn(&mut self, fact: &str) -> Result<()> {
        let (g, u) = self.ids();
        self.tx.execute(
            "INSERT OR IGNORE INTO rpg_intel (guild_id, user_id, week, fact) VALUES (?1, ?2, ?3, ?4)",
            params![g, u, self.p.week(), fact],
        )?;
        if !self.p.facts.iter().any(|f| f == fact) {
            self.p.facts.push(fact.to_owned());
        }
        Ok(())
    }

    pub fn set_patron(&mut self, god: God) -> Result<()> {
        self.p.patron = Some(god);
        let (g, u) = self.ids();
        self.tx.execute(
            "UPDATE rpg_players SET patron = ?3 WHERE guild_id = ?1 AND user_id = ?2",
            params![g, u, god.id()],
        )?;
        Ok(())
    }

    pub fn set_lesser(&mut self, god: Option<God>) -> Result<()> {
        self.p.lesser = god;
        let (g, u) = self.ids();
        self.tx.execute(
            "UPDATE rpg_players SET lesser = ?3 WHERE guild_id = ?1 AND user_id = ?2",
            params![g, u, god.map(|g| g.id())],
        )?;
        Ok(())
    }

    pub fn add_gear(&mut self, gear: &'static data::Gear, equip: bool) -> Result<i64> {
        self.add_piece(gear, data::Rarity::Common, Vec::new(), equip, "forged")
    }

    pub fn add_piece(
        &mut self,
        gear: &'static data::Gear,
        rarity: data::Rarity,
        traits: crate::engine::loot::Traits,
        equip: bool,
        why: &str,
    ) -> Result<i64> {
        let (g, u) = self.ids();
        if equip {
            self.unequip(gear.slot)?;
        }
        self.tx.execute(
            "INSERT INTO rpg_gear (guild_id, user_id, gear, upgrade, equipped, made_at, rarity, traits) VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, ?7)",
            params![g, u, gear.id, equip, self.p.now, rarity.index(), serde_json::to_string(&traits)?],
        )?;
        let id = self.tx.last_insert_rowid();
        self.p.gear.push(GearRow {
            id,
            gear,
            upgrade: 0,
            equipped: equip,
            rarity,
            traits,
        });
        self.ledger(gear.id, 1, why)?;
        Ok(id)
    }

    pub fn set_piece(
        &mut self,
        gear_id: i64,
        rarity: data::Rarity,
        traits: crate::engine::loot::Traits,
    ) -> Result<()> {
        self.tx.execute(
            "UPDATE rpg_gear SET rarity = ?2, traits = ?3 WHERE id = ?1",
            params![gear_id, rarity.index(), serde_json::to_string(&traits)?],
        )?;
        if let Some(r) = self.p.gear.iter_mut().find(|r| r.id == gear_id) {
            r.rarity = rarity;
            r.traits = traits;
        }
        Ok(())
    }

    pub fn set_tutorial(&mut self, step: i64) -> Result<()> {
        let (g, u) = self.ids();
        self.tx.execute(
            "UPDATE rpg_players SET tutorial = ?3 WHERE guild_id = ?1 AND user_id = ?2",
            params![g, u, step],
        )?;
        self.p.tutorial = step;
        Ok(())
    }

    pub fn save_delve(&mut self, d: Option<crate::dungeons::Delve>) -> Result<()> {
        let (g, u) = self.ids();
        match &d {
            Some(x) => {
                self.tx.execute(
                    "INSERT INTO rpg_druns (guild_id, user_id, state) VALUES (?1, ?2, ?3) ON CONFLICT (guild_id, user_id) DO UPDATE SET state = excluded.state",
                    params![g, u, serde_json::to_string(x)?],
                )?;
            }
            None => {
                self.tx.execute(
                    "DELETE FROM rpg_druns WHERE guild_id = ?1 AND user_id = ?2",
                    params![g, u],
                )?;
            }
        }
        self.p.delve = d;
        Ok(())
    }

    pub fn save_side(&mut self, quest: &str, step: usize, progress: i64) -> Result<()> {
        let (g, u) = self.ids();
        self.tx.execute(
            "INSERT INTO rpg_side (guild_id, user_id, quest, step, progress) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (guild_id, user_id, quest) DO UPDATE SET step = excluded.step, progress = excluded.progress",
            params![g, u, quest, step as i64, progress],
        )?;
        match self.p.side.iter_mut().find(|x| x.0 == quest) {
            Some(x) => {
                x.1 = step;
                x.2 = progress;
            }
            None => self.p.side.push((quest.to_owned(), step, progress)),
        }
        Ok(())
    }

    pub fn save_run(&mut self, run: Option<crate::labyrinth::Run>) -> Result<()> {
        let (g, u) = self.ids();
        match &run {
            Some(r) => {
                self.tx.execute(
                    "INSERT INTO rpg_runs (guild_id, user_id, state) VALUES (?1, ?2, ?3) ON CONFLICT (guild_id, user_id) DO UPDATE SET state = excluded.state",
                    params![g, u, serde_json::to_string(r)?],
                )?;
            }
            None => {
                self.tx.execute(
                    "DELETE FROM rpg_runs WHERE guild_id = ?1 AND user_id = ?2",
                    params![g, u],
                )?;
            }
        }
        self.p.run = run;
        Ok(())
    }

    /// Record a floor reached this week; true if it's the player's best.
    pub fn lab_best(&mut self, floor: u32) -> Result<bool> {
        let (g, u) = self.ids();
        let week = self.p.week();
        let old: i64 = self
            .tx
            .query_row(
                "SELECT best FROM rpg_lab WHERE guild_id = ?1 AND week = ?2 AND user_id = ?3",
                params![g, week, u],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if floor as i64 > old {
            self.tx.execute(
                "INSERT INTO rpg_lab (guild_id, week, user_id, best) VALUES (?1, ?2, ?3, ?4) ON CONFLICT (guild_id, week, user_id) DO UPDATE SET best = excluded.best",
                params![g, week, u, floor as i64],
            )?;
            return Ok(true);
        }
        Ok(false)
    }

    fn unequip(&mut self, slot: Slot) -> Result<()> {
        let ids: Vec<i64> = self
            .p
            .gear
            .iter()
            .filter(|r| r.equipped && r.gear.slot == slot)
            .map(|r| r.id)
            .collect();
        for id in ids {
            self.tx.execute(
                "UPDATE rpg_gear SET equipped = 0 WHERE id = ?1",
                params![id],
            )?;
            if let Some(r) = self.p.gear.iter_mut().find(|r| r.id == id) {
                r.equipped = false;
            }
        }
        Ok(())
    }

    pub fn equip(&mut self, gear_id: i64) -> Result<()> {
        let Some(slot) = self
            .p
            .gear
            .iter()
            .find(|r| r.id == gear_id)
            .map(|r| r.gear.slot)
        else {
            return Err(anyhow!("no such gear"));
        };
        self.unequip(slot)?;
        self.tx.execute(
            "UPDATE rpg_gear SET equipped = 1 WHERE id = ?1",
            params![gear_id],
        )?;
        if let Some(r) = self.p.gear.iter_mut().find(|r| r.id == gear_id) {
            r.equipped = true;
        }
        Ok(())
    }

    pub fn upgrade(&mut self, gear_id: i64) -> Result<()> {
        self.tx.execute(
            "UPDATE rpg_gear SET upgrade = upgrade + 1 WHERE id = ?1",
            params![gear_id],
        )?;
        if let Some(r) = self.p.gear.iter_mut().find(|r| r.id == gear_id) {
            r.upgrade += 1;
        }
        Ok(())
    }

    pub fn remove_gear(&mut self, gear_id: i64) -> Result<()> {
        self.tx
            .execute("DELETE FROM rpg_gear WHERE id = ?1", params![gear_id])?;
        if let Some(r) = self.p.gear.iter().find(|r| r.id == gear_id) {
            self.ledger(r.gear.id, -1, "salvaged")?;
        }
        self.p.gear.retain(|r| r.id != gear_id);
        Ok(())
    }

    pub fn start_expedition(&mut self, e: Expedition) -> Result<()> {
        let (g, u) = self.ids();
        self.tx.execute(
            "INSERT OR REPLACE INTO rpg_expeditions (guild_id, user_id, plan, power, scouting, started_at, ends_at, seed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![g, u, serde_json::to_string(&e.plan)?, e.power, e.scouting, e.started_at, e.ends_at, e.seed as i64],
        )?;
        self.p.expedition = Some(e);
        Ok(())
    }

    pub fn end_expedition(&mut self) -> Result<()> {
        let (g, u) = self.ids();
        self.tx.execute(
            "DELETE FROM rpg_expeditions WHERE guild_id = ?1 AND user_id = ?2",
            params![g, u],
        )?;
        self.p.expedition = None;
        Ok(())
    }

    pub fn pity(&self, region: &str) -> Result<u32> {
        let (g, u) = self.ids();
        Ok(self
            .tx
            .query_row(
                "SELECT hours FROM rpg_pity WHERE guild_id = ?1 AND user_id = ?2 AND region = ?3",
                params![g, u, region],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0) as u32)
    }

    pub fn set_pity(&self, region: &str, hours: u32) -> Result<()> {
        let (g, u) = self.ids();
        self.tx.execute(
            "INSERT OR REPLACE INTO rpg_pity (guild_id, user_id, region, hours) VALUES (?1, ?2, ?3, ?4)",
            params![g, u, region, hours as i64],
        )?;
        Ok(())
    }

    pub fn set_report(&mut self, r: Option<Report>) -> Result<()> {
        let (g, u) = self.ids();
        let json = r.as_ref().map(serde_json::to_string).transpose()?;
        self.tx.execute(
            "UPDATE rpg_players SET report = ?3 WHERE guild_id = ?1 AND user_id = ?2",
            params![g, u, json],
        )?;
        self.p.report = r;
        Ok(())
    }

    pub fn save_fight(&mut self, f: Option<Fight>) -> Result<()> {
        let (g, u) = self.ids();
        match &f {
            Some(state) => {
                self.tx.execute(
                    "INSERT INTO rpg_fights (guild_id, user_id, state, started_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (guild_id, user_id) DO UPDATE SET state = excluded.state",
                    params![g, u, serde_json::to_string(state)?, self.p.now],
                )?;
            }
            None => {
                self.tx.execute(
                    "DELETE FROM rpg_fights WHERE guild_id = ?1 AND user_id = ?2",
                    params![g, u],
                )?;
            }
        }
        self.p.fight = f;
        Ok(())
    }

    pub fn boss_won(&mut self, turns: u32) -> Result<bool> {
        let (g, u) = self.ids();
        let first = !self.p.boss_won;
        self.p.boss_won = true;
        self.p.boss_reward_week = self.p.week();
        self.tx.execute(
            "UPDATE rpg_players SET boss_won = 1, boss_reward_week = ?3 WHERE guild_id = ?1 AND user_id = ?2",
            params![g, u, self.p.boss_reward_week],
        )?;
        self.tx.execute(
            "INSERT OR IGNORE INTO rpg_kills (guild_id, user_id, boss, at, turns) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![g, u, data::BOSS_ID, self.p.now, turns as i64],
        )?;
        Ok(first)
    }

    pub fn add_title(&mut self, title: &str) -> Result<bool> {
        if self.p.titles.iter().any(|t| t == title) {
            return Ok(false);
        }
        let (g, u) = self.ids();
        self.tx.execute("INSERT OR IGNORE INTO rpg_titles (guild_id, user_id, title, at) VALUES (?1, ?2, ?3, ?4)", params![g, u, title, self.p.now])?;
        self.p.titles.push(title.to_owned());
        if self.p.title.is_none() {
            self.set_title(Some(title))?;
        }
        Ok(true)
    }

    pub fn set_title(&mut self, title: Option<&str>) -> Result<()> {
        let (g, u) = self.ids();
        self.tx.execute(
            "UPDATE rpg_players SET title = ?3 WHERE guild_id = ?1 AND user_id = ?2",
            params![g, u, title],
        )?;
        self.p.title = title.map(str::to_owned);
        Ok(())
    }

    pub fn set_event(&mut self, event: Option<&str>) -> Result<()> {
        let (g, u) = self.ids();
        self.tx.execute(
            "UPDATE rpg_players SET event = ?3 WHERE guild_id = ?1 AND user_id = ?2",
            params![g, u, event],
        )?;
        self.p.event = event.map(str::to_owned);
        Ok(())
    }

    pub fn save_saga(&mut self) -> Result<()> {
        let (g, u) = self.ids();
        self.tx.execute(
            "UPDATE rpg_players SET saga = ?3, saga_progress = ?4, saga_choices = ?5 WHERE guild_id = ?1 AND user_id = ?2",
            params![g, u, self.p.saga as i64, serde_json::to_string(&self.p.saga_progress)?, serde_json::to_string(&self.p.saga_choices)?],
        )?;
        Ok(())
    }

    /// Store today's bounty state, fixing the day's level the first time.
    pub fn save_bounty(&mut self, idx: usize) -> Result<()> {
        let (g, u) = self.ids();
        let level = *self.p.bounty_level.get_or_insert(self.p.level);
        for i in 0..3 {
            if i != idx {
                self.tx.execute(
                    "INSERT OR IGNORE INTO rpg_bounties (guild_id, user_id, day, idx, progress, claimed, level) VALUES (?1, ?2, ?3, ?4, 0, 0, ?5)",
                    params![g, u, crate::quests::day(self.p.now), i as i64, level as i64],
                )?;
            }
        }
        let (pr, cl) = self.p.bounty[idx];
        self.tx.execute(
            "INSERT INTO rpg_bounties (guild_id, user_id, day, idx, progress, claimed, level) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (guild_id, user_id, day, idx) DO UPDATE SET progress = excluded.progress, claimed = excluded.claimed",
            params![g, u, crate::quests::day(self.p.now), idx as i64, pr, cl, level as i64],
        )?;
        Ok(())
    }

    pub fn record_kill(&mut self, id: &str, turns: u32) -> Result<bool> {
        let (g, u) = self.ids();
        Ok(self.tx.execute(
            "INSERT OR IGNORE INTO rpg_kills (guild_id, user_id, boss, at, turns) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![g, u, id, self.p.now, turns as i64],
        )? == 1)
    }

    /// Mark a state change, so buttons from before it are refused.
    pub fn bump(&mut self) -> Result<()> {
        self.p.version += 1;
        let (g, u) = self.ids();
        self.tx.execute(
            "UPDATE rpg_players SET version = version + 1 WHERE guild_id = ?1 AND user_id = ?2",
            params![g, u],
        )?;
        Ok(())
    }
}

/// Run `f` on a player inside one transaction. `None` when they have no character yet.
pub fn run<T>(
    db: &Db,
    guild: u64,
    user: u64,
    name: Option<&str>,
    f: impl FnOnce(&mut Game) -> Result<T>,
) -> Result<Option<T>> {
    let mut conn = db.lock();
    let now = now_ms();
    let tx = conn.transaction()?;
    let Some(p) = load(&tx, guild, user, now)? else {
        return Ok(None);
    };
    let mut game = Game { tx, p };
    if let Some(n) = name
        && n != game.p.name
    {
        game.tx.execute(
            "UPDATE rpg_players SET name = ?3 WHERE guild_id = ?1 AND user_id = ?2",
            params![guild as i64, user as i64, n],
        )?;
        game.p.name = n.to_owned();
    }
    game.tx.execute(
        "UPDATE rpg_players SET last_seen = ?3 WHERE guild_id = ?1 AND user_id = ?2",
        params![guild as i64, user as i64, now],
    )?;
    let out = f(&mut game)?;
    game.tx.commit()?;
    Ok(Some(out))
}

/// Run `f` on a player and throw the changes away: for previews.
pub fn peek<T>(
    db: &Db,
    guild: u64,
    user: u64,
    f: impl FnOnce(&mut Game) -> Result<T>,
) -> Result<Option<T>> {
    let mut conn = db.lock();
    let tx = conn.transaction()?;
    let Some(p) = load(&tx, guild, user, now_ms())? else {
        return Ok(None);
    };
    let mut game = Game { tx, p };
    let out = f(&mut game)?;
    // Dropping the transaction rolls it back.
    Ok(Some(out))
}

/// Read a player without changing anything.
pub fn get(db: &Db, guild: u64, user: u64) -> Result<Option<Player>> {
    let conn = db.lock();
    load(&conn, guild, user, now_ms())
}

/// Create a character with the patron's gifts: the starter gear, a little gold, two potions.
pub fn create(db: &Db, guild: u64, user: u64, name: &str, patron: God) -> Result<()> {
    let mut conn = db.lock();
    let now = now_ms();
    let tx = conn.transaction()?;
    let (g, u) = (guild as i64, user as i64);
    let fresh = tx.execute(
        "INSERT OR IGNORE INTO rpg_players (guild_id, user_id, name, patron, gold, vigor, vigor_at, created_at, last_seen)
         VALUES (?1, ?2, ?3, ?4, 0, 50, ?5, ?5, ?5)",
        params![g, u, name, patron.id(), now],
    )? == 1;
    if fresh {
        let p = load(&tx, guild, user, now)?.ok_or_else(|| anyhow!("player vanished"))?;
        let mut game = Game { tx, p };
        for id in data::STARTER_GEAR {
            if let Some(gear) = data::gear(id) {
                game.add_gear(gear, true)?;
            }
        }
        game.gold(60, "patron's gift")?;
        game.item("potion", 2, "patron's gift")?;
        game.item("rations", 2, "patron's gift")?;
        game.tx.commit()?;
    } else {
        tx.commit()?;
    }
    Ok(())
}

pub fn set_hub(db: &Db, guild: u64, user: u64, channel: u64, message: u64) -> Result<()> {
    db.with(|c| {
        c.execute(
            "UPDATE rpg_players SET hub_channel = ?3, hub_message = ?4 WHERE guild_id = ?1 AND user_id = ?2",
            params![guild as i64, user as i64, channel as i64, message as i64],
        )
    })?;
    Ok(())
}

// ---- server-wide reads ----

/// This week's offering per god, in [`data::GODS`] order.
pub fn offerings(c: &Connection, guild: u64, week: i64) -> Result<[i64; 3]> {
    let mut out = [0; 3];
    let mut st = c.prepare_cached(
        "SELECT god, tribute FROM rpg_offerings WHERE guild_id = ?1 AND week = ?2",
    )?;
    for row in st.query_map(params![guild as i64, week], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })? {
        let (god, t) = row?;
        if let Some(g) = God::from_id(&god) {
            out[g as usize] = t;
        }
    }
    Ok(out)
}

/// Players seen since the start of `week`.
pub fn active_players(c: &Connection, guild: u64, week: i64) -> Result<i64> {
    Ok(c.query_row(
        "SELECT COUNT(*) FROM rpg_players WHERE guild_id = ?1 AND last_seen >= ?2",
        params![guild as i64, progress::week_start(week)],
        |r| r.get(0),
    )?)
}

pub fn goal(c: &Connection, guild: u64, week: i64) -> Result<i64> {
    Ok(data::GOAL_PER_PLAYER * active_players(c, guild, week)?.max(data::GOAL_MIN_PLAYERS))
}

/// The blessing active this week: last week's offering met its goal, and the god who got the
/// most tribute grants it.
pub fn blessing(c: &Connection, guild: u64, week: i64) -> Result<Option<God>> {
    let last = offerings(c, guild, week - 1)?;
    let total: i64 = last.iter().sum();
    let goal =
        data::GOAL_PER_PLAYER * active_players_in(c, guild, week - 1)?.max(data::GOAL_MIN_PLAYERS);
    if total < goal || total == 0 {
        return Ok(None);
    }
    let (i, _) = last
        .iter()
        .enumerate()
        .max_by_key(|(_, t)| **t)
        .unwrap_or((0, &0));
    Ok(Some(data::GODS[i]))
}

/// Players who gave anything in `week`, or were seen in it: the goal's size for that week.
fn active_players_in(c: &Connection, guild: u64, week: i64) -> Result<i64> {
    Ok(c.query_row(
        "SELECT COUNT(*) FROM rpg_givers WHERE guild_id = ?1 AND week = ?2",
        params![guild as i64, week],
        |r| r.get(0),
    )?)
}

/// Top givers this week: (user, name, tribute).
pub fn top_givers(
    c: &Connection,
    guild: u64,
    week: i64,
    limit: i64,
) -> Result<Vec<(u64, String, i64)>> {
    let mut st = c.prepare_cached(
        "SELECT g.user_id, COALESCE(p.name, ''), g.tribute FROM rpg_givers g
         LEFT JOIN rpg_players p ON p.guild_id = g.guild_id AND p.user_id = g.user_id
         WHERE g.guild_id = ?1 AND g.week = ?2 ORDER BY g.tribute DESC LIMIT ?3",
    )?;
    let rows = st
        .query_map(params![guild as i64, week, limit], |r| {
            Ok((r.get::<_, i64>(0)? as u64, r.get(1)?, r.get(2)?))
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

/// This week's deepest Labyrinth floors: (user, name, floor).
pub fn lab_board(
    c: &Connection,
    guild: u64,
    week: i64,
    limit: i64,
) -> Result<Vec<(u64, String, i64)>> {
    let mut st = c.prepare_cached(
        "SELECT l.user_id, COALESCE(p.name, ''), l.best FROM rpg_lab l
         LEFT JOIN rpg_players p ON p.guild_id = l.guild_id AND p.user_id = l.user_id
         WHERE l.guild_id = ?1 AND l.week = ?2 ORDER BY l.best DESC LIMIT ?3",
    )?;
    let rows = st
        .query_map(params![guild as i64, week, limit], |r| {
            Ok((r.get::<_, i64>(0)? as u64, r.get(1)?, r.get(2)?))
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

/// Remember a public line the bot posted, so the tick deletes it at `delete_at`.
pub fn delete_later(db: &Db, channel: u64, message: u64, delete_at: i64) -> Result<()> {
    db.with(|c| {
        c.execute(
            "INSERT OR REPLACE INTO rpg_posts (channel_id, message_id, delete_at) VALUES (?1, ?2, ?3)",
            params![channel as i64, message as i64, delete_at],
        )
    })?;
    Ok(())
}

/// Public lines whose time is up, as (channel, message), at most `limit`. They leave the list
/// here: a message someone already deleted by hand shouldn't be retried forever.
pub fn take_due_posts(db: &Db, now: i64, limit: usize) -> Result<Vec<(u64, u64)>> {
    let mut conn = db.lock();
    let tx = conn.transaction()?;
    let rows: Vec<(i64, i64)> = {
        let mut st = tx.prepare_cached(
            "SELECT channel_id, message_id FROM rpg_posts WHERE delete_at <= ?1 ORDER BY delete_at LIMIT ?2",
        )?;
        st.query_map(params![now, limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?
    };
    for (ch, id) in &rows {
        tx.execute(
            "DELETE FROM rpg_posts WHERE channel_id = ?1 AND message_id = ?2",
            params![ch, id],
        )?;
    }
    tx.commit()?;
    Ok(rows
        .into_iter()
        .map(|(c, m)| (c as u64, m as u64))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posts_come_due_once() {
        let db = db();
        delete_later(&db, 10, 1, 1_000).unwrap();
        delete_later(&db, 10, 2, 5_000).unwrap();
        assert!(take_due_posts(&db, 999, 50).unwrap().is_empty());
        assert_eq!(take_due_posts(&db, 1_000, 50).unwrap(), vec![(10, 1)]);
        assert!(take_due_posts(&db, 1_000, 50).unwrap().is_empty());
        assert_eq!(take_due_posts(&db, 9_000, 50).unwrap(), vec![(10, 2)]);
    }

    fn db() -> Db {
        let db = Db::memory().unwrap();
        db.migrate("rpg", MIGRATIONS).unwrap();
        db
    }

    #[test]
    fn create_gives_the_starter_kit_and_logs_it() {
        let db = db();
        create(&db, 1, 2, "Antho", God::Thor).unwrap();
        create(&db, 1, 2, "Antho", God::Hades).unwrap(); // second call changes nothing
        let p = get(&db, 1, 2).unwrap().unwrap();
        assert_eq!(p.patron, Some(God::Thor));
        assert_eq!(p.gold, 60);
        assert_eq!(p.have("potion"), 2);
        assert!(p.equipped(Slot::Weapon).is_some() && p.equipped(Slot::Armour).is_some());
        assert_eq!(p.power(), 10 + 20 + 15);
        let rows: i64 = db
            .with(|c| c.query_row("SELECT COUNT(*) FROM rpg_ledger", [], |r| r.get(0)))
            .unwrap();
        assert_eq!(rows, 5);
    }

    #[test]
    fn a_failed_action_changes_nothing() {
        let db = db();
        create(&db, 1, 2, "Antho", God::Athena).unwrap();
        let r = run(&db, 1, 2, None, |g| {
            g.gold(-50, "test")?;
            g.gold(-50, "test")?; // only 10 left: fails
            Ok(())
        });
        assert!(r.is_err());
        assert_eq!(get(&db, 1, 2).unwrap().unwrap().gold, 60);
    }

    #[test]
    fn tribute_fills_the_offering_and_the_bond() {
        let db = db();
        create(&db, 1, 2, "Antho", God::Hades).unwrap();
        run(&db, 1, 2, None, |g| g.tribute(God::Athena, 1600)).unwrap();
        let p = get(&db, 1, 2).unwrap().unwrap();
        assert_eq!(p.bond(God::Athena), 2);
        let o = db
            .with(|c| Ok(offerings(c, 1, week_now()).unwrap()))
            .unwrap();
        assert_eq!(o, [0, 1600, 0]);
    }
}
