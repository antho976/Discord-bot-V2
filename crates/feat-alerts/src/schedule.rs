//! The Twitch schedule: segments read from Helix every 30 minutes, reminders before each one,
//! and the weekly image.

use std::fmt::Write as _;
use std::sync::Arc;

use bot_core::anyhow::Result;
use bot_core::discord::{CommandCall, Reply};
use bot_core::jiff::civil::{Date, Weekday};
use bot_core::jiff::tz::TimeZone;
use bot_core::jiff::{Span, Timestamp};
use bot_core::rusqlite::{params, params_from_iter};
use bot_core::twilight_model::channel::message::AllowedMentions;
use bot_core::twilight_model::http::attachment::Attachment;
use bot_core::twilight_model::id::Id;
use bot_core::{Ctx, now_ms, text, tracing};
use bot_render::escape;
use serde::Deserialize;

use crate::twitch::{self, Broadcaster};
use crate::{KEY, Settings, alert_guilds, logins, state_get, state_set};

#[derive(Debug, Clone)]
pub struct Segment {
    pub id: String,
    pub broadcaster_id: String,
    pub starts_at: i64,
    pub ends_at: Option<i64>,
    pub title: String,
    pub game: String,
    pub canceled: bool,
}

/// Read every followed broadcaster's schedule from Helix and replace the stored future segments.
pub async fn refresh(ctx: &Ctx) -> Result<()> {
    #[derive(Deserialize)]
    struct Wrap {
        data: Data,
    }
    #[derive(Deserialize)]
    struct Data {
        segments: Option<Vec<HSegment>>,
    }
    #[derive(Deserialize)]
    struct HSegment {
        id: String,
        start_time: String,
        end_time: Option<String>,
        #[serde(default)]
        title: String,
        canceled_until: Option<String>,
        category: Option<Category>,
    }
    #[derive(Deserialize)]
    struct Category {
        name: String,
    }
    let logins = logins(&alert_guilds(ctx));
    let ms = |s: &str| s.parse::<Timestamp>().ok().map(|t| t.as_millisecond());
    let mut failed = None;
    for b in twitch::broadcasters(ctx).into_iter().filter(|b| logins.contains(&b.login)) {
        let q = [("broadcaster_id", b.user_id.clone()), ("first", "25".to_owned())];
        // 404 means the broadcaster has no schedule. Any other failure keeps what's stored, and
        // the other broadcasters are still read.
        let segments = match twitch::get::<Wrap>(ctx, "schedule", &q).await {
            Ok(w) => w.and_then(|w| w.data.segments).unwrap_or_default(),
            Err(e) => {
                failed = Some(e.context(format!("schedule of {}", b.login)));
                continue;
            }
        };
        let now = now_ms();
        let mut conn = ctx.db.lock();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM alerts_segments WHERE broadcaster_id = ?1 AND starts_at >= ?2", params![b.user_id, now])?;
        for s in &segments {
            let Some(start) = ms(&s.start_time) else { continue };
            tx.execute(
                "INSERT OR REPLACE INTO alerts_segments (id, broadcaster_id, starts_at, ends_at, title, game, canceled) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    s.id,
                    b.user_id,
                    start,
                    s.end_time.as_deref().and_then(ms),
                    s.title,
                    s.category.as_ref().map(|c| c.name.as_str()).unwrap_or_default(),
                    s.canceled_until.is_some() as i64
                ],
            )?;
        }
        tx.execute("DELETE FROM alerts_segments WHERE starts_at < ?1", [now - 8 * 86_400_000])?;
        tx.execute("DELETE FROM alerts_reminders_sent WHERE at < ?1", [now - 14 * 86_400_000])?;
        tx.commit()?;
    }
    failed.map_or(Ok(()), Err)
}

