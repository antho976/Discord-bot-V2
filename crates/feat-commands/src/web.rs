use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::rusqlite::params;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};
use serde::Deserialize;

use crate::{CardEmbed, CustomCommand, KEY, Kind, MAX_COMMANDS, MAX_TRIGGERS, Mode, Settings, Trigger, valid_name};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/commands", get(commands_page))
        .route("/g/{guild}/commands/edit", get(command_edit).post(command_save))
        .route("/g/{guild}/commands/{id}/delete", post(command_delete))
        .route("/g/{guild}/commands/options", post(save_options))
        .route("/g/{guild}/commands/responder", get(responder_page))
        .route("/g/{guild}/commands/responder/edit", get(trigger_edit).post(trigger_save))
        .route("/g/{guild}/commands/responder/{id}/delete", post(trigger_delete))
}

fn tabs(page: &Page, s: &Settings, active: &str) -> Markup {
    ui::tabs_counted(
        &[
            ("Commands", page.path("commands"), Some(s.commands.len() as i64).filter(|n| *n > 0)),
            ("Auto-responder", page.path("commands/responder"), Some(s.triggers.len() as i64).filter(|n| *n > 0)),
        ],
        active,
    )
}

fn channel_name(ctx: &Ctx, guild: u64, channel: u64) -> String {
    ctx.text_channels(guild).into_iter().find(|(id, _)| *id == channel).map(|(_, n)| format!("#{n}")).unwrap_or_else(|| "a deleted channel".into())
}

/// "12 uses · last 3 h ago", or "Not used yet".
fn use_line(uses: &crate::Uses, kind: Kind, id: u32) -> String {
    match uses.get(&(kind.key().to_owned(), id)) {
        Some((n, at)) if *n > 0 => format!("{} {} · last {}", text::num(*n), if *n == 1 { "use" } else { "uses" }, text::ago(*at)),
        _ => "Not used yet".into(),
    }
}

/// Re-register the server's slash commands in the background, and say so behind the bell if
/// Discord refuses them.
fn reregister(ctx: &Ctx, guild: u64) {
    if !ctx.enabled(guild, KEY) || ctx.cfg.preview {
        return;
    }
    let ctx = ctx.clone();
    bot_core::tokio::spawn(async move {
        if let Err(e) = bot_core::discord::register_commands(&ctx, guild).await {
            bot_core::tracing::warn!(guild, error = %e, "couldn't register custom commands");
            ctx.notify_once(
                guild,
                KEY,
                "register",
                format!("The custom commands didn't reach Discord. {}", bot_core::discord::why_not_registered(&e)),
                Some(format!("/g/{guild}/commands")),
                std::time::Duration::from_secs(3600),
            );
        }
    });
}

// ---- commands ----

