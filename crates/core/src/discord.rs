//! The gateway connection, event routing to features, slash-command registration and the
//! helpers features use to answer interactions.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use twilight_gateway::{CloseFrame, Config as GatewayConfig, Event, EventTypeFlags, Intents, Shard, StreamExt as _};
use twilight_model::application::command::CommandOptionChoice;
use twilight_model::application::interaction::application_command::{
    CommandData, CommandDataOption, CommandOptionValue,
};
use twilight_model::application::interaction::message_component::MessageComponentInteractionData;
use twilight_model::application::interaction::modal::{ModalInteractionComponent, ModalInteractionData};
use twilight_model::application::interaction::{Interaction, InteractionData, InteractionType};
use twilight_model::channel::message::component::{ActionRow, Button, ButtonStyle, Component};
use twilight_model::channel::message::{AllowedMentions, Embed, MessageFlags};
use twilight_model::gateway::payload::incoming::GuildCreate;
use twilight_model::gateway::payload::outgoing::RequestGuildMembers;
use twilight_model::http::interaction::{InteractionResponse, InteractionResponseData, InteractionResponseType};
use twilight_model::id::Id;
use twilight_model::id::marker::{ChannelMarker, GuildMarker, RoleMarker, UserMarker};
use twilight_model::user::User;

use crate::{Ctx, Feature};

pub fn intents() -> Intents {
    Intents::GUILDS
        | Intents::GUILD_MEMBERS
        | Intents::GUILD_MODERATION
        | Intents::GUILD_INVITES
        | Intents::GUILD_VOICE_STATES
        | Intents::GUILD_MESSAGES
        | Intents::GUILD_MESSAGE_REACTIONS
        | Intents::MESSAGE_CONTENT
        | Intents::DIRECT_MESSAGES
        | Intents::GUILD_MESSAGE_POLLS
}

/// Connect and run until `shutdown` fires. Reconnects are handled inside twilight.
pub async fn run(ctx: Ctx, mut shutdown: tokio::sync::watch::Receiver<bool>) -> Result<()> {
    let app = ctx
        .http
        .current_user_application()
        .await
        .context("fetching the bot's application (is DISCORD_TOKEN right?)")?
        .model()
        .await?;
    ctx.set_app_id(app.id);

    let config = GatewayConfig::new(ctx.cfg.discord_token.clone(), intents());
    let mut shard = Shard::with_config(twilight_gateway::ShardId::ONE, config);
    let registered: Arc<Mutex<HashSet<u64>>> = Default::default();
    let mut members = MemberRequests::default();

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                shard.close(CloseFrame::NORMAL);
                break;
            }
            item = shard.next_event(EventTypeFlags::all()) => {
                let Some(item) = item else { break };
                let event = match item {
                    Ok(event) => event,
                    Err(source) => {
                        tracing::warn!(?source, "gateway error");
                        continue;
                    }
                };
                snapshot_before(&ctx, &event);
                ctx.cache.update(&event);
                track_member_count(&ctx, &event);
                match &event {
                    Event::Ready(ready) => {
                        ctx.set_bot_id(ready.user.id.get());
                        members.session_started();
                        tracing::info!(user = %ready.user.name, guilds = ready.guilds.len(), "connected to Discord");
                    }
                    Event::GuildCreate(g) => {
                        // The shard paces its commands (120 a minute, heartbeats kept aside),
                        // so this queues behind anything else it has to send.
                        if let Some(ask) = members.for_guild(g) {
                            shard.command(&ask);
                        }
                        let id = g.id().get();
                        if registered.lock().unwrap().insert(id) {
                            let ctx = ctx.clone();
                            tokio::spawn(async move {
                                if let Err(e) = register_commands(&ctx, id).await {
                                    tracing::warn!(guild = id, error = %e, "couldn't register commands");
                                }
                            });
                        }
                    }
                    _ => {}
                }
                let ctx = ctx.clone();
                tokio::spawn(async move { dispatch(ctx, event).await });
            }
        }
    }
    Ok(())
}

/// Which servers' full member lists were asked for in this gateway session. A server over 50
/// members comes with only the members Discord counts as online, so everyone else would be
/// "Unknown member" with a default avatar until they did something. The rest are asked for
/// once per server per session (an empty query with no limit, under the GUILD_MEMBERS intent)
/// and the cache fills from the chunks Discord sends back.
#[derive(Default)]
struct MemberRequests(HashSet<u64>);

impl MemberRequests {
    /// A new session (Ready, not a resume) sends every server again: ask again.
    fn session_started(&mut self) {
        self.0.clear();
    }

    /// The request for a server that just arrived, once per session, and only when its
    /// GuildCreate didn't already bring every member.
    fn for_guild(&mut self, g: &GuildCreate) -> Option<RequestGuildMembers> {
        let GuildCreate::Available(g) = g else { return None };
        if g.member_count.is_some_and(|n| g.members.len() as u64 >= n) || !self.0.insert(g.id.get()) {
            return None;
        }
        Some(RequestGuildMembers::builder(g.id).query("", None))
    }
}

/// A message as the cache had it just before an edit or delete event.
#[derive(Debug, Clone)]
pub struct MessageSnapshot {
    pub id: u64,
    pub channel_id: u64,
    pub guild_id: Option<u64>,
    pub author_id: u64,
    pub content: String,
    pub attachments: Vec<String>,
}