/// Segments of these broadcasters starting in `[from, until)`, soonest first.
pub fn segments(ctx: &Ctx, ids: &[String], from: i64, until: i64) -> Vec<Segment> {
    if ids.is_empty() {
        return Vec::new();
    }
    let marks = vec!["?"; ids.len()].join(", ");
    let sql = format!(
        "SELECT id, broadcaster_id, starts_at, ends_at, title, game, canceled FROM alerts_segments
         WHERE broadcaster_id IN ({marks}) AND starts_at >= {from} AND starts_at < {until} ORDER BY starts_at"
    );
    ctx.db
        .with(|c| {
            let mut s = c.prepare(&sql)?;
            s
                .query_map(params_from_iter(ids.iter()), |r| {
                    Ok(Segment {
                        id: r.get(0)?,
                        broadcaster_id: r.get(1)?,
                        starts_at: r.get(2)?,
                        ends_at: r.get(3)?,
                        title: r.get(4)?,
                        game: r.get(5)?,
                        canceled: r.get::<_, i64>(6)? != 0,
                    })
                })?
                .collect()
        })
        .unwrap_or_default()
}

/// Which reminder to post now for a segment starting at `start`, given the configured offsets
/// (minutes before) and the ones already sent. Returns the offset to post, if any, and every
/// offset to mark as sent. When several are due at once (the bot was down, or the segment was
/// added late) only the closest one is posted, so people don't get two messages in a row.
pub fn due_reminder(now: i64, start: i64, offsets: &[i64], sent: &[i64]) -> (Option<i64>, Vec<i64>) {
    let pending: Vec<i64> = offsets.iter().copied().filter(|m| !sent.contains(m)).collect();
    if start <= now {
        return (None, pending);
    }
    let due: Vec<i64> = pending.into_iter().filter(|m| start - now <= m * 60_000).collect();
    (due.iter().copied().min(), due)
}

pub async fn send_reminders(ctx: &Ctx, guilds: &[(u64, Arc<Settings>)]) {
    let now = now_ms();
    let live: Vec<String> = twitch::open_streams(ctx).unwrap_or_default().into_iter().map(|r| r.broadcaster_id).collect();
    for (guild, s) in guilds {
        let Some(channel) = s.reminder_channel.filter(|_| s.reminders) else { continue };
        let Some(max) = s.reminder_minutes.iter().copied().max() else { continue };
        let bs = twitch::for_settings(ctx, s);
        let ids: Vec<String> = bs.iter().map(|b| b.user_id.clone()).collect();
        for seg in segments(ctx, &ids, now, now + max * 60_000 + 60_000) {
            if seg.canceled || live.contains(&seg.broadcaster_id) {
                continue;
            }
            let key = format!("{}@{}", seg.id, seg.starts_at);
            let sent: Vec<i64> = ctx
                .db
                .with(|c| {
                    let mut st = c.prepare_cached("SELECT minutes FROM alerts_reminders_sent WHERE guild_id = ?1 AND segment = ?2")?;
                    st.query_map(params![*guild as i64, key], |r| r.get(0))?.collect()
                })
                .unwrap_or_default();
            let (post, mark) = due_reminder(now, seg.starts_at, &s.reminder_minutes, &sent);
            if mark.is_empty() {
                continue;
            }
            // Marked before posting: a failing channel shouldn't be retried every minute.
            let _ = ctx.db.with(|c| {
                for m in &mark {
                    c.execute(
                        "INSERT OR IGNORE INTO alerts_reminders_sent (guild_id, segment, minutes, at) VALUES (?1, ?2, ?3, ?4)",
                        params![*guild as i64, key, m, now],
                    )?;
                }
                Ok(())
            });
            if post.is_none() {
                continue;
            }
            let Some(b) = bs.iter().find(|b| b.user_id == seg.broadcaster_id) else { continue };
            let content = reminder_text(s.reminder_role, b, &seg);
            let mentions = AllowedMentions { roles: s.reminder_role.map(Id::new).into_iter().collect(), ..Default::default() };
            if let Err(e) = ctx.http.create_message(Id::new(channel)).content(&content).allowed_mentions(Some(&mentions)).await {
                tracing::warn!(guild, channel, error = %e, "couldn't post a stream reminder");
            }
        }
    }
}

/// "Antho streams <t:..:R>: **Title** (Game)". Discord's relative time stays right even when
/// the reminder is late.
pub fn reminder_text(role: Option<u64>, b: &Broadcaster, seg: &Segment) -> String {
    let at = seg.starts_at / 1000;
    let mut out = String::new();
    if let Some(r) = role {
        let _ = write!(out, "<@&{r}> ");
    }
    let _ = write!(out, "{} streams <t:{at}:R> at <t:{at}:t>", b.name);
    if !seg.title.is_empty() {
        let _ = write!(out, ": **{}**", text::clip(&seg.title, 140));
    }
    if !seg.game.is_empty() {
        let _ = write!(out, " ({})", seg.game);
    }
    let _ = write!(out, "\n<{}>", b.url());
    out
}

