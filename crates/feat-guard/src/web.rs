use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::{IntoResponse, Redirect};
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::twilight_model::channel::ChannelType;
use bot_core::twilight_model::id::Id;
use bot_core::web::{dc, ui};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};
use serde::Deserialize;

use crate::score::{self, Facts, Signal, Strictness};
use crate::{Catch, KEY, Outcome, Settings, TIMEOUTS, Week, links};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/guard", get(page).post(save))
        .route("/g/{guild}/guard/settings", get(settings_page).post(save))
        .route("/g/{guild}/guard/catches/{id}/release", post(release))
        .route("/g/{guild}/guard/catches/{id}/ban", post(ban))
        .route("/g/{guild}/guard/test", post(send_test))
        .route("/g/{guild}/guard/notice", post(post_notice))
}

/// The two tabs, with how many catches wait on a decision.
fn tabs(page: &Page, waiting: i64, active: &str) -> Markup {
    ui::tabs_counted(&[("Catches", page.path(KEY), Some(waiting).filter(|n| *n > 0)), ("Settings", page.path("guard/settings"), None)], active)
}

/// A channel set in the settings that the server no longer has. Only once the server is in the
/// cache, so a restart doesn't show every channel as gone.
fn deleted(ctx: &Ctx, guild: u64, channel: Option<u64>) -> bool {
    channel.is_some_and(|c| ctx.cache.guild(Id::new(guild)).is_some() && ctx.cache.channel(Id::new(c)).is_none())
}

/// The header pill: whether the checks run, and how strict they are.
fn status(ctx: &Ctx, guild: u64, s: &Settings) -> Markup {
    let (dot, label) = if s.scam_detection {
        ("dot live", format!("On · {} strictness", s.strictness.key()))
    } else if s.honeypot_channel.is_some() && !deleted(ctx, guild, s.honeypot_channel) {
        ("dot waiting", "Honeypot only".to_owned())
    } else {
        ("dot", "Off".to_owned())
    };
    html! { span.status { span class=(dot) {} (label) } }
}

/// Which catches the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Show {
    All,
    Waiting,
    Banned,
    Released,
}

impl Show {
    const ALL: [Show; 4] = [Show::All, Show::Waiting, Show::Banned, Show::Released];

    fn key(self) -> &'static str {
        match self {
            Show::All => "",
            Show::Waiting => "waiting",
            Show::Banned => "banned",
            Show::Released => "released",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Show::All => "All",
            Show::Waiting => "Waiting",
            Show::Banned => "Banned",
            Show::Released => "Released",
        }
    }

    fn outcomes(self) -> &'static [Outcome] {
        match self {
            Show::All => &[],
            Show::Waiting => &[Outcome::TimedOut, Outcome::NotTimedOut],
            Show::Banned => &[Outcome::Banned],
            Show::Released => &[Outcome::Released],
        }
    }

    fn empty(self) -> (&'static str, &'static str) {
        match self {
            Show::All => ("Nothing caught yet", "Catches show up here when someone posts a scam or writes in the honeypot channel."),
            Show::Waiting => ("Nothing waiting", "Every catch has been released or banned."),
            Show::Banned => ("Nobody banned", "Catches you ban, here or in Discord, show up here."),
            Show::Released => ("Nobody released", "Catches you release as mistakes show up here."),
        }
    }
}

#[derive(Deserialize, Default)]
struct TestQuery {
    show: Option<String>,
    test: Option<String>,
    new: Option<String>,
    repeat: Option<String>,
    ads: Option<String>,
}