async fn commands_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let uses = crate::uses(&ctx, guild)?;
    let total: i64 = uses.iter().filter(|((k, _), _)| k == Kind::Command.key()).map(|(_, (n, _))| n).sum();
    let actions = (page.can_edit() && s.commands.len() < MAX_COMMANDS).then(|| html! { a.btn.primary href=(page.path("commands/edit")) { "Add a command" } });
    let taken = crate::names_elsewhere(&ctx, guild);
    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, &s, "Commands"))
        div.with-rail {
            div.stack.cc-main {
                (ui::readings(&[
                    ("commands", format!("{} of {MAX_COMMANDS}", s.commands.len())),
                    ("uses, all time", text::num(total)),
                ]))
                @if s.commands.is_empty() {
                    (ui::empty(Icon::Slash, "No custom commands yet", "Add one to answer a slash command with your own text, like /links or /rules."))
                } @else {
                    div.cc-list {
                        @for c in &s.commands {
                            div.cc-row {
                                span.cc-icon { (ui::icon(Icon::Slash)) }
                                div.cc-body {
                                    div.cc-title {
                                        strong.mono { "/" (c.name) }
                                        @if c.ephemeral { (ui::badge("Only they see it", Tone::Muted)) }
                                        @if c.embed.is_some() { (ui::badge("Embed", Tone::Muted)) }
                                        @if taken.contains(&c.name) { (ui::badge("Not registered", Tone::Warn)) }
                                    }
                                    @if taken.contains(&c.name) {
                                        span.small.cc-warn { "/" (c.name) " is now one of the bot's own commands, so this one isn't registered. Rename it." }
                                    }
                                    span.muted.small.clip { (crate::description(c)) }
                                    @let shown = if c.reply.trim().is_empty() { c.embed.as_ref().map(|e| if e.title.is_empty() { e.description.clone() } else { e.title.clone() }).unwrap_or_default() } else { c.reply.clone() };
                                    p.cc-reply { (text::clip(shown.trim(), 140)) }
                                    span.muted.small { (use_line(&uses, Kind::Command, c.id)) }
                                }
                                @if page.can_edit() {
                                    div.cc-acts {
                                        a.btn.small href={ (page.path("commands/edit")) "?id=" (c.id) } { "Edit" }
                                        (ui::action_button(&page.path(&format!("commands/{}/delete", c.id)), "Delete", Tone::Bad, true))
                                    }
                                }
                            }
                        }
                    }
                }
            }
            aside.rail {
                (placeholder_block())
                div.rail-block {
                    h2 { "Text commands" }
                    form.stack method="post" action=(page.path("commands/options")) {
                        fieldset.cc-fs disabled[!page.can_edit()] {
                            (ui::switch_row("text_commands", "Answer !name in chat", Some("Members can type !links as well as /links, like on the old bot. Each answers once every 5 seconds in a channel."), s.text_commands))
                        }
                        @if page.can_edit() { div { button.btn.small type="submit" { "Save" } } }
                    }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Custom commands", actions, body).into_response())
}

fn placeholder_block() -> Markup {
    html! {
        div.rail-block {
            h2 { "Placeholders" }
            div.cc-ph {
                code { "{user}" } span { "their name" }
                code { "{mention}" } span { "pings them" }
                code { "{server}" } span { "the server's name" }
                code { "{channel}" } span { "the channel, as a link" }
                code { "{members}" } span { "the member count" }
            }
        }
    }
}

#[derive(Deserialize)]
struct EditQuery {
    id: Option<u32>,
}

/// What the placeholders turn into in a preview: the person looking stands in for the member.
struct Sample {
    user: String,
    avatar: String,
    server: String,
    channel: String,
    members: String,
}

impl Sample {
    fn new(ctx: &Ctx, page: &Page) -> Self {
        let guild = page.guild_id;
        Self {
            user: page.user.name.clone(),
            avatar: page.user.avatar_url(),
            server: ctx.guild_name(guild),
            channel: ctx.text_channels(guild).into_iter().next().map(|(_, n)| n).unwrap_or_else(|| "general".into()),
            members: ctx.member_count(guild).map(|n| text::num(n as i64)).unwrap_or_default(),
        }
    }

    fn fill(&self, t: &str) -> String {
        text::fill(t, &[("user", &self.user), ("mention", &format!("@{}", self.user)), ("server", &self.server), ("channel", &format!("#{}", self.channel)), ("members", &self.members)])
    }
}

/// The bot's reply as Discord draws it, drawn in the dashboard's own colours. `above` goes
/// over the message (who used the command, or what they said). app JS fills it as you type.
fn preview(ctx: &Ctx, page: &Page, sample: &Sample, above: Markup, reply: &str, embed: Option<&CardEmbed>, ephemeral: bool) -> Markup {
    let bot = ctx.cache.current_user();
    let bot_name = bot.as_ref().map(|u| u.name.clone()).unwrap_or_else(|| "Bot".into());
    let bot_avatar = bot.as_ref().map(|u| bot_core::discord::avatar_url(u.id.get(), u.avatar.map(|a| a.to_string()).as_deref()));
    let time = page.user.local(now_ms()).strftime("%-I:%M %p").to_string();
    let e = embed.cloned().unwrap_or_default();
    let img_ok = e.image.trim().starts_with("https://");
    html! {
        div.cc-preview #cc-preview data-user=(sample.user) data-server=(sample.server) data-channel=(sample.channel) data-members=(sample.members) {
            (above)
            div.cc-msg {
                @if let Some(url) = &bot_avatar {
                    img.cc-av src=(url) alt="" width="36" height="36" loading="lazy";
                } @else {
                    span.cc-av.letter { (bot_name.chars().next().unwrap_or('B')) }
                }
                div.cc-msg-main {
                    div.cc-head { b { (bot_name) } span.cc-tag { "APP" } span.cc-time { "Today at " (time) } }
                    div.cc-text #cc-p-text { (sample.fill(reply)) }
                    div.cc-embed #cc-p-embed hidden[embed.is_none()] {
                        strong #cc-p-etitle { (sample.fill(&e.title)) }
                        p #cc-p-edesc { (sample.fill(&e.description)) }
                        img #cc-p-eimg src=[img_ok.then_some(e.image.trim())] alt="" hidden[!img_ok];
                    }
                    p.cc-only #cc-p-eph hidden[!ephemeral] { "Only you can see this" }
                }
            }
        }
    }
}

