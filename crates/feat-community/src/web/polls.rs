use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::maud::{Markup, html};
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, text};

use super::{TabQuery, channel_in, channel_name, channel_select, header_pop, open_link, style, tabs, until};
use crate::KEY;
use crate::polls::{self as pl, Poll};

pub async fn page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<TabQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let ended_tab = q.tab.as_deref() == Some("ended");
    let edit = page.can_edit();
    let polls = ctx.db.with(|c| pl::recent(c, guild, 200))?;
    let (open, over): (Vec<Poll>, Vec<Poll>) = polls.into_iter().partition(|p| !p.is_over());

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

/// The newest open poll: who asked, the question and its answers. Live counts are not kept
/// here, so the answers show without bars.
fn hero(ctx: &Ctx, page: &Page, guild: u64, p: &Poll, edit: bool) -> Markup {
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
            div.cm-answers {
                @for a in &p.answers { div.cm-answer { (a) } }
            }
            div.cm-poll-foot {
                p.muted.small {
                    @if p.multi { "Members can pick more than one answer. " }
                    "This page doesn't keep live vote counts. Discord shows them on the poll, and the final results appear here once it closes."
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
    html! {
        div.cm-row {
            div.cm-tile { (ui::icon(Icon::Poll)) }
            div.cm-row-main {
                strong.clip { (p.question) }
                span.muted.small {
                    (p.answers.len()) " answers · " (channel_name(ctx, guild, p.channel_id)) " · " (until("Closes in", p.ends_at))
                    @if p.multi { " · pick several" }
                }
            }
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
            div.cm-list {
                @for p in over {
                    div.cm-row {
                        div.cm-tile { (ui::icon(Icon::Poll)) }
                        div.cm-row-main {
                            strong.clip { (p.question) }
                            span.muted.small { (channel_name(ctx, guild, p.channel_id)) " · ended " (text::ago(p.ends_at)) }
                        }
                        (results(p))
                        div.cm-row-actions {
                            (open_link(page, p.channel_id, Some(p.message_id), "Open in Discord"))
                        }
                    }
                }
            }
        }
    }
}

/// Final counts as bars, the winning answer filled. Says what's missing when there are none.
fn results(p: &Poll) -> Markup {
    let Some(counts) = p.results.as_deref() else {
        return html! { span.muted.small.cm-results { "Counting" } };
    };
    if counts.is_empty() {
        return html! { span.muted.small.cm-results { "Results not available" } };
    }
    let shares = pl::shares(p);
    let top = shares.iter().map(|s| s.1).max().unwrap_or(0);
    let total: i64 = counts.iter().sum();
    html! {
        div.cm-results {
            div.cm-bars {
                @for (answer, n, pct) in &shares {
                    div.cm-bar-row {
                        div.cm-bar.win[*n == top && top > 0] {
                            i style=(format!("width:{pct}%")) {}
                            span { (answer) }
                            span.pct { (pct) "%" }
                        }
                        span.votes { (text::num(*n)) }
                    }
                }
            }
            p.muted.small { (text::num(total)) " " (if total == 1 { "vote" } else { "votes" }) " in all" }
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
                span.cm-chip.data { (leaders[0].0) " · " (leaders[0].2) "%" }
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
