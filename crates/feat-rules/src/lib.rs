//! Channel rules: per channel, keep it to media, delete old messages, keep a message at the
//! bottom, or start a thread on every message.

mod purge;
mod sticky;
mod web;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bot_core::anyhow::Result;
use bot_core::discord::is_not_found;
use bot_core::twilight_model::channel::Message;
use bot_core::twilight_model::channel::message::{AllowedMentions, MessageType};
use bot_core::twilight_model::gateway::event::Event;
use bot_core::twilight_model::guild::Permissions;
use bot_core::twilight_model::id::Id;
use bot_core::{Ctx, Feature, FeatureMeta, Group, Icon, Migration, NavItem, feature_async, text};
use serde::{Deserialize, Serialize};

pub const KEY: &str = "rules";
/// The most channels one server can give rules to.
pub const MAX_CHANNELS: usize = 25;
/// How long the media-only notice stays up.
const NOTICE_LIFETIME: Duration = Duration::from_secs(6);
/// At most one media-only notice per channel this often.
const NOTICE_EVERY: Duration = Duration::from_secs(10);

pub const DEFAULT_THREAD_NAME: &str = "{user} · {date}";

/// What one channel does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub channel: u64,
    /// Delete messages with no attachment and no link.
    pub media_only: bool,
    /// Say why, in a notice that removes itself.
    pub media_notice: bool,
    /// Delete messages older than this, in minutes. 0 is off.
    pub purge_after_mins: i64,
    /// Kept at the bottom of the channel. Empty is off.
    pub sticky: String,
    /// Repost the sticky message after this many new messages.
    pub sticky_every: i64,
    /// Or once the channel has been quiet this long after a new message, in minutes. 0 is off.
    pub sticky_quiet_mins: i64,
    /// Start a thread on every message.
    pub thread: bool,
    /// `{user}`, `{date}` and `{text}` are filled in.
    pub thread_name: String,
}

impl Default for Rule {
    fn default() -> Self {
        Self {
            channel: 0,
            media_only: false,
            media_notice: true,
            purge_after_mins: 0,
            sticky: String::new(),
            sticky_every: 10,
            sticky_quiet_mins: 0,
            thread: false,
            thread_name: DEFAULT_THREAD_NAME.into(),
        }
    }
}

impl Rule {
    pub fn has_sticky(&self) -> bool {
        !self.sticky.trim().is_empty()
    }

    /// Whether the rule does anything at all.
    pub fn is_empty(&self) -> bool {
        !self.media_only && self.purge_after_mins == 0 && !self.has_sticky() && !self.thread
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub channels: Vec<Rule>,
}

impl Settings {
    pub fn rule(&self, channel: u64) -> Option<&Rule> {
        self.channels.iter().find(|r| r.channel == channel)
    }
}

const MIGRATIONS: &[Migration] = &[Migration {
    name: "001_rules",
    sql: "
        CREATE TABLE rules_sticky (
            channel_id INTEGER PRIMARY KEY, guild_id INTEGER NOT NULL,
            message_id INTEGER NOT NULL, posted_at INTEGER NOT NULL);
    ",
}];

#[derive(Default)]
pub struct Rules {
    /// Sticky messages waiting to be reposted, per channel.
    sticky: Arc<Mutex<HashMap<u64, sticky::State>>>,
    /// When each channel last got the media-only notice.
    notices: Mutex<HashMap<u64, Instant>>,
    purge: Arc<purge::Runs>,
}

#[feature_async]
impl Feature for Rules {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Channel rules",
            description: "Media-only channels, auto-purge, sticky messages and a thread on every message.",
            group: Group::Tools,
            nav: vec![NavItem { label: "Channel rules", path: "rules", icon: Icon::Rules, about: "Media only, auto-purge, sticky messages and auto-threads, per channel." }],
        }
    }

    fn migrations(&self) -> &'static [Migration] {
        MIGRATIONS
    }

    fn routes(&self) -> bot_core::axum::Router<Ctx> {
        web::routes()
    }

    async fn on_event(&self, ctx: &Ctx, event: &Event) -> Result<()> {
        if let Event::MessageCreate(m) = event
            && let Some(guild) = m.guild_id
            && !m.author.bot
            && m.webhook_id.is_none()
        {
            self.on_message(ctx, guild.get(), m).await?;
        }
        Ok(())
    }

    async fn tick(&self, ctx: &Ctx) -> Result<()> {
        purge::tick(&self.purge, ctx);
        Ok(())
    }

    fn seed_preview(&self, ctx: &Ctx, guild: u64) -> Result<()> {
        web::seed(ctx, guild)
    }
}

