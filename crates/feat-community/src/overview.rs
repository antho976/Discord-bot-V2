//! What the community features add to the server overview: what is running, what is waiting on
//! staff, and the recent wins and approvals for "Around the server".

use bot_core::maud::{Markup, html};
use bot_core::rusqlite::{Connection, params};
use bot_core::web::Page;
use bot_core::web::ui;
use bot_core::{Ctx, Happening, Icon, Need, Overview, now_ms, text};

use crate::id_i64;
use crate::suggestions::Status;

const DAY: i64 = 86_400_000;
/// Most rows in "Running now", soonest first.
const RUNNING_MAX: i64 = 4;
/// Most entries from this feature in "Around the server".
const FEED_MAX: usize = 4;
/// The end of a giveaway: the time it was ended, or its scheduled end for older rows.
const ENDED_AT: &str = "CASE WHEN ended_at > 0 THEN ended_at ELSE ends_at END";

pub fn build(ctx: &Ctx, page: &Page) -> Overview {
    let mut o = Overview::default();
    if let Some(block) = running(ctx, page) {
        o.panel.push((30, block));
    }
    if page.can_edit() {
        o.needs.extend(waiting(ctx, page));
    }
    o.feed = feed(ctx, page.guild_id);
    o
}

/// "45 min", "5 h", "2 days" until a time in epoch milliseconds.
fn until(at: i64) -> String {
    super::span(((at - now_ms()) / 1000).max(0))
}

/// A row of "Running now".
struct Open {
    ends_at: i64,
    icon: Icon,
    title: String,
    detail: String,
    href: String,
}

// ---- running now ----

fn running(ctx: &Ctx, page: &Page) -> Option<Markup> {
    let now = now_ms();
    let (giveaways, polls) = ctx
        .db
        .with(|c| Ok((running_giveaways(c, page, now)?, running_polls(c, page, now)?)))
        .unwrap_or_default();
    let mut rows: Vec<Open> = giveaways.into_iter().chain(polls).collect();
    rows.sort_by_key(|r| r.ends_at);
    rows.truncate(RUNNING_MAX as usize);
    if rows.is_empty() {
        return None;
    }
    Some(html! {
        h2 { "Running now" }
        @for r in &rows {
            a.mini href=(r.href) {
                (ui::icon(r.icon))
                div.grow { span { (r.title) } span.muted { (r.detail) } }
            }
        }
    })
}

fn running_giveaways(c: &Connection, page: &Page, now: i64) -> bot_core::rusqlite::Result<Vec<Open>> {
    let href = page.path("community/giveaways");
    let mut s = c.prepare_cached(
        "SELECT g.prize, g.ends_at, (SELECT COUNT(*) FROM giveaway_entries e WHERE e.giveaway_id = g.id)
         FROM giveaways g WHERE g.guild_id = ?1 AND g.ended = 0 AND g.ends_at > ?2 ORDER BY g.ends_at ASC LIMIT ?3",
    )?;
    s.query_map(params![id_i64(page.guild_id), now, RUNNING_MAX], |r| {
        let (prize, ends_at, entries): (String, i64, i64) = (r.get(0)?, r.get(1)?, r.get(2)?);
        Ok(Open {
            ends_at,
            icon: Icon::Gift,
            title: text::clip(&prize, 60),
            detail: format!("Ends in {} · {} {}", until(ends_at), text::num(entries), if entries == 1 { "entry" } else { "entries" }),
            href: href.clone(),
        })
    })?
    .collect()
}

/// Polls only show when they close: live results aren't stored, so the line leaves them out.
fn running_polls(c: &Connection, page: &Page, now: i64) -> bot_core::rusqlite::Result<Vec<Open>> {
    let href = page.path("community/polls");
    let mut s = c.prepare_cached(
        "SELECT question, ends_at FROM polls WHERE guild_id = ?1 AND ends_at > ?2 ORDER BY ends_at ASC LIMIT ?3",
    )?;
    s.query_map(params![id_i64(page.guild_id), now, RUNNING_MAX], |r| {
        let (question, ends_at): (String, i64) = (r.get(0)?, r.get(1)?);
        Ok(Open {
            ends_at,
            icon: Icon::Poll,
            title: text::clip(&question, 80),
            detail: format!("Closes in {}", until(ends_at)),
            href: href.clone(),
        })
    })?
    .collect()
}

// ---- needs you ----

