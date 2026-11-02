use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::maud::{Markup, html};
use bot_core::web::ui;
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};

use super::{TabQuery, discord_message, dm_refused, month_start, open_link, still_figures, style};
use crate::suggestions::{self as sg, Status, Suggestion};
use crate::{KEY, Settings};

/// What the status says. A suggestion nobody has answered is waiting for review.
fn state_label(s: Status) -> &'static str {
    match s {
        Status::Open => "Waiting for review",
        other => other.label(),
    }
}

/// The colour of the status dot.
fn status_class(s: Status) -> &'static str {
    match s {
        Status::Open => "open",
        Status::Considering => "warn",
        Status::Approved | Status::Implemented => "ok",
        Status::Denied => "bad",
    }
}

pub async fn page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<TabQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let tab = q.tab.as_deref().unwrap_or("all");
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let now = now_ms();
    let since = month_start(&page.user, now);
    let weeks_back = now - 12 * WEEK;
    let (rows, waiting_n, month, senders, arrived) = ctx.db.with(|c| {
        Ok((sg::list(c, guild, 100)?, sg::open_count(c, guild)?, sg::month_stats(c, guild, since)?, sg::top_senders(c, guild, now - 30 * DAY, 5)?, sg::arrivals(c, guild, weeks_back)?))
    })?;
    let rows: Vec<Suggestion> = rows.into_iter().map(|(r, _)| r).collect();
    // The box reads newest first; the queue reads oldest first, so the longest wait leads.
    let mut waiting: Vec<&Suggestion> = rows.iter().filter(|r| r.status == Status::Open).collect();
    waiting.reverse();
    let edit = page.can_edit();
    let active = match tab {
        "review" => "Waiting for review",
        "settings" => "Settings",
        _ => "All",
    };
    // Suggestions per week over the last twelve, oldest first, the current week labelled "This week".
    let weekly: Vec<(String, i64)> = (0..12)
        .map(|i| {
            let start = weeks_back + i * WEEK;
            let n = arrived.iter().filter(|at| **at >= start && **at < start + WEEK).count() as i64;
            let label = if i == 11 { "This week".to_owned() } else { page.user.local(start).strftime("%b %-d").to_string() };
            (label, n)
        })
        .collect();

    let main = html! {
        (style())
        (ui::tabs_counted(
            &[
                ("All", page.path("community/suggestions"), None),
                ("Waiting for review", page.path("community/suggestions?tab=review"), Some(waiting_n)),
                ("Settings", page.path("community/suggestions?tab=settings"), None),
            ],
            active,
        ))
        @if tab == "settings" {
            // The settings beside the post they shape, Save inside the settings panel.
            div.compose.cm-compose.sg-compose {
                (ui::settings_form(&page, &page.path("community/suggestions"), html! {
                    h2.sg-h { "The suggestion box" }
                    (ui::card(settings_fields(&ctx, guild, &s)))
                }))
                aside.rail.cm-rail.sg-rail.compose-preview {
                    div.rail-block {
                        h2 { "What staff see" }
                        @match rows.first() {
                            Some(r) => {
                                (discord_message(&ctx, &page, r.created_at, post_preview(&ctx, guild, r, s.anonymous, s.threads)))
                                p.cm-dc-note { "The newest suggestion, drawn with the settings as saved." }
                            },
                            None => p.muted.small { "Once a member uses /suggest, the post shows here as Discord draws it." },
                        }
                        @if edit {
                            div.cm-test {
                                (ui::action_button(&page.path("community/suggestions/test"), "Send a test", ui::Tone::Muted, false))
                                span.cm-dc-note { "DMs you this post and the message its author gets when staff decide." }
                            }
                        }
                    }
                }
            }
        } @else {
            div.with-rail {
                div.cm-main {
                    @if tab == "review" {
                        // The one that has waited longest, large, with the decision right on it.
                        @match waiting.first() {
                            Some(r) => { (focus(&ctx, &page, guild, r, edit)) }
                            None => section.sgx-panel { (ui::empty(Icon::Bulb, "Nothing waiting", "New suggestions wait here until you decide on them.")) },
                        }
                        @if waiting.len() > 1 {
                            section.sgx-panel {
                                h2 { "Also waiting" span.muted { (waiting.len() - 1) " · oldest first" } }
                                div.sgx-rows { @for r in waiting.iter().skip(1) { (row_of(&ctx, &page, guild, r, edit)) } }
                            }
                        }
                    } @else {
                        // How much comes in: this month as a figure, the weeks as a line.
                        section.sgx-panel.sgx-lead {
                            div.sgx-lead-top {
                                div.sgx-big { b { (text::num(month.0)) } span { "sent this month" } }
                                div.sgx-small {
                                    a href=(page.path("community/suggestions?tab=review")) { b.accent[waiting_n > 0] { (text::num(waiting_n)) } span { "waiting for you" } }
                                    div { b { (text::num(month.1)) } span { "approved" } }
                                    div { b { (text::num(month.2)) } span { "done" } }
                                }
                            }
                            @if weekly.iter().any(|w| w.1 > 0) {
                                h3 { "Suggestions per week" span.muted { "last 12 weeks" } }
                                (ui::area_chart_series(&weekly, "suggestions"))
                            }
                        }
                        section.sgx-panel {
                            h2 { "All suggestions" @if !rows.is_empty() { span.muted { "newest first" } } }
                            @if rows.is_empty() {
                                @if s.suggestions_channel.is_some() {
                                    (ui::empty(Icon::Bulb, "No suggestions yet", "Members send them with /suggest. They show up here for you to decide on."))
                                } @else {
                                    (ui::empty(Icon::Bulb, "The box is closed", "Pick a channel under Settings and members can use /suggest."))
                                }
                            } @else {
                                div.sgx-rows { @for r in &rows { (row_of(&ctx, &page, guild, r, edit)) } }
                            }
                        }
                    }
                }
                aside.rail.cm-rail.sg-rail {
                    div.rail-block {
                        h2 { "Who sends the most" span.muted { "30 days" } }
                        @if senders.is_empty() {
                            p.muted.small { "Nobody has sent one in the last 30 days." }
                        } @else {
                            @let top = senders.first().map(|x| x.1).unwrap_or(1).max(1);
                            ol.sgx-senders {
                                @for (i, (u, n)) in senders.iter().enumerate() {
                                    li {
                                        img.avatar src=(ctx.member_avatar(guild, *u)) alt="" width="28" height="28" loading="lazy";
                                        span.clip { (ctx.display_name(guild, *u)) }
                                        span.mono { (text::num(*n)) }
                                        span.sgx-track { i.lead[i == 0] style={ "width:" (format!("{:.0}", *n as f64 * 100.0 / top as f64)) "%" } {} }
                                    }
                                }
                            }
                        }
                    }
                    @if tab == "review" {
                        div.rail-block {
                            h2 { "This month" }
                            (still_figures(&[("sent", month.0), ("approved", month.1), ("done", month.2)]))
                        }
                    } @else if let Some(r) = rows.first() {
                        div.rail-block {
                            h2 { "The newest, as staff see it" }
                            (discord_message(&ctx, &page, r.created_at, post_preview(&ctx, guild, r, r.anonymous, false)))
                        }
                    }
                }
            }
        }
    };
    let main = html! { (main) (ui::sent_toast(q.sent.is_some(), "Test sent to your DMs")) };
    Ok(ui::layout(&ctx, &page, "Suggestions", None, main).into_response())
}

