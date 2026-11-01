use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::maud::{Markup, html};
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};

use super::{
    TabQuery, channel_in, channel_name, channel_select, countdown, discord_message, header_pop, month_start, open_link, plural, post_block, rail_figures, script,
    style,
};
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
    let month = over.iter().filter(|p| p.ends_at >= since).fold((0i64, 0i64), |(n, v), p| (n + 1, v + total_of(p)));
    // The closed polls with the most votes, for the rail.
    let mut busiest: Vec<(&Poll, i64)> = over.iter().map(|p| (p, total_of(p))).filter(|(_, n)| *n > 0).collect();
    busiest.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    busiest.truncate(4);
    let most = busiest.first().map(|b| b.1).unwrap_or(1).max(1);

    let actions = edit.then(|| header_pop("Post a poll", post_form(&ctx, &page, guild)));
    let this_month = html! {
        div.rail-block {
            h2 { "This month" }
            (rail_figures(&[("closed", month.0), ("votes", month.1), ("open now", open.len() as i64)]))
        }
    };
    let nothing_ended = html! { (ui::empty(Icon::Poll, "Nothing ended yet", "Closed polls and their winning answers show up here.")) };
    // More than one open: the others fill the main column under the hero. With none, the recently
    // ended ones take it over.
    let others = open.len() > 1;

    let body = html! {
        (style())
        (ui::tabs_counted(
            &[("Open", page.path("community/polls"), Some(open.len() as i64)), ("Ended", page.path("community/polls?tab=ended"), None)],
            if ended_tab { "Ended" } else { "Open" },
        ))
        @if ended_tab {
            div.with-rail {
                div.cm-main { (ended_list(&ctx, &page, guild, &over)) }
                aside.rail.cm-rail {
                    (this_month)
                    div.rail-block {
                        h2 { "Most votes" }
                        @if busiest.is_empty() {
                            p.muted.small { "Closed polls with votes show up here, most votes first." }
                        }
                        @for (p, n) in &busiest {
                            div.pl-busy {
                                div { span title=(p.question) { (p.question) } span.mono { (text::num(*n)) } }
                                span.meter { span style=(format!("width:{}%", n * 100 / most)) {} }
                            }
                        }
                    }
                }
            }
        } @else {
            @if let Some(p) = open.first() {
                (hero(&ctx, &page, guild, p, edit))
            } @else {
                (ui::empty(Icon::Poll, "No open polls", "Post one with the button above or /poll. Open polls wait here until they close."))
            }
            div.with-rail {
                div.cm-main {
                    @if others {
                        section.cm-block {
                            h2 { "Also open" }
                            div.tiles.pl-grid {
                                @for (i, p) in open[1..].iter().enumerate() { (open_card(&ctx, &page, guild, p, i, edit)) }
                            }
                        }
                    } @else {
                        section.cm-block {
                            h2 { "Recently ended" }
                            @if over.is_empty() {
                                (nothing_ended)
                            } @else {
                                div.tiles.cm-tiles.pl-recents {
                                    @for p in over.iter().take(4) { (recent_item(p)) }
                                }
                            }
                        }
                    }
                }
                aside.rail.cm-rail {
                    @if others {
                        div.rail-block {
                            h2 { "Recently ended" }
                            @if over.is_empty() {
                                (nothing_ended)
                            } @else {
                                @for p in over.iter().take(3) { (recent_item(p)) }
                            }
                        }
                    }
                    (this_month)
                }
            }
        }
        (script())
    };
    Ok(ui::layout(&ctx, &page, "Polls", actions, body).into_response())
}

/// Votes in a closed poll's final counts; 0 while they're being counted.
fn total_of(p: &Poll) -> i64 {
    p.results.as_deref().map(|r| r.iter().sum::<i64>()).unwrap_or(0)
}

fn votes(n: i64) -> String {
    format!("{} {}", text::num(n), plural(n, "vote", "votes"))
}

/// Live counts for an open poll, with every answer present even when nothing was read.
fn live_answers(ctx: &Ctx, p: &Poll) -> pl::Live {
    ctx.db.with(|c| pl::live(c, p)).unwrap_or_else(|_| pl::Live { answers: p.answers.iter().map(|a| (a.clone(), 0, 0, Vec::new())).collect(), voters: 0 })
}

