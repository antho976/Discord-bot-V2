use std::collections::HashMap;

use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::{IntoResponse, Redirect};
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::rusqlite::{Connection, params};
use bot_core::twilight_model::channel::ChannelType;
use bot_core::twilight_model::channel::message::AllowedMentions;
use bot_core::twilight_model::id::Id;
use bot_core::web::dc;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};
use serde::Deserialize;

use crate::{DEFAULT_THREAD_NAME, KEY, MAX_CHANNELS, Rule, Settings, sticky};

const MIN_MS: i64 = 60_000;
/// The longest auto-purge limit, in minutes: 90 days.
const MAX_PURGE_MINS: i64 = 90 * 24 * 60;

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/rules", get(list_page))
        .route("/g/{guild}/rules/edit", get(edit_page).post(save))
        .route("/g/{guild}/rules/{channel}/remove", post(remove))
        .route("/g/{guild}/rules/{channel}/test-notice", post(test_notice))
        .route("/g/{guild}/rules/{channel}/repost", post(repost))
}

#[derive(Deserialize)]
struct ListQuery {
    /// After a test: which one went out, for the toast.
    sent: Option<String>,
}

/// Channels that can take rules: text and announcement channels. Forums hold posts, not
/// messages, so sticky and media-only rules have nothing to act on there.
fn rule_channels(ctx: &Ctx, guild: u64) -> Vec<(u64, String)> {
    ctx.text_channels(guild).into_iter().filter(|(id, _)| ctx.cache.channel(Id::new(*id)).is_some_and(|c| c.kind != ChannelType::GuildForum)).collect()
}

fn channel_name(ctx: &Ctx, guild: u64, channel: u64) -> String {
    ctx.text_channels(guild).into_iter().find(|(id, _)| *id == channel).map(|(_, n)| format!("#{n}")).unwrap_or_else(|| "a deleted channel".into())
}

/// "7 days", "12 h", "1 day 6 h", "30 min" for a limit in minutes.
pub fn limit_words(mins: i64) -> String {
    let (d, h, m) = (mins / 1440, (mins % 1440) / 60, mins % 60);
    let mut parts = Vec::new();
    if d > 0 {
        parts.push(if d == 1 { "1 day".to_owned() } else { format!("{d} days") });
    }
    if h > 0 {
        parts.push(format!("{h} h"));
    }
    if m > 0 {
        parts.push(format!("{m} min"));
    }
    parts.join(" ")
}

/// A limit in minutes as it's typed: "7d", "1d 12h", "30m". Empty for off.
pub fn limit_input(mins: i64) -> String {
    if mins <= 0 {
        return String::new();
    }
    let (d, h, m) = (mins / 1440, (mins % 1440) / 60, mins % 60);
    [(d, "d"), (h, "h"), (m, "m")].iter().filter(|(n, _)| *n > 0).map(|(n, u)| format!("{n}{u}")).collect::<Vec<_>>().join(" ")
}

/// What was typed for the auto-purge limit, in minutes. Empty is off.
pub fn parse_limit(raw: &str) -> Result<i64, String> {
    if raw.trim().is_empty() || raw.trim() == "0" {
        return Ok(0);
    }
    let bad = || "Type how old messages may get before they go, like 30m, 12h or 7d, between 10 minutes and 90 days.".to_owned();
    let secs = text::parse_duration(raw).ok_or_else(bad)?;
    let mins = secs / 60;
    if (10..=MAX_PURGE_MINS).contains(&mins) { Ok(mins) } else { Err(bad()) }
}

/// The four counters on the list page: daily_stats key, reading label, unit in a bar's tooltip,
/// and what the bars show, for screen readers.
const STATS: [(&str, &str, &str, &str); 4] = [
    ("rules_media", "removed, no media", "removed", "Messages removed for having no media, each of the last 7 days"),
    ("rules_purged", "purged", "purged", "Messages purged for their age, each of the last 7 days"),
    ("rules_sticky", "sticky reposts", "reposts", "Sticky messages reposted, each of the last 7 days"),
    ("rules_threads", "threads started", "threads", "Threads started, each of the last 7 days"),
];

/// The last 7 days, oldest first, in UTC like the counters.
fn week_days() -> Vec<bot_core::jiff::civil::Date> {
    let today = bot_core::jiff::Timestamp::now().to_zoned(bot_core::jiff::tz::TimeZone::UTC).date();
    (0..7i64).rev().filter_map(|back| today.checked_sub(bot_core::jiff::Span::new().days(back)).ok()).collect()
}

/// The feature's daily counters over the last 7 days, by key and then by day.
fn week_counts(conn: &Connection, guild: u64, since: &str) -> bot_core::rusqlite::Result<HashMap<String, HashMap<String, i64>>> {
    let mut stmt = conn.prepare(
        "SELECT key, day, SUM(value) FROM daily_stats WHERE guild_id = ?1 AND day >= ?2
           AND key IN ('rules_media', 'rules_purged', 'rules_sticky', 'rules_threads') GROUP BY key, day",
    )?;
    let mut out: HashMap<String, HashMap<String, i64>> = HashMap::new();
    for row in stmt.query_map(params![guild as i64, since], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))? {
        let (key, day, n) = row?;
        out.entry(key).or_default().insert(day, n);
    }
    Ok(out)
}

/// When each sticky message in the server was last posted, by channel.
fn sticky_times(conn: &Connection, guild: u64) -> bot_core::rusqlite::Result<HashMap<u64, i64>> {
    let mut stmt = conn.prepare("SELECT channel_id, posted_at FROM rules_sticky WHERE guild_id = ?1")?;
    stmt.query_map([guild as i64], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?)))?.collect()
}

/// The four things a channel can do, in the order the cards and the edit form show them: the
/// three switches, then the sticky message.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Media,
    Purge,
    Sticky,
    Thread,
}