/// Open suggestions that no staff member has answered for two days, as one line.
fn waiting(ctx: &Ctx, page: &Page) -> Option<Need> {
    let guild = page.guild_id;
    let cutoff = now_ms() - 2 * DAY;
    let (count, oldest, authors) = ctx
        .db
        .with(|c| {
            let (count, oldest): (i64, i64) = c.query_row(
                "SELECT COUNT(*), COALESCE(MIN(created_at), 0) FROM suggestions
                 WHERE guild_id = ?1 AND status = ?2 AND created_at <= ?3",
                params![id_i64(guild), Status::Open.as_str(), cutoff],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let mut s = c.prepare_cached(
                "SELECT author_id FROM suggestions
                 WHERE guild_id = ?1 AND status = ?2 AND created_at <= ?3 AND anonymous = 0
                 ORDER BY created_at ASC LIMIT 2",
            )?;
            let authors: Vec<i64> = s
                .query_map(params![id_i64(guild), Status::Open.as_str(), cutoff], |r| r.get(0))?
                .collect::<bot_core::rusqlite::Result<_>>()?;
            Ok((count, oldest, authors))
        })
        .unwrap_or_default();
    if count == 0 {
        return None;
    }
    Some(Need {
        since: oldest,
        people: authors.iter().map(|a| ctx.member_avatar(guild, *a as u64)).collect(),
        icon: Icon::Bulb,
        title: if count == 1 { "1 suggestion waiting for a reply".into() } else { format!("{count} suggestions waiting for a reply") },
        detail: format!("oldest {}", text::ago(oldest)),
        href: page.path("community/suggestions"),
        action: "Review",
        alert: false,
    })
}

// ---- around the server ----

/// Suggestions approved or implemented in the last three days, and giveaways that ended in the
/// last two days with winners. Anonymous suggestions are left out: there is no one to name.
fn feed(ctx: &Ctx, guild: u64) -> Vec<Happening> {
    let now = now_ms();
    let (suggestions, giveaways) = ctx
        .db
        .with(|c| Ok((decided_suggestions(c, guild, now - 3 * DAY)?, won_giveaways(c, guild, now - 2 * DAY)?)))
        .unwrap_or_default();
    let mut out: Vec<Happening> = suggestions
        .into_iter()
        .map(|(author, status, content, at)| Happening {
            at,
            avatar: ctx.member_avatar(guild, author),
            who: ctx.display_name(guild, author),
            text: format!("had a suggestion {}: {}", status.label().to_lowercase(), text::clip(&one_line(&content), 50)),
        })
        .chain(giveaways.into_iter().map(|(winner, prize, at)| Happening {
            at,
            avatar: ctx.member_avatar(guild, winner),
            who: ctx.display_name(guild, winner),
            text: format!("won {}", text::clip(&prize, 60)),
        }))
        .collect();
    out.sort_by_key(|h| -h.at);
    out.truncate(FEED_MAX);
    out
}

/// `(author, status, content, when)` for approved and implemented suggestions, newest first.
fn decided_suggestions(c: &Connection, guild: u64, since: i64) -> bot_core::rusqlite::Result<Vec<(u64, Status, String, i64)>> {
    let mut s = c.prepare_cached(
        "SELECT author_id, status, content, status_at FROM suggestions
         WHERE guild_id = ?1 AND status IN (?2, ?3) AND anonymous = 0 AND status_at >= ?4
         ORDER BY status_at DESC LIMIT ?5",
    )?;
    let rows = s.query_map(
        params![id_i64(guild), Status::Approved.as_str(), Status::Implemented.as_str(), since, FEED_MAX as i64],
        |r| {
            let status: String = r.get(1)?;
            Ok((r.get::<_, i64>(0)? as u64, Status::parse(&status).unwrap_or(Status::Open), r.get(2)?, r.get(3)?))
        },
    )?;
    rows.collect()
}

/// `(first winner, prize, when)` for giveaways that ended with winners, newest first.
fn won_giveaways(c: &Connection, guild: u64, since: i64) -> bot_core::rusqlite::Result<Vec<(u64, String, i64)>> {
    let mut s = c.prepare_cached(&format!(
        "SELECT winner_ids, prize, {ENDED_AT} AS at FROM giveaways
         WHERE guild_id = ?1 AND ended = 1 AND winner_ids != '' AND {ENDED_AT} >= ?2
         ORDER BY at DESC LIMIT ?3"
    ))?;
    let rows = s.query_map(params![id_i64(guild), since, FEED_MAX as i64], |r| {
        let winners: String = r.get(0)?;
        Ok((winners.split(',').next().and_then(|w| w.parse::<u64>().ok()), r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        if let (Some(winner), prize, at) = row? {
            out.push((winner, prize, at));
        }
    }
    Ok(out)
}

/// A suggestion on one line, for the feed.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line_collapses_whitespace() {
        assert_eq!(one_line("Add a\n  role\tcolour"), "Add a role colour");
    }
}
