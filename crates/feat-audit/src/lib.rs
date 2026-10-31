//! Logs edits, deletes, joins, leaves, role and nickname changes, bans and timeouts to a channel,
//! keeps two weeks of them for the dashboard, and batches bursts so Discord's limits aren't hit.

mod batch;
mod store;
mod web;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bot_core::anyhow::Result;
use bot_core::discord::avatar_url;
use bot_core::twilight_model::channel::message::{AllowedMentions, Embed};
use bot_core::twilight_model::gateway::event::Event;
use bot_core::twilight_model::id::Id;
use bot_core::twilight_model::util::Timestamp;
use bot_core::twilight_util::builder::embed::{EmbedAuthorBuilder, EmbedBuilder, EmbedFieldBuilder, EmbedFooterBuilder, ImageSource};
use bot_core::{Ctx, Feature, FeatureMeta, Group, Icon, Migration, NavItem, feature_async, now_ms, text};
use serde::{Deserialize, Serialize};

use batch::Batcher;

pub const KEY: &str = "audit";

const GREY: u32 = 0x8b8f98;
const CORAL: u32 = 0xf0806a;
/// Accounts younger than this get flagged when they join.
const NEW_ACCOUNT_MS: i64 = 7 * 86_400_000;
/// Log entries older than this are dropped from the dashboard table.
pub(crate) const KEEP_DAYS: i64 = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    MessageDelete,
    MessageEdit,
    BulkDelete,
    Join,
    Leave,
    Nickname,
    Roles,
    Ban,
    Unban,
    Timeout,
    TimeoutEnd,
    Boost,
    VoiceJoin,
    VoiceLeave,
    VoiceMove,
}

impl Kind {
    pub const ALL: [Kind; 15] = [
        Kind::MessageDelete,
        Kind::MessageEdit,
        Kind::BulkDelete,
        Kind::Join,
        Kind::Leave,
        Kind::Nickname,
        Kind::Roles,
        Kind::Ban,
        Kind::Unban,
        Kind::Timeout,
        Kind::TimeoutEnd,
        Kind::Boost,
        Kind::VoiceJoin,
        Kind::VoiceLeave,
        Kind::VoiceMove,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Kind::MessageDelete => "message_delete",
            Kind::MessageEdit => "message_edit",
            Kind::BulkDelete => "bulk_delete",
            Kind::Join => "join",
            Kind::Leave => "leave",
            Kind::Nickname => "nickname",
            Kind::Roles => "roles",
            Kind::Ban => "ban",
            Kind::Unban => "unban",
            Kind::Timeout => "timeout",
            Kind::TimeoutEnd => "timeout_end",
            Kind::Boost => "boost",
            Kind::VoiceJoin => "voice_join",
            Kind::VoiceLeave => "voice_leave",
            Kind::VoiceMove => "voice_move",
        }
    }

    pub fn from_key(key: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::MessageDelete => "Message deleted",
            Kind::MessageEdit => "Message edited",
            Kind::BulkDelete => "Bulk delete",
            Kind::Join => "Joined",
            Kind::Leave => "Left",
            Kind::Nickname => "Nickname",
            Kind::Roles => "Roles",
            Kind::Ban => "Banned",
            Kind::Unban => "Unbanned",
            Kind::Timeout => "Timed out",
            Kind::TimeoutEnd => "Timeout removed",
            Kind::Boost => "Boosted",
            Kind::VoiceJoin => "Voice joined",
            Kind::VoiceLeave => "Voice left",
            Kind::VoiceMove => "Voice moved",
        }
    }

    /// Message events go to the message channel, everything else to the member channel.
    fn is_message(self) -> bool {
        matches!(self, Kind::MessageDelete | Kind::MessageEdit | Kind::BulkDelete)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Where events go unless a more specific channel below is set.
    pub channel: Option<u64>,
    pub message_channel: Option<u64>,
    pub member_channel: Option<u64>,
    pub message_delete: bool,
    pub message_edit: bool,
    pub bulk_delete: bool,
    pub join: bool,
    pub leave: bool,
    pub nickname: bool,
    pub roles: bool,
    pub ban: bool,
    pub unban: bool,
    pub timeout: bool,
    pub boost: bool,
    pub voice: bool,
    pub ignore_bots: bool,
    pub ignored_channels: Vec<u64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            channel: None,
            message_channel: None,
            member_channel: None,
            message_delete: true,
            message_edit: true,
            bulk_delete: true,
            join: true,
            leave: true,
            nickname: true,
            roles: true,
            ban: true,
            unban: true,
            timeout: true,
            boost: true,
            // Noisy, so off until someone wants it.
            voice: false,
            ignore_bots: true,
            ignored_channels: Vec::new(),
        }
    }
}

