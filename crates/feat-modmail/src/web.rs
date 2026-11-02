use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::{IntoResponse, Redirect};
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::web::dc;
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
        .route("/g/{guild}/modmail/test", post(send_test))
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
    sent: Option<String>,
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
        (ui::sent_toast(q.sent.is_some(), "Test sent to your DMs"))
        (tabs)
        @if view == "settings" {
            @if let Some(ch) = s.channel.filter(|c| everyone_can_view(&ctx, guild, *c)) {
                section.mm-panel.mm-alert {
                    div.need.alert {
                        span.need-icon.alert { (ui::icon(Icon::Users)) }
                        div.grow {
                            strong { "Not private" }
                            span { "Everyone can read conversations in " (channel_name(&ctx, guild, ch)) ". Hide it from @everyone in Discord." }
                        }
                    }
                }
            }
            // The texts beside the DMs they become, one at a time: what the member gets, or the
            // thread staff see. The side blocks sit under it so the columns end together.
            div.compose.mm-compose {
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
                aside.rail.mm-rail.compose-preview {
                    div.rail-block.mm-previews {
                        div.mm-pv-head {
                            div.seg role="group" aria-label="Which side" {
                                label { input type="radio" name="mm-which" value="member" checked; "What the member gets" }
                                label { input type="radio" name="mm-which" value="staff"; "What staff see" }
                            }
                        }
                        div.mm-pv.mm-pv-member { (dm_preview(&ctx, &page, guild, &s)) }
                        div.mm-pv.mm-pv-staff { (staff_preview(&ctx, &page, guild, &s)) }
                    }
                    div.rail-block {
                        h2 { "Modmail" }
                        (figures(&stats, true))
                    }
                    (closed_recently(&ctx, &page, guild))
                }
            }
        } @else if view == "closed" {
            // A sub-tab: the closed conversations, the numbers and where they open on the right.
            div.with-rail.mm-cols {
                section.mm-panel {
                    h2 { "Closed conversations" @if rows.len() == 100 { span.muted { "the latest 100" } } }
                    @if rows.is_empty() {
                        (ui::empty(Icon::Mail, "Nothing closed yet", "Closed conversations stay here with who closed them."))
                    } @else {
                        div.mm-list { @for c in &rows { (conversation(&ctx, &page, guild, c, now)) } }
                    }
                }
                aside.rail.mm-rail {
                    div.rail-block {
                        h2 { "Modmail" }
                        (figures(&stats, true))
                    }
                    div.rail-block {
                        h2 { "Opened, last 14 days" span.muted { (stats.opened_week) " this week" } }
                        (ui::area_chart(&opened_per_day(&ctx, &page, guild, now, 14), "opened"))
                    }
                    (where_they_open(&ctx, guild, &s, &base))
                }
            }
        } @else {
            // The answer first: how many are open, the two weeks they came from, where they
            // open and who closed the last ones; then the open conversations themselves.
            @let days = opened_per_day(&ctx, &page, guild, now, 28);
            @let (before, recent) = days.split_at(14);
            section.mm-panel.mm-lead {
                div.mm-lead-main {
                    div.mm-lead-head {
                        div.mm-big {
                            b { (stats.open) }
                            span { @if stats.open == 1 { "conversation open" } @else { "conversations open" } }
                        }
                        (figures(&stats, false))
                    }
                    div.mm-chart {
                        h2 {
                            "Opened per day"
                            span.muted { "last 14 days" @if before.iter().any(|d| d.1 > 0) { ", the 14 before dashed" } }
                        }
                        (ui::area_chart_vs(recent, Some(before), "opened"))
                    }
                }
                div.mm-lead-side {
                    (where_they_open(&ctx, guild, &s, &base))
                    (closed_recently(&ctx, &page, guild))
                }
            }
            section.mm-panel {
                h2 { "Open conversations" @if rows.len() == 100 { span.muted { "the latest 100" } } }
                @if rows.is_empty() {
                    @if s.channel.is_none() {
                        (ui::empty(Icon::Mail, "Modmail is off", "Pick a staff channel in Settings. Conversations show up here when members DM the bot."))
                    } @else {
                        (ui::empty(Icon::Mail, "No open conversations", "Conversations show up here when members DM the bot."))
                    }
                } @else {
                    div.mm-list { @for c in &rows { (conversation(&ctx, &page, guild, c, now)) } }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Modmail", None, body).into_response())
}

/// Conversations opened on each of the last `days` days in the viewer's time, oldest first.
fn opened_per_day(ctx: &Ctx, page: &Page, guild: u64, now: i64, days: i64) -> Vec<(String, i64)> {
    let opened: Vec<i64> = ctx
        .db
        .with(|c| {
            let mut st = c.prepare_cached("SELECT opened_at FROM modmail WHERE guild_id = ?1 AND opened_at > ?2")?;
            st.query_map(bot_core::rusqlite::params![guild as i64, now - (days + 1) * 86_400_000], |r| r.get(0))?.collect()
        })
        .unwrap_or_default();
    let today = page.user.local(now).date();
    (0..days)
        .rev()
        .filter_map(|back| today.checked_sub(bot_core::jiff::Span::new().days(back)).ok())
        .map(|day| {
            let n = opened.iter().filter(|at| page.user.local(**at).date() == day).count() as i64;
            (day.strftime("%b %-d").to_string(), n)
        })
        .collect()
}

/// The figures beside the lead number, or in a side block with the open count too.
fn figures(stats: &mail::Stats, with_open: bool) -> Markup {
    html! {
        div.mm-figs {
            @if with_open { div { b { (stats.open) } span { "open now" } } }
            div.hot[stats.waiting > 0] {
                b { (stats.waiting) }
                span { @if stats.waiting > 0 { span.dot.waiting {} } "waiting on staff" }
            }
            div { b { (stats.opened_week) } span { "opened this week" } }
            div { b { (stats.closed_week) } span { "closed this week" } }
        }
    }
}

/// The staff channel and the role pinged, or where to pick them.
fn where_they_open(ctx: &Ctx, guild: u64, s: &Settings, base: &str) -> Markup {
    html! {
        div.rail-block {
            h2 { "Where they open" }
            div.mm-line {
                span.mm-ic { (ui::icon(Icon::Hash)) }
                div.grow {
                    @match s.channel {
                        Some(ch) => { strong { (channel_name(ctx, guild, ch)) } span { "A thread for each conversation" } },
                        None => { strong { "No staff channel" } span { a href={ (base) "?view=settings" } { "Pick one in Settings" } } },
                    }
                }
            }
            @if let Some(role) = s.staff_role {
                @if let Some((_, name, _)) = ctx.roles(guild).into_iter().find(|(id, _, _)| *id == role) {
                    div.mm-line {
                        span.mm-ic { (ui::icon(Icon::Bell)) }
                        div.grow { strong { "@" (name) } span { "Pinged when one opens" } }
                    }
                }
            }
        }
    }
}

/// One conversation as a row: the member's face and name, their latest message (or staff's,
/// with the staff face), where it stands, and the buttons, in columns that line up down the
/// list.
fn conversation(ctx: &Ctx, page: &Page, guild: u64, c: &Conv, now: i64) -> Markup {
    let name = c.name(ctx, guild);
    let staff_wrote = !c.waiting_on_staff();
    let stamp = |ms: i64| page.user.local(ms).strftime("%b %-d, %H:%M").to_string();
    let earlier = (c.messages - 1).max(0);
    let waiting = c.is_open() && c.waiting_on_staff();
    html! {
        article.mm-row {
            div.mm-face {
                img.avatar src=(ctx.member_avatar(guild, c.user_id)) alt="" width="28" height="28" loading="lazy";
                @if c.is_open() { span class={ "mm-turn " (if waiting { "staff" } else { "member" }) } {} }
            }
            div.mm-body {
                strong.clip { (name) }
                span {
                    span title=(stamp(c.opened_at)) { "Opened " (text::ago(c.opened_at)) }
                    @if earlier > 0 { " · " (earlier) @if earlier == 1 { " earlier message" } @else { " earlier messages" } }
                }
            }
            div.mm-convo {
                @if !c.preview.is_empty() {
                    @if staff_wrote {
                        div.mm-reply {
                            img.avatar src=(ctx.member_avatar(guild, c.last_author)) alt="" width="20" height="20" loading="lazy";
                            p {
                                strong { (ctx.display_name(guild, c.last_author)) } " " (c.preview)
                                " " time.mm-time datetime=(c.last_at) title=(stamp(c.last_at)) { (text::ago(c.last_at)) }
                            }
                        }
                    } @else {
                        p.mm-said {
                            (c.preview)
                            " " time.mm-time datetime=(c.last_at) title=(stamp(c.last_at)) { (text::ago(c.last_at)) }
                        }
                    }
                }
                @if !c.is_open() {
                    div.mm-reply {
                        @if let Some(by) = c.closed_by {
                            img.avatar src=(ctx.member_avatar(guild, by)) alt="" width="20" height="20" loading="lazy";
                        }
                        p {
                            @match c.closed_by {
                                Some(by) => { "Closed by " strong { (ctx.display_name(guild, by)) } },
                                None => { "Closed in Discord" },
                            }
                            @if let Some(at) = c.closed_at { " " time datetime=(at) title=(stamp(at)) { (text::ago(at)) } }
                        }
                    }
                }
            }
            div.mm-state {
                @if waiting {
                    span.badge.warn title={ "Waiting " (mail::span((now - c.last_at) / 1000)) ", since " (stamp(c.last_at)) } {
                        "Waiting " (short_wait((now - c.last_at) / 1000))
                    }
                } @else if c.is_open() {
                    span.badge { "Waiting on " (name) }
                } @else {
                    span.badge { "Closed" }
                }
            }
            div.mm-acts {
                a.btn.small href=(thread_url(guild, c.thread_id)) target="_blank" rel="noopener" { (ui::icon(Icon::External)) "Open thread" }
                @if page.can_edit() && c.is_open() { (ui::action_button(&page.path(&format!("modmail/{}/close", c.id)), "Close", Tone::Bad, true)) }
            }
        }
    }
}

/// How long a member has waited, in its largest unit ("26 h", "3 days"). The exact span is in
/// the tooltip.
fn short_wait(secs: i64) -> String {
    match secs.max(0) {
        s if s >= 2 * 86_400 => format!("{} days", s / 86_400),
        s if s >= 3600 => format!("{} h", s / 3600),
        s => format!("{} min", (s / 60).max(1)),
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
                div.mm-line {
                    span.mm-pair {
                        img.avatar src=(ctx.member_avatar(guild, c.user_id)) alt="" width="28" height="28" loading="lazy";
                        @if let Some(by) = c.closed_by { img.avatar.mm-by-face src=(ctx.member_avatar(guild, by)) alt="" width="16" height="16" loading="lazy"; }
                    }
                    div.grow {
                        strong { (c.name(ctx, guild)) }
                        span {
                            @match c.closed_by {
                                Some(by) => { "by " (ctx.display_name(guild, by)) },
                                None => { "in Discord" },
                            }
                        }
                    }
                    @if let Some(at) = c.closed_at { time.mm-ago datetime=(at) title=(page.user.local(at).strftime("%b %-d, %H:%M").to_string()) { (text::ago(at)) } }
                }
            }
        }
    }
}

