//! Polls: Discord's own polls, posted by the bot. Discord counts the votes; we remember the poll
//! and, once it's over, its final counts for the dashboard.

use bot_core::anyhow::{Result, anyhow};
use bot_core::discord::{CommandCall, Reply};
use bot_core::rusqlite::{Connection, Row, params};
use bot_core::serde_json::{self, json};
use bot_core::twilight_http::error::ErrorType;
use bot_core::twilight_http::request::Request;
use bot_core::twilight_http::routing::Route;
use bot_core::twilight_model::application::command::{Command, CommandType};
use bot_core::twilight_model::channel::Message;
use bot_core::twilight_model::guild::Permissions;
use bot_core::twilight_model::id::Id;
use bot_core::twilight_util::builder::command::{BooleanBuilder, CommandBuilder, IntegerBuilder, StringBuilder};
use bot_core::{Ctx, now_ms, text, tracing};

use crate::{KEY, id_i64};

pub const MAX_ANSWERS: usize = 10;
pub const MAX_ANSWER_LEN: usize = 55;
pub const MAX_QUESTION_LEN: usize = 300;
pub const MAX_HOURS: i64 = 768;

#[derive(Debug, Clone)]
pub struct Poll {
    pub id: i64,
    pub guild_id: u64,
    pub channel_id: u64,
    pub message_id: u64,
    pub question: String,
    pub answers: Vec<String>,
    pub multi: bool,
    pub ends_at: i64,
    /// Final counts, in answer order. Empty once we've given up fetching them.
    pub results: Option<Vec<i64>>,
}

impl Poll {
    pub fn is_over(&self) -> bool {
        self.ends_at <= now_ms()
    }
}

const COLS: &str = "id, guild_id, channel_id, message_id, question, answers, multi, ends_at, results";

fn from_row(r: &Row) -> bot_core::rusqlite::Result<Poll> {
    let answers: String = r.get(5)?;
    let results: Option<String> = r.get(8)?;
    Ok(Poll {
        id: r.get(0)?,
        guild_id: r.get::<_, i64>(1)? as u64,
        channel_id: r.get::<_, i64>(2)? as u64,
        message_id: r.get::<_, i64>(3)? as u64,
        question: r.get(4)?,
        answers: serde_json::from_str(&answers).unwrap_or_default(),
        multi: r.get::<_, i64>(6)? != 0,
        ends_at: r.get(7)?,
        results: results.and_then(|s| serde_json::from_str(&s).ok()),
    })
}

pub fn recent(c: &Connection, guild: u64, limit: i64) -> bot_core::rusqlite::Result<Vec<Poll>> {
    let mut s = c.prepare_cached(&format!("SELECT {COLS} FROM polls WHERE guild_id = ?1 ORDER BY created_at DESC, id DESC LIMIT ?2"))?;
    s.query_map(params![id_i64(guild), limit], from_row)?.collect()
}

pub fn get(c: &Connection, id: i64) -> bot_core::rusqlite::Result<Option<Poll>> {
    use bot_core::rusqlite::OptionalExtension;
    c.query_row(&format!("SELECT {COLS} FROM polls WHERE id = ?1"), [id], from_row).optional()
}

/// Split "Pizza | Tacos | Soup" (or one answer per line) into answers Discord will accept.
pub fn parse_answers(input: &str) -> Result<Vec<String>, String> {
    let answers: Vec<String> = input
        .split(['|', '\n'])
        .map(|a| a.trim().to_owned())
        .filter(|a| !a.is_empty())
        .collect();
    if answers.len() < 2 {
        return Err("A poll needs at least two answers, separated by |.".into());
    }
    if answers.len() > MAX_ANSWERS {
        return Err(format!("A poll can have {MAX_ANSWERS} answers at most."));
    }
    if let Some(long) = answers.iter().find(|a| a.chars().count() > MAX_ANSWER_LEN) {
        return Err(format!("\"{}\" is too long. Answers can be {MAX_ANSWER_LEN} characters at most.", text::clip(long, 20)));
    }
    Ok(answers)
}

/// The vote counts as `(answer, votes, share)` for display; share is 0 to 100.
pub fn shares(poll: &Poll) -> Vec<(String, i64, i64)> {
    let counts = poll.results.clone().unwrap_or_default();
    let total: i64 = counts.iter().sum();
    poll.answers
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let n = counts.get(i).copied().unwrap_or(0);
            (a.clone(), n, if total > 0 { (n * 100 + total / 2) / total } else { 0 })
        })
        .collect()
}

