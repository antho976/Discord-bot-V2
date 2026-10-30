//! The last two weeks of entries, for the dashboard.

use bot_core::Ctx;
use bot_core::anyhow::Result;
use bot_core::rusqlite::{ToSql, params, params_from_iter};

use crate::{Kind, Settings};

const DAY_MS: i64 = 86_400_000;

#[derive(Debug, Clone)]
pub struct Row {
    pub at: i64,
    pub kind: Kind,
    pub user_id: Option<u64>,
    pub user_name: String,
    pub channel_id: Option<u64>,
    pub summary: String,
}

#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub kind: Option<Kind>,
    pub search: Option<String>,
}

pub fn record(ctx: &Ctx, guild: u64, kind: Kind, user: Option<u64>, user_name: &str, channel: Option<u64>, summary: &str) -> Result<()> {
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO audit_events (guild_id, at, kind, user_id, user_name, channel_id, summary) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![guild as i64, bot_core::now_ms(), kind.key(), user.map(|u| u as i64), user_name, channel.map(|c| c as i64), summary],
        )
    })?;
    Ok(())
}

pub fn prune(ctx: &Ctx, before: i64) -> Result<()> {
    ctx.db.with(|c| c.execute("DELETE FROM audit_events WHERE at < ?1", [before]))?;
    Ok(())
}

fn where_clause(guild: u64, f: &Filter) -> (String, Vec<Box<dyn ToSql>>) {
    let mut sql = "guild_id = ?1".to_owned();
    let mut args: Vec<Box<dyn ToSql>> = vec![Box::new(guild as i64)];
    if let Some(k) = f.kind {
        args.push(Box::new(k.key()));
        sql.push_str(&format!(" AND kind = ?{}", args.len()));
    }
    if let Some(q) = f.search.as_deref().filter(|q| !q.is_empty()) {
        args.push(Box::new(q.to_owned()));
        let name = args.len();
        args.push(Box::new(q.parse::<i64>().unwrap_or(-1)));
        sql.push_str(&format!(" AND (instr(lower(user_name), lower(?{name})) > 0 OR user_id = ?{})", args.len()));
    }
    (sql, args)
}

