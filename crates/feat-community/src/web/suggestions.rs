use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::response::IntoResponse;
use bot_core::maud::{Markup, html};
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, now_ms, text};

use super::{TabQuery, month_start, open_link, style, tabs};
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

/// The colour rule beside a staff reply.
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
        Ok((sg::list(c, guild, 100)?, sg::open_count(c, guild)?, sg::month_stats(c, guild, since)?, sg::top_voters(c, guild, 3)?))
    })?;
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
        (tabs(
            &[
                ("All", page.path("community/suggestions"), None),
                ("Waiting for review", page.path("community/suggestions?tab=review"), Some(waiting)),
                ("Settings", page.path("community/suggestions?tab=settings"), None),
            ],
            active,
        ))
        @if tab == "settings" {
            (ui::settings_form(&page, &page.path("community/suggestions"), ui::card(settings_fields(&ctx, guild, &s))))
        } @else {
            div.with-rail {
                div.stack.cm-main {
                    @if shown.is_empty() {
                        @if tab == "review" {
                            (ui::empty(Icon::Bulb, "Nothing waiting", "New suggestions wait here until staff review them."))
                        } @else if s.suggestions_channel.is_some() {
                            (ui::empty(Icon::Bulb, "No suggestions yet", "Members post them with /suggest. They show up here with their votes."))
                        } @else {
                            (ui::empty(Icon::Bulb, "Suggestions are off", "Pick a channel under Settings and members can use /suggest."))
                        }
                    } @else {
                        div.cm-list {
                            @for (r, t) in &shown { (row(&ctx, &page, guild, r, t, edit)) }
                        }
                    }
                }
                aside.rail {
                    div.rail-block {
                        h2 { "This month" }
                        div.cm-month {
                            div { b.mono { (text::num(month.0)) } span { "suggested" } }
                            div { b.mono { (text::num(month.1)) } span { "approved" } }
                            div { b.mono { (text::num(month.2)) } span { "done" } }
                        }
                    }
                    div.rail-block {
                        h2 { "Most votes given" }
                        @if voters.is_empty() {
                            (ui::empty(Icon::Bulb, "No votes yet", "Members vote with the buttons on each suggestion."))
                        } @else {
                            div.cm-top {
                                @for (u, n) in &voters {
                                    div.cm-voter {
                                        img.avatar src=(ctx.member_avatar(guild, *u)) alt="" width="44" height="44" loading="lazy";
                                        strong { (ctx.display_name(guild, *u)) }
                                        span { (text::num(*n)) " " (if *n == 1 { "vote" } else { "votes" }) }
                                    }
                                }
                            }
                            p.muted.small { "All time. Votes aren't dated, so they can't be counted by month." }
                        }
                    }
                }
            }
        }
    };
    Ok(ui::layout(&ctx, &page, "Suggestions", None, main).into_response())
}

fn row(ctx: &Ctx, page: &Page, guild: u64, r: &Suggestion, t: &Tally, edit: bool) -> Markup {
    let net = t.up - t.down;
    let decided = r.status != Status::Open;
    html! {
        div.cm-sg {
            div class={ "cm-vote" @if net < 0 { " down" } } {
                (arrow(net >= 0))
                span { (net) }
            }
            div.cm-sg-main {
                strong.cm-sg-title title=(r.content) { (text::clip(&r.content, 160)) }
                div.cm-sg-meta {
                    img.avatar src=(ctx.member_avatar(guild, r.author_id)) alt="" loading="lazy";
                    span { strong { (ctx.display_name(guild, r.author_id)) } " · " (text::ago(r.created_at)) " · " (tally_text(t)) }
                    @if r.anonymous { span { "· posted anonymously" } }
                }
                @if decided {
                    blockquote class={ "cm-reply " (reply_class(r.status)) } {
                        @if let Some(staff) = r.staff_id {
                            img.avatar src=(ctx.member_avatar(guild, staff)) alt="" loading="lazy";
                        }
                        span {
                            @if let Some(staff) = r.staff_id { strong { (ctx.display_name(guild, staff)) } ": " }
                            @if r.reason.is_empty() { span.muted { "No reason given." } } @else { (r.reason) }
                        }
                    }
                }
            }
            div.cm-sg-side {
                (ui::badge(state_label(r.status), tone(r.status)))
                @if edit { (review_pop(page, r)) }
                @if !edit { (open_link(page, r.channel_id, r.message_id, "Open in Discord")) }
            }
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
                    input.input type="text" name="reason" value=(r.reason) placeholder="Reason, sent to the author" maxlength="800";
                    button.btn.small.primary type="submit" style="align-self:flex-start" { "Save status" }
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

/// A small chevron, up or down, for the vote tile.
fn arrow(up: bool) -> Markup {
    let d = if up { "M4 9.5 8 5.5l4 4" } else { "M4 6.5 8 10.5l4-4" };
    html! {
        svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" {
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
    match ctx.db.with(|c| sg::get(c, id))? {
        Some(s) if s.guild_id == guild => {}
        _ => return Err(WebError::NotFound),
    }
    let status = Status::parse(&form.string("status")).ok_or_else(|| WebError::Bad("Pick a status.".into()))?;
    sg::set_status(&ctx, id, status, &form.string("reason"), page.user.id).await?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Marked suggestion #{id} {}", status.label().to_lowercase()));
    Ok(saved(&page.path("community/suggestions")))
}