impl Settings {
    pub fn on(&self, kind: Kind) -> bool {
        match kind {
            Kind::MessageDelete => self.message_delete,
            Kind::MessageEdit => self.message_edit,
            Kind::BulkDelete => self.bulk_delete,
            Kind::Join => self.join,
            Kind::Leave => self.leave,
            Kind::Nickname => self.nickname,
            Kind::Roles => self.roles,
            Kind::Ban => self.ban,
            Kind::Unban => self.unban,
            Kind::Timeout | Kind::TimeoutEnd => self.timeout,
            Kind::Boost => self.boost,
            Kind::VoiceJoin | Kind::VoiceLeave | Kind::VoiceMove => self.voice,
        }
    }

    /// The channel this kind of event is posted in, if the switch is on and one is set.
    pub fn route(&self, kind: Kind) -> Option<u64> {
        if !self.on(kind) {
            return None;
        }
        let specific = if kind.is_message() { self.message_channel } else { self.member_channel };
        specific.or(self.channel)
    }

    /// Messages in the log channels themselves are never logged, so a mod deleting a log
    /// entry doesn't create another one.
    fn skips_channel(&self, channel: u64) -> bool {
        self.ignored_channels.contains(&channel)
            || [self.channel, self.message_channel, self.member_channel].contains(&Some(channel))
    }
}

const MIGRATIONS: &[Migration] = &[Migration {
    name: "001_audit",
    sql: "
        CREATE TABLE audit_events (
            id INTEGER PRIMARY KEY, guild_id INTEGER NOT NULL, at INTEGER NOT NULL, kind TEXT NOT NULL,
            user_id INTEGER, user_name TEXT NOT NULL DEFAULT '', channel_id INTEGER,
            summary TEXT NOT NULL DEFAULT '');
        CREATE INDEX audit_events_by_time ON audit_events(guild_id, at DESC);
    ",
}, Migration {
    name: "002_audit_moderators",
    sql: "
        CREATE TABLE audit_mod_actions (
            id INTEGER PRIMARY KEY, guild_id INTEGER NOT NULL, at INTEGER NOT NULL,
            moderator_id INTEGER NOT NULL, action TEXT NOT NULL, target_id INTEGER);
        CREATE INDEX audit_mod_actions_by_time ON audit_mod_actions(guild_id, at DESC);
    ",
}];

#[derive(Default)]
pub struct Audit {
    batcher: Arc<Batcher>,
    /// Recent bans, so the member-left event Discord sends with a ban isn't logged as well.
    bans: Mutex<HashMap<(u64, u64), Instant>>,
    /// Recent boosts, so a member's later updates don't log the same boost again.
    boosts: Mutex<HashMap<(u64, u64), Instant>>,
}

/// One thing to post and remember.
struct Item {
    guild: u64,
    kind: Kind,
    user: Option<u64>,
    channel: Option<u64>,
    summary: String,
    embed: Embed,
}

