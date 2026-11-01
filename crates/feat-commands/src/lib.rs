//! Custom commands and the auto-responder: slash commands each server defines on the
//! dashboard (also usable as `!name` in chat), and replies to messages that match a phrase.

mod web;

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bot_core::anyhow::Result;
use bot_core::discord::{CommandCall, Reply};
use bot_core::rusqlite::params;
use bot_core::twilight_model::application::command::{Command, CommandType};
use bot_core::twilight_model::channel::Message;
use bot_core::twilight_model::channel::message::{AllowedMentions, Embed, MessageType};
use bot_core::twilight_model::gateway::event::Event;
use bot_core::twilight_model::id::Id;
use bot_core::twilight_model::util::Timestamp;
use bot_core::twilight_util::builder::command::CommandBuilder;
use bot_core::twilight_util::builder::embed::{EmbedBuilder, ImageSource};
use bot_core::{Ctx, Feature, FeatureMeta, Group, Icon, Migration, NavItem, feature_async, now_ms, text};
use serde::{Deserialize, Serialize};

pub const KEY: &str = "commands";
/// The most custom commands a server can have. Discord allows 100 per server across every
/// feature, and the bot's own take some.
pub const MAX_COMMANDS: usize = 50;
pub const MAX_TRIGGERS: usize = 50;

/// A command a server made: `/name`, and `!name` in chat when text commands are on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomCommand {
    pub id: u32,
    /// Lowercase, `[a-z0-9_-]`, 1 to 32 characters.
    pub name: String,
    pub description: String,
    pub reply: String,
    /// Only the person who used the slash command sees the reply.
    pub ephemeral: bool,
    pub embed: Option<CardEmbed>,
}

/// The embed under a command's reply, with the coral edge.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CardEmbed {
    pub title: String,
    pub description: String,
    /// An https link to a picture.
    pub image: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Contains,
    Exact,
    StartsWith,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Contains => "Contains",
            Mode::Exact => "Is exactly",
            Mode::StartsWith => "Starts with",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Mode::Contains => "contains",
            Mode::Exact => "exact",
            Mode::StartsWith => "starts_with",
        }
    }

    pub fn from_key(k: &str) -> Self {
        match k {
            "exact" => Mode::Exact,
            "starts_with" => Mode::StartsWith,
            _ => Mode::Contains,
        }
    }
}

/// An auto-responder entry: a reply to messages that match `pattern`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Trigger {
    pub id: u32,
    pub mode: Mode,
    /// Stored lowercased; messages are lowercased once before matching.
    pub pattern: String,
    pub reply: String,
    /// Only in these channels and their threads. Empty is everywhere.
    pub channels: Vec<u64>,
    /// Seconds before this trigger answers again in the server.
    pub cooldown_secs: i64,
}

impl Default for Trigger {
    fn default() -> Self {
        Self { id: 0, mode: Mode::Contains, pattern: String::new(), reply: String::new(), channels: Vec::new(), cooldown_secs: 30 }
    }
}

impl Trigger {
    /// Whether an already lowercased message matches. Contains and Starts with match whole
    /// words: "gm" answers "gm all" but not "gmail".
    pub fn matches(&self, lower: &str) -> bool {
        if self.pattern.is_empty() {
            return false;
        }
        match self.mode {
            Mode::Contains => contains_words(lower, &self.pattern),
            Mode::Exact => lower.trim() == self.pattern,
            Mode::StartsWith => {
                let lower = lower.trim_start();
                lower.starts_with(&self.pattern) && after_ok(&self.pattern, &lower[self.pattern.len()..])
            }
        }
    }

    /// Whether it answers in `channel`, given the channel's parent when it's a thread.
    pub fn allowed_in(&self, channel: u64, thread_parent: Option<u64>) -> bool {
        self.channels.is_empty() || self.channels.contains(&channel) || thread_parent.is_some_and(|p| self.channels.contains(&p))
    }
}