/// A member as the cache had them just before an update or their leaving.
#[derive(Debug, Clone)]
pub struct MemberSnapshot {
    pub guild_id: u64,
    pub user_id: u64,
    pub nick: Option<String>,
    pub roles: Vec<u64>,
    pub joined_at_ms: Option<i64>,
    pub timed_out_until_ms: Option<i64>,
}

#[derive(Default)]
pub(crate) struct Snapshots {
    messages: std::collections::HashMap<u64, (std::time::Instant, MessageSnapshot)>,
    members: std::collections::HashMap<(u64, u64), (std::time::Instant, MemberSnapshot)>,
    voice: std::collections::HashMap<(u64, u64), (std::time::Instant, Option<u64>)>,
}

impl Snapshots {
    fn prune(&mut self) {
        let keep = |t: &std::time::Instant| t.elapsed().as_secs() < 120;
        if self.messages.len() > 500 {
            self.messages.retain(|_, (t, _)| keep(t));
        }
        if self.members.len() > 500 {
            self.members.retain(|_, (t, _)| keep(t));
        }
        if self.voice.len() > 500 {
            self.voice.retain(|_, (t, _)| keep(t));
        }
    }
}

fn snapshot_before(ctx: &Ctx, event: &Event) {
    use twilight_model::id::marker::MessageMarker;
    let now = std::time::Instant::now();
    let message = |id: Id<MessageMarker>| {
        ctx.cache.message(id).map(|m| MessageSnapshot {
            id: id.get(),
            channel_id: m.channel_id().get(),
            guild_id: m.guild_id().map(|g| g.get()),
            author_id: m.author().get(),
            content: m.content().to_owned(),
            attachments: m.attachments().iter().map(|a| a.url.clone()).collect(),
        })
    };
    let member = |g: Id<GuildMarker>, u: Id<UserMarker>| {
        ctx.cache.member(g, u).map(|m| MemberSnapshot {
            guild_id: g.get(),
            user_id: u.get(),
            nick: m.nick().map(str::to_owned),
            roles: m.roles().iter().map(|r| r.get()).collect(),
            joined_at_ms: m.joined_at().map(|t| t.as_micros() / 1000),
            timed_out_until_ms: m.communication_disabled_until().map(|t| t.as_micros() / 1000),
        })
    };
    let mut snaps = ctx.snapshots.lock().unwrap();
    match event {
        Event::MessageUpdate(m) => {
            if let Some(s) = message(m.id) {
                snaps.messages.insert(m.id.get(), (now, s));
            }
        }
        Event::MessageDelete(m) => {
            if let Some(s) = message(m.id) {
                snaps.messages.insert(m.id.get(), (now, s));
            }
        }
        Event::MessageDeleteBulk(m) => {
            for id in &m.ids {
                if let Some(s) = message(*id) {
                    snaps.messages.insert(id.get(), (now, s));
                }
            }
        }
        Event::MemberUpdate(m) => {
            if let Some(s) = member(m.guild_id, m.user.id) {
                snaps.members.insert((m.guild_id.get(), m.user.id.get()), (now, s));
            }
        }
        Event::MemberRemove(m) => {
            if let Some(s) = member(m.guild_id, m.user.id) {
                snaps.members.insert((m.guild_id.get(), m.user.id.get()), (now, s));
            }
        }
        Event::VoiceStateUpdate(v) => {
            if let Some(g) = v.guild_id {
                let before = ctx.cache.voice_state(v.user_id, g).map(|s| s.channel_id().get());
                snaps.voice.insert((g.get(), v.user_id.get()), (now, before));
            }
        }
        _ => {}
    }
    snaps.prune();
}

impl Ctx {
    /// The message as it was before the edit or delete event being handled. Only messages the
    /// cache had (the last 25 per channel since the bot connected) have one.
    pub fn before_message(&self, message_id: u64) -> Option<MessageSnapshot> {
        self.snapshots.lock().unwrap().messages.get(&message_id).map(|(_, s)| s.clone())
    }

    /// The member as they were before the update or leave event being handled.
    pub fn before_member(&self, guild_id: u64, user_id: u64) -> Option<MemberSnapshot> {
        self.snapshots.lock().unwrap().members.get(&(guild_id, user_id)).map(|(_, s)| s.clone())
    }

    /// The voice channel a member was in before the voice update being handled.
    pub fn before_voice(&self, guild_id: u64, user_id: u64) -> Option<u64> {
        self.snapshots.lock().unwrap().voice.get(&(guild_id, user_id)).and_then(|(_, c)| *c)
    }
}

fn track_member_count(ctx: &Ctx, event: &Event) {
    let mut counts = ctx.member_counts.lock().unwrap();
    match event {
        Event::GuildCreate(g) => {
            if let twilight_model::gateway::payload::incoming::GuildCreate::Available(g) = &**g
                && let Some(n) = g.member_count
            {
                counts.insert(g.id.get(), n);
            }
        }
        Event::MemberAdd(m) => {
            if let Some(n) = counts.get_mut(&m.guild_id.get()) {
                *n += 1;
            }
        }
        Event::MemberRemove(m) => {
            if let Some(n) = counts.get_mut(&m.guild_id.get()) {
                *n = n.saturating_sub(1);
            }
        }
        _ => {}
    }
}