#[feature_async]
impl Feature for Audit {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Audit log",
            description: "Logs edits, deletes, joins, leaves, role and nickname changes, bans and timeouts to a channel.",
            group: Group::Safety,
            nav: vec![NavItem { label: "Audit log", path: KEY, icon: Icon::Log, about: "Edits, deletes, joins, roles and bans, also posted to a channel." }],
        }
    }

    fn migrations(&self) -> &'static [Migration] {
        MIGRATIONS
    }

    fn routes(&self) -> bot_core::axum::Router<Ctx> {
        web::routes()
    }

    async fn on_event(&self, ctx: &Ctx, event: &Event) -> Result<()> {
        // Who did it comes from Discord's own audit log, a moment after the event itself.
        if let Event::GuildAuditLogEntryCreate(e) = event {
            store::record_moderator(ctx, &e.0)?;
            return Ok(());
        }
        let Some(guild) = event_guild(event) else { return Ok(()) };
        let s = ctx.settings::<Settings>(guild, KEY)?;
        if s.channel.is_none() && s.message_channel.is_none() && s.member_channel.is_none() {
            return Ok(());
        }
        for item in self.items(ctx, &s, guild, event).await {
            self.emit(ctx, &s, item);
        }
        Ok(())
    }

    async fn tick(&self, ctx: &Ctx) -> Result<()> {
        store::prune(ctx, now_ms() - KEEP_DAYS * 86_400_000)?;
        let keep = Duration::from_secs(300);
        self.bans.lock().unwrap().retain(|_, t| t.elapsed() < keep);
        self.boosts.lock().unwrap().retain(|_, t| t.elapsed() < keep);
        Ok(())
    }

    fn seed_preview(&self, ctx: &Ctx, guild: u64) -> Result<()> {
        store::seed_preview(ctx, guild)
    }
}

fn event_guild(event: &Event) -> Option<u64> {
    let id = match event {
        Event::MessageDelete(m) => m.guild_id,
        Event::MessageDeleteBulk(m) => m.guild_id,
        Event::MessageUpdate(m) => m.guild_id,
        Event::MemberAdd(m) => Some(m.guild_id),
        Event::MemberRemove(m) => Some(m.guild_id),
        Event::MemberUpdate(m) => Some(m.guild_id),
        Event::BanAdd(b) => Some(b.guild_id),
        Event::BanRemove(b) => Some(b.guild_id),
        Event::VoiceStateUpdate(v) => v.guild_id,
        _ => None,
    };
    id.map(|g| g.get())
}