async fn command_edit(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<EditQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let existing = q.id.and_then(|id| s.commands.iter().find(|c| c.id == id).cloned());
    if q.id.is_some() && existing.is_none() {
        return Err(WebError::NotFound);
    }
    let c = existing.clone().unwrap_or_default();
    let e = c.embed.clone().unwrap_or_default();
    let sample = Sample::new(&ctx, &page);
    let title = if existing.is_some() { "Edit command" } else { "Add a command" };
    let used = html! {
        div.cc-used {
            img.avatar src=(sample.avatar) alt="" width="16" height="16";
            span { b { (sample.user) } " used " span.cc-cmd #cc-p-name { "/" (if c.name.is_empty() { "name" } else { &c.name }) } }
        }
    };
    let body = html! {
        style { (PreEscaped(CSS)) }
        a.cc-back.small.muted href=(page.path("commands")) { "All commands" }
        div.with-rail {
            (ui::settings_form(&page, &page.path("commands/edit"), html! {
                @if let Some(c) = &existing { input type="hidden" name="id" value=(c.id); }
                (ui::section("Command", None, ui::card(html! {
                    (ui::field_text("name", "Name", Some("Lowercase letters, numbers, - and _, up to 32. Members type /name, or !name in chat when text commands are on."), &c.name, "links"))
                    (ui::field_text("description", "Description", Some("Shown under the name in Discord's command list."), &c.description, "Where to find us"))
                })))
                (ui::section("Reply", Some("{user}, {mention}, {server}, {channel} and {members} are filled in. Only {mention} pings."), ui::card(html! {
                    (ui::field_textarea("reply", "Message", None, &c.reply, "Twitch: twitch.tv/antho, schedule in #announcements"))
                    (ui::field_toggle("ephemeral", "Only they see it", Some("For the slash command. A !name reply is always public."), c.ephemeral))
                })))
                (ui::section("Embed", None, html! { div.cc-ecard { (ui::card(html! {
                    (ui::field_toggle("embed", "Add an embed", Some("A box under the message with a coral edge."), c.embed.is_some()))
                    (ui::field_text("embed_title", "Title", None, &e.title, "Links"))
                    (ui::field_textarea("embed_desc", "Text", None, &e.description, ""))
                    (ui::field_text("embed_image", "Picture", Some("An https link to an image."), &e.image, "https://"))
                })) } }))
            }))
            aside.rail.cc-side {
                div.rail-block {
                    h2 { "Preview" }
                    (preview(&ctx, &page, &sample, used, &c.reply, c.embed.as_ref(), c.ephemeral))
                }
            }
        }
        script { (PreEscaped(JS)) }
    };
    Ok(ui::layout(&ctx, &page, title, None, body).into_response())
}

async fn command_save(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let mut s = (*ctx.settings::<Settings>(guild, KEY)?).clone();
    let id = form.i64("id").map(|i| i as u32);
    let pos = id.and_then(|id| s.commands.iter().position(|c| c.id == id));
    if id.is_some() && pos.is_none() {
        return Err(WebError::Bad("That command was deleted. Add it again.".into()));
    }
    let name = form.string("name").trim().trim_start_matches(['/', '!']).to_lowercase();
    if !valid_name(&name) {
        return Err(WebError::Bad("Use 1 to 32 lowercase letters, numbers, - or _ for the name, with no spaces.".into()));
    }
    if s.commands.iter().any(|c| c.name == name && Some(c.id) != id) {
        return Err(WebError::Bad(format!("You already have /{name}. Edit that one instead.")));
    }
    if crate::names_elsewhere(&ctx, guild).contains(&name) {
        return Err(WebError::Bad(format!("/{name} is one of the bot's own commands. Pick another name.")));
    }
    if pos.is_none() && s.commands.len() >= MAX_COMMANDS {
        return Err(WebError::Bad(format!("You can have {MAX_COMMANDS} custom commands. Delete one first.")));
    }
    let embed = form.bool("embed").then(|| CardEmbed {
        title: text::clip(form.string("embed_title").trim(), 256),
        description: text::clip(form.text("embed_desc").trim(), 4000),
        image: form.string("embed_image").trim().to_owned(),
    });
    if let Some(e) = &embed {
        if !e.image.is_empty() && !e.image.starts_with("https://") {
            return Err(WebError::Bad("The picture needs an https:// link.".into()));
        }
        if e.title.is_empty() && e.description.is_empty() && e.image.is_empty() {
            return Err(WebError::Bad("The embed is empty. Give it a title, text or a picture, or switch it off.".into()));
        }
    }
    let reply = text::clip(form.text("reply").trim(), 2000);
    if reply.is_empty() && embed.is_none() {
        return Err(WebError::Bad("Write a reply, or add an embed.".into()));
    }
    let before = s.registered().iter().map(|(n, d)| (n.to_string(), d.to_string())).collect::<Vec<_>>();
    let id = match pos {
        Some(i) => s.commands[i].id,
        None => s.take_id(),
    };
    let c = CustomCommand { id, name: name.clone(), description: text::clip(form.string("description").trim(), 100), reply, ephemeral: form.bool("ephemeral"), embed };
    match pos {
        Some(i) => s.commands[i] = c,
        None => s.commands.push(c),
    }
    let changed = s.registered().iter().map(|(n, d)| (n.to_string(), d.to_string())).collect::<Vec<_>>() != before;
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, if pos.is_some() { format!("Changed /{name}") } else { format!("Added /{name}") });
    if changed {
        reregister(&ctx, guild);
    }
    Ok(saved(&page.path("commands")))
}