/// Replace a server's slash commands with those of the features switched on there, fixed and
/// per server. One at a time, reading the switches once it's this call's turn, so two quick
/// toggles can't finish out of order and leave the older set registered. Does nothing in the
/// preview, which has no Discord connection.
pub async fn register_commands(ctx: &Ctx, guild_id: u64) -> Result<()> {
    if ctx.cfg.preview {
        return Ok(());
    }
    static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _turn = ONE_AT_A_TIME.lock().await;
    let mut seen = HashSet::new();
    let commands: Vec<_> = ctx
        .features()
        .iter()
        .filter(|f| ctx.enabled(guild_id, f.meta().key))
        .flat_map(|f| {
            let mut c = f.commands();
            c.extend(f.guild_commands(ctx, guild_id));
            c
        })
        // Discord refuses the whole set over one repeated name; the first feature keeps it,
        // the same one `route_interaction` sends the call to.
        .filter(|c| seen.insert(c.name.clone()))
        .collect();
    ctx.interaction().set_guild_commands(Id::new(guild_id), &commands).await?;
    tracing::info!(guild = guild_id, count = commands.len(), "registered commands");
    Ok(())
}

async fn dispatch(ctx: Ctx, event: Event) {
    if let Event::InteractionCreate(i) = &event {
        route_interaction(&ctx, &i.0).await;
        return;
    }
    let guild = event.guild_id().map(|g| g.get());
    match &event {
        Event::MessageCreate(m) if !m.author.bot => {
            if let Some(g) = guild {
                ctx.count(g, "messages", 1);
                ctx.mark_active(g, m.author.id.get());
            }
        }
        Event::MemberAdd(m) => ctx.count(m.guild_id.get(), "joins", 1),
        Event::MemberRemove(m) => ctx.count(m.guild_id.get(), "leaves", 1),
        _ => {}
    }
    for f in ctx.features() {
        let key = f.meta().key;
        if let Some(g) = guild
            && !ctx.enabled(g, key) {
                continue;
            }
        if let Err(e) = f.on_event(&ctx, &event).await {
            tracing::warn!(feature = key, kind = ?event.kind(), error = %e, "event handler failed");
        }
    }
}

/// The answer to a slash command no feature declares any more.
const GONE: &str = "That command isn't here any more.";

/// The feature a slash command belongs to: its fixed commands first, then the server's own.
/// None when no feature declares the name, switched on or not.
fn command_owner<'a>(ctx: &'a Ctx, guild: Option<u64>, name: &str) -> Option<&'a Arc<dyn Feature>> {
    ctx.features().iter().find(|f| f.commands().iter().any(|c| c.name == name)).or_else(|| {
        let g = guild?;
        ctx.features().iter().find(|f| f.guild_commands(ctx, g).iter().any(|c| c.name == name))
    })
}

async fn route_interaction(ctx: &Ctx, i: &Interaction) {
    let guild = i.guild_id.map(|g| g.get());
    let usable = |key: &str| guild.is_none_or(|g| ctx.enabled(g, key));
    // A button, form or command from a feature that has since been switched off (Discord can
    // take a moment to drop it) still gets an answer, rather than Discord's "interaction failed".
    let off = || async {
        if i.kind != InteractionType::ApplicationCommandAutocomplete {
            let _ = ctx.reply(i, Reply::text("That's switched off in this server.").ephemeral()).await;
        }
        Ok(())
    };
    let result = match (&i.kind, &i.data) {
        (InteractionType::ApplicationCommand | InteractionType::ApplicationCommandAutocomplete, Some(InteractionData::ApplicationCommand(data))) => {
            match command_owner(ctx, guild, &data.name) {
                Some(f) if usable(f.meta().key) => {
                    let call = CommandCall { interaction: i.clone(), data: (**data).clone() };
                    let res = if i.kind == InteractionType::ApplicationCommandAutocomplete {
                        f.on_autocomplete(ctx, &call).await
                    } else {
                        f.on_command(ctx, &call).await
                    };
                    res.map_err(|e| (f.meta().key, e))
                }
                Some(_) => off().await,
                // A per-server command that was renamed or deleted, still in the member's list
                // until Discord catches up.
                None => {
                    if i.kind != InteractionType::ApplicationCommandAutocomplete {
                        let _ = ctx.reply(i, Reply::text(GONE).ephemeral()).await;
                    }
                    Ok(())
                }
            }
        }
        (InteractionType::MessageComponent, Some(InteractionData::MessageComponent(data))) => {
            let prefix = data.custom_id.split(':').next().unwrap_or_default();
            match ctx.feature(prefix) {
                Some(f) if usable(prefix) => {
                    let call = ComponentCall { interaction: i.clone(), data: (**data).clone() };
                    f.on_component(ctx, &call).await.map_err(|e| (prefix, e))
                }
                _ => off().await,
            }
        }
        (InteractionType::ModalSubmit, Some(InteractionData::ModalSubmit(data))) => {
            let prefix = data.custom_id.split(':').next().unwrap_or_default();
            match ctx.feature(prefix) {
                Some(f) if usable(prefix) => {
                    let call = ModalCall { interaction: i.clone(), data: (**data).clone() };
                    f.on_modal(ctx, &call).await.map_err(|e| (prefix, e))
                }
                _ => off().await,
            }
        }
        _ => Ok(()),
    };
    if let Err((feature, e)) = result {
        tracing::warn!(feature, error = %e, "interaction failed");
        if i.kind != InteractionType::ApplicationCommandAutocomplete {
            let msg = Reply::text("That didn't work. The error is in the bot's log.").ephemeral();
            if ctx.reply(i, msg.clone()).await.is_err() {
                let _ = ctx.followup(i, msg).await;
            }
        }
    }
}

// ---- interaction calls ----

pub struct CommandCall {
    pub interaction: Interaction,
    pub data: CommandData,
}