/// Whether `phrase` is in `text` with no letter or digit running into it on either side.
fn contains_words(text: &str, phrase: &str) -> bool {
    let mut from = 0;
    while let Some(at) = text[from..].find(phrase).map(|i| from + i) {
        if before_ok(phrase, &text[..at]) && after_ok(phrase, &text[at + phrase.len()..]) {
            return true;
        }
        // Step one character on, so overlapping matches are tried too.
        from = at + text[at..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// A letter or digit can't run into the start of a phrase that starts with one. A phrase
/// that starts with punctuation, like ":)", needs no gap.
fn before_ok(phrase: &str, before: &str) -> bool {
    !phrase.starts_with(char::is_alphanumeric) || !before.ends_with(char::is_alphanumeric)
}

/// The same for the end of the phrase.
fn after_ok(phrase: &str, after: &str) -> bool {
    !phrase.ends_with(char::is_alphanumeric) || !after.starts_with(char::is_alphanumeric)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub commands: Vec<CustomCommand>,
    pub triggers: Vec<Trigger>,
    /// `!name` in chat runs a command too, like the old bot.
    pub text_commands: bool,
    /// The id the next command or trigger gets; ids are never reused, so use counts stay with
    /// the right one.
    pub next_id: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self { commands: Vec::new(), triggers: Vec::new(), text_commands: true, next_id: 1 }
    }
}

impl Settings {
    pub fn command(&self, name: &str) -> Option<&CustomCommand> {
        self.commands.iter().find(|c| c.name == name)
    }

    /// What Discord holds for this server's commands: a change here needs a re-registration.
    pub fn registered(&self) -> Vec<(&str, &str)> {
        self.commands.iter().map(|c| (c.name.as_str(), description(c))).collect()
    }

    /// The first trigger that answers this lowercased message, ignoring cooldowns. `place` is the
    /// channel and its thread parent; None ignores channel limits (the dashboard's test box).
    pub fn first_trigger(&self, lower: &str, place: Option<(u64, Option<u64>)>) -> Option<&Trigger> {
        self.triggers.iter().find(|t| place.is_none_or(|(c, p)| t.allowed_in(c, p)) && t.matches(lower))
    }

    /// Whether a command or trigger with this id is still there. Ids are never reused.
    pub fn has(&self, kind: Kind, id: u32) -> bool {
        match kind {
            Kind::Command => self.commands.iter().any(|c| c.id == id),
            Kind::Trigger => self.triggers.iter().any(|t| t.id == id),
        }
    }

    pub fn take_id(&mut self) -> u32 {
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        id
    }
}

/// Slash command names other features use or may use in this server. A custom command with
/// one of them isn't registered: the bot's own comes first.
pub fn names_elsewhere(ctx: &Ctx, guild: u64) -> HashSet<String> {
    let mut names = HashSet::new();
    for f in ctx.features().iter().filter(|f| f.meta().key != KEY) {
        names.extend(f.commands().into_iter().chain(f.guild_commands(ctx, guild)).map(|c| c.name));
        names.extend(f.reserved_command_names().iter().map(|n| (*n).to_owned()));
    }
    names
}

pub(crate) fn description(c: &CustomCommand) -> &str {
    if c.description.trim().is_empty() { "A custom command" } else { c.description.trim() }
}

/// A command name as Discord takes it: lowercase letters, digits, `-` and `_`, 1 to 32 long.
pub fn valid_name(name: &str) -> bool {
    (1..=32).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// The command named by a `!name` message, lowercased, when the message is one.
pub fn bang_name(content: &str) -> Option<String> {
    let rest = content.strip_prefix('!')?;
    let name = rest.split(char::is_whitespace).next()?.to_lowercase();
    valid_name(&name).then_some(name)
}

/// What fills the placeholders in a reply.
pub struct Fill<'a> {
    pub user: &'a str,
    pub user_id: u64,
    pub server: &'a str,
    pub channel: u64,
    pub members: Option<u64>,
}

impl Fill<'_> {
    pub fn apply(&self, template: &str) -> String {
        let mention = format!("<@{}>", self.user_id);
        let channel = format!("<#{}>", self.channel);
        let members = self.members.map(|n| text::num(n as i64)).unwrap_or_default();
        text::fill(template, &[("user", self.user), ("mention", &mention), ("server", self.server), ("channel", &channel), ("members", &members)])
    }
}

/// The reply as Discord gets it: text, the embed, and a ping only for `{mention}`.
fn build_reply(content: &str, embed: Option<&CardEmbed>, f: &Fill) -> (String, Option<Embed>, AllowedMentions) {
    let text = f.apply(content);
    let embed = embed.map(|e| {
        let mut b = EmbedBuilder::new().color(0xf0806a);
        if !e.title.trim().is_empty() {
            b = b.title(text::clip(&f.apply(&e.title), 256));
        }
        if !e.description.trim().is_empty() {
            b = b.description(text::clip(&f.apply(&e.description), 4000));
        }
        let img = e.image.trim();
        if img.starts_with("https://")
            && let Ok(src) = ImageSource::url(img)
        {
            b = b.image(src);
        }
        b.build()
    });
    // A mention inside an embed never pings, so only the text decides.
    let pings = content.contains("{mention}");
    let mentions = AllowedMentions { users: if pings { vec![Id::new(f.user_id)] } else { Vec::new() }, ..Default::default() };
    (text::clip(&text, 2000), embed, mentions)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Command,
    Trigger,
}

impl Kind {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Kind::Command => "command",
            Kind::Trigger => "trigger",
        }
    }
}