async fn command_delete(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, u32)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let mut s = (*ctx.settings::<Settings>(guild, KEY)?).clone();
    let Some(i) = s.commands.iter().position(|c| c.id == id) else { return Ok(saved(&page.path("commands"))) };
    let gone = s.commands.remove(i);
    ctx.save_settings(guild, KEY, &s)?;
    crate::forget_uses(&ctx, guild, Kind::Command, id)?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Deleted /{}", gone.name));
    reregister(&ctx, guild);
    Ok(saved(&page.path("commands")))
}

async fn save_options(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let mut s = (*ctx.settings::<Settings>(guild, KEY)?).clone();
    s.text_commands = form.bool("text_commands");
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, if s.text_commands { "Turned !name commands on" } else { "Turned !name commands off" });
    Ok(saved(&page.path("commands")))
}

// ---- auto-responder ----

#[derive(Deserialize)]
struct TestQuery {
    test: Option<String>,
    channel: Option<String>,
}

/// "#general, #pets and 2 more" for a trigger's channels.
fn channel_list(ctx: &Ctx, guild: u64, t: &Trigger) -> String {
    let names: Vec<String> = t.channels.iter().take(3).map(|c| channel_name(ctx, guild, *c)).collect();
    let more = t.channels.len().saturating_sub(3);
    if more > 0 { format!("{} and {more} more", names.join(", ")) } else { names.join(", ") }
}

/// "5 min cooldown", or "No cooldown".
fn cooldown_line(secs: i64) -> String {
    if secs <= 0 { "No cooldown".into() } else { format!("{} cooldown", text::duration(secs)) }
}

fn where_line(ctx: &Ctx, guild: u64, t: &Trigger) -> String {
    if t.channels.is_empty() { "Everywhere".into() } else { format!("In {}", channel_list(ctx, guild, t)) }
}