async fn page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<TestQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let week = crate::week(&ctx, guild)?;
    let show = Show::ALL.into_iter().find(|s| Some(s.key()) == q.show.as_deref()).unwrap_or(Show::All);
    let catches = crate::recent_catches(&ctx, guild, show.outcomes(), 50)?;
    let waiting = crate::count_open(&ctx, guild)?;
    let test = q.test.as_deref().map(|t| text::clip(t, 2000)).filter(|t| !t.trim().is_empty());
    let settings_href = page.path("guard/settings");
    let threshold = i64::from(s.strictness.threshold());

    let body = html! {
        (style())
        (tabs(&page, waiting, "Catches"))
        div.with-rail {
            div.gd-main {
                @let log_gone = deleted(&ctx, guild, s.log_channel);
                @let honeypot_gone = deleted(&ctx, guild, s.honeypot_channel);
                @if log_gone || honeypot_gone || s.log_channel.is_none() {
                    div.gd-needs {
                        @if log_gone {
                            (need(true, Icon::Bell, "The alert channel was deleted", "Catches only show up here and behind the bell until you pick another.", &settings_href, "Pick another"))
                        } @else if s.log_channel.is_none() {
                            (need(false, Icon::Bell, "No alert channel yet", "Catches only show up here and behind the bell. Staff get buttons to release or ban in the alert.", &settings_href, "Pick one"))
                        }
                        @if honeypot_gone {
                            (need(true, Icon::Hash, "The honeypot channel was deleted", "Nothing is caught there until you pick another.", &settings_href, "Pick another"))
                        }
                    }
                }
                (readings(&week, waiting))
                section.gd-catches {
                    div.gd-list-head {
                        div {
                            h2 { "Recent catches" }
                            p.muted.small { "Release lifts the timeout and marks the catch as a mistake. Ban also deletes their messages from the last day." }
                        }
                        nav.seg.gd-show aria-label="Show" {
                            @for sh in Show::ALL {
                                a.on[sh == show] href={ (page.path(KEY)) @if !sh.key().is_empty() { "?show=" (sh.key()) } } aria-current=[(sh == show).then_some("true")] {
                                    (sh.label())
                                    @if sh == Show::Waiting && waiting > 0 { " " span.gd-count { (waiting) } }
                                }
                            }
                        }
                    }
                    @if catches.is_empty() {
                        @let (title, line) = show.empty();
                        (ui::empty(Icon::Shield, title, line))
                    } @else {
                        div.gd-list {
                            @for (i, c) in catches.iter().enumerate() { (catch_card(&ctx, &page, guild, c, threshold, s.new_account_days, i)) }
                        }
                    }
                }
            }
            aside.rail {
                (week_rail(&ctx, guild, &week))
                (test_rail(&page, &s, Some(show.key()).filter(|k| !k.is_empty()), test.as_deref(), q.new.is_some(), q.repeat.is_some(), if test.is_some() { q.ads.is_some() } else { s.catch_ads }))
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Scam protection", Some(status(&ctx, guild, &s)), body).into_response())
}

/// A row that waits on you: a broken or missing channel, with the way to fix it.
fn need(alert: bool, ic: Icon, title: &str, line: &str, href: &str, action: &str) -> Markup {
    html! {
        div.need.alert[alert] {
            span.need-icon.alert[alert] { (ui::icon(ic)) }
            div.grow { strong { (title) } p.muted.small { (line) } }
            a class=(if alert { "btn small primary" } else { "btn small outline" }) href=(href) { (action) }
        }
    }
}

/// The week's numbers, counting up as they arrive.
fn readings(week: &Week, waiting: i64) -> Markup {
    html! {
        div.readings.gd-readings {
            div.reading {
                b data-count { (text::num(week.caught)) }
                span { "Caught this week" }
            }
            div.reading.gd-wait[waiting > 0] {
                b data-count { (text::num(waiting)) }
                span { "Waiting on a decision" }
            }
            div.reading { b data-count { (text::num(week.banned)) } span { "Banned" } }
            div.reading { b data-count { (text::num(week.released)) } span { "Released as mistakes" } }
        }
    }
}

/// The label for one of the seven bars, counted back from the last 24 hours.
fn day_label(i: usize) -> String {
    if i == 6 { "Last 24 hours".to_owned() } else { format!("{} to {} days ago", 6 - i, 7 - i) }
}

/// The rail's week: catches per day, messages removed, and the moderators who decided.
fn week_rail(ctx: &Ctx, guild: u64, week: &Week) -> Markup {
    let series: Vec<(String, Option<i64>)> = week.daily.iter().enumerate().map(|(i, n)| (day_label(i), Some(*n))).collect();
    let names: Vec<String> = week.handled_by.iter().take(3).map(|u| ctx.display_name(guild, *u)).collect();
    html! {
        div.rail-block {
            h2 { "This week" span.muted.small { (plural(week.removed, "message")) " removed" } }
            (ui::mini_bars(&series, "caught", "Catches in each of the last seven days"))
            div.gd-axis { span { "7 days ago" } span { "Today" } }
            @if week.handled_by.is_empty() {
                p.muted.small { "Nobody has released or banned anyone this week." }
            } @else {
                div.gd-handlers {
                    (ui::avatars(&week.handled_by.iter().map(|u| ctx.member_avatar(guild, *u)).collect::<Vec<_>>(), 4))
                    p.small { (and_list(&names, week.handled_by.len())) " decided this week's catches." }
                }
            }
        }
    }
}

/// "Mira", "Mira and Oskar", "Mira, Oskar and 2 more".
fn and_list(names: &[String], total: usize) -> String {
    match (names, total) {
        ([a], 1) => a.clone(),
        ([a, b], 2) => format!("{a} and {b}"),
        ([a, b, c], 3) => format!("{a}, {b} and {c}"),
        _ => format!("{} and {} more", names.iter().take(2).cloned().collect::<Vec<_>>().join(", "), total.saturating_sub(2)),
    }
}

/// One catch: the message as Discord showed it with the bait lit, what the bot did, the score
/// on its dial, and the decision.
fn catch_card(ctx: &Ctx, page: &Page, guild: u64, c: &Catch, threshold: i64, new_days: i64, i: usize) -> Markup {
    let name = member_name(ctx, guild, c.user_id, &c.user_name);
    let (shown, cut) = shown_part(&c.content);
    let (class, state) = match c.outcome {
        Outcome::TimedOut | Outcome::NotTimedOut => ("gd-catch open", "gd-state open"),
        Outcome::Released => ("gd-catch released", "gd-state released"),
        Outcome::Banned => ("gd-catch banned", "gd-state banned"),
    };
    let young = new_days > 0 && now_ms() - crate::account_created_ms(c.user_id) < new_days * 86_400_000;
    html! {
        article class=(class) style={ "--i:" (i.min(6)) } {
            div.gd-what {
                div.gd-discord {
                    div.gd-chan {
                        (ui::icon(Icon::Hash))
                        span { (channel_name(ctx, c.channel_id).trim_start_matches('#')) }
                        @if c.honeypot { span.gd-chan-tag { "honeypot" } }
                    }
                    (dc::message(ctx, page)
                        .from(name, Some(ctx.member_avatar(guild, c.user_id)))
                        .time_text(discord_time(page, c.at))
                        .stamp(page.user.local(c.at).strftime("%b %-d, %H:%M").to_string())
                        .render(html! {
                            div.dp-line {
                                @if shown.trim().is_empty() {
                                    em.gd-none { "No text, attachments only" }
                                } @else {
                                    (message(shown, &c.signals))
                                    @if cut { "…" }
                                }
                            }
                        }))
                }
                @if !c.signals.is_empty() {
                    ul.gd-sigs aria-label="Why it was caught" {
                        @for s in &c.signals {
                            li.plain[s.weight <= 0] { @if s.weight > 0 { b { "+" (s.weight) } } span { (s.text) } }
                        }
                    }
                }
                ol.gd-steps aria-label="What happened" {
                    (steps(ctx, page, guild, c))
                }
                @if let Some(reason) = &c.acted_reason {
                    p.gd-why { span { "Reason" } (reason) }
                }
            }
            div.gd-verdict {
                @if c.honeypot {
                    div.gd-hp {
                        span.gd-hp-ic { (ui::icon(Icon::Hash)) }
                        strong { "Honeypot" }
                        span { "Anything written there is a catch" }
                    }
                } @else {
                    (gauge(c.score, threshold, false))
                }
                div.gd-out {
                    span class=(state) { span.dot {} (c.outcome.label()) }
                    span.gd-age.new[young] { "Account made " (crate::account_made(c.user_id)) }
                }
                @if page.can_edit() && c.outcome.open() {
                    div.gd-acts {
                        (decide_pop(page, c, Outcome::Released))
                        (decide_pop(page, c, Outcome::Banned))
                    }
                }
            }
        }
    }
}

/// "Today at 14:02", "Yesterday at 09:10", then "10/24/2026 21:49", as Discord stamps a message,
/// in the viewer's timezone.
fn discord_time(page: &Page, at: i64) -> String {
    let z = page.user.local(at);
    let hm = z.strftime("%H:%M").to_string();
    match (page.user.local(now_ms()).date() - z.date()).get_days() {
        0 => format!("Today at {hm}"),
        1 => format!("Yesterday at {hm}"),
        _ => z.strftime("%m/%d/%Y %H:%M").to_string(),
    }
}

/// The start of a caught message for the list, cut back to a whole word.
fn shown_part(content: &str) -> (&str, bool) {
    const MAX: usize = 400;
    match content.char_indices().nth(MAX) {
        None => (content, false),
        Some((cut, _)) => {
            let end = content[..cut].char_indices().rev().find(|(_, c)| c.is_whitespace()).map_or(cut, |(i, _)| i);
            (&content[..end], true)
        }
    }
}

/// The caught message, with the parts the stored signals matched lit in red. Defanged like the
/// staff alert, so a link in it can't be clicked.
fn message(text: &str, signals: &[Signal]) -> Markup {
    let spans = score::highlight(text, signals);
    let pieces = segments(text, &spans);
    html! {
        @for (piece, hit) in &pieces {
            @if *hit { mark { (piece) } } @else { (piece) }
        }
    }
}

/// Split `text` at the highlighted ranges, defanging each piece.
fn segments(text: &str, spans: &[(usize, usize)]) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut at = 0;
    for &(start, end) in spans {
        if start > at {
            out.push((links::defang(&text[at..start]), false));
        }
        out.push((links::defang(&text[start..end]), true));
        at = end;
    }
    if at < text.len() {
        out.push((links::defang(&text[at..]), false));
    }
    out
}

/// What the bot did about the catch, then who decided, on one line under a hairline.
fn steps(ctx: &Ctx, page: &Page, guild: u64, c: &Catch) -> Markup {
    html! {
        @if c.deleted > 0 {
            li.gd-step { (ui::icon(Icon::Trash)) "Removed " (plural(c.deleted, "message")) }
        }
        @if c.outcome == Outcome::NotTimedOut {
            li.gd-step.bad { (ui::icon(Icon::Clock)) "Couldn't time them out" }
        } @else if let Some(until) = c.timeout_until {
            li.gd-step { (ui::icon(Icon::Clock)) "Timed out for " (timeout_for(c.at, until)) }
        }
        @if let Some((channel, _)) = c.alert {
            li.gd-step { (ui::icon(Icon::Bell)) "Alerted " (channel_name(ctx, channel)) }
        }
        (decided(ctx, page, guild, c))
    }
}

/// Release or Ban, with an optional reason, in a small popover. The popover is the second step,
/// so Ban needs no confirm click.
fn decide_pop(page: &Page, c: &Catch, outcome: Outcome) -> Markup {
    let ban = outcome == Outcome::Banned;
    let (label, path, help) = if ban {
        ("Ban", "ban", "Also deletes their messages from the last day.")
    } else if c.outcome == Outcome::TimedOut {
        ("Release as mistake", "release", "Lifts the timeout.")
    } else {
        ("Release as mistake", "release", "Marks the catch as a mistake.")
    };
    let class = if ban { "btn small danger" } else { "btn small" };
    html! {
        details.pop.gd-pop {
            summary class=(class) { (label) }
            div.pop-body {
                form.stack method="post" action=(page.path(&format!("{KEY}/catches/{}/{path}", c.id))) {
                    input.input type="text" name="reason" maxlength="300" placeholder="Reason, optional" aria-label="Reason, optional" autocomplete="off";
                    div.gd-pop-foot {
                        span.muted.small { (help) }
                        button class=(if ban { "btn small danger" } else { "btn small primary" }) type="submit" { (label) }
                    }
                }
            }
        }
    }
}

/// The last step: who released or banned the member, when and where, or that it waits.
fn decided(ctx: &Ctx, page: &Page, guild: u64, c: &Catch) -> Markup {
    let verb = match c.outcome {
        Outcome::Released if c.acted_in_discord => "removed the timeout in Discord",
        Outcome::Released => "released them as a mistake",
        _ if c.acted_in_discord => "banned them in Discord",
        _ => "banned them",
    };
    let nameless = match c.outcome {
        Outcome::Released => "Timeout removed in Discord",
        _ => "Banned in Discord",
    };
    let when = c.acted_at.map(text::ago);
    let exact = c.acted_at.map(|at| page.user.local(at).strftime("%b %-d, %H:%M").to_string());
    html! {
        @if c.outcome.open() {
            li.gd-step.gd-waiting {
                span.dot.waiting {}
                @if page.can_edit() { "Waiting on you" } @else { "Waiting on a moderator" }
            }
        } @else {
            li.gd-step.gd-by {
                @if let (Some(id), Some(by)) = (c.acted_by, &c.acted_by_name) {
                    img.avatar src=(ctx.member_avatar(guild, id)) alt="" width="20" height="20" loading="lazy";
                    span { strong { (member_name(ctx, guild, id, by)) } " " (verb) }
                } @else {
                    (ui::icon(if c.outcome == Outcome::Banned { Icon::Close } else { Icon::Check }))
                    span { (nameless) }
                }
                @if let (Some(w), Some(x)) = (&when, &exact) { time.gd-when title=(x) { (w) } }
            }
        }
    }
}

/// The score on a half dial: amber below the catch line, red from it, with a tick at the line.
/// The arc sweeps to the score and the number counts up as it arrives.
fn gauge(score: i64, threshold: i64, big: bool) -> Markup {
    let scale = score.max(16).max(threshold * 2) as f64;
    let fill = (score.max(0) as f64 / scale).min(1.0) * 100.0;
    let line = (threshold as f64 / scale).min(1.0);
    let caught = score >= threshold;
    // The dial: centre (60, 58), radius 48, from the left end over the top to the right.
    let at = |f: f64, r: f64| {
        let a = std::f64::consts::PI * (1.0 - f);
        (60.0 + r * a.cos(), 58.0 - r * a.sin())
    };
    let (t1, t2) = (at(line, 41.0), at(line, 55.0));
    html! {
        div.gd-gauge.caught[caught].big[big] role="img" aria-label={ "Score " (score) ", catches at " (threshold) } {
            svg viewBox="0 0 120 66" aria-hidden="true" {
                path.gd-track d="M12 58 A48 48 0 0 1 108 58" pathLength="100" {}
                path.gd-arc d="M12 58 A48 48 0 0 1 108 58" pathLength="100" style={ "stroke-dashoffset:" (format!("{:.1}", 100.0 - fill)) } {}
                line.gd-tick x1=(format!("{:.1}", t1.0)) y1=(format!("{:.1}", t1.1)) x2=(format!("{:.1}", t2.0)) y2=(format!("{:.1}", t2.1)) {}
            }
            div.gd-num {
                b data-count { (score) }
                span { "catches at " (threshold) }
            }
        }
    }
}

/// The score, the catch line on the dial, and each signal with its points.
fn test_rail(page: &Page, s: &Settings, show: Option<&str>, test: Option<&str>, new: bool, repeat: bool, ads: bool) -> Markup {
    // A new account is one a day old, or made today when the setting counts only that.
    let young = if s.new_account_days > 1 { 1.0 } else { 0.5 };
    let checks_age = s.new_account_days > 0;
    let result = test.map(|t| {
        let facts = Facts {
            account_age_days: Some(if new && checks_age { young } else { 365.0 }),
            new_account_days: s.new_account_days,
            repeat_channels: if repeat { 3 } else { 1 },
            has_attachments: false,
            catch_ads: ads,
        };
        score::score(t, &facts)
    });
    let threshold = s.strictness.threshold();
    html! {
        div.rail-block #test {
            h2 { "Test a message" }
            p.muted.small { "See how the bot would score it. Nothing is posted or stored." }
            form.stack.gd-test method="get" action={ (page.path(KEY)) "#test" } {
                @if let Some(show) = show { input type="hidden" name="show" value=(show); }
                textarea.input name="test" rows="4" placeholder="free nitro for everyone https://..." aria-label="Message to score" { (test.unwrap_or_default()) }
                div.chips {
                    @if checks_age {
                        label.chip { input type="checkbox" name="new" value="1" checked[new]; span { "From a new account" } }
                    }
                    label.chip { input type="checkbox" name="repeat" value="1" checked[repeat]; span { "Posted in 3 channels at once" } }
                    label.chip { input type="checkbox" name="ads" value="1" checked[ads]; span { "Count ads" } }
                }
                div { button.btn.primary type="submit" { "Score it" } }
            }
            @if let Some(v) = &result {
                div.gd-result {
                    (gauge(i64::from(v.score), i64::from(threshold), true))
                    div.gd-result-say {
                        @if v.caught(s.strictness) {
                            span.gd-state.banned { span.dot {} "Would catch" }
                        } @else {
                            span.gd-state.released { span.dot {} "Not caught" }
                        }
                        p.muted.small { "The tick is where a message gets caught at " (s.strictness.key()) " strictness." }
                    }
                    @if v.signals.is_empty() {
                        p.muted.small { "Nothing suspicious in it." }
                    } @else {
                        div.gd-tsigs {
                            @for (i, sig) in v.signals.iter().enumerate() {
                                div.gd-tsig style={ "--s:" (i) } {
                                    b { "+" (sig.weight) }
                                    span { (sig.text) }
                                    span.meter { span style={ "width:" ((sig.weight * 100 / v.score.max(1)).clamp(4, 100)) "%" } {} }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Deserialize, Default)]
struct SentQuery {
    /// A test alert went out: "1" a scam one, "honeypot" a honeypot one.
    sent: Option<String>,
    /// The honeypot notice was posted again.
    posted: Option<String>,
}

async fn settings_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<SentQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let waiting = crate::count_open(&ctx, guild)?;
    let detection = ui::section("Detection", None, ui::card(html! {
        (ui::field_toggle("scam_detection", "Check messages for scams", Some("Scores every message from members without Manage Messages. The honeypot works either way."), s.scam_detection))
        div.row.gd-strict-row {
            div.row-text {
                label { "Strictness" }
                p.help { "How sure the bot must be before it acts. Low catches at 8 points, only near-certain scams; normal at 5; high at 4, and expect some mistakes. Try a message on the Catches tab to see the difference." }
            }
            div.row-control { (strictness(s.strictness)) }
        }
        (ui::field_toggle("catch_ads", "Also catch commission and art ads", Some("Off by default: ads aren't scams, and a caught member is timed out."), s.catch_ads))
        (ui::field_number("new_account_days", "New accounts", Some("Accounts younger than this count against a suspicious message. 0 turns it off."), s.new_account_days, 0, 90, Some("days")))
    }));
    let honeypot = ui::section("Honeypot", Some("A channel nobody has a reason to write in. Scam bots post in every channel, so anything written there is treated as a catch."), ui::card(html! {
        (ui::field_channel(&ctx, guild, "honeypot_channel", "Honeypot channel", Some("The bot posts a notice there telling members not to write in it, and keeps it up to date when you save."), s.honeypot_channel))
    }));
    let caught = ui::section("When someone is caught", Some("Their messages from the last ten minutes are deleted and they're timed out."), ui::card(html! {
        (ui::field_channel(&ctx, guild, "log_channel", "Alert channel", Some("Staff get the alert here, with buttons to remove the timeout or ban."), s.log_channel))
        (ui::field_select("timeout_minutes", "Timeout", None,
            &TIMEOUTS.iter().map(|(m, l)| (m.to_string(), (*l).to_owned())).collect::<Vec<_>>(), &s.timeout_minutes.to_string()))
        (ui::field_toggle("dm_member", "Message the member", Some("A DM saying their account looked compromised and how to get the timeout lifted."), s.dm_member))
    }));
    let never = ui::section("Never check", None, ui::card(html! {
        (ui::field_roles(&ctx, guild, "exempt_roles", "Roles", Some("Members with any of these roles are never checked, in the honeypot either."), &s.exempt_roles))
        (ui::field_channels(&ctx, guild, "exempt_channels", "Channels", Some("Messages here aren't scored. Threads inside them neither."), &s.exempt_channels))
    }));
    let body = html! {
        (style())
        @if let Some(kind) = &q.sent {
            (ui::sent_toast(true, &format!("Test {} alert sent to {}", if kind == "honeypot" { "honeypot" } else { "scam" }, s.log_channel.map(|c| channel_name(&ctx, c)).unwrap_or_default())))
        }
        @if q.posted.is_some() { (ui::sent_toast(true, &format!("Notice posted in {}", s.honeypot_channel.map(|c| channel_name(&ctx, c)).unwrap_or_default()))) }
        (tabs(&page, waiting, "Settings"))
        div.with-rail {
            (ui::settings_form(&page, &page.path("guard/settings"), html! {
                // Two columns of sections once the column is wide enough. Stacked, they keep this order.
                div.gd-sets {
                    div.gd-set-col { (detection) (honeypot) }
                    div.gd-set-col { (caught) (never) }
                }
            }))
            aside.rail { (flow(&ctx, &page, guild, &s)) }
        }
    };
    Ok(ui::layout(&ctx, &page, "Scam protection", Some(status(&ctx, guild, &s)), body).into_response())
}

/// Low, normal or high, as a segmented choice.
fn strictness(on: Strictness) -> Markup {
    html! {
        div.seg role="radiogroup" aria-label="Strictness" {
            @for x in Strictness::ALL {
                label {
                    input type="radio" name="strictness" value=(x.key()) checked[x == on];
                    span { (match x { Strictness::Low => "Low", Strictness::Normal => "Normal", Strictness::High => "High" }) }
                }
            }
        }
    }
}

/// What happens to someone who is caught, step by step, as the saved settings have it, and a
/// test alert to see the post for yourself.
fn flow(ctx: &Ctx, page: &Page, guild: u64, s: &Settings) -> Markup {
    let honeypot = s.honeypot_channel.filter(|_| !deleted(ctx, guild, s.honeypot_channel)).map(|c| channel_name(ctx, c));
    let log = s.log_channel.filter(|_| !deleted(ctx, guild, s.log_channel)).map(|c| channel_name(ctx, c));
    let roles = ctx.roles(guild);
    let exempt: Vec<String> = s
        .exempt_roles
        .iter()
        .filter_map(|r| roles.iter().find(|(id, _, _)| id == r).map(|(_, n, _)| format!("@{n}")))
        .chain(s.exempt_channels.iter().map(|c| channel_name(ctx, *c)))
        .collect();
    let trigger = match (s.scam_detection, &honeypot) {
        (true, Some(h)) => format!("A message scores {} or more, or anyone writes in {h}", s.strictness.threshold()),
        (true, None) => format!("A message scores {} or more", s.strictness.threshold()),
        (false, Some(h)) => format!("Anyone writes in {h}. Scam checks are off"),
        (false, None) => "Nothing: scam checks are off and there's no honeypot".to_owned(),
    };
    let on = s.scam_detection || honeypot.is_some();
    html! {
        div.rail-block {
            h2 { "When someone is caught" }
            ol.gd-flow.off[!on] {
                li style="--s:0" { span.gd-flow-ic { (ui::icon(Icon::Shield)) } div { strong { "Caught" } span { (trigger) } } }
                li style="--s:1" { span.gd-flow-ic { (ui::icon(Icon::Trash)) } div { strong { "Cleaned up" } span { "Their messages from the last ten minutes are deleted" } } }
                li style="--s:2" { span.gd-flow-ic { (ui::icon(Icon::Clock)) } div { strong { "Timed out" } span { "For " (crate::timeout_label(s.timeout_minutes)) } } }
                li.muted[log.is_none()] style="--s:3" {
                    span.gd-flow-ic { (ui::icon(Icon::Bell)) }
                    div { strong { "Staff alerted" } span { @match &log { Some(l) => { "In " (l) ", with Release and Ban buttons" }, None => { "Only here and behind the bell: no alert channel" } } } }
                }
                li.muted[!s.dm_member] style="--s:4" {
                    span.gd-flow-ic { (ui::icon(Icon::Mail)) }
                    div { strong { "Member told" } span { @if s.dm_member { "A DM on how to get the timeout lifted" } @else { "No DM is sent" } } }
                }
            }
            @if page.can_edit() {
                div.gd-send {
                    div.gd-send-row {
                        form.inline method="post" action=(page.path("guard/test")) {
                            button.btn.small.outline type="submit" { (ui::icon(Icon::Bell)) "Send a test alert" }
                        }
                        @if honeypot.is_some() {
                            form.inline method="post" action=(page.path("guard/test")) {
                                input type="hidden" name="kind" value="honeypot";
                                button.btn.small.outline type="submit" { (ui::icon(Icon::Bell)) "Test a honeypot catch" }
                            }
                        }
                    }
                    p.muted.small {
                        @match &log {
                            Some(l) => { "Posts a sample alert in " (l) ", marked as a test. Nobody is timed out or banned." },
                            None => { "Pick an alert channel and save, then send a test." },
                        }
                    }
                }
            }
        }
        @if let Some(h) = &honeypot {
            div.rail-block {
                h2 { "Honeypot notice" }
                p.muted.small {
                    @if s.honeypot_notice.is_some_and(|(c, _)| Some(c) == s.honeypot_channel) {
                        "The bot's notice in " (h) " tells members not to write there and what happens if they do."
                    } @else {
                        "There's no notice in " (h) " yet, so members aren't told to stay out of it."
                    }
                }
                @if page.can_edit() {
                    form.inline method="post" action=(page.path("guard/notice")) {
                        button.btn.small.outline type="submit" {
                            (ui::icon(Icon::Shield))
                            @if s.honeypot_notice.is_some() { "Post it again" } @else { "Post the notice" }
                        }
                    }
                }
            }
        }
        div.rail-block {
            h2 { "Never checked" }
            @if exempt.is_empty() {
                p.muted.small { "Staff with Manage Messages or Administrator. Add roles or channels on the left." }
            } @else {
                div.gd-exempt { @for e in &exempt { span { (e) } } }
                p.muted.small { "Plus staff with Manage Messages or Administrator." }
            }
        }
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
        honeypot_notice: ctx.settings::<Settings>(guild, KEY)?.honeypot_notice,
    };
    if s.honeypot_channel.is_some() && s.honeypot_channel == s.log_channel {
        return Err(WebError::Bad("The honeypot and the alert channel can't be the same channel.".into()));
    }
    // A forum takes posts, not messages, so an alert can't be sent there.
    let forum = s.log_channel.and_then(Id::new_checked).and_then(|c| ctx.cache.channel(c)).is_some_and(|c| matches!(c.kind, ChannelType::GuildForum | ChannelType::GuildMedia));
    if forum {
        return Err(WebError::Bad("The alert channel can't be a forum. Pick a text channel.".into()));
    }
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed scam protection settings");
    // The notice in the honeypot channel follows the settings: posted, moved, edited or removed.
    // The preview has no Discord to post to.
    let mut s = s;
    let before = s.honeypot_notice;
    let synced = if ctx.cfg.preview { Ok(()) } else { crate::sync_notice(&ctx, &mut s, false).await };
    if s.honeypot_notice != before {
        ctx.save_settings(guild, KEY, &s)?;
    }
    if let Err(e) = synced {
        bot_core::tracing::warn!(guild, error = %e, "couldn't post the honeypot notice");
        let channel = s.honeypot_channel.map(|c| channel_name(&ctx, c)).unwrap_or_default();
        return Err(WebError::Bad(format!("Saved, but the bot couldn't post its notice in {channel}: {}", bot_core::discord::why_refused_any(&e))));
    }
    Ok(saved(&page.path("guard/settings")))
}

/// Post the honeypot notice again, as the newest message in the channel.
async fn post_notice(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let mut s = Settings::clone(&*ctx.settings::<Settings>(guild, KEY)?);
    let Some(channel) = s.honeypot_channel else {
        return Err(WebError::Bad("Pick a honeypot channel and save first.".into()));
    };
    if deleted(&ctx, guild, s.honeypot_channel) {
        return Err(WebError::Bad("The honeypot channel was deleted. Pick another one and save first.".into()));
    }
    let before = s.honeypot_notice;
    let synced = crate::sync_notice(&ctx, &mut s, true).await;
    if s.honeypot_notice != before {
        ctx.save_settings(guild, KEY, &s)?;
    }
    if let Err(e) = synced {
        bot_core::tracing::warn!(guild, error = %e, "couldn't post the honeypot notice");
        return Err(WebError::Bad(bot_core::discord::why_refused_any(&e)));
    }
    ctx.audit(&page.user, Some(guild), KEY, format!("Posted the honeypot notice in {}", channel_name(&ctx, channel)));
    Ok(Redirect::to(&format!("{}?posted=1", page.path("guard/settings"))).into_response())
}

/// Post a sample alert in the alert channel, as saved, so staff can see one without a scam.
async fn send_test(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let honeypot = form.string("kind") == "honeypot";
    if honeypot && (s.honeypot_channel.is_none() || deleted(&ctx, guild, s.honeypot_channel)) {
        return Err(WebError::Bad("Pick a honeypot channel and save first.".into()));
    }
    let Some(channel) = s.log_channel else {
        return Err(WebError::Bad("Pick an alert channel and save first.".into()));
    };
    if deleted(&ctx, guild, s.log_channel) {
        return Err(WebError::Bad("The alert channel was deleted. Pick another one and save first.".into()));
    }
    if let Err(e) = crate::send_test_alert(&ctx, &s, channel, honeypot, page.user.id, &page.user.name).await {
        bot_core::tracing::warn!(guild, error = %e, "test scam alert failed");
        return Err(WebError::Bad(bot_core::discord::why_refused_any(&e)));
    }
    let kind = if honeypot { "honeypot" } else { "scam" };
    ctx.audit(&page.user, Some(guild), KEY, format!("Sent a test {kind} alert to {}", channel_name(&ctx, channel)));
    Ok(Redirect::to(&format!("{}?sent={}", page.path("guard/settings"), if honeypot { "honeypot" } else { "1" })).into_response())
}

async fn release(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>, form: FormData) -> WebResult {
    act(ctx, user, guild, id, Outcome::Released, form.get("reason")).await
}

async fn ban(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>, form: FormData) -> WebResult {
    act(ctx, user, guild, id, Outcome::Banned, form.get("reason")).await
}

async fn act(ctx: Ctx, user: User, guild: u64, id: i64, outcome: Outcome, reason: Option<&str>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let c = crate::resolve(&ctx, guild, id, outcome, page.user.id, &page.user.name, reason, true).await.map_err(WebError::Bad)?;
    let mut what = match outcome {
        Outcome::Banned => format!("Banned {} from a scam catch", c.user_name),
        _ => format!("Released {} from a scam catch as a mistake", c.user_name),
    };
    if let Some(r) = &c.acted_reason {
        what.push_str(&format!(": {r}"));
    }
    ctx.audit(&page.user, Some(guild), KEY, what);
    Ok(saved(&page.path(KEY)))
}

/// Keep the stored name when the member is no longer cached.
fn member_name(ctx: &Ctx, guild: u64, user: u64, saved: &str) -> String {
    if saved.is_empty() || ctx.cache.user(Id::new(user)).is_some() {
        ctx.display_name(guild, user)
    } else {
        saved.to_owned()
    }
}

fn channel_name(ctx: &Ctx, channel: u64) -> String {
    match ctx.cache.channel(Id::new(channel)).and_then(|c| c.name.clone()) {
        Some(n) => format!("#{n}"),
        None => "#deleted-channel".into(),
    }
}

/// "Removed 1 message", "Removed 3 messages".
fn plural(n: i64, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{} {word}s", text::num(n)) }
}

/// The timeout's length as the settings name it ("1 day"), from when it started to when it ends.
fn timeout_for(at: i64, until: i64) -> String {
    let span = until - at;
    TIMEOUTS
        .iter()
        .find(|(m, _)| (*m * 60_000 - span).abs() < 120_000)
        .map(|(_, label)| (*label).to_owned())
        .unwrap_or_else(|| text::duration(span / 1000))
}

/// The catch cards, the Discord message, the dial and the rails. Classes are prefixed `gd-`.
fn style() -> Markup {
    html! { style { (PreEscaped(STYLE)) } }
}

const STYLE: &str = r#"
.gd-main { display: flex; flex-direction: column; gap: 26px; min-width: 0; }
.gd-needs { display: flex; flex-direction: column; gap: 8px; }

/* Readings: the week, with what waits on you in amber. */
.gd-readings { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 10px; }
.gd-readings .reading { min-width: 0; }
.gd-readings .gd-wait b { color: var(--waiting); }
.gd-readings .gd-wait::before { background: linear-gradient(90deg, var(--waiting), transparent); }

/* The list and its filter. */
.gd-catches { display: flex; flex-direction: column; gap: 14px; }
.gd-list-head { display: flex; align-items: flex-end; justify-content: space-between; gap: 12px 20px; flex-wrap: wrap; }
.gd-list-head h2 { font-size: 15px; }
.gd-list-head p { margin-top: 3px; max-width: 60ch; color: var(--ink-2); }
.gd-show a { text-decoration: none; }
.gd-count { color: var(--waiting); font-weight: 600; font-variant-numeric: tabular-nums; }
/* Catches sit two across once each still has room for the message beside its dial, so a wide
   screen shows more of them instead of one long card. One catch fills the row. */
.gd-list { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 600px), 1fr)); gap: 14px; }

/* One catch: the evidence on the left, the verdict in a column of its own. */
.gd-catch { position: relative; display: grid; grid-template-columns: minmax(0, 1fr) 168px; gap: 0 24px; padding: 18px 20px 18px 18px; border-radius: var(--r-card); background: linear-gradient(180deg, rgba(243, 242, 248, 0.04), rgba(243, 242, 248, 0.015)); box-shadow: inset 0 0 0 1px var(--edge-1); animation: rise 0.4s var(--ease) both; animation-delay: calc(var(--i, 0) * 45ms + 100ms); }
/* An open Release or Ban popover stays above the card under it. */
.gd-catch:has(.gd-pop[open]) { z-index: 4; }
.gd-catch.open { box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--waiting) 24%, transparent); }
.gd-what { display: flex; flex-direction: column; gap: 14px; min-width: 0; }

/* The message as Discord drew it. Discord's own colours: its chat grey, its name white. */
.gd-discord { border-radius: 12px; background: #313338; overflow: hidden; }
.gd-chan { display: flex; align-items: center; gap: 6px; height: 32px; padding: 0 14px 0 12px; background: #2b2d31; color: #b5bac1; font-size: 12.5px; font-weight: 600; }
.gd-chan .icon { width: 15px; height: 15px; color: #80848e; }
.gd-chan-tag { margin-left: 4px; color: #949ba4; font-weight: 500; }
/* The message itself is Discord's (web/dc.rs), a size down to sit in the card. */
.gd-discord .dp-msg { padding: 12px 16px 14px 14px; }
.gd-discord .dp-post { gap: 14px; font-size: 14px; }
.gd-discord .dp-line { line-height: 1.55; }
.gd-none { color: #949ba4; }
/* The bait: a red wash that sweeps across the words the score counted. */
.gd-discord mark { color: #ffe1e4; padding: 1px 3px; margin: 0 -1px; border-radius: 4px; background: linear-gradient(var(--gd-bait), var(--gd-bait)) no-repeat 0 0 / 0% 100%; --gd-bait: color-mix(in oklab, var(--held) 30%, transparent);-webkit-box-decoration-break: clone; box-decoration-break: clone; animation: gd-bait 0.5s var(--ease) forwards; animation-delay: calc(var(--i, 0) * 45ms + 380ms); }
@keyframes gd-bait { to { background-size: 100% 100%; } }

/* Why it was caught: the points in a column of their own, then the reason. Two columns once the
   card is wide enough for both to stay on one line. */
.gd-sigs { list-style: none; margin: 0; padding: 0; display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 300px), 1fr)); gap: 5px 24px; font-size: 13px; line-height: 1.4; }
.gd-sigs li { display: grid; grid-template-columns: 28px minmax(0, 1fr); align-items: baseline; color: var(--ink-2); }
.gd-sigs li.plain { grid-template-columns: minmax(0, 1fr); }
.gd-sigs b { font-weight: 600; color: var(--ink); font-variant-numeric: tabular-nums; }

/* What happened, on one line under a hairline. */
.gd-steps { list-style: none; margin: 0; padding: 12px 0 0; border-top: 1px solid var(--edge-1); display: flex; flex-wrap: wrap; align-items: center; gap: 8px 18px; font-size: 12.5px; color: var(--ink-2); }
.gd-step { display: inline-flex; align-items: center; gap: 6px; min-height: 22px; }
.gd-step > .icon { width: 14px; height: 14px; color: var(--ink-3); flex: none; }
.gd-step.bad, .gd-step.bad > .icon { color: var(--held); }
.gd-step > .avatar { width: 20px; height: 20px; }
.gd-step strong { color: var(--ink); font-weight: 600; }
.gd-waiting { color: var(--waiting); font-weight: 500; gap: 8px; }
.gd-waiting .dot::after { content: ""; position: absolute; inset: 0; border-radius: 50%; background: var(--waiting); animation: ping 2s ease-out infinite 1s; }
.gd-by { color: var(--ink-2); }
.gd-when { margin-left: 2px; color: var(--ink-3); }
.gd-why { font-size: 12.5px; color: var(--ink-2); overflow-wrap: anywhere; margin-top: -4px; }
.gd-why > span { color: var(--ink-3); margin-right: 6px; }

/* The verdict: the dial, the outcome, the decision. */
.gd-verdict { display: flex; flex-direction: column; align-items: center; gap: 14px; text-align: center; padding-top: 4px; }
.gd-out { display: flex; flex-direction: column; align-items: center; gap: 3px; }
.gd-state { display: inline-flex; align-items: center; gap: 7px; font-size: 13px; font-weight: 600; color: var(--ink); }
.gd-state.open .dot { background: var(--waiting); }
.gd-state.banned .dot { background: var(--held); }
.gd-state.released { color: var(--ink-2); }
.gd-age { font-size: 12px; color: var(--ink-3); }
.gd-age.new { color: var(--waiting); }
.gd-acts { position: relative; display: flex; flex-direction: column; gap: 6px; align-self: stretch; }
.gd-acts .gd-pop > summary { width: 100%; }
.gd-pop .pop-body { right: 0; left: auto; width: 320px; }
.gd-pop-foot { display: flex; align-items: center; justify-content: space-between; gap: 12px; text-align: left; }
.gd-pop-foot .btn.danger { background: var(--held); color: #1a0507; font-weight: 600; }
.gd-hp { display: flex; flex-direction: column; align-items: center; gap: 3px; padding: 4px 0 0; }
.gd-hp-ic { display: grid; place-items: center; width: 46px; height: 46px; margin-bottom: 6px; border-radius: 50%; color: var(--hue); box-shadow: inset 0 0 0 1.5px color-mix(in oklab, var(--hue) 55%, transparent); }
.gd-hp-ic .icon { width: 20px; height: 20px; }
.gd-hp strong { font-size: 14px; font-weight: 600; }
.gd-hp span:last-child { font-size: 12px; color: var(--ink-3); max-width: 18ch; }

/* The dial. */
.gd-gauge { position: relative; width: 140px; }
.gd-gauge.big { width: 184px; margin: 4px auto 0; }
.gd-gauge svg { display: block; width: 100%; height: auto; overflow: visible; }
.gd-track, .gd-arc { fill: none; stroke-width: 7; stroke-linecap: round; }
.gd-track { stroke: var(--edge-1); }
.gd-arc { stroke: var(--waiting); stroke-dasharray: 100 200; animation: gd-arc 0.9s var(--ease) both; animation-delay: calc(var(--i, 0) * 45ms + 200ms); }
.gd-gauge.caught .gd-arc { stroke: var(--held); }
.gd-tick { stroke: var(--ink-2); stroke-width: 2; stroke-linecap: round; }
@keyframes gd-arc { from { stroke-dashoffset: 100; } }
.gd-num { position: absolute; left: 0; right: 0; bottom: -2px; display: flex; flex-direction: column; align-items: center; }
.gd-num b { font: 600 28px/1 var(--sans); letter-spacing: -0.03em; font-variant-numeric: tabular-nums; color: var(--ink); }
.gd-num span { margin-top: 4px; font-size: 11.5px; color: var(--ink-3); }
.gd-gauge.big .gd-num b { font-size: 36px; }

/* The rail. */
.gd-axis { display: flex; justify-content: space-between; margin-top: -4px; color: var(--ink-3); font-size: 11.5px; }
.gd-handlers { display: flex; align-items: center; gap: 10px; }
.gd-handlers .avatar { width: 30px; height: 30px; }
.gd-handlers p { color: var(--ink-2); }
.gd-test textarea { min-height: 84px; }
.gd-result { display: flex; flex-direction: column; gap: 12px; padding-top: 4px; }
.gd-result-say { display: flex; flex-direction: column; align-items: center; gap: 6px; text-align: center; }
.gd-tsigs { display: flex; flex-direction: column; }
.gd-tsig { display: grid; grid-template-columns: 28px minmax(0, 1fr); gap: 6px 8px; align-items: baseline; padding: 9px 0; font-size: 13px; border-top: 1px solid var(--edge-1); }
.gd-tsig b { font-weight: 600; color: var(--ink); font-variant-numeric: tabular-nums; }
.gd-tsig > span:not(.meter) { color: var(--ink-2); }
.gd-tsig .meter { grid-column: 2; height: 4px; }
.gd-tsig .meter > span { background: var(--data); animation-delay: calc(var(--s, 0) * 60ms + 150ms); }

/* Settings: what a catch does, in order, on a thin line. */
.gd-flow { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; position: relative; }
.gd-flow::before { content: ""; position: absolute; left: 14px; top: 22px; bottom: 22px; width: 1px; background: var(--edge-2); }
.gd-flow li { position: relative; display: flex; gap: 12px; align-items: flex-start; padding: 7px 0; }
.gd-flow-ic { display: grid; place-items: center; width: 29px; height: 29px; flex: none; border-radius: 50%; color: var(--hue); background: var(--raised); box-shadow: inset 0 0 0 1px var(--edge-2); }
.gd-flow-ic .icon { width: 14px; height: 14px; }
.gd-flow li div { display: flex; flex-direction: column; gap: 1px; padding-top: 4px; min-width: 0; }
.gd-flow strong { font-size: 13.5px; font-weight: 600; }
.gd-flow li > div > span { font-size: 12.5px; color: var(--ink-2); }
.gd-flow li.muted .gd-flow-ic { color: var(--ink-3); }
.gd-flow li.muted strong { color: var(--ink-2); }
.gd-flow li.muted > div > span { color: var(--ink-3); }
.gd-flow.off .gd-flow-ic { color: var(--ink-3); }
.gd-send { display: flex; flex-direction: column; align-items: flex-start; gap: 8px; padding-top: 4px; }
.gd-send-row { display: flex; flex-wrap: wrap; gap: 8px; }
.gd-exempt { display: flex; flex-wrap: wrap; gap: 6px; }
.gd-exempt span { display: inline-flex; align-items: center; height: 26px; padding: 0 10px; border-radius: 13px; background: var(--control); color: var(--ink-2); font-size: 12.5px; }

/* Settings: the sections in two columns once each column can hold a label beside its control. */
.gd-sets { display: grid; grid-template-columns: minmax(0, 1fr); gap: 18px 24px; align-items: start; }
.gd-set-col { display: flex; flex-direction: column; gap: 18px; min-width: 0; }
@container col (min-width: 1060px) {
  .gd-sets { grid-template-columns: repeat(2, minmax(0, 1fr)); }
}

/* Coming back to the page after a release, a ban or another filter: no entrance again. */
html.same .gd-catch, html.same .gd-arc, html.same .gd-discord mark { animation: none; }
html.same .gd-discord mark { background-size: 100% 100%; }

/* Laid out for the column's own width, so the sidebar and rail are already counted. */
@container col (max-width: 700px) {
  .gd-readings { grid-template-columns: repeat(2, minmax(0, 1fr)); }
}
@media (max-width: 920px) {
  .row.gd-strict-row:has(> .row-control > .seg) { grid-template-columns: minmax(0, 1fr); }
  .gd-strict-row .row-control { justify-content: flex-start; }
}
@container col (max-width: 599px) {
  .gd-catch { grid-template-columns: minmax(0, 1fr); padding: 14px; }
  .gd-verdict { flex-direction: row; flex-wrap: wrap; align-items: center; justify-content: flex-start; text-align: left; gap: 12px 16px; margin-top: 14px; padding-top: 14px; border-top: 1px solid var(--edge-1); }
  .gd-gauge { width: 108px; }
  .gd-gauge .gd-num b { font-size: 23px; }
  .gd-out { align-items: flex-start; }
  .gd-acts { flex-direction: row; flex-wrap: wrap; flex-basis: 100%; }
  .gd-acts .gd-pop { position: static; }
  .gd-pop .pop-body { left: 0; right: 0; width: auto; }
  .gd-discord .dp-msg { padding: 10px 12px 12px 10px; }
  .gd-discord .dp-post { gap: 10px; font-size: 13.5px; }
  .gd-discord .dp-av { width: 34px; height: 34px; }
  .gd-hp { flex-direction: row; gap: 10px; padding: 0; }
  .gd-hp-ic { width: 38px; height: 38px; margin: 0; }
  .gd-hp span:last-child { display: none; }
  .gd-list-head { align-items: flex-start; }
}
"#;
