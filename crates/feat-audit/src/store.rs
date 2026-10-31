//! The last two weeks of entries, for the dashboard.

use bot_core::Ctx;
use bot_core::anyhow::Result;
use bot_core::rusqlite::{ToSql, params, params_from_iter};
use bot_core::twilight_model::guild::audit_log::{AuditLogChange, AuditLogEntry, AuditLogEventType};

use crate::{Kind, RoleChange, RoleRef, Settings, diff_json};

#[derive(Debug, Clone)]
pub struct Row {
    pub at: i64,
    pub kind: Kind,
    pub user_id: Option<u64>,
    pub user_name: String,
    pub channel_id: Option<u64>,
    pub summary: String,
}

/// The filter chips on the page: everything, or one group of event kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    All,
    Messages,
    Members,
    Roles,
    Bans,
}

impl Group {
    pub const ALL: [Group; 5] = [Group::All, Group::Messages, Group::Members, Group::Roles, Group::Bans];

    pub fn key(self) -> &'static str {
        match self {
            Group::All => "",
            Group::Messages => "messages",
            Group::Members => "members",
            Group::Roles => "roles",
            Group::Bans => "bans",
        }
    }

    pub fn from_key(key: &str) -> Option<Group> {
        Group::ALL.into_iter().find(|g| g.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Group::All => "Everything",
            Group::Messages => "Messages",
            Group::Members => "Joins and leaves",
            Group::Roles => "Roles",
            Group::Bans => "Bans and timeouts",
        }
    }

    pub fn kinds(self) -> &'static [Kind] {
        match self {
            Group::All => &[],
            Group::Messages => &[Kind::MessageDelete, Kind::MessageEdit, Kind::BulkDelete],
            Group::Members => &[Kind::Join, Kind::Leave],
            Group::Roles => &[Kind::Roles],
            Group::Bans => &[Kind::Ban, Kind::Unban, Kind::Timeout, Kind::TimeoutEnd],
        }
    }
}

#[derive(Debug, Clone)]
pub struct Filter {
    pub group: Group,
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
    ctx.db.with(|c| {
        c.execute("DELETE FROM audit_events WHERE at < ?1", [before])?;
        c.execute("DELETE FROM audit_mod_actions WHERE at < ?1", [before])
    })?;
    Ok(())
}

/// A moderator's action, from Discord's audit log: `ban`, `unban`, `kick`, `timeout`,
/// `untimeout`, `delete` or `roles`.
#[derive(Debug, Clone)]
pub struct ModAction {
    pub at: i64,
    pub moderator: u64,
    pub action: String,
    pub target: Option<u64>,
}

/// Remember who did what, for the actions moderators take by hand. The bot's own actions
/// (scam protection's timeouts and bans) are left out.
pub fn record_moderator(ctx: &Ctx, e: &AuditLogEntry) -> Result<()> {
    let (Some(guild), Some(moderator)) = (e.guild_id, e.user_id) else { return Ok(()) };
    if ctx.bot_id() == Some(moderator.get()) {
        return Ok(());
    }
    let action = match e.action_type {
        AuditLogEventType::MemberBanAdd => "ban",
        AuditLogEventType::MemberBanRemove => "unban",
        AuditLogEventType::MemberKick => "kick",
        AuditLogEventType::MessageDelete => "delete",
        AuditLogEventType::MemberRoleUpdate => "roles",
        AuditLogEventType::MemberUpdate => {
            match e.changes.iter().find_map(|c| match c {
                AuditLogChange::CommunicationDisabledUntil { new, .. } => Some(new.is_some()),
                _ => None,
            }) {
                Some(true) => "timeout",
                Some(false) => "untimeout",
                None => return Ok(()),
            }
        }
        _ => return Ok(()),
    };
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO audit_mod_actions (guild_id, at, moderator_id, action, target_id) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![guild.get() as i64, bot_core::now_ms(), moderator.get() as i64, action, e.target_id.map(|t| t.get() as i64)],
        )
    })?;
    Ok(())
}

