//! Carry XP over from antho-s-bot. Reads its SQLite database (a backup from its dashboard, or a
//! copy of `/data/bot.db`) or a JSON list, and writes members, rewards and curve settings.

use std::collections::BTreeMap;
use std::path::Path;

use bot_core::anyhow::{Context, Result, bail};
use bot_core::rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use bot_core::serde_json;
use bot_core::Ctx;

use crate::{KEY, Settings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Overwrite each member's XP with the imported value.
    Replace,
    /// Keep whichever is higher, for re-importing after members already earned XP here.
    KeepHigher,
}

#[derive(Debug, Default)]
pub struct Report {
    /// Server id and how many members came over.
    pub imported: Vec<(u64, usize)>,
    /// Servers in the file the bot isn't in, with their member counts.
    pub skipped: Vec<(u64, usize)>,
    pub rewards: usize,
    pub settings: usize,
}

impl Report {
    pub fn summary(&self, ctx: &Ctx) -> String {
        let mut parts: Vec<String> = self
            .imported
            .iter()
            .map(|(g, n)| format!("{} members into {}", bot_core::text::num(*n as i64), ctx.guild_name(*g)))
            .collect();
        if self.rewards > 0 {
            parts.push(format!("{} reward roles", self.rewards));
        }
        if self.settings > 0 {
            parts.push("XP settings".into());
        }
        if parts.is_empty() {
            parts.push("Nothing matched a server this bot is in".into());
        }
        let mut s = format!("Imported {}.", parts.join(", "));
        if !self.skipped.is_empty() {
            let list: Vec<String> = self.skipped.iter().map(|(g, n)| format!("{g} ({n} members)")).collect();
            s.push_str(&format!(" Skipped servers the bot isn't in: {}.", list.join(", ")));
        }
        s
    }
}

struct Row {
    guild: u64,
    user: u64,
    xp: i64,
    messages: i64,
    voice_minutes: i64,
    prestige: i64,
}

/// Import from bytes: a SQLite file or JSON. `only` limits it to one server.
/// `any_role`: keep reward roles with moderation or admin power. Only for admins and the CLI;
/// otherwise such rewards are skipped.
pub fn import_bytes(ctx: &Ctx, bytes: &[u8], only: Option<u64>, mode: Mode, any_role: bool) -> Result<Report> {
    if bytes.starts_with(b"SQLite format 3\0") {
        let dir = std::env::temp_dir().join(format!("bot-import-{}", bot_core::now_ms()));
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("import.db");
        std::fs::write(&path, bytes)?;
        let res = import_db(ctx, &path, only, mode, any_role);
        let _ = std::fs::remove_dir_all(&dir);
        res
    } else {
        import_json(ctx, bytes, only, mode)
    }
}

pub fn import_db(ctx: &Ctx, path: &Path, only: Option<u64>, mode: Mode, any_role: bool) -> Result<Report> {
    let src = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening {}", path.display()))?;
    let has = |table: &str| -> bool {
        src.query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1", [table], |_| Ok(()))
            .optional()
            .ok()
            .flatten()
            .is_some()
    };
    if !has("levels") {
        bail!("This file has no levels table. Is it antho-s-bot's database?");
    }
    let mut rows = Vec::new();
    {
        let mut stmt = src.prepare("SELECT guild_id, user_id, xp, messages, voice_minutes, prestige FROM levels")?;
        let it = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?;
        for row in it {
            let (g, u, xp, messages, voice_minutes, prestige) = row?;
            let (Ok(guild), Ok(user)) = (g.parse(), u.parse()) else { continue };
            rows.push(Row { guild, user, xp, messages, voice_minutes, prestige });
        }
    }

    let mut report = Report::default();
    let targets = targets(ctx, &rows, only, &mut report);

    // Settings and the custom curve first, so levels are computed with the right curve.
    if has("level_config") {
        for guild in &targets {
            let cfg = src
                .query_row(
                    "SELECT xp_msg_min, xp_msg_max, msg_cooldown_sec, xp_per_voice_min, xp_per_reaction, curve_type,
                            curve_base, curve_factor, announce, announce_channel_id, announce_ping, announce_min_level,
                            level_up_message, stack_role_rewards, voice_require_active
                     FROM level_config WHERE guild_id = ?1",
                    [guild.to_string()],
                    |r| {
                        Ok(Settings {
                            xp_min: r.get(0)?,
                            xp_max: r.get(1)?,
                            cooldown_secs: r.get(2)?,
                            voice_xp: r.get(3)?,
                            reaction_xp: r.get(4)?,
                            curve_base: r.get(6)?,
                            curve_factor: r.get(7)?,
                            announce: r.get(8)?,
                            announce_channel: r.get::<_, Option<String>>(9)?.and_then(|s| s.parse().ok()),
                            announce_ping: r.get(10)?,
                            announce_min_level: r.get::<_, i64>(11)?.max(0) as u32,
                            message: r.get::<_, Option<String>>(12)?.filter(|m| !m.trim().is_empty()).unwrap_or_else(|| crate::DEFAULT_MESSAGE.into()),
                            stack_rewards: r.get(13)?,
                            voice_require_active: r.get(14)?,
                            custom_curve: if r.get::<_, String>(5)? == "custom" { BTreeMap::from([(0, 0)]) } else { BTreeMap::new() },
                            ..Settings::default()
                        })
                    },
                )
                .optional()?;
            if let Some(mut s) = cfg {
                if s.custom_curve.remove(&0).is_some() && has("level_curve") {
                    let mut stmt = src.prepare("SELECT level, xp_required FROM level_curve WHERE guild_id = ?1")?;
                    s.custom_curve = stmt
                        .query_map([guild.to_string()], |r| Ok((r.get::<_, i64>(0)? as u32, r.get::<_, i64>(1)?)))?
                        .collect::<Result<_, _>>()?;
                }
                let current = ctx.settings::<Settings>(*guild, KEY)?;
                s.no_xp_channels = current.no_xp_channels.clone();
                s.no_xp_roles = current.no_xp_roles.clone();
                ctx.save_settings(*guild, KEY, &s)?;
                report.settings += 1;
            }
        }
    }

    if has("level_rewards") {
        let mut stmt = src.prepare("SELECT guild_id, level, role_id FROM level_rewards")?;
        let rewards: Vec<(u64, i64, u64)> = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))?
            .filter_map(|r| r.ok())
            .filter_map(|(g, l, role)| Some((g.parse().ok()?, l, role.parse().ok()?)))
            .filter(|(g, _, role)| any_role || ctx.safe_role(*g, *role))
            .collect();
        ctx.db.with(|c| {
            for (g, level, role) in &rewards {
                if targets.contains(g) {
                    c.execute(
                        "INSERT INTO level_rewards (guild_id, level, role_id) VALUES (?1, ?2, ?3)
                         ON CONFLICT (guild_id, level) DO UPDATE SET role_id = excluded.role_id",
                        params![*g as i64, level, *role as i64],
                    )?;
                }
            }
            Ok(())
        })?;
        report.rewards = rewards.iter().filter(|(g, _, _)| targets.contains(g)).count();
    }

    write_rows(ctx, &rows, &targets, mode, &mut report)?;
    Ok(report)
}

