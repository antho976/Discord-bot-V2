//! Welcome and goodbye messages with a card, auto-roles, roles back on rejoin, invite tracking.

pub mod card;
pub mod invites;
pub mod roles;
mod web;

use bot_core::anyhow::{Result, anyhow};
use bot_core::rusqlite::params;
use bot_core::twilight_model::channel::message::AllowedMentions;
use bot_core::twilight_model::gateway::event::Event;
use bot_core::twilight_model::gateway::payload::incoming::{MemberAdd, MemberRemove};
use bot_core::twilight_model::http::attachment::Attachment;
use bot_core::twilight_model::id::Id;
use bot_core::{Ctx, Feature, FeatureMeta, Group, Icon, Migration, NavItem, feature_async, text};
use serde::{Deserialize, Serialize};

pub const KEY: &str = "welcome";

pub const DEFAULT_WELCOME: &str = "Welcome to {server}, {user}. You are member #{count}.";
pub const DEFAULT_GOODBYE: &str = "{name} left {server}.";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub welcome_on: bool,
    pub welcome_channel: Option<u64>,
    pub welcome_text: String,
    /// Attach the welcome card image to the message.
    pub welcome_card: bool,
    pub goodbye_on: bool,
    pub goodbye_channel: Option<u64>,
    pub goodbye_text: String,
    /// Roles given to everyone who joins.
    pub autoroles: Vec<u64>,
    /// Give the auto-roles to bots too.
    pub autorole_bots: bool,
    /// Give a returning member the roles they left with.
    pub rejoin_on: bool,
    pub rejoin_exclude: Vec<u64>,
    pub rejoin_days: i64,
    pub track_invites: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            welcome_on: true,
            welcome_channel: None,
            welcome_text: DEFAULT_WELCOME.into(),
            welcome_card: false,
            goodbye_on: false,
            goodbye_channel: None,
            goodbye_text: DEFAULT_GOODBYE.into(),
            autoroles: Vec::new(),
            autorole_bots: false,
            rejoin_on: true,
            rejoin_exclude: Vec::new(),
            rejoin_days: 90,
            track_invites: true,
        }
    }
}