async fn responder_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<TestQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let uses = crate::uses(&ctx, guild)?;
    let total: i64 = uses.iter().filter(|((k, _), _)| k == Kind::Trigger.key()).map(|(_, (n, _))| n).sum();
    let actions = (page.can_edit() && s.triggers.len() < MAX_TRIGGERS).then(|| html! { a.btn.primary href=(page.path("commands/responder/edit")) { "Add a trigger" } });
    let test = q.test.as_deref().map(|t| text::clip(t, 2000)).filter(|t| !t.trim().is_empty());
    let channel = q.channel.as_deref().and_then(|c| c.parse::<u64>().ok()).and_then(|c| ctx.channel_in(guild, Some(c)));
    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, &s, "Auto-responder"))
        div.with-rail {
            div.stack.cc-main {
                (ui::readings(&[
                    ("triggers", format!("{} of {MAX_TRIGGERS}", s.triggers.len())),
                    ("replies, all time", text::num(total)),
                ]))
                @if s.triggers.is_empty() {
                    (ui::empty(Icon::Slash, "No triggers yet", "Add a phrase, like \"when is stream\", and the reply the bot posts when a message has it."))
                } @else {
                    p.muted.small { "Checked top to bottom; the first match answers." }
                    div.cc-list {
                        @for t in &s.triggers {
                            div.cc-row {
                                span.cc-icon { (ui::icon(Icon::Slash)) }
                                div.cc-body {
                                    div.cc-title { span.muted { (t.mode.label()) } strong { "“" (t.pattern) "”" } }
                                    p.cc-reply { (text::clip(t.reply.trim(), 140)) }
                                    span.muted.small { (where_line(&ctx, guild, t)) " · " (cooldown_line(t.cooldown_secs)) " · " (use_line(&uses, Kind::Trigger, t.id)) }
                                }
                                @if page.can_edit() {
                                    div.cc-acts {
                                        a.btn.small href={ (page.path("commands/responder/edit")) "?id=" (t.id) } { "Edit" }
                                        (ui::action_button(&page.path(&format!("commands/responder/{}/delete", t.id)), "Delete", Tone::Bad, true))
                                    }
                                }
                            }
                        }
                    }
                }
            }
            aside.rail {
                (test_block(&ctx, &page, &s, test.as_deref(), channel))
                (placeholder_block())
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Custom commands", actions, body).into_response())
}

/// Try a message against the triggers with the same matching the bot uses. Nothing is posted.
fn test_block(ctx: &Ctx, page: &Page, s: &Settings, test: Option<&str>, channel: Option<u64>) -> Markup {
    let guild = page.guild_id;
    let mut channels = vec![(String::new(), "Any channel".to_owned())];
    channels.extend(ctx.text_channels(guild).into_iter().map(|(id, n)| (id.to_string(), format!("#{n}"))));
    let lower = test.map(str::to_lowercase);
    let place = channel.map(|c| (c, None));
    let hit = lower.as_deref().and_then(|l| s.first_trigger(l, place));
    let sample = Sample::new(ctx, page);
    html! {
        div.rail-block #test {
            h2 { "Test a message" }
            p.muted.small { "See which trigger would answer. Nothing is posted." }
            form.stack method="get" action={ (page.path("commands/responder")) "#test" } {
                textarea.input name="test" rows="3" placeholder="hey when is stream today" { (test.unwrap_or_default()) }
                (ui::select("channel", &channels, &channel.map(|c| c.to_string()).unwrap_or_default()))
                div { button.btn.small type="submit" { "Test" } }
            }
            @if test.is_some() {
                @match hit {
                    Some(t) => {
                        div.cc-hit {
                            (ui::badge("Answers", Tone::Ok))
                            span { (t.mode.label()) " “" (t.pattern) "”" }
                        }
                        @if channel.is_none() && !t.channels.is_empty() { p.muted.small { "It answers only in " (channel_list(ctx, guild, t)) ". Pick a channel to check." } }
                        p.cc-said { (sample.fill(&t.reply)) }
                    }
                    None => {
                        div.cc-hit { (ui::badge("No match", Tone::Muted)) span.muted { "No trigger answers this" @if channel.is_some() { " here" } "." } }
                    }
                }
            }
        }
    }
}

