use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::web::ui::{self, Tone};
use bot_core::twilight_model::channel::ChannelType;
use bot_core::twilight_model::channel::permission_overwrite::{PermissionOverwrite, PermissionOverwriteType};
use bot_core::twilight_model::guild::Permissions;
use bot_core::twilight_model::id::Id;
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};
use serde::Deserialize;

use crate::mail::{self, Conv};
use crate::{DEFAULT_CLOSING, DEFAULT_GREETING, KEY, Settings};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/modmail", get(page))
        .route("/g/{guild}/modmail/settings", post(save))
        .route("/g/{guild}/modmail/{id}/close", post(close))
}

fn thread_url(guild: u64, thread: u64) -> String {
    format!("https://discord.com/channels/{guild}/{thread}")
}

fn channel_name(ctx: &Ctx, guild: u64, channel: u64) -> String {
    ctx.text_channels(guild).into_iter().find(|(id, _)| *id == channel).map(|(_, n)| format!("#{n}")).unwrap_or_else(|| "a deleted channel".into())
}

/// Channels that can hold conversations: text channels and forums. Announcement channels can't
/// take the public threads a conversation opens.
fn staff_channels(ctx: &Ctx, guild: u64) -> Vec<(u64, String)> {
    ctx.text_channels(guild)
        .into_iter()
        .filter(|(id, _)| ctx.cache.channel(Id::new(*id)).is_some_and(|c| c.kind != ChannelType::GuildAnnouncement))
        .collect()
}

/// Whether @everyone can see the channel, from the cached @everyone role and the channel's own
/// overwrites.
fn everyone_can_view(ctx: &Ctx, guild: u64, channel: u64) -> bool {
    let Some(ch) = ctx.cache.channel(Id::new(channel)) else { return false };
    let base = ctx.cache.role(Id::new(guild)).map(|r| r.permissions).unwrap_or_else(Permissions::empty);
    everyone_sees(base, ch.permission_overwrites.as_deref().unwrap_or_default(), guild)
}

/// @everyone's view of a channel: the role's permissions, then the channel's overwrite for
/// @everyone (the role shares the server's id).
fn everyone_sees(base: Permissions, overwrites: &[PermissionOverwrite], guild: u64) -> bool {
    if base.contains(Permissions::ADMINISTRATOR) {
        return true;
    }
    let mut view = base.contains(Permissions::VIEW_CHANNEL);
    if let Some(o) = overwrites.iter().find(|o| o.kind == PermissionOverwriteType::Role && o.id.get() == guild) {
        if o.deny.contains(Permissions::VIEW_CHANNEL) {
            view = false;
        }
        if o.allow.contains(Permissions::VIEW_CHANNEL) {
            view = true;
        }
    }
    view
}

#[derive(Deserialize)]
struct ViewQuery {
    view: Option<String>,
}