impl CommandCall {
    pub fn opts(&self) -> Opts<'_> {
        Opts(&self.data.options)
    }
    pub fn guild_id(&self) -> Option<u64> {
        self.interaction.guild_id.map(|g| g.get())
    }
    pub fn user(&self) -> Option<&User> {
        self.interaction.author()
    }
    pub fn user_id(&self) -> u64 {
        self.interaction.author_id().map(|u| u.get()).unwrap_or_default()
    }
    pub fn channel_id(&self) -> Option<u64> {
        self.interaction.channel.as_ref().map(|c| c.id.get())
    }
    /// True when the member running the command has any of these permission bits.
    pub fn member_has(&self, perms: twilight_model::guild::Permissions) -> bool {
        self.interaction
            .member
            .as_ref()
            .and_then(|m| m.permissions)
            .is_some_and(|p| p.contains(twilight_model::guild::Permissions::ADMINISTRATOR) || p.intersects(perms))
    }
}

/// Why Discord refused a post, in words a server owner can act on: a missing permission, a
/// deleted channel, or Discord's own message.
pub fn why_refused(e: &twilight_http::Error) -> String {
    let status = match e.kind() {
        twilight_http::error::ErrorType::Response { status, .. } => Some(status.get()),
        _ => None,
    };
    refusal(status, e)
}

/// [`why_refused`] for an error that may wrap a Discord one.
pub fn why_refused_any(e: &anyhow::Error) -> String {
    match e.downcast_ref::<twilight_http::Error>() {
        Some(http) => why_refused(http),
        None => refusal(None, e),
    }
}

fn refusal(status: Option<u16>, e: &dyn std::fmt::Display) -> String {
    match status {
        Some(403) => "The bot can't post in that channel. Check it can see the channel and send messages there, and attach files if it posts images.".into(),
        Some(404) => "The channel was deleted. Pick another one.".into(),
        _ => format!("Discord refused the message: {e}"),
    }
}

/// True when a Discord REST error is a 404 (the message, channel or member is gone).
pub fn is_not_found(e: &twilight_http::Error) -> bool {
    matches!(e.kind(), twilight_http::error::ErrorType::Response { status, .. } if status.get() == 404)
}

pub struct ComponentCall {
    pub interaction: Interaction,
    pub data: MessageComponentInteractionData,
}

impl ComponentCall {
    /// The custom id after the `"{key}:"` prefix, split on `:`.
    pub fn args(&self) -> Vec<&str> {
        self.data.custom_id.split(':').skip(1).collect()
    }
    pub fn guild_id(&self) -> Option<u64> {
        self.interaction.guild_id.map(|g| g.get())
    }
    pub fn user_id(&self) -> u64 {
        self.interaction.author_id().map(|u| u.get()).unwrap_or_default()
    }
    /// True when the clicking member has any of these permission bits in the server.
    pub fn member_has(&self, perms: twilight_model::guild::Permissions) -> bool {
        self.interaction
            .member
            .as_ref()
            .and_then(|m| m.permissions)
            .is_some_and(|p| p.contains(twilight_model::guild::Permissions::ADMINISTRATOR) || p.intersects(perms))
    }
}

pub struct ModalCall {
    pub interaction: Interaction,
    pub data: ModalInteractionData,
}

impl ModalCall {
    pub fn args(&self) -> Vec<&str> {
        self.data.custom_id.split(':').skip(1).collect()
    }
    /// The value typed into the text input with this custom id.
    pub fn value(&self, custom_id: &str) -> Option<String> {
        fn walk(c: &ModalInteractionComponent, id: &str) -> Option<String> {
            match c {
                ModalInteractionComponent::TextInput(t) if t.custom_id == id => Some(t.value.clone()),
                ModalInteractionComponent::Label(l) => walk(&l.component, id),
                ModalInteractionComponent::ActionRow(r) => r.components.iter().find_map(|c| walk(c, id)),
                _ => None,
            }
        }
        self.data.components.iter().find_map(|c| walk(c, custom_id))
    }
    pub fn guild_id(&self) -> Option<u64> {
        self.interaction.guild_id.map(|g| g.get())
    }
    pub fn user_id(&self) -> u64 {
        self.interaction.author_id().map(|u| u.get()).unwrap_or_default()
    }
}

/// Typed access to slash-command options.
#[derive(Clone, Copy)]
pub struct Opts<'a>(pub &'a [CommandDataOption]);

