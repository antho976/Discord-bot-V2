use std::collections::HashMap;

use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::rusqlite::{Connection, params};
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

/// Sums of the feature's daily counters over the last 7 days.
fn week_counts(conn: &Connection, guild: u64) -> bot_core::rusqlite::Result<HashMap<String, i64>> {
    let since = bot_core::jiff::Timestamp::now()
        .to_zoned(bot_core::jiff::tz::TimeZone::UTC)
        .date()
        .checked_sub(bot_core::jiff::Span::new().days(6))
        .map(|d| d.to_string())
        .unwrap_or_default();
    let mut stmt = conn.prepare(
        "SELECT key, SUM(value) FROM daily_stats WHERE guild_id = ?1 AND day >= ?2 AND key IN ('rules_media', 'rules_purged', 'rules_sticky', 'rules_threads') GROUP BY key",
    )?;
    stmt.query_map(params![guild as i64, since], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
}

/// When each sticky message in the server was last posted, by channel.
fn sticky_times(conn: &Connection, guild: u64) -> bot_core::rusqlite::Result<HashMap<u64, i64>> {
    let mut stmt = conn.prepare("SELECT channel_id, posted_at FROM rules_sticky WHERE guild_id = ?1")?;
    stmt.query_map([guild as i64], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?)))?.collect()
}