const MIGRATIONS: &[Migration] = &[Migration {
    name: "001_commands",
    sql: "
        CREATE TABLE commands_uses (
            guild_id INTEGER NOT NULL, kind TEXT NOT NULL, item_id INTEGER NOT NULL,
            uses INTEGER NOT NULL DEFAULT 0, last_at INTEGER NOT NULL,
            PRIMARY KEY (guild_id, kind, item_id));
    ",
}];

/// How long a `!name` command stays quiet in a channel after it answers.
const BANG_COOLDOWN_SECS: i64 = 5;

/// What a cooldown holds back: a trigger across the server, or a `!name` command in one channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Quiet {
    Trigger { guild: u64, id: u32 },
    Bang { guild: u64, id: u32, channel: u64 },
}

#[derive(Default)]
pub struct Commands {
    /// When each cooldown ends.
    cooldowns: Mutex<HashMap<Quiet, Instant>>,
    /// Uses since the last flush.
    uses: Mutex<Pending>,
}

/// `(server, kind, id) -> (count, last time)`, waiting for the minute's flush.
type Pending = HashMap<(u64, Kind, u32), (i64, i64)>;

#[feature_async]
impl Feature for Commands {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Custom commands",
            description: "Slash commands you write, and replies to messages that match a phrase.",
            group: Group::Tools,
            nav: vec![NavItem { label: "Custom commands", path: "commands", icon: Icon::Slash, about: "Slash commands you write, and replies to messages that match a phrase." }],
        }
    }

    fn migrations(&self) -> &'static [Migration] {
        MIGRATIONS
    }

    fn guild_commands(&self, ctx: &Ctx, guild: u64) -> Vec<Command> {
        let Ok(s) = ctx.settings::<Settings>(guild, KEY) else { return Vec::new() };
        if s.commands.is_empty() {
            return Vec::new();
        }
        let taken = names_elsewhere(ctx, guild);
        s.registered()
            .into_iter()
            .filter(|(name, _)| !taken.contains(*name))
            .map(|(name, about)| CommandBuilder::new(name, about, CommandType::ChatInput).build())
            .collect()
    }

    fn routes(&self) -> bot_core::axum::Router<Ctx> {
        web::routes()
    }

    async fn on_command(&self, ctx: &Ctx, call: &CommandCall) -> Result<()> {
        let Some(guild) = call.guild_id() else { return Ok(()) };
        let s = ctx.settings::<Settings>(guild, KEY)?;
        let Some(c) = s.command(&call.data.name) else {
            ctx.reply(&call.interaction, Reply::text("That command was removed.").ephemeral()).await?;
            return Ok(());
        };
        let user = call.user_id();
        let name = ctx.display_name(guild, user);
        let server = ctx.guild_name(guild);
        let f = Fill { user: &name, user_id: user, server: &server, channel: call.channel_id().unwrap_or_default(), members: ctx.member_count(guild) };
        let (content, embed, mentions) = build_reply(&c.reply, c.embed.as_ref(), &f);
        let mut reply = Reply { content: (!content.is_empty()).then_some(content), embeds: embed.into_iter().collect(), ..Default::default() }.mentions(mentions);
        if c.ephemeral {
            reply = reply.ephemeral();
        }
        ctx.reply(&call.interaction, reply).await?;
        self.used(guild, Kind::Command, c.id);
        Ok(())
    }

    async fn on_event(&self, ctx: &Ctx, event: &Event) -> Result<()> {
        if let Event::MessageCreate(m) = event
            && let Some(guild) = m.guild_id
            && !m.author.bot
            && m.webhook_id.is_none()
            && written(m.kind)
            && !timed_out(ctx, m)
        {
            self.on_message(ctx, guild.get(), m).await?;
        }
        Ok(())
    }

    async fn tick(&self, ctx: &Ctx) -> Result<()> {
        self.flush(ctx)
    }

    fn seed_preview(&self, ctx: &Ctx, guild: u64) -> Result<()> {
        web::seed(ctx, guild)
    }
}

