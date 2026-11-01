//! Carry XP over from antho-s-bot. Reads its SQLite database (a backup from its dashboard, or a
//! copy of `/data/bot.db`) or a JSON list, and writes members, rewards and curve settings.

use std::collections::BTreeMap;
use std::path::Path;

use bot_core::anyhow::{Context, Result, bail};
use bot_core::rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use bot_core::serde_json;
use bot_core::{Ctx, text};

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
    /// None when the file doesn't say, as in the JSON format: the prestige here stays.
    prestige: Option<i64>,
}

/// The most prestige the dashboard lets staff set; imports are held to it too.
const MAX_PRESTIGE: i64 = 1000;

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
                r.get::<_, Option<i64>>(5)?,
            ))
        })?;
        for row in it {
            let (g, u, xp, messages, voice_minutes, prestige) = row?;
            let (Some(guild), Some(user)) = (id(&g), id(&u)) else { continue };
            rows.push(Row { guild, user, xp, messages, voice_minutes, prestige });
        }
    }

    let mut report = Report::default();
    let targets = targets(ctx, &rows, only, &mut report);

    // Settings and the custom curve first, so levels are computed with the right curve. Each
    // server keeps what it set here that the old bot didn't have (no-XP lists, the weekly post,
    // prestige, streaks): only the file's own settings are replaced.
    if has("level_config") {
        for guild in &targets {
            let current = ctx.settings::<Settings>(*guild, KEY)?;
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
                            // None posts where the member levelled up, in both bots. A channel
                            // that isn't this server's keeps the one set here.
                            announce_channel: match r.get::<_, Option<String>>(9)?.filter(|c| !c.trim().is_empty()) {
                                None => None,
                                Some(c) => file_channel(ctx, *guild, &c).or(current.announce_channel),
                            },
                            announce_ping: r.get(10)?,
                            announce_min_level: r.get::<_, i64>(11)?.clamp(0, 500) as u32,
                            message: r
                                .get::<_, Option<String>>(12)?
                                .filter(|m| !m.trim().is_empty())
                                .map(|m| text::clip(&m, 500))
                                .unwrap_or_else(|| current.message.clone()),
                            stack_rewards: r.get(13)?,
                            voice_require_active: r.get(14)?,
                            custom_curve: if r.get::<_, String>(5)? == "custom" { BTreeMap::from([(0, 0)]) } else { BTreeMap::new() },
                            ..(*current).clone()
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
                // The old bot didn't check these; hold them to what the settings page allows.
                s.xp_min = s.xp_min.clamp(0, 1000);
                s.xp_max = s.xp_max.clamp(s.xp_min, 1000);
                s.cooldown_secs = s.cooldown_secs.clamp(0, 3600);
                s.voice_xp = s.voice_xp.clamp(0, 1000);
                s.reaction_xp = s.reaction_xp.clamp(0, 1000);
                s.curve_base = s.curve_base.clamp(1, 100_000);
                if !(1.0..=5.0).contains(&s.curve_factor) {
                    s.curve_factor = Settings::default().curve_factor;
                }
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
            .filter_map(|(g, l, role)| Some((id(&g)?, l, id(&role)?)))
            // Only the servers being imported: roles of other servers in the file aren't looked at.
            .filter(|(g, _, _)| targets.contains(g))
            // The levels the rewards page takes. A reward at level 0 would go to everyone.
            .filter(|(_, level, _)| (1..=500).contains(level))
            .filter(|(g, _, role)| any_role || ctx.safe_role(*g, *role))
            .collect();
        ctx.db.with(|c| {
            for (g, level, role) in &rewards {
                c.execute(
                    "INSERT INTO level_rewards (guild_id, level, role_id) VALUES (?1, ?2, ?3)
                     ON CONFLICT (guild_id, level) DO UPDATE SET role_id = excluded.role_id",
                    params![*g as i64, level, *role as i64],
                )?;
            }
            Ok(())
        })?;
        report.rewards = rewards.len();
    }

    write_rows(ctx, &rows, &targets, mode, &mut report)?;
    Ok(report)
}

