//! The last two weeks of entries, for the dashboard.

use bot_core::Ctx;
use bot_core::anyhow::Result;
use bot_core::twilight_model::id::Id;
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
    /// Who did it, as decided when the entry was posted, and why.
    pub moderator: Option<u64>,
    pub reason: Option<String>,
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
    pub search: Option<Search>,
}

/// A search, and what it matches outside the stored rows: names and channels as the page shows
/// them now, from the cache.
#[derive(Debug, Clone, Default)]
pub struct Search {
    /// As typed, trimmed. SQLite's `lower()` only folds ASCII, so the stored text and this are
    /// both lowered there, the same way.
    pub text: String,
    /// Members whose name now contains the text.
    pub users: Vec<u64>,
    /// Channels whose name contains it.
    pub channels: Vec<u64>,
    /// Moderators whose name contains it.
    pub moderators: Vec<u64>,
    /// Kinds of event whose label contains it ("banned", "joined").
    pub kinds: Vec<Kind>,
}

impl Search {
    pub fn new(ctx: &Ctx, guild: u64, q: &str) -> Search {
        let text = q.trim().to_owned();
        let lower = text.to_lowercase();
        let has = |name: &str| name.to_lowercase().contains(&lower);
        let g = Id::new(guild);
        let users = ctx
            .cache
            .guild_members(g)
            .map(|set| set.iter().map(|u| u.get()).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .filter(|u| has(&ctx.display_name(guild, *u)))
            .take(500)
            .collect();
        let bare = lower.trim_start_matches('#');
        let channels = ctx
            .cache
            .guild_channels(g)
            .map(|set| set.iter().copied().collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .filter(|c| !bare.is_empty() && ctx.cache.channel(*c).and_then(|c| c.name.clone()).is_some_and(|n| n.to_lowercase().contains(bare)))
            .map(|c| c.get())
            .collect();
        let moderators = moderator_ids(ctx, guild).unwrap_or_default().into_iter().filter(|m| has(&ctx.display_name(guild, *m))).collect();
        let kinds = Kind::ALL.into_iter().filter(|k| has(k.label())).collect();
        Search { text, users, channels, moderators, kinds }
    }
}

/// Everyone named as the moderator behind an entry in the last two weeks.
fn moderator_ids(ctx: &Ctx, guild: u64) -> Result<Vec<u64>> {
    ctx.db.with(|c| {
        let mut st = c.prepare_cached("SELECT DISTINCT moderator_id FROM audit_events WHERE guild_id = ?1 AND moderator_id IS NOT NULL")?;
        st.query_map([guild as i64], |r| Ok(r.get::<_, i64>(0)? as u64))?.collect()
    })
}

/// Store an entry. Returns its id, to add the moderator to once Discord says who it was.
pub fn record(ctx: &Ctx, guild: u64, kind: Kind, user: Option<u64>, user_name: &str, channel: Option<u64>, summary: &str) -> Result<i64> {
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO audit_events (guild_id, at, kind, user_id, user_name, channel_id, summary) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![guild as i64, bot_core::now_ms(), kind.key(), user.map(|u| u as i64), user_name, channel.map(|c| c as i64), summary],
        )?;
        Ok(c.last_insert_rowid())
    })
}

