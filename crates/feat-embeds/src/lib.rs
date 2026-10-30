//! Write embeds on the dashboard and send or edit them in a channel. Saved templates, and a
//! record of what was sent so a message can be loaded back and edited in place.

pub mod model;
mod web;

use bot_core::anyhow::Result;
use bot_core::rusqlite::{OptionalExtension, params};
use bot_core::{Ctx, Feature, FeatureMeta, Group, Icon, Migration, NavItem, feature_async};

use model::{EmbedData, Field};

pub const KEY: &str = "embeds";
/// Templates a server can keep, and sent messages remembered.
const MAX_TEMPLATES: i64 = 50;
const KEEP_SENT: i64 = 100;

#[derive(Default)]
pub struct Embeds;

#[derive(Debug, Clone)]
pub struct Template {
    pub id: i64,
    pub name: String,
    pub data: EmbedData,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct Sent {
    pub id: i64,
    pub channel_id: u64,
    pub message_id: u64,
    pub template: Option<String>,
    pub summary: String,
    pub data: EmbedData,
    pub sent_by: String,
    pub sent_at: i64,
    pub edited_at: Option<i64>,
}

const MIGRATIONS: &[Migration] = &[Migration {
    name: "001_embeds",
    sql: "
        CREATE TABLE embeds_templates (
            id INTEGER PRIMARY KEY AUTOINCREMENT, guild_id INTEGER NOT NULL, name TEXT NOT NULL,
            data TEXT NOT NULL, updated_at INTEGER NOT NULL, UNIQUE (guild_id, name));
        CREATE TABLE embeds_sent (
            id INTEGER PRIMARY KEY AUTOINCREMENT, guild_id INTEGER NOT NULL, channel_id INTEGER NOT NULL,
            message_id INTEGER NOT NULL, template TEXT, summary TEXT NOT NULL, data TEXT NOT NULL,
            sent_by TEXT NOT NULL, sent_at INTEGER NOT NULL, edited_at INTEGER);
        CREATE INDEX embeds_sent_by_guild ON embeds_sent(guild_id, sent_at DESC);
    ",
}];

#[feature_async]
impl Feature for Embeds {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Embed builder",
            description: "Write embeds on the dashboard and send or edit them in a channel.",
            group: Group::Tools,
            nav: vec![NavItem { label: "Embed builder", path: KEY, icon: Icon::Brush }],
        }
    }

    fn migrations(&self) -> &'static [Migration] {
        MIGRATIONS
    }

    fn routes(&self) -> bot_core::axum::Router<Ctx> {
        web::routes()
    }

    fn seed_preview(&self, ctx: &Ctx, guild: u64) -> Result<()> {
        let rules = EmbedData {
            title: "Server rules".into(),
            description: "Be kind to each other. No spam, no spoilers outside the spoiler channels, and keep chat in the right channel.\n\nMods have the last word. If something feels off, ping **@Mods**.".into(),
            color: "#f0806a".into(),
            footer: "Last updated this month".into(),
            fields: vec![
                Field { name: "Spoilers".into(), value: "Use ||spoiler tags||.".into(), inline: true },
                Field { name: "Questions".into(), value: "Ask in #general.".into(), inline: true },
                Field { name: "Reports".into(), value: "DM a mod.".into(), inline: true },
            ],
            ..Default::default()
        };
        let live = EmbedData {
            content: "We're live!".into(),
            title: "Boss rush, then the new update".into(),
            url: "https://twitch.tv/".into(),
            description: "Come say hi. Today: the new world, a few deaths, and giveaways in the last hour.".into(),
            color: "#9b84ee".into(),
            author: "Antho".into(),
            thumbnail: "https://cdn.discordapp.com/embed/avatars/2.png".into(),
            ..Default::default()
        };
        let welcome = EmbedData { title: "Welcome".into(), description: "Say hi in #general and pick your roles.".into(), color: "#4fc38a".into(), ..Default::default() };
        let channels = ctx.text_channels(guild);
        let find = |name: &str| channels.iter().find(|(_, n)| n == name).or(channels.first()).map(|(id, _)| *id).unwrap_or_default();
        template_save(ctx, guild, "Server rules", &rules)?;
        template_save(ctx, guild, "Stream announcement", &live)?;
        sent_add(ctx, guild, find("welcome"), 1_300_000_000_000_000_001, Some("Server rules"), &rules, "Antho")?;
        sent_add(ctx, guild, find("announcements"), 1_300_000_000_000_000_002, None, &live, "Antho")?;
        sent_add(ctx, guild, find("welcome"), 1_300_000_000_000_000_003, None, &welcome, "Mira")?;
        // Spread the sample messages over the past days, oldest first.
        let day = 86_400_000;
        ctx.db.with(|c| {
            c.execute("UPDATE embeds_sent SET sent_at = ?2 WHERE guild_id = ?1 AND message_id = 1300000000000000001", params![guild as i64, bot_core::now_ms() - 12 * day])?;
            c.execute("UPDATE embeds_sent SET sent_at = ?2 WHERE guild_id = ?1 AND message_id = 1300000000000000002", params![guild as i64, bot_core::now_ms() - 3 * 3_600_000])?;
            c.execute("UPDATE embeds_templates SET updated_at = ?2 WHERE guild_id = ?1 AND name = 'Server rules'", params![guild as i64, bot_core::now_ms() - 13 * day])
        })?;
        Ok(())
    }
}

fn parse(json: String) -> EmbedData {
    bot_core::serde_json::from_str(&json).unwrap_or_default()
}