pub fn command() -> Command {
    CommandBuilder::new("poll", "Post a poll", CommandType::ChatInput)
        .default_member_permissions(Permissions::MANAGE_MESSAGES)
        .option(StringBuilder::new("question", "What to ask").required(true).max_length(MAX_QUESTION_LEN as u16))
        .option(StringBuilder::new("answers", "Two or more answers, separated by |").required(true).max_length(600))
        .option(IntegerBuilder::new("duration", "Hours it stays open, default 24").min_value(1).max_value(MAX_HOURS))
        .option(BooleanBuilder::new("multi", "Let people pick more than one answer"))
        .build()
}

pub async fn on_command(ctx: &Ctx, call: &CommandCall) -> Result<()> {
    let Some(guild) = call.guild_id() else { return Ok(()) };
    let opts = call.opts();
    let answers = match parse_answers(opts.str("answers").unwrap_or_default()) {
        Ok(a) => a,
        Err(e) => return ctx.reply(&call.interaction, Reply::text(e).ephemeral()).await,
    };
    let question = opts.str("question").unwrap_or_default().trim();
    let Some(channel) = call.channel_id() else { return Ok(()) };
    ctx.defer(&call.interaction, true).await?;
    let hours = opts.int("duration").unwrap_or(24).clamp(1, MAX_HOURS);
    let new = New { guild, channel, question, answers: &answers, hours, multi: opts.bool("multi").unwrap_or(false), by: call.user_id() };
    let msg = match create(ctx, new).await {
        Ok(_) => "Poll posted.".to_owned(),
        Err(e) => {
            tracing::warn!(guild, error = %e, "couldn't post a poll");
            "Couldn't post the poll. Check that the bot can send messages in this channel.".to_owned()
        }
    };
    ctx.edit_original(&call.interaction, Reply::text(msg)).await
}

/// What it takes to post a poll.
pub struct New<'a> {
    pub guild: u64,
    pub channel: u64,
    pub question: &'a str,
    pub answers: &'a [String],
    pub hours: i64,
    pub multi: bool,
    pub by: u64,
}

/// Post a native poll and remember it. Returns our id for it.
///
/// twilight's `Poll` type has no `duration`, which Discord needs when creating one, so the
/// message is sent as raw JSON.
pub async fn create(ctx: &Ctx, new: New<'_>) -> Result<i64> {
    let New { guild, channel, question, answers, hours, multi, by } = new;
    let question = text::clip(question.trim(), MAX_QUESTION_LEN);
    let body = json!({
        "poll": {
            "question": { "text": question },
            "answers": answers.iter().map(|a| json!({ "poll_media": { "text": a } })).collect::<Vec<_>>(),
            "duration": hours,
            "allow_multiselect": multi,
            "layout_type": 1,
        }
    });
    let request = Request::builder(&Route::CreateMessage { channel_id: channel }).json(&body).build()?;
    let message: Message = ctx.http.request::<Message>(request).await?.model().await?;
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO polls (guild_id, channel_id, message_id, question, answers, multi, ends_at, created_by, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id_i64(guild),
                id_i64(channel),
                id_i64(message.id.get()),
                question,
                serde_json::to_string(answers).unwrap_or_else(|_| "[]".into()),
                multi as i64,
                now_ms() + hours * 3_600_000,
                id_i64(by),
                now_ms()
            ],
        )?;
        Ok(c.last_insert_rowid())
    })
}

/// Close a poll early. Discord finalises the counts; `tick` collects them.
pub async fn end_now(ctx: &Ctx, id: i64) -> Result<bool> {
    let Some(p) = ctx.db.with(|c| get(c, id))? else { return Ok(false) };
    ctx.http.end_poll(Id::new(p.channel_id), Id::new(p.message_id)).await?;
    ctx.db.with(|c| c.execute("UPDATE polls SET ends_at = ?2 WHERE id = ?1 AND ends_at > ?2", params![id, now_ms()]))?;
    Ok(true)
}

/// Fetch the final counts of polls that have just closed.
pub async fn tick(ctx: &Ctx) {
    for guild in ctx.enabled_guilds(KEY) {
        let waiting: Vec<Poll> = ctx
            .db
            .with(|c| {
                let mut s = c.prepare_cached(&format!(
                    "SELECT {COLS} FROM polls WHERE guild_id = ?1 AND results IS NULL AND ends_at <= ?2 AND ends_at > ?3 LIMIT 5"
                ))?;
                s.query_map(params![id_i64(guild), now_ms(), now_ms() - 7 * 86_400_000], from_row)?.collect()
            })
            .unwrap_or_default();
        for p in waiting {
            match fetch_counts(ctx, &p).await {
                Ok(Some(counts)) => store_results(ctx, p.id, &counts),
                Ok(None) => {}
                Err(e) => {
                    let gone = e.downcast_ref::<bot_core::twilight_http::Error>().is_some_and(
                        |e| matches!(e.kind(), ErrorType::Response { status, .. } if matches!(status.get(), 403 | 404)),
                    );
                    if gone {
                        store_results(ctx, p.id, &[]);
                    } else {
                        tracing::warn!(poll = p.id, error = %e, "couldn't read poll results");
                    }
                }
            }
        }
    }
}

