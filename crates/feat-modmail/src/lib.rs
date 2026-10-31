//! Modmail: members DM the bot, staff answer in a thread per conversation, and the bot carries
//! messages both ways.

mod mail;
mod web;

use std::collections::HashMap;
use std::sync::Mutex;

use bot_core::anyhow::Result;
use bot_core::discord::{CommandCall, ComponentCall};
use bot_core::twilight_model::application::command::{Command, CommandType};
use bot_core::twilight_model::gateway::event::Event;
use bot_core::twilight_util::builder::command::{CommandBuilder, SubCommandBuilder};
use bot_core::{Ctx, Feature, FeatureMeta, Group, Icon, Migration, NavItem, Need, Overview, feature_async, now_ms, text};
use serde::{Deserialize, Serialize};

pub const KEY: &str = "modmail";
const HOUR_MS: i64 = 3_600_000;

pub const DEFAULT_GREETING: &str = "Your message reached the staff of {server}. They answer here, in this DM.";
pub const DEFAULT_CLOSING: &str = "The staff of {server} closed this conversation. Message again any time to start a new one.";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Where conversations go: a text channel gets a thread per conversation, a forum a post.
    /// Modmail is off in a server until this is set.
    pub channel: Option<u64>,
    /// Pinged once when a conversation opens.
    pub staff_role: Option<u64>,
    /// Sent to the member when a conversation opens. `{server}` is the server's name.
    pub greeting: String,
    /// Sent to the member when staff close the conversation.
    pub closing: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self { channel: None, staff_role: None, greeting: DEFAULT_GREETING.into(), closing: DEFAULT_CLOSING.into() }
    }
}

const MIGRATIONS: &[Migration] = &[Migration {
    name: "001_modmail",
    sql: "
        CREATE TABLE modmail (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, user_name TEXT NOT NULL DEFAULT '',
            thread_id INTEGER NOT NULL DEFAULT 0, status TEXT NOT NULL DEFAULT 'open',
            opened_at INTEGER NOT NULL, last_at INTEGER NOT NULL, last_author_id INTEGER NOT NULL,
            preview TEXT NOT NULL DEFAULT '', messages INTEGER NOT NULL DEFAULT 0,
            closed_at INTEGER, closed_by INTEGER);
        CREATE UNIQUE INDEX modmail_one_open ON modmail(guild_id, user_id) WHERE status = 'open';
        CREATE INDEX modmail_by_user ON modmail(user_id, status);
        CREATE INDEX modmail_by_thread ON modmail(thread_id);
        CREATE INDEX modmail_by_guild ON modmail(guild_id, status, last_at);
    ",
}];

#[derive(Default)]
pub struct Modmail {
    /// One DM at a time, so two quick messages can't open two conversations.
    dm_lock: bot_core::tokio::sync::Mutex<()>,
    /// Messages from members who share several servers with modmail on, held until they pick one.
    held: Mutex<HashMap<u64, mail::Held>>,
}

#[feature_async]
impl Feature for Modmail {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Modmail",
            description: "Members DM the bot and staff answer in a thread, without sharing accounts.",
            group: Group::Safety,
            nav: vec![NavItem { label: "Modmail", path: "modmail", icon: Icon::Mail, about: "Members DM the bot; staff answer in a thread per conversation." }],
        }
    }

    fn migrations(&self) -> &'static [Migration] {
        MIGRATIONS
    }

    fn commands(&self) -> Vec<Command> {
        vec![
            CommandBuilder::new("modmail", "Modmail conversations", CommandType::ChatInput)
                .option(SubCommandBuilder::new("close", "Close the modmail conversation in this thread"))
                .build(),
        ]
    }

    fn routes(&self) -> bot_core::axum::Router<Ctx> {
        web::routes()
    }

    async fn on_event(&self, ctx: &Ctx, event: &Event) -> Result<()> {
        match event {
            Event::MessageCreate(m) if !m.author.bot => match m.guild_id {
                None => mail::on_dm(self, ctx, m).await?,
                Some(guild) => mail::on_staff_message(ctx, guild.get(), m).await?,
            },
            Event::ThreadDelete(t) => mail::on_thread_gone(ctx, t.guild_id.get(), t.parent_id.get(), t.id.get()).await?,
            Event::ThreadUpdate(t) => {
                if let (Some(guild), Some(parent), Some(meta)) = (t.guild_id, t.parent_id, &t.thread_metadata)
                    && meta.archived
                    && meta.locked
                {
                    mail::on_thread_gone(ctx, guild.get(), parent.get(), t.id.get()).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn on_command(&self, ctx: &Ctx, call: &CommandCall) -> Result<()> {
        mail::on_command(ctx, call).await
    }

    async fn on_component(&self, ctx: &Ctx, call: &ComponentCall) -> Result<()> {
        mail::on_component(self, ctx, call).await
    }

    fn seed_preview(&self, ctx: &Ctx, guild: u64) -> Result<()> {
        let settings = Settings { channel: Some(guild + 100), staff_role: Some(guild + 100), ..Settings::default() };
        ctx.save_settings(guild, KEY, &settings)?;
        mail::seed(ctx, guild)
    }

    fn overview(&self, ctx: &Ctx, page: &bot_core::web::Page) -> Overview {
        let guild = page.guild_id;
        let now = now_ms();
        let waiting = ctx.db.with(|c| mail::waiting_on_staff(c, guild, now - HOUR_MS, 3)).unwrap_or_default();
        let needs = waiting
            .into_iter()
            .map(|m| Need {
                since: m.last_at,
                people: vec![ctx.member_avatar(guild, m.user_id)],
                icon: Icon::Mail,
                title: format!("Modmail from {} waiting {}", m.name(ctx, guild), mail::span((now - m.last_at) / 1000)),
                detail: if m.preview.is_empty() { "No staff reply yet".into() } else { format!("“{}”", text::clip(&m.preview, 80)) },
                href: page.path("modmail"),
                action: "Open",
                alert: false,
            })
            .collect();
        Overview { needs, ..Overview::default() }
    }
}