/// The greeting and the closing text as the member gets them: DMs from the bot, as saved.
fn dm_preview(ctx: &Ctx, page: &Page, guild: u64, s: &Settings) -> Markup {
    let [(_, greeting), (_, closing)] = mail::member_texts(s, &ctx.guild_name(guild));
    let (bot, _) = dc::bot_identity(ctx);
    let line = |label: &str, body: String| dc::message(ctx, page).time_text(label).render(dc::line(html! { (body) }));
    html! {
        div.mm-pv-body {
            p.muted.small { "DMs from the bot, as saved. Save to see a change here." }
            div.mm-dms {
                div.mm-dms-head { span.mm-at { "@" } strong { (bot) } }
                (line("when it opens", greeting))
                div.mm-dms-gap { span { "Staff answer in the thread, and their replies arrive here" } }
                (line("when staff close it", closing))
            }
            @if page.can_edit() {
                div.mm-send {
                    form.inline method="post" action=(page.path("modmail/test")) {
                        button.btn.small type="submit" { (ui::icon(Icon::Mail)) "Send to my DMs" }
                    }
                    span.muted.small { "Both texts, as saved, in your own DMs from the bot." }
                }
            } @else {
                p.muted.small { "Ask an admin to change these texts." }
            }
        }
    }
}