/// JSON: a list of `{ "guild_id", "user_id", "xp", "messages"?, "voice_minutes"? }`, with ids as
/// strings or numbers. This is what the Render Shell one-liner in the README prints.
pub fn import_json(ctx: &Ctx, bytes: &[u8], only: Option<u64>, mode: Mode) -> Result<Report> {
    let v: serde_json::Value = serde_json::from_slice(bytes).context("The file isn't a SQLite database or JSON")?;
    let list = v.as_array().or_else(|| v.get("levels").and_then(|l| l.as_array())).context("Expected a list of members")?;
    let json_id = |v: &serde_json::Value| -> Option<u64> { v.as_u64().filter(|n| *n != 0).or_else(|| id(v.as_str()?)) };
    let num = |v: Option<&serde_json::Value>| -> i64 { v.and_then(|x| x.as_i64()).unwrap_or(0) };
    let mut rows = Vec::new();
    for item in list {
        let guild = item.get("guild_id").and_then(json_id).or(only);
        let user = item.get("user_id").and_then(json_id);
        if let (Some(guild), Some(user)) = (guild, user) {
            rows.push(Row {
                guild,
                user,
                xp: num(item.get("xp")),
                messages: num(item.get("messages")),
                voice_minutes: num(item.get("voice_minutes")),
                prestige: item.get("prestige").and_then(|x| x.as_i64()),
            });
        }
    }
    let mut report = Report::default();
    let targets = targets(ctx, &rows, only, &mut report);
    write_rows(ctx, &rows, &targets, mode, &mut report)?;
    Ok(report)
}

/// An id from the file. 0 is never a Discord id, so it counts as none, like text that isn't a number.
fn id(s: &str) -> Option<u64> {
    s.trim().parse().ok().filter(|n| *n != 0)
}