impl Kind {
    const ALL: [Kind; 4] = [Kind::Media, Kind::Purge, Kind::Thread, Kind::Sticky];

    fn key(self) -> &'static str {
        match self {
            Kind::Media => "media",
            Kind::Purge => "purge",
            Kind::Sticky => "sticky",
            Kind::Thread => "thread",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Kind::Media => "Media only",
            Kind::Purge => "Auto-purge",
            Kind::Sticky => "Sticky",
            Kind::Thread => "Auto-thread",
        }
    }

    /// Whether the rule is on, and the few words a tile shows under its name.
    fn state(self, r: &Rule) -> (bool, String) {
        match self {
            Kind::Media if r.media_only => (true, if r.media_notice { "With a notice".into() } else { "Silently".into() }),
            Kind::Purge if r.purge_after_mins > 0 => (true, limit_words(r.purge_after_mins)),
            Kind::Sticky if r.has_sticky() => (true, format!("Every {}", r.sticky_every)),
            Kind::Thread if r.thread => (true, "Every message".into()),
            _ => (false, "Off".into()),
        }
    }
}

/// Each rule's icon, drawn on the dashboard's 24-unit grid at its stroke weight.
fn rule_icon(k: Kind) -> Markup {
    let body = match k {
        // A picture.
        Kind::Media => {
            "M5.5 4h13A1.5 1.5 0 0 1 20 5.5v13a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 18.5v-13A1.5 1.5 0 0 1 5.5 4ZM4 16l4.5-4.5a1.5 1.5 0 0 1 2.1 0L16 17M14 14.5l1.4-1.4a1.5 1.5 0 0 1 2.1 0L20 15.5M14 8.75a1.25 1.25 0 1 0 2.5 0 1.25 1.25 0 1 0-2.5 0"
        }
        // A clock sweeping into a bin lid: old messages go.
        Kind::Purge => "M4.5 7h15M9.5 7V4.5h5V7M6.5 7l.9 12a1.5 1.5 0 0 0 1.5 1.4h6.2a1.5 1.5 0 0 0 1.5-1.4l.9-12M12 11v3.5l2 1.25",
        // A pushpin.
        Kind::Sticky => "M9 3.5h6M10 3.5v5.25L7 12.5v1.5h10v-1.5l-3-3.75V3.5M12 14v6.5",
        // Two speech bubbles: a message and the thread under it.
        Kind::Thread => "M5 4.5h9.5A1.5 1.5 0 0 1 16 6v5.5a1.5 1.5 0 0 1-1.5 1.5H9.5L6 16v-3H5a1.5 1.5 0 0 1-1.5-1.5V6A1.5 1.5 0 0 1 5 4.5ZM19 9.5a1.5 1.5 0 0 1 1.5 1.5v5.5A1.5 1.5 0 0 1 19 18h-1v3l-3.5-3H11",
    };
    html! {
        svg.icon viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" {
            path d=(body) {}
        }
    }
}

/// The four rule tiles: lit in the page's colour when the rule is on, with its short value.
/// `card` staggers the lighting across the grid.
fn tiles(r: &Rule, card: usize) -> Markup {
    html! {
        div.ru-tiles style={ "--c:" (card) } {
            @for (i, k) in Kind::ALL.iter().enumerate() {
                @let (on, value) = k.state(r);
                div.ru-tile.on[on] data-rule=(k.key()) style={ "--i:" (i) } {
                    span.ru-ti { (rule_icon(*k)) }
                    span.ru-tt {
                        span.ru-tl { (k.label()) }
                        span.ru-tv { (value) }
                    }
                }
            }
        }
    }
}

/// The sticky message: a message from the bot on Discord's grey, posted at `time`. `id` names the
/// text so the edit page's script can rewrite it in place.
fn bot_message(ctx: &Ctx, page: &Page, time: String, text: &str, id: Option<&str>) -> Markup {
    html! { div.ru-msg { (dc::message(ctx, page).time_text(time).render(html! { div.dp-line id=[id] { (text.trim()) } })) } }
}

/// "After 25 messages or 10 min of quiet".
fn sticky_when(r: &Rule) -> String {
    let count = format!("After {} {}", r.sticky_every, if r.sticky_every == 1 { "message" } else { "messages" });
    if r.sticky_quiet_mins > 0 { format!("{count} or {} min of quiet", r.sticky_quiet_mins) } else { count }
}

fn thread_template(r: &Rule) -> &str {
    if r.thread_name.trim().is_empty() { DEFAULT_THREAD_NAME } else { r.thread_name.trim() }
}