/// Fill `{placeholders}` in one pass, so a name that happens to contain `{count}` stays as typed.
/// Unknown placeholders are left alone.
pub fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}').map(|end| (&after[..end], end)) {
            Some((key, end)) if vars.iter().any(|(k, _)| *k == key) => {
                out.push_str(vars.iter().find(|(k, _)| *k == key).map(|(_, v)| *v).unwrap_or_default());
                rest = &after[end + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

const MIGRATIONS: &[Migration] = &[Migration {
    name: "001_welcome",
    sql: "
        CREATE TABLE welcome_invites (
            guild_id INTEGER NOT NULL, code TEXT NOT NULL, uses INTEGER NOT NULL DEFAULT 0,
            max_uses INTEGER NOT NULL DEFAULT 0, inviter_id INTEGER, PRIMARY KEY (guild_id, code));
        CREATE TABLE welcome_invite_state (
            guild_id INTEGER PRIMARY KEY, checked_at INTEGER NOT NULL, ok INTEGER NOT NULL, error TEXT);
        CREATE TABLE welcome_joins (
            id INTEGER PRIMARY KEY AUTOINCREMENT, guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL,
            inviter_id INTEGER, code TEXT, joined_at INTEGER NOT NULL);
        CREATE INDEX welcome_joins_by_time ON welcome_joins(guild_id, joined_at);
        CREATE TABLE welcome_saved_roles (
            guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, roles TEXT NOT NULL, left_at INTEGER NOT NULL,
            restored_at INTEGER, restored_roles TEXT, PRIMARY KEY (guild_id, user_id));
    ",
}];

#[derive(Default)]
pub struct Welcome {
    /// Held while a join reads the invite counts, so two joins at once don't share a reading.
    invite_lock: bot_core::tokio::sync::Mutex<()>,
}

#[feature_async]
impl Feature for Welcome {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Welcome",
            description: "Welcome and goodbye messages with a card, auto-roles, roles back on rejoin, invite tracking.",
            group: Group::Members,
            nav: vec![NavItem { label: "Welcome", path: KEY, icon: Icon::Wave }],
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
            Event::MemberAdd(m) => self.joined(ctx, m).await,
            Event::MemberRemove(m) => self.left(ctx, m).await,
            Event::InviteCreate(i) => {
                let guild = i.guild_id.get();
                if ctx.settings::<Settings>(guild, KEY)?.track_invites {
                    invites::on_invite_create(ctx, guild, &i.code, i.max_uses as i64, i.inviter.as_ref().map(|u| u.id.get()));
                }
                Ok(())
            }
            Event::InviteDelete(i) => {
                let guild = i.guild_id.get();
                if ctx.settings::<Settings>(guild, KEY)?.track_invites {
                    invites::on_invite_delete(ctx, guild, &i.code);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    async fn tick(&self, ctx: &Ctx) -> Result<()> {
        for guild in ctx.enabled_guilds(KEY) {
            let s = ctx.settings::<Settings>(guild, KEY)?;
            if s.track_invites && invites::due(ctx, guild) {
                let _guard = self.invite_lock.lock().await;
                invites::refresh(ctx, guild).await;
            }
            roles::prune(ctx, guild, s.rejoin_days.max(1))?;
        }
        Ok(())
    }

    fn seed_preview(&self, ctx: &Ctx, guild: u64) -> Result<()> {
        seed(ctx, guild)
    }
}

impl Welcome {
    async fn joined(&self, ctx: &Ctx, m: &MemberAdd) -> Result<()> {
        let guild = m.guild_id.get();
        let user = m.user.id.get();
        let bot = m.user.bot;
        let s = ctx.settings::<Settings>(guild, KEY)?;

        invites::record_join(ctx, &self.invite_lock, guild, user, bot, s.track_invites).await;
        give_roles(ctx, &s, guild, user, bot, &m.member.roles.iter().map(|r| r.get()).collect::<Vec<_>>()).await;

        if s.welcome_on && !bot && s.welcome_channel.is_some() {
            let name = m.member.nick.clone().or_else(|| m.user.global_name.clone()).unwrap_or_else(|| m.user.name.clone());
            let avatar = bot_core::discord::avatar_url(user, m.user.avatar.map(|a| a.to_string()).as_deref());
            if let Err(e) = post_welcome(ctx, &s, guild, user, &name, &avatar).await {
                bot_core::tracing::warn!(guild, user, error = %e, "couldn't post a welcome");
            }
        }
        Ok(())
    }

    async fn left(&self, ctx: &Ctx, m: &MemberRemove) -> Result<()> {
        let guild = m.guild_id.get();
        let user = m.user.id.get();
        let s = ctx.settings::<Settings>(guild, KEY)?;

        if s.rejoin_on
            && !m.user.bot
            && let Some(before) = ctx.before_member(guild, user)
        {
            let kept: Vec<u64> = before.roles.into_iter().filter(|r| *r != guild).collect();
            if !kept.is_empty() {
                roles::save(ctx, guild, user, &kept)?;
            }
        }

        if s.goodbye_on
            && !m.user.bot
            && let Some(channel) = s.goodbye_channel
        {
            let name = ctx
                .before_member(guild, user)
                .and_then(|b| b.nick)
                .or_else(|| m.user.global_name.clone())
                .unwrap_or_else(|| m.user.name.clone());
            let count = if s.goodbye_text.contains("{count}") { member_total(ctx, guild).await } else { None };
            let text = fill_message(ctx, &s.goodbye_text, DEFAULT_GOODBYE, guild, &name, &name, count);
            if let Err(e) = ctx.say(channel, &text).await {
                bot_core::tracing::warn!(guild, channel, error = %e, "couldn't post a goodbye");
            }
        }
        Ok(())
    }
}

/// The server's member count, from Discord when it answers and from the cache otherwise.
pub async fn member_total(ctx: &Ctx, guild: u64) -> Option<i64> {
    let live = match ctx.http.guild(Id::new(guild)).with_counts(true).await {
        Ok(res) => res.model().await.ok().and_then(|g| g.approximate_member_count),
        Err(_) => None,
    };
    live.or_else(|| ctx.member_count(guild)).map(|n| n as i64)
}

fn fill_message(ctx: &Ctx, template: &str, default: &str, guild: u64, user: &str, name: &str, count: Option<i64>) -> String {
    let template = if template.trim().is_empty() { default } else { template };
    let count = count.map(text::num).unwrap_or_else(|| "?".into());
    let server = ctx.guild_name(guild);
    text::clip(&fill(template, &[("user", user), ("name", name), ("server", &server), ("count", &count)]), 1900)
}

/// Post the welcome message for `user` in the welcome channel: text, plus the card when on.
/// Mentions only that user. Used for real joins and for the dashboard's test button.
pub async fn post_welcome(ctx: &Ctx, s: &Settings, guild: u64, user: u64, name: &str, avatar_url: &str) -> Result<()> {
    let channel = s.welcome_channel.ok_or_else(|| anyhow!("Pick a welcome channel first."))?;
    let wants_count = s.welcome_card || s.welcome_text.contains("{count}") || s.welcome_text.trim().is_empty();
    let count = if wants_count { member_total(ctx, guild).await } else { None };
    let content = fill_message(ctx, &s.welcome_text, DEFAULT_WELCOME, guild, &format!("<@{user}>"), name, count);
    let mentions = AllowedMentions { users: vec![Id::new(user)], ..Default::default() };

    let mut files = Vec::new();
    if s.welcome_card {
        match card::render(ctx, name, &ctx.guild_name(guild), count, avatar_url).await {
            Ok(png) => files.push(Attachment::from_bytes("welcome.png".into(), png, 1)),
            Err(e) => bot_core::tracing::warn!(guild, error = %e, "couldn't draw the welcome card"),
        }
    }
    ctx.http
        .create_message(Id::new(channel))
        .content(&content)
        .attachments(&files)
        .allowed_mentions(Some(&mentions))
        .await
        .map_err(|e| anyhow!("Discord refused the message: {e}"))?;
    Ok(())
}

/// Auto-roles, plus the roles a returning member left with. Each role is tried on its own so
/// one the bot can't give doesn't block the rest.
async fn give_roles(ctx: &Ctx, s: &Settings, guild: u64, user: u64, bot: bool, has: &[u64]) {
    let mut wanted: Vec<u64> = Vec::new();
    if !bot || s.autorole_bots {
        wanted.extend(&s.autoroles);
    }
    let mut restore: Vec<u64> = Vec::new();
    if s.rejoin_on && !bot {
        match roles::waiting(ctx, guild, user, s.rejoin_days.max(1)) {
            Ok(saved) => restore = saved.into_iter().filter(|r| !s.rejoin_exclude.contains(r) && roles::assignable(ctx, guild, *r)).collect(),
            Err(e) => bot_core::tracing::warn!(guild, user, error = %e, "couldn't read saved roles"),
        }
        wanted.extend(&restore);
    }
    let mut seen = Vec::new();
    let mut given_back = Vec::new();
    for role in wanted {
        if has.contains(&role) || seen.contains(&role) {
            continue;
        }
        seen.push(role);
        if !roles::assignable(ctx, guild, role) {
            bot_core::tracing::warn!(guild, user, role, "the bot can't give this role");
            continue;
        }
        match ctx.http.add_guild_member_role(Id::new(guild), Id::new(user), Id::new(role)).await {
            Ok(_) => {
                if restore.contains(&role) {
                    given_back.push(role);
                }
            }
            Err(e) => bot_core::tracing::warn!(guild, user, role, error = %e, "couldn't give a role"),
        }
    }
    if !given_back.is_empty()
        && let Err(e) = roles::mark_restored(ctx, guild, user, &given_back)
    {
        bot_core::tracing::warn!(guild, user, error = %e, "couldn't note restored roles");
    }
}

fn seed(ctx: &Ctx, guild: u64) -> Result<()> {
    use bot_core::preview::MEMBERS;
    let ch = |n: u64| guild + n * 10;
    let role = |n: u64| guild + n * 100;
    let s = Settings {
        welcome_channel: Some(ch(2)),
        welcome_card: true,
        goodbye_on: true,
        goodbye_channel: Some(ch(4)),
        autoroles: vec![role(3)],
        rejoin_exclude: vec![role(1), role(2)],
        ..Default::default()
    };
    ctx.save_settings(guild, KEY, &s)?;

    let now = bot_core::now_ms();
    let day = 86_400_000;
    let codes = [("hX9d2Q", 0usize), ("Lm3vTz", 1), ("q8WnRb", 0), ("vanity", 99)];
    let hours = [3, 20, 31, 52, 70, 120, 200, 300, 410, 520, 700, 900, 1500, 2100, 2600, 3300];
    ctx.db.with(|c| {
        for (i, h) in hours.iter().enumerate() {
            let user = MEMBERS[(i + 3) % MEMBERS.len()].0;
            let (code, who) = codes[i % codes.len()];
            let inviter = MEMBERS.get(who).map(|m| m.0);
            let known = inviter.is_some() && i % 7 != 5;
            c.execute(
                "INSERT INTO welcome_joins (guild_id, user_id, inviter_id, code, joined_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    guild as i64,
                    user as i64,
                    if known { inviter.map(|u| u as i64) } else { None },
                    if known { Some(code) } else { None },
                    now - h * 3_600_000
                ],
            )?;
        }
        for (code, who, uses, max) in [("hX9d2Q", 0usize, 14, 0), ("Lm3vTz", 1, 6, 0), ("q8WnRb", 0, 3, 25)] {
            c.execute(
                "INSERT INTO welcome_invites (guild_id, code, uses, max_uses, inviter_id) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![guild as i64, code, uses, max, MEMBERS[who].0 as i64],
            )?;
        }
        c.execute(
            "INSERT INTO welcome_invite_state (guild_id, checked_at, ok, error) VALUES (?1, ?2, 1, NULL)",
            params![guild as i64, now - 12 * 60_000],
        )?;
        for (user, saved, restored_ago) in [(7usize, vec![role(3), role(4)], Some(3 * 3_600_000)), (8, vec![role(3), role(5), role(4)], Some(2 * day)), (9, vec![role(3)], None)] {
            let list = saved.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
            c.execute(
                "INSERT INTO welcome_saved_roles (guild_id, user_id, roles, left_at, restored_at, restored_roles) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    guild as i64,
                    MEMBERS[user].0 as i64,
                    list,
                    now - 5 * day,
                    restored_ago.map(|a| now - a),
                    restored_ago.map(|_| list.clone())
                ],
            )?;
        }
        Ok(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::fill;

    #[test]
    fn fills_the_placeholders() {
        let vars = [("user", "<@1>"), ("name", "Mira"), ("server", "Antho's server"), ("count", "1,284")];
        assert_eq!(fill("Welcome {user} to {server}, member #{count}", &vars), "Welcome <@1> to Antho's server, member #1,284");
    }

    #[test]
    fn unknown_and_unclosed_braces_stay() {
        let vars = [("name", "Mira")];
        assert_eq!(fill("{nope} {name} {", &vars), "{nope} Mira {");
        assert_eq!(fill("{{name}}", &vars), "{Mira}");
        assert_eq!(fill("no placeholders", &vars), "no placeholders");
    }

    #[test]
    fn a_value_is_not_filled_again() {
        let vars = [("name", "{count}"), ("count", "9")];
        assert_eq!(fill("{name} is {count}", &vars), "{count} is 9");
    }
}
