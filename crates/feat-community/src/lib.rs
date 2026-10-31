//! Giveaways, polls and suggestions: slash commands, buttons and the dashboard pages.

mod giveaways;
mod overview;
mod polls;
mod suggestions;
mod web;

use bot_core::anyhow::Result;
use bot_core::discord::{CommandCall, ComponentCall, ModalCall};
use bot_core::twilight_model::application::command::Command;
use bot_core::twilight_model::application::interaction::Interaction;
use bot_core::twilight_model::channel::message::component::{ActionRow, Button, ButtonStyle, Component};
use bot_core::twilight_model::gateway::event::Event;
use bot_core::twilight_model::guild::Permissions;
use bot_core::{Ctx, Feature, FeatureMeta, Group, Icon, Migration, NavItem, feature_async};
use serde::{Deserialize, Serialize};

pub const KEY: &str = "community";

/// The accent colour for embeds that need the person.
pub(crate) const CORAL: u32 = 0xf0806a;
/// Finished or undecided things.
pub(crate) const NEUTRAL: u32 = 0x8e9297;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    /// Where `/suggest` posts. None turns suggestions off.
    pub suggestions_channel: Option<u64>,
    /// A role that may set suggestion statuses, besides members with Manage Messages.
    pub staff_role: Option<u64>,
    /// Open a thread under each suggestion.
    pub threads: bool,
    /// Leave the author's name off the suggestion.
    pub anonymous: bool,
}

#[derive(Default)]
pub struct Community;

const MIGRATIONS: &[Migration] = &[Migration {
    name: "001_community",
    sql: "
        CREATE TABLE giveaways (
            id INTEGER PRIMARY KEY AUTOINCREMENT, guild_id INTEGER NOT NULL, channel_id INTEGER NOT NULL,
            message_id INTEGER, prize TEXT NOT NULL, winners INTEGER NOT NULL DEFAULT 1,
            ends_at INTEGER NOT NULL, required_role INTEGER, created_by INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL DEFAULT 0, ended INTEGER NOT NULL DEFAULT 0,
            winner_ids TEXT NOT NULL DEFAULT '');
        CREATE INDEX giveaways_by_guild ON giveaways(guild_id, ended, ends_at);
        CREATE TABLE giveaway_entries (
            giveaway_id INTEGER NOT NULL, user_id INTEGER NOT NULL, PRIMARY KEY (giveaway_id, user_id));
        CREATE TABLE polls (
            id INTEGER PRIMARY KEY AUTOINCREMENT, guild_id INTEGER NOT NULL, channel_id INTEGER NOT NULL,
            message_id INTEGER NOT NULL, question TEXT NOT NULL, answers TEXT NOT NULL,
            multi INTEGER NOT NULL DEFAULT 0, ends_at INTEGER NOT NULL, created_by INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL DEFAULT 0, results TEXT);
        CREATE INDEX polls_by_guild ON polls(guild_id, created_at DESC);
        CREATE TABLE suggestions (
            id INTEGER PRIMARY KEY AUTOINCREMENT, guild_id INTEGER NOT NULL, channel_id INTEGER NOT NULL,
            message_id INTEGER, author_id INTEGER NOT NULL, content TEXT NOT NULL, anonymous INTEGER NOT NULL DEFAULT 0,
            status TEXT NOT NULL DEFAULT 'open', reason TEXT NOT NULL DEFAULT '', staff_id INTEGER,
            created_at INTEGER NOT NULL DEFAULT 0);
        CREATE INDEX suggestions_by_guild ON suggestions(guild_id, created_at DESC);
        CREATE TABLE suggestion_votes (
            suggestion_id INTEGER NOT NULL, user_id INTEGER NOT NULL, vote INTEGER NOT NULL,
            PRIMARY KEY (suggestion_id, user_id));
    ",
}, Migration {
    name: "002_community_times",
    sql: "
        ALTER TABLE giveaways ADD COLUMN ended_at INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE suggestions ADD COLUMN status_at INTEGER NOT NULL DEFAULT 0;
    ",
}, Migration {
    name: "003_community_live",
    sql: "
        ALTER TABLE giveaway_entries ADD COLUMN entered_at INTEGER NOT NULL DEFAULT 0;
        CREATE TABLE poll_votes (
            poll_id INTEGER NOT NULL, user_id INTEGER NOT NULL, answer INTEGER NOT NULL, at INTEGER NOT NULL,
            PRIMARY KEY (poll_id, user_id, answer));
    ",
}, Migration {
    name: "004_giveaway_passed",
    sql: "ALTER TABLE giveaways ADD COLUMN passed_ids TEXT NOT NULL DEFAULT '';",
}, Migration {
    name: "005_suggestion_vote_times",
    sql: "ALTER TABLE suggestion_votes ADD COLUMN at INTEGER NOT NULL DEFAULT 0;",
}, Migration {
    name: "006_giveaway_emoji",
    sql: "ALTER TABLE giveaways ADD COLUMN emoji TEXT NOT NULL DEFAULT '';",
}];