const DAY: i64 = 86_400_000;
const WEEK: i64 = 7 * DAY;

/// The suggestion that has waited longest, large: who sent it, what it says, and the decision
/// buttons with a reply box, so deciding takes one click.
fn focus(ctx: &Ctx, page: &Page, guild: u64, r: &Suggestion, edit: bool) -> Markup {
    html! {
        section.sgx-panel.sgx-focus {
            div.sgx-who {
                img.avatar src=(ctx.member_avatar(guild, r.author_id)) alt="" width="48" height="48" loading="lazy";
                div.grow {
                    strong { (ctx.display_name(guild, r.author_id)) }
                    span { "Suggestion #" (r.id) " · sent " (text::ago(r.created_at)) @if r.anonymous { " · posted without their name" } }
                }
                (open_link(page, r.channel_id, r.message_id, true))
            }
            p.sgx-text { (r.content) }
            @if edit {
                form.sgx-decide method="post" action=(page.path(&format!("community/suggestions/{}/status", r.id))) {
                    textarea.input name="reason" rows="2" placeholder="Reply to them (optional)" maxlength="800" aria-label="Reply" {}
                    div.sgx-buttons {
                        button.btn.primary type="submit" name="status" value="approved" { "Approve" }
                        button.btn type="submit" name="status" value="considering" { "Considering" }
                        button.btn type="submit" name="status" value="denied" { "Deny" }
                        span.muted.small { "They get a message with your reply." }
                    }
                }
            }
        }
    }
}

