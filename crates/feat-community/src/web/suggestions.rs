use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::maud::{Markup, html};
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};

use super::{TabQuery, discord_message, month_start, open_link, plural, rail_figures, style};
use crate::suggestions::{self as sg, Status, Suggestion, Tally};
use crate::{KEY, Settings};

fn tone(s: Status) -> Tone {
    match s {
        Status::Open => Tone::Muted,
        Status::Considering => Tone::Warn,
        Status::Approved | Status::Implemented => Tone::Ok,
        Status::Denied => Tone::Bad,
    }
}

/// What the badge says. A suggestion nobody has answered is waiting for review.
fn state_label(s: Status) -> &'static str {
    match s {
        Status::Open => "Waiting for review",
        other => other.label(),
    }
}

/// The tint of a staff reply.
fn reply_class(s: Status) -> &'static str {
    match s {
        Status::Open => "",
        Status::Considering => "warn",
        Status::Approved | Status::Implemented => "ok",
        Status::Denied => "bad",
    }
}

pub async fn page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<TabQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let tab = q.tab.as_deref().unwrap_or("all");
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let since = month_start(&page.user, now_ms());
    let (mut rows, waiting, month, voters) = ctx.db.with(|c| {
        Ok((sg::list(c, guild, 100)?, sg::open_count(c, guild)?, sg::month_stats(c, guild, since)?, sg::top_voters(c, guild, since, 3)?))
    })?;
    // The newest one, before the board is sorted: the settings tab shows it as it would post.
    let newest = rows.first().cloned();
    // The board reads top-first: net votes, then newest.
    rows.sort_by_key(|(r, t)| (std::cmp::Reverse(t.up - t.down), std::cmp::Reverse(r.id)));
    let edit = page.can_edit();
    let shown: Vec<&(Suggestion, Tally)> = rows.iter().filter(|(r, _)| tab != "review" || r.status == Status::Open).collect();
    let active = match tab {
        "review" => "Waiting for review",
        "settings" => "Settings",
        _ => "All",
    };

    let main = html! {
        (style())
        (ui::tabs_counted(
            &[
                ("All", page.path("community/suggestions"), None),
                ("Waiting for review", page.path("community/suggestions?tab=review"), Some(waiting)),
                ("Settings", page.path("community/suggestions?tab=settings"), None),
            ],
            active,
        ))
        @if tab == "settings" {
            div.with-rail {
                div.cm-main {
                    (ui::settings_form(&page, &page.path("community/suggestions"), ui::card(settings_fields(&ctx, guild, &s))))
                }
                aside.rail.cm-rail {
                    div.rail-block {
                        h2 { "How one posts" }
                        @match &newest {
                            Some((r, t)) => {
                                (discord_message(&ctx, &page, r.created_at, post_preview(&ctx, guild, r, t, s.anonymous, s.threads)))
                                p.cm-dc-note { "The newest suggestion, drawn with the settings as saved." }
                            },
                            None => p.muted.small { "Once a member uses /suggest, the post shows here as Discord draws it." },
                        }
                    }
                }
            }
        } @else {
            div.with-rail {
                div.cm-main {
                    @if shown.is_empty() {
                        @if tab == "review" {
                            (ui::empty(Icon::Bulb, "Nothing waiting", "New suggestions wait here until staff review them."))
                        } @else if s.suggestions_channel.is_some() {
                            (ui::empty(Icon::Bulb, "No suggestions yet", "Members post them with /suggest. They show up here with their votes."))
                        } @else {
                            (ui::empty(Icon::Bulb, "Suggestions are off", "Pick a channel under Settings and members can use /suggest."))
                        }
                    } @else {
                        div.sg-list {
                            @for (i, (r, t)) in shown.iter().enumerate() { (card(&ctx, &page, guild, r, t, i, edit)) }
                        }
                    }
                }
                aside.rail.cm-rail {
                    div.rail-block {
                        h2 { "This month" }
                        (rail_figures(&[("suggested", month.0), ("approved", month.1), ("done", month.2)]))
                    }
                    div.rail-block {
                        h2 { "Most votes this month" }
                        @if voters.is_empty() {
                            (ui::empty(Icon::Bulb, "No votes this month", "Members vote with the buttons on each suggestion."))
                        } @else {
                            div.sg-top {
                                @for (i, (u, n)) in voters.iter().enumerate() {
                                    div.sg-voter.first[i == 0] style={ "--i:" (i) } {
                                        img.avatar src=(ctx.member_avatar(guild, *u)) alt="" width="54" height="54" loading="lazy";
                                        strong { (ctx.display_name(guild, *u)) }
                                        span { (text::num(*n)) " " (plural(*n, "vote", "votes")) }
                                    }
                                }
                            }
                        }
                    }
                    @if let Some((r, t)) = shown.first() {
                        div.rail-block {
                            h2 { "The top one in Discord" }
                            (discord_message(&ctx, &page, r.created_at, post_preview(&ctx, guild, r, t, r.anonymous, false)))
                        }
                    }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Suggestions", None, main).into_response())
}

/// One suggestion on the board: its score, the text and who sent it, the split of votes, and
/// the staff reply once there is one.
fn card(ctx: &Ctx, page: &Page, guild: u64, r: &Suggestion, t: &Tally, i: usize, edit: bool) -> Markup {
    let net = t.up - t.down;
    let votes = t.up + t.down;
    let score = match net {
        n if n > 0 => "sg-score",
        0 => "sg-score flat",
        _ => "sg-score down",
    };
    html! {
        article.sg-card style={ "--i:" (i.min(10)) } {
            div class=(score) aria-label={ "Net votes: " (net) } {
                (arrow(net >= 0))
                b { (if net > 0 { format!("+{net}") } else { net.to_string() }) }
            }
            div.sg-body {
                p.sg-text title=(r.content) { (text::clip(&r.content, 220)) }
                div.sg-by {
                    img.avatar src=(ctx.member_avatar(guild, r.author_id)) alt="" width="22" height="22" loading="lazy";
                    span { strong { (ctx.display_name(guild, r.author_id)) } " · " (text::ago(r.created_at)) }
                    @if r.anonymous { span { "· posted anonymously" } }
                }
                @if votes > 0 {
                    div.sg-tally style={ "--i:" (i.min(10)) } {
                        span.sg-split role="img" aria-label=(tally_text(t)) {
                            @if t.up > 0 { span.up style={ "flex:" (t.up) } {} }
                            @if t.down > 0 { span.down style={ "flex:" (t.down) } {} }
                        }
                        span { (tally_text(t)) }
                    }
                } @else {
                    div.sg-tally { span { "No votes yet" } }
                }
                @if r.status != Status::Open {
                    div class={ "sg-reply " (reply_class(r.status)) } {
                        @if let Some(staff) = r.staff_id {
                            img.avatar src=(ctx.member_avatar(guild, staff)) alt="" width="28" height="28" loading="lazy";
                        }
                        div.sg-bubble {
                            strong {
                                @if let Some(staff) = r.staff_id { (ctx.display_name(guild, staff)) " " }
                                span { (r.status.label().to_lowercase()) " it" }
                            }
                            @if r.reason.is_empty() { span.muted { "No reason given." } } @else { span { (r.reason) } }
                        }
                    }
                }
            }
            div.sg-side {
                (ui::badge(state_label(r.status), tone(r.status)))
                div.cm-actions {
                    (open_link(page, r.channel_id, r.message_id, true))
                    @if edit { (review_pop(page, r)) }
                }
            }
        }
    }
}

/// The suggestion's post as members see it: the embed in its status colour, the author unless
/// it's anonymous, staff's answer, and the vote buttons (closed once it's decided).
fn post_preview(ctx: &Ctx, guild: u64, r: &Suggestion, t: &Tally, anonymous: bool, thread: bool) -> Markup {
    let closed = r.status.is_decided();
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
        div.cm-dc-btns {
            span.cm-dc-btn.off[closed] { "Upvote · " (t.up) }
            span.cm-dc-btn.off[closed] { "Downvote · " (t.down) }
            span.cm-dc-btn { "Review" }
        }
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
            summary.btn.small.outline { @if r.status == Status::Open { "Review" } @else { "Change" } }
            div.pop-body {
                form.stack method="post" action=(page.path(&format!("community/suggestions/{}/status", r.id))) {
                    (ui::select("status", &options, r.status.as_str()))
                    textarea.input name="reason" rows="3" placeholder="Reply, shown on the post" maxlength="800" aria-label="Reply" { (r.reason) }
                    @if r.status != Status::Open { p.muted.small { "Keeping the status edits the reply. The author hears only about a new status." } }
                    button.btn.small.primary type="submit" style="align-self:flex-start" { "Save" }
                }
            }
        }
    }
}

/// "10 up, none down"
fn tally_text(t: &Tally) -> String {
    let part = |n: i64, word: &str| if n == 0 { format!("none {word}") } else { format!("{} {word}", text::num(n)) };
    format!("{}, {}", part(t.up, "up"), part(t.down, "down"))
}

/// A small chevron, up or down, for the score tile.
fn arrow(up: bool) -> Markup {
    let d = if up { "M4 9.5 8 5.5l4 4" } else { "M4 6.5 8 10.5l4-4" };
    html! {
        svg.icon viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" {
            path d=(d);
        }
    }
}

fn settings_fields(ctx: &Ctx, guild: u64, s: &Settings) -> Markup {
    html! {
        (ui::field_channel(ctx, guild, "suggestions_channel", "Channel", Some("Where /suggest posts. None turns suggestions off."), s.suggestions_channel))
        (ui::field_role(ctx, guild, "staff_role", "Staff role", Some("Can review suggestions from Discord, besides anyone with Manage Messages."), s.staff_role))
        (ui::field_toggle("threads", "Open a thread", Some("Gives every suggestion a thread to talk it over in."), s.threads))
        (ui::field_toggle("anonymous", "Hide the author", Some("Suggestions show no name. The dashboard still shows who sent them."), s.anonymous))
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
    Ok(saved(&page.path("community/suggestions")))
}