/// Drop a member's entries of one kind stored since `since`: a leave the ban that followed
/// speaks for.
pub fn forget(ctx: &Ctx, guild: u64, kind: Kind, user: u64, since: i64) -> Result<()> {
    ctx.db.with(|c| {
        c.execute(
            "DELETE FROM audit_events WHERE guild_id = ?1 AND kind = ?2 AND user_id = ?3 AND at >= ?4",
            params![guild as i64, kind.key(), user as i64, since],
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
    pub id: i64,
    pub at: i64,
    pub moderator: u64,
    pub action: String,
    pub target: Option<u64>,
    /// For a deleted message, the channel it was in.
    pub channel: Option<u64>,
    pub reason: Option<String>,
    /// How many events the entry explains: one, or for deleted messages Discord's count.
    pub covers: i64,
    /// How many of those it has explained already.
    pub used: i64,
}

/// Remember who did what. The bot's own actions (scam protection, reward roles) are kept too, so
/// they aren't put down to a moderator who acted on the same member a moment before.
pub fn record_moderator(ctx: &Ctx, e: &AuditLogEntry) -> Result<()> {
    let (Some(guild), Some(moderator)) = (e.guild_id, e.user_id) else { return Ok(()) };
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
    let channel = e.options.as_ref().and_then(|o| o.channel_id).map(|c| c.get() as i64);
    let reason = e.reason.as_deref().map(str::trim).filter(|r| !r.is_empty()).map(|r| bot_core::text::clip(r, 300));
    // Discord counts the messages one entry covers; anything else is one event.
    let covers = match e.action_type {
        AuditLogEventType::MessageDelete => e.options.as_ref().and_then(|o| o.count.as_deref()).and_then(|n| n.parse::<i64>().ok()).filter(|n| *n > 0).unwrap_or(1),
        _ => 1,
    };
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO audit_mod_actions (guild_id, at, moderator_id, action, target_id, channel_id, reason, covers, entry_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![guild.get() as i64, bot_core::now_ms(), moderator.get() as i64, action, e.target_id.map(|t| t.get() as i64), channel, reason, covers, e.id.get() as i64],
        )
    })?;
    Ok(())
}

fn mod_action(r: &bot_core::rusqlite::Row<'_>) -> bot_core::rusqlite::Result<ModAction> {
    Ok(ModAction {
        id: r.get(0)?,
        at: r.get(1)?,
        moderator: r.get::<_, i64>(2)? as u64,
        action: r.get(3)?,
        target: r.get::<_, Option<i64>>(4)?.map(|t| t as u64),
        channel: r.get::<_, Option<i64>>(5)?.map(|c| c as u64),
        reason: r.get(6)?,
        covers: r.get(7)?,
        used: r.get(8)?,
    })
}

/// Moderator actions since `since` that can still explain an event, newest first.
pub fn mod_actions(ctx: &Ctx, guild: u64, since: i64) -> Result<Vec<ModAction>> {
    ctx.db.with(|c| {
        let mut st = c.prepare_cached(
            "SELECT id, at, moderator_id, action, target_id, channel_id, reason, covers, used FROM audit_mod_actions
             WHERE guild_id = ?1 AND at >= ?2 AND used < covers ORDER BY at DESC LIMIT 2000",
        )?;
        st.query_map(params![guild as i64, since], mod_action)?.collect()
    })
}

/// How long Discord goes on adding a moderator's deletes of one member's messages in one channel
/// to the entry it already sent, raising its count instead of sending a new one. Only the
/// first delete comes as an entry, so later ones find the entry used up.
pub const MERGE_MS: i64 = 10 * 60_000;

/// Message delete entries since `since`, newest first, each with Discord's id for it: any may
/// have gone on to cover more deletes than it said, whether or not its uses so far were taken
/// (a delete the bot didn't log leaves one untaken).
pub fn recent_deletes(ctx: &Ctx, guild: u64, since: i64) -> Result<Vec<(ModAction, u64)>> {
    ctx.db.with(|c| {
        let mut st = c.prepare_cached(
            "SELECT id, at, moderator_id, action, target_id, channel_id, reason, covers, used, entry_id FROM audit_mod_actions
             WHERE guild_id = ?1 AND at >= ?2 AND action = 'delete' AND entry_id IS NOT NULL ORDER BY at DESC LIMIT 50",
        )?;
        st.query_map(params![guild as i64, since], |r| Ok((mod_action(r)?, r.get::<_, i64>(9)? as u64)))?.collect()
    })
}

/// Discord's count on an entry went up: it explains that many events now. Never lowered.
pub fn raise_covers(ctx: &Ctx, action: i64, covers: i64) -> Result<()> {
    ctx.db.with(|c| c.execute("UPDATE audit_mod_actions SET covers = MAX(covers, ?2) WHERE id = ?1", params![action, covers]))?;
    Ok(())
}

/// Use up one event of an action, and when `row` is given, name its moderator and reason on that
/// stored entry so the dashboard shows what the post said. False when another event took the
/// action's last use first.
pub fn use_action(ctx: &Ctx, action: &ModAction, row: Option<i64>, named: bool) -> Result<bool> {
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    let n = tx.execute("UPDATE audit_mod_actions SET used = used + 1 WHERE id = ?1 AND used < covers", [action.id])?;
    if n == 1
        && named
        && let Some(row) = row
    {
        tx.execute("UPDATE audit_events SET moderator_id = ?1, reason = ?2 WHERE id = ?3", params![action.moderator as i64, action.reason, row])?;
    }
    tx.commit()?;
    Ok(n == 1)
}

/// Moderators by how many actions they took since `since`, most first, leaving out `skip` (the bot).
pub fn moderator_counts(ctx: &Ctx, guild: u64, since: i64, skip: Option<u64>) -> Result<Vec<(u64, i64)>> {
    ctx.db.with(|c| {
        let mut st = c.prepare_cached(
            "SELECT moderator_id, COUNT(*) AS n FROM audit_mod_actions WHERE guild_id = ?1 AND at >= ?2 AND moderator_id != ?3
             GROUP BY moderator_id ORDER BY n DESC, MAX(at) DESC LIMIT 12",
        )?;
        st.query_map(params![guild as i64, since, skip.map_or(-1, |s| s as i64)], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?)))?.collect()
    })
}

