//! Scam protection: scores every message for scams (lookalike links, scam wording, the
//! compromised-account repeat across channels) and treats any message in the honeypot channel as
//! a catch. A catch deletes the member's recent messages, times them out and alerts staff with
//! buttons to release or ban. Everything else (caps, invites, word lists) is left to Discord's
//! own AutoMod.

pub mod links;
pub mod recent;
pub mod score;
mod web;

use std::sync::Mutex;

use bot_core::anyhow::Result;
use bot_core::discord::{ComponentCall, Reply, button_row};
use bot_core::rusqlite::{OptionalExtension, params};
use bot_core::twilight_http::request::AuditLogReason as _;
use bot_core::twilight_model::channel::message::component::{ButtonStyle, Component};
use bot_core::twilight_model::channel::message::{Embed, Message};
use bot_core::twilight_model::gateway::event::Event;
use bot_core::twilight_model::guild::audit_log::{AuditLogChange, AuditLogEntry, AuditLogEventType};
use bot_core::twilight_model::guild::Permissions;
use bot_core::twilight_model::id::Id;
use bot_core::twilight_model::id::marker::{ChannelMarker, MessageMarker};
use bot_core::twilight_model::util::Timestamp;
use bot_core::twilight_util::builder::embed::{EmbedBuilder, EmbedFieldBuilder, EmbedFooterBuilder};
use bot_core::{Ctx, Feature, FeatureMeta, Group, Icon, Migration, NavItem, Need, Overview, feature_async, now_ms, text, tracing};
use serde::{Deserialize, Serialize};

use recent::Recent;
use score::{Facts, Signal, Strictness};

pub const KEY: &str = "guard";
const COLOR: u32 = 0xf0806a;
const DAY_MS: i64 = 86_400_000;
/// Discord's longest timeout is 28 days.
const MAX_TIMEOUT_MIN: i64 = 28 * 24 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub scam_detection: bool,
    pub honeypot_channel: Option<u64>,
    pub log_channel: Option<u64>,
    pub timeout_minutes: i64,
    pub strictness: Strictness,
    pub new_account_days: i64,
    pub exempt_roles: Vec<u64>,
    pub exempt_channels: Vec<u64>,
    pub dm_member: bool,
    /// Also score commission, art and service ads.
    pub catch_ads: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            scam_detection: true,
            honeypot_channel: None,
            log_channel: None,
            timeout_minutes: 24 * 60,
            strictness: Strictness::Normal,
            new_account_days: 7,
            exempt_roles: Vec::new(),
            exempt_channels: Vec::new(),
            dm_member: false,
            catch_ads: false,
        }
    }
}

/// Timeout lengths offered on the dashboard, in minutes.
pub const TIMEOUTS: &[(i64, &str)] = &[
    (60, "1 hour"),
    (6 * 60, "6 hours"),
    (12 * 60, "12 hours"),
    (24 * 60, "1 day"),
    (3 * 24 * 60, "3 days"),
    (7 * 24 * 60, "7 days"),
    (MAX_TIMEOUT_MIN, "28 days"),
];

pub fn timeout_label(minutes: i64) -> String {
    TIMEOUTS.iter().find(|(m, _)| *m == minutes).map(|(_, l)| (*l).to_owned()).unwrap_or_else(|| text::duration(minutes * 60))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Messages deleted and timed out.
    TimedOut,
    /// Messages deleted, but the timeout failed (missing permission, higher role).
    NotTimedOut,
    /// Staff lifted it: a mistake.
    Released,
    Banned,
}

impl Outcome {
    fn key(self) -> &'static str {
        match self {
            Outcome::TimedOut => "timed_out",
            Outcome::NotTimedOut => "not_timed_out",
            Outcome::Released => "released",
            Outcome::Banned => "banned",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "released" => Outcome::Released,
            "banned" => Outcome::Banned,
            "not_timed_out" => Outcome::NotTimedOut,
            _ => Outcome::TimedOut,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Outcome::TimedOut => "Timed out",
            Outcome::NotTimedOut => "Not timed out",
            Outcome::Released => "Released",
            Outcome::Banned => "Banned",
        }
    }
    pub fn open(self) -> bool {
        matches!(self, Outcome::TimedOut | Outcome::NotTimedOut)
    }
}

#[derive(Debug, Clone)]
pub struct Catch {
    pub id: i64,
    pub at: i64,
    pub user_id: u64,
    pub user_name: String,
    pub honeypot: bool,
    pub score: i64,
    pub reason: String,
    pub signals: Vec<Signal>,
    pub content: String,
    pub channel_id: u64,
    pub deleted: i64,
    pub timeout_until: Option<i64>,
    pub outcome: Outcome,
    pub acted_by: Option<u64>,
    pub acted_by_name: Option<String>,
    pub acted_at: Option<i64>,
    /// Why they were released or banned, when the moderator said.
    pub acted_reason: Option<String>,
    /// Released or banned with Discord's own tools rather than the alert or the dashboard.
    pub acted_in_discord: bool,
    pub alert: Option<(u64, u64)>,
}

const MIGRATIONS: &[Migration] = &[Migration {
    name: "001_guard",
    sql: "
        CREATE TABLE guard_catches (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            guild_id INTEGER NOT NULL, at INTEGER NOT NULL,
            user_id INTEGER NOT NULL, user_name TEXT NOT NULL,
            honeypot INTEGER NOT NULL DEFAULT 0, score INTEGER NOT NULL DEFAULT 0,
            reason TEXT NOT NULL, signals TEXT NOT NULL DEFAULT '[]', content TEXT NOT NULL DEFAULT '',
            channel_id INTEGER NOT NULL, deleted INTEGER NOT NULL DEFAULT 0,
            timeout_until INTEGER, outcome TEXT NOT NULL,
            acted_by INTEGER, acted_by_name TEXT, acted_at INTEGER,
            alert_channel INTEGER, alert_message INTEGER);
        CREATE INDEX guard_catches_by_guild ON guard_catches(guild_id, at DESC);
        CREATE INDEX guard_catches_by_user ON guard_catches(guild_id, user_id, at DESC);
    ",
}, Migration {
    name: "002_guard_reason",
    sql: "
        ALTER TABLE guard_catches ADD COLUMN acted_reason TEXT;
        ALTER TABLE guard_catches ADD COLUMN acted_in_discord INTEGER NOT NULL DEFAULT 0;
    ",
}];