/// One suggestion as a row: the sender's face, the text, who and when, the status and its action.
fn row_of(ctx: &Ctx, page: &Page, guild: u64, r: &Suggestion, edit: bool) -> Markup {
    html! {
        div.sgx-row {
            img.avatar src=(ctx.member_avatar(guild, r.author_id)) alt="" width="28" height="28" loading="lazy";
            div.grow {
                strong title=(r.content) { (text::clip(&r.content, 140)) }
                span {
                    (ctx.display_name(guild, r.author_id)) " · " (text::ago(r.created_at))
                    @if r.status != Status::Open && !r.reason.is_empty() { " · “" (text::clip(&r.reason, 80)) "”" }
                }
            }
            span class={ "sgx-status " (status_class(r.status)) } { span.dot {} (state_label(r.status)) }
            div.cm-actions {
                (open_link(page, r.channel_id, r.message_id, true))
                @if edit { (review_pop(page, r)) }
            }
        }
    }
}

/// The suggestion's post as staff see it in the channel: the embed in its status colour, the
/// author unless it's anonymous, the decision, and the Review button.
fn post_preview(ctx: &Ctx, guild: u64, r: &Suggestion, anonymous: bool, thread: bool) -> Markup {
    html! {
        div.cm-dc-embed style={ "--embed:#" (format!("{:06x}", r.status.color())) } {
            @if !anonymous {
                div.cm-dc-author {
                    img src=(ctx.member_avatar(guild, r.author_id)) alt="" width="22" height="22" loading="lazy";
                    (ctx.display_name(guild, r.author_id))
                }
            }
            div.cm-dc-desc { (text::clip(&r.content, 600)) }
            @if r.status != Status::Open {
                div.cm-dc-fields {
                    div {
                        b { (r.status.label()) @if let Some(u) = r.staff_id { " by " (ctx.display_name(guild, u)) } }
                        span { @if r.reason.is_empty() { "No reason given." } @else { (r.reason) } }
                    }
                }
            }
            div.cm-dc-foot { "Suggestion #" (r.id) }
        }
        div.cm-dc-btns { span.cm-dc-btn { "Review" } }
        @if thread { (thread_line(r.id)) }
    }
}

/// The thread Discord shows under a suggestion when threads are on.
fn thread_line(id: i64) -> Markup {
    html! {
        div.cm-dc-thread {
            span.cm-dc-curve {}
            span.cm-dc-tbox { strong { "Suggestion #" (id) } span { "See thread" } (ui::icon(Icon::Chevron)) }
        }
    }
}