async fn trigger_edit(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<EditQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let existing = q.id.and_then(|id| s.triggers.iter().find(|t| t.id == id).cloned());
    if q.id.is_some() && existing.is_none() {
        return Err(WebError::NotFound);
    }
    let t = existing.clone().unwrap_or_default();
    let sample = Sample::new(&ctx, &page);
    let modes: Vec<(String, String)> = [Mode::Contains, Mode::StartsWith, Mode::Exact].iter().map(|m| (m.key().to_owned(), m.label().to_owned())).collect();
    let said = match t.mode {
        Mode::Exact => t.pattern.clone(),
        Mode::StartsWith => format!("{} …", t.pattern),
        Mode::Contains => format!("… {} …", t.pattern),
    };
    let above = html! {
        div.cc-msg.cc-member {
            img.cc-av src=(sample.avatar) alt="" width="36" height="36";
            div.cc-msg-main {
                div.cc-head { b { (sample.user) } }
                div.cc-text #cc-p-said { (said) }
            }
        }
    };
    let title = if existing.is_some() { "Edit trigger" } else { "Add a trigger" };
    let body = html! {
        style { (PreEscaped(CSS)) }
        a.cc-back.small.muted href=(page.path("commands/responder")) { "All triggers" }
        div.with-rail {
            (ui::settings_form(&page, &page.path("commands/responder/edit"), html! {
                @if let Some(t) = &existing { input type="hidden" name="id" value=(t.id); }
                (ui::section("When a message", None, ui::card(html! {
                    (ui::field_select("mode", "Match", None, &modes, t.mode.key()))
                    (ui::field_text("pattern", "Phrase", Some("Capital letters don't matter. Contains and Starts with look for whole words: “gm” answers “gm all” but not “gmail”."), &t.pattern, "when is stream"))
                })))
                (ui::section("Reply", Some("{user}, {mention}, {server}, {channel} and {members} are filled in. Only {mention} pings."), ui::card(html! {
                    (ui::field_textarea("reply", "Message", None, &t.reply, "Streams are Tuesday and Friday at 8 pm. Schedule in #announcements."))
                    (ui::field_number("cooldown", "Cooldown", Some("After it answers, it stays quiet this long across the server."), t.cooldown_secs, 0, 86_400, Some("seconds")))
                    (ui::field_channels(&ctx, guild, "channels", "Only in", Some("None picked answers everywhere. Threads in these channels count too."), &t.channels))
                })))
            }))
            aside.rail.cc-side {
                div.rail-block {
                    h2 { "Preview" }
                    (preview(&ctx, &page, &sample, above, &t.reply, None, false))
                }
            }
        }
        script { (PreEscaped(JS)) }
    };
    Ok(ui::layout(&ctx, &page, title, None, body).into_response())
}

async fn trigger_save(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let mut s = (*ctx.settings::<Settings>(guild, KEY)?).clone();
    let id = form.i64("id").map(|i| i as u32);
    let pos = id.and_then(|id| s.triggers.iter().position(|t| t.id == id));
    if id.is_some() && pos.is_none() {
        return Err(WebError::Bad("That trigger was deleted. Add it again.".into()));
    }
    if pos.is_none() && s.triggers.len() >= MAX_TRIGGERS {
        return Err(WebError::Bad(format!("You can have {MAX_TRIGGERS} triggers. Delete one first.")));
    }
    let pattern = text::clip(&form.string("pattern").trim().to_lowercase(), 200);
    if pattern.is_empty() {
        return Err(WebError::Bad("Type the phrase to look for.".into()));
    }
    let reply = text::clip(form.text("reply").trim(), 2000);
    if reply.is_empty() {
        return Err(WebError::Bad("Write the reply.".into()));
    }
    let mode = Mode::from_key(&form.string("mode"));
    let id = match pos {
        Some(i) => s.triggers[i].id,
        None => s.take_id(),
    };
    let t = Trigger { id, mode, pattern: pattern.clone(), reply, channels: form.channels(&ctx, guild, "channels"), cooldown_secs: form.int_in("cooldown", 0, 86_400, 30) };
    match pos {
        Some(i) => s.triggers[i] = t,
        None => s.triggers.push(t),
    }
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, if pos.is_some() { format!("Changed the reply to “{pattern}”") } else { format!("Added a reply to “{pattern}”") });
    Ok(saved(&page.path("commands/responder")))
}