/// Moderator actions since `since`, newest first.
pub fn mod_actions(ctx: &Ctx, guild: u64, since: i64) -> Result<Vec<ModAction>> {
    ctx.db.with(|c| {
        let mut st = c.prepare_cached(
            "SELECT at, moderator_id, action, target_id FROM audit_mod_actions WHERE guild_id = ?1 AND at >= ?2 ORDER BY at DESC LIMIT 2000",
        )?;
        st.query_map(params![guild as i64, since], |r| {
            Ok(ModAction { at: r.get(0)?, moderator: r.get::<_, i64>(1)? as u64, action: r.get(2)?, target: r.get::<_, Option<i64>>(3)?.map(|t| t as u64) })
        })?
        .collect()
    })
}

/// The moderator behind a logged event: the same kind of action on the same member within a
/// minute of it.
pub fn moderator_for(actions: &[ModAction], kind: Kind, target: Option<u64>, at: i64) -> Option<u64> {
    let want = match kind {
        Kind::Ban => "ban",
        Kind::Unban => "unban",
        Kind::Timeout => "timeout",
        Kind::TimeoutEnd => "untimeout",
        Kind::MessageDelete => "delete",
        Kind::Roles => "roles",
        Kind::Leave => "kick",
        _ => return None,
    };
    let target = target?;
    actions.iter().find(|a| a.action == want && a.target == Some(target) && (a.at - at).abs() < 60_000).map(|a| a.moderator)
}

