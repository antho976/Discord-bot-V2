use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::twilight_model::id::Id;
use bot_core::web::ui::{self, Tone};
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
            div.reading.gd-lead {
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
    let class = match c.outcome {
        Outcome::TimedOut | Outcome::NotTimedOut => "gd-catch open",
        Outcome::Released => "gd-catch released",
        Outcome::Banned => "gd-catch banned",
    };
    let tone = match c.outcome {
        Outcome::TimedOut | Outcome::NotTimedOut => Tone::Warn,
        Outcome::Released => Tone::Muted,
        Outcome::Banned => Tone::Bad,
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
                        @if c.deleted > 0 { span.gd-gone { (ui::icon(Icon::Trash)) "Deleted by the bot" } }
                    }
                    div.gd-dmsg {
                        img.avatar src=(ctx.member_avatar(guild, c.user_id)) alt="" width="40" height="40" loading="lazy";
                        div.gd-dbody {
                            div.gd-dhead {
                                strong { (name) }
                                time.gd-dtime datetime=(c.at) title=(page.user.local(c.at).strftime("%b %-d, %H:%M").to_string()) { (discord_time(page, c.at)) }
                            }
                            div.gd-dtext {
                                @if shown.trim().is_empty() {
                                    em.gd-none { "No text, attachments only" }
                                } @else {
                                    (message(shown, &c.signals))
                                    @if cut { "…" }
                                }
                            }
                        }
                    }
                }
                @if !c.signals.is_empty() {
                    div.gd-sigs aria-label="Why it was caught" {
                        @for s in &c.signals {
                            span.gd-sig { @if s.weight > 0 { b { "+" (s.weight) } } (s.text) }
                        }
                    }
                }
                ol.gd-steps aria-label="What happened" {
                    (steps(ctx, page, guild, c))
                }
                @if let Some(reason) = &c.acted_reason {
                    p.gd-why { span.muted { "Reason " } (reason) }
                }
            }
            div.gd-verdict {
                @if c.honeypot {
                    div.gd-hp {
                        span.gd-hp-ic { (ui::icon(Icon::Hash)) }
                        strong { "Honeypot" }
                        span.muted.small { "Anything written there is a catch" }
                    }
                } @else {
                    (gauge(c.score, threshold, false))
                }
                div.gd-out {
                    (ui::badge(c.outcome.label(), tone))
                    span.muted.small.gd-age.new[young] { "Account made " (crate::account_made(c.user_id)) }
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

/// What the bot did about the catch, then who decided, as steps that arrive one after another.
fn steps(ctx: &Ctx, page: &Page, guild: u64, c: &Catch) -> Markup {
    let mut n = 0usize;
    let mut next = || {
        n += 1;
        format!("--s:{n}")
    };
    html! {
        @if c.deleted > 0 {
            li.gd-step style=(next()) { span.gd-ic { (ui::icon(Icon::Trash)) } "Removed " (plural(c.deleted, "message")) }
        }
        @if c.outcome == Outcome::NotTimedOut {
            li.gd-step.bad style=(next()) { span.gd-ic { (ui::icon(Icon::Clock)) } "Couldn't time them out" }
        } @else if let Some(until) = c.timeout_until {
            li.gd-step style=(next()) { span.gd-ic { (ui::icon(Icon::Clock)) } "Timed out for " (timeout_for(c.at, until)) }
        }
        @if let Some((channel, _)) = c.alert {
            li.gd-step style=(next()) { span.gd-ic { (ui::icon(Icon::Bell)) } "Alerted " (channel_name(ctx, channel)) }
        }
        (decided(ctx, page, guild, c, &next()))
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
fn decided(ctx: &Ctx, page: &Page, guild: u64, c: &Catch, style: &str) -> Markup {
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
    let class = match c.outcome {
        Outcome::Banned => "gd-step gd-by banned",
        _ => "gd-step gd-by",
    };
    html! {
        @if c.outcome.open() {
            li.gd-step.gd-waiting style=(style) {
                span.dot.waiting {}
                @if page.can_edit() { "Waiting on you" } @else { "Waiting on a moderator" }
            }
        } @else {
            li class=(class) style=(style) {
                @if let (Some(id), Some(by)) = (c.acted_by, &c.acted_by_name) {
                    img.avatar src=(ctx.member_avatar(guild, id)) alt="" width="20" height="20" loading="lazy";
                    span { strong { (member_name(ctx, guild, id, by)) } " " (verb) @if let Some(w) = &when { span.muted { " · " (w) } } }
                } @else {
                    span.gd-ic { (ui::icon(if c.outcome == Outcome::Banned { Icon::Close } else { Icon::Check })) }
                    span { (nameless) @if let Some(w) = &when { span.muted { " · " (w) } } }
                }
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
    let (t1, t2) = (at(line, 38.0), at(line, 58.0));
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
                            (ui::badge("Would catch", Tone::Bad))
                        } @else {
                            (ui::badge("Not caught", Tone::Muted))
                        }
                        p.muted.small { "The tick is where a message gets caught at " (s.strictness.key()) " strictness." }
                    }
                    @if v.signals.is_empty() {
                        p.muted.small { "Nothing suspicious in it." }
                    } @else {
                        div.gd-tsigs {
                            @for (i, sig) in v.signals.iter().enumerate() {
                                div.gd-tsig style={ "--s:" (i) } {
                                    b.mono { "+" (sig.weight) }
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

async fn settings_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
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
        (ui::field_channel(&ctx, guild, "honeypot_channel", "Honeypot channel", Some("Tell members not to post there, for example in the channel topic."), s.honeypot_channel))
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
        (tabs(&page, waiting, "Settings"))
        div.with-rail {
            (ui::settings_form(&page, &page.path("guard/settings"), html! {
                // Two columns of sections once the column is wide enough. Stacked, they keep this order.
                div.gd-sets {
                    div.gd-set-col { (detection) (honeypot) }
                    div.gd-set-col { (caught) (never) }
                }
            }))
            aside.rail { (flow(&ctx, guild, &s)) }
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

/// What happens to someone who is caught, step by step, as the saved settings have it.
fn flow(ctx: &Ctx, guild: u64, s: &Settings) -> Markup {
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
        }
        div.rail-block {
            h2 { "Never checked" }
            @if exempt.is_empty() {
                p.muted.small { "Staff with Manage Messages or Administrator. Add roles or channels on the left." }
            } @else {
                div.gd-exempt { @for e in &exempt { span.gd-sig { (e) } } }
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
    };
    if s.honeypot_channel.is_some() && s.honeypot_channel == s.log_channel {
        return Err(WebError::Bad("The honeypot and the alert channel can't be the same channel.".into()));
    }
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed scam protection settings");
    Ok(saved(&page.path("guard/settings")))
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

/* Readings: the week, with what waits on you lit amber. */
.gd-readings { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 10px; }
.gd-readings .reading { min-width: 0; }
.gd-readings .gd-lead { background: var(--well); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--hue) 30%, transparent); }
.gd-readings .gd-lead::before { width: 100%; opacity: 1; }
.gd-readings .gd-wait b { color: var(--waiting); }
.gd-readings .gd-wait::before { background: linear-gradient(90deg, var(--waiting), transparent); }

/* The list and its filter. */
.gd-catches { display: flex; flex-direction: column; gap: 14px; }
.gd-list-head { display: flex; align-items: flex-end; justify-content: space-between; gap: 12px 20px; flex-wrap: wrap; }
.gd-list-head h2 { font-size: 15px; }
.gd-list-head p { margin-top: 3px; max-width: 60ch; }
.gd-show a { text-decoration: none; }
.gd-count { color: var(--waiting); font-family: var(--mono); font-weight: 600; }
/* Catches sit two across once each still has room for the message beside its dial, so a wide
   screen shows more of them instead of one long card. One catch fills the row. */
.gd-list { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 600px), 1fr)); gap: 14px; }

/* One catch. */
.gd-catch { position: relative; display: grid; grid-template-columns: minmax(0, 1fr) 176px; gap: 18px 22px; padding: 16px 18px 18px 16px; border-radius: var(--r-card); background: linear-gradient(180deg, rgba(243, 242, 248, 0.045), rgba(243, 242, 248, 0.015)); box-shadow: inset 0 0 0 1px var(--edge-1); animation: rise 0.6s var(--ease) both; animation-delay: calc(var(--i, 0) * 90ms + 180ms); transition: box-shadow 0.25s, transform 0.35s var(--ease); }
.gd-catch:hover { box-shadow: inset 0 0 0 1px var(--edge-2), var(--shadow-lift); }
/* An open Release or Ban popover stays above the card under it. */
.gd-catch:has(.gd-pop[open]) { z-index: 4; }
.gd-catch.open { background: linear-gradient(180deg, rgba(243, 242, 248, 0.05), rgba(243, 242, 248, 0.015)); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--waiting) 32%, transparent); }
.gd-what { display: flex; flex-direction: column; gap: 12px; min-width: 0; }

/* The message as Discord drew it. Discord's own colours: its chat grey, its name white. */
.gd-discord { border-radius: 12px; background: #313338; overflow: hidden; box-shadow: 0 14px 30px -18px rgba(0, 0, 0, 0.9), inset 0 0 0 1px rgba(255, 255, 255, 0.05); }
.gd-chan { display: flex; align-items: center; gap: 6px; height: 34px; padding: 0 14px 0 12px; background: #2b2d31; border-bottom: 1px solid rgba(0, 0, 0, 0.28); color: #b5bac1; font-size: 12.5px; font-weight: 600; }
.gd-chan .icon { width: 15px; height: 15px; color: #80848e; }
.gd-chan-tag { height: 18px; padding: 0 7px; border-radius: 9px; font: 600 10.5px/18px var(--sans); background: color-mix(in oklab, var(--hue) 22%, transparent); color: var(--hue); }
.gd-gone { margin-left: auto; display: inline-flex; align-items: center; gap: 5px; color: #f57f8b; font-weight: 500; font-size: 11.5px; }
.gd-gone .icon { width: 13px; height: 13px; color: inherit; }
.gd-dmsg { display: grid; grid-template-columns: 40px minmax(0, 1fr); gap: 14px; padding: 12px 16px 14px 14px; }
.gd-dmsg > .avatar { width: 40px; height: 40px; }
.gd-dbody { min-width: 0; }
.gd-dhead { display: flex; align-items: baseline; gap: 8px; flex-wrap: wrap; }
.gd-dhead strong { color: #f2f3f5; font-weight: 600; font-size: 14.5px; }
.gd-dtime { color: #949ba4; font-size: 11.5px; }
.gd-dtext { margin-top: 2px; color: #dbdee1; font-size: 14px; line-height: 1.5; white-space: pre-wrap; overflow-wrap: anywhere; }
.gd-dtext .gd-none { color: #949ba4; }
/* The bait: a red wash that sweeps across once the card has landed, over a wavy underline. */
.gd-dtext mark { color: #ffe3e6; padding: 1px 2px; margin: 0 -1px; border-radius: 3px; background: linear-gradient(90deg, color-mix(in oklab, var(--held) 34%, transparent), color-mix(in oklab, var(--held) 22%, transparent)) no-repeat 0 0 / 0% 100%; -webkit-box-decoration-break: clone; box-decoration-break: clone; text-decoration: underline wavy color-mix(in oklab, var(--held) 85%, transparent); text-decoration-thickness: 1.5px; text-underline-offset: 4px; text-decoration-skip-ink: none; animation: gd-bait 0.8s var(--ease) forwards; animation-delay: calc(var(--i, 0) * 90ms + 620ms); }
@keyframes gd-bait { to { background-size: 100% 100%; } }

/* Why it was caught. */
.gd-sigs, .gd-exempt { display: flex; flex-wrap: wrap; gap: 6px; }
.gd-sig { display: inline-flex; align-items: center; gap: 7px; min-height: 26px; padding: 3px 10px 3px 4px; border-radius: 13px; background: var(--control); color: var(--ink-2); font-size: 12px; line-height: 1.3; }
.gd-sig b { flex: none; font: 600 11px/18px var(--mono); padding: 0 6px; border-radius: 9px; background: color-mix(in oklab, var(--hue) 20%, transparent); color: var(--hue); }
.gd-sig:not(:has(b)) { padding-left: 10px; }

/* What happened, one step after another. */
.gd-steps { list-style: none; margin: 0; padding: 0; display: flex; flex-wrap: wrap; align-items: center; gap: 6px; font-size: 12.5px; color: var(--ink-2); }
.gd-step { position: relative; display: inline-flex; align-items: center; gap: 7px; min-height: 30px; padding: 4px 12px 4px 5px; border-radius: 15px; background: var(--well); box-shadow: inset 0 0 0 1px var(--edge-1); animation: gd-step 0.5s var(--spring) both; animation-delay: calc(var(--i, 0) * 90ms + var(--s, 0) * 120ms + 560ms); }
.gd-ic { display: grid; place-items: center; width: 21px; height: 21px; flex: none; border-radius: 50%; background: color-mix(in oklab, var(--hue) 16%, transparent); color: var(--hue); }
.gd-ic .icon { width: 12px; height: 12px; }
.gd-step.bad { color: var(--held); }
.gd-step.bad .gd-ic { background: color-mix(in oklab, var(--held) 18%, transparent); color: var(--held); }
.gd-step > .avatar { width: 21px; height: 21px; }
.gd-step strong { color: var(--ink); font-weight: 600; }
.gd-waiting { color: var(--waiting); font-weight: 500; padding-left: 12px; background: color-mix(in oklab, var(--waiting) 10%, transparent); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--waiting) 30%, transparent); }
.gd-waiting .dot::after { content: ""; position: absolute; inset: 0; border-radius: 50%; background: var(--waiting); animation: ping 2s ease-out infinite 1.6s; }
.gd-by.banned { box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--held) 26%, transparent); }
.gd-by .gd-ic { background: var(--control); color: var(--ink-2); }
@keyframes gd-step { from { opacity: 0; transform: translateX(-6px) scale(0.92); } }
.gd-why { font-size: 12.5px; color: var(--ink-2); overflow-wrap: anywhere; }

/* The verdict: the dial, the outcome, the decision. */
.gd-verdict { display: flex; flex-direction: column; align-items: center; gap: 12px; text-align: center; padding-top: 2px; }
.gd-out { display: flex; flex-direction: column; align-items: center; gap: 5px; }
.gd-age.new { color: var(--waiting); }
.gd-acts { position: relative; display: flex; flex-direction: column; gap: 6px; align-self: stretch; }
.gd-acts .gd-pop > summary { width: 100%; }
.gd-pop .pop-body { right: 0; left: auto; width: 320px; }
.gd-pop-foot { display: flex; align-items: center; justify-content: space-between; gap: 12px; text-align: left; }
.gd-pop-foot .btn.danger { background: var(--held); color: #1a0507; font-weight: 600; }
.gd-hp { display: flex; flex-direction: column; align-items: center; gap: 4px; padding: 6px 0 2px; }
.gd-hp-ic { display: grid; place-items: center; width: 52px; height: 52px; margin-bottom: 4px; border-radius: 16px; color: #0b0b10; background: linear-gradient(150deg, color-mix(in oklab, var(--hue), white 25%), var(--hue) 60%, color-mix(in oklab, var(--hue), black 20%)); box-shadow: none; animation: pop-in 0.6s var(--spring) both; animation-delay: calc(var(--i, 0) * 90ms + 500ms); }
.gd-hp-ic .icon { width: 24px; height: 24px; }
.gd-hp strong { font-size: 14px; font-weight: 600; }

/* The dial. */
.gd-gauge { position: relative; width: 150px; }
.gd-gauge.big { width: 196px; margin: 4px auto 0; }
.gd-gauge svg { display: block; width: 100%; height: auto; overflow: visible; }
.gd-track, .gd-arc { fill: none; stroke-width: 9; stroke-linecap: round; }
.gd-track { stroke: var(--edge-1); }
.gd-arc { stroke: var(--waiting); stroke-dasharray: 100 200; animation: gd-arc 1.4s var(--ease) both; animation-delay: calc(var(--i, 0) * 90ms + 420ms); }
.gd-gauge.caught .gd-arc { stroke: var(--held); }
.gd-tick { stroke: var(--ink); stroke-width: 2.5; stroke-linecap: round; }
@keyframes gd-arc { from { stroke-dashoffset: 100; } }
.gd-num { position: absolute; left: 0; right: 0; bottom: -3px; display: flex; flex-direction: column; align-items: center; }
.gd-num b { font: 650 30px/1 var(--sans); letter-spacing: -0.035em; font-variant-numeric: tabular-nums; }
.gd-gauge.caught .gd-num b { color: #ffd9dd; }
.gd-num span { margin-top: 4px; font-size: 11px; color: var(--ink-3); }
.gd-gauge.big .gd-num b { font-size: 40px; }

/* The rail. */
.gd-axis { display: flex; justify-content: space-between; margin-top: -4px; color: var(--ink-3); font: 11px var(--mono); }
.gd-handlers { display: flex; align-items: center; gap: 10px; }
.gd-handlers .avatar { width: 30px; height: 30px; }
.gd-handlers p { color: var(--ink-2); }
.gd-test textarea { min-height: 84px; }
.gd-result { display: flex; flex-direction: column; gap: 12px; padding-top: 4px; animation: rise 0.5s var(--ease) both; }
.gd-result-say { display: flex; flex-direction: column; align-items: center; gap: 6px; text-align: center; }
.gd-tsigs { display: flex; flex-direction: column; }
.gd-tsig { display: grid; grid-template-columns: 34px minmax(0, 1fr); gap: 4px 10px; align-items: baseline; padding: 9px 0; font-size: 13px; border-top: 1px solid var(--edge-1); animation: rise 0.45s var(--ease) both; animation-delay: calc(var(--s, 0) * 80ms + 300ms); }
.gd-tsig b { font-weight: 600; color: var(--hue); }
.gd-tsig > span:not(.meter) { color: var(--ink-2); }
.gd-tsig .meter { grid-column: 2; height: 4px; }
.gd-tsig .meter > span { background: linear-gradient(90deg, color-mix(in oklab, var(--hue), black 20%), var(--hue)); box-shadow: none; animation-delay: calc(var(--s, 0) * 80ms + 450ms); }

/* Settings: what a catch does, in order. */
.gd-flow { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; position: relative; }
.gd-flow::before { content: ""; position: absolute; left: 17px; top: 20px; bottom: 20px; width: 2px; border-radius: 1px; background: linear-gradient(180deg, var(--hue), color-mix(in oklab, var(--hue) 15%, transparent)); transform-origin: top; animation: grow 1.1s var(--ease) both 0.3s; }
.gd-flow li { position: relative; display: flex; gap: 12px; align-items: flex-start; padding: 8px 0; animation: rise 0.5s var(--ease) both; animation-delay: calc(var(--s, 0) * 110ms + 250ms); }
.gd-flow-ic { display: grid; place-items: center; width: 36px; height: 36px; flex: none; border-radius: 12px; color: #0b0b10; background: var(--hue); box-shadow: 0 0 0 4px var(--raised); }
.gd-flow-ic .icon { width: 17px; height: 17px; }
.gd-flow li div { display: flex; flex-direction: column; gap: 1px; padding-top: 1px; min-width: 0; }
.gd-flow strong { font-size: 13.5px; font-weight: 600; }
.gd-flow span { font-size: 12.5px; color: var(--ink-3); }
.gd-flow li.muted .gd-flow-ic { color: var(--hue); background: color-mix(in oklab, var(--hue) 14%, var(--raised)); box-shadow: 0 0 0 4px var(--raised); }
.gd-flow li.muted strong { color: var(--ink-2); }
.gd-flow.off .gd-flow-ic { filter: grayscale(1); opacity: 0.6; }

/* Settings: the sections in two columns once each column can hold a label beside its control. */
.gd-sets { display: grid; grid-template-columns: minmax(0, 1fr); gap: 18px 24px; align-items: start; }
.gd-set-col { display: flex; flex-direction: column; gap: 18px; min-width: 0; }
@container col (min-width: 1060px) {
  .gd-sets { grid-template-columns: repeat(2, minmax(0, 1fr)); }
}

/* Laid out for the column's own width, so the sidebar and rail are already counted. */
@container col (max-width: 700px) {
  .gd-readings { grid-template-columns: repeat(2, minmax(0, 1fr)); }
}
@media (max-width: 920px) {
  .row.gd-strict-row:has(> .row-control > .seg) { grid-template-columns: minmax(0, 1fr); }
  .gd-strict-row .row-control { justify-content: flex-start; }
}
@container col (max-width: 599px) {
  .gd-catch { grid-template-columns: minmax(0, 1fr); padding: 12px; }
  .gd-verdict { flex-direction: row; flex-wrap: wrap; align-items: center; justify-content: flex-start; text-align: left; gap: 12px 16px; padding-top: 0; border-top: 1px solid var(--edge-1); padding-top: 14px; }
  .gd-gauge { width: 112px; }
  .gd-gauge .gd-num b { font-size: 24px; }
  .gd-out { align-items: flex-start; }
  .gd-acts { flex-direction: row; flex-wrap: wrap; flex-basis: 100%; }
  .gd-acts .gd-pop { position: static; }
  .gd-pop .pop-body { left: 0; right: 0; width: auto; }
  .gd-dmsg { grid-template-columns: 34px minmax(0, 1fr); gap: 10px; padding: 10px 12px 12px 10px; }
  .gd-dmsg > .avatar { width: 34px; height: 34px; }
  .gd-dtext { font-size: 13.5px; }
  .gd-hp { flex-direction: row; gap: 10px; }
  .gd-hp-ic { width: 40px; height: 40px; margin: 0; }
  .gd-hp .muted { display: none; }
  .gd-list-head { align-items: flex-start; }
}
"#;
