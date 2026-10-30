use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, html};
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, text};
use serde::Deserialize;

use crate::score::{self, Facts, Strictness};
use crate::{Catch, KEY, Outcome, Settings, TIMEOUTS, links};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/guard", get(page).post(save))
        .route("/g/{guild}/guard/catches/{id}/release", post(release))
        .route("/g/{guild}/guard/catches/{id}/ban", post(ban))
}

#[derive(Deserialize, Default)]
struct TestQuery {
    test: Option<String>,
    new: Option<String>,
    repeat: Option<String>,
    ads: Option<String>,
}

async fn page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<TestQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let (week, released, banned) = crate::counts(&ctx, guild)?;
    let catches = crate::recent_catches(&ctx, guild, 50)?;
    let test = q.test.as_deref().map(|t| text::clip(t, 2000)).filter(|t| !t.trim().is_empty());

    let body = html! {
        (ui::readings(&[
            ("caught this week", text::num(week)),
            ("released as mistakes, 30 days", text::num(released)),
            ("banned, 30 days", text::num(banned)),
        ]))
        @if s.log_channel.is_none() {
            p.muted.small { "No log channel yet, so catches only show up here and behind the bell. Pick one below." }
        }
        (ui::section("Recent catches", Some("Release lifts the timeout and marks the catch as a mistake. Ban also deletes their messages from the last day."), html! {
            @if catches.is_empty() {
                (ui::empty(Icon::Shield, "Nothing caught yet", "Catches show up here when someone posts a scam or writes in the honeypot channel."))
            } @else {
                (bot_core::maud::PreEscaped(PHONE_CSS))
                div.table-wrap { table.table.catches {
                    thead { tr { th { "When" } th { "Member" } th { "Why" } th { "Outcome" } @if page.can_edit() { th {} } } }
                    tbody { @for c in &catches { (catch_row(&ctx, &page, c)) } }
                } }
            }
        }))
        (test_section(&page, &s, test.as_deref(), q.new.is_some(), q.repeat.is_some(), if test.is_some() { q.ads.is_some() } else { s.catch_ads }))
        (ui::settings_form(&page, &page.path(KEY), html! {
            (ui::section("Detection", None, ui::card(html! {
                (ui::field_toggle("scam_detection", "Check messages for scams", Some("Scores every message from members without Manage Messages. The honeypot works either way."), s.scam_detection))
                (ui::field_select("strictness", "Strictness", Some("How sure the bot must be before it acts. Try messages in the test box above to see the difference."),
                    &Strictness::ALL.iter().map(|x| (x.key().to_owned(), x.label().to_owned())).collect::<Vec<_>>(), s.strictness.key()))
                (ui::field_toggle("catch_ads", "Also catch commission and art ads", Some("Off by default: ads aren't scams, and a caught member is timed out."), s.catch_ads))
                (ui::field_number("new_account_days", "New accounts", Some("Accounts younger than this count against a suspicious message. 0 turns it off."), s.new_account_days, 0, 90, Some("days")))
            })))
            (ui::section("Honeypot", Some("A channel nobody has a reason to write in. Scam bots post in every channel, so anything written there is treated as a catch."), ui::card(html! {
                (ui::field_channel(&ctx, guild, "honeypot_channel", "Honeypot channel", Some("Tell members not to post there, for example in the channel topic."), s.honeypot_channel))
            })))
            (ui::section("When someone is caught", Some("Their messages from the last ten minutes are deleted and they're timed out."), ui::card(html! {
                (ui::field_channel(&ctx, guild, "log_channel", "Alert channel", Some("Staff get the alert here, with buttons to remove the timeout or ban."), s.log_channel))
                (ui::field_select("timeout_minutes", "Timeout", None,
                    &TIMEOUTS.iter().map(|(m, l)| (m.to_string(), (*l).to_owned())).collect::<Vec<_>>(), &s.timeout_minutes.to_string()))
                (ui::field_toggle("dm_member", "Message the member", Some("A DM saying their account looked compromised and how to get the timeout lifted."), s.dm_member))
            })))
            (ui::section("Never check", None, ui::card(html! {
                (ui::field_roles(&ctx, guild, "exempt_roles", "Roles", Some("Members with any of these roles are never checked, in the honeypot either."), &s.exempt_roles))
                (ui::field_channels(&ctx, guild, "exempt_channels", "Channels", Some("Messages here aren't scored. Threads inside them neither."), &s.exempt_channels))
            })))
        }))
    };
    Ok(ui::layout(&ctx, &page, "Scam protection", None, body).into_response())
}

/// On a phone each catch stacks: member and outcome, then why, then when and the buttons.
const PHONE_CSS: &str = "<style>
.catches td.when, .catches td.outcome { white-space: nowrap; }
.catches td.why { min-width: 260px; }
@media (max-width: 640px) {
  .catches thead { display: none; }
  .catches tr { display: grid; grid-template-columns: minmax(0, 1fr) auto; grid-template-areas: 'who outcome' 'why why' 'when actions'; gap: 6px 12px; padding: 12px 0; border-top: 1px solid var(--edge-1); }
  .catches td { border: 0; padding: 0; }
  .catches td.who { grid-area: who; } .catches td.outcome { grid-area: outcome; text-align: right; }
  .catches td.why { grid-area: why; min-width: 0; } .catches td.when { grid-area: when; align-self: center; }
  .catches td.actions { grid-area: actions; }
  .catches tr:hover td { background: none; }
}
</style>";