/// JSON: a list of `{ "guild_id", "user_id", "xp", "messages"?, "voice_minutes"? }`, with ids as
/// strings or numbers. This is what the Render Shell one-liner in the README prints.
pub fn import_json(ctx: &Ctx, bytes: &[u8], only: Option<u64>, mode: Mode) -> Result<Report> {
    let v: serde_json::Value = serde_json::from_slice(bytes).context("The file isn't a SQLite database or JSON")?;
    let list = v.as_array().or_else(|| v.get("levels").and_then(|l| l.as_array())).context("Expected a list of members")?;
    let id = |v: &serde_json::Value| -> Option<u64> { v.as_u64().or_else(|| v.as_str()?.parse().ok()) };
    let num = |v: Option<&serde_json::Value>| -> i64 { v.and_then(|x| x.as_i64()).unwrap_or(0) };
    let mut rows = Vec::new();
    for item in list {
        let guild = item.get("guild_id").and_then(id).or(only);
        let user = item.get("user_id").and_then(id);
        if let (Some(guild), Some(user)) = (guild, user) {
            rows.push(Row {
                guild,
                user,
                xp: num(item.get("xp")),
                messages: num(item.get("messages")),
                voice_minutes: num(item.get("voice_minutes")),
                prestige: num(item.get("prestige")),
            });
        }
    }
    let mut report = Report::default();
    let targets = targets(ctx, &rows, only, &mut report);
    write_rows(ctx, &rows, &targets, mode, &mut report)?;
    Ok(report)
}

fn targets(ctx: &Ctx, rows: &[Row], only: Option<u64>, report: &mut Report) -> Vec<u64> {
    let mut guilds: Vec<u64> = rows.iter().map(|r| r.guild).collect();
    guilds.sort_unstable();
    guilds.dedup();
    let known = ctx.guild_ids();
    let mut out = Vec::new();
    for g in guilds {
        let n = rows.iter().filter(|r| r.guild == g).count();
        if only.is_some_and(|o| o != g) {
            continue;
        }
        // Before the gateway connects the cache is empty; trust the file then (CLI import).
        if known.is_empty() || known.contains(&g) {
            out.push(g);
        } else {
            report.skipped.push((g, n));
        }
    }
    out
}

fn write_rows(ctx: &Ctx, rows: &[Row], targets: &[u64], mode: Mode, report: &mut Report) -> Result<()> {
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    for guild in targets {
        let s: Settings = {
            let raw: Option<String> = tx
                .query_row("SELECT json FROM guild_settings WHERE guild_id = ?1 AND feature = ?2", params![*guild as i64, KEY], |r| r.get(0))
                .optional()?;
            raw.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default()
        };
        let mut n = 0;
        for r in rows.iter().filter(|r| r.guild == *guild) {
            let existing: Option<i64> = tx
                .query_row("SELECT xp FROM levels WHERE guild_id = ?1 AND user_id = ?2", params![r.guild as i64, r.user as i64], |x| x.get(0))
                .optional()?;
            let xp = match (mode, existing) {
                (Mode::KeepHigher, Some(cur)) => cur.max(r.xp),
                _ => r.xp,
            };
            let level = s.progress(xp).level;
            tx.execute(
                "INSERT INTO levels (guild_id, user_id, xp, level, messages, voice_minutes, prestige, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (guild_id, user_id) DO UPDATE SET xp = excluded.xp, level = excluded.level,
                   messages = MAX(messages, excluded.messages), voice_minutes = MAX(voice_minutes, excluded.voice_minutes),
                   prestige = excluded.prestige, updated_at = excluded.updated_at",
                params![r.guild as i64, r.user as i64, xp, level as i64, r.messages, r.voice_minutes, r.prestige, bot_core::now_ms()],
            )?;
            n += 1;
        }
        report.imported.push((*guild, n));
    }
    tx.commit()?;
    Ok(())
}