impl Rules {
    /// Every message in a server with the feature on comes through here; channels without a rule
    /// stop at the lookup in the cached settings.
    async fn on_message(&self, ctx: &Ctx, guild: u64, m: &Message) -> Result<()> {
        let s = ctx.settings::<Settings>(guild, KEY)?;
        let Some(rule) = s.rule(m.channel_id.get()) else { return Ok(()) };
        let said = matches!(m.kind, MessageType::Regular | MessageType::Reply);
        if said && rule.media_only && !has_media(m) && !exempt(ctx, guild, m) {
            return self.remove_for_media(ctx, guild, m, rule.media_notice).await;
        }
        if rule.has_sticky() {
            sticky::on_message(&self.sticky, ctx, guild, rule);
        }
        if said && rule.thread {
            let name = thread_name(&rule.thread_name, &author_name(m), &bot_core::today(), &m.content);
            match ctx.http.create_thread_from_message(m.channel_id, m.id, &name).await {
                Ok(_) => ctx.count(guild, "rules_threads", 1),
                // Another feature (scam protection runs first) already deleted the message.
                Err(e) if is_not_found(&e) => {}
                Err(e) => {
                    bot_core::tracing::warn!(guild, channel = m.channel_id.get(), error = %e, "couldn't start a thread");
                    ctx.notify_once(guild, KEY, "thread", "Auto-thread couldn't start a thread. The bot needs Create Public Threads in that channel.", Some(format!("/g/{guild}/rules")), Duration::from_secs(86_400));
                }
            }
        }
        Ok(())
    }

    async fn remove_for_media(&self, ctx: &Ctx, guild: u64, m: &Message, notice: bool) -> Result<()> {
        if let Err(e) = ctx.http.delete_message(m.channel_id, m.id).await {
            // Already gone: another feature deleted it first, so there's nothing to say.
            if is_not_found(&e) {
                return Ok(());
            }
            bot_core::tracing::warn!(guild, channel = m.channel_id.get(), error = %e, "couldn't remove a message from a media-only channel");
            ctx.notify_once(guild, KEY, "media", "A media-only channel couldn't remove a message. The bot needs Manage Messages there.", Some(format!("/g/{guild}/rules")), Duration::from_secs(86_400));
            return Ok(());
        }
        ctx.count(guild, "rules_media", 1);
        if !notice || !notice_due(&mut self.notices.lock().unwrap(), m.channel_id.get(), Instant::now()) {
            return Ok(());
        }
        let user = m.author.id;
        let mentions = AllowedMentions { users: vec![user], ..Default::default() };
        let sent = ctx
            .http
            .create_message(m.channel_id)
            .content(&media_notice(user.get()))
            .allowed_mentions(Some(&mentions))
            .await;
        if let Ok(r) = sent
            && let Ok(msg) = r.model().await
        {
            let ctx = ctx.clone();
            bot_core::tokio::spawn(async move {
                bot_core::tokio::time::sleep(NOTICE_LIFETIME).await;
                let _ = ctx.http.delete_message(msg.channel_id, msg.id).await;
            });
        }
        Ok(())
    }
}

/// A message with an attachment or a link, or one forwarding a message that has them. Stickers
/// alone don't count.
fn has_media(m: &Message) -> bool {
    !m.attachments.is_empty()
        || has_link(&m.content)
        || m.message_snapshots.iter().any(|s| !s.message.attachments.is_empty() || has_link(&s.message.content))
}

fn has_link(content: &str) -> bool {
    content.split_whitespace().any(|w| {
        // A masked link, `[text](https://…)`, has its address after the "](".
        let w = w.split_once("](").map_or(w, |(_, url)| url);
        let w = w.trim_start_matches(['<', '(', '|']);
        w.len() > 8 && (w.starts_with("https://") || w.starts_with("http://"))
    })
}

/// The server owner and people whose roles grant Manage Messages.
fn exempt(ctx: &Ctx, guild: u64, m: &Message) -> bool {
    let user = m.author.id.get();
    if ctx.cache.guild(Id::new(guild)).is_some_and(|g| g.owner_id().get() == user) {
        return true;
    }
    let roles = m.member.as_ref().map(|p| p.roles.clone()).unwrap_or_default();
    let mut perms = Permissions::empty();
    // The @everyone role shares the server's id.
    for role in std::iter::once(Id::new(guild)).chain(roles) {
        if let Some(r) = ctx.cache.role(role) {
            perms |= r.permissions;
        }
    }
    perms.intersects(Permissions::ADMINISTRATOR | Permissions::MANAGE_MESSAGES)
}

/// True when a channel hasn't had the media-only notice in the last [`NOTICE_EVERY`]; marks it.
fn notice_due(last: &mut HashMap<u64, Instant>, channel: u64, now: Instant) -> bool {
    if last.len() > 500 {
        last.retain(|_, t| now.duration_since(*t) < NOTICE_EVERY);
    }
    match last.get(&channel) {
        Some(t) if now.duration_since(*t) < NOTICE_EVERY => false,
        _ => {
            last.insert(channel, now);
            true
        }
    }
}