/// The newest open poll, across the page: who asked, the question, and each answer's bar filling
/// to its share, with the latest voters' faces; beside it, the poll as members see it.
fn hero(ctx: &Ctx, page: &Page, guild: u64, p: &Poll, edit: bool) -> Markup {
    let pl::Live { answers, voters } = live_answers(ctx, p);
    let top = answers.iter().map(|a| a.1).max().unwrap_or(0);
    let total: i64 = answers.iter().map(|a| a.1).sum();
    html! {
        article.pl-hero {
            div.pl-main {
                div.pl-ask {
                    img.avatar src=(ctx.member_avatar(guild, p.created_by)) alt="" width="34" height="34" loading="lazy";
                    span { strong { (ctx.display_name(guild, p.created_by)) } " asked in " (channel_name(ctx, guild, p.channel_id)) " · " (text::ago(p.created_at)) }
                    span.status { span.dot.live {} (countdown("Closes in", p.ends_at)) }
                }
                h2.pl-q { (p.question) }
                div.pl-answers {
                    @for (i, (answer, n, pct, who)) in answers.iter().enumerate() {
                        div.pl-ans.lead[*n == top && top > 0] style={ "--i:" (i) } {
                            div.pl-bar {
                                span.pl-fill style={ "width:" (pct) "%" } {}
                                span.pl-label { @if *n == top && top > 0 { (ui::icon(Icon::Check)) } (answer) }
                                b.pl-pct data-count { (pct) "%" }
                            }
                            span.pl-voters { (ui::avatars(&who.iter().map(|u| ctx.member_avatar(guild, *u)).collect::<Vec<_>>(), 3)) }
                            span.pl-n { (votes(*n)) }
                        }
                    }
                }
                div.pl-foot {
                    span {
                        @if total == 0 { "No votes yet" } @else {
                            b { (votes(total)) } " from " b { (text::num(voters)) " " (plural(voters, "member", "members")) }
                        }
                        " · " (if p.multi { "several answers allowed" } else { "one answer each" })
                    }
                    div.cm-actions {
                        (open_link(page, p.channel_id, Some(p.message_id), false))
                        @if edit { (ui::action_button(&page.path(&format!("community/polls/{}/end", p.id)), "End now", Tone::Muted, true)) }
                    }
                }
            }
            (post_block("The poll", &channel_name(ctx, guild, p.channel_id), discord_message(ctx, page, p.created_at, poll_preview(ctx, p))))
        }
    }
}

/// Another open poll: the question, the share of votes as one bar, and who leads.
fn open_card(ctx: &Ctx, page: &Page, guild: u64, p: &Poll, i: usize, edit: bool) -> Markup {
    let answers = live_answers(ctx, p).answers;
    let total: i64 = answers.iter().map(|a| a.1).sum();
    let parts: Vec<(String, i64)> = answers.iter().map(|a| (a.0.clone(), a.1)).collect();
    let mut faces: Vec<u64> = Vec::new();
    for u in answers.iter().flat_map(|a| a.3.iter()) {
        if !faces.contains(u) {
            faces.push(*u);
        }
    }
    let shares: Vec<(String, i64, i64)> = answers.iter().map(|a| (a.0.clone(), a.1, a.2)).collect();
    html! {
        article.pl-card style={ "--i:" (i.min(8)) } {
            div.pl-card-top {
                img.avatar src=(ctx.member_avatar(guild, p.created_by)) alt="" width="30" height="30" loading="lazy";
                div.pl-card-text {
                    strong { (p.question) }
                    span.muted {
                        (channel_name(ctx, guild, p.channel_id)) " · " (countdown("Closes in", p.ends_at))
                        @if p.multi { " · pick several" }
                    }
                }
                div.cm-actions {
                    (open_link(page, p.channel_id, Some(p.message_id), true))
                    @if edit { (ui::action_button(&page.path(&format!("community/polls/{}/end", p.id)), "End now", Tone::Muted, true)) }
                }
            }
            (stack(&parts))
            div.pl-lead {
                span { (leader(&shares, total)) }
                span.gv-people {
                    (ui::avatars(&faces.iter().map(|u| ctx.member_avatar(guild, *u)).collect::<Vec<_>>(), 4))
                    span { (votes(total)) }
                }
            }
        }
    }
}

/// "Dune Part Two leads with 60%", a tie, or no votes.
fn leader(shares: &[(String, i64, i64)], total: i64) -> Markup {
    let top = shares.iter().map(|s| s.1).max().unwrap_or(0);
    let leaders: Vec<&(String, i64, i64)> = shares.iter().filter(|s| s.1 == top && top > 0).collect();
    html! {
        @if total == 0 { "No votes yet" }
        @else if leaders.len() == 1 { strong { (leaders[0].0) } " leads with " (leaders[0].2) "%" }
        @else { "Tied at " (leaders.first().map(|l| l.2).unwrap_or(0)) "% between " (leaders.len()) " answers" }
    }
}

/// Every answer's share of the votes as one bar, the biggest share brightest. Hovering a part
/// names the answer.
fn stack(parts: &[(String, i64)]) -> Markup {
    let total: i64 = parts.iter().map(|p| p.1).sum();
    let mut order: Vec<usize> = (0..parts.len()).collect();
    order.sort_by_key(|i| std::cmp::Reverse(parts[*i].1));
    let shade = |i: usize| match order.iter().position(|j| *j == i).unwrap_or(9) {
        0 => "var(--data)",
        1 => "color-mix(in oklab, var(--data) 62%, transparent)",
        2 => "color-mix(in oklab, var(--data) 42%, transparent)",
        _ => "color-mix(in oklab, var(--data) 26%, transparent)",
    };
    html! {
        div.pl-stack role="img" aria-label="Share of the votes by answer" {
            @for (i, (answer, n)) in parts.iter().enumerate() {
                @if *n > 0 {
                    span style={ "flex:" (n) ";background:" (shade(i)) } data-tip={ (answer) ": " ((n * 100 + total / 2) / total.max(1)) "%" } {}
                }
            }
        }
    }
}

