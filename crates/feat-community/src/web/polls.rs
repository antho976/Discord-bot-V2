use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::maud::{Markup, html};
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};

use super::{TabQuery, channel_in, channel_name, channel_select, header_pop, month_start, open_link, style, tabs, until};
use crate::KEY;
use crate::polls::{self as pl, Poll};

pub async fn page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<TabQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let ended_tab = q.tab.as_deref() == Some("ended");
    let edit = page.can_edit();
    let polls = ctx.db.with(|c| pl::recent(c, guild, 200))?;
    let (open, mut over): (Vec<Poll>, Vec<Poll>) = polls.into_iter().partition(|p| !p.is_over());
    // Ended ones read by when they closed, not when they were posted.
    over.sort_by_key(|p| std::cmp::Reverse(p.ends_at));
    // Votes cast while the bot was offline: read the open polls from Discord when they're due.
    pl::wait_briefly(pl::catch_up(&ctx, &open, pl::RESYNC_MS), 1500).await;

    // Polls that closed this month in the viewer's time, and the votes they got.
    let since = month_start(&page.user, now_ms());
    let month = over.iter().filter(|p| p.ends_at >= since).fold((0i64, 0i64), |(n, v), p| (n + 1, v + p.results.as_deref().map(|r| r.iter().sum::<i64>()).unwrap_or(0)));
    let main = if ended_tab { ended_list(&ctx, &page, guild, &over) } else { open_view(&ctx, &page, guild, &open, edit) };
    let actions = edit.then(|| header_pop("Post a poll", post_form(&ctx, &page, guild)));

    let body = html! {
        (style())
        (tabs(
            &[("Open", page.path("community/polls"), Some(open.len() as i64)), ("Ended", page.path("community/polls?tab=ended"), None)],
            if ended_tab { "Ended" } else { "Open" },
        ))
        div.with-rail {
            div.stack.cm-main { (main) }
            aside.rail {
                @if ended_tab {
                    div.rail-block {
                        h2 { "This month" }
                        div.cm-month {
                            div { b.mono { (text::num(month.0)) } span { "closed" } }
                            div { b.mono { (text::num(month.1)) } span { "votes" } }
                            div { b.mono { (text::num(open.len() as i64)) } span { "open now" } }
                        }
                    }
                } @else {
                    div.rail-block {
                        h2 { "Recently ended" }
                        @if over.is_empty() {
                            (ui::empty(Icon::Poll, "Nothing ended yet", "Closed polls and their winning answers show up here."))
                        } @else {
                            @for p in over.iter().take(3) { (recent_item(p)) }
                        }
                    }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Polls", actions, body).into_response())
}

fn open_view(ctx: &Ctx, page: &Page, guild: u64, open: &[Poll], edit: bool) -> Markup {
    html! {
        @if let Some(p) = open.first() {
            (hero(ctx, page, guild, p, edit))
        } @else {
            (ui::empty(Icon::Poll, "No open polls", "Post one with the button above or /poll. Open polls wait here until they close."))
        }
        @if open.len() > 1 {
            div.cm-list {
                @for p in &open[1..] { (row_open(ctx, page, guild, p, edit)) }
            }
        }
    }
}

/// The newest open poll: who asked, the question, and each answer with its live count and the
/// latest voters.
fn hero(ctx: &Ctx, page: &Page, guild: u64, p: &Poll, edit: bool) -> Markup {
    let live = ctx.db.with(|c| pl::live(c, p)).ok();
    let top = live.as_ref().and_then(|l| l.answers.iter().map(|a| a.1).max()).unwrap_or(0);
    let total: i64 = live.as_ref().map(|l| l.answers.iter().map(|a| a.1).sum()).unwrap_or(0);
    html! {
        article.cm-poll {
            div.cm-poll-head {
                img.avatar src=(ctx.member_avatar(guild, p.created_by)) alt="" width="28" height="28" loading="lazy";
                span {
                    strong { (ctx.display_name(guild, p.created_by)) } " asked in " (channel_name(ctx, guild, p.channel_id)) " · " (text::ago(p.created_at))
                }
                span.status { span.dot.live {} (until("Closes in", p.ends_at)) }
            }
            h2.cm-question { (p.question) }
            div.cm-bars.cm-live {
                @for (answer, n, pct, voters) in live.as_ref().map(|l| l.answers.as_slice()).unwrap_or_default() {
                    div.cm-bar-row {
                        div.cm-bar.win[*n == top && top > 0] {
                            i style=(format!("width:{pct}%")) {}
                            span { (answer) }
                            span.pct { (pct) "%" }
                        }
                        span.cm-voters { (ui::avatars(&voters.iter().map(|u| ctx.member_avatar(guild, *u)).collect::<Vec<_>>(), 3)) }
                        span.votes { (text::num(*n)) " " (if *n == 1 { "vote" } else { "votes" }) }
                    }
                }
            }
            div.cm-poll-foot {
                p.muted.small {
                    @let voters = live.as_ref().map(|l| l.voters).unwrap_or(0);
                    @if total == 0 { "No votes yet" } @else {
                        (text::num(total)) " " (if total == 1 { "vote" } else { "votes" }) " from " (text::num(voters)) " " (if voters == 1 { "member" } else { "members" })
                    }
                    " · " (if p.multi { "several answers allowed" } else { "one answer each" })
                }
                div.cm-actions {
                    (open_link(page, p.channel_id, Some(p.message_id), "Open in Discord"))
                    @if edit { (ui::action_button(&page.path(&format!("community/polls/{}/end", p.id)), "End now", Tone::Muted, true)) }
                }
            }
        }
    }
}

fn row_open(ctx: &Ctx, page: &Page, guild: u64, p: &Poll, edit: bool) -> Markup {
    let live = ctx.db.with(|c| pl::live(c, p)).ok();
    let total: i64 = live.as_ref().map(|l| l.answers.iter().map(|a| a.1).sum()).unwrap_or(0);
    let lead = live.as_ref().and_then(|l| {
        let top = l.answers.iter().map(|a| a.1).max()?;
        let leaders: Vec<_> = l.answers.iter().filter(|a| a.1 == top && top > 0).collect();
        (leaders.len() == 1).then(|| format!("{} leads with {}%", leaders[0].0, leaders[0].2))
    });
    html! {
        div.cm-row {
            div.cm-tile { (ui::icon(Icon::Poll)) }
            div.cm-row-main {
                strong.clip { (p.question) }
                span.muted.small {
                    @match &lead { Some(l) => { (l) }, None => { (p.answers.len()) " answers" } }
                    " · " (channel_name(ctx, guild, p.channel_id)) " · " (until("Closes in", p.ends_at))
                    @if p.multi { " · pick several" }
                }
            }
            span.cm-entries { (text::num(total)) " " (if total == 1 { "vote" } else { "votes" }) }
            div.cm-row-actions {
                (open_link(page, p.channel_id, Some(p.message_id), "Open in Discord"))
                @if edit { (ui::action_button(&page.path(&format!("community/polls/{}/end", p.id)), "End now", Tone::Muted, true)) }
            }
        }
    }
}

fn ended_list(ctx: &Ctx, page: &Page, guild: u64, over: &[Poll]) -> Markup {
    html! {
        @if over.is_empty() {
            (ui::empty(Icon::Poll, "Nothing ended yet", "Closed polls show their final results here."))
        } @else {
            div.cm-ended {
                @for p in over { (ended_card(ctx, page, guild, p)) }
            }
        }
    }
}

/// An ended poll in the hero's shape: who asked, the question, and the final counts as bars
/// with the winning answer filled. Says what's missing when Discord hasn't given the counts.
fn ended_card(ctx: &Ctx, page: &Page, guild: u64, p: &Poll) -> Markup {
    let counts = p.results.as_deref();
    let shares = pl::shares(p);
    let total: i64 = counts.map(|c| c.iter().sum()).unwrap_or(0);
    let top = shares.iter().map(|s| s.1).max().unwrap_or(0);
    html! {
        article.cm-poll.ended {
            div.cm-poll-head {
                img.avatar src=(ctx.member_avatar(guild, p.created_by)) alt="" width="28" height="28" loading="lazy";
                span {
                    strong { (ctx.display_name(guild, p.created_by)) } " asked in " (channel_name(ctx, guild, p.channel_id)) " · ended " (text::ago(p.ends_at))
                }
                span.status { span.dot {} "Ended" }
            }
            h2.cm-question { (p.question) }
            @match counts {
                None => p.muted.small { "Counting. Discord finalises the results a moment after the poll closes." },
                Some([]) => p.muted.small { "Results not available. The message was deleted or the bot lost access before it could read them." },
                Some(_) => div.cm-bars.cm-live {
                    @for (answer, n, pct) in &shares {
                        div.cm-bar-row {
                            div.cm-bar.win[*n == top && top > 0] {
                                i style=(format!("width:{pct}%")) {}
                                span { (answer) }
                                span.pct { (pct) "%" }
                            }
                            span.votes { (text::num(*n)) " " (if *n == 1 { "vote" } else { "votes" }) }
                        }
                    }
                },
            }
            div.cm-poll-foot {
                p.muted.small {
                    @if counts.is_some_and(|c| !c.is_empty()) {
                        (text::num(total)) " " (if total == 1 { "vote" } else { "votes" }) " in all · "
                    }
                    (if p.multi { "several answers allowed" } else { "one answer each" })
                }
                div.cm-actions { (open_link(page, p.channel_id, Some(p.message_id), "Open in Discord")) }
            }
        }
    }
}

/// One line in "Recently ended": the winning answer as a chip and the vote total.
fn recent_item(p: &Poll) -> Markup {
    let Some(counts) = p.results.as_deref().filter(|c| !c.is_empty()) else {
        let note = if p.results.is_some() { "Results not available" } else { "Counting" };
        return html! { div.cm-recent { strong.clip { (p.question) } span.muted { (note) } } };
    };
    let total: i64 = counts.iter().sum();
    let shares = pl::shares(p);
    let top = shares.iter().map(|s| s.1).max().unwrap_or(0);
    let leaders: Vec<&(String, i64, i64)> = shares.iter().filter(|s| s.1 == top && top > 0).collect();
    html! {
        div.cm-recent {
            strong.clip { (p.question) }
            @if total == 0 {
                span.muted { "No votes" }
            } @else if leaders.len() == 1 {
                span.cm-chip.data title=(leaders[0].0) { span.cm-chip-text { (leaders[0].0) } span.pct { "· " (leaders[0].2) "%" } }
            } @else {
                span.cm-chip.data { "Tie" }
            }
            span.muted { (text::num(total)) " " (if total == 1 { "vote" } else { "votes" }) " · " (text::ago(p.ends_at)) }
        }
    }
}

fn post_form(ctx: &Ctx, page: &Page, guild: u64) -> Markup {
    html! {
        form.settings method="post" action=(page.path("community/polls")) {
            (ui::card(html! {
                (ui::field_text("question", "Question", None, "", "What should the next stream be?"))
                (ui::field_textarea("answers", "Answers", Some("Separate with | or put each on its own line. Two to ten."), "", "Hades II\nIdleon grind\nJust chatting"))
                (ui::field_number("hours", "Open for", None, 24, 1, pl::MAX_HOURS, Some("hours")))
                (ui::field_toggle("multi", "Allow several answers", None, false))
                (channel_select(ctx, guild, "channel", "Channel", None))
            }))
            div.cm-pop-foot { button.btn.primary type="submit" { "Post poll" } }
        }
    }
}

pub async fn create(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let question = form.string("question");
    if question.is_empty() {
        return Err(WebError::Bad("Write the question.".into()));
    }
    let answers = pl::parse_answers(&form.text("answers")).map_err(WebError::Bad)?;
    let channel = channel_in(&ctx, guild, form.id("channel"))?;
    let hours = form.int_in("hours", 1, pl::MAX_HOURS, 24);
    pl::create(&ctx, pl::New { guild, channel, question: &question, answers: &answers, hours, multi: form.bool("multi"), by: page.user.id })
        .await
        .map_err(|e| {
            bot_core::tracing::warn!(guild, error = %e, "couldn't post a poll from the dashboard");
            WebError::Bad("Couldn't post in that channel. Check that the bot can send messages there.".into())
        })?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Posted a poll: {}", text::clip(&question, 60)));
    Ok(saved(&page.path("community/polls")))
}

pub async fn end(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    match ctx.db.with(|c| pl::get(c, id))? {
        Some(p) if p.guild_id == guild => {}
        _ => return Err(WebError::NotFound),
    }
    pl::end_now(&ctx, id).await.map_err(|_| WebError::Bad("Discord wouldn't end that poll. It may already be over or the message may be gone.".into()))?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Ended poll #{id}"));
    Ok(saved(&page.path("community/polls")))
}