// ---- the weekly image ----

/// The Monday starting the week that contains `date`.
pub fn monday_of(date: Date) -> Date {
    date.checked_sub(Span::new().days(date.weekday().to_monday_zero_offset() as i64)).unwrap_or(date)
}

fn day_start(date: Date, tz: &TimeZone) -> i64 {
    date.to_zoned(tz.clone()).map(|z| z.timestamp().as_millisecond()).unwrap_or(0)
}

/// This week's segments for a server, and the Monday it starts on.
pub fn week(ctx: &Ctx, s: &Settings) -> (Vec<Broadcaster>, Date, Vec<Segment>) {
    let tz = s.tz();
    let today = Timestamp::now().to_zoned(tz.clone()).date();
    let monday = monday_of(today);
    let next = monday.checked_add(Span::new().days(7)).unwrap_or(monday);
    let bs = twitch::for_settings(ctx, s);
    let ids: Vec<String> = bs.iter().map(|b| b.user_id.clone()).collect();
    let segs = segments(ctx, &ids, day_start(monday, &tz), day_start(next, &tz));
    (bs, monday, segs)
}

fn time(ms: i64, tz: &TimeZone) -> String {
    Timestamp::from_millisecond(ms).map(|t| t.to_zoned(tz.clone()).strftime("%-I:%M %p").to_string()).unwrap_or_default()
}