#[derive(Default)]
pub struct Guard {
    recent: Mutex<Recent>,
}

#[feature_async]
impl Feature for Guard {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Scam protection",
            description: "Catches scam messages and anyone posting in the honeypot channel.",
            group: Group::Safety,
            nav: vec![NavItem { label: "Scam protection", path: KEY, icon: Icon::Shield, about: "Catches scam messages and anyone posting in the honeypot channel." }],
        }
    }

    fn migrations(&self) -> &'static [Migration] {
        MIGRATIONS
    }

    fn routes(&self) -> bot_core::axum::Router<Ctx> {
        web::routes()
    }

    async fn on_event(&self, ctx: &Ctx, event: &Event) -> Result<()> {
        match event {
            Event::MessageCreate(m) => self.on_message(ctx, m).await?,
            // A moderator lifted the timeout in Discord itself: count it as released.
            Event::MemberUpdate(u) if u.communication_disabled_until.is_none() => {
                let guild = u.guild_id.get();
                let had = ctx.before_member(guild, u.user.id.get()).and_then(|b| b.timed_out_until_ms).is_some_and(|t| t > now_ms());
                if had {
                    close_open(ctx, guild, u.user.id.get(), Outcome::Released, None).await?;
                }
            }
            Event::BanAdd(b) => close_open(ctx, b.guild_id.get(), b.user.id.get(), Outcome::Banned, None).await?,
            // Who did it and why, from Discord's audit log. It can come before or after the ban or
            // member update itself; the bot's own actions are already recorded by `resolve`.
            Event::GuildAuditLogEntryCreate(e) => {
                if let (Some(guild), Some(by), Some((user, outcome))) = (e.guild_id, e.user_id, native_decision(&e.0))
                    && ctx.bot_id() != Some(by.get())
                {
                    let reason = e.reason.as_deref().map(str::trim).filter(|r| !r.is_empty());
                    close_open(ctx, guild.get(), user, outcome, Some((by.get(), reason))).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn on_component(&self, ctx: &Ctx, call: &ComponentCall) -> Result<()> {
        let Some(guild) = call.guild_id() else { return Ok(()) };
        let (outcome, id) = match call.args().as_slice() {
            ["release", id] => (Outcome::Released, id.parse::<i64>()?),
            ["ban", id] => (Outcome::Banned, id.parse::<i64>()?),
            _ => return Ok(()),
        };
        let allowed = match outcome {
            Outcome::Banned => call.member_has(Permissions::BAN_MEMBERS),
            _ => call.member_has(Permissions::MODERATE_MEMBERS | Permissions::BAN_MEMBERS),
        };
        if !allowed {
            let need = if outcome == Outcome::Banned { "Ban Members" } else { "Moderate Members" };
            ctx.reply(&call.interaction, Reply::text(format!("You need the {need} permission for this.")).ephemeral()).await?;
            return Ok(());
        }
        ctx.defer_update(&call.interaction).await?;
        let by = ctx.display_name(guild, call.user_id());
        match resolve(ctx, guild, id, outcome, call.user_id(), &by, None, false).await {
            Ok(c) => {
                let embed = alert_embed(&c);
                ctx.edit_original(&call.interaction, Reply::embed(embed)).await?;
            }
            Err(msg) => ctx.followup(&call.interaction, Reply::text(msg).ephemeral()).await?,
        }
        Ok(())
    }

    async fn tick(&self, _ctx: &Ctx) -> Result<()> {
        self.recent.lock().unwrap().prune(now_ms());
        Ok(())
    }

    fn seed_preview(&self, ctx: &Ctx, guild: u64) -> Result<()> {
        seed(ctx, guild)
    }

    fn overview(&self, ctx: &Ctx, page: &bot_core::web::Page) -> Overview {
        // Caught and still timed out (or not timed out at all) and nobody has released or banned.
        let since = now_ms() - DAY_MS;
        let needs = recent_unhandled(ctx, page.guild_id, since, 3)
            .unwrap_or_default()
            .into_iter()
            .map(|c| {
                let head = format!("{} {}", c.outcome.label(), text::ago(c.at));
                let detail = if c.reason.trim().is_empty() { head } else { format!("{head} · {}", text::clip(&c.reason, 60)) };
                Need {
                    since: c.at,
                    people: Vec::new(),
                    icon: Icon::Shield,
                    title: format!("Scam caught: {}", c.user_name),
                    detail,
                    href: page.path(KEY),
                    action: "Review",
                    alert: true,
                }
            })
            .collect();
        Overview { needs, ..Overview::default() }
    }
}

/// What set off a catch.
struct Found {
    honeypot: bool,
    score: i32,
    reason: String,
    signals: Vec<Signal>,
}

impl Guard {
    async fn on_message(&self, ctx: &Ctx, m: &Message) -> Result<()> {
        let Some(guild) = m.guild_id.map(|g| g.get()) else { return Ok(()) };
        if m.author.bot || m.webhook_id.is_some() || m.author.system == Some(true) {
            return Ok(());
        }
        let s = ctx.settings::<Settings>(guild, KEY)?;
        let (user, channel) = (m.author.id.get(), m.channel_id.get());
        let honeypot = s.honeypot_channel == Some(channel);
        if !honeypot && !s.scam_detection {
            return Ok(());
        }
        let roles: Vec<u64> = match &m.member {
            Some(member) => member.roles.iter().map(|r| r.get()).collect(),
            None => ctx.cache.member(Id::new(guild), m.author.id).map(|mm| mm.roles().iter().map(|r| r.get()).collect()).unwrap_or_default(),
        };
        if is_staff(ctx, guild, user, &roles) || roles.iter().any(|r| s.exempt_roles.contains(r)) {
            return Ok(());
        }

        let text = full_text(m);
        let attachments: Vec<(String, u64)> = m.attachments.iter().map(|a| (a.filename.clone(), a.size)).collect();
        let now = now_ms();
        let repeat = {
            let print = recent::fingerprint(&text, &attachments);
            self.recent.lock().unwrap().record(guild, user, channel, m.id.get(), print, now)
        };

        let found = if honeypot {
            let signal = Signal { weight: 0, text: "Posted in the honeypot channel".into() };
            Some(Found { honeypot: true, score: 0, reason: signal.text.clone(), signals: vec![signal] })
        } else {
            if is_exempt_channel(ctx, &s, channel) {
                return Ok(());
            }
            let facts = Facts {
                account_age_days: Some(((now - account_created_ms(user)) as f64 / DAY_MS as f64).max(0.0)),
                new_account_days: s.new_account_days,
                repeat_channels: repeat,
                has_attachments: !attachments.is_empty(),
                catch_ads: s.catch_ads,
            };
            let v = score::score(&text, &facts);
            v.caught(s.strictness).then(|| Found { honeypot: false, score: v.score, reason: v.reason(), signals: v.signals })
        };
        let Some(found) = found else { return Ok(()) };

        // A burst from one account raises one alert; later messages are only deleted.
        let (repeat_catch, ids) = {
            let mut r = self.recent.lock().unwrap();
            (r.already_caught(guild, user, now), r.take(guild, user, now))
        };
        let mut ids = ids;
        if repeat_catch {
            delete_all(ctx, &[(channel, m.id.get())]).await;
            return Ok(());
        }
        ids.extend(cached_messages_of(ctx, guild, user, now));
        ids.sort_unstable();
        ids.dedup();
        act(ctx, guild, &s, m, &text, found, ids).await
    }
}

/// The text a message carries, including forwarded messages.
fn full_text(m: &Message) -> String {
    let mut t = m.content.clone();
    for snap in &m.message_snapshots {
        if !snap.message.content.is_empty() {
            t.push('\n');
            t.push_str(&snap.message.content);
        }
    }
    t
}

/// When a Discord id was made: for a user, when the account was; for a message, when it was sent.
fn snowflake_ms(id: u64) -> i64 {
    ((id >> 22) + 1_420_070_400_000) as i64
}

/// When a Discord account was made, from its id.
pub fn account_created_ms(user: u64) -> i64 {
    snowflake_ms(user)
}

/// "made today", "3 days old", "8 months old", "4 years old".
pub fn account_age(user: u64) -> String {
    let days = ((now_ms() - account_created_ms(user)) / DAY_MS).max(0);
    match days {
        0 => "made today".into(),
        1 => "1 day old".into(),
        2..60 => format!("{days} days old"),
        60..730 => format!("{} months old", days / 30),
        _ => format!("{} years old", days / 365),
    }
}

/// Server owner, or anyone whose roles grant Administrator or Manage Messages.
fn is_staff(ctx: &Ctx, guild: u64, user: u64, roles: &[u64]) -> bool {
    if ctx.cache.guild(Id::new(guild)).is_some_and(|g| g.owner_id().get() == user) {
        return true;
    }
    let mut perms = Permissions::empty();
    // The @everyone role shares the server's id.
    for role in std::iter::once(guild).chain(roles.iter().copied()) {
        if let Some(r) = ctx.cache.role(Id::new(role)) {
            perms |= r.permissions;
        }
    }
    perms.intersects(Permissions::ADMINISTRATOR | Permissions::MANAGE_MESSAGES | Permissions::MODERATE_MEMBERS)
}

/// The channel, or the channel a thread is in, is on the exempt list.
fn is_exempt_channel(ctx: &Ctx, s: &Settings, channel: u64) -> bool {
    if s.exempt_channels.contains(&channel) {
        return true;
    }
    let parent = ctx.cache.channel(Id::new(channel)).and_then(|c| c.parent_id).map(|p| p.get());
    parent.is_some_and(|p| s.exempt_channels.contains(&p))
}

/// The member's messages from the last ten minutes still in the message cache.
fn cached_messages_of(ctx: &Ctx, guild: u64, user: u64, now: i64) -> Vec<(u64, u64)> {
    let Some(channels) = ctx.cache.guild_channels(Id::new(guild)).map(|c| c.iter().copied().collect::<Vec<_>>()) else { return Vec::new() };
    let mut out = Vec::new();
    for channel in channels {
        let Some(ids) = ctx.cache.channel_messages(channel).map(|q| q.iter().copied().collect::<Vec<_>>()) else { continue };
        for id in ids {
            let sent = snowflake_ms(id.get());
            if now - sent > recent::KEEP_MS {
                continue;
            }
            if ctx.cache.message(id).is_some_and(|msg| msg.author().get() == user) {
                out.push((channel.get(), id.get()));
            }
        }
    }
    out
}

/// Delete messages, in bulk per channel where there are several. Returns how many went.
async fn delete_all(ctx: &Ctx, ids: &[(u64, u64)]) -> usize {
    let mut by_channel: Vec<(u64, Vec<u64>)> = Vec::new();
    for (ch, id) in ids {
        match by_channel.iter_mut().find(|(c, _)| c == ch) {
            Some((_, list)) => list.push(*id),
            None => by_channel.push((*ch, vec![*id])),
        }
    }
    let mut done = 0;
    for (ch, list) in by_channel {
        let channel: Id<ChannelMarker> = Id::new(ch);
        for chunk in list.chunks(100) {
            let res = if chunk.len() == 1 {
                ctx.http.delete_message(channel, Id::new(chunk[0])).reason("Scam protection").await.map(|_| ())
            } else {
                let ids: Vec<Id<MessageMarker>> = chunk.iter().map(|i| Id::new(*i)).collect();
                ctx.http.delete_messages(channel, &ids).reason("Scam protection").await.map(|_| ())
            };
            match res {
                Ok(()) => done += chunk.len(),
                Err(e) => tracing::warn!(channel = ch, error = %e, "couldn't delete caught messages"),
            }
        }
    }
    done
}

async fn act(ctx: &Ctx, guild: u64, s: &Settings, m: &Message, text: &str, found: Found, ids: Vec<(u64, u64)>) -> Result<()> {
    let user = m.author.id.get();
    let name = ctx.display_name(guild, user);
    let deleted = delete_all(ctx, &ids).await as i64;

    let minutes = s.timeout_minutes.clamp(1, MAX_TIMEOUT_MIN);
    // A minute under the limit, so Discord's own check never refuses it.
    let until_ms = now_ms() + (minutes * 60_000).min(MAX_TIMEOUT_MIN * 60_000 - 60_000);
    let timed_out = match Timestamp::from_secs(until_ms / 1000) {
        Ok(ts) => {
            let res = ctx
                .http
                .update_guild_member(Id::new(guild), m.author.id)
                .communication_disabled_until(Some(ts))
                .reason("Scam protection: likely compromised account")
                .await;
            if let Err(e) = &res {
                tracing::warn!(guild, user, error = %e, "couldn't time out a caught member");
            }
            res.is_ok()
        }
        Err(_) => false,
    };
    let outcome = if timed_out { Outcome::TimedOut } else { Outcome::NotTimedOut };
    let signals = bot_core::serde_json::to_string(&found.signals)?;
    let content = text::clip(text, 1500);
    let id = ctx.db.with(|c| {
        c.execute(
            "INSERT INTO guard_catches (guild_id, at, user_id, user_name, honeypot, score, reason, signals, content, channel_id, deleted, timeout_until, outcome)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                guild as i64, now_ms(), user as i64, name, found.honeypot, found.score, found.reason, signals, content,
                m.channel_id.get() as i64, deleted, timed_out.then_some(until_ms), outcome.key()
            ],
        )?;
        Ok(c.last_insert_rowid())
    })?;
    ctx.count(guild, "guard_catches", 1);

    let Some(c) = catch(ctx, guild, id)? else { return Ok(()) };
    if let Some(log) = s.log_channel {
        let embed = alert_embed(&c);
        let rows = alert_buttons(&c);
        let none = bot_core::twilight_model::channel::message::AllowedMentions::default();
        match ctx.http.create_message(Id::new(log)).embeds(&[embed]).components(&rows).allowed_mentions(Some(&none)).await {
            Ok(res) => {
                if let Ok(msg) = res.model().await {
                    ctx.db.with(|c| {
                        c.execute(
                            "UPDATE guard_catches SET alert_channel = ?2, alert_message = ?3 WHERE id = ?1",
                            params![id, log as i64, msg.id.get() as i64],
                        )
                    })?;
                }
            }
            Err(e) => tracing::warn!(guild, channel = log, error = %e, "couldn't post a scam alert"),
        }
    }
    let what = if found.honeypot { "posted in the honeypot channel".to_owned() } else { found.reason.to_lowercase() };
    ctx.notify(Some(guild), KEY, format!("Caught {name}: {what}"), Some(format!("/g/{guild}/{KEY}")));

    if s.dm_member && timed_out {
        let server = ctx.guild_name(guild);
        let msg = format!(
            "You were timed out in {server} for {} because your account posted what looks like a scam. \
             If someone else got into your account, change your password and turn on two-factor authentication, \
             then ask a moderator to lift the timeout.",
            timeout_label(minutes)
        );
        let _ = ctx.dm(user, &msg, &[], &[]).await;
    }
    Ok(())
}

/// Release (lift the timeout, a mistake) or ban the member of a catch. Shared by the alert's
/// buttons and the dashboard. `edit_alert` also updates the alert message in the log channel.
#[allow(clippy::too_many_arguments)]
pub async fn resolve(
    ctx: &Ctx,
    guild: u64,
    id: i64,
    outcome: Outcome,
    by: u64,
    by_name: &str,
    reason: Option<&str>,
    edit_alert: bool,
) -> std::result::Result<Catch, String> {
    let fail = |e: bot_core::anyhow::Error| {
        tracing::warn!(error = %e, "scam catch action failed");
        "That didn't work. The error is in the bot's log.".to_owned()
    };
    if !matches!(outcome, Outcome::Released | Outcome::Banned) {
        return Err("Unknown action.".into());
    }
    let c = catch(ctx, guild, id).map_err(fail)?.ok_or_else(|| "That catch is gone.".to_owned())?;
    if !c.outcome.open() {
        return Err(already(&c));
    }
    let reason = reason.map(str::trim).filter(|r| !r.is_empty()).map(|r| text::clip(r, 300));
    // Claim the catch before acting: of two moderators clicking at once only one acts, and the
    // ban or lifted timeout Discord reports back finds the catch already decided.
    let claimed = ctx
        .db
        .with(|db| {
            db.execute(
                "UPDATE guard_catches SET outcome = ?3, acted_by = ?4, acted_by_name = ?5, acted_reason = ?6, acted_at = ?7, acted_in_discord = 0
                 WHERE guild_id = ?1 AND id = ?2 AND outcome = ?8",
                params![guild as i64, id, outcome.key(), by as i64, by_name, reason, now_ms(), c.outcome.key()],
            )
        })
        .map_err(fail)?;
    if claimed == 0 {
        let now = catch(ctx, guild, id).map_err(fail)?.ok_or_else(|| "That catch is gone.".to_owned())?;
        return Err(already(&now));
    }
    let done = act_on_member(ctx, guild, &c, outcome, by_name, reason.as_deref()).await;
    if let Err(msg) = done {
        // Put it back, so someone can try again.
        ctx.db
            .with(|db| {
                db.execute(
                    "UPDATE guard_catches SET outcome = ?2, acted_by = NULL, acted_by_name = NULL, acted_reason = NULL, acted_at = NULL WHERE id = ?1",
                    params![id, c.outcome.key()],
                )
            })
            .map_err(fail)?;
        return Err(msg);
    }
    let c = catch(ctx, guild, id).map_err(fail)?.ok_or_else(|| "That catch is gone.".to_owned())?;
    if edit_alert {
        update_alert(ctx, guild, &c).await;
    }
    Ok(c)
}

/// "Already banned by Mira."
fn already(c: &Catch) -> String {
    let who = c.acted_by_name.clone().unwrap_or_else(|| "someone".into());
    format!("Already {} by {who}.", c.outcome.label().to_lowercase())
}

/// Lift the timeout or ban, in Discord, with who decided in the audit log reason.
async fn act_on_member(ctx: &Ctx, guild: u64, c: &Catch, outcome: Outcome, by_name: &str, reason: Option<&str>) -> std::result::Result<(), String> {
    let (g, u) = (Id::new(guild), Id::new(c.user_id));
    let why = reason.map(|r| format!(": {r}")).unwrap_or_default();
    match outcome {
        Outcome::Released if c.outcome == Outcome::TimedOut => {
            let res = ctx.http.update_guild_member(g, u).communication_disabled_until(None).reason(&format!("Released by {by_name}{why}")).await;
            // 404: they already left, nothing to lift.
            match res {
                Err(e) if !bot_core::discord::is_not_found(&e) => Err(format!("Couldn't remove the timeout: {e}")),
                _ => Ok(()),
            }
        }
        Outcome::Banned => {
            let reason = format!("Scam protection, banned by {by_name}{why}");
            ctx.http
                .create_ban(g, u)
                .delete_message_seconds(86_400)
                .reason(&text::clip(&reason, 500))
                .await
                .map(|_| ())
                .map_err(|e| format!("Couldn't ban: {e}"))
        }
        _ => Ok(()),
    }
}

/// Show the decision on the alert in the log channel, without its buttons.
async fn update_alert(ctx: &Ctx, guild: u64, c: &Catch) {
    let Some((ch, msg)) = c.alert else { return };
    let embed = alert_embed(c);
    let res = ctx.http.update_message(Id::new(ch), Id::new(msg)).embeds(Some(&[embed])).components(Some(&[])).await;
    if let Err(e) = res {
        tracing::warn!(guild, error = %e, "couldn't update a scam alert");
    }
}

/// A ban, or a lifted timeout, from Discord's audit log: the member and what it means for their
/// catch.
fn native_decision(e: &AuditLogEntry) -> Option<(u64, Outcome)> {
    let target = e.target_id?.get();
    match e.action_type {
        AuditLogEventType::MemberBanAdd => Some((target, Outcome::Banned)),
        AuditLogEventType::MemberUpdate => e
            .changes
            .iter()
            .any(|c| matches!(c, AuditLogChange::CommunicationDisabledUntil { new: None, .. }))
            .then_some((target, Outcome::Released)),
        _ => None,
    }
}

/// A member's open catch from the last 28 days, released or banned with Discord's own tools.
/// `by` is the moderator and their reason, from the audit log; the ban or member update itself
/// doesn't say. Whichever of the two arrives second fills in the catch the first one closed.
async fn close_open(ctx: &Ctx, guild: u64, user: u64, outcome: Outcome, by: Option<(u64, Option<&str>)>) -> Result<()> {
    let now = now_ms();
    let by = by.map(|(who, reason)| (who, ctx.display_name(guild, who), reason.map(|r| text::clip(r, 300))));
    let ids: Vec<i64> = ctx.db.with(|c| {
        let mut st = match &by {
            None => c.prepare_cached(
                "UPDATE guard_catches SET outcome = ?3, acted_by_name = 'a moderator in Discord', acted_at = ?4, acted_in_discord = 1
                 WHERE guild_id = ?1 AND user_id = ?2 AND outcome IN ('timed_out', 'not_timed_out') AND at > ?5
                 RETURNING id",
            )?,
            Some(_) => c.prepare_cached(
                "UPDATE guard_catches SET outcome = ?3, acted_by = ?6, acted_by_name = ?7, acted_reason = ?8, acted_at = ?4, acted_in_discord = 1
                 WHERE guild_id = ?1 AND user_id = ?2 AND at > ?5
                   AND (outcome IN ('timed_out', 'not_timed_out')
                        OR (outcome = ?3 AND acted_in_discord = 1 AND acted_by IS NULL AND acted_at > ?4 - 300000))
                 RETURNING id",
            )?,
        };
        let base = params![guild as i64, user as i64, outcome.key(), now, now - 28 * DAY_MS];
        let rows = match &by {
            None => st.query_map(base, |r| r.get(0))?.collect::<bot_core::rusqlite::Result<Vec<_>>>()?,
            Some((who, name, reason)) => {
                let all = params![guild as i64, user as i64, outcome.key(), now, now - 28 * DAY_MS, *who as i64, name, reason];
                st.query_map(all, |r| r.get(0))?.collect::<bot_core::rusqlite::Result<Vec<_>>>()?
            }
        };
        Ok(rows)
    })?;
    for id in ids {
        if let Some(c) = catch(ctx, guild, id)? {
            update_alert(ctx, guild, &c).await;
        }
    }
    Ok(())
}

fn alert_buttons(c: &Catch) -> Vec<Component> {
    if !c.outcome.open() {
        return Vec::new();
    }
    let release = format!("{KEY}:release:{}", c.id);
    let ban = format!("{KEY}:ban:{}", c.id);
    let label = if c.outcome == Outcome::TimedOut { "Remove timeout" } else { "Mark as mistake" };
    vec![button_row(&[(&release, label, ButtonStyle::Secondary), (&ban, "Ban", ButtonStyle::Danger)])]
}

/// The staff alert for a catch, also used to show who acted on it.
pub fn alert_embed(c: &Catch) -> Embed {
    let age = account_age(c.user_id);
    let title = if c.honeypot { "Honeypot catch" } else { "Scam caught" };
    let why = c.signals.iter().map(|s| format!("- {}", s.text)).collect::<Vec<_>>().join("\n");
    let quoted = if c.content.trim().is_empty() { "(no text, attachments only)".to_owned() } else { links::defang(&c.content.replace('`', "'")) };
    let mut action = format!("Deleted {} message{}", c.deleted, if c.deleted == 1 { "" } else { "s" });
    match (c.outcome, c.timeout_until) {
        (Outcome::NotTimedOut, _) => action.push_str(". Couldn't time them out: check the bot's role and permissions."),
        (_, Some(until)) => action.push_str(&format!(", timed out until <t:{}:f>.", until / 1000)),
        _ => action.push('.'),
    }
    let mut e = EmbedBuilder::new()
        .title(title)
        .color(COLOR)
        .description(format!("<@{}> · {} · account {}\nIn <#{}>", c.user_id, c.user_name, age, c.channel_id))
        .field(EmbedFieldBuilder::new("Why", text::clip(&why, 1000)))
        .field(EmbedFieldBuilder::new("Message", format!("```\n{}\n```", text::clip(&quoted, 900))))
        .field(EmbedFieldBuilder::new("Done", action));
    if !c.outcome.open() {
        e = e.field(EmbedFieldBuilder::new("Outcome", outcome_line(c)));
    }
    e = e.footer(EmbedFooterBuilder::new(format!("Catch #{} · user id {}", c.id, c.user_id)));
    if let Ok(ts) = Timestamp::from_secs(c.at / 1000) {
        e = e.timestamp(ts);
    }
    e.build()
}

/// Who released or banned the member, where, and why, for the alert.
fn outcome_line(c: &Catch) -> String {
    let by = c.acted_by.and(c.acted_by_name.as_deref());
    let mut line = match (c.outcome, c.acted_in_discord, by) {
        (Outcome::Released, false, Some(by)) => format!("Released by {by}, marked as a mistake."),
        (Outcome::Released, true, Some(by)) => format!("Timeout removed in Discord by {by}, counted as a mistake."),
        (Outcome::Released, _, None) => "Timeout removed in Discord, counted as a mistake.".to_owned(),
        (_, false, Some(by)) => format!("Banned by {by}. Their messages from the last day are deleted."),
        (_, true, Some(by)) => format!("Banned in Discord by {by}."),
        (_, _, None) => "Banned in Discord.".to_owned(),
    };
    if let Some(reason) = &c.acted_reason {
        line.push_str(&format!("\nReason: {}", text::clip(reason, 300)));
    }
    line
}

// ---- queries ----

const CATCH_COLS: &str = "id, at, user_id, user_name, honeypot, score, reason, signals, content, channel_id, deleted, timeout_until, outcome, acted_by_name, acted_at, alert_channel, alert_message, acted_by, acted_reason, acted_in_discord";

fn row_to_catch(r: &bot_core::rusqlite::Row) -> bot_core::rusqlite::Result<Catch> {
    let signals: String = r.get(7)?;
    let outcome: String = r.get(12)?;
    let alert = match (r.get::<_, Option<i64>>(15)?, r.get::<_, Option<i64>>(16)?) {
        (Some(c), Some(m)) => Some((c as u64, m as u64)),
        _ => None,
    };
    Ok(Catch {
        id: r.get(0)?,
        at: r.get(1)?,
        user_id: r.get::<_, i64>(2)? as u64,
        user_name: r.get(3)?,
        honeypot: r.get(4)?,
        score: r.get(5)?,
        reason: r.get(6)?,
        signals: bot_core::serde_json::from_str(&signals).unwrap_or_default(),
        content: r.get(8)?,
        channel_id: r.get::<_, i64>(9)? as u64,
        deleted: r.get(10)?,
        timeout_until: r.get(11)?,
        outcome: Outcome::parse(&outcome),
        acted_by: r.get::<_, Option<i64>>(17)?.map(|u| u as u64),
        acted_by_name: r.get(13)?,
        acted_at: r.get(14)?,
        acted_reason: r.get(18)?,
        acted_in_discord: r.get(19)?,
        alert,
    })
}

pub fn catch(ctx: &Ctx, guild: u64, id: i64) -> Result<Option<Catch>> {
    ctx.db.with(|c| {
        c.query_row(&format!("SELECT {CATCH_COLS} FROM guard_catches WHERE guild_id = ?1 AND id = ?2"), params![guild as i64, id], row_to_catch)
            .optional()
    })
}

/// Catches since `since` that are still open (timed out, or not), newest first.
pub fn recent_unhandled(ctx: &Ctx, guild: u64, since: i64, limit: i64) -> Result<Vec<Catch>> {
    ctx.db.with(|c| {
        let mut st = c.prepare_cached(&format!(
            "SELECT {CATCH_COLS} FROM guard_catches WHERE guild_id = ?1 AND at > ?2 AND outcome IN ('timed_out', 'not_timed_out') ORDER BY at DESC LIMIT ?3"
        ))?;
        st.query_map(params![guild as i64, since, limit], row_to_catch)?.collect()
    })
}

/// The newest catches, all of them or only those with one of `outcomes`.
pub fn recent_catches(ctx: &Ctx, guild: u64, outcomes: &[Outcome], limit: i64) -> Result<Vec<Catch>> {
    // The keys are fixed words from `Outcome::key`, so they can go in as text.
    let only = if outcomes.is_empty() {
        String::new()
    } else {
        format!(" AND outcome IN ({})", outcomes.iter().map(|o| format!("'{}'", o.key())).collect::<Vec<_>>().join(", "))
    };
    ctx.db.with(|c| {
        let mut st = c.prepare_cached(&format!("SELECT {CATCH_COLS} FROM guard_catches WHERE guild_id = ?1{only} ORDER BY at DESC LIMIT ?2"))?;
        let rows = st.query_map(params![guild as i64, limit], row_to_catch)?.collect::<bot_core::rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// Catches nobody has released or banned yet, however old.
pub fn count_open(ctx: &Ctx, guild: u64) -> Result<i64> {
    ctx.db.with(|c| {
        c.query_row("SELECT COUNT(*) FROM guard_catches WHERE guild_id = ?1 AND outcome IN ('timed_out', 'not_timed_out')", [guild as i64], |r| r.get(0))
    })
}

/// The last seven days of catches, for the dashboard's readings and chart.
pub struct Week {
    pub caught: i64,
    pub banned: i64,
    pub released: i64,
    pub removed: i64,
    /// Catches in each of the last seven rolling days, oldest first.
    pub daily: [i64; 7],
    /// The moderators who released or banned, most actions first.
    pub handled_by: Vec<u64>,
}

pub fn week(ctx: &Ctx, guild: u64) -> Result<Week> {
    let now = now_ms();
    let rows: Vec<(i64, Outcome, i64, Option<u64>)> = ctx.db.with(|c| {
        let mut st = c.prepare_cached("SELECT at, outcome, deleted, acted_by FROM guard_catches WHERE guild_id = ?1 AND at > ?2")?;
        let rows = st
            .query_map(params![guild as i64, now - 7 * DAY_MS], |r| {
                let outcome: String = r.get(1)?;
                Ok((r.get(0)?, Outcome::parse(&outcome), r.get(2)?, r.get::<_, Option<i64>>(3)?.map(|u| u as u64)))
            })?
            .collect::<bot_core::rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })?;
    let mut w = Week { caught: 0, banned: 0, released: 0, removed: 0, daily: [0; 7], handled_by: Vec::new() };
    let mut by: Vec<(u64, i64)> = Vec::new();
    for (at, outcome, deleted, who) in rows {
        w.caught += 1;
        w.removed += deleted;
        match outcome {
            Outcome::Banned => w.banned += 1,
            Outcome::Released => w.released += 1,
            _ => {}
        }
        let days_ago = ((now - at) / DAY_MS).clamp(0, 6) as usize;
        w.daily[6 - days_ago] += 1;
        if let Some(who) = who {
            match by.iter_mut().find(|(u, _)| *u == who) {
                Some(entry) => entry.1 += 1,
                None => by.push((who, 1)),
            }
        }
    }
    by.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    w.handled_by = by.into_iter().map(|(u, _)| u).collect();
    Ok(w)
}

/// "today", "yesterday", "2 days ago", "5 months ago", "3 years ago": how old an account is,
/// from its id.
pub fn account_made(user: u64) -> String {
    let days = ((now_ms() - account_created_ms(user)) / DAY_MS).max(0);
    match days {
        0 => "today".into(),
        1 => "yesterday".into(),
        2..60 => format!("{days} days ago"),
        60..730 => format!("{} months ago", days / 30),
        _ => format!("{} years ago", days / 365),
    }
}

/// A snowflake whose account was made at `ms`, for the preview's sample members.
fn snowflake_at(ms: i64) -> u64 {
    ((ms - 1_420_070_400_000) as u64) << 22
}

// ---- preview ----

fn seed(ctx: &Ctx, guild: u64) -> Result<()> {
    let channels = ctx.text_channels(guild);
    let find = |name: &str| channels.iter().find(|(_, n)| n == name).map(|(id, _)| *id);
    let roles = ctx.roles(guild);
    let s = Settings {
        honeypot_channel: find("suggestions"),
        log_channel: find("mod-log"),
        exempt_roles: roles.iter().filter(|(_, n, _)| n == "Streamer").map(|(id, _, _)| *id).collect(),
        exempt_channels: find("tickets").into_iter().collect(),
        ..Settings::default()
    };
    ctx.save_settings(guild, KEY, &s)?;
    let general = find("general").unwrap_or_default();
    let honeypot = s.honeypot_channel.unwrap_or_default();
    let now = now_ms();
    let members = bot_core::preview::MEMBERS;
    let log = s.log_channel;
    // (hours ago, account age in days, user (ignored when the age is set), name,
    //  honeypot, text, outcome, moderator)
    type Sample<'a> = (i64, Option<i64>, u64, &'a str, bool, &'a str, Outcome, Option<(u64, &'a str)>);
    let samples: &[Sample] = &[
        (2, Some(2), 1_250_000_000_000_000_001, "kaiden.trades", false, "I'm leaving CS2, giving away my whole inventory. first come first serve, send a trade offer https://steamcommunlty.com/tradeoffer/new", Outcome::TimedOut, None),
        (9, Some(800), members[5].0, members[5].1, false, "@everyone free nitro for everyone https://discord-nitro.shop/claim", Outcome::Banned, Some((members[0].0, "Mira"))),
        (48, Some(1), 1_250_000_000_000_000_002, "lunafae_", true, "hey cuties check my bio 😘", Outcome::Banned, Some((members[0].0, "Mira"))),
        (51, Some(400), members[8].0, members[8].1, false, "is free nitro from epic still a thing? https://bit.ly/3epicnitro", Outcome::Released, Some((members[1].0, "Oskar"))),
        (77, Some(0), 1_250_000_000_000_000_003, "crypto_drop_bot", false, "Huge crypto airdrop is live, connect your wallet to claim https://claim-drop.site", Outcome::NotTimedOut, None),
        (140, Some(1500), members[3].0, members[3].1, false, "look at this https://gift-steam.ru/card", Outcome::Banned, Some((members[0].0, "Mira"))),
    ];
    for (hours, account_days, user, name, honeypot_catch, msg, outcome, by) in samples {
        let at = now - hours * 3_600_000;
        let user = match account_days {
            Some(days) => snowflake_at(now - days * DAY_MS),
            None => *user,
        };
        let (score, reason, signals) = if *honeypot_catch {
            let sig = vec![Signal { weight: 0, text: "Posted in the honeypot channel".into() }];
            (0, "Posted in the honeypot channel".to_owned(), sig)
        } else {
            let repeat = if *hours == 140 { 3 } else { 1 };
            let facts = Facts { account_age_days: Some(account_days.unwrap_or(365) as f64), new_account_days: 7, repeat_channels: repeat, has_attachments: false, catch_ads: false };
            let v = score::score(msg, &facts);
            (v.score, v.reason(), v.signals)
        };
        let timeout_until = (*outcome == Outcome::TimedOut).then_some(at + DAY_MS);
        let (by_id, by_name) = by.map_or((None, None), |(id, name)| (Some(id as i64), Some(name)));
        // One ban with a reason from the dashboard, one made with Discord's own ban.
        let (why, in_discord) = match hours {
            48 => (Some("Spam bot, nothing else on the account"), false),
            140 => (Some("Compromised. Told them to rejoin once it's secured"), true),
            _ => (None, false),
        };
        ctx.db.with(|c| {
            c.execute(
                "INSERT INTO guard_catches (guild_id, at, user_id, user_name, honeypot, score, reason, signals, content, channel_id, deleted, timeout_until, outcome, acted_by, acted_by_name, acted_at, alert_channel, alert_message, acted_reason, acted_in_discord)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
                params![
                    guild as i64, at, user as i64, name, honeypot_catch, score, reason, bot_core::serde_json::to_string(&signals).unwrap_or_default(),
                    msg, (if *honeypot_catch { honeypot } else { general }) as i64, 1 + (hours % 4), timeout_until, outcome.key(),
                    by_id, by_name, by.map(|_| at + 600_000), log.map(|c| c as i64), log.map(|_| 1i64), why, in_discord
                ],
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::{Config, Db};

    const G: u64 = 42;
    const U: u64 = 1_250_000_000_000_000_001;
    const MOD: u64 = 1_250_000_000_000_000_009;

    fn ctx() -> Ctx {
        let ctx = Ctx::new(Config::preview(0), Db::memory().unwrap()).unwrap();
        ctx.install_features(vec![std::sync::Arc::new(Guard::default())]).unwrap();
        ctx.set_enabled(G, KEY, true).unwrap();
        ctx
    }

    /// An open catch of `U`, as `act` stores it, without an alert message.
    fn open_catch(ctx: &Ctx) -> i64 {
        ctx.db
            .with(|c| {
                c.execute(
                    "INSERT INTO guard_catches (guild_id, at, user_id, user_name, reason, channel_id, outcome) VALUES (?1, ?2, ?3, 'kaiden', 'x', 7, 'timed_out')",
                    params![G as i64, now_ms() - 60_000, U as i64],
                )?;
                Ok(c.last_insert_rowid())
            })
            .unwrap()
    }

    fn entry(json: &str) -> AuditLogEntry {
        bot_core::serde_json::from_str(json).unwrap()
    }

    #[test]
    fn audit_entries_that_decide_a_catch() {
        let ban = entry(r#"{"action_type":22,"changes":[],"guild_id":"42","id":"5","target_id":"77","user_id":"9","reason":"spam"}"#);
        assert_eq!(native_decision(&ban), Some((77, Outcome::Banned)));
        let lift = entry(r#"{"action_type":24,"changes":[{"key":"communication_disabled_until","old_value":"2026-10-31T00:00:00+00:00"}],"id":"5","target_id":"77","user_id":"9"}"#);
        assert_eq!(native_decision(&lift), Some((77, Outcome::Released)));
        let timeout = entry(r#"{"action_type":24,"changes":[{"key":"communication_disabled_until","new_value":"2026-10-31T00:00:00+00:00"}],"id":"5","target_id":"77","user_id":"9"}"#);
        assert_eq!(native_decision(&timeout), None);
        let nick = entry(r#"{"action_type":24,"changes":[{"key":"nick","new_value":"x"}],"id":"5","target_id":"77","user_id":"9"}"#);
        assert_eq!(native_decision(&nick), None);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn a_ban_in_discord_then_its_audit_entry() {
        let ctx = ctx();
        let id = open_catch(&ctx);
        close_open(&ctx, G, U, Outcome::Banned, None).await.unwrap();
        let c = catch(&ctx, G, id).unwrap().unwrap();
        assert_eq!((c.outcome, c.acted_by, c.acted_in_discord), (Outcome::Banned, None, true));
        // The audit entry names the moderator and their reason.
        close_open(&ctx, G, U, Outcome::Banned, Some((MOD, Some("selling accounts")))).await.unwrap();
        let c = catch(&ctx, G, id).unwrap().unwrap();
        assert_eq!((c.acted_by, c.acted_reason.as_deref()), (Some(MOD), Some("selling accounts")));
        assert!(outcome_line(&c).starts_with("Banned in Discord by "));
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn the_audit_entry_can_come_first() {
        let ctx = ctx();
        let id = open_catch(&ctx);
        close_open(&ctx, G, U, Outcome::Released, Some((MOD, None))).await.unwrap();
        // The member update that follows changes nothing.
        close_open(&ctx, G, U, Outcome::Released, None).await.unwrap();
        let c = catch(&ctx, G, id).unwrap().unwrap();
        assert_eq!((c.outcome, c.acted_by, c.acted_reason), (Outcome::Released, Some(MOD), None));
        assert_eq!(c.acted_by_name.as_deref(), Some(ctx.display_name(G, MOD).as_str()));
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn a_decided_catch_isnt_decided_again() {
        let ctx = ctx();
        let id = open_catch(&ctx);
        close_open(&ctx, G, U, Outcome::Banned, Some((MOD, None))).await.unwrap();
        // A later audit entry for another action, and a release from the dashboard, leave it be.
        close_open(&ctx, G, U, Outcome::Released, Some((7, Some("oops")))).await.unwrap();
        let err = resolve(&ctx, G, id, Outcome::Released, 7, "Oskar", Some("mistake"), false).await.unwrap_err();
        assert!(err.starts_with("Already banned by "), "{err}");
        let c = catch(&ctx, G, id).unwrap().unwrap();
        assert_eq!((c.outcome, c.acted_by, c.acted_reason), (Outcome::Banned, Some(MOD), None));
        // An older catch from before the 28 days isn't touched.
        ctx.db.with(|c| c.execute("UPDATE guard_catches SET outcome = 'timed_out', at = ?1", [now_ms() - 30 * DAY_MS])).unwrap();
        close_open(&ctx, G, U, Outcome::Banned, None).await.unwrap();
        assert!(catch(&ctx, G, id).unwrap().unwrap().outcome.open());
    }

    #[test]
    fn the_alert_says_who_and_why() {
        let mut c = Catch {
            id: 1,
            at: 0,
            user_id: U,
            user_name: "kaiden".into(),
            honeypot: false,
            score: 9,
            reason: String::new(),
            signals: Vec::new(),
            content: String::new(),
            channel_id: 7,
            deleted: 2,
            timeout_until: None,
            outcome: Outcome::Released,
            acted_by: Some(MOD),
            acted_by_name: Some("Mira".into()),
            acted_at: Some(1),
            acted_reason: Some("their brother's account".into()),
            acted_in_discord: false,
            alert: None,
        };
        assert_eq!(outcome_line(&c), "Released by Mira, marked as a mistake.\nReason: their brother's account");
        c.outcome = Outcome::Banned;
        c.acted_in_discord = true;
        c.acted_by = None;
        c.acted_reason = None;
        assert_eq!(outcome_line(&c), "Banned in Discord.");
    }
}