async fn list_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<ListQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let days = week_days();
    let since = days.first().map(|d| d.to_string()).unwrap_or_default();
    let week = ctx.db.with(|c| week_counts(c, guild, &since))?;
    let posted = ctx.db.with(|c| sticky_times(c, guild))?;
    let actions = (page.can_edit() && s.channels.len() < MAX_CHANNELS).then(|| {
        html! { a.btn.primary href=(page.path("rules/edit")) { (ui::icon(Icon::Plus)) "Add a channel" } }
    });
    let toast = match q.sent.as_deref() {
        Some("notice") => Some("Test notice sent"),
        Some("sticky") => Some("Sticky message reposted"),
        _ => None,
    };
    let body = html! {
        style { (PreEscaped(CSS)) }
        @if let Some(toast) = toast { (ui::sent_toast(true, toast)) }
        div.ru-week {
            div.ru-stats {
                @for (key, label, unit, about) in STATS {
                    @let by_day = week.get(key);
                    @let counts: Vec<i64> = days.iter().map(|d| by_day.and_then(|m| m.get(&d.to_string())).copied().unwrap_or(0)).collect();
                    @let total: i64 = counts.iter().sum();
                    @let top = counts.iter().copied().max().unwrap_or(0).max(1);
                    div.ru-stat {
                        div.ru-stat-head {
                            b data-count { (text::num(total)) }
                            span { (label) }
                        }
                        div.ru-bars role="img" aria-label={ (about) ": " (counts.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(", ")) } {
                            @for (i, (d, n)) in days.iter().zip(&counts).enumerate() {
                                @let today = i + 1 == days.len();
                                @let when = if today { "Today".to_owned() } else { d.strftime("%a %b %-d").to_string() };
                                span.ru-col data-tip={ (when) ": " (text::num(*n)) " " (unit) } {
                                    span.ru-bar.today[today].zero[*n == 0] style={ "--v:" (format!("{:.1}", *n as f64 / top as f64 * 100.0)) ";--i:" (i) } {}
                                    span.ru-day.today[today] { (d.strftime("%a").to_string().chars().next().unwrap_or(' ')) }
                                }
                            }
                        }
                    }
                }
            }
            p.muted.small { "Last 7 days, across " (s.channels.len()) @if s.channels.len() == 1 { " channel." } @else { " channels." } }
        }
        @if s.channels.is_empty() {
            (ui::empty(Icon::Rules, "No channel rules yet", "Add a channel to keep it to media, clear out old messages, keep a message at the bottom, or start a thread on every message."))
        } @else {
            div.ru-grid {
                @for (i, r) in s.channels.iter().enumerate() { (rule_card(&ctx, &page, guild, r, posted.get(&r.channel).copied(), i)) }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Channel rules", actions, body).into_response())
}

fn rule_card(ctx: &Ctx, page: &Page, guild: u64, r: &Rule, posted: Option<i64>, i: usize) -> Markup {
    let on = Kind::ALL.iter().filter(|k| k.state(r).0).count();
    html! {
        article.ru-card {
            header.ru-head {
                div.ru-name {
                    strong { (channel_name(ctx, guild, r.channel)) }
                    span { (on) @if on == 1 { " rule on" } @else { " rules on" } }
                }
                @if page.can_edit() {
                    div.ru-acts {
                        a.btn.small href={ (page.path("rules/edit")) "?channel=" (r.channel) } { "Edit" }
                        (ui::action_button(&page.path(&format!("rules/{}/remove", r.channel)), "Remove", Tone::Bad, true))
                    }
                }
            }
            (tiles(r, i))
            @if r.media_only || r.purge_after_mins > 0 || r.thread {
                div.ru-notes {
                    @if r.media_only {
                        div.ru-note-row {
                            p.ru-note { "Removes messages without a file or link" @if r.media_notice { ", with a short notice" } "." }
                            @if r.media_notice && page.can_edit() {
                                form.inline method="post" action=(page.path(&format!("rules/{}/test-notice", r.channel))) {
                                    button.btn.small type="submit" data-tip="Posts the notice in this channel, marked as a test" { "Send a test" }
                                }
                            }
                        }
                    }
                    @if r.purge_after_mins > 0 {
                        p.ru-note { "Deletes messages older than " (limit_words(r.purge_after_mins)) ". Pinned ones stay." }
                    }
                    @if r.thread {
                        p.ru-note { "A thread on every message, named " code { (thread_template(r)) } }
                    }
                }
            }
            @if r.has_sticky() {
                div.ru-sticky {
                    (bot_message(ctx, page, posted.map(|at| dc::time(page, at)).unwrap_or_else(|| "Not posted yet".into()), &text::clip(r.sticky.trim(), 300), None))
                    div.ru-sticky-foot {
                        p.ru-sticky-cap {
                            (rule_icon(Kind::Sticky))
                            span {
                                (sticky_when(r))
                                @if let Some(at) = posted {
                                    " · " span data-tip={ "Reposted: " (page.user.local(at).strftime("%b %-d at %-I:%M %p")) } { "reposted " (text::ago(at)) }
                                }
                            }
                        }
                        @if page.can_edit() {
                            form.inline method="post" action=(page.path(&format!("rules/{}/repost", r.channel))) {
                                button.btn.small type="submit" data-tip="Posts it at the bottom now and deletes the copy above" { "Repost now" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct EditQuery {
    channel: Option<u64>,
}

/// A rule section's head: its icon tile, title and one line, and the switch that turns it on
/// when it has one.
fn sec_head(ic: Markup, title: &str, line: &str, switch: Option<(&str, &str, bool)>) -> Markup {
    html! {
        div.ru-sec-head {
            span.ru-sec-ic { (ic) }
            div.grow {
                @match switch {
                    Some((id, ..)) => h2 { label for=(id) { (title) } },
                    None => h2 { (title) },
                }
                p.help { (line) }
            }
            @if let Some((id, aria, on)) = switch {
                label.switch.ru-sec-ctl {
                    input type="checkbox" id=(id) name=(id) checked[on] aria-label=(aria);
                    span.track { span.thumb {} }
                }
            }
        }
    }
}

async fn edit_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<EditQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let existing = q.channel.and_then(|c| s.rule(c).cloned());
    if q.channel.is_some() && existing.is_none() {
        return Err(WebError::NotFound);
    }
    let r = existing.clone().unwrap_or_default();
    let mut channels = vec![(String::new(), "Pick a channel".to_owned())];
    channels.extend(
        rule_channels(&ctx, guild)
            .into_iter()
            .filter(|(id, _)| *id == r.channel || s.rule(*id).is_none())
            .map(|(id, n)| (id.to_string(), format!("#{n}"))),
    );
    let title = if existing.is_some() { "Edit channel rules" } else { "Add a channel" };
    // Sample values for the preview: you as the member, today, a first line someone might post.
    let (sample_user, sample_date, sample_text) = (page.user.name.clone(), bot_core::today(), "My build for the boss fight");
    let shown_name = if r.channel == 0 { "Pick a channel".to_owned() } else { channel_name(&ctx, guild, r.channel) };
    let body = html! {
        style { (PreEscaped(CSS)) }
        a.ru-back href=(page.path("rules")) { (ui::icon(Icon::Chevron)) "All channels" }
        div.with-rail.ru-edit {
            (ui::settings_form(&page, &page.path("rules/edit"), html! {
                @if existing.is_some() { input type="hidden" name="was" value=(r.channel); }
                div.card.ru-sec data-rule="channel" {
                    div.ru-sec-head {
                        span.ru-sec-ic { (ui::icon(Icon::Hash)) }
                        div.grow {
                            h2 { label for="channel" { "Channel" } }
                            p.help { "Each channel has one set of rules. Threads in it aren't affected." }
                        }
                        div.ru-sec-pick { (ui::select("channel", &channels, &if r.channel == 0 { String::new() } else { r.channel.to_string() })) }
                    }
                }
                div.card.ru-sec data-rule="media" {
                    (sec_head(rule_icon(Kind::Media), "Media only", "Removes messages with no attachment and no link. The owner and people with Manage Messages can still talk.", Some(("media_only", "Media only", r.media_only))))
                    div.ru-sec-body {
                        (ui::field_toggle("media_notice", "Say why", Some("Posts a notice that removes itself after 6 seconds, at most once every 10 seconds."), r.media_notice))
                    }
                }
                div.card.ru-sec data-rule="purge" {
                    (sec_head(rule_icon(Kind::Purge), "Auto-purge", "Clears out messages once they reach the age you set.", None))
                    div.ru-sec-body {
                        (ui::field_text("purge", "Delete messages older than", Some("Like 30m, 12h or 7d, up to 90 days. Pinned messages and the sticky message stay. Empty leaves messages alone."), &limit_input(r.purge_after_mins), "Off"))
                    }
                }
                div.card.ru-sec data-rule="thread" {
                    (sec_head(rule_icon(Kind::Thread), "Auto-thread", "Starts a thread on each new message, for replies.", Some(("thread", "Thread on every message", r.thread))))
                    div.ru-sec-body {
                        (ui::field_text("thread_name", "Thread name", Some("{user}, {date} and {text}, the start of the message, are filled in."), &r.thread_name, DEFAULT_THREAD_NAME))
                    }
                }
                div.card.ru-sec data-rule="sticky" {
                    (sec_head(rule_icon(Kind::Sticky), "Sticky message", "Keeps one message from the bot at the bottom of the channel.", None))
                    div.ru-sec-body {
                        (ui::field_textarea("sticky", "Message", Some("Posted again at the bottom of the channel, and the previous copy deleted. Empty turns it off."), &r.sticky, "Read the pinned rules before posting."))
                        (ui::field_number("sticky_every", "Repost after", Some("New messages since the last copy. The bot waits for a 3 second pause, or 30 seconds at most."), r.sticky_every, 1, 100, Some("messages")))
                        (ui::field_number("sticky_quiet_mins", "Or after a quiet spell of", Some("Reposts once the channel has been quiet this long after any new message. 0 waits for the count."), r.sticky_quiet_mins, 0, 120, Some("min")))
                    }
                }
            }))
            aside.rail.ru-side #ru-pv data-user=(sample_user) data-date=(sample_date) data-text=(sample_text) data-default=(DEFAULT_THREAD_NAME) {
                div.rail-block {
                    h2 { "Preview" }
                    div.ru-head {
                        div.ru-name {
                            strong #ru-pv-name { (shown_name) }
                            span { "What the channel does" }
                        }
                    }
                    (tiles(&r, 0))
                    p.ru-note #ru-pv-none hidden[!r.is_empty()] { "Nothing is on yet. Turn on at least one rule to save." }
                    p.ru-note #ru-pv-thread hidden[!r.thread] {
                        "A thread on every message, named " code #ru-pv-tname { (crate::thread_name(&r.thread_name, &sample_user, &sample_date, sample_text)) }
                    }
                    div.ru-sticky #ru-pv-sticky hidden[!r.has_sticky()] {
                        (bot_message(&ctx, &page, dc::time(&page, now_ms()), &r.sticky, Some("ru-pv-stext")))
                        p.ru-sticky-cap { (rule_icon(Kind::Sticky)) span #ru-pv-scap { (sticky_when(&r)) } }
                    }
                    p.muted.small { "Sample values: you as the member, today's date and a first line someone might post." }
                }
            }
        }
        script { (PreEscaped(JS)) }
    };
    Ok(ui::layout(&ctx, &page, title, None, body).into_response())
}

async fn save(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let channel = form.channel(&ctx, guild, "channel").ok_or_else(|| WebError::Bad("Pick a channel.".into()))?;
    if !rule_channels(&ctx, guild).iter().any(|(id, _)| *id == channel) {
        return Err(WebError::Bad("Forums can't take channel rules. Pick a text or announcement channel.".into()));
    }
    let was = form.id("was");
    let mut s = (*ctx.settings::<Settings>(guild, KEY)?).clone();
    if was != Some(channel) && s.rule(channel).is_some() {
        return Err(WebError::Bad(format!("{} already has rules. Edit them instead.", channel_name(&ctx, guild, channel))));
    }
    let old = was.and_then(|w| s.rule(w).cloned());
    if old.is_none() && s.channels.len() >= MAX_CHANNELS {
        return Err(WebError::Bad(format!("You can give rules to {MAX_CHANNELS} channels at most. Remove one first.")));
    }
    let rule = Rule {
        channel,
        media_only: form.bool("media_only"),
        media_notice: form.bool("media_notice"),
        purge_after_mins: parse_limit(&form.string("purge")).map_err(WebError::Bad)?,
        sticky: text::clip(&form.text("sticky"), 1900),
        sticky_every: form.int_in("sticky_every", 1, 100, 10),
        sticky_quiet_mins: form.int_in("sticky_quiet_mins", 0, 120, 0),
        thread: form.bool("thread"),
        thread_name: text::clip(&form.string("thread_name"), 100),
    };
    if rule.is_empty() {
        return Err(WebError::Bad("Turn on at least one rule for this channel, or remove it from the list.".into()));
    }
    match was.and_then(|w| s.channels.iter().position(|r| r.channel == w)) {
        Some(i) => s.channels[i] = rule.clone(),
        None => s.channels.push(rule.clone()),
    }
    ctx.save_settings(guild, KEY, &s)?;
    let name = channel_name(&ctx, guild, channel);
    ctx.audit(&page.user, Some(guild), KEY, if old.is_some() { format!("Changed the rules for {name}") } else { format!("Added rules for {name}") });

    // A new or changed sticky message goes up now; one that moved or went away comes down.
    if let Some(o) = old.as_ref().filter(|o| o.has_sticky() && (o.channel != channel || !rule.has_sticky())) {
        sticky::remove(&ctx, o.channel).await;
    }
    let changed = old.as_ref().is_none_or(|o| o.channel != channel || o.sticky.trim() != rule.sticky.trim());
    if rule.has_sticky() && changed && ctx.enabled(guild, KEY) {
        let _ = sticky::post(&ctx, guild, channel, &rule.sticky).await;
    }
    Ok(saved(&page.path("rules")))
}

async fn remove(State(ctx): State<Ctx>, user: User, Path((guild, channel)): Path<(u64, u64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let mut s = (*ctx.settings::<Settings>(guild, KEY)?).clone();
    let Some(i) = s.channels.iter().position(|r| r.channel == channel) else { return Ok(saved(&page.path("rules"))) };
    let gone = s.channels.remove(i);
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Removed the rules for {}", channel_name(&ctx, guild, channel)));
    if gone.has_sticky() {
        sticky::remove(&ctx, channel).await;
    }
    Ok(saved(&page.path("rules")))
}

/// Post the media-only notice in the rule's channel, to the person testing, marked as a test.
async fn test_notice(State(ctx): State<Ctx>, user: User, Path((guild, channel)): Path<(u64, u64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let rule = s.rule(channel).ok_or(WebError::NotFound)?;
    let name = channel_name(&ctx, guild, channel);
    if !rule.media_only {
        return Err(WebError::Bad(format!("{name} isn't media only, so there's no notice to test.")));
    }
    if !rule.media_notice {
        return Err(WebError::Bad(format!("{name} removes messages silently. Turn on Say why and save to post a notice.")));
    }
    let me = page.user.id;
    let mentions = AllowedMentions { users: vec![Id::new(me)], ..Default::default() };
    let sent = ctx
        .http
        .create_message(Id::new(channel))
        .content(&format!("{}\n-# Test from the dashboard, sent by {}", crate::media_notice(me), page.user.name))
        .allowed_mentions(Some(&mentions))
        .await;
    if let Err(e) = sent {
        return Err(WebError::Bad(bot_core::discord::why_refused(&e)));
    }
    ctx.audit(&page.user, Some(guild), KEY, format!("Sent a test media-only notice to {name}"));
    Ok(Redirect::to(&page.path("rules?sent=notice")).into_response())
}

/// Repost a channel's sticky message now, as the bot would after its count: at the bottom, with
/// the copy above deleted.
async fn repost(State(ctx): State<Ctx>, user: User, Path((guild, channel)): Path<(u64, u64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let rule = s.rule(channel).ok_or(WebError::NotFound)?;
    let name = channel_name(&ctx, guild, channel);
    if !rule.has_sticky() {
        return Err(WebError::Bad(format!("{name} has no sticky message. Write one and save first.")));
    }
    if !ctx.enabled(guild, KEY) {
        return Err(WebError::Bad("Channel rules are off for this server, so nothing is reposted. Turn them on first.".into()));
    }
    sticky::post(&ctx, guild, channel, &rule.sticky).await.map_err(WebError::Bad)?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Reposted the sticky message in {name}"));
    Ok(Redirect::to(&page.path("rules?sent=sticky")).into_response())
}

// ---- preview ----

pub fn seed(ctx: &Ctx, guild: u64) -> bot_core::anyhow::Result<()> {
    let ch = |n: u64| guild + n * 10;
    let settings = Settings {
        channels: vec![
            Rule { channel: ch(6), media_only: true, thread: true, thread_name: "{user}: {text}".into(), ..Rule::default() },
            Rule {
                channel: ch(4),
                sticky: "Trading and pet swaps go in #pets. Read #welcome before posting links.".into(),
                sticky_every: 25,
                sticky_quiet_mins: 10,
                ..Rule::default()
            },
            Rule { channel: ch(5), purge_after_mins: 7 * 1440, ..Rule::default() },
            Rule {
                channel: ch(7),
                sticky: "One idea per message. Staff answer in the thread under it.".into(),
                sticky_every: 5,
                thread: true,
                thread_name: "{text}".into(),
                ..Rule::default()
            },
        ],
    };
    ctx.save_settings(guild, KEY, &settings)?;
    let now = now_ms();
    let today = bot_core::jiff::Timestamp::now().to_zoned(bot_core::jiff::tz::TimeZone::UTC).date();
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    for (channel, ago) in [(ch(4), 4 * MIN_MS), (ch(7), 52 * MIN_MS)] {
        tx.execute(
            "INSERT OR REPLACE INTO rules_sticky (channel_id, guild_id, message_id, posted_at) VALUES (?1, ?2, ?3, ?4)",
            params![channel as i64, guild as i64, (channel + 1) as i64, now - ago],
        )?;
    }
    for back in 0..7i64 {
        let day = today.checked_sub(bot_core::jiff::Span::new().days(back))?.to_string();
        for (key, base) in [("rules_media", 3 + back % 4), ("rules_purged", if back % 2 == 0 { 40 + back * 3 } else { 0 }), ("rules_sticky", 9 + (back * 5) % 7), ("rules_threads", 4 + (back * 3) % 5)] {
            tx.execute(
                "INSERT OR REPLACE INTO daily_stats (guild_id, day, key, value) VALUES (?1, ?2, ?3, ?4)",
                params![guild as i64, day, key, base],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

const CSS: &str = r#"
/* The week: one panel, four counters split by hairlines, each with its seven days as bars. */
.ru-week { display: flex; flex-direction: column; gap: 10px; }
.ru-stats { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); border-radius: var(--r-card); background: linear-gradient(180deg, rgba(243, 242, 248, 0.05), rgba(243, 242, 248, 0.02)); box-shadow: inset 0 0 0 1px var(--edge-1); }
.ru-stat { display: flex; flex-direction: column; gap: 16px; min-width: 0; padding: 18px 22px 14px; }
.ru-stat + .ru-stat { border-left: 1px solid var(--edge-1); }
.ru-stat-head { display: flex; align-items: baseline; flex-wrap: wrap; gap: 4px 10px; min-width: 0; }
.ru-stat-head b { font: 600 26px/1.1 var(--sans); letter-spacing: -0.03em; font-variant-numeric: tabular-nums; }
.ru-stat-head span { font-size: 13px; color: var(--ink-2); }
.ru-bars { display: grid; grid-template-columns: repeat(7, minmax(0, 1fr)); gap: 6px; }
.ru-col { display: flex; flex-direction: column; align-items: stretch; gap: 7px; min-width: 0; cursor: default; }
.ru-bar { order: 1; display: block; width: 100%; max-width: 32px; height: 40px; margin: 0 auto; border-radius: 3px; background: var(--data-dim); transform-origin: 50% 100%; clip-path: inset(calc(100% - max(var(--v) * 1%, 3px)) 0 0 0 round 3px); animation: ru-grow 0.8s var(--ease) both; animation-delay: calc(var(--i, 0) * 40ms + 200ms); transition: background 0.15s; }
.ru-bar.today { background: var(--data); }
.ru-bar.zero { background: var(--edge-2); }
.ru-col:hover .ru-bar { background: color-mix(in oklab, var(--data), white 15%); }
.ru-col:hover .ru-bar.zero { background: var(--edge-3); }
.ru-day { order: 2; font-size: 11px; line-height: 1; text-align: center; color: var(--ink-3); }
.ru-day.today { color: var(--ink); font-weight: 600; }
@keyframes ru-grow { from { transform: scaleY(0); } }
html.same .ru-bar { animation: none; }

.ru-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(420px, 100%), 1fr)); gap: 16px; align-items: stretch; }
.ru-card { display: flex; flex-direction: column; gap: 16px; min-width: 0; padding: 18px; border-radius: var(--r-card); background: linear-gradient(180deg, var(--veil), var(--well)); box-shadow: inset 0 0 0 1px var(--edge-1); transition: transform 0.35s var(--ease), box-shadow 0.25s; }
.ru-card:hover { transform: translateY(-2px); box-shadow: inset 0 0 0 1px var(--edge-2), var(--shadow-lift); }
.ru-head { display: flex; align-items: center; gap: 12px; min-width: 0; }
.ru-name { display: flex; flex-direction: column; gap: 1px; min-width: 0; flex: 1; }
.ru-name strong { font-size: 16px; font-weight: 600; letter-spacing: -0.015em; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ru-name span { font-size: 12.5px; color: var(--ink-2); }
.ru-acts { display: flex; gap: 6px; flex: none; }

/* The four rules at a glance. A lit tile is a rule that's on. */
.ru-tiles { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 8px; }
.ru-tile { display: flex; flex-direction: column; gap: 9px; min-width: 0; padding: 10px 11px 11px; border-radius: 12px; background: var(--well); transition: background 0.35s var(--ease), box-shadow 0.35s var(--ease); }
.ru-tile.on { background: color-mix(in oklab, var(--hue) 10%, transparent); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--hue) 30%, transparent); }
.ru-ti { display: grid; place-items: center; width: 30px; height: 30px; border-radius: 9px; color: var(--ink-3); transition: background 0.3s, color 0.3s, box-shadow 0.3s; }
.ru-ti .icon { width: 16px; height: 16px; }
.ru-tile.on .ru-ti { color: var(--ground); background: var(--hue); box-shadow: none; }
.ru-tt { display: flex; flex-direction: column; min-width: 0; }
.ru-tl, .ru-tv { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ru-tl { font-size: 12.5px; font-weight: 600; color: var(--ink-2); }
.ru-tv { font-size: 11.5px; color: var(--ink-3); }
.ru-tile.on .ru-tl { color: var(--ink); }
.ru-tile.on .ru-tv { color: color-mix(in oklab, var(--hue), white 40%); }
/* The page's one moment: the lit tiles switch on across the grid, card by card. */
.ru-grid .ru-tile.on .ru-ti { animation: ru-light 0.7s var(--spring) both; animation-delay: calc(var(--c, 0) * 90ms + var(--i, 0) * 55ms + 280ms); }
.ru-tile.pop .ru-ti { animation: ru-light 0.55s var(--spring) both; }
@keyframes ru-light { from { transform: scale(0.55); opacity: 0.35; filter: saturate(0) brightness(0.6); } }

.ru-notes { display: flex; flex-direction: column; gap: 5px; }
.ru-note-row { display: flex; align-items: center; justify-content: space-between; gap: 6px 12px; flex-wrap: wrap; }
.ru-note-row .ru-note { flex: 1 1 220px; }
.ru-sticky-foot { display: flex; align-items: center; justify-content: space-between; gap: 6px 12px; flex-wrap: wrap; }
.ru-sticky-foot .ru-sticky-cap { flex: 1 1 220px; }
.ru-note { font-size: 13px; line-height: 1.5; color: var(--ink-2); overflow-wrap: anywhere; }
.ru-note code { font: 12px var(--mono); color: var(--ink); background: var(--control); padding: 1px 6px; border-radius: 6px; -webkit-box-decoration-break: clone; box-decoration-break: clone; }

/* The sticky message as Discord draws it. Discord's own colours: it's their surface. */
.ru-sticky { display: flex; flex-direction: column; gap: 7px; margin-top: auto; }
.ru-msg { padding: 12px 14px 13px; border-radius: 12px; background: #313338; box-shadow: 0 10px 26px -16px rgba(0, 0, 0, 0.9); }
/* A card's width: Discord's message (web/dc.rs) a size down, so the text keeps its line. */
.ru-msg .dp-post { gap: 12px; font-size: 14px; }
.ru-sticky-cap { display: flex; align-items: flex-start; gap: 7px; font-size: 12px; color: var(--ink-3); }
.ru-sticky-cap .icon { width: 14px; height: 14px; margin-top: 2px; color: var(--hue); }

/* Edit page. */
.ru-back { display: inline-flex; align-items: center; gap: 6px; align-self: flex-start; height: 28px; padding: 0 10px 0 6px; margin: -10px 0 -14px -6px; border-radius: 8px; font-size: 13px; color: var(--ink-3); transition: color 0.18s, background 0.18s; }
.ru-back:hover { color: var(--ink); background: var(--hover); }
.ru-back .icon { width: 14px; height: 14px; transform: rotate(90deg); transition: transform 0.25s var(--ease); }
.ru-back:hover .icon { transform: rotate(90deg) translateY(2px); }
.ru-sec { padding: 0 20px; }
.ru-sec-head { display: flex; align-items: center; gap: 14px; padding: 16px 0; }
.ru-sec-head h2 { font-size: 14.5px; }
.ru-sec-head .help { margin-top: 2px; }
.ru-sec-ic { display: grid; place-items: center; width: 38px; height: 38px; flex: none; border-radius: 12px; color: var(--hue); background: color-mix(in oklab, var(--hue) 13%, transparent); transform: scale(0.94); transition: background 0.3s, color 0.3s, box-shadow 0.3s, transform 0.45s var(--spring); }
.ru-sec-ic .icon { width: 18px; height: 18px; }
.ru-sec[data-rule="channel"] .ru-sec-ic,
.ru-sec:has(.ru-sec-ctl input:checked) .ru-sec-ic,
.ru-sec:has(#purge:not(:placeholder-shown)) .ru-sec-ic,
.ru-sec:has(#sticky:not(:placeholder-shown)) .ru-sec-ic { color: var(--ground); background: var(--hue); box-shadow: none; transform: none; }
.ru-sec-ctl, .ru-sec-pick { flex: none; }
.ru-sec-pick { width: min(280px, 40%); }
.ru-sec-body { border-top: 1px solid var(--edge-1); transition: opacity 0.3s var(--ease); }
.ru-sec:has(.ru-sec-ctl input:not(:checked)) .ru-sec-body { opacity: 0.55; }
.ru-sec:has(.ru-sec-ctl input:not(:checked)) .ru-sec-body:hover, .ru-sec:has(.ru-sec-ctl input:not(:checked)) .ru-sec-body:focus-within { opacity: 1; }
/* A wide form puts the three switches side by side, each card's field under its label, and the
   sticky message across the full width with the message beside its two repost settings. */
@container col (min-width: 1000px) {
  .ru-sec[data-rule="sticky"] .ru-sec-body { display: grid; grid-template-columns: minmax(0, 1.3fr) minmax(0, 1fr); column-gap: 36px; align-items: start; }
  .ru-sec[data-rule="sticky"] .row.wide { grid-row: span 2; }
  .ru-sec[data-rule="sticky"] .row:not(.wide) { grid-template-columns: minmax(0, 1fr); }
  .ru-sec[data-rule="sticky"] .row.wide + .row { border-top: 0; }
}
@container col (min-width: 1080px) {
  .ru-edit .settings fieldset { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 18px; }
  .ru-edit .ru-sec:is([data-rule="channel"], [data-rule="sticky"]) { grid-column: 1 / -1; }
  .ru-edit .ru-sec:is([data-rule="media"], [data-rule="purge"], [data-rule="thread"]) .row { grid-template-columns: minmax(0, 1fr); }
  .ru-edit .ru-sec:is([data-rule="media"], [data-rule="purge"], [data-rule="thread"]) .row:has(.switch) { grid-template-columns: minmax(0, 1fr) auto; }
}
.ru-side .ru-tiles { grid-template-columns: repeat(2, minmax(0, 1fr)); }
.ru-side .rail-block { gap: 14px; }
.ru-side [hidden] { display: none !important; }
.ru-side .ru-sticky { margin-top: 0; }

@container col (max-width: 760px) {
  .ru-stats { grid-template-columns: repeat(2, minmax(0, 1fr)); }
  .ru-stat:nth-child(3) { border-left: 0; }
  .ru-stat:nth-child(n + 3) { border-top: 1px solid var(--edge-1); }
}
@media (max-width: 920px) {
  .ru-stat { padding: 16px 16px 12px; gap: 14px; }
  .ru-stat-head b { font-size: 23px; }
  .ru-bars { gap: 4px; }
  .ru-tiles { grid-template-columns: repeat(2, minmax(0, 1fr)); }
  .ru-card { padding: 16px; }
  .ru-head { flex-wrap: wrap; }
  .ru-sec { padding: 0 16px; }
  .ru-sec-head { flex-wrap: wrap; }
  .ru-sec-head > .grow { flex: 1 1 180px; }
  .ru-sec-pick { width: 100%; }
}
@media (prefers-reduced-motion: reduce) {
  .ru-grid .ru-tile.on .ru-ti, .ru-tile.pop .ru-ti, .ru-bar { animation: none; }
}
"#;

/// The edit page's preview: the channel's name, the four tiles and the sticky message follow
/// the form as you change it. Tiles light with a short pop; text is rewritten in place.
const JS: &str = r#"(() => {
  const pv = document.getElementById("ru-pv");
  const f = document.querySelector("form.settings");
  if (!pv || !f) return;
  const d = pv.dataset;
  const field = (n) => f.elements.namedItem(n);
  const val = (n) => { const e = field(n); return e && typeof e.value === "string" ? e.value : ""; };
  const on = (n) => { const e = field(n); return !!(e && e.checked); };
  const byId = (id) => document.getElementById(id);
  const words = (mins) => {
    const p = [], dd = Math.floor(mins / 1440), h = Math.floor((mins % 1440) / 60), m = mins % 60;
    if (dd) p.push(dd === 1 ? "1 day" : dd + " days");
    if (h) p.push(h + " h");
    if (m) p.push(m + " min");
    return p.join(" ");
  };
  // Minutes, like the server reads it: "90" is minutes, "1d 12h" adds up. -1 when it can't be read.
  const minutes = (raw) => {
    const s = raw.trim().toLowerCase();
    if (!s || s === "0") return 0;
    if (/^\d+$/.test(s)) return +s;
    const unit = { s: 1, m: 60, h: 3600, d: 86400, w: 604800 };
    let secs = 0, num = "";
    for (const ch of s) {
      if (/\d/.test(ch)) num += ch;
      else if (/\s/.test(ch)) continue;
      else if (unit[ch] && num) { secs += +num * unit[ch]; num = ""; }
      else return -1;
    }
    return num || secs <= 0 ? -1 : Math.floor(secs / 60);
  };
  const clip = (s, n) => ([...s].length <= n ? s : [...s].slice(0, n - 1).join("") + "\u2026");
  const threadName = (template) => {
    const t = template.trim() ? template : d.default;
    let name = t.split("{user}").join(d.user).split("{date}").join(d.date).split("{text}").join(clip(d.text, 60));
    name = name.split(/\s+/).filter(Boolean).join(" ").replace(/^[\s:\-\u00b7]+|[\s:\-\u00b7]+$/g, "");
    return clip(name || d.user, 100);
  };
  const tile = (k, lit, value) => {
    const t = pv.querySelector('[data-rule="' + k + '"]');
    if (!t) return;
    const v = t.querySelector(".ru-tv");
    if (v && v.textContent !== value) v.textContent = value;
    if (t.classList.contains("on") === lit) return;
    t.classList.toggle("on", lit);
    t.classList.remove("pop");
    if (lit) { void t.offsetWidth; t.classList.add("pop"); }
  };
  const text = (id, s) => { const e = byId(id); if (e && e.textContent !== s) e.textContent = s; };
  const show = (id, yes) => { const e = byId(id); if (e) e.hidden = !yes; };
  const update = () => {
    const sel = field("channel");
    text("ru-pv-name", sel && sel.value && sel.selectedIndex >= 0 ? sel.options[sel.selectedIndex].text : "Pick a channel");
    const media = on("media_only");
    tile("media", media, media ? (on("media_notice") ? "With a notice" : "Silently") : "Off");
    const mins = minutes(val("purge"));
    const purge = mins >= 10 && mins <= 129600;
    tile("purge", purge, mins === 0 ? "Off" : purge ? words(mins) : "Check the limit");
    const sticky = val("sticky").trim();
    const every = Math.max(1, Math.min(100, parseInt(val("sticky_every"), 10) || 10));
    const quiet = Math.max(0, Math.min(120, parseInt(val("sticky_quiet_mins"), 10) || 0));
    tile("sticky", !!sticky, sticky ? "Every " + every : "Off");
    show("ru-pv-sticky", !!sticky);
    text("ru-pv-stext", sticky);
    text("ru-pv-scap", "After " + every + (every === 1 ? " message" : " messages") + (quiet > 0 ? " or " + quiet + " min of quiet" : ""));
    const thread = on("thread");
    tile("thread", thread, thread ? "Every message" : "Off");
    show("ru-pv-thread", thread);
    text("ru-pv-tname", threadName(val("thread_name")));
    show("ru-pv-none", !(media || purge || sticky || thread));
  };
  f.addEventListener("input", update);
  f.addEventListener("change", update);
  update();
})();"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purge_limits_read_and_write() {
        assert_eq!(parse_limit(""), Ok(0));
        assert_eq!(parse_limit("30m"), Ok(30));
        assert_eq!(parse_limit("1d 12h"), Ok(36 * 60));
        assert_eq!(parse_limit("90"), Ok(90));
        assert!(parse_limit("5m").is_err());
        assert!(parse_limit("100d").is_err());
        assert!(parse_limit("soon").is_err());
        assert_eq!(limit_input(36 * 60), "1d 12h");
        assert_eq!(limit_input(0), "");
        assert_eq!(parse_limit(&limit_input(7 * 1440 + 30)), Ok(7 * 1440 + 30));
        assert_eq!(limit_words(7 * 1440), "7 days");
        assert_eq!(limit_words(1440 + 90), "1 day 1 h 30 min");
    }
}