/// How long before an event its audit entry may have come in. A leave is logged after waiting a
/// moment for its ban, so the kick behind it is a little older than the entry.
pub const MATCH_BEFORE_MS: i64 = 3_000;
/// How long after an event its audit entry may come in. A post waits this long for it.
pub const MATCH_AFTER_MS: i64 = 5_000;

/// A logged event a moderator may be behind: what happened, to whom, where and when.
#[derive(Debug, Clone, Copy)]
pub struct Logged {
    pub kind: Kind,
    pub target: u64,
    pub channel: Option<u64>,
    pub at: i64,
}

/// Whether an action could be behind an event: the same kind of action on the same member (and,
/// for a deleted message, in the same channel), from a few seconds around the event. Uses left
/// aren't looked at here.
fn could_explain(a: &ModAction, e: &Logged) -> bool {
    let want = match e.kind {
        Kind::Ban => "ban",
        Kind::Unban => "unban",
        Kind::Timeout => "timeout",
        Kind::TimeoutEnd => "untimeout",
        Kind::MessageDelete => "delete",
        Kind::Roles => "roles",
        Kind::Leave => "kick",
        _ => return false,
    };
    // Discord says which channel a deleted message was in; a delete elsewhere is someone else's.
    let same_channel = e.kind != Kind::MessageDelete || a.channel.is_none() || e.channel.is_none() || a.channel == e.channel;
    a.action == want && a.target == Some(e.target) && same_channel && (-MATCH_BEFORE_MS..=MATCH_AFTER_MS).contains(&(a.at - e.at))
}

/// Put each event down to the moderator action behind it, if any, and count each action as used
/// so it explains no more events than it covers. Matched across all the events at once, closest
/// pairs first, an entry at or after its event before an earlier one: so two moderators acting
/// on one member moments apart each get their own event, and a member's own delete a moment
/// before a moderator's doesn't take the moderator's entry because it was queued first.
pub fn assign(actions: &mut [ModAction], events: &[Logged]) -> Vec<Option<ModAction>> {
    let mut pairs: Vec<((bool, i64), usize, usize)> = Vec::new();
    for (ei, e) in events.iter().enumerate() {
        for (ai, a) in actions.iter().enumerate() {
            if a.used < a.covers && could_explain(a, e) {
                pairs.push(((a.at < e.at, (a.at - e.at).abs()), ei, ai));
            }
        }
    }
    // Ties go to the earlier event, then the action listed first.
    pairs.sort_unstable();
    let mut out: Vec<Option<ModAction>> = vec![None; events.len()];
    for (_, ei, ai) in pairs {
        let a = &mut actions[ai];
        if out[ei].is_some() || a.used >= a.covers {
            continue;
        }
        a.used += 1;
        out[ei] = Some(a.clone());
    }
    out
}

/// [`assign`] for one event.
#[cfg(test)]
pub fn claim(actions: &mut [ModAction], kind: Kind, target: u64, channel: Option<u64>, at: i64) -> Option<ModAction> {
    assign(actions, &[Logged { kind, target, channel, at }]).pop().flatten()
}

/// `'a', 'b'`: the keys are fixed words from `Kind::key`, so they can go in as text.
fn kind_list(kinds: &[Kind]) -> String {
    kinds.iter().map(|k| format!("'{}'", k.key())).collect::<Vec<_>>().join(", ")
}

/// `1, 2, 3`, or `NULL` for none (so `IN (NULL)` matches nothing).
fn id_list(ids: &[u64]) -> String {
    if ids.is_empty() { "NULL".into() } else { ids.iter().map(|i| (*i as i64).to_string()).collect::<Vec<_>>().join(", ") }
}

/// What a search matches, in the words the page shows: the member's name then or now, their id,
/// the channel, the event's label, the deleted text, an edit's or nickname's before and after,
/// the roles' names, and the moderator named on it and their reason.
const SEARCH_SQL: &str = "
    instr(lower(e.user_name), ?Q) > 0 OR e.user_id = ?N OR e.user_id IN ({users}) OR e.channel_id IN ({channels}) OR e.kind IN ({kinds})
    OR (e.kind IN ('message_edit', 'nickname') AND json_valid(e.summary)
        AND (instr(lower(COALESCE(json_extract(e.summary, '$.before'), '')), ?Q) > 0
             OR instr(lower(COALESCE(json_extract(e.summary, '$.after'), '')), ?Q) > 0))
    OR (e.kind = 'roles' AND json_valid(e.summary) AND (
        EXISTS (SELECT 1 FROM json_each(e.summary, '$.added') j WHERE j.type = 'object' AND instr(lower(json_extract(j.value, '$.name')), ?Q) > 0)
        OR EXISTS (SELECT 1 FROM json_each(e.summary, '$.removed') j WHERE j.type = 'object' AND instr(lower(json_extract(j.value, '$.name')), ?Q) > 0)))
    -- Plain summaries, including edits, nicknames and role changes stored as text by older versions.
    OR ((e.kind NOT IN ('message_edit', 'nickname', 'roles') OR NOT json_valid(e.summary)) AND instr(lower(e.summary), ?Q) > 0)
    OR e.moderator_id IN ({moderators}) OR e.moderator_id = ?N OR instr(lower(COALESCE(e.reason, '')), ?Q) > 0";

