//! Support tickets as private threads, and a starboard for popular messages.

mod starboard;
mod tickets;
mod web;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use bot_core::anyhow::Result;
use bot_core::discord::{CommandCall, ComponentCall, ModalCall};
use bot_core::twilight_model::application::command::{Command, CommandType};
use bot_core::twilight_model::gateway::event::Event;
use bot_core::twilight_util::builder::command::{CommandBuilder, StringBuilder, SubCommandBuilder};
use bot_core::{Ctx, Feature, FeatureMeta, Group, Icon, Migration, NavItem, feature_async};
use serde::{Deserialize, Serialize};

pub const KEY: &str = "tickets";

pub const DEFAULT_PANEL_TITLE: &str = "Support";
pub const DEFAULT_PANEL_TEXT: &str =
    "Need help from the team? Open a ticket and describe the problem. You get a private thread with the staff.";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Where the panel is posted and where ticket threads are created.
    pub ticket_channel: Option<u64>,
    /// Pinged once when a ticket opens, and allowed to close any ticket.
    pub staff_role: Option<u64>,
    /// Tickets one member can have open at once.
    pub max_open: i64,
    /// Close tickets with no messages for this many days. 0 is off.
    pub auto_close_days: i64,
    pub panel_title: String,
    pub panel_text: String,
    /// The last panel posted: `(channel, message)`. Sending a new one removes this one.
    pub panel: Option<(u64, u64)>,

    /// The starboard is off until a channel is picked.
    pub star_channel: Option<u64>,
    /// A unicode emoji, or a custom one pasted as `<:name:id>`.
    pub star_emoji: String,
    pub star_threshold: i64,
    /// Take the post down when the count falls back under the threshold.
    pub star_remove_below: bool,
    /// Whether a message's author starring their own message counts.
    pub star_self: bool,
    pub star_ignored: Vec<u64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ticket_channel: None,
            staff_role: None,
            max_open: 1,
            auto_close_days: 0,
            panel_title: DEFAULT_PANEL_TITLE.into(),
            panel_text: DEFAULT_PANEL_TEXT.into(),
            panel: None,
            star_channel: None,
            star_emoji: "⭐".into(),
            star_threshold: 3,
            star_remove_below: true,
            star_self: false,
            star_ignored: Vec::new(),
        }
    }
}

const MIGRATIONS: &[Migration] = &[Migration {
    name: "001_tickets",
    sql: "
        CREATE TABLE tickets (
            guild_id INTEGER NOT NULL, number INTEGER NOT NULL, opener_id INTEGER NOT NULL,
            subject TEXT NOT NULL, details TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'open',
            thread_id INTEGER NOT NULL DEFAULT 0, opened_at INTEGER NOT NULL,
            closed_at INTEGER, closed_by INTEGER, close_reason TEXT,
            PRIMARY KEY (guild_id, number));
        CREATE INDEX tickets_by_thread ON tickets(thread_id);
        CREATE INDEX tickets_by_opener ON tickets(guild_id, opener_id, status);
        CREATE TABLE starboard_posts (
            guild_id INTEGER NOT NULL, message_id INTEGER NOT NULL, channel_id INTEGER NOT NULL,
            author_id INTEGER NOT NULL, post_channel_id INTEGER NOT NULL, post_id INTEGER NOT NULL,
            stars INTEGER NOT NULL, preview TEXT NOT NULL DEFAULT '', created_at INTEGER NOT NULL,
            removed INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (guild_id, message_id));
        CREATE INDEX starboard_by_stars ON starboard_posts(guild_id, stars DESC);
        CREATE INDEX starboard_by_post ON starboard_posts(guild_id, post_id);
    ",
}];

#[derive(Default)]
pub struct Tickets {
    /// One starboard update at a time, so two quick reactions can't post the same message twice.
    star_lock: bot_core::tokio::sync::Mutex<()>,
    /// When each server's open tickets were last checked for idleness.
    swept: Mutex<HashMap<u64, Instant>>,
}

#[feature_async]
impl Feature for Tickets {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Tickets and starboard",
            description: "Support tickets as private threads, and a starboard for popular messages.",
            group: Group::Community,
            nav: vec![
                NavItem { label: "Tickets", path: "tickets", icon: Icon::Ticket },
                NavItem { label: "Starboard", path: "tickets/starboard", icon: Icon::Star },
            ],
        }
    }

    fn migrations(&self) -> &'static [Migration] {
        MIGRATIONS
    }

    fn commands(&self) -> Vec<Command> {
        vec![
            CommandBuilder::new("ticket", "Support tickets", CommandType::ChatInput)
                .option(SubCommandBuilder::new("panel", "Post the Open a ticket panel (staff)"))
                .option(
                    SubCommandBuilder::new("close", "Close the ticket you're in")
                        .option(StringBuilder::new("reason", "Why it's being closed").max_length(200)),
                )
                .build(),
        ]
    }

    fn routes(&self) -> bot_core::axum::Router<Ctx> {
        web::routes()
    }

    async fn on_event(&self, ctx: &Ctx, event: &Event) -> Result<()> {
        match event {
            Event::ReactionAdd(r) => {
                if r.member.as_ref().is_some_and(|m| m.user.bot) {
                    return Ok(());
                }
                starboard::on_reaction(self, ctx, r.guild_id, r.channel_id, r.message_id, &r.emoji).await?;
            }
            Event::ReactionRemove(r) => {
                starboard::on_reaction(self, ctx, r.guild_id, r.channel_id, r.message_id, &r.emoji).await?;
            }
            Event::MessageDelete(m) => starboard::on_message_delete(ctx, m.guild_id, m.channel_id, m.id)?,
            Event::ThreadDelete(t) => tickets::on_thread_gone(ctx, t.guild_id.get(), t.id.get(), "The thread was deleted")?,
            Event::ThreadUpdate(t) => {
                if let (Some(guild), Some(meta)) = (t.guild_id, &t.thread_metadata)
                    && meta.archived
                {
                    tickets::on_thread_gone(ctx, guild.get(), t.id.get(), "The thread was archived in Discord")?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn on_command(&self, ctx: &Ctx, call: &CommandCall) -> Result<()> {
        tickets::on_command(ctx, call).await
    }

    async fn on_component(&self, ctx: &Ctx, call: &ComponentCall) -> Result<()> {
        tickets::on_component(ctx, call).await
    }

    async fn on_modal(&self, ctx: &Ctx, call: &ModalCall) -> Result<()> {
        tickets::on_modal(ctx, call).await
    }

    async fn tick(&self, ctx: &Ctx) -> Result<()> {
        tickets::sweep_idle(self, ctx).await
    }

    fn seed_preview(&self, ctx: &Ctx, guild: u64) -> Result<()> {
        let settings = Settings {
            ticket_channel: Some(guild + 100),
            staff_role: Some(guild + 100),
            max_open: 1,
            auto_close_days: 7,
            star_channel: Some(guild + 90),
            star_ignored: vec![guild + 80],
            ..Settings::default()
        };
        ctx.save_settings(guild, KEY, &settings)?;
        tickets::seed(ctx, guild)?;
        starboard::seed(ctx, guild)
    }
}

impl Tickets {
    pub(crate) fn sweep_due(&self, guild: u64) -> bool {
        let mut swept = self.swept.lock().unwrap();
        if swept.get(&guild).is_some_and(|t| t.elapsed().as_secs() < 3600) {
            return false;
        }
        swept.insert(guild, Instant::now());
        true
    }
}
