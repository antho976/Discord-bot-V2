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
use bot_core::twilight_model::channel::message::embed::EmbedField;
use bot_core::twilight_model::channel::message::{AllowedMentions, Embed};
use bot_core::twilight_model::gateway::event::Event;
use bot_core::twilight_model::guild::Permissions;
use bot_core::twilight_model::id::Id;
use bot_core::twilight_model::user::User as DiscordUser;
use bot_core::twilight_model::util::Timestamp;
use bot_core::twilight_util::builder::embed::{EmbedAuthorBuilder, EmbedBuilder, EmbedFieldBuilder, EmbedFooterBuilder, ImageSource};
use bot_core::{Ctx, Feature, FeatureMeta, Group, Icon, Migration, NavItem, feature_async, now_ms, text};
use serde::{Deserialize, Serialize};

use batch::{Batcher, Who};

pub const KEY: &str = "audit";

const GREY: u32 = 0x8b8f98;
const CORAL: u32 = 0xf0806a;
/// Accounts younger than this get flagged when they join.
const NEW_ACCOUNT_MS: i64 = 7 * 86_400_000;
/// Log entries older than this are dropped from the dashboard table.
pub(crate) const KEEP_DAYS: i64 = 14;
/// In place of the text of a message from a channel some readers of the log can't see.
pub(crate) const PRIVATE_NOTE: &str = "Text not logged: the channel is private.";
/// What the AFK feature puts in front of a nickname (`feat_utility::NICK_PREFIX`).
const AFK_PREFIX: &str = "[AFK] ";

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
    /// entry doesn't create another one. A thread or forum post goes with its channel (`parent`).
    fn skips_channel(&self, channel: u64, parent: Option<u64>) -> bool {
        let skips = |c: u64| self.ignored_channels.contains(&c) || [self.channel, self.message_channel, self.member_channel].contains(&Some(c));
        skips(channel) || parent.is_some_and(skips)
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
}, Migration {
    name: "003_audit_mod_reasons",
    sql: "
        ALTER TABLE audit_mod_actions ADD COLUMN channel_id INTEGER;
        ALTER TABLE audit_mod_actions ADD COLUMN reason TEXT;
    ",
}, Migration {
    // Each audit entry explains as many events as it covers, and the moderator an entry was put
    // down to is kept with it, so the dashboard shows what the post said.
    name: "004_audit_attribution",
    sql: "
        ALTER TABLE audit_mod_actions ADD COLUMN covers INTEGER NOT NULL DEFAULT 1;
        ALTER TABLE audit_mod_actions ADD COLUMN used INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE audit_events ADD COLUMN moderator_id INTEGER;
        ALTER TABLE audit_events ADD COLUMN reason TEXT;
    ",
}, Migration {
    // Discord's id for each audit entry, to find it again when Discord raises its count without
    // sending it anew.
    name: "005_audit_entry_ids",
    sql: "ALTER TABLE audit_mod_actions ADD COLUMN entry_id INTEGER;",
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
        let is_bot = |id: u64| s.ignore_bots && (ctx.bot_id() == Some(id) || ctx.cache.user(Id::new(id)).is_some_and(|u| u.bot));
        match event {
            Event::MessageDelete(m) if s.route(Kind::MessageDelete).is_some() => {
                let channel = m.channel_id.get();
                if s.skips_channel(channel, thread_parent(ctx, channel)) {
                    return out;
                }
                let before = ctx.before_message(m.id.get());
                // The bot's own messages it deletes (an old sticky, an expired notice) are known
                // as its own even once the cache has dropped them.
                let own = ctx.own_message_deleted(m.id.get());
                if (own && s.ignore_bots) || before.as_ref().is_some_and(|b| is_bot(b.author_id)) {
                    return out;
                }
                let author = before.as_ref().map(|b| b.author_id).or_else(|| ctx.bot_id().filter(|_| own));
                // Text from a channel some readers of the log can't see stays there.
                let private = private_source(ctx, guild, channel, s.route(Kind::MessageDelete));
                let content = before.as_ref().map(|b| if private { PRIVATE_NOTE } else { b.content.as_str() });
                let files: Vec<String> = before.iter().filter(|_| !private).flat_map(|b| b.attachments.iter().map(|u| file_name(u))).collect();
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
                if s.skips_channel(channel, thread_parent(ctx, channel)) || (s.ignore_bots && m.author.bot) {
                    return out;
                }
                let before = ctx.before_message(m.id.get());
                let edited_ms = m.edited_timestamp.map(|t| t.as_micros() / 1000);
                if !edit_changed(before.as_ref().map(|b| b.content.as_str()), &m.content, edited_ms, now_ms()) {
                    return out;
                }
                let author = m.author.id.get();
                let link = format!("https://discord.com/channels/{guild}/{channel}/{}", m.id.get());
                let mut e = embed(ctx, guild, "Message edited", GREY, Some(author));
                let summary = if private_source(ctx, guild, channel, s.route(Kind::MessageEdit)) {
                    e = e.description(format!("In <#{channel}>, [jump to message]({link})\n{PRIVATE_NOTE}"));
                    PRIVATE_NOTE.to_owned()
                } else {
                    e = e.description(format!("In <#{channel}>, [jump to message]({link})"));
                    e = field(e, "Before", &before.as_ref().map(|b| clip_or(&b.content, 900, "No text")).unwrap_or_else(|| "Not available".into()), false);
                    e = field(e, "After", &clip_or(&m.content, 900, "No text"), false);
                    diff_json(before.as_ref().map(|b| b.content.as_str()), Some(&m.content))
                };
                out.push(Item { guild, kind: Kind::MessageEdit, user: Some(author), channel: Some(channel), summary, embed: e.build() });
            }
            Event::MessageDeleteBulk(m) if s.route(Kind::BulkDelete).is_some() => {
                let channel = m.channel_id.get();
                if s.skips_channel(channel, thread_parent(ctx, channel)) {
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
                let nick = before.as_ref().and_then(|b| b.nick.clone());
                let mut e = embed_for(ctx, guild, "Member left", GREY, &m.user, nick);
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
                    let account = m.user.global_name.as_deref().unwrap_or(&m.user.name);
                    // The AFK prefix going on or off is the bot's doing, not the member's.
                    if s.route(Kind::Nickname).is_some() && before.nick != m.nick && !afk_toggle(before.nick.as_deref(), m.nick.as_deref(), account) {
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
                // Only a ban that is logged speaks for the leave. With bans off, the leave stays.
                if s.route(Kind::Ban).is_none() || (s.ignore_bots && b.user.bot) {
                    return out;
                }
                self.bans.lock().unwrap().insert((guild, user), Instant::now());
                // A leave that came more than a moment ahead of its ban, and hasn't posted yet,
                // is taken back. It was stored seconds ago, so a minute back covers it.
                if self.batcher.retract(guild, Kind::Leave, user)
                    && let Err(e) = store::forget(ctx, guild, Kind::Leave, user, now_ms() - 60_000)
                {
                    bot_core::tracing::warn!(guild, error = %e, "couldn't take back a leave entry");
                }
                let e = embed_for(ctx, guild, "Member banned", CORAL, &b.user, None);
                out.push(Item { guild, kind: Kind::Ban, user: Some(user), channel: None, summary: String::new(), embed: e.build() });
            }
            Event::BanRemove(b) if s.route(Kind::Unban).is_some() => {
                let user = b.user.id.get();
                if s.ignore_bots && b.user.bot {
                    return out;
                }
                let e = embed_for(ctx, guild, "Member unbanned", GREY, &b.user, None);
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
        if let Some(target) = self.queue(ctx, s, item) {
            let (ctx, batcher) = (ctx.clone(), self.batcher.clone());
            bot_core::tokio::spawn(async move { flush(ctx, batcher, target).await });
        }
    }

    /// Store the entry and queue its post. The channel to flush when this started a new batch.
    fn queue(&self, ctx: &Ctx, s: &Settings, item: Item) -> Option<u64> {
        let target = s.route(item.kind)?;
        // The name the post shows, so the page and the channel agree: a member who left or was
        // banned is named from the event, not "Unknown member".
        let name = match (&item.embed.author, item.user) {
            (Some(a), _) => a.name.clone(),
            (None, Some(u)) => ctx.display_name(item.guild, u),
            (None, None) => String::new(),
        };
        let row = match store::record(ctx, item.guild, item.kind, item.user, &name, item.channel, &item.summary) {
            Ok(row) => Some(row),
            Err(e) => {
                bot_core::tracing::warn!(guild = item.guild, error = %e, "couldn't store an audit entry");
                None
            }
        };
        let who = match (item.kind, item.user) {
            (Kind::Ban | Kind::Unban | Kind::Timeout | Kind::TimeoutEnd | Kind::MessageDelete | Kind::Roles | Kind::Leave, Some(target)) => {
                Some(Who { guild: item.guild, kind: item.kind, target, channel: item.channel, at: now_ms(), row })
            }
            _ => None,
        };
        self.batcher.push(target, item.kind, item.embed, who).then_some(target)
    }
}

/// How long after an event its post waits for Discord's audit entry to say who did it.
const LATE_MS: i64 = store::MATCH_AFTER_MS;

/// Wait out the window, then post everything that gathered for the channel.
async fn flush(ctx: Ctx, batcher: Arc<Batcher>, channel: u64) {
    bot_core::tokio::time::sleep(batch::WINDOW).await;
    // Attributed while still queued, so events arriving meanwhile join this batch instead of
    // starting their own and being posted ahead of it.
    attribute(&ctx, &batcher, channel).await;
    let queued = batcher.drain(channel);
    for embeds in batch::chunk(queued.into_iter().map(|q| q.embed).collect()) {
        let sent = ctx.http.create_message(Id::new(channel)).embeds(&embeds).allowed_mentions(Some(&AllowedMentions::default())).await;
        if let Err(e) = sent {
            bot_core::tracing::warn!(channel, error = %e, "couldn't post to the audit log channel");
        }
    }
}

/// Add who did it, and why, to the posts queued for `channel` that a moderator may be behind, and
/// to their stored entries. Each audit entry explains as many events as it covers and no more, in
/// the order the events came. The entry usually comes with the event, before or after it; one
/// still missing gets until `LATE_MS` after its event. A few rounds at most, so events joining the
/// batch while it waits are looked up too without holding it back for long.
async fn attribute(ctx: &Ctx, batcher: &Batcher, channel: u64) {
    for round in 0..4 {
        if round > 0 {
            let Some(last) = batcher.waiting(channel).iter().map(|(_, w)| w.at).max() else { return };
            let wait = LATE_MS - (now_ms() - last);
            if wait <= 0 {
                return;
            }
            bot_core::tokio::time::sleep(Duration::from_millis(wait as u64)).await;
        }
        let waiting = batcher.waiting(channel);
        let Some(first) = waiting.iter().map(|(_, w)| w.at).min() else { return };
        let guild = waiting[0].1.guild;
        let mut actions = match store::mod_actions(ctx, guild, first - store::MATCH_BEFORE_MS) {
            Ok(a) => a,
            Err(e) => {
                bot_core::tracing::warn!(error = %e, "couldn't read moderator actions");
                return;
            }
        };
        let events: Vec<store::Logged> = waiting.iter().map(|(_, w)| store::Logged { kind: w.kind, target: w.target, channel: w.channel, at: w.at }).collect();
        let found = store::assign(&mut actions, &events);
        let mut unexplained = Vec::new();
        for ((i, who), a) in waiting.into_iter().zip(found) {
            match a {
                Some(a) => explain(ctx, batcher, channel, i, &who, &a),
                None if who.kind == Kind::MessageDelete && who.channel.is_some() => unexplained.push((i, who)),
                None => {}
            }
        }
        if !unexplained.is_empty() {
            for (i, who, a) in merged_deletes(ctx, guild, unexplained).await {
                explain(ctx, batcher, channel, i, &who, &a);
            }
        }
    }
}

/// Use up one event of an action for queued post `i`, and name its moderator on the post and the
/// stored entry.
fn explain(ctx: &Ctx, batcher: &Batcher, channel: u64, i: usize, who: &Who, a: &store::ModAction) {
    // A member's own change (to their own roles) is explained, but names nobody.
    let named = a.moderator != who.target;
    match store::use_action(ctx, a, who.row, named) {
        Ok(true) => batcher.settle(channel, i, |e| {
            if named {
                add_moderator(e, who.kind, a.moderator, a.reason.as_deref());
            }
        }),
        // Another event took it first.
        Ok(false) => {}
        Err(e) => bot_core::tracing::warn!(error = %e, "couldn't use a moderator action"),
    }
}

/// How often a server's audit log may be read for [`merged_deletes`], so a mass delete can't
/// send a request per message.
const AUDIT_READ_EVERY: Duration = Duration::from_secs(5);

/// Deletes no entry explains, put down to an entry Discord added them to. Discord adds a
/// moderator's further deletes of one member's messages in one channel to the entry it sent for
/// the first, raising its count without sending it again; so when such an entry is used up, its
/// count is read again from the audit log and the new uses go to these deletes, in order. At
/// most one read a server every [`AUDIT_READ_EVERY`], and only when the bot may view the audit
/// log; anything that goes wrong just leaves the deletes unexplained.
async fn merged_deletes(ctx: &Ctx, guild: u64, unexplained: Vec<(usize, Who)>) -> Vec<(usize, Who, store::ModAction)> {
    let mut recent = match store::recent_deletes(ctx, guild, now_ms() - store::MERGE_MS) {
        Ok(u) => u,
        Err(e) => {
            bot_core::tracing::warn!(guild, error = %e, "couldn't read recent delete entries");
            return Vec::new();
        }
    };
    recent.retain(|(a, _)| unexplained.iter().any(|(_, w)| merges_into(a, w)));
    if recent.is_empty() || !may_read_audit_log(ctx, guild) {
        return Vec::new();
    }
    let Some(g) = Id::new_checked(guild) else { return Vec::new() };
    use bot_core::twilight_model::guild::audit_log::AuditLogEventType;
    // The most Discord gives in one read: an entry a few minutes old can sit behind a busy
    // bot's own deletes.
    let entries = match ctx.http.audit_log(g).action_type(AuditLogEventType::MessageDelete).limit(100).await {
        Ok(r) => match r.model().await {
            Ok(log) => log.entries,
            Err(_) => return Vec::new(),
        },
        Err(e) => {
            bot_core::tracing::debug!(guild, error = %e, "couldn't read the audit log");
            return Vec::new();
        }
    };
    let counts: HashMap<u64, i64> =
        entries.iter().filter_map(|e| Some((e.id.get(), e.options.as_ref()?.count.as_deref()?.parse::<i64>().ok()?))).collect();
    let (raised, out) = hand_out_merged(&mut recent, &counts, unexplained);
    for (action, covers) in raised {
        if let Err(e) = store::raise_covers(ctx, action, covers) {
            bot_core::tracing::warn!(guild, error = %e, "couldn't raise an entry's count");
        }
    }
    out
}

/// A queued event (by its place in the queue) and the entry that explains it.
type Explained = (usize, Who, store::ModAction);

/// From one read of the audit log (`counts`: Discord's count by entry id): each entry whose count
/// rose since it was last read explains up to that rise of the unexplained deletes it could have
/// taken, oldest first. Uses an entry had before (deletes the bot never logged) aren't handed
/// out, so they can't land on a member deleting their own message. Returns the entries' new
/// counts and the deletes explained.
fn hand_out_merged(recent: &mut [(store::ModAction, u64)], counts: &HashMap<u64, i64>, mut unexplained: Vec<(usize, Who)>) -> (Vec<(i64, i64)>, Vec<Explained>) {
    let mut raised = Vec::new();
    let mut fresh: Vec<i64> = Vec::with_capacity(recent.len());
    for (a, entry) in recent.iter_mut() {
        let rise = counts.get(entry).map_or(0, |n| (n - a.covers).max(0));
        if rise > 0 {
            a.covers += rise;
            raised.push((a.id, a.covers));
        }
        fresh.push(rise);
    }
    unexplained.sort_by_key(|(_, w)| w.at);
    let mut out = Vec::new();
    for (i, who) in unexplained {
        let Some(k) = recent.iter().enumerate().position(|(k, (a, _))| fresh[k] > 0 && merges_into(a, &who)) else { continue };
        fresh[k] -= 1;
        recent[k].0.used += 1;
        out.push((i, who, recent[k].0.clone()));
    }
    (raised, out)
}

/// Whether Discord could have added a delete to a delete entry: the same member's message in the
/// same channel, after the entry.
fn merges_into(a: &store::ModAction, who: &Who) -> bool {
    who.kind == Kind::MessageDelete && a.target == Some(who.target) && who.channel.is_some() && a.channel == who.channel && a.at <= who.at + store::MATCH_AFTER_MS
}

/// Whether to read a server's audit log now: the bot may view it, and it wasn't read for this in
/// the last [`AUDIT_READ_EVERY`].
fn may_read_audit_log(ctx: &Ctx, guild: u64) -> bool {
    static READ: std::sync::LazyLock<Mutex<HashMap<u64, Instant>>> = std::sync::LazyLock::new(Default::default);
    if !bot_views_audit_log(ctx, guild) {
        return false;
    }
    let mut read = READ.lock().unwrap();
    read.retain(|_, t| t.elapsed() < AUDIT_READ_EVERY);
    if read.contains_key(&guild) {
        return false;
    }
    read.insert(guild, Instant::now());
    true
}

/// Whether the bot has View Audit Log in a server, from the cache: its roles and @everyone, or
/// owning the server. Not known means no.
fn bot_views_audit_log(ctx: &Ctx, guild: u64) -> bool {
    let (Some(g), Some(bot)) = (Id::new_checked(guild), ctx.bot_id().and_then(Id::new_checked)) else { return false };
    if ctx.cache.guild(g).is_some_and(|x| x.owner_id() == bot) {
        return true;
    }
    let Some(roles) = ctx.cache.member(g, bot).map(|m| m.roles().to_vec()) else { return false };
    let perms = roles.into_iter().chain([g.cast()]).filter_map(|r| ctx.cache.role(r).map(|r| r.permissions)).fold(Permissions::empty(), |p, r| p | r);
    perms.intersects(Permissions::ADMINISTRATOR | Permissions::VIEW_AUDIT_LOG)
}

/// Where the log posts, as saved: each channel once, with what goes there. Message events go to
/// their own channel or the log channel, member events the same, whatever the switches say.
pub fn test_destinations(s: &Settings) -> Vec<(u64, &'static str)> {
    let messages = s.message_channel.or(s.channel);
    let members = s.member_channel.or(s.channel);
    match (messages, members) {
        (Some(a), Some(b)) if a == b => vec![(a, "Message and member events are posted here.")],
        _ => messages
            .map(|c| (c, "Message events are posted here: edits and deletes."))
            .into_iter()
            .chain(members.map(|c| (c, "Member events are posted here: joins, leaves, roles, bans, timeouts and voice.")))
            .collect(),
    }
}

/// A test entry in each channel the log posts in, with the person who sent it as the member.
/// Nothing is stored and nothing happened in the server. Returns the channels posted in.
pub async fn send_test(ctx: &Ctx, guild: u64, s: &Settings, by: u64) -> Result<Vec<u64>> {
    let mut sent = Vec::new();
    for (channel, what) in test_destinations(s) {
        let e = embed(ctx, guild, "Test entry", GREY, Some(by))
            .description(format!("{what}\nSent from the dashboard to check the bot can post in this channel."))
            .footer(EmbedFooterBuilder::new("A test. Nothing happened in the server."))
            .build();
        ctx.http.create_message(Id::new(channel)).embeds(&[e]).allowed_mentions(Some(&AllowedMentions::default())).await?;
        sent.push(channel);
    }
    Ok(sent)
}

/// "By @Mira" and the reason on a posted entry. A leave with a kick behind it becomes a kick.
pub fn add_moderator(e: &mut Embed, kind: Kind, moderator: u64, reason: Option<&str>) {
    if kind == Kind::Leave {
        e.title = Some("Member kicked".into());
    }
    let field = |name: &str, value: String| EmbedField { name: name.into(), value, inline: true };
    e.fields.push(field("By", format!("<@{moderator}>")));
    if let Some(r) = reason {
        e.fields.push(field("Reason", text::clip(r, 500)));
    }
}

// ---- pure helpers ----

/// Whether a nickname change is only the AFK prefix going on or coming off: "[AFK] " and the name
/// they had (cut to Discord's 32 characters), or that taken off again. The bot does it for /afk.
/// `account` is the name shown when there is no nickname.
pub fn afk_toggle(before: Option<&str>, after: Option<&str>, account: &str) -> bool {
    let fits = |name: &str| name.chars().take(32 - AFK_PREFIX.chars().count()).collect::<String>();
    match (before.and_then(|b| b.strip_prefix(AFK_PREFIX)), after.and_then(|a| a.strip_prefix(AFK_PREFIX))) {
        (None, Some(rest)) => rest == fits(before.unwrap_or(account)),
        (Some(rest), None) => after.is_none_or(|a| fits(a) == rest),
        _ => false,
    }
}

/// Whether a message's text in `source` should stay out of the log posted in `log`: true unless
/// everyone who can read the log can read the source too (a thread counting by its channel, a
/// private thread as hidden from anyone not in it). A channel the cache doesn't know counts as
/// private.
fn private_source(ctx: &Ctx, guild: u64, source: u64, log: Option<u64>) -> bool {
    log.is_some_and(|log| !ctx.readers_can_see(guild, log, source))
}

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
    let who = who.map(|user| {
        let hash = ctx.cache.user(Id::new(user)).and_then(|u| u.avatar.map(|a| a.to_string()));
        (user, ctx.display_name(guild, user), hash)
    });
    titled(title, color, who)
}

/// [`embed`] for an event that carries the member's account: by a leave, ban or unban the cache
/// has usually dropped them, so the name and avatar come from the event.
fn embed_for(ctx: &Ctx, guild: u64, title: &str, color: u32, user: &DiscordUser, nick: Option<String>) -> EmbedBuilder {
    let id = user.id.get();
    let name = event_name(nick, ctx.known_name(guild, id), user);
    titled(title, color, Some((id, name, user.avatar.map(|a| a.to_string()))))
}

/// A member's name from a leave, ban or unban: the nickname they had when they left, else what
/// the cache still knows, else the account's own name.
fn event_name(nick: Option<String>, known: Option<String>, user: &DiscordUser) -> String {
    nick.or(known).or_else(|| user.global_name.clone()).unwrap_or_else(|| user.name.clone())
}

/// `who` is `(user id, name, avatar hash)`.
fn titled(title: &str, color: u32, who: Option<(u64, String, Option<String>)>) -> EmbedBuilder {
    let mut b = EmbedBuilder::new().title(title).color(color);
    if let Ok(ts) = Timestamp::from_micros(now_ms() * 1000) {
        b = b.timestamp(ts);
    }
    if let Some((user, name, hash)) = who {
        let mut author = EmbedAuthorBuilder::new(name);
        if let Ok(icon) = ImageSource::url(avatar_url(user, hash.as_deref())) {
            author = author.icon_url(icon);
        }
        b = b.author(author).footer(EmbedFooterBuilder::new(format!("User ID {user}")));
    }
    b
}

/// The channel a thread or forum post belongs to, from the cache.
fn thread_parent(ctx: &Ctx, channel: u64) -> Option<u64> {
    let c = ctx.cache.channel(Id::new_checked(channel)?)?;
    if c.kind.is_thread() { c.parent_id.map(|p| p.get()) } else { None }
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

    fn action(at: i64, moderator: u64, action: &str, target: u64, channel: Option<u64>) -> store::ModAction {
        store::ModAction { id: 0, at, moderator, action: action.into(), target: Some(target), channel, reason: None, covers: 1, used: 0 }
    }

    fn claim_by(actions: &mut [store::ModAction], kind: Kind, target: u64, channel: Option<u64>, at: i64) -> Option<u64> {
        store::claim(actions, kind, target, channel, at).map(|a| a.moderator)
    }

    #[test]
    fn the_audit_entry_can_come_before_or_after_the_event() {
        let at = 1_000_000;
        assert_eq!(claim_by(&mut [action(at + 800, 9, "ban", 5, None)], Kind::Ban, 5, None, at), Some(9));
        assert_eq!(claim_by(&mut [action(at - 800, 9, "ban", 5, None)], Kind::Ban, 5, None, at), Some(9));
        // Too far apart, another member, or another kind of action: nobody.
        assert_eq!(claim_by(&mut [action(at - 30_000, 9, "ban", 5, None)], Kind::Ban, 5, None, at), None);
        assert_eq!(claim_by(&mut [action(at + 30_000, 9, "ban", 5, None)], Kind::Ban, 5, None, at), None);
        assert_eq!(claim_by(&mut [action(at + 800, 9, "ban", 5, None)], Kind::Ban, 6, None, at), None);
        assert_eq!(claim_by(&mut [action(at + 800, 9, "ban", 5, None)], Kind::Unban, 5, None, at), None);
        assert_eq!(claim_by(&mut [action(at + 800, 9, "ban", 5, None)], Kind::Join, 5, None, at), None);
    }

    #[test]
    fn one_audit_entry_explains_one_event() {
        let at = 1_000_000;
        // A moderator deletes one message; the author deletes another of theirs a moment later.
        let mut actions = [action(at + 300, 9, "delete", 5, Some(20))];
        assert_eq!(claim_by(&mut actions, Kind::MessageDelete, 5, Some(20), at), Some(9));
        assert_eq!(claim_by(&mut actions, Kind::MessageDelete, 5, Some(20), at + 2_000), None);
        // Discord counts three deletes on one entry.
        let mut actions = [store::ModAction { covers: 3, ..action(at + 300, 9, "delete", 5, Some(20)) }];
        for i in 0..3 {
            assert_eq!(claim_by(&mut actions, Kind::MessageDelete, 5, Some(20), at + i * 100), Some(9));
        }
        assert_eq!(claim_by(&mut actions, Kind::MessageDelete, 5, Some(20), at + 400), None);
    }

    #[test]
    fn an_entry_after_the_event_wins_over_one_before() {
        let at = 1_000_000;
        let mut actions = [action(at + 1_500, 2, "roles", 5, None), action(at - 200, 1, "roles", 5, None)];
        assert_eq!(claim_by(&mut actions, Kind::Roles, 5, None, at), Some(2));
        // The earlier one is left for the event it came with.
        assert_eq!(claim_by(&mut actions, Kind::Roles, 5, None, at - 300), Some(1));
    }

    #[test]
    fn several_at_once_each_get_their_own_moderator() {
        let at = 1_000_000;
        // Newest first, as `mod_actions` returns them: two moderators change one member's roles
        // seconds apart, and a third bans two members together.
        let mut actions = [
            action(at + 9_000, 2, "roles", 5, None),
            action(at + 300, 1, "roles", 5, None),
            action(at + 200, 3, "ban", 6, None),
            action(at + 100, 3, "ban", 7, None),
        ];
        assert_eq!(claim_by(&mut actions, Kind::Roles, 5, None, at), Some(1));
        assert_eq!(claim_by(&mut actions, Kind::Roles, 5, None, at + 8_500), Some(2));
        assert_eq!(claim_by(&mut actions, Kind::Ban, 6, None, at), Some(3));
        assert_eq!(claim_by(&mut actions, Kind::Ban, 7, None, at), Some(3));
    }

    #[test]
    fn each_entry_goes_to_its_nearest_event_whatever_the_queue_order() {
        let at = 1_000_000;
        // The author deletes a message of theirs at t-1s, queued first; a moderator deletes
        // another of theirs at t, and its entry comes at t+0.3s.
        let mut actions = [action(at + 300, 9, "delete", 5, Some(20))];
        let own = store::Logged { kind: Kind::MessageDelete, target: 5, channel: Some(20), at: at - 1_000 };
        let moderators = store::Logged { at, ..own };
        let found: Vec<Option<u64>> = store::assign(&mut actions, &[own, moderators]).into_iter().map(|a| a.map(|a| a.moderator)).collect();
        assert_eq!(found, [None, Some(9)]);
        assert_eq!(actions[0].used, 1);
        // An entry covering two goes to both, and one event never takes two entries.
        let mut actions = [store::ModAction { covers: 2, ..action(at + 300, 9, "delete", 5, Some(20)) }, action(at + 200, 8, "delete", 5, Some(20))];
        let found: Vec<Option<u64>> = store::assign(&mut actions, &[own, moderators]).into_iter().map(|a| a.map(|a| a.moderator)).collect();
        assert_eq!(found, [Some(9), Some(8)]);
        assert_eq!((actions[0].used, actions[1].used), (1, 1));
    }

    #[test]
    fn later_deletes_can_merge_into_an_earlier_entry() {
        let at = 1_000_000;
        let entry = action(at, 9, "delete", 5, Some(20));
        let who = |target, channel, at| Who { guild: 1, kind: Kind::MessageDelete, target, channel, at, row: None };
        assert!(merges_into(&entry, &who(5, Some(20), at + 120_000)));
        // Another member, another channel, or before the entry.
        assert!(!merges_into(&entry, &who(6, Some(20), at + 120_000)));
        assert!(!merges_into(&entry, &who(5, Some(21), at + 120_000)));
        assert!(!merges_into(&entry, &who(5, None, at + 120_000)));
        assert!(!merges_into(&entry, &who(5, Some(20), at - 60_000)));
        assert!(!merges_into(&entry, &Who { kind: Kind::Ban, ..who(5, Some(20), at + 1_000) }));
    }

    #[test]
    fn only_the_rise_in_discords_count_is_handed_out() {
        let at = 1_000_000;
        let who = |target, at| Who { guild: 1, kind: Kind::MessageDelete, target, channel: Some(20), at, row: None };
        // A moderator's delete the bot never logged: entry 777 (count 1) with its use untaken.
        let mut recent = vec![(action(at, 9, "delete", 5, Some(20)), 777u64)];
        assert_eq!((recent[0].0.covers, recent[0].0.used), (1, 0));
        // Discord still says 1: a member deleting their own message later gets no name.
        let (raised, out) = hand_out_merged(&mut recent, &HashMap::from([(777, 1)]), vec![(0, who(5, at + 60_000))]);
        assert!(raised.is_empty() && out.is_empty());
        // The moderator deletes two more and Discord says 3: two deletes are named, oldest
        // first, and a third waits for the next rise.
        let (raised, out) = hand_out_merged(&mut recent, &HashMap::from([(777, 3)]), vec![(2, who(5, at + 90_000)), (1, who(5, at + 80_000)), (3, who(5, at + 95_000))]);
        assert_eq!(raised, [(recent[0].0.id, 3)]);
        assert_eq!(out.iter().map(|(i, _, a)| (*i, a.moderator)).collect::<Vec<_>>(), [(1, 9), (2, 9)]);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn a_used_up_delete_entry_can_be_raised_to_discords_count() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.install_features(vec![Arc::new(Audit::default())]).unwrap();
        let entry = |id: u64, count: &str| -> bot_core::twilight_model::guild::audit_log::AuditLogEntry {
            bot_core::serde_json::from_value(bot_core::serde_json::json!({
                "id": id.to_string(), "guild_id": "1", "user_id": "9", "target_id": "5", "action_type": 72,
                "options": {"channel_id": "20", "count": count}, "changes": []
            }))
            .unwrap()
        };
        store::record_moderator(&ctx, &entry(777, "1")).unwrap();
        let since = now_ms() - store::MERGE_MS;
        // Listed whether or not its use was taken.
        let a = store::mod_actions(&ctx, 1, since).unwrap().remove(0);
        assert_eq!(store::recent_deletes(&ctx, 1, since).unwrap().iter().map(|(a, e)| (a.id, *e)).collect::<Vec<_>>(), [(a.id, 777)]);
        assert!(store::use_action(&ctx, &a, None, true).unwrap());
        let used_up = store::recent_deletes(&ctx, 1, since).unwrap();
        assert_eq!(used_up.iter().map(|(a, e)| (a.id, *e)).collect::<Vec<_>>(), [(a.id, 777)]);
        // Discord's count went to 3: two more deletes are explained, and it's never lowered.
        store::raise_covers(&ctx, a.id, 3).unwrap();
        store::raise_covers(&ctx, a.id, 2).unwrap();
        let again = store::mod_actions(&ctx, 1, since).unwrap();
        assert_eq!((again[0].covers, again[0].used), (3, 1));
        // Without the bot's own id the audit log isn't read.
        assert!(!may_read_audit_log(&ctx, 1));
    }

    #[test]
    fn a_deleted_message_matches_its_channel() {
        let at = 1_000_000;
        // The author deleting their own message in another channel at the same moment.
        assert_eq!(claim_by(&mut [action(at + 100, 9, "delete", 5, Some(20))], Kind::MessageDelete, 5, Some(21), at), None);
        assert_eq!(claim_by(&mut [action(at + 100, 9, "delete", 5, Some(20))], Kind::MessageDelete, 5, Some(20), at), Some(9));
    }

    #[test]
    fn posts_say_who_and_why() {
        let mut e = embed_without_ctx("Member left");
        add_moderator(&mut e, Kind::Leave, 9, Some("raid account"));
        assert_eq!(e.title.as_deref(), Some("Member kicked"));
        let fields: Vec<(&str, &str)> = e.fields.iter().map(|f| (f.name.as_str(), f.value.as_str())).collect();
        assert_eq!(fields, [("By", "<@9>"), ("Reason", "raid account")]);
        let mut e = embed_without_ctx("Member banned");
        add_moderator(&mut e, Kind::Ban, 9, None);
        assert_eq!((e.title.as_deref(), e.fields.len()), (Some("Member banned"), 1));
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn a_post_waits_a_moment_for_a_late_audit_entry() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.install_features(vec![Arc::new(Audit::default())]).unwrap();
        let now = now_ms();
        let b = Batcher::default();
        let push = |kind, target, at| b.push(5, kind, embed_without_ctx("x"), Some(Who { guild: 1, kind, target, channel: None, at, row: None }));
        // A ban whose entry is already in, a timeout whose entry comes while the post waits, and
        // a member who left on their own.
        let record = |ctx: &Ctx, action: &str, target: i64| {
            ctx.db
                .with(|c| {
                    c.execute(
                        "INSERT INTO audit_mod_actions (guild_id, at, moderator_id, action, target_id, reason) VALUES (1, ?1, 9, ?2, ?3, 'spam')",
                        bot_core::rusqlite::params![now_ms(), action, target],
                    )
                })
                .unwrap();
        };
        record(&ctx, "ban", 5);
        push(Kind::Ban, 5, now - 4_000);
        push(Kind::Timeout, 6, now - 3_500);
        push(Kind::Leave, 7, now - 3_500);
        let late = ctx.clone();
        bot_core::tokio::spawn(async move {
            bot_core::tokio::time::sleep(Duration::from_millis(400)).await;
            record(&late, "timeout", 6);
        });
        // An event that comes while the batch waits joins it, behind the others.
        let attributing = attribute(&ctx, &b, 5);
        let joining = async {
            bot_core::tokio::time::sleep(Duration::from_millis(200)).await;
            assert!(!b.push(5, Kind::Join, embed_without_ctx("joined"), None), "no second window while the first waits");
        };
        bot_core::tokio::join!(attributing, joining);
        let items = b.drain(5);
        let by = |q: &batch::Queued| q.embed.fields.iter().find(|f| f.name == "By").map(|f| f.value.clone());
        assert_eq!(by(&items[0]).as_deref(), Some("<@9>"));
        assert_eq!(by(&items[1]).as_deref(), Some("<@9>"));
        assert_eq!(by(&items[2]), None);
        assert_eq!(items[2].embed.title.as_deref(), Some("x"));
        assert_eq!(items[3].embed.title.as_deref(), Some("joined"));
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn search_matches_what_the_page_shows() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.install_features(vec![Arc::new(Audit::default())]).unwrap();
        let g = 1;
        let roles = r#"{"added":[{"name":"Mods","color":0}],"removed":[{"name":"Regulars","color":5}]}"#;
        store::record(&ctx, g, Kind::MessageEdit, Some(11), "kasia", Some(20), &diff_json(Some("see you at 8"), Some("see you at 9"))).unwrap();
        store::record(&ctx, g, Kind::Nickname, Some(12), "pavel", None, &diff_json(None, Some("Pavel (afk)"))).unwrap();
        store::record(&ctx, g, Kind::Roles, Some(13), "mira", None, roles).unwrap();
        store::record(&ctx, g, Kind::MessageDelete, Some(14), "noor", Some(21), "anyone up for a raid").unwrap();
        let ban = store::record(&ctx, g, Kind::Ban, Some(15), "lu", None, "").unwrap();
        let by = store::ModAction { id: 1, at: now_ms(), moderator: 99, action: "ban".into(), target: Some(15), channel: None, reason: Some("Selling accounts".into()), covers: 1, used: 0 };
        ctx.db
            .with(|c| c.execute("INSERT INTO audit_mod_actions (id, guild_id, at, moderator_id, action, target_id, reason) VALUES (1, 1, ?1, 99, 'ban', 15, 'Selling accounts')", [by.at]))
            .unwrap();
        assert!(store::use_action(&ctx, &by, Some(ban), true).unwrap());
        assert!(!store::use_action(&ctx, &by, Some(ban), true).unwrap(), "used up");
        let find = |search: store::Search| -> Vec<String> {
            let f = store::Filter { group: store::Group::All, search: Some(search) };
            let mut names: Vec<String> = store::recent(&ctx, g, &f, 50).unwrap().0.into_iter().map(|r| r.user_name).collect();
            names.sort();
            names
        };
        let text = |t: &str| store::Search { text: t.into(), ..Default::default() };
        assert_eq!(find(text("at 9")), ["kasia"]);
        assert_eq!(find(text("at 8")), ["kasia"]);
        assert_eq!(find(text("afk")), ["pavel"]);
        assert_eq!(find(text("regulars")), ["mira"]);
        assert_eq!(find(text("raid")), ["noor"]);
        assert_eq!(find(text("selling")), ["lu"]);
        assert_eq!(find(text("15")), ["lu"]);
        // Older plain-text summaries, and names typed with their capitals.
        store::record(&ctx, g, Kind::Nickname, Some(16), "émile", None, "Old to Néo").unwrap();
        store::record(&ctx, g, Kind::Roles, Some(17), "Émile", None, "Admins given").unwrap();
        assert_eq!(find(text("néo")), ["émile"]);
        assert_eq!(find(text("admins")), ["Émile"]);
        assert_eq!(find(text("Émile")), ["Émile"]);
        // The JSON around the text isn't searched.
        assert!(find(text("before")).is_empty());
        assert!(find(text("color")).is_empty());
        assert!(find(text("added")).is_empty());
        // Names, channels, moderators and labels resolved from the cache.
        assert_eq!(find(store::Search { text: "zz".into(), users: vec![12], ..Default::default() }), ["pavel"]);
        assert_eq!(find(store::Search { text: "zz".into(), channels: vec![21], ..Default::default() }), ["noor"]);
        assert_eq!(find(store::Search { text: "zz".into(), moderators: vec![99], ..Default::default() }), ["lu"]);
        assert_eq!(find(store::Search { text: "zz".into(), kinds: vec![Kind::Nickname], ..Default::default() }), ["pavel", "émile"]);
    }

    fn embed_without_ctx(title: &str) -> Embed {
        EmbedBuilder::new().title(title).build()
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
        assert!(s.skips_channel(20, None));
    }

    #[test]
    fn a_thread_goes_with_its_channel() {
        let s = Settings { channel: Some(10), ignored_channels: vec![30], ..Settings::default() };
        assert!(s.skips_channel(31, Some(30)), "a thread in an ignored channel");
        assert!(s.skips_channel(11, Some(10)), "a thread in the log channel");
        assert!(!s.skips_channel(31, Some(40)));
        assert!(!s.skips_channel(31, None));
    }

    fn account(json: bot_core::serde_json::Value) -> DiscordUser {
        let mut v = bot_core::serde_json::json!({"id": "4242", "username": "lu_99", "discriminator": "0", "avatar": "a1b2c3d4e5f60718293a4b5c6d7e8f90"});
        v.as_object_mut().unwrap().extend(json.as_object().unwrap().clone());
        bot_core::serde_json::from_value(v).unwrap()
    }

    #[test]
    fn a_member_who_is_gone_is_named_from_the_event() {
        let user = account(bot_core::serde_json::json!({"global_name": "Lu"}));
        assert_eq!(event_name(Some("Lulu".into()), Some("Cached".into()), &user), "Lulu");
        assert_eq!(event_name(None, Some("Cached".into()), &user), "Cached");
        assert_eq!(event_name(None, None, &user), "Lu");
        assert_eq!(event_name(None, None, &account(bot_core::serde_json::json!({}))), "lu_99");
    }

    fn audit_ctx() -> Ctx {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.install_features(vec![Arc::new(Audit::default())]).unwrap();
        ctx
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn bans_and_unbans_name_the_member_and_store_that_name() {
        use bot_core::twilight_model::gateway::payload::incoming::{BanAdd, BanRemove};
        let ctx = audit_ctx();
        let audit = Audit::default();
        let s = Settings { channel: Some(10), ..Settings::default() };
        let user = account(bot_core::serde_json::json!({"global_name": "Lu"}));
        for event in [
            Event::BanAdd(BanAdd { guild_id: Id::new(1), user: user.clone() }),
            Event::BanRemove(BanRemove { guild_id: Id::new(1), user: user.clone() }),
        ] {
            let items = audit.items(&ctx, &s, 1, &event).await;
            let author = items[0].embed.author.as_ref().unwrap();
            assert_eq!(author.name, "Lu");
            assert_eq!(author.icon_url.as_deref(), Some("https://cdn.discordapp.com/avatars/4242/a1b2c3d4e5f60718293a4b5c6d7e8f90.png?size=128"));
            for item in items {
                audit.emit(&ctx, &s, item);
            }
        }
        let f = store::Filter { group: store::Group::All, search: None };
        let names: Vec<String> = store::recent(&ctx, 1, &f, 10).unwrap().0.into_iter().map(|r| r.user_name).collect();
        assert_eq!(names, ["Lu", "Lu"]);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn a_late_ban_takes_back_the_leave() {
        use bot_core::twilight_model::gateway::payload::incoming::BanAdd;
        let ctx = audit_ctx();
        let audit = Audit::default();
        let s = Settings { channel: Some(10), ..Settings::default() };
        let user = account(bot_core::serde_json::json!({}));
        // The leave went out to the queue before the ban arrived.
        let leave = Item { guild: 1, kind: Kind::Leave, user: Some(4242), channel: None, summary: "Had no roles".into(), embed: embed_without_ctx("Member left") };
        audit.emit(&ctx, &s, leave);
        for item in audit.items(&ctx, &s, 1, &Event::BanAdd(BanAdd { guild_id: Id::new(1), user })).await {
            audit.emit(&ctx, &s, item);
        }
        let titles: Vec<Option<String>> = audit.batcher.drain(10).into_iter().map(|q| q.embed.title).collect();
        assert_eq!(titles, [Some("Member banned".to_owned())]);
        let f = store::Filter { group: store::Group::All, search: None };
        let kinds: Vec<Kind> = store::recent(&ctx, 1, &f, 10).unwrap().0.into_iter().map(|r| r.kind).collect();
        assert_eq!(kinds, [Kind::Ban]);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn the_bots_own_deletes_are_ignored_with_bots() {
        use bot_core::twilight_model::gateway::payload::incoming::MessageDelete;
        let ctx = audit_ctx();
        let audit = Audit::default();
        let mut s = Settings { channel: Some(10), ..Settings::default() };
        let delete = |id| Event::MessageDelete(MessageDelete { channel_id: Id::new(5), guild_id: Some(Id::new(1)), id: Id::new(id) });
        // An old sticky the cache no longer has.
        ctx.deleting_own_message(77);
        assert!(audit.items(&ctx, &s, 1, &delete(77)).await.is_empty());
        // Someone else's message the cache didn't have is still logged.
        assert_eq!(audit.items(&ctx, &s, 1, &delete(78)).await.len(), 1);
        s.ignore_bots = false;
        assert_eq!(audit.items(&ctx, &s, 1, &delete(77)).await.len(), 1);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn the_dashboard_names_the_moderator_the_post_named() {
        let ctx = audit_ctx();
        let audit = Audit::default();
        let s = Settings { channel: Some(10), ..Settings::default() };
        let item = |user| Item { guild: 1, kind: Kind::MessageDelete, user: Some(user), channel: Some(20), summary: "hi".into(), embed: embed_without_ctx("Message deleted") };
        // One entry from Discord: a moderator deleted one of 5's messages. Then 5 deletes one of
        // their own, and the moderator deletes one of 6's that Discord sent no entry for yet.
        ctx.db
            .with(|c| c.execute("INSERT INTO audit_mod_actions (guild_id, at, moderator_id, action, target_id, channel_id) VALUES (1, ?1, 9, 'delete', 5, 20)", [now_ms()]))
            .unwrap();
        // Queued without the flush task, which would post (and drain) them while this waits.
        audit.queue(&ctx, &s, item(5));
        audit.queue(&ctx, &s, item(5));
        audit.queue(&ctx, &s, item(6));
        // Their posts wait for entries until a few seconds after the events; pretend that's now.
        attribute(&ctx, &audit.batcher, 10).await;
        let by = |q: &batch::Queued| q.embed.fields.iter().find(|f| f.name == "By").map(|f| f.value.clone());
        let posted: Vec<Option<String>> = audit.batcher.drain(10).iter().map(by).collect();
        assert_eq!(posted, [Some("<@9>".to_owned()), None, None]);
        let f = store::Filter { group: store::Group::All, search: None };
        let mut rows = store::recent(&ctx, 1, &f, 10).unwrap().0;
        rows.reverse();
        assert_eq!(rows.iter().map(|r| r.moderator).collect::<Vec<_>>(), [Some(9), None, None]);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn with_bans_off_the_leave_stays() {
        use bot_core::twilight_model::gateway::payload::incoming::BanAdd;
        let ctx = audit_ctx();
        let audit = Audit::default();
        let s = Settings { channel: Some(10), ban: false, ..Settings::default() };
        let leave = Item { guild: 1, kind: Kind::Leave, user: Some(4242), channel: None, summary: "Had no roles".into(), embed: embed_without_ctx("Member left") };
        audit.emit(&ctx, &s, leave);
        let ban = Event::BanAdd(BanAdd { guild_id: Id::new(1), user: account(bot_core::serde_json::json!({})) });
        assert!(audit.items(&ctx, &s, 1, &ban).await.is_empty());
        assert!(!audit.recently_banned(1, 4242));
        let titles: Vec<Option<String>> = audit.batcher.drain(10).into_iter().map(|q| q.embed.title).collect();
        assert_eq!(titles, [Some("Member left".to_owned())]);
    }

    #[test]
    fn the_afk_prefix_is_not_a_nickname_change() {
        assert!(afk_toggle(Some("Mira"), Some("[AFK] Mira"), "mira_01"));
        assert!(afk_toggle(None, Some("[AFK] mira_01"), "mira_01"));
        assert!(afk_toggle(Some("[AFK] Mira"), Some("Mira"), "mira_01"));
        assert!(afk_toggle(Some("[AFK] mira_01"), None, "mira_01"));
        // Cut to fit Discord's 32 characters.
        let long = "n".repeat(32);
        assert!(afk_toggle(Some(&long), Some(&format!("[AFK] {}", "n".repeat(26))), "x"));
        // Real changes.
        assert!(!afk_toggle(Some("Mira"), Some("[AFK] Kasia"), "mira_01"));
        assert!(!afk_toggle(Some("[AFK] Mira"), Some("Kasia"), "mira_01"));
        assert!(!afk_toggle(Some("Mira"), Some("Kasia"), "mira_01"));
        assert!(!afk_toggle(None, Some("Mira"), "mira_01"));
    }

    #[test]
    fn a_test_goes_to_each_channel_once() {
        let mut s = Settings::default();
        assert!(test_destinations(&s).is_empty());
        s.channel = Some(10);
        assert_eq!(test_destinations(&s), vec![(10, "Message and member events are posted here.")]);
        s.message_channel = Some(20);
        let to: Vec<u64> = test_destinations(&s).into_iter().map(|(c, _)| c).collect();
        assert_eq!(to, vec![20, 10]);
        s.member_channel = Some(20);
        assert_eq!(test_destinations(&s).len(), 1);
    }
}