impl<'a> Opts<'a> {
    fn get(&self, name: &str) -> Option<&'a CommandOptionValue> {
        self.0.iter().find(|o| o.name == name).map(|o| &o.value)
    }
    /// The chosen subcommand (or subcommand group) and its options.
    pub fn sub(&self) -> Option<(&'a str, Opts<'a>)> {
        self.0.iter().find_map(|o| match &o.value {
            CommandOptionValue::SubCommand(inner) | CommandOptionValue::SubCommandGroup(inner) => {
                Some((o.name.as_str(), Opts(inner.as_slice())))
            }
            _ => None,
        })
    }
    pub fn str(&self, name: &str) -> Option<&'a str> {
        match self.get(name)? {
            CommandOptionValue::String(s) => Some(s),
            _ => None,
        }
    }
    pub fn int(&self, name: &str) -> Option<i64> {
        match self.get(name)? {
            CommandOptionValue::Integer(n) => Some(*n),
            _ => None,
        }
    }
    pub fn num(&self, name: &str) -> Option<f64> {
        match self.get(name)? {
            CommandOptionValue::Number(n) => Some(*n),
            CommandOptionValue::Integer(n) => Some(*n as f64),
            _ => None,
        }
    }
    pub fn bool(&self, name: &str) -> Option<bool> {
        match self.get(name)? {
            CommandOptionValue::Boolean(b) => Some(*b),
            _ => None,
        }
    }
    pub fn user(&self, name: &str) -> Option<Id<UserMarker>> {
        match self.get(name)? {
            CommandOptionValue::User(u) => Some(*u),
            _ => None,
        }
    }
    pub fn channel(&self, name: &str) -> Option<Id<ChannelMarker>> {
        match self.get(name)? {
            CommandOptionValue::Channel(c) => Some(*c),
            _ => None,
        }
    }
    pub fn role(&self, name: &str) -> Option<Id<RoleMarker>> {
        match self.get(name)? {
            CommandOptionValue::Role(r) => Some(*r),
            _ => None,
        }
    }
    /// For autocomplete: the option being typed and what's typed so far.
    pub fn focused(&self) -> Option<(&'a str, &'a str)> {
        for o in self.0 {
            match &o.value {
                CommandOptionValue::Focused(v, _) => return Some((o.name.as_str(), v.as_str())),
                CommandOptionValue::SubCommand(inner) | CommandOptionValue::SubCommandGroup(inner) => {
                    if let Some(f) = Opts(inner).focused() {
                        return Some(f);
                    }
                }
                _ => {}
            }
        }
        None
    }
}

// ---- replies ----

/// What to send back to an interaction. Mentions are off unless set with `mentions`.
#[derive(Clone, Default)]
pub struct Reply {
    pub content: Option<String>,
    pub embeds: Vec<Embed>,
    pub components: Vec<Component>,
    pub ephemeral: bool,
    pub mentions: Option<AllowedMentions>,
}

impl Reply {
    pub fn text(s: impl Into<String>) -> Self {
        Self { content: Some(s.into()), ..Default::default() }
    }
    pub fn embed(e: Embed) -> Self {
        Self { embeds: vec![e], ..Default::default() }
    }
    pub fn ephemeral(mut self) -> Self {
        self.ephemeral = true;
        self
    }
    pub fn with_embed(mut self, e: Embed) -> Self {
        self.embeds.push(e);
        self
    }
    pub fn components(mut self, rows: Vec<Component>) -> Self {
        self.components = rows;
        self
    }
    pub fn mentions(mut self, m: AllowedMentions) -> Self {
        self.mentions = Some(m);
        self
    }
    fn data(self) -> InteractionResponseData {
        InteractionResponseData {
            content: self.content,
            embeds: Some(self.embeds),
            components: Some(self.components),
            flags: self.ephemeral.then_some(MessageFlags::EPHEMERAL),
            allowed_mentions: Some(self.mentions.unwrap_or_default()),
            ..Default::default()
        }
    }
}

/// A row of buttons. Each tuple is `(custom_id, label, style)`.
pub fn button_row(buttons: &[(&str, &str, ButtonStyle)]) -> Component {
    Component::ActionRow(ActionRow {
        id: None,
        components: buttons
            .iter()
            .map(|(id, label, style)| {
                Component::Button(Button {
                    id: None,
                    custom_id: Some((*id).to_owned()),
                    disabled: false,
                    emoji: None,
                    label: Some((*label).to_owned()),
                    style: *style,
                    url: None,
                    sku_id: None,
                })
            })
            .collect(),
    })
}

/// A link button row.
pub fn link_row(label: &str, url: &str) -> Component {
    Component::ActionRow(ActionRow {
        id: None,
        components: vec![Component::Button(Button {
            id: None,
            custom_id: None,
            disabled: false,
            emoji: None,
            label: Some(label.to_owned()),
            style: ButtonStyle::Link,
            url: Some(url.to_owned()),
            sku_id: None,
        })],
    })
}

impl Ctx {
    async fn respond(&self, i: &Interaction, kind: InteractionResponseType, data: Option<InteractionResponseData>) -> Result<()> {
        let resp = InteractionResponse { kind, data };
        self.interaction().create_response(i.id, &i.token, &resp).await?;
        Ok(())
    }

    /// Answer with a new message.
    pub async fn reply(&self, i: &Interaction, r: Reply) -> Result<()> {
        self.respond(i, InteractionResponseType::ChannelMessageWithSource, Some(r.data())).await
    }

    /// Answer a button press by editing the message it's on.
    pub async fn update(&self, i: &Interaction, r: Reply) -> Result<()> {
        self.respond(i, InteractionResponseType::UpdateMessage, Some(r.data())).await
    }

    /// Show "thinking…" now and answer later with `edit_original` or `followup`.
    pub async fn defer(&self, i: &Interaction, ephemeral: bool) -> Result<()> {
        let data = InteractionResponseData {
            flags: ephemeral.then_some(MessageFlags::EPHEMERAL),
            ..Default::default()
        };
        self.respond(i, InteractionResponseType::DeferredChannelMessageWithSource, Some(data)).await
    }

    /// Acknowledge a button press without changing anything yet.
    pub async fn defer_update(&self, i: &Interaction) -> Result<()> {
        self.respond(i, InteractionResponseType::DeferredUpdateMessage, None).await
    }

    pub async fn edit_original(&self, i: &Interaction, r: Reply) -> Result<()> {
        let client = self.interaction();
        let mentions = r.mentions.unwrap_or_default();
        client
            .update_response(&i.token)
            .content(r.content.as_deref())
            .embeds(Some(&r.embeds))
            .components(Some(&r.components))
            .allowed_mentions(Some(&mentions))
            .await?;
        Ok(())
    }