async fn page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<ViewQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let now = now_ms();
    let view = q.view.as_deref().unwrap_or("open");
    let stats = ctx.db.with(|c| mail::stats(c, guild, now))?;
    let rows = match view {
        "closed" => ctx.db.with(|c| mail::list_closed(c, guild, 100))?,
        "settings" => Vec::new(),
        _ => ctx.db.with(|c| mail::list_open(c, guild, 100))?,
    };
    let mut channel_options = vec![(String::new(), "None".to_owned())];
    channel_options.extend(staff_channels(&ctx, guild).into_iter().map(|(id, n)| (id.to_string(), format!("#{n}"))));
    let base = page.path("modmail");
    let active = match view {
        "closed" => "Closed",
        "settings" => "Settings",
        _ => "Open",
    };
    let tabs = ui::tabs_counted(
        &[("Open", base.clone(), Some(stats.open)), ("Closed", format!("{base}?view=closed"), None), ("Settings", format!("{base}?view=settings"), None)],
        active,
    );

    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs)
        @if view == "settings" {
            @if let Some(ch) = s.channel.filter(|c| everyone_can_view(&ctx, guild, *c)) {
                div.need.alert {
                    span.need-icon.alert { (ui::icon(Icon::Users)) }
                    div.grow {
                        strong { "Not private" }
                        p.muted.small { "Everyone can read conversations in " (channel_name(&ctx, guild, ch)) ". Hide it from @everyone in Discord." }
                    }
                }
            }
            // The texts beside the DMs they become, the way the member reads them.
            div.compose {
                (ui::settings_form(&page, &page.path("modmail/settings"), html! {
                    (ui::section("Where they open", None, ui::card(html! {
                        (ui::field_select("channel", "Staff channel", Some("A text channel gets a thread per conversation, a forum a post. Keep it private to staff. None turns modmail off."), &channel_options, &s.channel.map(|c| c.to_string()).unwrap_or_default()))
                        (ui::field_role(&ctx, guild, "staff_role", "Staff role", Some("Pinged once when a conversation opens, and allowed to close one."), s.staff_role))
                    })))
                    (ui::section("Messages to the member", None, ui::card(html! {
                        (ui::field_textarea("greeting", "Greeting", Some("Sent to the member when a conversation opens. {server} is the server's name."), &s.greeting, DEFAULT_GREETING))
                        (ui::field_textarea("closing", "Closing text", Some("Sent to the member when staff close the conversation."), &s.closing, DEFAULT_CLOSING))
                    })))
                }))
                div.compose-preview.mm-previews {
                    (dm_preview(&ctx, &page, guild, &s))
                    (staff_preview(&ctx, &page, guild, &s))
                }
            }
        } @else {
            div.with-rail {
                div.mm-main {
                    div.readings.mm-readings {
                        div.reading { b data-count { (stats.open) } span { "Open now" } }
                        div.reading.mm-wait[stats.waiting > 0] { b data-count { (stats.waiting) } span { "Waiting on staff" } }
                        div.reading { b data-count { (stats.opened_week) } span { "Opened this week" } }
                        div.reading { b data-count { (stats.closed_week) } span { "Closed this week" } }
                    }
                    @if rows.is_empty() {
                        @if view == "closed" {
                            (ui::empty(Icon::Mail, "Nothing closed yet", "Closed conversations stay here with who closed them."))
                        } @else if s.channel.is_none() {
                            (ui::empty(Icon::Mail, "Modmail is off", "Pick a staff channel in Settings. Conversations show up here when members DM the bot."))
                        } @else {
                            (ui::empty(Icon::Mail, "No open conversations", "Conversations show up here when members DM the bot."))
                        }
                    } @else {
                        div.mm-list {
                            @for (i, c) in rows.iter().enumerate() { (conversation(&ctx, &page, guild, c, now, i)) }
                        }
                        @if rows.len() == 100 { p.muted.small { "Showing the latest 100." } }
                    }
                }
                aside.rail {
                    (closed_recently(&ctx, &page, guild))
                    div.rail-block {
                        h2 { "Where they open" }
                        div.mini {
                            span.mm-rail-ic { (ui::icon(Icon::Hash)) }
                            div.grow {
                                @match s.channel {
                                    Some(ch) => { span { (channel_name(&ctx, guild, ch)) } span.muted { "A thread for each conversation" } },
                                    None => { span { "No staff channel" } span.muted { a href={ (base) "?view=settings" } { "Pick one in Settings" } } },
                                }
                            }
                        }
                        @if let Some(role) = s.staff_role {
                            @if let Some((_, name, color)) = ctx.roles(guild).into_iter().find(|(id, _, _)| *id == role) {
                                div.mini {
                                    span.mm-rail-ic style=[(color != 0).then(|| format!("--role:#{color:06x}"))] { (ui::icon(Icon::Bell)) }
                                    div.grow { span { "@" (name) } span.muted { "Pinged when one opens" } }
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Modmail", None, body).into_response())
}

/// One conversation: who, how long it's waited and on whom, and its latest message as a chat
/// bubble, the member's on the left and staff's on the right.
fn conversation(ctx: &Ctx, page: &Page, guild: u64, c: &Conv, now: i64, i: usize) -> Markup {
    let name = c.name(ctx, guild);
    let staff_wrote = !c.waiting_on_staff();
    let author = if staff_wrote { ctx.display_name(guild, c.last_author) } else { name.clone() };
    let stamp = |ms: i64| page.user.local(ms).strftime("%b %-d, %H:%M").to_string();
    let earlier = (c.messages - 1).max(0);
    let waiting = c.is_open() && c.waiting_on_staff();
    html! {
        article.mm-conv.waiting[waiting].closed[!c.is_open()] style={ "--i:" (i.min(8)) } {
            header.mm-head {
                img.avatar.mm-face src=(ctx.member_avatar(guild, c.user_id)) alt="" width="44" height="44" loading="lazy";
                div.grow.mm-who {
                    strong.clip { (name) }
                    span.muted.small {
                        "Opened " span title=(stamp(c.opened_at)) { (text::ago(c.opened_at)) }
                        " · " (c.messages) @if c.messages == 1 { " message" } @else { " messages" }
                    }
                }
                @if waiting {
                    span.mm-state.wait title={ "Last message " (stamp(c.last_at)) } { span.dot.waiting {} "Waiting " (mail::span((now - c.last_at) / 1000)) }
                } @else if c.is_open() {
                    span.mm-state { "Waiting on " (name) }
                } @else {
                    span.mm-state.done { (ui::icon(Icon::Check)) "Closed" }
                }
            }
            div.mm-thread {
                @if earlier > 0 {
                    div.mm-earlier {
                        span.mm-ghosts aria-hidden="true" { span {} span {} }
                        (earlier) @if earlier == 1 { " earlier message" } @else { " earlier messages" } " in the thread"
                    }
                }
                @if !c.preview.is_empty() {
                    div.mm-msg.staff[staff_wrote] {
                        img.avatar src=(ctx.member_avatar(guild, c.last_author)) alt="" width="30" height="30" loading="lazy";
                        div.mm-bubble {
                            div.mm-by {
                                strong { (author) }
                                @if staff_wrote { span.mm-tag { "Staff" } }
                                span title=(stamp(c.last_at)) { (text::ago(c.last_at)) }
                            }
                            p { (c.preview) }
                        }
                    }
                }
            }
            footer.mm-foot {
                @if !c.is_open() {
                    span.mm-closer {
                        @match c.closed_by {
                            Some(by) => {
                                img.avatar src=(ctx.member_avatar(guild, by)) alt="" width="20" height="20" loading="lazy";
                                span { "Closed by " strong { (ctx.display_name(guild, by)) } " · " (text::ago(c.closed_at.unwrap_or(now))) }
                            },
                            None => { span { "Closed in Discord · " (text::ago(c.closed_at.unwrap_or(now))) } },
                        }
                    }
                }
                div.mm-acts {
                    a.btn.small.outline href=(thread_url(guild, c.thread_id)) target="_blank" rel="noopener" { (ui::icon(Icon::External)) "Open thread" }
                    @if page.can_edit() && c.is_open() { (ui::action_button(&page.path(&format!("modmail/{}/close", c.id)), "Close", Tone::Bad, true)) }
                }
            }
        }
    }
}

/// The last three conversations to close, with who closed them.
fn closed_recently(ctx: &Ctx, page: &Page, guild: u64) -> Markup {
    let recent = ctx.db.with(|c| mail::list_closed(c, guild, 3)).unwrap_or_default();
    html! {
        div.rail-block {
            h2 {
                "Closed recently"
                @if !recent.is_empty() { a.small.muted href={ (page.path("modmail")) "?view=closed" } { "All closed" } }
            }
            @if recent.is_empty() {
                p.muted.small { "Closed conversations show up here with who closed them." }
            }
            @for c in &recent {
                div.mini.mm-closed-line {
                    span.mm-pair {
                        img.avatar src=(ctx.member_avatar(guild, c.user_id)) alt="" width="30" height="30" loading="lazy";
                        @if let Some(by) = c.closed_by { img.avatar.mm-by-face src=(ctx.member_avatar(guild, by)) alt="" width="18" height="18" loading="lazy"; }
                    }
                    div.grow {
                        span { (c.name(ctx, guild)) }
                        span.muted {
                            @match c.closed_by {
                                Some(by) => { "by " (ctx.display_name(guild, by)) },
                                None => { "in Discord" },
                            }
                            " · " (c.messages) " messages"
                        }
                    }
                    @if let Some(at) = c.closed_at { span.mono.small title=(page.user.local(at).strftime("%b %-d, %H:%M").to_string()) { (text::ago(at)) } }
                }
            }
        }
    }
}

/// The greeting and the closing text as the member gets them: DMs from the bot, as saved.
fn dm_preview(ctx: &Ctx, page: &Page, guild: u64, s: &Settings) -> Markup {
    let server = ctx.guild_name(guild);
    let fill = |t: &str, default: &str| text::fill(if t.trim().is_empty() { default } else { t.trim() }, &[("server", &server)]);
    let bot = ctx.bot_id().and_then(|b| ctx.known_name(guild, b)).unwrap_or_else(|| "The bot".to_owned());
    let line = |label: &str, body: String, i: usize| {
        html! {
            div.mm-dm style={ "--s:" (i) } {
                span.mm-bot { (ui::mark()) }
                div.grow {
                    div.mm-dm-by { strong { (bot) } span.mm-app { "APP" } span.mm-dm-when { (label) } }
                    p { (body) }
                }
            }
        }
    };
    html! {
        section.block {
            div.block-head {
                h2 { "What the member gets" }
                p.muted { "DMs from the bot, as saved. Save to see a change here." }
            }
            div.mm-dms {
                div.mm-dms-head { span.mm-at { "@" } strong { (bot) } }
                (line("when it opens", fill(&s.greeting, DEFAULT_GREETING), 0))
                div.mm-dms-gap { span { "Staff answer in the thread, and their replies arrive here" } }
                (line("when staff close it", fill(&s.closing, DEFAULT_CLOSING), 1))
            }
            @if !page.can_edit() { p.muted.small { "Ask an admin to change these texts." } }
        }
    }
}

/// The thread a conversation opens in, as staff see it: the ping and the bot's note, as saved,
/// with the viewer standing in for the member.
fn staff_preview(ctx: &Ctx, page: &Page, guild: u64, s: &Settings) -> Markup {
    let bot = ctx.bot_id().and_then(|b| ctx.known_name(guild, b)).unwrap_or_else(|| "The bot".to_owned());
    let who = &page.user.name;
    let role = s.staff_role.and_then(|r| ctx.roles(guild).into_iter().find(|(id, _, _)| *id == r));
    let channel = s.channel.and_then(|c| ctx.text_channels(guild).into_iter().find(|(id, _)| *id == c)).map(|(_, n)| n);
    html! {
        section.block {
            div.block-head {
                h2 { "What staff see" }
                p.muted { "The thread each conversation opens, as if you wrote in." }
            }
            @match channel {
                None => { (ui::empty(Icon::Hash, "No staff channel", "Pick one and each conversation opens a thread there.")) },
                Some(ch) => {
                    div.mm-dms {
                        div.mm-dms-head {
                            span.mm-at { "#" } span.mm-crumb { (ch) }
                            (ui::icon(Icon::Chevron))
                            strong.clip { (who) }
                        }
                        div.mm-dm style="--s:2" {
                            span.mm-bot { (ui::mark()) }
                            div.grow {
                                div.mm-dm-by { strong { (bot) } span.mm-app { "APP" } }
                                p {
                                    @if let Some((_, name, color)) = &role {
                                        span.mm-mention.role style=[(*color != 0).then(|| format!("--role:#{color:06x}"))] { "@" (name) } ", new modmail from "
                                    } @else {
                                        "New modmail from "
                                    }
                                    span.mm-mention { "@" (who) } "."
                                }
                                div.embed-preview.mm-intro style={ "--embed:#" (format!("{:06x}", mail::COLOR)) } {
                                    strong { (who) }
                                    span { (mail::THREAD_INTRO) }
                                }
                                span.mm-dbtn { "Close" }
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn save(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let mut s = (*ctx.settings::<Settings>(guild, KEY)?).clone();
    s.channel = form.channel(&ctx, guild, "channel");
    if let Some(ch) = s.channel
        && !staff_channels(&ctx, guild).iter().any(|(id, _)| *id == ch)
    {
        return Err(WebError::Bad("Conversations can't open in an announcement channel. Pick a text channel or a forum.".into()));
    }
    s.staff_role = form.role(&ctx, guild, "staff_role");
    s.greeting = text::clip(&form.text("greeting"), 1500);
    s.closing = text::clip(&form.text("closing"), 1500);
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed modmail settings");
    Ok(saved(&page.path("modmail?view=settings")))
}

async fn close(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    if let Some(c) = ctx.db.with(|c| mail::by_id(c, guild, id))?
        && mail::close_from_dashboard(&ctx, guild, id, page.user.id).await?
    {
        ctx.audit(&page.user, Some(guild), KEY, format!("Closed the modmail conversation with {}", c.name(&ctx, guild)));
    }
    Ok(saved(&page.path("modmail")))
}

const CSS: &str = r#"
.mm-main { display: flex; flex-direction: column; gap: 22px; min-width: 0; }
.mm-readings { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 10px; }
.mm-readings .reading { min-width: 0; }
.mm-readings .reading:first-child { background: radial-gradient(120% 130% at 0% 0%, color-mix(in oklab, var(--hue) 8%, transparent), rgba(243, 242, 248, 0.02) 70%); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--hue) 28%, transparent); }
.mm-readings .reading:first-child::before { width: 100%; opacity: 1; }
.mm-readings .mm-wait b { color: var(--waiting); }
.mm-readings .mm-wait::before { background: linear-gradient(90deg, var(--waiting), transparent); }

/* A conversation: who, then its latest message as a bubble. */
/* Conversations sit two or three across on a wide screen instead of one stretched card each.
   One conversation fills the row. */
.mm-list { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 480px), 1fr)); gap: 12px; }
.mm-conv { display: flex; flex-direction: column; gap: 14px; padding: 16px 18px; border-radius: var(--r-card); background: linear-gradient(180deg, rgba(243, 242, 248, 0.045), rgba(243, 242, 248, 0.015)); box-shadow: inset 0 0 0 1px var(--edge-1); animation: rise 0.55s var(--ease) both; animation-delay: calc(var(--i, 0) * 80ms + 160ms); transition: box-shadow 0.25s, transform 0.35s var(--ease); }
.mm-conv:hover { transform: translateY(-1px); box-shadow: inset 0 0 0 1px var(--edge-2), var(--shadow-lift); }
.mm-conv.waiting { background: radial-gradient(90% 120% at 100% 0%, color-mix(in oklab, var(--waiting) 5%, transparent), transparent 55%), linear-gradient(180deg, rgba(243, 242, 248, 0.05), rgba(243, 242, 248, 0.015)); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--waiting) 28%, transparent); }
.mm-head { display: flex; align-items: center; gap: 13px; min-width: 0; }
.mm-face { width: 44px; height: 44px; box-shadow: 0 0 0 2px var(--page), 0 0 0 3.5px var(--edge-2); }
.mm-conv.waiting .mm-face { box-shadow: 0 0 0 2px var(--page), 0 0 0 3.5px color-mix(in oklab, var(--waiting) 70%, transparent); }
.mm-who { display: flex; flex-direction: column; gap: 1px; }
.mm-who strong { font-size: 14.5px; font-weight: 600; }
.mm-state { flex: none; display: inline-flex; align-items: center; gap: 7px; height: 26px; padding: 0 11px; border-radius: 13px; background: var(--control); color: var(--ink-2); font-size: 12px; font-weight: 500; }
.mm-state.wait { color: var(--waiting); background: color-mix(in oklab, var(--waiting) 13%, transparent); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--waiting) 30%, transparent); }
.mm-state.wait .dot::after { content: ""; position: absolute; inset: 0; border-radius: 50%; background: var(--waiting); animation: ping 2.2s ease-out infinite 1s; }
.mm-state.done .icon { width: 13px; height: 13px; }
.mm-thread { display: flex; flex-direction: column; gap: 8px; padding-left: 57px; }
.mm-earlier { display: flex; align-items: center; gap: 10px; font-size: 12px; color: var(--ink-3); }
.mm-ghosts { position: relative; width: 30px; height: 16px; flex: none; }
.mm-ghosts span { position: absolute; left: 0; height: 7px; border-radius: 4px; background: var(--control); }
.mm-ghosts span:first-child { top: 1px; width: 26px; }
.mm-ghosts span:last-child { top: 9px; width: 18px; background: var(--hover); }
.mm-msg { display: flex; align-items: flex-end; gap: 9px; max-width: min(560px, 100%); }
.mm-msg > .avatar { width: 30px; height: 30px; }
.mm-bubble { min-width: 0; padding: 9px 13px 10px; border-radius: 16px 16px 16px 5px; background: #2b2d31; color: #dbdee1; box-shadow: 0 10px 24px -16px rgba(0, 0, 0, 0.9); transform-origin: 0% 100%; animation: mm-pop 0.55s var(--spring) both; animation-delay: calc(var(--i, 0) * 80ms + 480ms); }
.mm-bubble p { font-size: 13.5px; line-height: 1.5; overflow-wrap: anywhere; }
.mm-by { display: flex; align-items: center; gap: 7px; margin-bottom: 2px; font-size: 11.5px; color: #949ba4; }
.mm-by strong { color: #f2f3f5; font-size: 12.5px; font-weight: 600; }
.mm-tag { height: 16px; padding: 0 5px; border-radius: 4px; font: 700 9.5px/16px var(--sans); letter-spacing: 0.02em; background: var(--accent); color: var(--accent-ink); }
.mm-msg.staff { flex-direction: row-reverse; align-self: flex-end; }
.mm-msg.staff .mm-bubble { border-radius: 16px 16px 5px 16px; background: linear-gradient(180deg, color-mix(in oklab, var(--accent) 26%, var(--solid)), color-mix(in oklab, var(--accent) 16%, var(--raised))); color: var(--ink); transform-origin: 100% 100%; box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--accent) 30%, transparent); }
.mm-msg.staff .mm-by { justify-content: flex-end; color: var(--ink-3); }
@keyframes mm-pop { from { opacity: 0; transform: scale(0.6); } }
.mm-foot { display: flex; align-items: center; justify-content: flex-end; gap: 10px; flex-wrap: wrap; padding-left: 57px; margin-top: auto; }
.mm-closer { display: inline-flex; align-items: center; gap: 7px; margin-right: auto; font-size: 12.5px; color: var(--ink-2); }
.mm-closer .avatar { width: 20px; height: 20px; }
.mm-closer strong { color: var(--ink); font-weight: 600; }
.mm-acts { display: flex; gap: 6px; align-items: center; }
.mm-conv.closed .mm-bubble { animation: none; }

/* The rail. */
.mm-rail-ic { display: grid; place-items: center; width: 30px; height: 30px; flex: none; border-radius: 10px; background: color-mix(in oklab, var(--role, var(--hue)) 16%, transparent); color: var(--role, var(--hue)); }
.mm-rail-ic .icon { width: 15px; height: 15px; }
.mm-pair { position: relative; flex: none; width: 30px; height: 30px; }
.mm-pair > .avatar:first-child { width: 30px; height: 30px; }
.mm-by-face { position: absolute; right: -5px; bottom: -4px; width: 18px; height: 18px; box-shadow: 0 0 0 2px var(--raised); }
.mm-closed-line .mono { color: var(--ink-3); }
.rail-block h2 a { font-weight: 400; }

/* The member's DMs from the bot, on Discord's own colours. */
.mm-dms { display: flex; flex-direction: column; gap: 12px; padding: 0 0 14px; border-radius: 12px; background: #313338; overflow: hidden; box-shadow: 0 14px 30px -18px rgba(0, 0, 0, 0.9), inset 0 0 0 1px rgba(255, 255, 255, 0.05); }
.mm-dms-head { display: flex; align-items: center; gap: 8px; height: 38px; padding: 0 14px; background: #2b2d31; border-bottom: 1px solid rgba(0, 0, 0, 0.28); color: #f2f3f5; font-size: 13px; }
.mm-at { color: #80848e; font-weight: 600; }
.mm-dm { display: flex; gap: 11px; padding: 0 14px; animation: rise 0.5s var(--ease) both; animation-delay: calc(var(--s, 0) * 260ms + 300ms); }
.mm-bot { display: grid; place-items: center; width: 34px; height: 34px; flex: none; border-radius: 50%; color: #fff; background: linear-gradient(145deg, var(--accent-hi), var(--accent-lo)); }
.mm-bot .icon { width: 20px; height: 20px; }
.mm-dm-by { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; }
.mm-dm-by strong { color: #f2f3f5; font-size: 13.5px; font-weight: 600; }
.mm-app { height: 15px; padding: 0 4px; border-radius: 3px; background: #5865f2; color: #fff; font: 600 9.5px/15px var(--sans); }
.mm-dm-when { color: #949ba4; font-size: 11px; }
.mm-dm p { max-width: 72ch; color: #dbdee1; font-size: 13.5px; line-height: 1.45; margin-top: 2px; white-space: pre-wrap; overflow-wrap: anywhere; }
.mm-dms-gap { display: flex; align-items: center; gap: 10px; padding: 0 14px; color: #949ba4; font-size: 11px; }
.mm-dms-gap::before, .mm-dms-gap::after { content: ""; flex: 1; height: 1px; background: rgba(255, 255, 255, 0.08); }
.mm-dms-gap span { text-align: center; max-width: 70%; }
/* The staff thread: its channel and name, Discord's mentions, the bot's note and its button. */
.mm-previews { display: flex; flex-direction: column; gap: 26px; }
.mm-dms-head .icon { width: 13px; height: 13px; color: #80848e; transform: rotate(-90deg); }
.mm-crumb { color: #b5bac1; font-weight: 600; }
.mm-mention { padding: 0 2px; border-radius: 3px; font-weight: 500; color: #c9cdfb; background: rgba(88, 101, 242, 0.3); }
.mm-mention.role { color: var(--role, #c9cdfb); background: color-mix(in oklab, var(--role, #5865f2) 10%, transparent); }
.mm-intro { margin-top: 6px; font-size: 13px; }
.mm-dbtn { display: inline-flex; align-items: center; height: 30px; margin-top: 8px; padding: 0 14px; border-radius: 3px; background: #4e5058; color: #fff; font-size: 13px; font-weight: 500; }

/* Laid out for the column's own width, so the sidebar and rail are already counted. */
@container col (max-width: 700px) {
  .mm-readings { grid-template-columns: repeat(2, minmax(0, 1fr)); }
}
@container col (max-width: 560px) {
  .mm-conv { padding: 14px; }
  .mm-head { flex-wrap: wrap; row-gap: 8px; }
  .mm-head .mm-who { flex: 1 1 calc(100% - 60px); }
  .mm-state { margin-left: 57px; }
  .mm-thread, .mm-foot { padding-left: 0; }
  .mm-foot { justify-content: flex-start; }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn overwrite(id: u64, allow: Permissions, deny: Permissions) -> PermissionOverwrite {
        PermissionOverwrite { allow, deny, id: Id::new(id), kind: PermissionOverwriteType::Role }
    }

    #[test]
    fn a_channel_hidden_from_everyone_is_private() {
        let (guild, view) = (1, Permissions::VIEW_CHANNEL);
        let none = Permissions::empty();
        assert!(everyone_sees(view, &[], guild));
        assert!(!everyone_sees(view, &[overwrite(guild, none, view)], guild));
        // An overwrite for another role doesn't hide it from everyone.
        assert!(everyone_sees(view, &[overwrite(5, none, view)], guild));
        assert!(!everyone_sees(none, &[], guild));
        assert!(everyone_sees(none, &[overwrite(guild, view, none)], guild));
        assert!(everyone_sees(Permissions::ADMINISTRATOR, &[overwrite(guild, none, view)], guild));
    }
}