/// The week as an SVG in the house style: near-black, off-white text, one coral accent.
pub fn week_svg(name: &str, link: &str, monday: Date, today: Date, tz: &TimeZone, tz_name: &str, segs: &[Segment]) -> String {
    const W: i64 = 960;
    const H: i64 = 884;
    const TOP: i64 = 160;
    const ROW: i64 = 92;
    let sunday = monday.checked_add(Span::new().days(6)).unwrap_or(monday);
    let range = if monday.month() == sunday.month() {
        format!("{} to {}", monday.strftime("%b %-d"), sunday.strftime("%-d"))
    } else {
        format!("{} to {}", monday.strftime("%b %-d"), sunday.strftime("%b %-d"))
    };
    let mut o = String::with_capacity(8 * 1024);
    let _ = write!(
        o,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}"><rect width="{W}" height="{H}" fill="#0f0f0f"/><text x="48" y="72" font-family="Geist Mono" font-size="16" letter-spacing="2" fill="#f0806a">STREAM SCHEDULE</text><text x="48" y="124" font-family="Geist" font-weight="600" font-size="44" fill="#ededed">{}</text><text x="912" y="124" text-anchor="end" font-family="Geist Mono" font-size="20" fill="#ededed" fill-opacity="0.55">{}</text>"##,
        escape(&text::clip(name, 28)),
        escape(&range)
    );
    for i in 0..7i64 {
        let date = monday.checked_add(Span::new().days(i)).unwrap_or(monday);
        let y = TOP + i * ROW;
        let day: Vec<&Segment> = segs
            .iter()
            .filter(|s| Timestamp::from_millisecond(s.starts_at).map(|t| t.to_zoned(tz.clone()).date()).ok() == Some(date))
            .collect();
        let is_today = date == today;
        let past = date < today;
        let _ = write!(o, r##"<line x1="48" y1="{y}" x2="912" y2="{y}" stroke="#ededed" stroke-opacity="0.08"/>"##);
        let _ = write!(o, r#"<g opacity="{}">"#, if past { "0.42" } else { "1" });
        if is_today {
            let _ = write!(o, r##"<rect x="28" y="{}" width="4" height="52" rx="2" fill="#f0806a"/>"##, y + 20);
        }
        let _ = write!(
            o,
            r##"<text x="48" y="{}" font-family="Geist" font-weight="600" font-size="26" fill="{}">{}</text><text x="48" y="{}" font-family="Geist Mono" font-size="18" fill="#ededed" fill-opacity="0.5">{}</text>"##,
            y + 42,
            if is_today { "#f0806a" } else { "#ededed" },
            date.strftime("%a"),
            y + 68,
            date.strftime("%b %-d")
        );
        match day.first() {
            None => {
                let _ = write!(o, r##"<text x="176" y="{}" font-family="Geist" font-size="22" fill="#ededed" fill-opacity="0.3">No stream</text>"##, y + 54);
            }
            Some(seg) => {
                let dim = if seg.canceled { "0.35" } else { "1" };
                let end = if seg.canceled {
                    "Cancelled".to_owned()
                } else {
                    seg.ends_at.map(|e| format!("to {}", time(e, tz))).unwrap_or_default()
                };
                let title = if seg.title.is_empty() { "Stream".to_owned() } else { text::clip(&seg.title, 40) };
                let mut sub = text::clip(&seg.game, 44);
                if day.len() > 1 {
                    let more = format!("+{} more", day.len() - 1);
                    sub = if sub.is_empty() { more } else { format!("{sub}  ·  {more}") };
                }
                let _ = write!(
                    o,
                    r##"<text x="176" y="{}" font-family="Geist Mono" font-size="26" fill="#ededed" fill-opacity="{dim}">{}</text><text x="176" y="{}" font-family="Geist Mono" font-size="18" fill="#ededed" fill-opacity="0.5">{}</text><text x="352" y="{}" font-family="Geist" font-size="26" fill="#ededed" fill-opacity="{dim}"{}>{}</text><text x="352" y="{}" font-family="Geist" font-size="19" fill="#ededed" fill-opacity="0.5">{}</text>"##,
                    y + 42,
                    time(seg.starts_at, tz),
                    y + 68,
                    escape(&end),
                    y + 42,
                    if seg.canceled { r#" text-decoration="line-through""# } else { "" },
                    escape(&title),
                    y + 68,
                    escape(&sub)
                );
            }
        }
        o.push_str("</g>");
    }
    let bottom = TOP + 7 * ROW;
    let _ = write!(
        o,
        r##"<line x1="48" y1="{bottom}" x2="912" y2="{bottom}" stroke="#ededed" stroke-opacity="0.08"/><text x="48" y="{}" font-family="Geist Mono" font-size="16" fill="#ededed" fill-opacity="0.45">Times in {}</text><text x="912" y="{}" text-anchor="end" font-family="Geist Mono" font-size="16" fill="#ededed" fill-opacity="0.45">{}</text></svg>"##,
        bottom + 44,
        escape(tz_name),
        bottom + 44,
        escape(link)
    );
    o
}

/// This week's image for a server, as PNG bytes.
pub async fn week_png(ctx: &Ctx, s: &Settings) -> Result<Vec<u8>> {
    let (bs, monday, segs) = week(ctx, s);
    let tz = s.tz();
    let today = Timestamp::now().to_zoned(tz.clone()).date();
    let name = if bs.is_empty() { "Your streams".to_owned() } else { bs.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(" & ") };
    let link = bs.first().map(|b| format!("twitch.tv/{}", b.login)).unwrap_or_default();
    let svg = week_svg(&name, &link, monday, today, &tz, &s.timezone, &segs);
    bot_core::tokio::task::spawn_blocking(move || bot_render::png(&svg)).await?
}

/// `/schedule`: post this week's image in the channel.
pub async fn command(ctx: &Ctx, call: &CommandCall) -> Result<()> {
    let Some(guild) = call.guild_id() else { return Ok(()) };
    let s = ctx.settings::<Settings>(guild, KEY)?;
    if twitch::for_settings(ctx, &s).is_empty() {
        return ctx.reply(&call.interaction, Reply::text("No Twitch channel is set up for this server yet.").ephemeral()).await;
    }
    ctx.defer(&call.interaction, false).await?;
    let png = week_png(ctx, &s).await?;
    let files = [Attachment::from_bytes("schedule.png".into(), png, 1)];
    ctx.interaction().update_response(&call.interaction.token).attachments(&files).await?;
    Ok(())
}

/// The Monday auto-post, once per server per week at the chosen hour.
pub async fn weekly_posts(ctx: &Ctx, guilds: &[(u64, Arc<Settings>)]) {
    for (guild, s) in guilds {
        let Some(channel) = s.weekly_channel.filter(|_| s.weekly_post) else { continue };
        let now = Timestamp::now().to_zoned(s.tz());
        if now.weekday() != Weekday::Monday || i64::from(now.hour()) < s.weekly_hour {
            continue;
        }
        let key = format!("weekly:{guild}");
        let today = now.date().to_string();
        // Nothing scheduled yet: check again next minute rather than post an empty week.
        if state_get(ctx, &key).as_deref() == Some(today.as_str()) || week(ctx, s).2.is_empty() {
            continue;
        }
        state_set(ctx, &key, &today);
        let res = async {
            let png = week_png(ctx, s).await?;
            let files = [Attachment::from_bytes("schedule.png".into(), png, 1)];
            let none = AllowedMentions::default();
            ctx.http.create_message(Id::new(channel)).content("This week's streams.").attachments(&files).allowed_mentions(Some(&none)).await?;
            Ok::<_, bot_core::anyhow::Error>(())
        }
        .await;
        if let Err(e) = res {
            tracing::warn!(guild, channel, error = %e, "couldn't post the weekly schedule");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    #[test]
    fn reminders_fire_once_each() {
        let start = 1_000 * MIN;
        let offsets = [60, 10];
        // Two hours out: nothing.
        assert_eq!(due_reminder(start - 120 * MIN, start, &offsets, &[]), (None, vec![]));
        // An hour out: the 60-minute one.
        assert_eq!(due_reminder(start - 60 * MIN, start, &offsets, &[]), (Some(60), vec![60]));
        // 30 minutes out, 60 already sent: nothing.
        assert_eq!(due_reminder(start - 30 * MIN, start, &offsets, &[60]), (None, vec![]));
        // 10 minutes out: the 10-minute one.
        assert_eq!(due_reminder(start - 10 * MIN, start, &offsets, &[60]), (Some(10), vec![10]));
        // Both sent: nothing more.
        assert_eq!(due_reminder(start - 2 * MIN, start, &offsets, &[60, 10]), (None, vec![]));
    }

    #[test]
    fn late_reminders_collapse() {
        let start = 1_000 * MIN;
        // First seen 5 minutes before: post only the 10-minute one, mark the hour one too.
        assert_eq!(due_reminder(start - 5 * MIN, start, &[60, 10], &[]), (Some(10), vec![60, 10]));
        // Already started: post nothing, mark everything.
        assert_eq!(due_reminder(start + MIN, start, &[60, 10], &[]), (None, vec![60, 10]));
    }

    #[test]
    fn week_starts_monday() {
        let thu: Date = "2026-10-29".parse().unwrap();
        assert_eq!(monday_of(thu).to_string(), "2026-10-26");
        let mon: Date = "2026-10-26".parse().unwrap();
        assert_eq!(monday_of(mon), mon);
        let sun: Date = "2026-11-01".parse().unwrap();
        assert_eq!(monday_of(sun).to_string(), "2026-10-26");
    }

    #[test]
    fn week_image_renders() {
        let tz = TimeZone::get("America/Toronto").unwrap();
        let monday: Date = "2026-10-26".parse().unwrap();
        let start = monday.at(19, 0, 0, 0).to_zoned(tz.clone()).unwrap().timestamp().as_millisecond();
        let segs = vec![Segment { id: "a".into(), broadcaster_id: "1".into(), starts_at: start, ends_at: Some(start + 3 * 3_600_000), title: "Pets & <stuff>".into(), game: "Idleon".into(), canceled: false }];
        let svg = week_svg("Antho", "twitch.tv/antho", monday, monday, &tz, "America/Toronto", &segs);
        assert!(svg.contains("7:00 PM") && svg.contains("to 10:00 PM") && svg.contains("Pets &amp; &lt;stuff&gt;"));
        assert!(svg.contains("Oct 26 to Nov 1"));
        let png = bot_render::png(&svg).unwrap();
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn reminder_text_reads() {
        let b = Broadcaster { login: "antho".into(), user_id: "1".into(), name: "Antho".into(), avatar: String::new() };
        let seg = Segment { id: "a".into(), broadcaster_id: "1".into(), starts_at: 1_700_000_000_000, ends_at: None, title: "Pets".into(), game: "Idleon".into(), canceled: false };
        assert_eq!(reminder_text(Some(7), &b, &seg), "<@&7> Antho streams <t:1700000000:R> at <t:1700000000:t>: **Pets** (Idleon)\n<https://www.twitch.tv/antho>");
    }
}