/// Whether a member wrote the message. "Started a thread", pins and channel renames carry the
/// member as author but are posted by Discord, and shouldn't get a reply.
fn written(kind: MessageType) -> bool {
    matches!(kind, MessageType::Regular | MessageType::Reply)
}

/// The author is timed out, for example by scam protection a moment ago, so nothing answers
/// (or pings) them.
fn timed_out(ctx: &Ctx, m: &Message) -> bool {
    let cached = || m.guild_id.and_then(|g| ctx.cache.member(g, m.author.id)).and_then(|mm| mm.communication_disabled_until());
    let until = m.member.as_ref().and_then(|p| p.communication_disabled_until).or_else(cached);
    until_in_future(until, now_ms())
}

fn until_in_future(until: Option<Timestamp>, now: i64) -> bool {
    until.is_some_and(|t| t.as_micros() / 1000 > now)
}

impl Commands {
    /// Every message in a server with the feature on comes here. A server with no triggers and
    /// no `!` commands stops at the cached settings.
    async fn on_message(&self, ctx: &Ctx, guild: u64, m: &Message) -> Result<()> {
        let s = ctx.settings::<Settings>(guild, KEY)?;
        let bang = s.text_commands && !s.commands.is_empty();
        if s.triggers.is_empty() && !bang {
            return Ok(());
        }
        let content = m.content.trim();
        if bang
            && content.starts_with('!')
            && let Some(c) = bang_name(content).and_then(|n| s.command(&n))
        {
            let quiet = Quiet::Bang { guild, id: c.id, channel: m.channel_id.get() };
            if !cooled_down(&mut self.cooldowns.lock().unwrap(), quiet, BANG_COOLDOWN_SECS, Instant::now()) {
                return Ok(());
            }
            self.send(ctx, guild, m, &c.reply, c.embed.as_ref()).await;
            self.used(guild, Kind::Command, c.id);
            return Ok(());
        }
        if s.triggers.is_empty() || content.is_empty() {
            return Ok(());
        }
        let lower = content.to_lowercase();
        let channel = m.channel_id.get();
        let parent = ctx.cache.channel(m.channel_id).filter(|c| c.kind.is_thread()).and_then(|c| c.parent_id.map(|p| p.get()));
        let Some(t) = s.first_trigger(&lower, Some((channel, parent))) else { return Ok(()) };
        if !cooled_down(&mut self.cooldowns.lock().unwrap(), Quiet::Trigger { guild, id: t.id }, t.cooldown_secs, Instant::now()) {
            return Ok(());
        }
        self.send(ctx, guild, m, &t.reply, None).await;
        self.used(guild, Kind::Trigger, t.id);
        Ok(())
    }

    async fn send(&self, ctx: &Ctx, guild: u64, m: &Message, reply: &str, embed: Option<&CardEmbed>) {
        let name = m.member.as_ref().and_then(|p| p.nick.clone()).or_else(|| m.author.global_name.clone()).unwrap_or_else(|| m.author.name.clone());
        let server = ctx.guild_name(guild);
        let f = Fill { user: &name, user_id: m.author.id.get(), server: &server, channel: m.channel_id.get(), members: ctx.member_count(guild) };
        let (content, embed, mentions) = build_reply(reply, embed, &f);
        let embeds: Vec<Embed> = embed.into_iter().collect();
        let mut req = ctx.http.create_message(m.channel_id).embeds(&embeds).allowed_mentions(Some(&mentions));
        if !content.is_empty() {
            req = req.content(&content);
        }
        if let Err(e) = req.await {
            bot_core::tracing::warn!(guild, channel = m.channel_id.get(), error = %e, "couldn't send a custom reply");
            ctx.notify_once(guild, KEY, "send", bot_core::discord::why_refused(&e), Some(format!("/g/{guild}/commands")), Duration::from_secs(86_400));
        }
    }