impl Audit {
    /// Turn a gateway event into the log entries it deserves, honouring the switches.
    async fn items(&self, ctx: &Ctx, s: &Settings, guild: u64, event: &Event) -> Vec<Item> {
        let mut out = Vec::new();
        let is_bot = |id: u64| s.ignore_bots && ctx.cache.user(Id::new(id)).is_some_and(|u| u.bot);
        match event {
            Event::MessageDelete(m) if s.route(Kind::MessageDelete).is_some() => {
                let channel = m.channel_id.get();
                if s.skips_channel(channel) {
                    return out;
                }
                let before = ctx.before_message(m.id.get());
                if before.as_ref().is_some_and(|b| is_bot(b.author_id)) {
                    return out;
                }
                let author = before.as_ref().map(|b| b.author_id);
                let content = before.as_ref().map(|b| b.content.as_str());
                let files: Vec<String> = before.iter().flat_map(|b| b.attachments.iter().map(|u| file_name(u))).collect();
                let mut e = embed(ctx, guild, "Message deleted", CORAL, author);
                e = e.description(match content {
                    None => "Content not available".to_owned(),
                    Some(c) if c.trim().is_empty() => "No text".to_owned(),
                    Some(c) => text::clip(c, 1000),
                });
                e = field(e, "Channel", &format!("<#{channel}>"), true);
                if !files.is_empty() {
                    e = field(e, "Files", &text::clip(&files.join("\n"), 500), true);
                }
                let summary = match content {
                    None => "Content not available".to_owned(),
                    Some(c) if c.trim().is_empty() && !files.is_empty() => files.join(", "),
                    Some(c) if c.trim().is_empty() => "No text".to_owned(),
                    Some(c) => text::clip(&one_line(c), 100),
                };
                out.push(Item { guild, kind: Kind::MessageDelete, user: author, channel: Some(channel), summary, embed: e.build() });
            }
            Event::MessageUpdate(m) if s.route(Kind::MessageEdit).is_some() => {
                let channel = m.channel_id.get();
                if s.skips_channel(channel) || (s.ignore_bots && m.author.bot) {
                    return out;
                }
                let before = ctx.before_message(m.id.get());
                let edited_ms = m.edited_timestamp.map(|t| t.as_micros() / 1000);
                if !edit_changed(before.as_ref().map(|b| b.content.as_str()), &m.content, edited_ms, now_ms()) {
                    return out;
                }
                let author = m.author.id.get();
                let link = format!("https://discord.com/channels/{guild}/{channel}/{}", m.id.get());
                let mut e = embed(ctx, guild, "Message edited", GREY, Some(author))
                    .description(format!("In <#{channel}>, [jump to message]({link})"));
                e = field(e, "Before", &before.as_ref().map(|b| clip_or(&b.content, 900, "No text")).unwrap_or_else(|| "Not available".into()), false);
                e = field(e, "After", &clip_or(&m.content, 900, "No text"), false);
                let summary = diff_json(before.as_ref().map(|b| b.content.as_str()), Some(&m.content));
                out.push(Item { guild, kind: Kind::MessageEdit, user: Some(author), channel: Some(channel), summary, embed: e.build() });
            }
            Event::MessageDeleteBulk(m) if s.route(Kind::BulkDelete).is_some() => {
                let channel = m.channel_id.get();
                if s.skips_channel(channel) {
                    return out;
                }
                let n = m.ids.len();
                let e = embed(ctx, guild, "Messages deleted in bulk", CORAL, None)
                    .description(format!("{} deleted in <#{channel}>", plural(n, "message")));
                out.push(Item { guild, kind: Kind::BulkDelete, user: None, channel: Some(channel), summary: format!("{} deleted", plural(n, "message")), embed: e.build() });
            }
            Event::MemberAdd(m) if s.route(Kind::Join).is_some() => {
                let user = m.user.id.get();
                if s.ignore_bots && m.user.bot {
                    return out;
                }
                let created = snowflake_ms(user);
                let fresh = now_ms() - created < NEW_ACCOUNT_MS;
                let mut description = format!("Account created {}", rel(created));
                if fresh {
                    description.push_str("\nNew account, under 7 days old");
                }
                let e = embed(ctx, guild, "Member joined", if fresh { CORAL } else { GREY }, Some(user)).description(description);
                let made = made_ago(created);
                let summary = if fresh { format!("New account, made {made}") } else { format!("Account made {made}") };
                out.push(Item { guild, kind: Kind::Join, user: Some(user), channel: None, summary, embed: e.build() });
            }
            Event::MemberRemove(m) if s.route(Kind::Leave).is_some() => {
                let user = m.user.id.get();
                if s.ignore_bots && m.user.bot {
                    return out;
                }
                // A ban sends a leave too. Give the ban a moment to arrive, then let it speak.
                bot_core::tokio::time::sleep(Duration::from_millis(1200)).await;
                if self.recently_banned(guild, user) {
                    return out;
                }
                let before = ctx.before_member(guild, user);
                let roles: Vec<u64> = before.as_ref().map(|b| b.roles.iter().copied().filter(|r| *r != guild).collect()).unwrap_or_default();
                let mut description = String::new();
                if let Some(joined) = before.as_ref().and_then(|b| b.joined_at_ms) {
                    description = format!("Joined {}", rel(joined));
                }
                let mut e = embed(ctx, guild, "Member left", GREY, Some(user));
                if !description.is_empty() {
                    e = e.description(description);
                }
                e = field(e, "Roles", &role_mentions(&roles, 20), false);
                let summary = match (&before, roles.len()) {
                    (None, _) => "Left the server".to_owned(),
                    (_, 0) => "Had no roles".to_owned(),
                    (_, n) => format!("Had {}", plural(n, "role")),
                };
                out.push(Item { guild, kind: Kind::Leave, user: Some(user), channel: None, summary, embed: e.build() });
            }
            Event::MemberUpdate(m) => {
                let user = m.user.id.get();
                if s.ignore_bots && m.user.bot {
                    return out;
                }
                let before = ctx.before_member(guild, user);
                if let Some(before) = &before {
                    if s.route(Kind::Nickname).is_some() && before.nick != m.nick {
                        let show = |n: &Option<String>| n.clone().unwrap_or_else(|| "None".into());
                        let e = embed(ctx, guild, "Nickname changed", GREY, Some(user));
                        let e = field(field(e, "Before", &show(&before.nick), true), "After", &show(&m.nick), true);
                        let summary = diff_json(before.nick.as_deref(), m.nick.as_deref());
                        out.push(Item { guild, kind: Kind::Nickname, user: Some(user), channel: None, summary, embed: e.build() });
                    }
                    if s.route(Kind::Roles).is_some() {
                        let after: Vec<u64> = m.roles.iter().map(|r| r.get()).collect();
                        let (added, removed) = role_diff(&before.roles, &after);
                        if !added.is_empty() || !removed.is_empty() {
                            let mut e = embed(ctx, guild, "Roles changed", GREY, Some(user));
                            if !added.is_empty() {
                                e = field(e, "Added", &role_mentions(&added, 20), false);
                            }
                            if !removed.is_empty() {
                                e = field(e, "Removed", &role_mentions(&removed, 20), false);
                            }
                            let summary = role_change_json(ctx, &added, &removed);
                            out.push(Item { guild, kind: Kind::Roles, user: Some(user), channel: None, summary, embed: e.build() });
                        }
                    }
                    if s.route(Kind::Timeout).is_some() {
                        let after = m.communication_disabled_until.map(|t| t.as_micros() / 1000);
                        let now = now_ms();
                        match timeout_change(before.timed_out_until_ms, after, now) {
                            Some(TimeoutChange::Started(until)) => {
                                let e = embed(ctx, guild, "Member timed out", CORAL, Some(user))
                                    .description(format!("Until <t:{}:f> (<t:{}:R>)", until / 1000, until / 1000));
                                let summary = format!("For {}", text::duration((until - now) / 1000));
                                out.push(Item { guild, kind: Kind::Timeout, user: Some(user), channel: None, summary, embed: e.build() });
                            }
                            Some(TimeoutChange::Ended) => {
                                let e = embed(ctx, guild, "Timeout removed", GREY, Some(user));
                                out.push(Item { guild, kind: Kind::TimeoutEnd, user: Some(user), channel: None, summary: String::new(), embed: e.build() });
                            }
                            None => {}
                        }
                    }
                }
                if s.route(Kind::Boost).is_some() {
                    let since = m.premium_since.map(|t| t.as_micros() / 1000);
                    if just_boosted(since, now_ms()) && self.first_boost(guild, user) {
                        let e = embed(ctx, guild, "Server boosted", GREY, Some(user)).description("Started boosting the server");
                        out.push(Item { guild, kind: Kind::Boost, user: Some(user), channel: None, summary: String::new(), embed: e.build() });
                    }
                }
            }
            Event::BanAdd(b) => {
                let user = b.user.id.get();
                self.bans.lock().unwrap().insert((guild, user), Instant::now());
                if s.route(Kind::Ban).is_none() || (s.ignore_bots && b.user.bot) {
                    return out;
                }
                let e = embed(ctx, guild, "Member banned", CORAL, Some(user));
                out.push(Item { guild, kind: Kind::Ban, user: Some(user), channel: None, summary: String::new(), embed: e.build() });
            }
            Event::BanRemove(b) if s.route(Kind::Unban).is_some() => {
                let user = b.user.id.get();
                if s.ignore_bots && b.user.bot {
                    return out;
                }
                let e = embed(ctx, guild, "Member unbanned", GREY, Some(user));
                out.push(Item { guild, kind: Kind::Unban, user: Some(user), channel: None, summary: String::new(), embed: e.build() });
            }
            Event::VoiceStateUpdate(v) if s.route(Kind::VoiceJoin).is_some() => {
                let user = v.user_id.get();
                let now = v.channel_id.map(|c| c.get());
                let was = ctx.before_voice(guild, user);
                if was == now || is_bot(user) || v.member.as_ref().is_some_and(|m| s.ignore_bots && m.user.bot) {
                    return out;
                }
                let (kind, description, channel, summary) = match (was, now) {
                    (None, Some(to)) => (Kind::VoiceJoin, format!("Joined <#{to}>"), to, String::new()),
                    (Some(from), None) => (Kind::VoiceLeave, format!("Left <#{from}>"), from, String::new()),
                    (Some(from), Some(to)) => (
                        Kind::VoiceMove,
                        format!("Moved from <#{from}> to <#{to}>"),
                        to,
                        format!("{} to {}", channel_label(ctx, from), channel_label(ctx, to)),
                    ),
                    (None, None) => return out,
                };
                let title = match kind {
                    Kind::VoiceJoin => "Joined voice",
                    Kind::VoiceLeave => "Left voice",
                    _ => "Moved voice channel",
                };
                let e = embed(ctx, guild, title, GREY, Some(user)).description(description);
                out.push(Item { guild, kind, user: Some(user), channel: Some(channel), summary, embed: e.build() });
            }
            _ => {}
        }
        out
    }