    pub async fn followup(&self, i: &Interaction, r: Reply) -> Result<()> {
        let client = self.interaction();
        let mentions = r.mentions.unwrap_or_default();
        let mut req = client
            .create_followup(&i.token)
            .embeds(&r.embeds)
            .components(&r.components)
            .allowed_mentions(Some(&mentions));
        if let Some(c) = r.content.as_deref() {
            req = req.content(c);
        }
        if r.ephemeral {
            req = req.flags(MessageFlags::EPHEMERAL);
        }
        req.await?;
        Ok(())
    }

    /// Open a modal. `fields` are `(custom_id, label, long, required, prefilled)`.
    pub async fn modal(&self, i: &Interaction, custom_id: &str, title: &str, fields: &[(&str, &str, bool, bool, Option<&str>)]) -> Result<()> {
        use twilight_model::channel::message::component::{Label, TextInput, TextInputStyle};
        #[allow(deprecated)]
        let components = fields
            .iter()
            .map(|(id, label, long, required, value)| {
                Component::Label(Label {
                    id: None,
                    label: (*label).to_owned(),
                    description: None,
                    component: Box::new(Component::TextInput(TextInput {
                        id: None,
                        custom_id: (*id).to_owned(),
                        label: None,
                        max_length: Some(if *long { 2000 } else { 200 }),
                        min_length: None,
                        placeholder: None,
                        required: Some(*required),
                        style: if *long { TextInputStyle::Paragraph } else { TextInputStyle::Short },
                        value: value.map(str::to_owned),
                    })),
                })
            })
            .collect();
        let data = InteractionResponseData {
            custom_id: Some(custom_id.to_owned()),
            title: Some(title.to_owned()),
            components: Some(components),
            ..Default::default()
        };
        self.respond(i, InteractionResponseType::Modal, Some(data)).await
    }

    /// Answer an autocomplete with up to 25 `(label, value)` choices.
    pub async fn autocomplete(&self, i: &Interaction, choices: Vec<(String, String)>) -> Result<()> {
        let choices = choices
            .into_iter()
            .take(25)
            .map(|(name, value)| CommandOptionChoice {
                name,
                name_localizations: None,
                value: twilight_model::application::command::CommandOptionChoiceValue::String(value),
            })
            .collect();
        let data = InteractionResponseData { choices: Some(choices), ..Default::default() };
        self.respond(i, InteractionResponseType::ApplicationCommandAutocompleteResult, Some(data)).await
    }

    /// Send a plain message to a channel with mentions off. Returns the message id.
    pub async fn say(&self, channel_id: u64, content: &str) -> Result<u64> {
        let none = AllowedMentions::default();
        let msg = self
            .http
            .create_message(Id::new(channel_id))
            .content(content)
            .allowed_mentions(Some(&none))
            .await?
            .model()
            .await?;
        Ok(msg.id.get())
    }

    /// DM a user. Returns false when their DMs are closed.
    pub async fn dm(&self, user_id: u64, content: &str, embeds: &[Embed], components: &[Component]) -> Result<Option<(u64, u64)>> {
        let channel = match self.http.create_private_channel(Id::new(user_id)).await {
            Ok(r) => r.model().await?,
            Err(_) => return Ok(None),
        };
        let none = AllowedMentions::default();
        let mut req = self
            .http
            .create_message(channel.id)
            .embeds(embeds)
            .components(components)
            .allowed_mentions(Some(&none));
        if !content.is_empty() {
            req = req.content(content);
        }
        match req.await {
            Ok(r) => {
                let m = r.model().await?;
                Ok(Some((channel.id.get(), m.id.get())))
            }
            Err(_) => Ok(None),
        }
    }

    /// A member's display name from the cache, or "Unknown member" for someone the cache hasn't
    /// seen (left, or not loaded in a large server). Ids never render on the dashboard. For
    /// display only: to store a name, use [`Ctx::known_name`] and keep the event's own name
    /// (or the one saved before) when it's None.
    pub fn display_name(&self, guild_id: u64, user_id: u64) -> String {
        self.known_name(guild_id, user_id).unwrap_or_else(|| "Unknown member".to_owned())
    }

    /// A member's server nickname, else their global name, else their username, when the cache
    /// has them. None means the cache doesn't know them, not that they have no name.
    pub fn known_name(&self, guild_id: u64, user_id: u64) -> Option<String> {
        let (g, u) = (Id::<GuildMarker>::new(guild_id), Id::<UserMarker>::new(user_id));
        if let Some(nick) = self.cache.member(g, u).and_then(|m| m.nick().map(str::to_owned)) {
            return Some(nick);
        }
        self.cache.user(u).map(|user| user.global_name.clone().unwrap_or_else(|| user.name.clone()))
    }

    /// Text channels of a server as `(id, name)`, in sidebar order.
    pub fn text_channels(&self, guild_id: u64) -> Vec<(u64, String)> {
        use twilight_model::channel::ChannelType;
        let Some(ids) = self.cache.guild_channels(Id::new(guild_id)) else { return Vec::new() };
        let mut out: Vec<_> = ids
            .iter()
            .filter_map(|id| self.cache.channel(*id))
            .filter(|c| matches!(c.kind, ChannelType::GuildText | ChannelType::GuildAnnouncement | ChannelType::GuildForum))
            .map(|c| (c.position.unwrap_or_default(), c.id.get(), c.name.clone().unwrap_or_default()))
            .collect();
        out.sort();
        out.into_iter().map(|(_, id, name)| (id, name)).collect()
    }

