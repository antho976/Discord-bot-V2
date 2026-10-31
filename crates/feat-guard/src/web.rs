use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::twilight_model::id::Id;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, text};
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

fn tabs(page: &Page, active: &str) -> Markup {
    ui::tabs(&[("Catches", page.path(KEY)), ("Settings", page.path("guard/settings"))], active)
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

    let body = html! {
        (style())
        (tabs(&page, "Catches"))
        div.with-rail {
            div.gd-main {
                (readings(&ctx, guild, &week))
                @let log_gone = deleted(&ctx, guild, s.log_channel);
                @let honeypot_gone = deleted(&ctx, guild, s.honeypot_channel);
                @if s.log_channel.is_none() {
                    p.muted.small { "No log channel yet, so catches only show up here and behind the bell. Pick one on the " a href=(page.path("guard/settings")) { "Settings tab" } "." }
                }
                @if log_gone || honeypot_gone {
                    div.gd-warns {
                        @if log_gone {
                            p.gd-warn { "The alert channel was deleted, so catches only show up here and behind the bell. Pick another on the " a href=(page.path("guard/settings")) { "Settings tab" } "." }
                        }
                        @if honeypot_gone {
                            p.gd-warn { "The honeypot channel was deleted, so nothing is caught there. Pick another on the " a href=(page.path("guard/settings")) { "Settings tab" } "." }
                        }
                    }
                }
                (ui::section("Recent catches", Some("Release lifts the timeout and marks the catch as a mistake. Ban also deletes their messages from the last day."), html! {
                    nav.seg.gd-show aria-label="Show" {
                        @for sh in Show::ALL {
                            a.on[sh == show] href={ (page.path(KEY)) @if !sh.key().is_empty() { "?show=" (sh.key()) } } aria-current=[(sh == show).then_some("true")] {
                                (sh.label())
                                @if sh == Show::Waiting && waiting > 0 { " " span.gd-count { (waiting) } }
                            }
                        }
                    }
                    @if catches.is_empty() {
                        @let (title, line) = show.empty();
                        (ui::empty(Icon::Shield, title, line))
                    } @else {
                        div.gd-list { @for c in &catches { (catch_row(&ctx, &page, guild, c)) } }
                    }
                }))
            }
            aside.rail {
                (test_rail(&page, &s, Some(show.key()).filter(|k| !k.is_empty()), test.as_deref(), q.new.is_some(), q.repeat.is_some(), if test.is_some() { q.ads.is_some() } else { s.catch_ads }))
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Scam protection", Some(status(&ctx, guild, &s)), body).into_response())
}

/// The week's numbers, and the seven-day chart in the alert colour.
fn readings(ctx: &Ctx, guild: u64, week: &Week) -> Markup {
    let max = week.daily.iter().copied().max().unwrap_or(0).max(1);
    let urls: Vec<String> = week.handled_by.iter().map(|u| ctx.member_avatar(guild, *u)).collect();
    html! {
        div.gd-top {
            div.readings {
                (reading(&text::num(week.caught), "Caught this week"))
                (reading(&text::num(week.banned), "Banned"))
                (reading(&text::num(week.released), "Released as mistakes"))
                (reading(&text::num(week.removed), "Messages removed"))
                div.reading {
                    @if urls.is_empty() { b.gd-none { "No one yet" } } @else { (ui::avatars(&urls, 3)) }
                    span { "Handled by" }
                }
            }
            figure.gd-bars {
                svg viewBox="0 0 67 36" role="img" aria-label="Catches in the last seven days" {
                    @for (i, n) in week.daily.iter().enumerate() {
                        @let h = if *n > 0 { (*n as f64 / max as f64 * 30.0).max(3.0) } else { 2.0 };
                        rect class=(if i == 6 { "now" } else { "" }) x=(i * 10) y=(36.0 - h) width="7" height=(h) rx="1.5" {
                            title { (day_label(i)) ": " (n) }
                        }
                    }
                }
                figcaption { "Catches, last 7 days" }
            }
        }
    }
}

fn reading(value: &str, label: &str) -> Markup {
    html! { div.reading { b.mono { (value) } span { (label) } } }
}

/// The label for one of the seven bars, counted back from the last 24 hours.
fn day_label(i: usize) -> String {
    if i == 6 { "Last 24 hours".to_owned() } else { format!("{} to {} days ago", 6 - i, 7 - i) }
}

fn catch_row(ctx: &Ctx, page: &Page, guild: u64, c: &Catch) -> Markup {
    let tone = match c.outcome {
        Outcome::TimedOut | Outcome::NotTimedOut => Tone::Warn,
        Outcome::Released => Tone::Muted,
        Outcome::Banned => Tone::Bad,
    };
    let name = member_name(ctx, guild, c.user_id, &c.user_name);
    let (shown, cut) = shown_part(&c.content);
    html! {
        article.gd-catch {
            img.avatar.gd-av src=(ctx.member_avatar(guild, c.user_id)) alt="" width="36" height="36" loading="lazy";
            div.gd-head {
                strong { (name) }
                span.muted.small { "account made " (crate::account_made(c.user_id)) " · " (text::ago(c.at)) }
            }
            div.gd-out { (ui::badge(c.outcome.label(), tone)) }
            div.gd-body {
                @if !c.signals.is_empty() {
                    div.gd-chips { @for s in &c.signals { (ui::badge(&s.text, Tone::Muted)) } }
                }
                div.gd-msg {
                    @if shown.trim().is_empty() {
                        span.muted { "No text, attachments only" }
                    } @else {
                        (message(shown, &c.signals))
                        @if cut { "…" }
                    }
                }
                div.gd-did { (did(ctx, c)) }
                @if page.can_edit() && c.outcome.open() {
                    div.gd-actions {
                        (decide_pop(page, c, Outcome::Released))
                        (decide_pop(page, c, Outcome::Banned))
                    }
                }
                (decided(ctx, guild, c))
            }
        }
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

/// The caught message, with the parts the stored signals matched underlined in red. Defanged
/// like the staff alert, so a link in it can't be clicked.
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

/// What the bot did about the catch.
fn did(ctx: &Ctx, c: &Catch) -> Markup {
    html! {
        @if c.deleted > 0 {
            span { (ui::icon(Icon::Trash)) (format!("Removed {}", plural(c.deleted, "message"))) }
        }
        @if c.outcome == Outcome::NotTimedOut {
            span { (ui::icon(Icon::Clock)) "Couldn't time them out" }
        } @else {
            @if let Some(until) = c.timeout_until {
                span { (ui::icon(Icon::Clock)) "Timed out for " (timeout_for(c.at, until)) }
            }
        }
        @if let Some((channel, _)) = c.alert {
            span { (ui::icon(Icon::Bell)) "Alerted " (channel_name(ctx, channel)) }
        }
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
                        button class=(class) type="submit" { (label) }
                    }
                }
            }
        }
    }
}

/// Who released or banned the member, when, where and why.
fn decided(ctx: &Ctx, guild: u64, c: &Catch) -> Markup {
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
    html! {
        @if c.outcome.open() {
            p.muted.small { "Nobody has decided yet." }
        } @else {
            div.gd-decided {
                @if let (Some(id), Some(by)) = (c.acted_by, &c.acted_by_name) {
                    img.avatar src=(ctx.member_avatar(guild, id)) alt="" width="18" height="18" loading="lazy";
                    span { strong { (member_name(ctx, guild, id, by)) } " " (verb) @if let Some(w) = &when { span.muted { " · " (w) } } }
                } @else {
                    span.muted { (nameless) @if let Some(w) = &when { " · " (w) } }
                }
            }
            @if let Some(reason) = &c.acted_reason {
                p.gd-why { (reason) }
            }
        }
    }
}

/// The score, the threshold on a bar, and each signal with its points.
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
        h2 { "Test a message" }
        p.muted.small { "See how the bot would score it. Nothing is posted or stored." }
        form.stack id="test" method="get" action={ (page.path(KEY)) "#test" } {
            @if let Some(show) = show { input type="hidden" name="show" value=(show); }
            textarea.input name="test" rows="4" placeholder="free nitro for everyone https://..." { (test.unwrap_or_default()) }
            div.chips {
                @if checks_age {
                    label.chip { input type="checkbox" name="new" value="1" checked[new]; span { "From a new account" } }
                }
                label.chip { input type="checkbox" name="repeat" value="1" checked[repeat]; span { "Posted in 3 channels at once" } }
                label.chip { input type="checkbox" name="ads" value="1" checked[ads]; span { "Count ads" } }
            }
            div.inline-form { button.btn type="submit" { "Score it" } }
        }
        @if let Some(v) = &result {
            @let scale = v.score.max(20).max(threshold);
            div.stack style="gap:10px" {
                div.gd-score {
                    span { "Score " b.mono { (v.score) } span.muted { " · catches at " (threshold) } }
                    @if v.caught(s.strictness) {
                        (ui::badge("Would catch", Tone::Bad))
                    } @else {
                        (ui::badge("Not caught", Tone::Muted))
                    }
                }
                div.gd-gauge role="img" aria-label={ "Score " (v.score) ", catches at " (threshold) } {
                    span.gd-fill.over[v.caught(s.strictness)] style={ "width:" (v.score.clamp(0, scale) * 100 / scale) "%" } {}
                    span.gd-mark style={ "left:" (threshold * 100 / scale) "%" } {}
                }
                p.muted.small { "The white line is where a message gets caught." }
                @if v.signals.is_empty() {
                    p.muted.small { "Nothing suspicious in it." }
                } @else {
                    div.gd-sigs {
                        @for sig in &v.signals {
                            div.gd-sig { b.mono { "+" (sig.weight) } span { (sig.text) } }
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
    let body = html! {
        (tabs(&page, "Settings"))
        (ui::settings_form(&page, &page.path("guard/settings"), html! {
            (ui::section("Detection", None, ui::card(html! {
                (ui::field_toggle("scam_detection", "Check messages for scams", Some("Scores every message from members without Manage Messages. The honeypot works either way."), s.scam_detection))
                (ui::field_select("strictness", "Strictness", Some("How sure the bot must be before it acts. Try a message on the Catches tab to see the difference."),
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
    Ok(ui::layout(&ctx, &page, "Scam protection", Some(status(&ctx, guild, &s)), body).into_response())
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

/// Layout for the catch list, the chart and the test rail. Classes are prefixed `gd-`.
fn style() -> Markup {
    html! { style { (PreEscaped(STYLE)) } }
}

const STYLE: &str = r#"
.gd-main { display: flex; flex-direction: column; gap: 26px; min-width: 0; }
.gd-top { display: flex; flex-wrap: wrap; align-items: flex-end; justify-content: space-between; gap: 20px 32px; }
.gd-top .readings { gap: 20px 36px; }
.gd-top .reading .avatars .avatar { width: 26px; height: 26px; }
.reading .gd-none { font-size: 14px; font-weight: 500; color: var(--ink-3); padding-top: 4px; }
.gd-warns { display: flex; flex-direction: column; gap: 6px; }
.gd-warn { font-size: 13px; color: var(--waiting); }
.gd-warn a { color: inherit; text-decoration: underline; text-underline-offset: 3px; }
.gd-bars { margin: 0; display: flex; flex-direction: column; align-items: flex-end; gap: 6px; }
.gd-bars svg { width: 120px; height: 36px; display: block; overflow: visible; }
.gd-bars rect { fill: var(--signal); opacity: 0.32; }
.gd-bars rect.now { opacity: 1; }
.gd-bars figcaption { font-size: 12px; color: var(--ink-3); }
.gd-show { align-self: flex-start; width: max-content; max-width: 100%; margin-bottom: 12px; }
.gd-show a { text-decoration: none; }
.gd-count { color: var(--signal); font-family: var(--mono); }
.gd-list { display: flex; flex-direction: column; }
.gd-catch { display: grid; grid-template-columns: 36px minmax(0, 1fr) auto; grid-template-areas: "av head out" "av body body"; column-gap: 14px; row-gap: 10px; align-items: start; padding: 16px 0; border-top: 1px solid var(--edge-1); }
.gd-catch:last-child { border-bottom: 1px solid var(--edge-1); }
.gd-av { grid-area: av; width: 36px; height: 36px; }
.gd-head { grid-area: head; display: flex; flex-wrap: wrap; align-items: baseline; gap: 2px 10px; min-width: 0; padding-top: 2px; }
.gd-head strong { font-weight: 500; font-size: 13.5px; }
.gd-out { grid-area: out; padding-top: 1px; }
.gd-body { grid-area: body; display: flex; flex-direction: column; gap: 10px; min-width: 0; }
.gd-chips { display: flex; flex-wrap: wrap; gap: 6px; }
.gd-msg { font: 12.5px/1.6 var(--mono); color: var(--ink-2); background: var(--well); border-radius: 8px; padding: 10px 12px; white-space: pre-wrap; overflow-wrap: anywhere; }
.gd-msg mark { background: none; color: var(--held); text-decoration: underline; text-decoration-color: var(--held); text-underline-offset: 3px; }
.gd-did { display: flex; flex-wrap: wrap; gap: 4px 16px; font-size: 12px; color: var(--ink-3); }
.gd-did span { display: inline-flex; align-items: center; gap: 6px; }
.gd-did .icon { width: 14px; height: 14px; }
.gd-actions { display: flex; flex-wrap: wrap; gap: 8px; }
.gd-actions { position: relative; }
.gd-pop { position: static; }
.gd-pop .pop-body { left: 0; right: auto; width: 340px; }
.gd-pop-foot { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.gd-decided { display: flex; align-items: center; gap: 8px; font-size: 12.5px; color: var(--ink-2); }
.gd-decided .avatar { width: 18px; height: 18px; }
.gd-why { font-size: 12.5px; color: var(--ink-2); padding-left: 10px; border-left: 2px solid var(--edge-2); overflow-wrap: anywhere; }
.gd-score { display: flex; align-items: center; justify-content: space-between; gap: 10px; font-size: 13px; }
.gd-score b { font-weight: 500; color: var(--ink); }
.gd-gauge { position: relative; height: 6px; border-radius: 3px; margin: 6px 0 2px; background: var(--control); }
.gd-fill { position: absolute; left: 0; top: 0; bottom: 0; border-radius: 3px; background: var(--waiting); }
.gd-fill.over { background: var(--signal); }
.gd-mark { position: absolute; top: -5px; bottom: -5px; width: 2px; border-radius: 1px; background: var(--ink); transform: translateX(-1px); }
.gd-sigs { display: flex; flex-direction: column; }
.gd-sig { display: grid; grid-template-columns: 34px minmax(0, 1fr); gap: 10px; align-items: baseline; padding: 8px 0; font-size: 13px; border-top: 1px solid var(--edge-1); }
.gd-sig:first-child { border-top: 0; }
.gd-sig b { font-weight: 500; color: var(--signal); }
.gd-sig span { color: var(--ink-2); }
@media (max-width: 640px) {
  .gd-catch { grid-template-columns: 36px minmax(0, 1fr); grid-template-areas: "av head" "av out" "body body"; }
  .gd-out { justify-self: start; }
  .gd-bars { align-items: flex-start; }
}
"#;