/// The file's level-up channel when it's a channel of `guild`, so a file can't point level-ups
/// at another server. Before the gateway connects (the CLI import) there's nothing to check
/// against, so the file is trusted then, as in `targets`; the dashboard is never in that state.
fn file_channel(ctx: &Ctx, guild: u64, raw: &str) -> Option<u64> {
    let channel = id(raw)?;
    if ctx.guild_ids().is_empty() { Some(channel) } else { ctx.channel_in(guild, Some(channel)) }
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

/// A member's XP and prestige after the import, from what's stored here (`(xp, prestige)`, if
/// anything) and what the file has. Keep the higher keeps whichever of the two ranks higher, as a
/// whole: a member who prestiged here keeps their prestige and the XP they've earned since,
/// rather than getting their old XP back on top of it.
fn merged(mode: Mode, existing: Option<(i64, i64)>, xp: i64, prestige: Option<i64>) -> (i64, i64) {
    let file = (xp.clamp(0, crate::curve::MAX_XP), prestige.or(existing.map(|e| e.1)).unwrap_or(0).clamp(0, MAX_PRESTIGE));
    match (mode, existing) {
        (Mode::KeepHigher, Some((cur_xp, cur_prestige))) if (cur_prestige, cur_xp) >= (file.1, file.0) => (cur_xp, cur_prestige),
        _ => file,
    }
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
            let existing: Option<(i64, i64)> = tx
                .query_row(
                    "SELECT xp, prestige FROM levels WHERE guild_id = ?1 AND user_id = ?2",
                    params![r.guild as i64, r.user as i64],
                    |x| Ok((x.get(0)?, x.get(1)?)),
                )
                .optional()?;
            let (xp, prestige) = merged(mode, existing, r.xp, r.prestige);
            let level = s.progress(xp).level;
            tx.execute(
                "INSERT INTO levels (guild_id, user_id, xp, level, messages, voice_minutes, prestige, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (guild_id, user_id) DO UPDATE SET xp = excluded.xp, level = excluded.level,
                   messages = MAX(messages, excluded.messages), voice_minutes = MAX(voice_minutes, excluded.voice_minutes),
                   prestige = excluded.prestige, updated_at = excluded.updated_at",
                params![r.guild as i64, r.user as i64, xp, level as i64, r.messages.max(0), r.voice_minutes.max(0), prestige, bot_core::now_ms()],
            )?;
            // Imported XP wasn't earned this week: keep it off the weekly board.
            crate::shift_week(&tx, r.guild, r.user, xp - existing.map(|e| e.0).unwrap_or(0))?;
            n += 1;
        }
        // The import may have brought a new curve: members who weren't in the file move with it.
        crate::recompute_levels(&tx, *guild, &s)?;
        report.imported.push((*guild, n));
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::twilight_model::gateway::event::Event;

    /// A server as the gateway's GuildCreate carries it, with these text channels and roles
    /// (`(id, permissions)`).
    fn guild_create(ctx: &Ctx, guild: u64, channels: &[u64], roles: &[(u64, u64)]) {
        let mut role_list = vec![serde_json::json!({"id": guild.to_string(), "name": "@everyone", "color": 0, "colors": {"primary_color": 0}, "hoist": false, "managed": false, "mentionable": false, "permissions": "0", "position": 0, "flags": 0})];
        for (i, (id, perms)) in roles.iter().enumerate() {
            role_list.push(serde_json::json!({"id": id.to_string(), "name": format!("Role {id}"), "color": 0, "colors": {"primary_color": 0}, "hoist": false, "managed": false, "mentionable": false, "permissions": perms.to_string(), "position": i + 1, "flags": 0}));
        }
        let channel_list: Vec<_> = channels
            .iter()
            .enumerate()
            .map(|(i, id)| serde_json::json!({"id": id.to_string(), "type": 0, "name": format!("chat-{id}"), "position": i, "guild_id": guild.to_string(), "permission_overwrites": []}))
            .collect();
        let g = serde_json::json!({
            "id": guild.to_string(), "name": format!("Server {guild}"), "icon": null, "owner_id": "1", "afk_timeout": 300,
            "default_message_notifications": 1, "explicit_content_filter": 0, "features": [],
            "mfa_level": 0, "nsfw_level": 0, "preferred_locale": "en-US", "premium_tier": 0,
            "system_channel_flags": 0, "verification_level": 1, "premium_progress_bar_enabled": false, "member_count": 2,
            "roles": role_list, "channels": channel_list, "emojis": [], "stickers": [], "members": [], "presences": [],
            "voice_states": [], "threads": [], "stage_instances": [], "guild_scheduled_events": [], "unavailable": false,
            "large": false, "joined_at": "2024-01-01T00:00:00.000000+00:00"
        });
        ctx.cache.update(&Event::GuildCreate(Box::new(serde_json::from_value(g).unwrap())));
    }

    fn ctx() -> Ctx {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.install_features(vec![std::sync::Arc::new(crate::Levels::default())]).unwrap();
        ctx
    }

    /// Two servers: 1 with channel 10 and role 100, 2 with channel 20 and role 200.
    fn two_servers() -> Ctx {
        let ctx = ctx();
        guild_create(&ctx, 1, &[10], &[(100, 0)]);
        guild_create(&ctx, 2, &[20], &[(200, 0)]);
        ctx
    }

    /// antho-s-bot's tables, with `rows` inserted, in a file under the build folder.
    fn old_db(rows: &str) -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-files");
        std::fs::create_dir_all(&dir).unwrap();
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = dir.join(format!("levels-import-{}-{n}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let c = Connection::open(&path).unwrap();
        c.execute_batch(&format!(
            "CREATE TABLE levels (guild_id TEXT, user_id TEXT, xp INTEGER, messages INTEGER, voice_minutes INTEGER, prestige INTEGER);
             CREATE TABLE level_config (guild_id TEXT, xp_msg_min INTEGER, xp_msg_max INTEGER, msg_cooldown_sec INTEGER,
               xp_per_voice_min INTEGER, xp_per_reaction INTEGER, curve_type TEXT, curve_base INTEGER, curve_factor REAL,
               announce INTEGER, announce_channel_id TEXT, announce_ping INTEGER, announce_min_level INTEGER,
               level_up_message TEXT, stack_role_rewards INTEGER, voice_require_active INTEGER);
             CREATE TABLE level_rewards (guild_id TEXT, level INTEGER, role_id TEXT);
             {rows}"
        ))
        .unwrap();
        path
    }

    /// Server 1's settings in the old bot, and a member, since only servers with members are imported.
    fn config_row(channel: &str, message: &str) -> String {
        format!(
            "INSERT INTO level_config VALUES ('1', 10, 20, 30, 4, 0, 'exponential', 120, 1.3, 1, {channel}, 0, 2, {message}, 1, 1);
             INSERT INTO levels VALUES ('1', '11', 500, 3, 0, 0);"
        )
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn a_file_cant_point_level_ups_at_another_server() {
        let ctx = two_servers();
        assert_eq!(file_channel(&ctx, 1, "10"), Some(10));
        assert_eq!(file_channel(&ctx, 1, "20"), None, "server 2's channel");
        assert_eq!(file_channel(&ctx, 1, "0"), None);
        assert_eq!(file_channel(&ctx, 1, "general"), None);
        // The CLI import runs before the gateway connects and has nothing to check against.
        assert_eq!(file_channel(&self::ctx(), 1, "20"), Some(20));
        assert_eq!(file_channel(&self::ctx(), 1, "0"), None);

        ctx.save_settings(1, KEY, &Settings { announce_channel: Some(10), ..Default::default() }).unwrap();
        let long = "x".repeat(600);
        for channel in ["'20'", "'0'", "'nope'"] {
            let path = old_db(&config_row(channel, &format!("'{long}'")));
            import_db(&ctx, &path, Some(1), Mode::Replace, false).unwrap();
            let s = ctx.settings::<Settings>(1, KEY).unwrap();
            assert_eq!(s.announce_channel, Some(10), "a file channel of {channel} keeps the one set here");
            // Held to what the settings page takes.
            assert_eq!(s.message.chars().count(), 500);
            let _ = std::fs::remove_file(&path);
        }
        // The file's own channel, and none (post where they levelled up).
        let path = old_db(&config_row("'10'", "NULL"));
        import_db(&ctx, &path, Some(1), Mode::Replace, false).unwrap();
        assert_eq!(ctx.settings::<Settings>(1, KEY).unwrap().announce_channel, Some(10));
        let _ = std::fs::remove_file(&path);
        let path = old_db(&config_row("NULL", "NULL"));
        import_db(&ctx, &path, Some(1), Mode::Replace, false).unwrap();
        assert_eq!(ctx.settings::<Settings>(1, KEY).unwrap().announce_channel, None);
        let _ = std::fs::remove_file(&path);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn a_reimport_keeps_the_settings_the_old_bot_never_had() {
        let ctx = two_servers();
        let mine = Settings {
            message: "GG {user}, level {level}".into(),
            no_xp_channels: vec![10],
            no_xp_categories: vec![5],
            no_xp_roles: vec![100],
            weekly_post: true,
            weekly_channel: Some(10),
            prestige_level: 30,
            prestige_bonus: 7,
            streak_bonus: 2,
            streak_max: 12,
            ..Default::default()
        };
        ctx.save_settings(1, KEY, &mine).unwrap();
        let path = old_db(&config_row("NULL", "NULL"));
        let report = import_db(&ctx, &path, Some(1), Mode::KeepHigher, false).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(report.settings, 1);
        let s = ctx.settings::<Settings>(1, KEY).unwrap();
        // The file's settings came over.
        assert_eq!((s.xp_min, s.xp_max, s.cooldown_secs, s.voice_xp, s.curve_base, s.announce_min_level), (10, 20, 30, 4, 120, 2));
        assert!(!s.announce_ping);
        // Everything else is as it was here, and no message in the file keeps this one.
        assert_eq!(s.message, mine.message);
        assert_eq!((s.no_xp_channels.clone(), s.no_xp_categories.clone(), s.no_xp_roles.clone()), (vec![10], vec![5], vec![100]));
        assert_eq!((s.weekly_post, s.weekly_channel), (true, Some(10)));
        assert_eq!((s.prestige_level, s.prestige_bonus, s.streak_bonus, s.streak_max), (30, 7, 2, 12));
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn zero_ids_in_a_file_are_skipped() {
        let ctx = two_servers();
        let path = old_db(
            "INSERT INTO levels VALUES ('1', '0', 500, 0, 0, 0), ('1', '11', 300, 0, 0, 0), ('0', '12', 100, 0, 0, 0);
             INSERT INTO level_rewards VALUES ('1', 5, '0'), ('1', 6, '100'), ('2', 7, '200'), ('2', 8, '0'), ('0', 9, '100');",
        );
        let report = import_db(&ctx, &path, Some(1), Mode::Replace, false).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(report.imported, vec![(1, 1)]);
        assert_eq!(report.rewards, 1);
        let users: Vec<i64> = ctx.db.with(|c| c.prepare("SELECT user_id FROM levels ORDER BY user_id")?.query_map([], |r| r.get(0))?.collect()).unwrap();
        assert_eq!(users, vec![11]);
        // Server 2's rewards aren't imported into server 1, and no reward is role 0.
        assert_eq!(crate::rewards(&ctx, 1).unwrap(), vec![(6, 100)]);
        assert!(crate::rewards(&ctx, 2).unwrap().is_empty());

        let json = br#"[{"guild_id": "1", "user_id": 0, "xp": 5}, {"guild_id": "1", "user_id": "0", "xp": 5}, {"guild_id": 1, "user_id": "13", "xp": 5}]"#;
        let report = import_json(&ctx, json, Some(1), Mode::Replace).unwrap();
        assert_eq!(report.imported, vec![(1, 1)]);
    }

    #[test]
    fn keep_the_higher_compares_prestige_and_xp_together() {
        // Prestiged here: the old bot's higher XP doesn't come back on top of the prestige.
        assert_eq!(merged(Mode::KeepHigher, Some((300, 1)), 9_000, Some(0)), (300, 1));
        // The file ranks higher: its XP and prestige both come over.
        assert_eq!(merged(Mode::KeepHigher, Some((300, 1)), 100, Some(2)), (100, 2));
        assert_eq!(merged(Mode::KeepHigher, Some((300, 1)), 900, Some(1)), (900, 1));
        // A file without prestige leaves it as it is here, in both modes.
        assert_eq!(merged(Mode::KeepHigher, Some((300, 2)), 900, None), (900, 2));
        assert_eq!(merged(Mode::KeepHigher, Some((300, 2)), 100, None), (300, 2));
        assert_eq!(merged(Mode::Replace, Some((300, 2)), 100, None), (100, 2));
        assert_eq!(merged(Mode::Replace, None, 100, None), (100, 0));
        // Held to what the dashboard allows.
        assert_eq!(merged(Mode::Replace, None, -5, Some(i64::MAX)), (0, MAX_PRESTIGE));
        assert_eq!(merged(Mode::Replace, None, 5, Some(-3)), (5, 0));
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn a_json_import_keeps_prestige() {
        let ctx = two_servers();
        crate::set_prestige(&ctx, 1, 11, 3).unwrap();
        let json = br#"[{"guild_id": "1", "user_id": "11", "xp": 500}]"#;
        import_json(&ctx, json, Some(1), Mode::Replace).unwrap();
        let m = crate::member(&ctx, 1, 11).unwrap().unwrap();
        assert_eq!((m.xp, m.prestige), (500, 3));
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn imported_rewards_and_prestige_are_held_to_the_dashboard_limits() {
        let ctx = two_servers();
        let path = old_db(
            "INSERT INTO levels VALUES ('1', '11', 300, 0, 0, 999999999999), ('1', '12', 300, 0, 0, -4), ('1', '13', 300, 0, 0, NULL);
             INSERT INTO level_rewards VALUES ('1', 0, '100'), ('1', -3, '100'), ('1', 501, '100'), ('1', 5, '100');",
        );
        let report = import_db(&ctx, &path, Some(1), Mode::Replace, false).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(report.rewards, 1);
        assert_eq!(crate::rewards(&ctx, 1).unwrap(), vec![(5, 100)]);
        let prestige = |user| crate::member(&ctx, 1, user).unwrap().unwrap().prestige;
        assert_eq!((prestige(11), prestige(12), prestige(13)), (MAX_PRESTIGE, 0, 0));
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn a_new_curve_moves_members_who_werent_in_the_file() {
        let ctx = two_servers();
        // Member 20 is only here, at a level from a much cheaper curve.
        ctx.db.with(|c| c.execute("INSERT INTO levels (guild_id, user_id, xp, level) VALUES (1, 20, 150, 9)", [])).unwrap();
        // The file's curve starts at 120 XP a level with a factor of 1.3.
        let path = old_db(&config_row("NULL", "NULL"));
        import_db(&ctx, &path, Some(1), Mode::Replace, false).unwrap();
        let _ = std::fs::remove_file(&path);
        let s = ctx.settings::<Settings>(1, KEY).unwrap();
        assert!(s.progress(150).level < 9);
        assert_eq!(crate::member(&ctx, 1, 20).unwrap().unwrap().level, s.progress(150).level);
        assert_eq!(crate::member(&ctx, 1, 11).unwrap().unwrap().level, s.progress(500).level);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn rewards_skip_roles_the_bot_cant_give() {
        let ctx = two_servers();
        assert!(crate::assignable(&ctx, 1, 100));
        // Missing, another server's view of @everyone, and role 0.
        assert!(!crate::assignable(&ctx, 1, 999));
        assert!(!crate::assignable(&ctx, 1, 1));
        assert!(!crate::assignable(&ctx, 1, 0));
    }
}