fn ended_list(ctx: &Ctx, page: &Page, guild: u64, over: &[Poll]) -> Markup {
    html! {
        @if over.is_empty() {
            (ui::empty(Icon::Poll, "Nothing ended yet", "Closed polls show their final results here."))
        } @else {
            div.pl-cols {
                @for (i, p) in over.iter().enumerate() { (ended_card(ctx, page, guild, p, i)) }
            }
        }
    }
}

/// A closed poll: who asked, the question, and the final counts as bars with the winning
/// answer lit. Says what's missing when Discord hasn't given the counts.
fn ended_card(ctx: &Ctx, page: &Page, guild: u64, p: &Poll, i: usize) -> Markup {
    let counts = p.results.as_deref();
    let shares = pl::shares(p);
    let total = total_of(p);
    let top = shares.iter().map(|s| s.1).max().unwrap_or(0);
    html! {
        article.pl-card.ended style={ "--i:" (i.min(8)) } {
            div.pl-card-top {
                img.avatar src=(ctx.member_avatar(guild, p.created_by)) alt="" width="30" height="30" loading="lazy";
                div.pl-card-text {
                    strong { (p.question) }
                    span.muted { (ctx.display_name(guild, p.created_by)) " asked in " (channel_name(ctx, guild, p.channel_id)) " · ended " (text::ago(p.ends_at)) }
                }
                div.cm-actions { (open_link(page, p.channel_id, Some(p.message_id), true)) }
            }
            @match counts {
                None => p.muted.small { "Counting. Discord finalises the results a moment after the poll closes." },
                Some([]) => p.muted.small { "Results not available. The message was deleted or the bot lost access before it could read them." },
                Some(_) => div.pl-answers {
                    @for (j, (answer, n, pct)) in shares.iter().enumerate() {
                        div.pl-ans.bare.lead[*n == top && top > 0] style={ "--i:" (j) } {
                            div.pl-bar {
                                span.pl-fill style={ "width:" (pct) "%" } {}
                                span.pl-label { @if *n == top && top > 0 { (ui::icon(Icon::Check)) } (answer) }
                                b.pl-pct data-count { (pct) "%" }
                            }
                            span.pl-n { (votes(*n)) }
                        }
                    }
                },
            }
            div.pl-foot {
                span {
                    @if counts.is_some_and(|c| !c.is_empty()) { b { (votes(total)) } " in all · " }
                    (if p.multi { "several answers allowed" } else { "one answer each" })
                }
            }
        }
    }
}

/// One closed poll in the rail: the question, the share bar, the winner and the vote total.
fn recent_item(p: &Poll) -> Markup {
    let Some(counts) = p.results.as_deref().filter(|c| !c.is_empty()) else {
        let note = if p.results.is_some() { "Results not available" } else { "Counting" };
        return html! { div.pl-recent { strong title=(p.question) { (p.question) } span.muted.small { (note) } } };
    };
    let total: i64 = counts.iter().sum();
    let shares = pl::shares(p);
    let parts: Vec<(String, i64)> = shares.iter().map(|s| (s.0.clone(), s.1)).collect();
    let top = shares.iter().map(|s| s.1).max().unwrap_or(0);
    let leaders: Vec<&(String, i64, i64)> = shares.iter().filter(|s| s.1 == top && top > 0).collect();
    html! {
        div.pl-recent {
            strong title=(p.question) { (p.question) }
            (stack(&parts))
            div.pl-lead {
                @if total == 0 {
                    span { "No votes" }
                } @else if leaders.len() == 1 {
                    span title=(leaders[0].0) { strong { (leaders[0].0) } " · " (leaders[0].2) "%" }
                } @else {
                    span { "Tie" }
                }
                span { (votes(total)) " · " (text::ago(p.ends_at)) }
            }
        }
    }
}

/// Time left on a poll as Discord writes it under one: "30h left", "6d left", "12m left".
fn poll_left(secs: i64) -> String {
    match secs {
        s if s >= 86_400 => format!("{}d left", s / 86_400),
        s if s >= 3600 => format!("{}h left", s / 3600),
        s if s >= 60 => format!("{}m left", s / 60),
        s => format!("{s}s left"),
    }
}

/// The poll as Discord draws it before you vote: the question, each answer with its choice
/// mark, the votes so far and the time left.
fn poll_preview(ctx: &Ctx, p: &Poll) -> Markup {
    let answers = live_answers(ctx, p).answers;
    let total: i64 = answers.iter().map(|a| a.1).sum();
    let secs = ((p.ends_at - now_ms()) / 1000).max(0);
    html! {
        div.cm-dc-poll {
            strong { (p.question) }
            small { (if p.multi { "Select one or more answers" } else { "Select one answer" }) }
            @for a in &p.answers {
                div.cm-dc-opt { span { (a) } span.cm-dc-radio.box[p.multi] {} }
            }
            div.cm-dc-poll-foot {
                span { (votes(total)) " · " (poll_left(secs)) }
                span.cm-dc-btn.blurple.off { "Vote" }
            }
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