/// The newest entries matching the filter, and how many match in all.
pub fn recent(ctx: &Ctx, guild: u64, f: &Filter, limit: i64) -> Result<(Vec<Row>, i64)> {
    let (clause, args) = where_clause(guild, f);
    ctx.db.with(|c| {
        let total: i64 = c.query_row(&format!("SELECT COUNT(*) FROM audit_events WHERE {clause}"), params_from_iter(args.iter().map(|a| a.as_ref())), |r| r.get(0))?;
        let mut stmt = c.prepare(&format!(
            "SELECT at, kind, user_id, user_name, channel_id, summary FROM audit_events WHERE {clause} ORDER BY at DESC, id DESC LIMIT {limit}"
        ))?;
        let rows = stmt
            .query_map(params_from_iter(args.iter().map(|a| a.as_ref())), |r| {
                let key: String = r.get(1)?;
                Ok(Row {
                    at: r.get(0)?,
                    kind: Kind::from_key(&key).unwrap_or(Kind::MessageEdit),
                    user_id: r.get::<_, Option<i64>>(2)?.map(|u| u as u64),
                    user_name: r.get(3)?,
                    channel_id: r.get::<_, Option<i64>>(4)?.map(|c| c as u64),
                    summary: r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok((rows, total))
    })
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub today: i64,
    pub deleted_week: i64,
    pub joined_week: i64,
    pub left_week: i64,
}

pub fn stats(ctx: &Ctx, guild: u64) -> Result<Stats> {
    let now = bot_core::now_ms();
    let day_start = now - now.rem_euclid(DAY_MS);
    let week = now - 7 * DAY_MS;
    ctx.db.with(|c| {
        let count = |sql: &str, since: i64| c.query_row(sql, params![guild as i64, since], |r| r.get::<_, i64>(0));
        Ok(Stats {
            today: count("SELECT COUNT(*) FROM audit_events WHERE guild_id = ?1 AND at >= ?2", day_start)?,
            deleted_week: count("SELECT COUNT(*) FROM audit_events WHERE guild_id = ?1 AND at >= ?2 AND kind IN ('message_delete', 'bulk_delete')", week)?,
            joined_week: count("SELECT COUNT(*) FROM audit_events WHERE guild_id = ?1 AND at >= ?2 AND kind = 'join'", week)?,
            left_week: count("SELECT COUNT(*) FROM audit_events WHERE guild_id = ?1 AND at >= ?2 AND kind = 'leave'", week)?,
        })
    })
}

/// Sample settings and a believable fortnight of entries for `bot preview`.
pub fn seed_preview(ctx: &Ctx, guild: u64) -> Result<()> {
    // Channels in the sample server sit at guild id + 10 * n: welcome 2, general 4, pets 6, suggestions 7, mod-log 8.
    let ch = |n: u64| guild + n * 10;
    let settings = Settings { channel: Some(ch(8)), ignored_channels: vec![ch(6)], ..Settings::default() };
    ctx.save_settings(guild, crate::KEY, &settings)?;

    let m = |i: usize| bot_core::preview::MEMBERS[i % bot_core::preview::MEMBERS.len()];
    let min = 60_000i64;
    // (minutes ago, kind, member, channel, summary)
    let rows: &[(i64, Kind, usize, Option<u64>, &str)] = &[
        (3, Kind::MessageDelete, 3, Some(ch(4)), "anyone know if the stream is still on tonight"),
        (9, Kind::Join, 7, None, "New account, created 2 days ago"),
        (14, Kind::MessageEdit, 0, Some(ch(4)), "see you at 8 to see you at 9"),
        (26, Kind::Roles, 2, None, "Added Regulars"),
        (41, Kind::Join, 5, None, "Account created 2 years ago"),
        (58, Kind::Nickname, 4, None, "Kasia to Kasia (afk)"),
        (95, Kind::Timeout, 6, None, "Timed out for 10 min"),
        (96, Kind::MessageDelete, 6, Some(ch(4)), "Content not available"),
        (130, Kind::BulkDelete, 1, Some(ch(4)), "14 messages deleted"),
        (190, Kind::Leave, 8, None, "Had 2 roles"),
        (240, Kind::Boost, 9, None, "Started boosting"),
        (310, Kind::MessageEdit, 1, Some(ch(7)), "Now: add a dark theme to the pet page"),
        (420, Kind::Roles, 0, None, "Added Mods, removed Regulars"),
        (600, Kind::Ban, 7, None, "Banned"),
        (640, Kind::MessageDelete, 5, Some(ch(2)), "IMG_2231.png"),
        (700, Kind::Join, 3, None, "Account created 5 months ago"),
        (1300, Kind::Leave, 4, None, "Had no roles"),
        (1500, Kind::TimeoutEnd, 6, None, "Timeout removed"),
        (1700, Kind::Unban, 7, None, "Unbanned"),
        (1900, Kind::Join, 8, None, "New account, created 5 days ago"),
        (2100, Kind::MessageDelete, 2, Some(ch(4)), "that was fast"),
        (2900, Kind::Nickname, 9, None, "None to Pavel"),
        (3300, Kind::Leave, 7, None, "Had 1 role"),
        (4000, Kind::Join, 1, None, "Account created 3 years ago"),
        (4600, Kind::MessageEdit, 4, Some(ch(4)), "its fine to its fine!"),
        (5900, Kind::Roles, 5, None, "Added Idleon"),
        (7000, Kind::MessageDelete, 3, Some(ch(4)), "Content not available"),
        (8800, Kind::Leave, 2, None, "Had 3 roles"),
        (9700, Kind::Join, 2, None, "Account created 8 months ago"),
        (12000, Kind::BulkDelete, 6, Some(ch(2)), "6 messages deleted"),
        (14000, Kind::Join, 0, None, "Account created 4 years ago"),
        (17000, Kind::MessageDelete, 9, Some(ch(4)), "wrong channel sorry"),
        (19500, Kind::Leave, 3, None, "Had 2 roles"),
    ];
    let now = bot_core::now_ms();
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    for (ago, kind, who, channel, summary) in rows {
        let (uid, name) = m(*who);
        let user = if *kind == Kind::BulkDelete { None } else { Some(uid as i64) };
        let name = if user.is_some() { name } else { "" };
        tx.execute(
            "INSERT INTO audit_events (guild_id, at, kind, user_id, user_name, channel_id, summary) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![guild as i64, now - ago * min, kind.key(), user, name, channel.map(|c| c as i64), summary],
        )?;
    }
    tx.commit()?;
    Ok(())
}