pub fn templates(ctx: &Ctx, guild: u64) -> Result<Vec<Template>> {
    ctx.db.with(|c| {
        let mut st = c.prepare_cached("SELECT id, name, data, updated_at FROM embeds_templates WHERE guild_id = ?1 ORDER BY name COLLATE NOCASE")?;
        st.query_map([guild as i64], |r| Ok(Template { id: r.get(0)?, name: r.get(1)?, data: parse(r.get(2)?), updated_at: r.get(3)? }))?
            .collect::<Result<Vec<_>, _>>()
    })
}

pub fn template(ctx: &Ctx, guild: u64, id: i64) -> Result<Option<Template>> {
    ctx.db.with(|c| {
        c.query_row(
            "SELECT id, name, data, updated_at FROM embeds_templates WHERE guild_id = ?1 AND id = ?2",
            params![guild as i64, id],
            |r| Ok(Template { id: r.get(0)?, name: r.get(1)?, data: parse(r.get(2)?), updated_at: r.get(3)? }),
        )
        .optional()
    })
}

/// Save under a name, replacing a template of the same name. `None` when the server is at its
/// limit and the name is new.
pub fn template_save(ctx: &Ctx, guild: u64, name: &str, data: &EmbedData) -> Result<Option<i64>> {
    let json = bot_core::serde_json::to_string(data)?;
    ctx.db.with(|c| {
        let exists: bool = c.query_row("SELECT EXISTS (SELECT 1 FROM embeds_templates WHERE guild_id = ?1 AND name = ?2)", params![guild as i64, name], |r| r.get(0))?;
        if !exists {
            let n: i64 = c.query_row("SELECT COUNT(*) FROM embeds_templates WHERE guild_id = ?1", [guild as i64], |r| r.get(0))?;
            if n >= MAX_TEMPLATES {
                return Ok(None);
            }
        }
        c.execute(
            "INSERT INTO embeds_templates (guild_id, name, data, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (guild_id, name) DO UPDATE SET data = excluded.data, updated_at = excluded.updated_at",
            params![guild as i64, name, json, bot_core::now_ms()],
        )?;
        c.query_row("SELECT id FROM embeds_templates WHERE guild_id = ?1 AND name = ?2", params![guild as i64, name], |r| r.get(0)).map(Some)
    })
}

pub fn template_delete(ctx: &Ctx, guild: u64, id: i64) -> Result<Option<String>> {
    ctx.db.with(|c| {
        c.query_row("DELETE FROM embeds_templates WHERE guild_id = ?1 AND id = ?2 RETURNING name", params![guild as i64, id], |r| r.get(0)).optional()
    })
}

fn sent_from_row(r: &bot_core::rusqlite::Row<'_>) -> bot_core::rusqlite::Result<Sent> {
    Ok(Sent {
        id: r.get(0)?,
        channel_id: r.get::<_, i64>(1)? as u64,
        message_id: r.get::<_, i64>(2)? as u64,
        template: r.get(3)?,
        summary: r.get(4)?,
        data: parse(r.get(5)?),
        sent_by: r.get(6)?,
        sent_at: r.get(7)?,
        edited_at: r.get(8)?,
    })
}

const SENT_COLUMNS: &str = "id, channel_id, message_id, template, summary, data, sent_by, sent_at, edited_at";

pub fn sent_list(ctx: &Ctx, guild: u64) -> Result<Vec<Sent>> {
    ctx.db.with(|c| {
        let mut st = c.prepare_cached(&format!("SELECT {SENT_COLUMNS} FROM embeds_sent WHERE guild_id = ?1 ORDER BY sent_at DESC, id DESC LIMIT 30"))?;
        st.query_map([guild as i64], sent_from_row)?.collect::<Result<Vec<_>, _>>()
    })
}

pub fn sent_get(ctx: &Ctx, guild: u64, id: i64) -> Result<Option<Sent>> {
    ctx.db.with(|c| {
        c.query_row(&format!("SELECT {SENT_COLUMNS} FROM embeds_sent WHERE guild_id = ?1 AND id = ?2"), params![guild as i64, id], sent_from_row).optional()
    })
}

pub fn sent_add(ctx: &Ctx, guild: u64, channel: u64, message: u64, template: Option<&str>, data: &EmbedData, by: &str) -> Result<i64> {
    let json = bot_core::serde_json::to_string(data)?;
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO embeds_sent (guild_id, channel_id, message_id, template, summary, data, sent_by, sent_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![guild as i64, channel as i64, message as i64, template, data.summary(), json, by, bot_core::now_ms()],
        )?;
        let id = c.last_insert_rowid();
        c.execute(
            "DELETE FROM embeds_sent WHERE guild_id = ?1 AND id NOT IN (SELECT id FROM embeds_sent WHERE guild_id = ?1 ORDER BY sent_at DESC, id DESC LIMIT ?2)",
            params![guild as i64, KEEP_SENT],
        )?;
        Ok(id)
    })
}

pub fn sent_update(ctx: &Ctx, guild: u64, id: i64, data: &EmbedData) -> Result<()> {
    let json = bot_core::serde_json::to_string(data)?;
    ctx.db.with(|c| {
        c.execute(
            "UPDATE embeds_sent SET data = ?3, summary = ?4, edited_at = ?5 WHERE guild_id = ?1 AND id = ?2",
            params![guild as i64, id, json, data.summary(), bot_core::now_ms()],
        )
    })?;
    Ok(())
}

pub fn sent_delete(ctx: &Ctx, guild: u64, id: i64) -> Result<()> {
    ctx.db.with(|c| c.execute("DELETE FROM embeds_sent WHERE guild_id = ?1 AND id = ?2", params![guild as i64, id]))?;
    Ok(())
}
