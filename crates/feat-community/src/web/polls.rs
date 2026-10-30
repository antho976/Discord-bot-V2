use bot_core::axum::extract::{Path, State};
use bot_core::axum::response::IntoResponse;
use bot_core::maud::html;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, text};

use super::{channel_in, channel_name, channel_select, open_link};
use crate::KEY;
use crate::polls::{self as pl, Poll};

pub async fn page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let polls = ctx.db.with(|c| pl::recent(c, guild, 25))?;
    let open = polls.iter().filter(|p| !p.is_over()).count();
    let edit = page.can_edit();

    let body = html! {
        (ui::readings(&[("open", open.to_string()), ("recent", polls.len().to_string())]))
        (ui::section("Recent polls", Some("Discord counts the votes. Results appear here a minute or so after a poll closes."), html! {
            @if polls.is_empty() {
                (ui::empty(Icon::Poll, "No polls yet", "Post one below or with /poll. It shows up here with its results once it closes."))
            } @else {
                div.table-wrap { table.table {
                    thead { tr { th { "Poll" } th { "Status" } th { "Result" } th {} } }
                    tbody { @for p in &polls {
                        tr {
                            td {
                                div.clip style="max-width:min(340px,50vw)" { (p.question) }
                                span.muted.small {
                                    (channel_name(&ctx, guild, p.channel_id))
                                    @if !p.is_over() { " · " (p.answers.join(", ")) @if p.multi { " · pick several" } }
                                }
                            }
                            td style="white-space:nowrap" {
                                @if p.is_over() { (ui::badge("Ended", Tone::Muted)) " " span.muted.small { (text::ago(p.ends_at)) } }
                                @else { (ui::badge("Open", Tone::Ok)) " " span.muted.small { "closes " (crate::relative(p.ends_at)) } }
                            }
                            td { (results(p)) }
                            td.actions {
                                (open_link(&page, p.channel_id, Some(p.message_id)))
                                @if edit && !p.is_over() { (ui::action_button(&page.path(&format!("community/polls/{}/end", p.id)), "End now", Tone::Muted, true)) }
                            }
                        }
                    } }
                } }
            }
        }))
        @if edit {
            (ui::section("Post a poll", Some("Posted as a Discord poll, so members vote on the message itself."), html! {
                form.settings method="post" action=(page.path("community/polls")) {
                    (ui::card(html! {
                        (ui::field_text("question", "Question", None, "", "What should the next stream be?"))
                        (ui::field_textarea("answers", "Answers", Some("One per line, two to ten."), "", "Hades II\nIdleon grind\nJust chatting"))
                        (ui::field_number("hours", "Open for", None, 24, 1, pl::MAX_HOURS, Some("hours")))
                        (ui::field_toggle("multi", "Allow several answers", None, false))
                        (channel_select(&ctx, guild, "channel", "Channel", None))
                    }))
                    div.save-bar { button.btn.primary type="submit" { "Post poll" } }
                }
            }))
        }
    };
    Ok(ui::layout(&ctx, &page, "Polls", None, body).into_response())
}

/// Final counts as thin bars, or what's missing. Open polls show nothing here.
fn results(p: &Poll) -> bot_core::maud::Markup {
    if !p.is_over() {
        return html! {};
    }
    match p.results.as_deref() {
        None => return html! { span.muted.small { "Counting" } },
        Some([]) => return html! { span.muted.small { "Not available" } },
        Some(_) => {}
    }
    let shares = pl::shares(p);
    let top = shares.iter().map(|s| s.1).max().unwrap_or(0);
    html! {
        div style="display:flex;flex-direction:column;gap:4px;min-width:220px" {
            @for (answer, n, pct) in &shares {
                div style="display:flex;align-items:center;gap:8px;font-size:12px" {
                    span.clip style=(format!("width:96px;color:{}", if *n == top && top > 0 { "var(--ink)" } else { "var(--ink-3)" })) { (answer) }
                    span style="flex:1;height:4px;background:var(--control);border-radius:2px;overflow:hidden;min-width:60px" {
                        span style=(format!("display:block;height:100%;width:{pct}%;background:var(--data)")) {}
                    }
                    span.mono.muted style="width:64px;text-align:right" { (text::num(*n)) " · " (pct) "%" }
                }
            }
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