/// The status form for editors. Waiting suggestions say Review, decided ones say Change.
fn review_pop(page: &Page, r: &Suggestion) -> Markup {
    let options: Vec<(String, String)> = Status::ALL.iter().map(|st| (st.as_str().to_owned(), state_label(*st).to_owned())).collect();
    html! {
        details.pop.cm-review {
            @if r.status == Status::Open { summary.btn.small.outline { "Review" } } @else { summary.btn.small.ghost { "Change" } }
            div.pop-body {
                form.stack method="post" action=(page.path(&format!("community/suggestions/{}/status", r.id))) {
                    (ui::select("status", &options, r.status.as_str()))
                    textarea.input name="reason" rows="3" placeholder="Reply, shown on the post and sent to them" maxlength="800" aria-label="Reply" { (r.reason) }
                    @if r.status != Status::Open { p.muted.small { "Keeping the status edits the reply. The author hears only about a new status." } }
                    button.btn.small.primary type="submit" style="align-self:flex-start" { "Save" }
                }
            }
        }
    }
}

fn settings_fields(ctx: &Ctx, guild: u64, s: &Settings) -> Markup {
    html! {
        (ui::field_channel(ctx, guild, "suggestions_channel", "Send them to", Some("A channel only staff can see keeps it a private box. None closes the box."), s.suggestions_channel))
        (ui::field_role(ctx, guild, "staff_role", "Staff role", Some("Can review suggestions from Discord, besides anyone with Manage Messages."), s.staff_role))
        (ui::field_toggle("threads", "Open a thread", Some("Gives every suggestion a thread for staff to talk it over in."), s.threads))
        (ui::field_toggle("anonymous", "Hide the author", Some("Posts in the channel show no name. The dashboard still shows who sent them."), s.anonymous))
    }
}

pub async fn save_settings(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let channel = form.id("suggestions_channel");
    if let Some(c) = channel {
        super::channel_in(&ctx, guild, Some(c))?;
    }
    let staff_role = form.id("staff_role").filter(|r| ctx.roles(guild).iter().any(|(id, _, _)| id == r));
    let s = Settings { suggestions_channel: channel, staff_role, threads: form.bool("threads"), anonymous: form.bool("anonymous") };
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed suggestion settings");
    Ok(saved(&page.path("community/suggestions?tab=settings")))
}

/// "Send a test" on the settings tab: a suggestion post and its author's message, in the
/// editor's DMs.
pub async fn send_test(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    // The preview has no Discord to send to.
    if !ctx.cfg.preview {
        match sg::send_test(&ctx, guild, page.user.id, &s).await {
            Ok(true) => {}
            Ok(false) => return Err(dm_refused()),
            Err(e) => {
                bot_core::tracing::warn!(guild, error = %e, "couldn't send a test suggestion");
                return Err(WebError::Bad("The test didn't send. Discord turned it down; try again in a minute.".into()));
            }
        }
    }
    ctx.audit(&page.user, Some(guild), KEY, "Sent themselves a test suggestion post");
    Ok(bot_core::axum::response::Redirect::to(&page.path("community/suggestions?tab=settings&sent=1")).into_response())
}

pub async fn set_status(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let before = match ctx.db.with(|c| sg::get(c, id))? {
        Some(s) if s.guild_id == guild => s,
        _ => return Err(WebError::NotFound),
    };
    let status = Status::parse(&form.string("status")).ok_or_else(|| WebError::Bad("Pick a status.".into()))?;
    sg::set_status(&ctx, id, status, &form.string("reason"), page.user.id).await?;
    let what = if before.status == status && status != Status::Open {
        format!("Edited the reply on suggestion #{id}")
    } else {
        format!("Marked suggestion #{id} {}", status.label().to_lowercase())
    };
    ctx.audit(&page.user, Some(guild), KEY, what);
    Ok(saved(&page.path(if before.status == Status::Open { "community/suggestions?tab=review" } else { "community/suggestions" })))
}