async fn trigger_delete(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, u32)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let mut s = (*ctx.settings::<Settings>(guild, KEY)?).clone();
    let Some(i) = s.triggers.iter().position(|t| t.id == id) else { return Ok(saved(&page.path("commands/responder"))) };
    let gone = s.triggers.remove(i);
    ctx.save_settings(guild, KEY, &s)?;
    crate::forget_uses(&ctx, guild, Kind::Trigger, id)?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Deleted the reply to “{}”", gone.pattern));
    Ok(saved(&page.path("commands/responder")))
}

// ---- preview ----

pub fn seed(ctx: &Ctx, guild: u64) -> bot_core::anyhow::Result<()> {
    let ch = |n: u64| guild + n * 10;
    let mut s = Settings::default();
    let commands = [
        ("links", "Where to find Antho", "Twitch: twitch.tv/antho\nYouTube: youtube.com/@antho\nSchedule in {channel}.", false, None),
        ("rules", "The server rules, short", "", false, Some(CardEmbed { title: "Rules of {server}".into(), description: "1. Be kind.\n2. No trading outside #pets.\n3. Links to scams get you banned.".into(), image: String::new() })),
        ("pets", "How to ask for a pet", "Post in #pets with the pet's name. A mod approves it within a day.", true, None),
        ("members", "How big the server is", "{server} has {members} members. Welcome, {mention}.", false, None),
    ];
    for (name, about, reply, ephemeral, embed) in commands {
        let id = s.take_id();
        s.commands.push(CustomCommand { id, name: name.into(), description: about.into(), reply: reply.into(), ephemeral, embed });
    }
    let triggers = [
        (Mode::Contains, "when is stream", "Streams are Tuesday and Friday at 8 pm Eastern. The schedule is in #announcements.", vec![], 60),
        (Mode::StartsWith, "how do i get pets", "Post the pet's name here, {user}. A mod approves it within a day.", vec![ch(6)], 120),
        (Mode::Exact, "gm", "gm {user}", vec![ch(4)], 300),
    ];
    for (mode, pattern, reply, channels, cooldown_secs) in triggers {
        let id = s.take_id();
        s.triggers.push(Trigger { id, mode, pattern: pattern.into(), reply: reply.into(), channels, cooldown_secs });
    }
    ctx.save_settings(guild, KEY, &s)?;
    let now = now_ms();
    let h = 3_600_000;
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    for (kind, id, n, ago) in [("command", 1, 214, h / 2), ("command", 2, 61, 5 * h), ("command", 3, 38, 26 * h), ("trigger", 5, 97, 3 * h), ("trigger", 6, 22, 30 * h), ("trigger", 7, 340, h / 6)] {
        tx.execute(
            "INSERT OR REPLACE INTO commands_uses (guild_id, kind, item_id, uses, last_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![guild as i64, kind, id, n, now - ago],
        )?;
    }
    tx.commit()?;
    Ok(())
}