    fn recently_banned(&self, guild: u64, user: u64) -> bool {
        self.bans.lock().unwrap().get(&(guild, user)).is_some_and(|t| t.elapsed() < Duration::from_secs(30))
    }

    /// True the first time a boost is seen for this member in the last few minutes.
    fn first_boost(&self, guild: u64, user: u64) -> bool {
        let mut boosts = self.boosts.lock().unwrap();
        if boosts.get(&(guild, user)).is_some_and(|t| t.elapsed() < Duration::from_secs(300)) {
            return false;
        }
        boosts.insert((guild, user), Instant::now());
        true
    }

    /// Remember the entry for the dashboard and queue the embed for its channel.
    fn emit(&self, ctx: &Ctx, s: &Settings, item: Item) {
        let Some(target) = s.route(item.kind) else { return };
        let name = item.user.map(|u| ctx.display_name(item.guild, u)).unwrap_or_default();
        if let Err(e) = store::record(ctx, item.guild, item.kind, item.user, &name, item.channel, &item.summary) {
            bot_core::tracing::warn!(guild = item.guild, error = %e, "couldn't store an audit entry");
        }
        if self.batcher.push(target, item.embed) {
            let (ctx, batcher) = (ctx.clone(), self.batcher.clone());
            bot_core::tokio::spawn(async move { flush(ctx, batcher, target).await });
        }
    }
}