#[feature_async]
impl Feature for Community {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Community",
            description: "Giveaways, polls and a suggestion box.",
            group: Group::Community,
            nav: vec![
                NavItem { label: "Giveaways", path: "community/giveaways", icon: Icon::Gift, about: "Members press Enter in Discord, and winners are picked when time runs out." },
                NavItem { label: "Polls", path: "community/polls", icon: Icon::Poll, about: "Real Discord polls: members vote on the message itself." },
                NavItem { label: "Suggestions", path: "community/suggestions", icon: Icon::Bulb, about: "/suggest posts them, and members vote with buttons." },
            ],
        }
    }

    fn migrations(&self) -> &'static [Migration] {
        MIGRATIONS
    }

    /// Votes on the polls posted from here, as members cast and take them back, so the
    /// dashboard can show live counts.
    async fn on_event(&self, ctx: &Ctx, event: &Event) -> Result<()> {
        match event {
            Event::MessagePollVoteAdd(v) => {
                polls::note_live_vote(v.message_id.get(), v.user_id.get());
                ctx.db.with(|c| polls::vote(c, v.message_id.get(), v.user_id.get(), v.answer_id, true))?;
            }
            Event::MessagePollVoteRemove(v) => {
                polls::note_live_vote(v.message_id.get(), v.user_id.get());
                ctx.db.with(|c| polls::vote(c, v.message_id.get(), v.user_id.get(), v.answer_id, false))?;
            }
            _ => {}
        }
        Ok(())
    }

    fn commands(&self) -> Vec<Command> {
        vec![giveaways::command(), polls::command(), suggestions::command()]
    }

    fn routes(&self) -> bot_core::axum::Router<Ctx> {
        web::routes()
    }

    async fn on_command(&self, ctx: &Ctx, call: &CommandCall) -> Result<()> {
        match call.data.name.as_str() {
            "giveaway" => giveaways::on_command(ctx, call).await,
            "poll" => polls::on_command(ctx, call).await,
            "suggest" => suggestions::on_command(ctx, call).await,
            _ => Ok(()),
        }
    }

    async fn on_component(&self, ctx: &Ctx, call: &ComponentCall) -> Result<()> {
        match call.args().as_slice() {
            ["gw", id] => giveaways::on_enter(ctx, call, id.parse().unwrap_or(0)).await,
            ["sg", rest @ ..] => suggestions::on_component(ctx, call, rest).await,
            _ => Ok(()),
        }
    }

    async fn on_modal(&self, ctx: &Ctx, call: &ModalCall) -> Result<()> {
        match call.args().as_slice() {
            ["sg", rest @ ..] => suggestions::on_modal(ctx, call, rest).await,
            _ => Ok(()),
        }
    }

    async fn tick(&self, ctx: &Ctx) -> Result<()> {
        giveaways::tick(ctx).await;
        polls::tick(ctx).await;
        Ok(())
    }

    fn overview(&self, ctx: &Ctx, page: &bot_core::web::Page) -> bot_core::Overview {
        overview::build(ctx, page)
    }

    fn seed_preview(&self, ctx: &Ctx, guild: u64) -> Result<()> {
        ctx.save_settings(
            guild,
            KEY,
            &Settings { suggestions_channel: Some(guild + 70), staff_role: Some(guild + 100), threads: true, anonymous: false },
        )?;
        giveaways::seed(ctx, guild)?;
        polls::seed(ctx, guild)?;
        suggestions::seed(ctx, guild)?;
        Ok(())
    }
}

// ---- helpers shared by the three parts ----

/// A button for a message. Custom ids start with the feature key.
pub(crate) fn button(id: &str, label: &str, style: ButtonStyle, disabled: bool) -> Component {
    Component::Button(Button {
        id: None,
        custom_id: Some(id.to_owned()),
        disabled,
        emoji: None,
        label: Some(label.to_owned()),
        style,
        url: None,
        sku_id: None,
    })
}

pub(crate) fn row(buttons: Vec<Component>) -> Component {
    Component::ActionRow(ActionRow { id: None, components: buttons })
}

/// A discord.com link to a message.
pub(crate) fn message_url(guild: u64, channel: u64, message: u64) -> String {
    format!("https://discord.com/channels/{guild}/{channel}/{message}")
}

fn span(secs: i64) -> String {
    match secs {
        0..60 => "under a minute".into(),
        60..3600 => format!("{} min", secs / 60),
        3600..172_800 => format!("{} h", secs / 3600),
        _ => format!("{} days", secs / 86_400),
    }
}

/// Does the member who triggered this interaction count as staff? Manage Messages, an
/// administrator, or the configured staff role.
pub(crate) fn is_staff(i: &Interaction, settings: &Settings) -> bool {
    let Some(member) = i.member.as_ref() else { return false };
    let perms = member.permissions.unwrap_or_else(Permissions::empty);
    perms.intersects(Permissions::ADMINISTRATOR | Permissions::MANAGE_MESSAGES)
        || settings.staff_role.is_some_and(|r| member.roles.iter().any(|id| id.get() == r))
}

/// Ids stored in the database as signed integers.
pub(crate) fn id_i64(id: u64) -> i64 {
    id as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_read_naturally() {
        assert_eq!(span(30), "under a minute");
        assert_eq!(span(600), "10 min");
        assert_eq!(span(7200), "2 h");
        assert_eq!(span(3 * 86_400), "3 days");
    }

    #[test]
    fn migrations_apply() {
        let db = bot_core::Db::memory().unwrap();
        db.migrate(KEY, MIGRATIONS).unwrap();
    }
}
