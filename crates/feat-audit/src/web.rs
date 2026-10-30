use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::get;
use bot_core::maud::{Markup, html};
use bot_core::twilight_model::id::Id;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebResult, saved};
use bot_core::{Ctx, Icon, text};
use serde::Deserialize;

use crate::store::{self, Filter};
use crate::{Kind, KEY, Settings, channel_label};

const SHOWN: i64 = 100;

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/audit", get(recent_page))
        .route("/g/{guild}/audit/settings", get(settings_page).post(save_settings))
}

fn tabs(page: &Page, active: &str) -> Markup {
    ui::tabs(&[("Recent", page.path(KEY)), ("Settings", page.path("audit/settings"))], active)
}

#[derive(Deserialize)]
struct RecentQuery {
    #[serde(rename = "type")]
    kind: Option<String>,
    q: Option<String>,
}

async fn recent_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<RecentQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let filter = Filter {
        kind: q.kind.as_deref().and_then(Kind::from_key),
        search: q.q.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned),
    };
    let filtered = filter.kind.is_some() || filter.search.is_some();
    let stats = store::stats(&ctx, guild)?;
    let (rows, total) = store::recent(&ctx, guild, &filter, SHOWN)?;
    let has_channel = s.channel.is_some() || s.message_channel.is_some() || s.member_channel.is_some();

    let mut options = vec![(String::new(), "All events".to_owned())];
    options.extend(Kind::ALL.iter().map(|k| (k.key().to_owned(), k.label().to_owned())));

    let body = html! {
        (tabs(&page, "Recent"))
        (ui::readings(&[
            ("events today", text::num(stats.today)),
            ("deletions this week", text::num(stats.deleted_week)),
            ("joined this week", text::num(stats.joined_week)),
            ("left this week", text::num(stats.left_week)),
        ]))
        form.inline-form method="get" action=(page.path(KEY)) {
            (ui::select("type", &options, filter.kind.map(Kind::key).unwrap_or("")))
            input.input type="search" name="q" value=(filter.search.as_deref().unwrap_or_default()) placeholder="Member name or ID";
            button.btn type="submit" { "Filter" }
            @if filtered { a.btn href=(page.path(KEY)) { "Clear" } }
        }
        @if rows.is_empty() {
            @if filtered {
                (ui::empty(Icon::Log, "No match", "Nothing in the last 14 days fits that filter."))
            } @else if has_channel {
                (ui::empty(Icon::Log, "Nothing logged yet", "Edits, deletes, joins and the rest show up here for 14 days as they happen."))
            } @else {
                (ui::empty(Icon::Log, "No log channel yet", "Pick one in Settings and events start appearing here and in Discord."))
            }
        } @else {
            div.table-wrap { table.table {
                thead { tr { th { "When" } th { "Member" } th { "Event" } } }
                tbody { @for r in &rows {
                    tr {
                        td style="white-space:nowrap;vertical-align:top" title=(when_exact(r.at)) { (text::ago(r.at)) }
                        td style="vertical-align:top" { @if let Some(u) = r.user_id { (ui::person(&member_name(&ctx, guild, u, &r.user_name), &avatar(&ctx, u), None)) } @else { span.muted { "Several members" } } }
                        td {
                            div.inline-form style="gap:8px" {
                                (ui::badge(r.kind.label(), Tone::Muted))
                                @if let Some(c) = r.channel_id { span.muted.small { (channel_label(&ctx, c)) } }
                            }
                            p.small style="margin:4px 0 0;color:var(--ink-2)" { (r.summary) }
                        }
                    }
                } }
            } }
            @if total > rows.len() as i64 {
                p.muted.small { "Showing the latest " (rows.len()) " of " (text::num(total)) ". Filter to narrow it down." }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Audit log", None, body).into_response())
}

/// The name now if the cache knows them, otherwise the one saved with the entry.
fn member_name(ctx: &Ctx, guild: u64, user: u64, saved_name: &str) -> String {
    if saved_name.is_empty() || ctx.cache.user(Id::new(user)).is_some() {
        ctx.display_name(guild, user)
    } else {
        saved_name.to_owned()
    }
}

fn avatar(ctx: &Ctx, user: u64) -> String {
    let hash = ctx.cache.user(Id::new(user)).and_then(|u| u.avatar.map(|a| a.to_string()));
    bot_core::discord::avatar_url(user, hash.as_deref())
}

fn when_exact(ms: i64) -> String {
    bot_core::jiff::Timestamp::from_millisecond(ms).map(|t| t.strftime("%Y-%m-%d %H:%M UTC").to_string()).unwrap_or_default()
}

async fn settings_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let body = html! {
        (tabs(&page, "Settings"))
        (ui::settings_form(&page, &page.path("audit/settings"), html! {
            (ui::section("Where it goes", Some("Nothing is logged until you pick a channel."), ui::card(html! {
                (ui::field_channel(&ctx, guild, "channel", "Log channel", Some("Every event goes here unless one of the two below is set."), s.channel))
                (ui::field_channel(&ctx, guild, "message_channel", "Message events", Some("Edits and deletes. None uses the log channel."), s.message_channel))
                (ui::field_channel(&ctx, guild, "member_channel", "Member events", Some("Joins, leaves, roles, bans, timeouts and voice. None uses the log channel."), s.member_channel))
            })))
            (ui::section("Messages", None, ui::card(html! {
                (ui::field_toggle("message_delete", "Deleted", Some("The author, channel, text and file names, when the bot had seen the message."), s.message_delete))
                (ui::field_toggle("message_edit", "Edited", Some("Before and after, with a link to the message."), s.message_edit))
                (ui::field_toggle("bulk_delete", "Bulk deleted", Some("How many messages a purge removed, and where."), s.bulk_delete))
            })))
            (ui::section("Members", None, ui::card(html! {
                (ui::field_toggle("join", "Joined", Some("With the account's age. Accounts under 7 days old are flagged."), s.join))
                (ui::field_toggle("leave", "Left", Some("With the roles they had."), s.leave))
                (ui::field_toggle("nickname", "Nickname changed", None, s.nickname))
                (ui::field_toggle("roles", "Roles added or removed", None, s.roles))
                (ui::field_toggle("ban", "Banned", None, s.ban))
                (ui::field_toggle("unban", "Unbanned", None, s.unban))
                (ui::field_toggle("timeout", "Timed out or released", None, s.timeout))
                (ui::field_toggle("boost", "Boosted", Some("When a member starts boosting the server."), s.boost))
                (ui::field_toggle("voice", "Voice joins, leaves and moves", Some("Off by default. In a busy server this is most of the log."), s.voice))
            })))
            (ui::section("Skip", None, ui::card(html! {
                (ui::field_toggle("ignore_bots", "Ignore bots", Some("Nothing is logged about bot accounts, including their messages."), s.ignore_bots))
                (ui::field_channels(&ctx, guild, "ignored_channels", "Ignored channels", Some("Edits and deletes in these channels aren't logged. The log channels themselves are always skipped."), &s.ignored_channels))
            })))
        }))
    };
    Ok(ui::layout(&ctx, &page, "Audit log", None, body).into_response())
}

async fn save_settings(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = Settings {
        channel: form.id("channel"),
        message_channel: form.id("message_channel"),
        member_channel: form.id("member_channel"),
        message_delete: form.bool("message_delete"),
        message_edit: form.bool("message_edit"),
        bulk_delete: form.bool("bulk_delete"),
        join: form.bool("join"),
        leave: form.bool("leave"),
        nickname: form.bool("nickname"),
        roles: form.bool("roles"),
        ban: form.bool("ban"),
        unban: form.bool("unban"),
        timeout: form.bool("timeout"),
        boost: form.bool("boost"),
        voice: form.bool("voice"),
        ignore_bots: form.bool("ignore_bots"),
        ignored_channels: form.ids("ignored_channels"),
    };
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed audit log settings");
    Ok(saved(&page.path("audit/settings")))
}