/// Wait out the window, then post everything that gathered for the channel.
async fn flush(ctx: Ctx, batcher: Arc<Batcher>, channel: u64) {
    bot_core::tokio::time::sleep(batch::WINDOW).await;
    for embeds in batcher.drain(channel) {
        let sent = ctx.http.create_message(Id::new(channel)).embeds(&embeds).allowed_mentions(Some(&AllowedMentions::default())).await;
        if let Err(e) = sent {
            bot_core::tracing::warn!(channel, error = %e, "couldn't post to the audit log channel");
        }
    }
}

// ---- pure helpers ----

/// Roles in `after` but not `before`, and the other way round, in their original order.
pub fn role_diff(before: &[u64], after: &[u64]) -> (Vec<u64>, Vec<u64>) {
    let added = after.iter().copied().filter(|r| !before.contains(r)).collect();
    let removed = before.iter().copied().filter(|r| !after.contains(r)).collect();
    (added, removed)
}

/// Whether a message update is a real edit. Discord also sends updates when a link unfurls or an
/// embed changes; those carry the same text. With no copy of the old message, an edit stamped
/// in the last few seconds counts, so an unfurl on a message edited long ago doesn't.
pub fn edit_changed(before: Option<&str>, after: &str, edited_ms: Option<i64>, now: i64) -> bool {
    match before {
        Some(b) => b != after,
        None => edited_ms.is_some_and(|t| now - t < 15_000),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutChange {
    /// Timed out until this time (epoch ms).
    Started(i64),
    Ended,
}

/// Compare a member's timeout before and after an update.
pub fn timeout_change(before: Option<i64>, after: Option<i64>, now: i64) -> Option<TimeoutChange> {
    let active = |t: Option<i64>| t.filter(|t| *t > now);
    match (active(before), active(after)) {
        (b, Some(a)) if b != Some(a) => Some(TimeoutChange::Started(a)),
        (Some(_), None) => Some(TimeoutChange::Ended),
        _ => None,
    }
}

/// A boost starts the member's `premium_since` at the moment it happens.
pub fn just_boosted(premium_since_ms: Option<i64>, now: i64) -> bool {
    premium_since_ms.is_some_and(|t| (0..120_000).contains(&(now - t)))
}

/// When a Discord account was created, from its id.
pub fn snowflake_ms(id: u64) -> i64 {
    ((id >> 22) + 1_420_070_400_000) as i64
}

/// "today", "yesterday", "2 days ago", "5 months ago", "3 years ago": how long ago an account
/// was made, given when it was made.
pub fn made_ago(created_ms: i64) -> String {
    let days = ((now_ms() - created_ms) / 86_400_000).max(0);
    match days {
        0 => "today".into(),
        1 => "yesterday".into(),
        2..60 => format!("{days} days ago"),
        60..730 => format!("{} months ago", days / 30),
        _ => format!("{} years ago", days / 365),
    }
}

/// A stored before and after, for edits and nickname changes. `before` is None when the old
/// text wasn't known (for a nickname: when there was none). Kept as JSON in the entry's summary.
#[derive(Debug, Serialize, Deserialize)]
pub struct Diff {
    pub before: Option<String>,
    pub after: Option<String>,
}

/// The roles a member gained and lost, kept as JSON in the entry's summary.
#[derive(Debug, Serialize, Deserialize)]
pub struct RoleChange {
    pub added: Vec<RoleRef>,
    pub removed: Vec<RoleRef>,
}

/// A role by name and colour, as it was when the change happened.
#[derive(Debug, Serialize, Deserialize)]
pub struct RoleRef {
    pub name: String,
    /// Discord's colour for the role, 0 for none.
    pub color: u32,
}

pub fn diff_json(before: Option<&str>, after: Option<&str>) -> String {
    let d = Diff { before: before.map(|b| text::clip(b, 300)), after: after.map(|a| text::clip(a, 300)) };
    bot_core::serde_json::to_string(&d).unwrap_or_default()
}

pub fn role_change_json(ctx: &Ctx, added: &[u64], removed: &[u64]) -> String {
    let refs = |ids: &[u64]| ids.iter().map(|r| RoleRef { name: role_name(ctx, *r), color: role_color(ctx, *r) }).collect();
    let change = RoleChange { added: refs(added), removed: refs(removed) };
    bot_core::serde_json::to_string(&change).unwrap_or_default()
}

fn role_color(ctx: &Ctx, role: u64) -> u32 {
    ctx.cache.role(Id::new(role)).map(|r| r.colors.primary_color).unwrap_or(0)
}

/// The file name at the end of an attachment URL.
pub fn file_name(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("file").to_owned()
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip_or(s: &str, max: usize, empty: &str) -> String {
    if s.trim().is_empty() { empty.to_owned() } else { text::clip(s, max) }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{} {word}s", text::num(n as i64)) }
}

/// `<t:...:R>`: Discord shows "3 days ago" in the reader's own clock.
fn rel(ms: i64) -> String {
    format!("<t:{}:R>", ms / 1000)
}

fn role_mentions(roles: &[u64], max: usize) -> String {
    if roles.is_empty() {
        return "None".into();
    }
    let mut out: Vec<String> = roles.iter().take(max).map(|r| format!("<@&{r}>")).collect();
    if roles.len() > max {
        out.push(format!("and {} more", roles.len() - max));
    }
    out.join(" ")
}

pub(crate) fn role_name(ctx: &Ctx, role: u64) -> String {
    ctx.cache.role(Id::new(role)).map(|r| r.name.clone()).unwrap_or_else(|| "deleted role".into())
}

pub(crate) fn channel_label(ctx: &Ctx, channel: u64) -> String {
    match ctx.cache.channel(Id::new(channel)).and_then(|c| c.name.clone()) {
        Some(n) => format!("#{n}"),
        None => "#deleted-channel".into(),
    }
}

/// A calm embed with the member as the author line and the time as the timestamp.
fn embed(ctx: &Ctx, guild: u64, title: &str, color: u32, who: Option<u64>) -> EmbedBuilder {
    let mut b = EmbedBuilder::new().title(title).color(color);
    if let Ok(ts) = Timestamp::from_micros(now_ms() * 1000) {
        b = b.timestamp(ts);
    }
    if let Some(user) = who {
        let hash = ctx.cache.user(Id::new(user)).and_then(|u| u.avatar.map(|a| a.to_string()));
        let mut author = EmbedAuthorBuilder::new(ctx.display_name(guild, user));
        if let Ok(icon) = ImageSource::url(avatar_url(user, hash.as_deref())) {
            author = author.icon_url(icon);
        }
        b = b.author(author).footer(EmbedFooterBuilder::new(format!("User ID {user}")));
    }
    b
}

fn field(b: EmbedBuilder, name: &str, value: &str, inline: bool) -> EmbedBuilder {
    let f = EmbedFieldBuilder::new(name, if value.is_empty() { "None" } else { value });
    b.field(if inline { f.inline() } else { f })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_diff_finds_both_sides() {
        let (added, removed) = role_diff(&[1, 2, 3], &[2, 3, 4, 5]);
        assert_eq!(added, vec![4, 5]);
        assert_eq!(removed, vec![1]);
        assert_eq!(role_diff(&[1, 2], &[2, 1]), (vec![], vec![]));
        assert_eq!(role_diff(&[], &[9]), (vec![9], vec![]));
    }

    #[test]
    fn embed_only_updates_are_not_edits() {
        let now = 1_000_000_000;
        // Same text with a copy of the old message: a link unfurling, not an edit.
        assert!(!edit_changed(Some("look https://x.dev"), "look https://x.dev", Some(now - 500), now));
        assert!(edit_changed(Some("look"), "look again", Some(now - 500), now));
        // No copy of the old message: only a fresh edit stamp counts.
        assert!(edit_changed(None, "look again", Some(now - 2_000), now));
        assert!(!edit_changed(None, "look again", Some(now - 3_600_000), now));
        assert!(!edit_changed(None, "look again", None, now));
    }

    #[test]
    fn timeouts_start_change_and_end() {
        let now = 1_000_000;
        assert_eq!(timeout_change(None, Some(now + 600_000), now), Some(TimeoutChange::Started(now + 600_000)));
        assert_eq!(timeout_change(Some(now + 600_000), None, now), Some(TimeoutChange::Ended));
        assert_eq!(timeout_change(Some(now + 600_000), Some(now + 600_000), now), None);
        assert_eq!(timeout_change(Some(now + 1_000), Some(now + 9_000), now), Some(TimeoutChange::Started(now + 9_000)));
        // An old, already expired timeout is not a timeout.
        assert_eq!(timeout_change(Some(now - 5_000), None, now), None);
        assert_eq!(timeout_change(None, Some(now - 5_000), now), None);
    }

    #[test]
    fn boosts_are_recent_premium_since() {
        let now = 10_000_000;
        assert!(just_boosted(Some(now - 5_000), now));
        assert!(!just_boosted(Some(now - 86_400_000), now));
        assert!(!just_boosted(None, now));
    }

    #[test]
    fn account_age_comes_from_the_id() {
        // 2020-01-01T00:00:00Z as a snowflake.
        let ms = 1_577_836_800_000i64;
        let id = ((ms - 1_420_070_400_000) as u64) << 22;
        assert_eq!(snowflake_ms(id), ms);
    }

    #[test]
    fn file_names_come_from_urls() {
        assert_eq!(file_name("https://cdn.discordapp.com/attachments/1/2/cat.png?ex=1&hm=2"), "cat.png");
        assert_eq!(file_name("https://x.dev/"), "file");
    }

    #[test]
    fn routing_follows_switches_and_channels() {
        let mut s = Settings { channel: Some(10), ..Settings::default() };
        assert_eq!(s.route(Kind::Join), Some(10));
        assert_eq!(s.route(Kind::VoiceJoin), None);
        s.message_channel = Some(20);
        assert_eq!(s.route(Kind::MessageEdit), Some(20));
        assert_eq!(s.route(Kind::Ban), Some(10));
        s.ban = false;
        assert_eq!(s.route(Kind::Ban), None);
        assert!(s.skips_channel(20));
    }
}