    /// Categories of a server as `(id, name)`.
    pub fn categories(&self, guild_id: u64) -> Vec<(u64, String)> {
        use twilight_model::channel::ChannelType;
        let Some(ids) = self.cache.guild_channels(Id::new(guild_id)) else { return Vec::new() };
        let mut out: Vec<_> = ids
            .iter()
            .filter_map(|id| self.cache.channel(*id))
            .filter(|c| c.kind == ChannelType::GuildCategory)
            .map(|c| (c.position.unwrap_or_default(), c.id.get(), c.name.clone().unwrap_or_default()))
            .collect();
        out.sort();
        out.into_iter().map(|(_, id, name)| (id, name)).collect()
    }

    /// Assignable roles of a server as `(id, name, colour)`, highest first, without @everyone
    /// and bot-managed roles.
    pub fn roles(&self, guild_id: u64) -> Vec<(u64, String, u32)> {
        let Some(ids) = self.cache.guild_roles(Id::new(guild_id)) else { return Vec::new() };
        let mut out: Vec<_> = ids
            .iter()
            .filter_map(|id| self.cache.role(*id))
            .filter(|r| r.id.get() != guild_id && !r.managed)
            .map(|r| (r.position, r.id.get(), r.name.clone(), r.colors.primary_color))
            .collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.0));
        out.into_iter().map(|(_, id, name, color)| (id, name, color)).collect()
    }

    /// `channel` if it belongs to this server, otherwise None. Use on every channel id that comes
    /// from a form, so a channel from another server can't be saved.
    pub fn channel_in(&self, guild_id: u64, channel: Option<u64>) -> Option<u64> {
        let id = channel?;
        let c = self.cache.channel(Id::new(id))?;
        (c.guild_id.map(|g| g.get()) == Some(guild_id)).then_some(id)
    }

    /// The bot's highest role position in a server.
    pub fn bot_top_role_position(&self, guild_id: u64) -> i64 {
        let Some(bot) = self.bot_id() else { return 0 };
        let Some(member) = self.cache.member(Id::new(guild_id), Id::new(bot)) else { return 0 };
        member
            .roles()
            .iter()
            .filter_map(|r| self.cache.role(*r))
            .map(|r| r.position)
            .max()
            .unwrap_or(0)
    }

    /// True when the bot can hand this role out and it carries no moderation or admin power.
    /// Features that give roles automatically (level rewards, auto-roles) only accept these
    /// from non-admins, so a dashboard grant can't be turned into server admin.
    pub fn safe_role(&self, guild_id: u64, role: u64) -> bool {
        use twilight_model::guild::Permissions as P;
        let Some(r) = self.cache.role(Id::new(role)) else { return false };
        let dangerous = P::ADMINISTRATOR
            | P::MANAGE_GUILD
            | P::MANAGE_ROLES
            | P::MANAGE_CHANNELS
            | P::MANAGE_WEBHOOKS
            | P::BAN_MEMBERS
            | P::KICK_MEMBERS
            | P::MODERATE_MEMBERS
            | P::MANAGE_MESSAGES
            | P::MENTION_EVERYONE;
        role != guild_id
            && !r.managed
            && self.cache.guild_roles(Id::new(guild_id)).is_some_and(|set| set.contains(&r.id))
            && !r.permissions.intersects(dangerous)
            && (self.bot_id().is_none() || r.position < self.bot_top_role_position(guild_id))
    }

    pub fn guild_name(&self, guild_id: u64) -> String {
        self.cache
            .guild(Id::new(guild_id))
            .map(|g| g.name().to_owned())
            .unwrap_or_else(|| format!("Server {guild_id}"))
    }

    pub fn guild_icon_url(&self, guild_id: u64) -> Option<String> {
        let g = self.cache.guild(Id::new(guild_id))?;
        let icon = g.icon()?;
        Some(format!("https://cdn.discordapp.com/icons/{guild_id}/{icon}.png?size=64"))
    }

    /// The server's banner, wide enough for the overview's header.
    pub fn guild_banner_url(&self, guild_id: u64) -> Option<String> {
        let g = self.cache.guild(Id::new(guild_id))?;
        let banner = g.banner()?;
        Some(format!("https://cdn.discordapp.com/banners/{guild_id}/{banner}.png?size=1024"))
    }

    /// The server icon at a size for the overview's header.
    pub fn guild_icon_large(&self, guild_id: u64) -> Option<String> {
        self.guild_icon_url(guild_id).map(|u| u.replace("size=64", "size=256"))
    }

    /// A member's avatar: their server avatar, else their account avatar, else Discord's default.
    pub fn member_avatar(&self, guild_id: u64, user_id: u64) -> String {
        let (g, u) = (Id::<GuildMarker>::new(guild_id), Id::<UserMarker>::new(user_id));
        if let Some(m) = self.cache.member(g, u)
            && let Some(a) = m.avatar() {
                return format!("https://cdn.discordapp.com/guilds/{guild_id}/users/{user_id}/avatars/{a}.png?size=128");
            }
        avatar_url(user_id, self.cache.user(u).and_then(|x| x.avatar.map(|a| a.to_string())).as_deref())
    }

    /// The server's member count, kept current as members join and leave.
    pub fn member_count(&self, guild_id: u64) -> Option<u64> {
        self.member_counts
            .lock()
            .unwrap()
            .get(&guild_id)
            .copied()
            .or_else(|| self.cache.guild(Id::new(guild_id)).and_then(|g| g.member_count()))
    }
}