fn store_results(ctx: &Ctx, id: i64, counts: &[i64]) {
    let json = serde_json::to_string(counts).unwrap_or_else(|_| "[]".into());
    if let Err(e) = ctx.db.with(|c| c.execute("UPDATE polls SET results = ?2 WHERE id = ?1", params![id, json])) {
        tracing::warn!(poll = id, error = %e, "couldn't store poll results");
    }
}

/// The final vote counts in answer order, or None while Discord is still counting.
async fn fetch_counts(ctx: &Ctx, p: &Poll) -> Result<Option<Vec<i64>>> {
    let message = ctx.http.message(Id::new(p.channel_id), Id::new(p.message_id)).await?.model().await?;
    let poll = message.poll.ok_or_else(|| anyhow!("the message has no poll"))?;
    let Some(results) = poll.results else { return Ok(None) };
    if !results.is_finalized {
        return Ok(None);
    }
    Ok(Some(
        poll.answers
            .iter()
            .map(|a| results.answer_counts.iter().find(|c| c.id == a.answer_id).map(|c| c.count as i64).unwrap_or(0))
            .collect(),
    ))
}

pub fn seed(ctx: &Ctx, guild: u64) -> Result<()> {
    let hour = 3_600_000i64;
    let now = now_ms();
    type Row = (&'static str, &'static [&'static str], bool, i64, Option<&'static [i64]>);
    let rows: [Row; 5] = [
        ("What should the next stream be?", &["Hades II", "Idleon grind", "Viewer games", "Just chatting"], false, 31 * hour, None),
        ("Movie night on Friday: which one?", &["Dune Part Two", "Spirited Away", "Paddington 2"], false, 6 * hour, None),
        ("Which channels do you actually read?", &["general", "announcements", "stream-alerts", "pets"], true, -20 * hour, Some(&[41, 63, 22, 18])),
        ("New emoji set: keep or revert?", &["Keep", "Revert"], false, -4 * 24 * hour, Some(&[88, 31])),
        ("Pick a colour for the Regulars role", &["Green", "Teal", "Amber"], false, -9 * 24 * hour, Some(&[14, 20, 9])),
    ];
    for (i, (question, answers, multi, ends_in, results)) in rows.into_iter().enumerate() {
        ctx.db.with(|c| {
            c.execute(
                "INSERT INTO polls (guild_id, channel_id, message_id, question, answers, multi, ends_at, created_by, created_at, results)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    id_i64(guild),
                    id_i64(guild + 40),
                    1_300_000_000_000_000_100i64 + i as i64,
                    question,
                    serde_json::to_string(answers).unwrap_or_default(),
                    multi as i64,
                    now + ends_in,
                    id_i64(bot_core::preview::MEMBERS[0].0),
                    now - (i as i64 + 1) * 5 * hour,
                    results.map(|r| serde_json::to_string(r).unwrap_or_default()),
                ],
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_split_on_bars_and_lines() {
        assert_eq!(parse_answers("Pizza | Tacos|  Soup ").unwrap(), vec!["Pizza", "Tacos", "Soup"]);
        assert_eq!(parse_answers("Yes\nNo\n\n").unwrap(), vec!["Yes", "No"]);
    }

    #[test]
    fn answers_are_checked() {
        assert!(parse_answers("Only one").is_err());
        assert!(parse_answers("a | | ").is_err());
        assert!(parse_answers(&"x".repeat(60)).is_err());
        assert!(parse_answers(&(0..11).map(|i| i.to_string()).collect::<Vec<_>>().join("|")).is_err());
        assert!(parse_answers(&(0..10).map(|i| i.to_string()).collect::<Vec<_>>().join("|")).is_ok());
        assert!(parse_answers(&format!("{} | b", "x".repeat(55))).is_ok());
    }

    #[test]
    fn shares_round_to_percent() {
        let p = Poll {
            id: 1,
            guild_id: 1,
            channel_id: 1,
            message_id: 1,
            question: "q".into(),
            answers: vec!["a".into(), "b".into(), "c".into()],
            multi: false,
            ends_at: 0,
            results: Some(vec![1, 1, 2]),
        };
        let s = shares(&p);
        assert_eq!(s.iter().map(|x| x.2).collect::<Vec<_>>(), vec![25, 25, 50]);
        let empty = Poll { results: None, ..p };
        assert!(shares(&empty).iter().all(|x| x.1 == 0 && x.2 == 0));
    }
}