fn catch_row(ctx: &Ctx, page: &Page, c: &Catch) -> Markup {
    let tone = match c.outcome {
        Outcome::TimedOut | Outcome::NotTimedOut => Tone::Warn,
        Outcome::Released => Tone::Muted,
        Outcome::Banned => Tone::Bad,
    };
    let hash = ctx.cache.user(bot_core::twilight_model::id::Id::new(c.user_id)).and_then(|u| u.avatar.map(|a| a.to_string()));
    let avatar = bot_core::discord::avatar_url(c.user_id, hash.as_deref());
    let all_signals = c.signals.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join("\n");
    let quoted = if c.content.trim().is_empty() { "No text, attachments only".to_owned() } else { links::defang(&text::clip(&c.content, 140)) };
    html! {
        tr {
            td.when.muted { (text::ago(c.at)) }
            td.who { (ui::person(&c.user_name, &avatar, None)) }
            td.why {
                div title=(all_signals) {
                    (c.reason)
                    @if c.signals.len() > 1 { span.muted { " and " (c.signals.len() - 1) " more" } }
                }
                div.muted.small.mono style="overflow-wrap:anywhere" { (quoted) }
            }
            td.outcome {
                (ui::badge(c.outcome.label(), tone))
                @if let Some(by) = &c.acted_by_name { div.muted.small { "by " (by) } }
            }
            @if page.can_edit() {
                td.actions {
                    @if c.outcome.open() {
                        (ui::action_button(&page.path(&format!("{KEY}/catches/{}/release", c.id)), "Release", Tone::Muted, false))
                        (ui::action_button(&page.path(&format!("{KEY}/catches/{}/ban", c.id)), "Ban", Tone::Bad, true))
                    }
                }
            }
        }
    }
}

fn test_section(page: &Page, s: &Settings, test: Option<&str>, new: bool, repeat: bool, ads: bool) -> Markup {
    let result = test.map(|t| {
        let facts = Facts {
            account_age_days: Some(if new { 1.0 } else { 365.0 }),
            new_account_days: s.new_account_days.max(2),
            repeat_channels: if repeat { 3 } else { 1 },
            has_attachments: false,
            catch_ads: ads,
        };
        score::score(t, &facts)
    });
    html! {
        (ui::section("Test a message", Some("Paste a message to see how it scores. Nothing is posted or stored."), html! {
            form.stack id="test" method="get" action={ (page.path(KEY)) "#test" } {
                textarea.input name="test" rows="3" placeholder="free nitro for everyone https://..." { (test.unwrap_or_default()) }
                div.inline-form {
                    label.chip { input type="checkbox" name="new" value="1" checked[new]; span { "From a new account" } }
                    label.chip { input type="checkbox" name="repeat" value="1" checked[repeat]; span { "Posted in 3 channels at once" } }
                    label.chip { input type="checkbox" name="ads" value="1" checked[ads]; span { "Count ads" } }
                    button.btn type="submit" { "Score it" }
                }
            }
            @if let Some(v) = &result {
                @let threshold = s.strictness.threshold();
                div.stack style="gap:6px" {
                    p {
                        b.mono { "Score " (v.score) }
                        span.muted {
                            @if v.caught(s.strictness) { " · would be caught at " (s.strictness.key()) " strictness, which needs " (threshold) }
                            @else { " · not caught at " (s.strictness.key()) " strictness, which needs " (threshold) }
                        }
                    }
                    @if v.signals.is_empty() {
                        p.muted.small { "Nothing suspicious in it." }
                    } @else {
                        div.list {
                            @for sig in &v.signals {
                                div.list-item style="padding:6px 0" {
                                    span.mono.muted style="min-width:28px" { "+" (sig.weight) }
                                    span { (sig.text) }
                                }
                            }
                        }
                    }
                }
            }
        }))
    }
}

async fn save(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let timeout = form.i64("timeout_minutes").filter(|m| TIMEOUTS.iter().any(|(t, _)| t == m)).ok_or_else(|| WebError::Bad("Pick a timeout length.".into()))?;
    let s = Settings {
        scam_detection: form.bool("scam_detection"),
        honeypot_channel: form.channel(&ctx, guild, "honeypot_channel"),
        log_channel: form.channel(&ctx, guild, "log_channel"),
        timeout_minutes: timeout,
        strictness: Strictness::parse(&form.string("strictness")),
        new_account_days: form.int_in("new_account_days", 0, 90, 7),
        exempt_roles: form.roles(&ctx, guild, "exempt_roles"),
        exempt_channels: form.channels(&ctx, guild, "exempt_channels"),
        dm_member: form.bool("dm_member"),
        catch_ads: form.bool("catch_ads"),
    };
    if s.honeypot_channel.is_some() && s.honeypot_channel == s.log_channel {
        return Err(WebError::Bad("The honeypot and the alert channel can't be the same channel.".into()));
    }
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed scam protection settings");
    Ok(saved(&page.path(KEY)))
}

async fn release(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    act(ctx, user, guild, id, Outcome::Released).await
}

async fn ban(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    act(ctx, user, guild, id, Outcome::Banned).await
}

async fn act(ctx: Ctx, user: User, guild: u64, id: i64, outcome: Outcome) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let c = crate::resolve(&ctx, guild, id, outcome, page.user.id, &page.user.name, true).await.map_err(WebError::Bad)?;
    let what = match outcome {
        Outcome::Banned => format!("Banned {} from a scam catch", c.user_name),
        _ => format!("Released {} from a scam catch as a mistake", c.user_name),
    };
    ctx.audit(&page.user, Some(guild), KEY, what);
    Ok(saved(&page.path(KEY)))
}