/// When a Discord id was made, in epoch milliseconds: an account's creation, a message's send.
pub fn snowflake_ms(id: u64) -> i64 {
    ((id >> 22) + 1_420_070_400_000) as i64
}

/// Avatar URL for a user, or Discord's default avatar.
pub fn avatar_url(user_id: u64, avatar: Option<&str>) -> String {
    match avatar {
        Some(hash) => format!("https://cdn.discordapp.com/avatars/{user_id}/{hash}.png?size=128"),
        None => format!("https://cdn.discordapp.com/embed/avatars/{}.png", (user_id >> 22) % 6),
    }
}

#[cfg(test)]
mod tests {
    use super::refusal;

    #[test]
    fn refusals_say_what_to_fix() {
        assert!(refusal(Some(403), &"x").starts_with("The bot can't post in that channel."));
        assert_eq!(refusal(Some(404), &"x"), "The channel was deleted. Pick another one.");
        assert_eq!(refusal(Some(500), &"Response error: status code 500"), "Discord refused the message: Response error: status code 500");
        assert_eq!(refusal(None, &"timed out"), "Discord refused the message: timed out");
    }

    fn guild(id: u64, members: u64) -> super::GuildCreate {
        let mut v = crate::preview::guild_json(id, "x", members);
        // The preview server lists 10 members; say how many it really has.
        v["member_count"] = members.into();
        super::GuildCreate::Available(serde_json::from_value(v).unwrap())
    }

    #[test]
    fn member_lists_are_asked_for_once_per_session() {
        let mut m = super::MemberRequests::default();
        let ask = m.for_guild(&guild(1, 266)).expect("a large server is asked for its members");
        assert_eq!((ask.d.guild_id.get(), ask.d.query.as_deref(), ask.d.limit, ask.d.presences), (1, Some(""), Some(0), None));
        // A second GuildCreate in the same session (an outage ending) doesn't ask again.
        assert!(m.for_guild(&guild(1, 266)).is_none());
        // A server whose GuildCreate listed everyone has nothing more to ask for.
        assert!(m.for_guild(&guild(2, 10)).is_none());
        // A new session asks again.
        m.session_started();
        assert!(m.for_guild(&guild(1, 266)).is_some());
        // A server that's down for Discord isn't asked.
        let down: super::GuildCreate = serde_json::from_value(serde_json::json!({"id": "3", "unavailable": true})).unwrap();
        assert!(m.for_guild(&down).is_none());
    }

    #[tokio::test]
    async fn names_known_and_unknown() {
        use crate::preview::{GUILD_A, MEMBERS};
        let ctx = crate::Ctx::new(crate::Config::preview(0), crate::Db::memory().unwrap()).unwrap();
        crate::preview::seed(&ctx).unwrap();
        let (mira, _) = MEMBERS[0];
        assert_eq!(ctx.known_name(GUILD_A, mira).as_deref(), Some("Mira"));
        assert_eq!(ctx.display_name(GUILD_A, mira), "Mira");
        // Someone the cache never saw: nothing to store, and no id on the page.
        assert_eq!(ctx.known_name(GUILD_A, 1_234_567_890_123), None);
        assert_eq!(ctx.display_name(GUILD_A, 1_234_567_890_123), "Unknown member");
    }

    #[tokio::test]
    async fn commands_route_to_the_feature_that_declares_them() {
        use twilight_model::application::command::{Command, CommandType};
        use twilight_util::builder::command::CommandBuilder;

        struct Fixed;
        impl crate::Feature for Fixed {
            fn meta(&self) -> crate::FeatureMeta {
                crate::FeatureMeta { key: "fixed", name: "Fixed", description: "", group: crate::Group::Tools, nav: Vec::new() }
            }
            fn commands(&self) -> Vec<Command> {
                vec![CommandBuilder::new("rank", "x", CommandType::ChatInput).build()]
            }
        }
        struct PerServer;
        impl crate::Feature for PerServer {
            fn meta(&self) -> crate::FeatureMeta {
                crate::FeatureMeta { key: "custom", name: "Custom", description: "", group: crate::Group::Tools, nav: Vec::new() }
            }
            fn guild_commands(&self, _ctx: &crate::Ctx, guild: u64) -> Vec<Command> {
                if guild == 1 { vec![CommandBuilder::new("links", "x", CommandType::ChatInput).build()] } else { Vec::new() }
            }
        }
        let ctx = crate::Ctx::new(crate::Config::preview(0), crate::Db::memory().unwrap()).unwrap();
        ctx.install_features(vec![std::sync::Arc::new(Fixed), std::sync::Arc::new(PerServer)]).unwrap();
        let owner = |guild, name| super::command_owner(&ctx, guild, name).map(|f| f.meta().key);
        assert_eq!(owner(Some(2), "rank"), Some("fixed"));
        assert_eq!(owner(None, "rank"), Some("fixed"));
        assert_eq!(owner(Some(1), "links"), Some("custom"));
        // Renamed or deleted in that server, or never there: nobody owns it, so it's "gone",
        // not "switched off".
        assert_eq!(owner(Some(2), "links"), None);
        assert_eq!(owner(None, "links"), None);
        assert_eq!(owner(Some(1), "old-name"), None);
    }

    #[test]
    fn errors_that_arent_discords_keep_their_words() {
        assert_eq!(super::why_refused_any(&anyhow::anyhow!("no link")), "Discord refused the message: no link");
    }
}
