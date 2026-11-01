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

/// How often it ran: "214 uses" and when last, over a bar measured against the busiest one
/// (`top`). `noun` is the singular and plural ("use", "uses").
fn usage(uses: &crate::Uses, kind: Kind, id: u32, top: i64, noun: (&str, &str)) -> Markup {
    let (n, at) = uses.get(&(kind.key().to_owned(), id)).copied().unwrap_or((0, 0));
    html! {
        div.cc-use {
            div.cc-use-line {
                @if n > 0 {
                    span { b { (text::num(n)) } " " (if n == 1 { noun.0 } else { noun.1 }) }
                    span.cc-last { "last " (text::ago(at)) }
                } @else {
                    span.cc-last { "Not used yet" }
                }
            }
            span.cc-meter aria-hidden="true" { span style={ "width:" (format!("{:.1}", n as f64 / top.max(1) as f64 * 100.0)) "%" } {} }
        }
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
    let actions = (page.can_edit() && s.commands.len() < MAX_COMMANDS).then(|| html! { a.btn.primary href=(page.path("commands/edit")) { (ui::icon(Icon::Plus)) "Add a command" } });
    let taken = crate::names_elsewhere(&ctx, guild);
    let sample = Sample::new(&ctx, &page);
    let count = |id: u32| uses.get(&(Kind::Command.key().to_owned(), id)).map_or(0, |u| u.0);
    let top = s.commands.iter().map(|c| count(c.id)).max().unwrap_or(0).max(1);
    let favourite = s.commands.iter().filter(|c| count(c.id) > 0).max_by_key(|c| count(c.id));
    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, &s, "Commands"))
        div.with-rail {
            div.cc-main {
                div.readings {
                    div.reading { b data-count { (s.commands.len()) " of " (MAX_COMMANDS) } span { "commands" } }
                    div.reading { b data-count { (text::num(total)) } span { "uses, all time" } }
                    @if let Some(c) = favourite { div.reading { b.cc-fav { span.cc-sigil { "/" } (c.name) } span { "used most" } } }
                }
                @if s.commands.is_empty() {
                    (ui::empty(Icon::Slash, "No custom commands yet", "Add one to answer a slash command with your own text, like /links or /rules."))
                } @else {
                    div.cc-list {
                        @for c in &s.commands {
                            article.cc-card {
                                header.cc-card-head {
                                    h3.cc-name { span.cc-sigil { "/" } (c.name) }
                                    @if taken.contains(&c.name) { (ui::badge("Not registered", Tone::Warn)) }
                                    @if page.can_edit() {
                                        div.cc-acts {
                                            a.btn.small href={ (page.path("commands/edit")) "?id=" (c.id) } { "Edit" }
                                            (ui::action_button(&page.path(&format!("commands/{}/delete", c.id)), "Delete", Tone::Bad, true))
                                        }
                                    }
                                }
                                div.cc-card-body {
                                    div.cc-meta {
                                        p.cc-about { (crate::description(c)) }
                                        @if taken.contains(&c.name) {
                                            p.cc-warn { "/" (c.name) " is now one of the bot's own commands, so this one isn't registered. Rename it." }
                                        } @else if s.text_commands {
                                            p.cc-alias { "Also " b { "!" (c.name) } " in chat" }
                                        }
                                        (usage(&uses, Kind::Command, c.id, top, ("use", "uses")))
                                    }
                                    (reply_bubble(&ctx, &sample, None, &c.reply, c.embed.as_ref(), c.ephemeral))
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
            p.muted.small { "Filled in when the bot replies. Only {mention} pings." }
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

/// The bot's name and face. In a preview without Discord, the dashboard's own mark.
fn bot_av(ctx: &Ctx, class: &str) -> (String, Markup) {
    let bot = ctx.cache.current_user();
    let name = bot.as_ref().map(|u| u.name.clone()).unwrap_or_else(|| "Bot".into());
    let face = match bot.as_ref() {
        Some(u) => html! { img class=(class) src=(bot_core::discord::avatar_url(u.id.get(), u.avatar.map(|a| a.to_string()).as_deref())) alt="" width="36" height="36" loading="lazy"; },
        None => html! { span class={ (class) " mark" } { img src="/static/icon.svg" alt="" width="20" height="20"; } },
    };
    (name, face)
}

/// A small copy of the exchange for a list row, in Discord's colours: what the member said (for
/// a trigger), then the bot's reply with placeholders filled, and the embed under it.
fn reply_bubble(ctx: &Ctx, sample: &Sample, said: Option<Markup>, reply: &str, embed: Option<&CardEmbed>, ephemeral: bool) -> Markup {
    let (name, face) = bot_av(ctx, "cc-bav");
    html! {
        div.cc-bubble {
            @if let Some(said) = said {
                div.cc-dmsg {
                    img.cc-bav src=(sample.avatar) alt="" width="26" height="26";
                    div.cc-bubble-main {
                        div.cc-bhead { b { (sample.user) } }
                        p.cc-btext { (said) }
                    }
                }
            }
            div.cc-dmsg {
                (face)
                div.cc-bubble-main {
                    div.cc-bhead { b { (name) } span.cc-tag { "APP" } }
                    @if !reply.trim().is_empty() { p.cc-btext { (sample.fill(reply.trim())) } }
                    @if let Some(e) = embed {
                        div.cc-bembed {
                            @if !e.title.is_empty() { strong { (sample.fill(&e.title)) } }
                            @if !e.description.is_empty() { p { (sample.fill(&e.description)) } }
                            @if e.image.trim().starts_with("https://") { img src=(e.image.trim()) alt="" loading="lazy"; }
                        }
                    }
                    @if ephemeral { span.cc-only { (eye_icon()) "Only they can see this" } }
                }
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

/// The bot's reply as Discord draws it, in Discord's own colours, inside a channel. `above` goes
/// over the message (who used the command, or what they said). The page script fills it as you
/// type.
#[allow(clippy::too_many_arguments)]
fn preview(ctx: &Ctx, page: &Page, sample: &Sample, channel: &str, above: Markup, reply: &str, embed: Option<&CardEmbed>, ephemeral: bool) -> Markup {
    let (bot_name, face) = bot_av(ctx, "cc-av");
    let time = page.user.local(now_ms()).strftime("%-I:%M %p").to_string();
    let e = embed.cloned().unwrap_or_default();
    let img_ok = e.image.trim().starts_with("https://");
    html! {
        div.cc-win {
            div.cc-win-bar { (ui::icon(Icon::Hash)) strong #cc-win-chan { (channel) } span.cc-win-tag { "Preview" } }
            div.cc-preview #cc-preview data-user=(sample.user) data-server=(sample.server) data-channel=(sample.channel) data-members=(sample.members) {
                (above)
                div.cc-msg.cc-bot {
                    (face)
                    div.cc-msg-main {
                        div.cc-head { b { (bot_name) } span.cc-tag { "APP" } span.cc-time { "Today at " (time) } }
                        div.cc-text #cc-p-text { (sample.fill(reply)) }
                        div.cc-embed #cc-p-embed hidden[embed.is_none()] {
                            strong #cc-p-etitle { (sample.fill(&e.title)) }
                            p #cc-p-edesc { (sample.fill(&e.description)) }
                            img #cc-p-eimg src=[img_ok.then_some(e.image.trim())] alt="" hidden[!img_ok];
                        }
                        div.cc-ghost #cc-p-ghost hidden[!(reply.trim().is_empty() && embed.is_none())] {
                            span {} span {}
                            p { "Write the reply and it shows here as members will see it." }
                        }
                        p.cc-only #cc-p-eph hidden[!ephemeral] { (eye_icon()) "Only you can see this" }
                    }
                }
            }
        }
    }
}

/// Discord's eye, for a reply only the person who asked sees.
fn eye_icon() -> Markup {
    html! {
        svg.icon viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" {
            (PreEscaped(r#"<path d="M1.75 8S4 3.75 8 3.75 14.25 8 14.25 8 12 12.25 8 12.25 1.75 8 1.75 8Z"/><circle cx="8" cy="8" r="2"/>"#))
        }
    }
}

/// Under an existing command's or trigger's preview: how often it ran, so an edit is made
/// knowing who it reaches. Nothing for a new one.
fn used_line(ctx: &Ctx, guild: u64, kind: Kind, id: Option<u32>, verb: &str) -> bot_core::anyhow::Result<Markup> {
    let Some(id) = id else { return Ok(html! {}) };
    let (n, at) = crate::uses(ctx, guild)?.get(&(kind.key().to_owned(), id)).copied().unwrap_or((0, 0));
    Ok(html! {
        p.cc-used-note {
            @if n > 0 { (verb) " " b { (text::num(n)) (if n == 1 { " time" } else { " times" }) } ", last " (text::ago(at)) "." } @else { "Not used yet." }
        }
    })
}

/// "‹ All commands", as a link with the dashboard's chevron.
fn back(href: &str, label: &str) -> Markup {
    html! { a.cc-back href=(href) { (ui::icon(Icon::Chevron)) (label) } }
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
            span.cc-used-line aria-hidden="true" {}
            img.avatar src=(sample.avatar) alt="" width="16" height="16";
            span { b { (sample.user) } " used " span.cc-cmd #cc-p-name { "/" (if c.name.is_empty() { "name" } else { &c.name }) } }
        }
    };
    let body = html! {
        style { (PreEscaped(CSS)) }
        (back(&page.path("commands"), "All commands"))
        div.compose.cc-edit {
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
            aside.cc-side.compose-preview {
                (preview(&ctx, &page, &sample, &sample.channel, used, &c.reply, c.embed.as_ref(), c.ephemeral))
                p.muted.small { "You stand in for the member. Placeholders fill in as they would for you." }
                (used_line(&ctx, guild, Kind::Command, existing.as_ref().map(|c| c.id), "Used")?)
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

/// What a member might say that a trigger answers: the phrase alone, or with words around it.
/// The phrase is marked, so it shows which part matches.
fn said(mode: Mode, pattern: &str) -> Markup {
    html! {
        @if mode == Mode::Contains { "… " }
        mark.cc-match { (pattern) }
        @if mode != Mode::Exact { " …" }
    }
}

async fn responder_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<TestQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let uses = crate::uses(&ctx, guild)?;
    let total: i64 = uses.iter().filter(|((k, _), _)| k == Kind::Trigger.key()).map(|(_, (n, _))| n).sum();
    let actions = (page.can_edit() && s.triggers.len() < MAX_TRIGGERS).then(|| html! { a.btn.primary href=(page.path("commands/responder/edit")) { (ui::icon(Icon::Plus)) "Add a trigger" } });
    let test = q.test.as_deref().map(|t| text::clip(t, 2000)).filter(|t| !t.trim().is_empty());
    let channel = q.channel.as_deref().and_then(|c| c.parse::<u64>().ok()).and_then(|c| ctx.channel_in(guild, Some(c)));
    let sample = Sample::new(&ctx, &page);
    let count = |id: u32| uses.get(&(Kind::Trigger.key().to_owned(), id)).map_or(0, |u| u.0);
    let top = s.triggers.iter().map(|t| count(t.id)).max().unwrap_or(0).max(1);
    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, &s, "Auto-responder"))
        div.with-rail {
            div.cc-main {
                div.readings {
                    div.reading { b data-count { (s.triggers.len()) " of " (MAX_TRIGGERS) } span { "triggers" } }
                    div.reading { b data-count { (text::num(total)) } span { "replies, all time" } }
                }
                @if s.triggers.is_empty() {
                    (ui::empty(Icon::Slash, "No triggers yet", "Add a phrase, like \"when is stream\", and the reply the bot posts when a message has it."))
                } @else {
                    p.muted.small.cc-order-note { "Checked in number order; the first match answers." }
                    div.cc-list {
                        @for (i, t) in s.triggers.iter().enumerate() {
                            article.cc-card {
                                header.cc-card-head {
                                    span.cc-num data-tip={ "Checked " (ordinal(i + 1)) } { (i + 1) }
                                    h3.cc-phrase {
                                        span.cc-mode { (t.mode.label()) }
                                        " “" (t.pattern) "”"
                                    }
                                    @if page.can_edit() {
                                        div.cc-acts {
                                            a.btn.small href={ (page.path("commands/responder/edit")) "?id=" (t.id) } { "Edit" }
                                            (ui::action_button(&page.path(&format!("commands/responder/{}/delete", t.id)), "Delete", Tone::Bad, true))
                                        }
                                    }
                                }
                                div.cc-card-body {
                                    div.cc-meta {
                                        ul.cc-facts {
                                            li { (ui::icon(Icon::Hash)) span { @if t.channels.is_empty() { "Everywhere" } @else { "Only in " (channel_list(&ctx, guild, t)) } } }
                                            li { (ui::icon(Icon::Clock)) span { (cooldown_line(t.cooldown_secs)) } }
                                        }
                                        (usage(&uses, Kind::Trigger, t.id, top, ("reply", "replies")))
                                    }
                                    (reply_bubble(&ctx, &sample, Some(said(t.mode, &t.pattern)), &t.reply, None, false))
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

/// "1st", "2nd", "11th", "23rd": the order the triggers are checked in.
fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
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
            form.stack.cc-test method="get" action={ (page.path("commands/responder")) "#test" } {
                textarea.input name="test" rows="3" placeholder="hey when is stream today" aria-label="A message to test" { (test.unwrap_or_default()) }
                (ui::select("channel", &channels, &channel.map(|c| c.to_string()).unwrap_or_default()))
                div { button.btn.small type="submit" { "Test" } }
            }
            @if let Some(said) = test {
                div.cc-result {
                    @match hit {
                        Some(t) => {
                            div.cc-hit {
                                (ui::badge("Answers", Tone::Ok))
                                span { (t.mode.label()) " “" (t.pattern) "”" }
                            }
                            @if channel.is_none() && !t.channels.is_empty() { p.muted.small { "It answers only in " (channel_list(ctx, guild, t)) ". Pick a channel to check." } }
                            (reply_bubble(ctx, &sample, Some(html! { (said) }), &t.reply, None, false))
                        }
                        None => {
                            div.cc-hit { (ui::badge("No match", Tone::Muted)) span.muted { "No trigger answers this" @if channel.is_some() { " here" } "." } }
                        }
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
    let time = page.user.local(now_ms()).strftime("%-I:%M %p").to_string();
    let above = html! {
        div.cc-msg.cc-member {
            img.cc-av src=(sample.avatar) alt="" width="36" height="36";
            div.cc-msg-main {
                div.cc-head { b { (sample.user) } span.cc-time { "Today at " (time) } }
                div.cc-text #cc-p-said {
                    @if t.pattern.is_empty() { span.cc-wait { "Type a phrase to see a message it answers." } } @else { (said(t.mode, &t.pattern)) }
                }
            }
        }
    };
    // The preview sits in the first channel it answers in.
    let first_channel = t.channels.first().map(|c| channel_name(&ctx, guild, *c).trim_start_matches('#').to_owned()).unwrap_or_else(|| sample.channel.clone());
    let title = if existing.is_some() { "Edit trigger" } else { "Add a trigger" };
    let body = html! {
        style { (PreEscaped(CSS)) }
        (back(&page.path("commands/responder"), "All triggers"))
        div.compose.cc-edit {
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
            aside.cc-side.compose-preview {
                (preview(&ctx, &page, &sample, &first_channel, above, &t.reply, None, false))
                p.muted.small { "You stand in for the member. Placeholders fill in as they would for you." }
                (used_line(&ctx, guild, Kind::Trigger, existing.as_ref().map(|t| t.id), "Answered")?)
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

const CSS: &str = r#"
.cc-main { display: flex; flex-direction: column; gap: 18px; min-width: 0; }
/* The readings span the column, so their edges meet the cards' below. */
.cc-main > .readings > .reading { flex: 1 1 140px; }
.reading b.cc-fav { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.cc-order-note { margin-bottom: -6px; }

/* A command or trigger. Left, what a member does: the name or phrase, what it's for and how
   often it ran. Right, what the bot answers, as Discord draws it. Stacked in a narrow card. */
.cc-list { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 520px), 1fr)); gap: 14px; }
.cc-card { container: cccard / inline-size; display: flex; flex-direction: column; gap: 14px; min-width: 0; padding: 16px 18px 18px; border-radius: var(--r-card); background: linear-gradient(180deg, rgba(243, 242, 248, 0.045), rgba(243, 242, 248, 0.015)); box-shadow: inset 0 0 0 1px var(--edge-1); transition: box-shadow 0.25s; }
.cc-card:hover { box-shadow: inset 0 0 0 1px var(--edge-2); }
.cc-card-head { display: flex; align-items: center; gap: 10px; min-width: 0; min-height: 30px; }
.cc-name, .cc-phrase { min-width: 0; font-size: 16px; font-weight: 600; line-height: 1.35; letter-spacing: -0.01em; overflow-wrap: anywhere; }
.cc-sigil { margin-right: 1px; color: var(--hue); }
.cc-mode { margin-right: 2px; font-size: 13px; font-weight: 500; letter-spacing: 0; color: var(--ink-2); }
.cc-num { flex: none; min-width: 14px; font-size: 13px; font-weight: 600; color: var(--ink-3); font-variant-numeric: tabular-nums; cursor: default; }
.cc-acts { display: flex; align-items: center; gap: 6px; margin-left: auto; flex: none; }
.cc-card-body { flex: 1; display: flex; flex-direction: column; gap: 12px; min-width: 0; }
.cc-meta { display: contents; }
.cc-card-body > .cc-bubble { order: 1; }
.cc-use { order: 2; margin-top: auto; }
@container cccard (min-width: 600px) {
  .cc-card-body { display: grid; grid-template-columns: minmax(0, 0.8fr) minmax(0, 1.2fr); gap: 24px; }
  .cc-meta { display: flex; flex-direction: column; gap: 10px; min-width: 0; }
  .cc-use { padding-top: 6px; }
}
.cc-about { font-size: 13.5px; line-height: 1.45; color: var(--ink-2); }
.cc-alias { font-size: 12.5px; color: var(--ink-3); }
.cc-alias b { font-weight: 500; color: var(--ink-2); }
.cc-warn { font-size: 12.5px; line-height: 1.45; color: var(--waiting); }
.cc-facts { display: flex; flex-direction: column; gap: 6px; margin: 0; padding: 0; list-style: none; }
.cc-facts li { display: flex; align-items: center; gap: 8px; min-width: 0; font-size: 13px; color: var(--ink-2); }
.cc-facts .icon { flex: none; width: 14px; height: 14px; color: var(--ink-3); }
.cc-facts span { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.cc-use { display: flex; flex-direction: column; gap: 7px; width: 100%; max-width: 300px; }
.cc-use-line { display: flex; align-items: baseline; justify-content: space-between; gap: 10px; font-size: 13px; color: var(--ink-2); }
.cc-use-line b { font-weight: 600; color: var(--ink); font-variant-numeric: tabular-nums; }
.cc-last { font-size: 12px; color: var(--ink-3); white-space: nowrap; }
.cc-meter { display: block; height: 4px; border-radius: 2px; background: var(--edge-1); overflow: hidden; }
.cc-meter > span { display: block; height: 100%; border-radius: 2px; background: var(--data); animation: fill 1.1s var(--ease) both 0.25s; }
html.same .cc-meter > span { animation: none; }

/* Discord's colours for the replies: the subject's own. */
.cc-bubble { display: flex; flex-direction: column; gap: 10px; min-width: 0; padding: 12px 14px 13px; border-radius: 12px; background: #313338; color: #dbdee1; font-size: 13.5px; line-height: 1.4; }
.cc-dmsg { display: flex; gap: 10px; min-width: 0; }
.cc-bav { flex: none; width: 28px; height: 28px; border-radius: 50%; object-fit: cover; }
.cc-bav.mark, .cc-av.mark { display: grid; place-items: center; background: linear-gradient(145deg, var(--accent-hi), var(--accent-lo)); }
.cc-bav.mark img { width: 16px; height: 16px; }
.cc-bubble-main { display: flex; flex: 1; flex-direction: column; gap: 1px; min-width: 0; }
.cc-bhead { display: flex; align-items: center; gap: 5px; }
.cc-bhead b { color: #f2f3f5; font-size: 13.5px; font-weight: 500; }
.cc-tag { height: 14px; padding: 0 4px; border-radius: 3px; background: #5865f2; color: #fff; font-size: 9.5px; font-weight: 600; line-height: 14px; }
.cc-btext { white-space: pre-line; overflow-wrap: break-word; display: -webkit-box; -webkit-line-clamp: 4; -webkit-box-orient: vertical; overflow: hidden; }
.cc-bembed { display: flex; flex-direction: column; gap: 2px; margin-top: 4px; padding: 8px 10px; border-left: 3px solid #f0806a; border-radius: 4px; background: #2b2d31; }
.cc-bembed strong { color: #f2f3f5; font-weight: 600; }
.cc-bembed p { white-space: pre-line; display: -webkit-box; -webkit-line-clamp: 3; -webkit-box-orient: vertical; overflow: hidden; }
.cc-bembed img { max-width: 100%; max-height: 90px; margin-top: 4px; border-radius: 4px; object-fit: cover; }
.cc-only { display: inline-flex; align-items: center; gap: 5px; margin-top: 3px; color: #949ba4; font-size: 11.5px; }
.cc-only .icon { width: 13px; height: 13px; }
/* The words that match, in what the member said. */
mark.cc-match { padding: 0 2px; border-radius: 3px; color: #f2f3f5; background: rgba(88, 101, 242, 0.3); }

/* Placeholders, text commands and the test, in the rail. */
.cc-ph { display: grid; grid-template-columns: auto 1fr; align-items: center; gap: 8px 12px; font-size: 12.5px; color: var(--ink-2); }
.cc-ph code { justify-self: start; padding: 2px 7px; border-radius: 6px; color: var(--ink); background: var(--control); }
.cc-fs { min-width: 0; margin: 0; padding: 0; border: 0; }
.cc-test textarea.input { min-height: 72px; }
.cc-result { display: flex; flex-direction: column; gap: 10px; animation: rise 0.45s var(--ease) both; }
.cc-hit { display: flex; align-items: center; flex-wrap: wrap; gap: 8px; font-size: 13px; }

/* The edit pages: the form, and the reply in a Discord channel beside it. */
.cc-back { align-self: flex-start; display: inline-flex; align-items: center; gap: 4px; margin-bottom: -12px; font-size: 13px; color: var(--ink-3); transition: color 0.15s; }
.cc-back:hover { color: var(--ink); }
.cc-back .icon { width: 14px; height: 14px; transform: rotate(90deg); transition: transform 0.25s var(--ease); }
.cc-back:hover .icon { transform: rotate(90deg) translateY(2px); }
/* The form, and the channel beside it from a medium width (it stacks under the form on a phone). */
@container col (min-width: 760px) {
  .compose.cc-edit { grid-template-columns: minmax(0, 1.25fr) minmax(320px, 1fr); }
  .compose.cc-edit > .compose-preview { grid-column: 2; grid-row: 1; position: sticky; top: 16px; }
}
.cc-ecard:has(#embed:not(:checked)) .row + .row { display: none; }
.cc-side { display: flex; flex-direction: column; gap: 10px; min-width: 0; }
.cc-side > p { padding: 0 4px; }
.cc-used-note { font-size: 12.5px; color: var(--ink-2); }
.cc-used-note b { font-weight: 600; color: var(--ink); }
.cc-win { overflow: hidden; border-radius: 16px; background: #313338; box-shadow: 0 30px 70px -30px rgba(0, 0, 0, 0.9), 0 0 0 1px rgba(0, 0, 0, 0.45), inset 0 0 0 1px rgba(255, 255, 255, 0.06); }
.cc-win-bar { display: flex; align-items: center; gap: 8px; height: 46px; padding: 0 14px; border-bottom: 1px solid #26272b; color: #80848e; }
.cc-win-bar .icon { width: 18px; height: 18px; }
.cc-win-bar strong { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: #f2f3f5; font-size: 14.5px; font-weight: 600; }
.cc-win-tag { padding: 3px 8px; border-radius: 6px; font-size: 11px; color: #b5bac1; background: rgba(255, 255, 255, 0.06); }
.cc-preview { display: flex; flex-direction: column; justify-content: flex-end; gap: 16px; min-height: 220px; padding: 18px 14px 20px; color: #dbdee1; }
.cc-preview [hidden] { display: none !important; }
.cc-used { position: relative; display: flex; align-items: center; gap: 6px; min-width: 0; margin-bottom: -12px; padding-left: 50px; font-size: 12.5px; color: #949ba4; animation: fade-in 0.5s ease both 0.2s; }
.cc-used-line { position: absolute; left: 17px; top: 9px; width: 27px; height: 12px; border-left: 2px solid #4e5058; border-top: 2px solid #4e5058; border-top-left-radius: 6px; }
.cc-used .avatar { width: 16px; height: 16px; }
.cc-used b { color: #f2f3f5; font-weight: 500; }
.cc-cmd { color: #00a8fc; font-weight: 500; }
.cc-msg { display: flex; gap: 14px; min-width: 0; }
.cc-av { flex: none; width: 36px; height: 36px; border-radius: 50%; object-fit: cover; }
.cc-av.mark img { width: 20px; height: 20px; }
.cc-msg-main { display: flex; flex: 1; flex-direction: column; gap: 2px; min-width: 0; }
.cc-head { display: flex; align-items: center; flex-wrap: wrap; gap: 6px; }
.cc-head b { color: #f2f3f5; font-size: 15px; font-weight: 500; }
.cc-head .cc-tag { height: 15px; padding: 0 4.5px; font-size: 10px; line-height: 15px; }
.cc-time { color: #949ba4; font-size: 11.5px; }
.cc-text { font-size: 14.5px; line-height: 1.375; white-space: pre-wrap; overflow-wrap: break-word; }
.cc-text:empty { display: none; }
.cc-embed { display: flex; flex-direction: column; gap: 6px; max-width: 516px; margin-top: 4px; padding: 10px 14px 12px 12px; border-left: 4px solid #f0806a; border-radius: 4px; background: #2b2d31; font-size: 13.5px; }
.cc-embed strong { color: #f2f3f5; font-size: 15px; font-weight: 600; }
.cc-embed strong:empty, .cc-embed p:empty { display: none; }
.cc-embed p { white-space: pre-wrap; overflow-wrap: break-word; }
.cc-embed img { max-width: 100%; max-height: 200px; margin-top: 4px; border-radius: 4px; object-fit: cover; }
.cc-wait { color: #949ba4; font-style: italic; }
/* An empty reply: the outline of a message, and a line on how it fills. */
.cc-ghost { display: flex; flex-direction: column; gap: 8px; max-width: 360px; margin-top: 6px; }
.cc-ghost span { height: 9px; border-radius: 5px; background: rgba(255, 255, 255, 0.07); }
.cc-ghost span:first-child { width: 88%; }
.cc-ghost span:nth-child(2) { width: 56%; }
.cc-ghost p { margin-top: 4px; color: #949ba4; font-size: 13px; }
.cc-bot { animation: cc-arrive 0.7s var(--ease) both 0.35s; }
@keyframes cc-arrive { from { opacity: 0; transform: translateY(14px); filter: blur(4px); } }
@media (max-width: 560px) {
  .cc-card { padding: 14px 14px 16px; }
  .cc-card-head { flex-wrap: wrap; }
}
"#;

/// Keeps the preview in step with the form as you type.
const JS: &str = r#"(() => {
  const p = document.getElementById('cc-preview');
  if (!p) return;
  const d = p.dataset;
  const $ = id => document.getElementById(id);
  const val = id => ($(id) || {}).value || '';
  const on = id => !!($(id) || {}).checked;
  const here = () => (($('cc-win-chan') || {}).textContent || d.channel).trim();
  const fill = s => s.replaceAll('{user}', d.user).replaceAll('{mention}', '@' + d.user).replaceAll('{server}', d.server).replaceAll('{channel}', '#' + here()).replaceAll('{members}', d.members);
  const set = (id, v) => { const el = $(id); if (el) el.textContent = v; };
  const show = (id, yes) => { const el = $(id); if (el) el.hidden = !yes; };
  const update = () => {
    // A trigger's preview sits in the first channel it answers in.
    const picked = document.querySelector('#channels input:checked');
    if ($('channels')) set('cc-win-chan', picked ? picked.closest('label').textContent.trim().replace(/^#/, '') : d.channel);
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
    // Nothing to say yet: a faint outline of a message instead of an empty one.
    const embedOn = $('embed') ? on('embed') && !!(val('embed_title').trim() || val('embed_desc').trim() || val('embed_image').trim().startsWith('https://')) : false;
    show('cc-p-ghost', !val('reply').trim() && !embedOn);
    const said = $('cc-p-said');
    if ($('pattern') && said) {
      // The member's message with the phrase marked, so it shows which part matches.
      const pat = val('pattern').trim().toLowerCase(), mode = val('mode');
      const parts = [];
      if (!pat) {
        const w = document.createElement('span');
        w.className = 'cc-wait';
        w.textContent = 'Type a phrase to see a message it answers.';
        parts.push(w);
      } else {
        const m = document.createElement('mark');
        m.className = 'cc-match';
        m.textContent = pat;
        if (mode === 'contains') parts.push('… ');
        parts.push(m);
        if (mode !== 'exact') parts.push(' …');
      }
      said.replaceChildren(...parts);
    }
  };
  // Until the page is swapped out (app.js).
  const signal = window.botPage?.signal;
  document.addEventListener('input', update, { signal });
  document.addEventListener('change', update, { signal });
  update();
})();"#;