const CSS: &str = "
.cc-main { gap: 16px; }
.cc-list { display: flex; flex-direction: column; }
.cc-row { display: grid; grid-template-columns: 32px minmax(0, 1fr) auto; gap: 14px; align-items: start; padding: 14px 0; border-top: 1px solid var(--edge-1); }
.cc-row:last-child { border-bottom: 1px solid var(--edge-1); }
.cc-icon { width: 32px; height: 32px; border-radius: var(--r-key); display: grid; place-items: center; background: var(--control); color: var(--ink-2); }
.cc-body { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.cc-title { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; font-size: 14px; }
.cc-title strong { font-weight: 500; }
.cc-title strong.mono { font-size: 13px; }
.cc-reply { font-size: 13px; color: var(--ink-2); white-space: pre-line; overflow-wrap: anywhere; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
.cc-warn { color: var(--waiting); }
.cc-ecard:has(#embed:not(:checked)) .row + .row { display: none; }
.cc-acts { display: flex; gap: 6px; align-items: center; }
.cc-fs { border: 0; padding: 0; margin: 0; min-width: 0; }
.cc-ph { display: grid; grid-template-columns: auto 1fr; gap: 6px 12px; font-size: 12.5px; color: var(--ink-2); align-items: baseline; }
.cc-ph code { color: var(--ink); }
.cc-back { display: inline-block; margin-bottom: 4px; }
.cc-back::before { content: '‹ '; }
.cc-hit { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; font-size: 13px; }
.cc-said { font-size: 13px; color: var(--ink-2); white-space: pre-wrap; overflow-wrap: anywhere; border-left: 3px solid var(--edge-3); padding-left: 10px; }
.cc-side { gap: 0; }
.cc-preview { display: flex; flex-direction: column; gap: 10px; }
.cc-preview [hidden] { display: none !important; }
.cc-used { display: flex; align-items: center; gap: 6px; font-size: 12px; color: var(--ink-3); padding-left: 46px; min-width: 0; }
.cc-used b { color: var(--ink-2); font-weight: 500; }
.cc-cmd { color: var(--data); font-family: var(--mono); }
.cc-msg { display: flex; gap: 10px; min-width: 0; }
.cc-av { flex: none; width: 36px; height: 36px; border-radius: 50%; object-fit: cover; background: var(--control); }
.cc-av.letter { display: grid; place-items: center; background: var(--signal); color: var(--signal-ink); font-weight: 600; }
.cc-msg-main { min-width: 0; flex: 1; display: flex; flex-direction: column; gap: 3px; }
.cc-head { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; font-size: 13px; }
.cc-head b { font-weight: 500; }
.cc-tag { background: var(--control); color: var(--ink-2); font-size: 10px; font-weight: 600; border-radius: 4px; padding: 0 4px; line-height: 15px; }
.cc-time { color: var(--ink-3); font-size: 11.5px; }
.cc-text { font-size: 13.5px; white-space: pre-wrap; overflow-wrap: anywhere; }
.cc-text:empty { display: none; }
.cc-embed { margin-top: 4px; display: flex; flex-direction: column; gap: 4px; border-left: 3px solid var(--signal); background: var(--veil); border-radius: 6px; padding: 10px 12px; font-size: 13px; }
.cc-embed strong { font-weight: 600; }
.cc-embed strong:empty, .cc-embed p:empty { display: none; }
.cc-embed p { white-space: pre-wrap; color: var(--ink-2); overflow-wrap: anywhere; }
.cc-embed img { max-width: 100%; max-height: 180px; border-radius: 4px; object-fit: cover; margin-top: 4px; }
.cc-only { font-size: 11.5px; color: var(--ink-3); }
.cc-member .cc-text { color: var(--ink-2); }
@media (max-width: 560px) {
  .cc-row { grid-template-columns: 32px minmax(0, 1fr); }
  .cc-acts { grid-column: 2; }
}
";

/// Keeps the preview in step with the form as you type.
const JS: &str = r#"(() => {
  const p = document.getElementById('cc-preview');
  if (!p) return;
  const d = p.dataset;
  const $ = id => document.getElementById(id);
  const val = id => ($(id) || {}).value || '';
  const on = id => !!($(id) || {}).checked;
  const fill = s => s.replaceAll('{user}', d.user).replaceAll('{mention}', '@' + d.user).replaceAll('{server}', d.server).replaceAll('{channel}', '#' + d.channel).replaceAll('{members}', d.members);
  const set = (id, v) => { const el = $(id); if (el) el.textContent = v; };
  const show = (id, yes) => { const el = $(id); if (el) el.hidden = !yes; };
  const update = () => {
    set('cc-p-name', '/' + (val('name').trim().replace(/^[\/!]+/, '').toLowerCase() || 'name'));
    set('cc-p-text', fill(val('reply')));
    if ($('embed')) {
      show('cc-p-embed', on('embed'));
      set('cc-p-etitle', fill(val('embed_title')));
      set('cc-p-edesc', fill(val('embed_desc')));
      const img = $('cc-p-eimg'), url = val('embed_image').trim();
      if (img) { img.hidden = !url.startsWith('https://'); if (!img.hidden) img.src = url; }
    }
    if ($('ephemeral')) show('cc-p-eph', on('ephemeral'));
    if ($('pattern')) {
      const pat = val('pattern').trim(), mode = val('mode');
      set('cc-p-said', mode === 'exact' ? pat : mode === 'starts_with' ? pat + ' …' : '… ' + pat + ' …');
    }
  };
  document.addEventListener('input', update);
  document.addEventListener('change', update);
})();"#;