pub(crate) fn where_clause(guild: u64, f: &Filter) -> (String, Vec<Box<dyn ToSql>>) {
    let mut sql = "e.guild_id = ?1".to_owned();
    let mut args: Vec<Box<dyn ToSql>> = vec![Box::new(guild as i64)];
    let kinds = f.group.kinds();
    if !kinds.is_empty() {
        sql.push_str(&format!(" AND e.kind IN ({})", kind_list(kinds)));
    }
    if let Some(q) = f.search.as_ref().filter(|q| !q.text.is_empty()) {
        args.push(Box::new(q.text.clone()));
        args.push(Box::new(q.text.parse::<i64>().unwrap_or(-1)));
        let kinds = if q.kinds.is_empty() { "NULL".to_owned() } else { kind_list(&q.kinds) };
        let clause = SEARCH_SQL
            .replace("?Q", "lower(?2)")
            .replace("?N", "?3")
            .replace("{users}", &id_list(&q.users))
            .replace("{channels}", &id_list(&q.channels))
            .replace("{moderators}", &id_list(&q.moderators))
            .replace("{kinds}", &kinds);
        sql.push_str(&format!(" AND ({clause})"));
    }
    (sql, args)
}

/// The newest entries matching the filter, and how many match in all.
pub fn recent(ctx: &Ctx, guild: u64, f: &Filter, limit: i64) -> Result<(Vec<Row>, i64)> {
    let (clause, args) = where_clause(guild, f);
    ctx.db.with(|c| {
        let total: i64 = c.query_row(&format!("SELECT COUNT(*) FROM audit_events e WHERE {clause}"), params_from_iter(args.iter().map(|a| a.as_ref())), |r| r.get(0))?;
        let mut stmt = c.prepare(&format!(
            "SELECT e.at, e.kind, e.user_id, e.user_name, e.channel_id, e.summary, e.moderator_id, e.reason FROM audit_events e WHERE {clause} ORDER BY e.at DESC, e.id DESC LIMIT {limit}"
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
                    moderator: r.get::<_, Option<i64>>(6)?.map(|m| m as u64),
                    reason: r.get(7)?,
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

/// Entries in each of the last `days` rolling days before `now`, oldest first, for the rail's chart.
pub fn daily(ctx: &Ctx, guild: u64, now: i64, days: usize) -> Result<Vec<i64>> {
    const DAY: i64 = 86_400_000;
    let mut out = vec![0; days];
    ctx.db.with(|c| {
        let mut st = c.prepare_cached("SELECT (?2 - at) / ?3, COUNT(*) FROM audit_events WHERE guild_id = ?1 AND at > ?2 - ?3 * ?4 AND at <= ?2 GROUP BY 1")?;
        let rows = st.query_map(params![guild as i64, now, DAY, days as i64], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (ago, n) = row?;
            if let Some(slot) = usize::try_from(ago).ok().filter(|a| *a < days) {
                out[days - 1 - slot] += n;
            }
        }
        Ok(())
    })?;
    Ok(out)
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
    // Who did the moderation, as Discord's audit log would report it: (minutes ago, moderator, action, member, reason).
    let mods: [(i64, usize, &str, usize, Option<&str>); 9] = [
        (95, 1, "timeout", 6, Some("Arguing after a warning")), (96, 1, "delete", 6, None), (420, 1, "roles", 0, None),
        (600, 0, "ban", 7, Some("Selling accounts in DMs")), (1500, 1, "untimeout", 6, None), (1700, 0, "unban", 7, Some("Appealed")),
        (2100, 1, "delete", 2, None), (5900, 3, "roles", 5, None), (7000, 0, "delete", 3, None),
    ];
    for (ago, moderator, action, target, reason) in mods {
        let (at, moderator, target) = (now - ago * min, m(moderator).0 as i64, m(target).0 as i64);
        tx.execute(
            "INSERT INTO audit_mod_actions (guild_id, at, moderator_id, action, target_id, reason, used) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)",
            params![guild as i64, at, moderator, action, target, reason],
        )?;
        // The entry it explained, as the post would have named it.
        tx.execute("UPDATE audit_events SET moderator_id = ?1, reason = ?2 WHERE guild_id = ?3 AND at = ?4 AND user_id = ?5", params![moderator, reason, guild as i64, at, target])?;
    }
    tx.commit()?;
    Ok(())
}