fn where_clause(guild: u64, f: &Filter) -> (String, Vec<Box<dyn ToSql>>) {
    let mut sql = "guild_id = ?1".to_owned();
    let mut args: Vec<Box<dyn ToSql>> = vec![Box::new(guild as i64)];
    let kinds = f.group.kinds();
    if !kinds.is_empty() {
        // The keys are fixed words from `Kind::key`, so they can go in as text.
        let list = kinds.iter().map(|k| format!("'{}'", k.key())).collect::<Vec<_>>().join(", ");
        sql.push_str(&format!(" AND kind IN ({list})"));
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

/// What happened since the start of the viewer's day.
#[derive(Debug, Clone, Copy, Default)]
pub struct Today {
    pub joined: i64,
    pub left: i64,
    pub edits: i64,
    pub deleted: i64,
}

pub fn today(ctx: &Ctx, guild: u64, since: i64) -> Result<Today> {
    ctx.db.with(|c| {
        c.query_row(
            "SELECT COALESCE(SUM(kind = 'join'), 0), COALESCE(SUM(kind = 'leave'), 0), COALESCE(SUM(kind = 'message_edit'), 0), COALESCE(SUM(kind = 'message_delete'), 0)
             FROM audit_events WHERE guild_id = ?1 AND at >= ?2",
            params![guild as i64, since],
            |r| Ok(Today { joined: r.get(0)?, left: r.get(1)?, edits: r.get(2)?, deleted: r.get(3)? }),
        )
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
    let roles = |added: &[(&str, u32)], removed: &[(&str, u32)]| {
        let refs = |list: &[(&str, u32)]| list.iter().map(|(name, color)| RoleRef { name: (*name).to_owned(), color: *color }).collect();
        bot_core::serde_json::to_string(&RoleChange { added: refs(added), removed: refs(removed) }).unwrap_or_default()
    };
    // (minutes ago, kind, member, channel, summary)
    let rows: Vec<(i64, Kind, usize, Option<u64>, String)> = vec![
        (3, Kind::MessageDelete, 3, Some(ch(4)), "anyone know if the stream is still on tonight".into()),
        (9, Kind::Join, 7, None, "New account, made 2 days ago".into()),
        (14, Kind::MessageEdit, 0, Some(ch(4)), diff_json(Some("see you at 8"), Some("see you at 9"))),
        (26, Kind::Roles, 2, None, roles(&[("Regulars", 0x4fc38a)], &[])),
        (41, Kind::Join, 5, None, "Account made 3 years ago".into()),
        (58, Kind::Nickname, 4, None, diff_json(Some("Kasia"), Some("Kasia (afk)"))),
        (95, Kind::Timeout, 6, None, "For 10 min".into()),
        (96, Kind::MessageDelete, 6, Some(ch(4)), "Content not available".into()),
        (130, Kind::BulkDelete, 1, Some(ch(4)), "14 messages deleted".into()),
        (190, Kind::Leave, 8, None, "Had 2 roles".into()),
        (240, Kind::Boost, 9, None, String::new()),
        (310, Kind::MessageEdit, 1, Some(ch(7)), diff_json(None, Some("add a dark theme to the pet page"))),
        (420, Kind::Roles, 0, None, roles(&[("Mods", 0x5865f2)], &[("Regulars", 0x4fc38a)])),
        (600, Kind::Ban, 7, None, String::new()),
        (640, Kind::MessageDelete, 5, Some(ch(2)), "IMG_2231.png".into()),
        (700, Kind::Join, 3, None, "Account made 5 months ago".into()),
        (1300, Kind::Leave, 4, None, "Had no roles".into()),
        (1500, Kind::TimeoutEnd, 6, None, String::new()),
        (1700, Kind::Unban, 7, None, String::new()),
        (1900, Kind::Join, 8, None, "New account, made 5 days ago".into()),
        (2100, Kind::MessageDelete, 2, Some(ch(4)), "that was fast".into()),
        (2900, Kind::Nickname, 9, None, diff_json(None, Some("Pavel"))),
        (3300, Kind::Leave, 7, None, "Had 1 role".into()),
        (4000, Kind::Join, 1, None, "Account made 3 years ago".into()),
        (4600, Kind::MessageEdit, 4, Some(ch(4)), diff_json(Some("its fine"), Some("its fine!"))),
        (5900, Kind::Roles, 5, None, roles(&[("Idleon", 0x9b84ee)], &[])),
        (7000, Kind::MessageDelete, 3, Some(ch(4)), "Content not available".into()),
        (8800, Kind::Leave, 2, None, "Had 3 roles".into()),
        (9700, Kind::Join, 2, None, "Account made 8 months ago".into()),
        (12000, Kind::BulkDelete, 6, Some(ch(2)), "6 messages deleted".into()),
        (14000, Kind::Join, 0, None, "Account made 4 years ago".into()),
        (17000, Kind::MessageDelete, 9, Some(ch(4)), "wrong channel sorry".into()),
        (19500, Kind::Leave, 3, None, "Had 2 roles".into()),
    ];
    let now = bot_core::now_ms();
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    for (ago, kind, who, channel, summary) in &rows {
        let (uid, name) = m(*who);
        let user = if *kind == Kind::BulkDelete { None } else { Some(uid as i64) };
        let name = if user.is_some() { name } else { "" };
        tx.execute(
            "INSERT INTO audit_events (guild_id, at, kind, user_id, user_name, channel_id, summary) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![guild as i64, now - ago * min, kind.key(), user, name, channel.map(|c| c as i64), summary],
        )?;
    }
    // Who did the moderation, as Discord's audit log would report it: (minutes ago, moderator, action, member).
    let mods: [(i64, usize, &str, usize); 9] = [
        (95, 1, "timeout", 6), (96, 1, "delete", 6), (420, 1, "roles", 0), (600, 0, "ban", 7), (1500, 1, "untimeout", 6),
        (1700, 0, "unban", 7), (2100, 1, "delete", 2), (5900, 3, "roles", 5), (7000, 0, "delete", 3),
    ];
    for (ago, moderator, action, target) in mods {
        tx.execute(
            "INSERT INTO audit_mod_actions (guild_id, at, moderator_id, action, target_id) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![guild as i64, now - ago * min, m(moderator).0 as i64, action, m(target).0 as i64],
        )?;
    }
    tx.commit()?;
    Ok(())
}