/// The thread a conversation opens in, as staff see it: the ping and the bot's note, as saved,
/// with the viewer standing in for the member.
fn staff_preview(ctx: &Ctx, page: &Page, guild: u64, s: &Settings) -> Markup {
    let who = &page.user.name;
    let role = s.staff_role.and_then(|r| ctx.roles(guild).into_iter().find(|(id, _, _)| *id == r));
    let channel = s.channel.and_then(|c| ctx.text_channels(guild).into_iter().find(|(id, _)| *id == c)).map(|(_, n)| n);
    html! {
        div.mm-pv-body {
            p.muted.small { "The thread each conversation opens, as if you wrote in." }
            @match channel {
                None => { (ui::empty(Icon::Hash, "No staff channel", "Pick one and each conversation opens a thread there.")) },
                Some(ch) => {
                    div.mm-dms.mm-staff {
                        div.mm-dms-head {
                            span.mm-at { "#" } span.mm-crumb { (ch) }
                            (ui::icon(Icon::Chevron))
                            strong.clip { (who) }
                        }
                        (dc::message(ctx, page).time_text("").render(html! {
                            div.dp-line {
                                @if let Some((_, name, color)) = &role {
                                    span.dp-ping.mm-role style=[(*color != 0).then(|| format!("--role:#{color:06x}"))] { "@" (name) } ", new modmail from "
                                } @else {
                                    "New modmail from "
                                }
                                span.dp-ping { "@" (who) } "."
                            }
                            (dc::embed(&format!("#{:06x}", mail::COLOR), html! {
                                strong.dp-title { (who) }
                                div.dp-desc { (mail::THREAD_INTRO) }
                            }))
                            (dc::buttons(dc::button("Close", dc::Style::Grey)))
                        }))
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

/// The greeting and the closing text in the viewer's own DMs, as saved.
async fn send_test(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    mail::send_test(&ctx, guild, &s, page.user.id).await.map_err(WebError::Bad)?;
    ctx.audit(&page.user, Some(guild), KEY, "Sent the modmail greeting and closing text to their own DMs as a test");
    Ok(Redirect::to(&format!("{}&sent=1", page.path("modmail?view=settings"))).into_response())
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
/* ---- Modmail in the house style (2026-11-02), the same shapes as Tickets ----
   Every section and side block is one kind of bordered panel; never a panel inside a panel.
   The Open tab leads with the open count and its chart; Closed and Settings get the same
   panels without it. Flat: no gradients, glows, pings or entrances of their own. */
.mm-panel { min-width: 0; display: flex; flex-direction: column; gap: 14px; background: var(--raised); border: 1px solid var(--edge-1); border-radius: 14px; padding: 18px 20px; }
.mm-panel h2, .mm-rail .rail-block > h2, .mm-compose h2 { display: flex; align-items: baseline; justify-content: space-between; gap: 8px; font-size: 14px; font-weight: 500; }
.mm-panel h2 .muted, .mm-rail h2 .muted { font-size: 12px; font-weight: 400; }
.mm-panel .empty, .mm-rail .empty { background: none; box-shadow: none; padding: 6px 0; }
.mm-panel .empty-icon, .mm-rail .empty-icon { animation: none; background: var(--control); color: var(--ink-2); box-shadow: none; }
.mm-alert { padding: 12px 20px; }
.mm-alert .need { margin: 0; padding: 0; }
.mm-alert .need:hover { background: none; }
.mm-alert .need .grow > span { font-size: 13px; color: var(--ink-3); }

/* Two columns: the main one and a right column of one fixed width, each side block a panel. */
.with-rail.mm-cols { grid-template-columns: minmax(0, 1fr) 400px; gap: 16px; }
.mm-rail { min-width: 0; background: none; box-shadow: none; padding: 0; gap: 16px; border-radius: 0; }
.mm-rail > .rail-block { background: var(--raised); border: 1px solid var(--edge-1); border-radius: 14px; padding: 18px 20px; }
.mm-rail > .rail-block + .rail-block { margin-top: 0; padding-top: 18px; border-top: 1px solid var(--edge-1); }
.mm-rail .area svg { height: 80px; }
/* Only the lead chart draws in; a side chart is just there. */
.mm-rail .area .line, .mm-rail .area .fill, .mm-rail .area-end { animation: none; }
@media (max-width: 1180px) { .with-rail.mm-cols { grid-template-columns: minmax(0, 1fr) 320px; } }
@media (max-width: 920px) { .with-rail.mm-cols { grid-template-columns: minmax(0, 1fr); } }

/* The lead: one panel, the answer on the left, where they open and the last closed past a
   hairline on the right. */
.mm-lead { display: grid; grid-template-columns: minmax(0, 1fr) 400px; gap: 0; padding: 0; }
.mm-lead-main { min-width: 0; display: flex; flex-direction: column; gap: 22px; padding: 20px 22px 18px; }
.mm-lead-side { min-width: 0; display: flex; flex-direction: column; padding: 18px 20px; border-left: 1px solid var(--edge-1); }
.mm-lead-head { display: flex; align-items: flex-end; justify-content: space-between; flex-wrap: wrap; gap: 16px 40px; }
.mm-big { display: flex; align-items: baseline; gap: 12px; }
.mm-big b { font: 600 48px/1 var(--sans); letter-spacing: -0.03em; font-variant-numeric: tabular-nums; }
.mm-big span { font-size: 14px; color: var(--ink-2); }
.mm-chart { display: flex; flex-direction: column; gap: 12px; }
.mm-lead .area svg { height: 170px; }
@container col (max-width: 1100px) {
  .mm-lead { grid-template-columns: minmax(0, 1fr); }
  .mm-lead-side { border-left: 0; border-top: 1px solid var(--edge-1); }
}

/* Figures: in a row beside the lead number, two across in a side block. */
.mm-figs { display: flex; flex-wrap: wrap; gap: 12px 36px; }
.mm-figs > div { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.mm-figs b { font: 600 20px/1.2 var(--sans); letter-spacing: -0.01em; white-space: nowrap; font-variant-numeric: tabular-nums; }
.mm-figs span { display: inline-flex; align-items: center; gap: 6px; font-size: 12px; color: var(--ink-3); white-space: nowrap; }
.mm-figs .hot b { color: var(--waiting); }
.mm-figs .dot { width: 6px; height: 6px; }
.mm-rail .mm-figs { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 14px 16px; }

/* Side lines: a 28 px face or icon, a 14 px name, a 13 px muted line. */
.mm-line { display: flex; align-items: center; gap: 12px; min-width: 0; }
.mm-line .grow { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.mm-line strong { font-size: 14px; font-weight: 500; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.mm-line .grow > span { font-size: 13px; color: var(--ink-3); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.mm-ic { display: grid; place-items: center; width: 28px; height: 28px; flex: none; border-radius: 8px; background: var(--control); color: var(--ink-2); }
.mm-ic .icon { width: 15px; height: 15px; }
.mm-pair { position: relative; flex: none; width: 28px; height: 28px; }
.mm-pair > .avatar:first-child { width: 28px; height: 28px; }
.mm-by-face { position: absolute; right: -4px; bottom: -3px; width: 16px; height: 16px; box-shadow: 0 0 0 2px var(--raised); }
.mm-ago { flex: none; margin-left: auto; font-size: 12px; color: var(--ink-3); }
.rail-block h2 a { font-weight: 400; }

/* ---- a conversation, as a row (the same columns as a ticket) ---- */
.mm-list { display: flex; flex-direction: column; }
.mm-row { display: grid; grid-template-columns: 28px minmax(0, 1fr) auto; grid-template-areas: "face body state" "face convo convo" "face acts acts"; gap: 8px 14px; align-items: start; padding: 14px 0; }
.mm-row + .mm-row { border-top: 1px solid var(--edge-1); }
.mm-row:first-child { padding-top: 2px; }
.mm-row:last-child { padding-bottom: 0; }
.mm-face { grid-area: face; position: relative; width: 28px; height: 28px; }
.mm-face .avatar { width: 28px; height: 28px; }
.mm-turn { position: absolute; right: -2px; bottom: -2px; width: 10px; height: 10px; border-radius: 50%; box-shadow: 0 0 0 2px var(--raised); }
.mm-turn.staff { background: var(--waiting); }
.mm-turn.member { background: var(--live); }
.mm-body { grid-area: body; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.mm-body strong { font-size: 14px; font-weight: 500; }
.mm-body > span { font-size: 13px; color: var(--ink-3); }
.mm-convo { grid-area: convo; display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.mm-convo:empty { display: none; }
.mm-said { align-self: flex-start; max-width: 100%; padding: 6px 11px; border-radius: 4px 12px 12px 12px; background: var(--well); color: var(--ink-2); font-size: 13px; line-height: 1.5; overflow-wrap: anywhere; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
.mm-reply { display: flex; align-items: flex-start; gap: 8px; min-width: 0; font-size: 13px; line-height: 20px; color: var(--ink-3); }
.mm-reply .avatar { width: 20px; height: 20px; flex: none; }
.mm-reply p { min-width: 0; overflow-wrap: anywhere; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
.mm-reply strong { color: var(--ink-2); font-weight: 500; }
.mm-time { font-size: 12px; color: var(--ink-3); white-space: nowrap; }
.mm-said .mm-time { margin-left: 4px; }
.mm-state { grid-area: state; display: flex; }
.mm-acts { grid-area: acts; display: grid; grid-template-columns: 112px 112px; gap: 6px; }
.mm-acts form { display: flex; }
.mm-acts .btn { width: 100%; }
.mm-acts .btn .icon { width: 14px; height: 14px; }
@container col (min-width: 700px) {
  .mm-row { grid-template-columns: 28px minmax(0, 1fr) auto auto; grid-template-areas: "face body state acts" "face convo convo convo"; column-gap: 20px; }
  .mm-state { padding-top: 3px; }
}
@container col (min-width: 1250px) {
  .mm-row { grid-template-columns: 28px minmax(0, 0.8fr) minmax(0, 1.4fr) 140px auto; grid-template-areas: "face body convo state acts"; column-gap: 24px; }
}

/* ---- settings beside the DMs they become ----
   The form is the panel with its Save inside; the previews take a fixed column, one at a time,
   Discord at three quarters, with the side blocks under them. */
.compose.mm-compose { gap: 16px; }
@container col (min-width: 1000px) {
  .compose.mm-compose:has(> .compose-preview) { grid-template-columns: minmax(0, 1fr) 500px; row-gap: 0; }
}
@container col (min-width: 1000px) and (max-width: 1200px) {
  .compose.mm-compose:has(> .compose-preview) { grid-template-columns: minmax(0, 1fr) 440px; }
}
.mm-compose > form.settings { background: var(--raised); border: 1px solid var(--edge-1); border-radius: 14px; padding: 18px 20px; display: flex; flex-direction: column; gap: 6px; }
.mm-compose > form.settings fieldset { display: flex; flex-direction: column; gap: 18px; }
.mm-compose > form.settings .block { gap: 4px; }
.mm-compose > form.settings .block + .block { padding-top: 18px; border-top: 1px solid var(--edge-1); }
.mm-compose > form.settings .card { background: none; border: 0; padding: 0; border-radius: 0; }
.mm-compose > form.settings .save-bar { padding-top: 8px; }
.mm-compose > form.settings textarea { min-height: 84px; }
.mm-compose > .compose-preview.mm-rail { position: static; }
.mm-previews { gap: 12px; }
.mm-pv-head { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.compose-preview:has(input[value="staff"]:checked) .mm-pv-member, .compose-preview:has(input[value="member"]:checked) .mm-pv-staff { display: none; }
.mm-pv-body { display: flex; flex-direction: column; gap: 12px; }

/* The member's DMs from the bot, on Discord's own colours, at three quarters. */
.mm-dms { zoom: 0.75; display: flex; flex-direction: column; gap: 12px; padding: 0 0 14px; border-radius: 12px; background: #313338; overflow: hidden; }
.mm-dms-head { display: flex; align-items: center; gap: 8px; height: 38px; padding: 0 14px; background: #2b2d31; border-bottom: 1px solid rgba(0, 0, 0, 0.28); color: #f2f3f5; font-size: 13px; }
.mm-at { color: #80848e; font-weight: 600; }
.mm-dms > .dp-msg { padding: 0 14px; }
.mm-dms .dp-post { gap: 11px; font-size: 13.5px; }
.mm-dms-gap { display: flex; align-items: center; gap: 10px; padding: 0 14px; color: #949ba4; font-size: 11px; }
.mm-dms-gap::before, .mm-dms-gap::after { content: ""; flex: 1; height: 1px; background: rgba(255, 255, 255, 0.08); }
.mm-dms-gap span { text-align: center; max-width: 70%; }
.mm-send { display: flex; align-items: center; flex-wrap: wrap; gap: 8px 12px; }
/* The staff thread: its channel and name, Discord's mentions, the bot's note and its button. */
.mm-dms-head .icon { width: 13px; height: 13px; color: #80848e; transform: rotate(-90deg); }
.mm-crumb { color: #b5bac1; font-weight: 600; }
.dp-ping.mm-role { color: var(--role, #c9cdfb); background: color-mix(in oklab, var(--role, #5865f2) 10%, transparent); }
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