    fn used(&self, guild: u64, kind: Kind, id: u32) {
        let mut uses = self.uses.lock().unwrap();
        let e = uses.entry((guild, kind, id)).or_insert((0, 0));
        e.0 += 1;
        e.1 = now_ms();
    }

    /// Write the use counts gathered since the last minute. A command or trigger deleted since
    /// has had its counts forgotten, so its last uses are dropped rather than written back.
    fn flush(&self, ctx: &Ctx) -> Result<()> {
        let drained: Vec<_> = self.uses.lock().unwrap().drain().collect();
        let drained: Vec<_> = drained.into_iter().filter(|((guild, kind, id), _)| ctx.settings::<Settings>(*guild, KEY).map_or(true, |s| s.has(*kind, *id))).collect();
        if drained.is_empty() {
            return Ok(());
        }
        let mut conn = ctx.db.lock();
        let tx = conn.transaction()?;
        for ((guild, kind, id), (n, at)) in drained {
            tx.execute(
                "INSERT INTO commands_uses (guild_id, kind, item_id, uses, last_at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (guild_id, kind, item_id) DO UPDATE SET uses = uses + excluded.uses, last_at = MAX(last_at, excluded.last_at)",
                params![guild as i64, kind.key(), id, n, at],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

/// True when `key` isn't in a cooldown; starts one of `secs`. The map holds when each cooldown
/// ends, and finished ones go once it grows, so it stays small without forgetting a long one.
fn cooled_down<K: std::hash::Hash + Eq>(until: &mut HashMap<K, Instant>, key: K, secs: i64, now: Instant) -> bool {
    if until.len() > 500 {
        until.retain(|_, end| *end > now);
    }
    match until.get(&key) {
        Some(end) if now < *end => false,
        _ => {
            until.insert(key, now + Duration::from_secs(secs.max(0) as u64));
            true
        }
    }
}

/// `(kind, id) -> (uses, last use)`.
pub type Uses = HashMap<(String, u32), (i64, i64)>;

/// Use counts and last use, by kind and id, for a server's dashboard.
pub fn uses(ctx: &Ctx, guild: u64) -> Result<Uses> {
    ctx.db.with(|c| {
        let mut st = c.prepare("SELECT kind, item_id, uses, last_at FROM commands_uses WHERE guild_id = ?1")?;
        st.query_map([guild as i64], |r| Ok(((r.get(0)?, r.get(1)?), (r.get(2)?, r.get(3)?))))?.collect()
    })
}

pub fn forget_uses(ctx: &Ctx, guild: u64, kind: Kind, id: u32) -> Result<()> {
    ctx.db.with(|c| c.execute("DELETE FROM commands_uses WHERE guild_id = ?1 AND kind = ?2 AND item_id = ?3", params![guild as i64, kind.key(), id]))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trigger(mode: Mode, pattern: &str) -> Trigger {
        Trigger { mode, pattern: pattern.into(), reply: "r".into(), ..Trigger::default() }
    }

    #[test]
    fn only_member_messages_get_answers() {
        assert!(written(MessageType::Regular) && written(MessageType::Reply));
        for notice in [MessageType::ThreadCreated, MessageType::ChannelNameChange, MessageType::ChannelMessagePinned, MessageType::UserJoin] {
            assert!(!written(notice), "{notice:?}");
        }
    }

    #[test]
    fn a_timeout_counts_until_it_ends() {
        let at = |secs| Timestamp::from_secs(secs).ok();
        assert!(until_in_future(at(2_000), 1_000_000));
        assert!(!until_in_future(at(500), 1_000_000));
        assert!(!until_in_future(None, 0));
    }

    #[test]
    fn modes_match_lowercased_text() {
        assert!(trigger(Mode::Contains, "when stream").matches("hey, when stream today?"));
        assert!(!trigger(Mode::Contains, "when stream").matches("streaming when?"));
        assert!(trigger(Mode::Exact, "gm").matches("  gm "));
        assert!(!trigger(Mode::Exact, "gm").matches("gm all"));
        assert!(trigger(Mode::StartsWith, "how do i").matches(" how do i get pets"));
        assert!(!trigger(Mode::StartsWith, "how do i").matches("so how do i"));
        assert!(!trigger(Mode::Contains, "").matches("anything"));
    }

    #[test]
    fn contains_and_starts_with_match_whole_words() {
        let gm = trigger(Mode::Contains, "gm");
        assert!(gm.matches("gm"));
        assert!(gm.matches("gm all"));
        assert!(gm.matches("ok, gm!"));
        assert!(gm.matches("my gmail, then gm"));
        assert!(!gm.matches("check your gmail"));
        assert!(!gm.matches("ogm"));
        assert!(!gm.matches("gm2"));
        assert!(!trigger(Mode::Contains, "hi").matches("this"));
        // Letters outside ASCII are letters too.
        assert!(!gm.matches("ägm"));
        assert!(!trigger(Mode::Contains, "café").matches("cafés"));
        assert!(trigger(Mode::Contains, "café").matches("le café."));
        // A match that only fits on a second, overlapping try still counts.
        assert!(trigger(Mode::Contains, "a a").matches("ba a a"));
        // A phrase ending in punctuation needs no gap there.
        assert!(trigger(Mode::Contains, ":)").matches("thanks:)"));
        assert!(trigger(Mode::StartsWith, "hi").matches("hi there"));
        assert!(trigger(Mode::StartsWith, "hi").matches("hi"));
        assert!(trigger(Mode::StartsWith, "hi").matches("hi, all"));
        assert!(!trigger(Mode::StartsWith, "hi").matches("hiya"));
        assert!(!trigger(Mode::Exact, "gm").matches("gmail"));
    }

    #[test]
    fn channel_limits_reach_threads() {
        let t = Trigger { channels: vec![10], ..trigger(Mode::Contains, "x") };
        assert!(t.allowed_in(10, None));
        assert!(t.allowed_in(99, Some(10)));
        assert!(!t.allowed_in(11, None));
        assert!(trigger(Mode::Contains, "x").allowed_in(11, None));
    }

    #[test]
    fn the_first_trigger_that_fits_answers() {
        let s = Settings {
            triggers: vec![
                Trigger { id: 1, channels: vec![5], ..trigger(Mode::Contains, "pets") },
                Trigger { id: 2, ..trigger(Mode::Contains, "pets") },
            ],
            ..Settings::default()
        };
        assert_eq!(s.first_trigger("my pets", Some((5, None))).map(|t| t.id), Some(1));
        assert_eq!(s.first_trigger("my pets", Some((6, None))).map(|t| t.id), Some(2));
        assert_eq!(s.first_trigger("my pets", None).map(|t| t.id), Some(1));
        assert!(s.first_trigger("nothing here", None).is_none());
    }

    #[test]
    fn command_names_and_bang_messages() {
        assert!(valid_name("links"));
        assert!(valid_name("pet-guide_2"));
        assert!(!valid_name(""));
        assert!(!valid_name("Links"));
        assert!(!valid_name("two words"));
        assert!(!valid_name(&"a".repeat(33)));
        assert_eq!(bang_name("!Links please").as_deref(), Some("links"));
        assert_eq!(bang_name("!links").as_deref(), Some("links"));
        assert_eq!(bang_name("! links"), None);
        assert_eq!(bang_name("!!!"), None);
        assert_eq!(bang_name("links"), None);
    }

    #[test]
    fn placeholders_fill() {
        let f = Fill { user: "Mira", user_id: 7, server: "Antho's server", channel: 3, members: Some(1284) };
        assert_eq!(f.apply("{user} in {server}, {members} members. {mention} {channel}"), "Mira in Antho's server, 1,284 members. <@7> <#3>");
        let (_, _, m) = build_reply("hi {mention}", None, &f);
        assert_eq!(m.users.len(), 1);
        let (_, _, m) = build_reply("hi {user}", None, &f);
        assert!(m.users.is_empty());
    }

    #[test]
    fn embeds_take_only_https_pictures() {
        let f = Fill { user: "Mira", user_id: 7, server: "S", channel: 3, members: None };
        let e = CardEmbed { title: "Hi {user}".into(), description: String::new(), image: "http://x.test/a.png".into() };
        let (_, embed, _) = build_reply("", Some(&e), &f);
        let embed = embed.unwrap();
        assert_eq!(embed.title.as_deref(), Some("Hi Mira"));
        assert!(embed.image.is_none());
        let e = CardEmbed { image: "https://x.test/a.png".into(), ..e };
        assert!(build_reply("", Some(&e), &f).1.unwrap().image.is_some());
    }

    #[test]
    fn cooldowns_hold_per_trigger() {
        let mut last = HashMap::new();
        let t = Instant::now();
        assert!(cooled_down(&mut last, (1, 1), 30, t));
        assert!(!cooled_down(&mut last, (1, 1), 30, t + Duration::from_secs(29)));
        assert!(cooled_down(&mut last, (1, 2), 30, t + Duration::from_secs(29)));
        assert!(cooled_down(&mut last, (2, 1), 30, t + Duration::from_secs(29)));
        assert!(cooled_down(&mut last, (1, 1), 30, t + Duration::from_secs(30)));
        assert!(cooled_down(&mut last, (1, 3), 0, t));
        assert!(cooled_down(&mut last, (1, 3), 0, t));
    }

    #[test]
    fn a_long_cooldown_outlives_the_cleanup() {
        let mut until = HashMap::new();
        let t = Instant::now();
        let day = 86_400;
        assert!(cooled_down(&mut until, 0u32, day, t));
        for k in 1..=600 {
            assert!(cooled_down(&mut until, k, 30, t));
        }
        // Two hours on the map is past 500 and cleans up: the short ones go, the day stays.
        let later = t + Duration::from_secs(7200);
        assert!(!cooled_down(&mut until, 0, day, later));
        assert!(until.len() < 10);
        assert!(cooled_down(&mut until, 0, day, t + Duration::from_secs(day as u64)));
    }

    #[test]
    fn bang_commands_cool_down_per_channel() {
        let mut until = HashMap::new();
        let t = Instant::now();
        let rules = |channel| Quiet::Bang { guild: 1, id: 2, channel };
        assert!(cooled_down(&mut until, rules(10), BANG_COOLDOWN_SECS, t));
        assert!(!cooled_down(&mut until, rules(10), BANG_COOLDOWN_SECS, t + Duration::from_secs(4)));
        assert!(cooled_down(&mut until, rules(11), BANG_COOLDOWN_SECS, t + Duration::from_secs(4)));
        // A trigger with the same id has its own cooldown.
        assert!(cooled_down(&mut until, Quiet::Trigger { guild: 1, id: 2 }, 30, t));
        assert!(cooled_down(&mut until, rules(10), BANG_COOLDOWN_SECS, t + Duration::from_secs(5)));
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn a_deleted_commands_last_uses_are_dropped() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.install_features(vec![std::sync::Arc::new(Commands::default())]).unwrap();
        let mut s = Settings::default();
        for name in ["links", "rules"] {
            let id = s.take_id();
            s.commands.push(CustomCommand { id, name: name.into(), reply: "r".into(), ..CustomCommand::default() });
        }
        ctx.save_settings(1, KEY, &s).unwrap();
        let c = Commands::default();
        c.used(1, Kind::Command, 1);
        c.used(1, Kind::Command, 2);
        // /rules is deleted before the minute's flush.
        s.commands.retain(|c| c.id != 2);
        ctx.save_settings(1, KEY, &s).unwrap();
        forget_uses(&ctx, 1, Kind::Command, 2).unwrap();
        c.flush(&ctx).unwrap();
        let uses = uses(&ctx, 1).unwrap();
        assert_eq!(uses.get(&("command".to_owned(), 1)).map(|u| u.0), Some(1));
        assert!(!uses.contains_key(&("command".to_owned(), 2)));
    }

    #[test]
    fn ids_are_never_reused() {
        let mut s = Settings::default();
        assert_eq!(s.take_id(), 1);
        assert_eq!(s.take_id(), 2);
        let mut old = Settings { next_id: 0, ..Settings::default() };
        assert_eq!(old.take_id(), 1);
    }
}