/// What the bot says when it removes a message from a media-only channel.
pub(crate) fn media_notice(user: u64) -> String {
    format!("<@{user}>, this channel is for pictures, videos, files and links.")
}

fn author_name(m: &Message) -> String {
    m.member
        .as_ref()
        .and_then(|p| p.nick.clone())
        .or_else(|| m.author.global_name.clone())
        .unwrap_or_else(|| m.author.name.clone())
}

/// A thread's name from the template: `{user}`, `{date}` and `{text}` (the message's first
/// line). Discord allows 100 characters.
pub fn thread_name(template: &str, user: &str, date: &str, content: &str) -> String {
    let first = content.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    let template = if template.trim().is_empty() { DEFAULT_THREAD_NAME } else { template };
    let name = text::fill(template, &[("user", user), ("date", date), ("text", &text::clip(first, 60))]);
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = name.trim_matches(|c: char| c == ':' || c == '-' || c == '·' || c.is_whitespace());
    if name.is_empty() { text::clip(user, 100) } else { text::clip(name, 100) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_count_as_media() {
        assert!(has_link("look https://youtu.be/abc"));
        assert!(has_link("<https://example.com/x>"));
        assert!(!has_link("https:// nope"));
        assert!(!has_link("just words, no link"));
        assert!(!has_link("http"));
        assert!(has_link("[my build](https://example.com/b.png)"));
        assert!(has_link("see [this](<https://example.com/x>)"));
        assert!(!has_link("[not a link](nope)"));
    }

    fn message(content: &str, forwarded: Option<(&str, bool)>) -> Message {
        let snapshots: Vec<_> = forwarded
            .map(|(text, file)| {
                let attachments = if file {
                    vec![bot_core::serde_json::json!({"id": "9", "filename": "a.png", "size": 1, "url": "https://cdn/a.png", "proxy_url": "https://cdn/a.png"})]
                } else {
                    Vec::new()
                };
                bot_core::serde_json::json!({"message": {"attachments": attachments, "content": text, "edited_timestamp": null, "embeds": [], "type": 0, "timestamp": "2026-01-01T00:00:00.000000+00:00"}})
            })
            .into_iter()
            .collect();
        bot_core::serde_json::from_value(bot_core::serde_json::json!({
            "id": "1", "channel_id": "2", "guild_id": "3", "type": 0, "content": content, "timestamp": "2026-01-01T00:00:00.000000+00:00",
            "author": {"id": "4", "username": "mira", "discriminator": "0", "avatar": null},
            "attachments": [], "embeds": [], "mentions": [], "mention_roles": [], "mention_everyone": false, "pinned": false, "tts": false,
            "message_snapshots": snapshots
        }))
        .unwrap()
    }

    #[test]
    fn forwarded_media_counts() {
        assert!(!has_media(&message("hello", None)));
        assert!(has_media(&message("", Some(("", true)))));
        assert!(has_media(&message("", Some(("https://youtu.be/abc", false)))));
        assert!(!has_media(&message("", Some(("just text", false)))));
    }

    #[test]
    fn one_notice_per_channel_every_ten_seconds() {
        let mut last = HashMap::new();
        let t = Instant::now();
        assert!(notice_due(&mut last, 1, t));
        assert!(!notice_due(&mut last, 1, t + Duration::from_secs(9)));
        assert!(notice_due(&mut last, 2, t + Duration::from_secs(9)));
        assert!(notice_due(&mut last, 1, t + Duration::from_secs(10)));
    }

    #[test]
    fn thread_names_from_the_template() {
        assert_eq!(thread_name("{user} · {date}", "Mira", "2026-10-31", "hi"), "Mira · 2026-10-31");
        assert_eq!(thread_name("{text}", "Mira", "2026-10-31", "\n  My build for the boss  \nmore"), "My build for the boss");
        // A picture with no text falls back to the name, without a dangling separator.
        assert_eq!(thread_name("{user}: {text}", "Mira", "d", ""), "Mira");
        assert_eq!(thread_name("{text}", "Mira", "d", ""), "Mira");
        assert_eq!(thread_name("", "Mira", "2026-10-31", ""), "Mira · 2026-10-31");
        assert_eq!(thread_name("{text}", "M", "d", &"x".repeat(300)).chars().count(), 60);
    }

    #[test]
    fn a_rule_with_nothing_on_is_empty() {
        assert!(Rule::default().is_empty());
        assert!(!Rule { sticky: "Read the pins".into(), ..Rule::default() }.is_empty());
        assert!(!Rule { sticky: "  ".into(), ..Rule::default() }.has_sticky());
    }
}