async fn list_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let week = ctx.db.with(|c| week_counts(c, guild))?;
    let posted = ctx.db.with(|c| sticky_times(c, guild))?;
    let n = |k: &str| text::num(week.get(k).copied().unwrap_or(0));
    let actions = (page.can_edit() && s.channels.len() < MAX_CHANNELS).then(|| {
        html! { a.btn.primary href=(page.path("rules/edit")) { "Add a channel" } }
    });
    let body = html! {
        style { (PreEscaped(CSS)) }
        div.stack.ru-main {
            (ui::readings(&[
                ("removed, no media", n("rules_media")),
                ("purged", n("rules_purged")),
                ("sticky reposts", n("rules_sticky")),
                ("threads started", n("rules_threads")),
            ]))
            p.muted.small.ru-caption { "Last 7 days, across " (s.channels.len()) @if s.channels.len() == 1 { " channel." } @else { " channels." } }
            @if s.channels.is_empty() {
                (ui::empty(Icon::Rules, "No channel rules yet", "Add a channel to keep it to media, clear out old messages, keep a message at the bottom, or start a thread on every message."))
            } @else {
                div.ru-list {
                    @for r in &s.channels { (rule_row(&ctx, &page, guild, r, posted.get(&r.channel).copied())) }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Channel rules", actions, body).into_response())
}

fn rule_row(ctx: &Ctx, page: &Page, guild: u64, r: &Rule, posted: Option<i64>) -> Markup {
    html! {
        div.ru-row {
            span.ru-icon { (ui::icon(Icon::Hash)) }
            div.ru-body {
                strong.clip { (channel_name(ctx, guild, r.channel)) }
                @if r.media_only {
                    p.ru-line { b { "Media only" } "Removes messages without a file or link" @if r.media_notice { ", with a short notice" } }
                }
                @if r.purge_after_mins > 0 {
                    p.ru-line { b { "Auto-purge" } "Deletes messages older than " (limit_words(r.purge_after_mins)) ", pinned ones stay" }
                }
                @if r.has_sticky() {
                    p.ru-line {
                        b { "Sticky" }
                        "“" (text::clip(r.sticky.trim(), 90)) "” after " (r.sticky_every) @if r.sticky_every == 1 { " message" } @else { " messages" }
                        @if r.sticky_quiet_mins > 0 { " or " (r.sticky_quiet_mins) " min of quiet" }
                        @if let Some(at) = posted { span.ru-when title=(page.user.local(at).strftime("%b %-d, %H:%M").to_string()) { " · reposted " (text::ago(at)) } }
                    }
                }
                @if r.thread {
                    p.ru-line { b { "Auto-thread" } "A thread on every message, named " code { (if r.thread_name.trim().is_empty() { DEFAULT_THREAD_NAME } else { r.thread_name.trim() }) } }
                }
            }
            div.ru-acts {
                @if page.can_edit() {
                    a.btn.small href={ (page.path("rules/edit")) "?channel=" (r.channel) } { "Edit" }
                    (ui::action_button(&page.path(&format!("rules/{}/remove", r.channel)), "Remove", Tone::Bad, true))
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct EditQuery {
    channel: Option<u64>,
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
        ctx.text_channels(guild)
            .into_iter()
            .filter(|(id, _)| *id == r.channel || s.rule(*id).is_none())
            .map(|(id, n)| (id.to_string(), format!("#{n}"))),
    );
    let title = if existing.is_some() { "Edit channel rules" } else { "Add a channel" };
    let body = html! {
        style { (PreEscaped(CSS)) }
        a.ru-back.small.muted href=(page.path("rules")) { "All channels" }
        (ui::settings_form(&page, &page.path("rules/edit"), html! {
            @if existing.is_some() { input type="hidden" name="was" value=(r.channel); }
            (ui::section("Channel", None, ui::card(html! {
                (ui::field_select("channel", "Channel", Some("Each channel has one set of rules. Threads in it aren't affected."), &channels, &if r.channel == 0 { String::new() } else { r.channel.to_string() }))
            })))
            (ui::section("Media only", None, ui::card(html! {
                (ui::field_toggle("media_only", "Media only", Some("Removes messages with no attachment and no link. The owner and people with Manage Messages can still talk."), r.media_only))
                (ui::field_toggle("media_notice", "Say why", Some("Posts a notice that removes itself after 6 seconds, at most once every 10 seconds."), r.media_notice))
            })))
            (ui::section("Auto-purge", None, ui::card(html! {
                (ui::field_text("purge", "Delete messages older than", Some("Like 30m, 12h or 7d, up to 90 days. Pinned messages and the sticky message stay. Empty leaves messages alone."), &limit_input(r.purge_after_mins), "Off"))
            })))
            (ui::section("Sticky message", None, ui::card(html! {
                (ui::field_textarea("sticky", "Message", Some("Posted again at the bottom of the channel, and the previous copy deleted. Empty turns it off."), &r.sticky, "Read the pinned rules before posting."))
                (ui::field_number("sticky_every", "Repost after", Some("New messages since the last copy. The bot waits for a 3 second pause, or 30 seconds at most."), r.sticky_every, 1, 100, Some("messages")))
                (ui::field_number("sticky_quiet_mins", "Or after a quiet spell of", Some("Reposts once the channel has been quiet this long after any new message. 0 waits for the count."), r.sticky_quiet_mins, 0, 120, Some("min")))
            })))
            (ui::section("Auto-thread", None, ui::card(html! {
                (ui::field_toggle("thread", "Thread on every message", Some("Starts a thread on each new message, for replies."), r.thread))
                (ui::field_text("thread_name", "Thread name", Some("{user}, {date} and {text}, the start of the message, are filled in."), &r.thread_name, DEFAULT_THREAD_NAME))
            })))
        }))
    };
    Ok(ui::layout(&ctx, &page, title, None, body).into_response())
}

async fn save(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let channel = form.channel(&ctx, guild, "channel").ok_or_else(|| WebError::Bad("Pick a channel.".into()))?;
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
        sticky::post(&ctx, guild, channel, &rule.sticky).await;
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

const CSS: &str = "
.ru-main { gap: 0; }
.ru-caption { margin: 8px 0 22px; }
.ru-list { display: flex; flex-direction: column; }
.ru-row { display: grid; grid-template-columns: 32px minmax(0, 1fr) auto; gap: 14px; align-items: start; padding: 16px 0; border-top: 1px solid var(--edge-1); }
.ru-row:last-child { border-bottom: 1px solid var(--edge-1); }
.ru-icon { width: 32px; height: 32px; border-radius: 8px; display: grid; place-items: center; background: var(--well); color: var(--ink-2); }
.ru-body { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.ru-body strong { font-weight: 500; font-size: 14px; }
.ru-line { font-size: 13px; color: var(--ink-2); line-height: 1.5; }
.ru-line b { display: inline-block; min-width: 92px; font-weight: 500; color: var(--ink); margin-right: 8px; }
.ru-line code { font: 12px var(--mono); color: var(--ink); }
.ru-when { color: var(--ink-3); }
.ru-acts { display: flex; gap: 6px; align-items: center; }
.ru-back { display: inline-block; margin-bottom: 4px; }
.ru-back::before { content: '‹ '; }
@media (max-width: 560px) {
  .ru-row { grid-template-columns: 32px minmax(0, 1fr); }
  .ru-acts { grid-column: 2; }
  .ru-line b { display: block; min-width: 0; }
}
";

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
