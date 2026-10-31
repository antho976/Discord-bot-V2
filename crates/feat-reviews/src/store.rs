//! The queue's table, adding requests without duplicates, and the name matching.

use bot_core::anyhow::Result;
use bot_core::rusqlite::{Connection, OptionalExtension, Row, params};
use bot_core::{Ctx, Migration, now_ms};

pub const MIGRATIONS: &[Migration] = &[Migration {
    name: "001_reviews",
    // status: open, done or removed. A removed row stays so its thread or Twitch redemption
    // isn't added again.
    sql: "
        CREATE TABLE reviews (
            id INTEGER PRIMARY KEY, guild_id INTEGER NOT NULL, name TEXT NOT NULL,
            user_id INTEGER, thread_id INTEGER, twitch_user TEXT,
            reward_id TEXT, redemption_id TEXT, redeemed_at INTEGER,
            requested_at INTEGER NOT NULL, status TEXT NOT NULL DEFAULT 'open',
            notes TEXT NOT NULL DEFAULT '', source TEXT NOT NULL,
            pinged_at INTEGER, closed_at INTEGER, closed_by TEXT);
        CREATE INDEX reviews_by_status ON reviews(guild_id, status, requested_at);
        CREATE UNIQUE INDEX reviews_thread ON reviews(guild_id, thread_id) WHERE thread_id IS NOT NULL;
        CREATE UNIQUE INDEX reviews_redemption ON reviews(guild_id, redemption_id) WHERE redemption_id IS NOT NULL;
        CREATE TABLE reviews_twitch (
            guild_id INTEGER PRIMARY KEY, broadcaster_id TEXT NOT NULL, login TEXT NOT NULL,
            access_token TEXT, refresh_token TEXT, expires_at INTEGER NOT NULL DEFAULT 0,
            scopes TEXT NOT NULL DEFAULT '', synced_at INTEGER, error TEXT);
    ",
}, Migration {
    name: "002_reviews_sources",
    // Every post (kind 'thread') and redemption (kind 'redemption') that joined a request, so a
    // merged one is never added again and Complete reaches all of a request's redemptions.
    // twitch is the status the bot set on Twitch for a redemption (FULFILLED or CANCELED), NULL
    // while it's untouched; twitch_pending is 1 until Twitch accepted it.
    sql: "
        CREATE TABLE reviews_sources (
            guild_id INTEGER NOT NULL, kind TEXT NOT NULL, ext_id TEXT NOT NULL,
            review_id INTEGER NOT NULL, reward_id TEXT,
            twitch TEXT, twitch_pending INTEGER NOT NULL DEFAULT 0,
            UNIQUE (guild_id, kind, ext_id));
        CREATE INDEX reviews_sources_by_review ON reviews_sources(review_id);
        CREATE INDEX reviews_sources_pending ON reviews_sources(guild_id) WHERE twitch_pending = 1;
        INSERT INTO reviews_sources (guild_id, kind, ext_id, review_id)
            SELECT guild_id, 'thread', CAST(thread_id AS TEXT), id FROM reviews WHERE thread_id IS NOT NULL;
        INSERT INTO reviews_sources (guild_id, kind, ext_id, review_id, reward_id, twitch, twitch_pending)
            SELECT guild_id, 'redemption', redemption_id, id, reward_id, CASE status WHEN 'done' THEN 'FULFILLED' END, status = 'done'
            FROM reviews WHERE redemption_id IS NOT NULL;
    ",
}];

/// Names at least this alike are the same request, like the old bot.
pub const SAME_NAME: f64 = 0.85;

#[derive(Debug, Clone, PartialEq)]
pub struct Review {
    pub id: i64,
    pub name: String,
    pub user_id: Option<u64>,
    pub thread_id: Option<u64>,
    pub twitch_user: Option<String>,
    pub reward_id: Option<String>,
    pub redemption_id: Option<String>,
    /// Set when it came with channel points: these go first.
    pub redeemed_at: Option<i64>,
    pub requested_at: i64,
    pub status: String,
    pub notes: String,
    pub source: String,
    pub pinged_at: Option<i64>,
    pub closed_at: Option<i64>,
    pub closed_by: Option<String>,
}

const COLS: &str = "id, name, user_id, thread_id, twitch_user, reward_id, redemption_id, redeemed_at, requested_at, status, notes, source, pinged_at, closed_at, closed_by";

fn row(r: &Row) -> bot_core::rusqlite::Result<Review> {
    Ok(Review {
        id: r.get(0)?,
        name: r.get(1)?,
        user_id: r.get::<_, Option<i64>>(2)?.map(|u| u as u64),
        thread_id: r.get::<_, Option<i64>>(3)?.map(|u| u as u64),
        twitch_user: r.get(4)?,
        reward_id: r.get(5)?,
        redemption_id: r.get(6)?,
        redeemed_at: r.get(7)?,
        requested_at: r.get(8)?,
        status: r.get(9)?,
        notes: r.get(10)?,
        source: r.get(11)?,
        pinged_at: r.get(12)?,
        closed_at: r.get(13)?,
        closed_by: r.get(14)?,
    })
}

/// Open requests: channel points first, then the longest waiting.
pub fn open(ctx: &Ctx, guild: u64) -> Result<Vec<Review>> {
    ctx.db.with(|c| open_in(c, guild))
}

fn open_in(c: &Connection, guild: u64) -> bot_core::rusqlite::Result<Vec<Review>> {
    let mut st = c.prepare(&format!(
        "SELECT {COLS} FROM reviews WHERE guild_id = ?1 AND status = 'open' ORDER BY redeemed_at IS NULL, requested_at, id"
    ))?;
    st.query_map([guild as i64], row)?.collect()
}

/// The last 50 finished or removed, newest first.
pub fn closed(ctx: &Ctx, guild: u64) -> Result<Vec<Review>> {
    ctx.db.with(|c| {
        let mut st = c.prepare(&format!("SELECT {COLS} FROM reviews WHERE guild_id = ?1 AND status != 'open' ORDER BY closed_at DESC, id DESC LIMIT 50"))?;
        st.query_map([guild as i64], row)?.collect()
    })
}

pub fn get(ctx: &Ctx, guild: u64, id: i64) -> Result<Option<Review>> {
    ctx.db.with(|c| c.query_row(&format!("SELECT {COLS} FROM reviews WHERE guild_id = ?1 AND id = ?2"), params![guild as i64, id], row).optional())
}

/// Reviews finished in the last week.
pub fn done_since(ctx: &Ctx, guild: u64, since: i64) -> Result<i64> {
    ctx.db.with(|c| c.query_row("SELECT COUNT(*) FROM reviews WHERE guild_id = ?1 AND status = 'done' AND closed_at >= ?2", params![guild as i64, since], |r| r.get(0)))
}

/// A request as it arrives from a thread or a Twitch redemption.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub name: String,
    pub user_id: Option<u64>,
    pub thread_id: Option<u64>,
    pub twitch_user: Option<String>,
    pub reward_id: Option<String>,
    pub redemption_id: Option<String>,
    pub redeemed_at: Option<i64>,
    pub requested_at: i64,
    pub notes: String,
    pub source: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Added {
    /// A new open request with this id.
    New(i64),
    /// The same as this open request: what it lacked (thread, member, channel points) was added.
    Merged(i64),
    /// Its thread or redemption was seen before, whatever happened to it since.
    Known,
}

/// Add a request unless it's already here. A thread or redemption is never added twice, even
/// after it was finished or removed, or merged into another request. Among open requests, the
/// same member, the same Twitch viewer or a name at least [`SAME_NAME`] alike is the same request.
pub fn add(ctx: &Ctx, guild: u64, r: &Request) -> Result<Added> {
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    let sources: Vec<(&str, String)> = r.thread_id.map(|t| ("thread", t.to_string())).into_iter().chain(r.redemption_id.clone().map(|d| ("redemption", d))).collect();
    for (kind, ext) in &sources {
        let seen = tx
            .query_row("SELECT 1 FROM reviews_sources WHERE guild_id = ?1 AND kind = ?2 AND ext_id = ?3", params![guild as i64, kind, ext], |_| Ok(()))
            .optional()?
            .is_some();
        if seen {
            return Ok(Added::Known);
        }
    }
    let open = open_in(&tx, guild)?;
    let (id, added) = match duplicate_of(&open, r) {
        Some(d) => {
            // The request keeps its first post and redemption for links; the rest are in
            // reviews_sources.
            tx.execute(
                "UPDATE reviews SET thread_id = COALESCE(thread_id, ?3), user_id = COALESCE(user_id, ?4),
                 twitch_user = COALESCE(twitch_user, ?5), reward_id = COALESCE(reward_id, ?6),
                 redemption_id = COALESCE(redemption_id, ?7), redeemed_at = COALESCE(redeemed_at, ?8),
                 notes = CASE WHEN notes = '' THEN ?9 ELSE notes END
                 WHERE guild_id = ?1 AND id = ?2",
                params![
                    guild as i64,
                    d.id,
                    r.thread_id.map(|t| t as i64),
                    r.user_id.map(|u| u as i64),
                    r.twitch_user,
                    r.redemption_id.as_ref().filter(|_| d.redemption_id.is_none()).and(r.reward_id.clone()),
                    r.redemption_id.as_ref().filter(|_| d.redemption_id.is_none()),
                    r.redeemed_at,
                    r.notes,
                ],
            )?;
            (d.id, Added::Merged(d.id))
        }
        None => {
            tx.execute(
                "INSERT INTO reviews (guild_id, name, user_id, thread_id, twitch_user, reward_id, redemption_id, redeemed_at, requested_at, notes, source)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    guild as i64,
                    r.name,
                    r.user_id.map(|u| u as i64),
                    r.thread_id.map(|t| t as i64),
                    r.twitch_user,
                    r.reward_id,
                    r.redemption_id,
                    r.redeemed_at,
                    r.requested_at,
                    r.notes,
                    r.source,
                ],
            )?;
            let id = tx.last_insert_rowid();
            (id, Added::New(id))
        }
    };
    for (kind, ext) in &sources {
        add_source(&tx, guild, kind, ext, id, r.reward_id.as_deref().filter(|_| *kind == "redemption"))?;
    }
    tx.commit()?;
    Ok(added)
}

/// Record a post or redemption as part of a request.
pub fn add_source(c: &Connection, guild: u64, kind: &str, ext_id: &str, review: i64, reward_id: Option<&str>) -> bot_core::rusqlite::Result<()> {
    c.execute(
        "INSERT OR IGNORE INTO reviews_sources (guild_id, kind, ext_id, review_id, reward_id) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![guild as i64, kind, ext_id, review, reward_id],
    )?;
    Ok(())
}

/// A channel points redemption the bot is setting on Twitch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redemption {
    pub id: String,
    pub reward_id: String,
    /// FULFILLED or CANCELED.
    pub status: String,
}

/// Set every redemption of a request that's still untouched on Twitch, or waiting for Twitch,
/// to `status` (FULFILLED or CANCELED), pending until [`twitch_settled`]. Returns them.
pub fn set_twitch(ctx: &Ctx, guild: u64, review: i64, status: &str) -> Result<Vec<Redemption>> {
    ctx.db.with(|c| {
        c.execute(
            "UPDATE reviews_sources SET twitch = ?3, twitch_pending = 1
             WHERE guild_id = ?1 AND review_id = ?2 AND kind = 'redemption' AND reward_id IS NOT NULL AND (twitch IS NULL OR twitch_pending = 1)",
            params![guild as i64, review, status],
        )?;
        let mut st = c.prepare("SELECT ext_id, reward_id, twitch FROM reviews_sources WHERE guild_id = ?1 AND review_id = ?2 AND kind = 'redemption' AND twitch_pending = 1")?;
        st.query_map(params![guild as i64, review], |r| Ok(Redemption { id: r.get(0)?, reward_id: r.get(1)?, status: r.get(2)? }))?.collect()
    })
}

/// Twitch took the status, or the redemption is gone from its queue.
pub fn twitch_settled(ctx: &Ctx, guild: u64, redemption: &str) -> Result<()> {
    ctx.db.with(|c| c.execute("UPDATE reviews_sources SET twitch_pending = 0 WHERE guild_id = ?1 AND kind = 'redemption' AND ext_id = ?2", params![guild as i64, redemption]))?;
    Ok(())
}

/// The open request this one repeats, if any. Two similar names from two different known
/// members are two requests (two people titling their posts "Account review").
pub fn duplicate_of<'a>(open: &'a [Review], r: &Request) -> Option<&'a Review> {
    open.iter().find(|o| {
        let same_member = o.user_id.is_some() && o.user_id == r.user_id;
        let same_viewer = matches!((&o.twitch_user, &r.twitch_user), (Some(a), Some(b)) if a.eq_ignore_ascii_case(b));
        let other_member = matches!((o.user_id, r.user_id), (Some(a), Some(b)) if a != b);
        same_member || same_viewer || (!other_member && similarity(&o.name, &r.name) >= SAME_NAME)
    })
}

/// How alike two names are, from 0 to 1: one minus the edit distance over the longer length,
/// ignoring case and outer spaces. Names whose lengths differ by more than a fifth score 0.
pub fn similarity(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.trim().to_lowercase().chars().collect();
    let b: Vec<char> = b.trim().to_lowercase().chars().collect();
    if a == b {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let longer = a.len().max(b.len());
    if a.len().abs_diff(b.len()) as f64 > longer as f64 * 0.2 {
        return 0.0;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    1.0 - prev[b.len()] as f64 / longer as f64
}

pub fn set_notes(ctx: &Ctx, guild: u64, id: i64, notes: &str) -> Result<()> {
    ctx.db.with(|c| c.execute("UPDATE reviews SET notes = ?3 WHERE guild_id = ?1 AND id = ?2", params![guild as i64, id, notes]))?;
    Ok(())
}

pub fn mark_pinged(ctx: &Ctx, guild: u64, id: i64) -> Result<()> {
    ctx.db.with(|c| c.execute("UPDATE reviews SET pinged_at = ?3 WHERE guild_id = ?1 AND id = ?2", params![guild as i64, id, now_ms()]))?;
    Ok(())
}

/// Close a request as `done` or `removed`, or open it again with `open`.
pub fn set_status(ctx: &Ctx, guild: u64, id: i64, status: &str, by: &str) -> Result<()> {
    ctx.db.with(|c| {
        if status == "open" {
            c.execute("UPDATE reviews SET status = 'open', closed_at = NULL, closed_by = NULL WHERE guild_id = ?1 AND id = ?2", params![guild as i64, id])
        } else {
            c.execute("UPDATE reviews SET status = ?3, closed_at = ?4, closed_by = ?5 WHERE guild_id = ?1 AND id = ?2", params![guild as i64, id, status, now_ms(), by])
        }
    })?;
    Ok(())
}

/// What a viewer typed when redeeming: who the review is for, and anything after it.
/// "review for Mira - check alchemy" is ("Mira", "check alchemy"); "Mira (main)" is
/// ("Mira", "main").
pub fn parse_input(input: &str) -> (String, String) {
    let mut s = input.trim();
    for prefix in ["reviewing for ", "review for ", "for ", "reviewing ", "review "] {
        if s.len() > prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
            s = s[prefix.len()..].trim_start();
            break;
        }
    }
    if let Some(open) = s.find('(')
        && s.ends_with(')')
        && open > 0
    {
        return (s[..open].trim().to_owned(), s[open + 1..s.len() - 1].trim().to_owned());
    }
    for sep in [", ", " - ", " – "] {
        if let Some(i) = s.find(sep)
            && i > 0
        {
            return (s[..i].trim().to_owned(), s[i + sep.len()..].trim().to_owned());
        }
    }
    (s.to_owned(), String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review(id: i64, name: &str, user: Option<u64>, twitch: Option<&str>) -> Review {
        Review {
            id,
            name: name.into(),
            user_id: user,
            thread_id: None,
            twitch_user: twitch.map(Into::into),
            reward_id: None,
            redemption_id: None,
            redeemed_at: None,
            requested_at: 0,
            status: "open".into(),
            notes: String::new(),
            source: "thread".into(),
            pinged_at: None,
            closed_at: None,
            closed_by: None,
        }
    }

    #[test]
    fn names_compare_like_the_old_bot() {
        assert_eq!(similarity("Mira", " mira "), 1.0);
        assert!(similarity("AnthoMain", "AnthoMainn") >= SAME_NAME);
        assert!(similarity("bramblepaw", "bramblepow") >= SAME_NAME);
        assert!(similarity("Teo", "Leo") < SAME_NAME);
        // Lengths a fifth apart never match.
        assert_eq!(similarity("Kasia", "Kasia the Great"), 0.0);
        assert_eq!(similarity("", "x"), 0.0);
    }

    #[test]
    fn duplicates_by_member_viewer_or_name() {
        let open = vec![review(1, "AnthoMain", Some(10), None), review(2, "Pavel", None, Some("pavel_tv"))];
        let req = |name: &str, user: Option<u64>, twitch: Option<&str>| Request { name: name.into(), user_id: user, twitch_user: twitch.map(Into::into), ..Request::default() };
        assert_eq!(duplicate_of(&open, &req("Something else", Some(10), None)).map(|r| r.id), Some(1));
        assert_eq!(duplicate_of(&open, &req("Main acct", None, Some("PAVEL_TV"))).map(|r| r.id), Some(2));
        assert_eq!(duplicate_of(&open, &req("anthomainn", None, None)).map(|r| r.id), Some(1));
        // A similar name from someone else is their own request.
        assert_eq!(duplicate_of(&open, &req("AnthoMainn", Some(11), None)), None);
        assert_eq!(duplicate_of(&open, &req("Noor", Some(12), None)), None);
    }

    #[test]
    fn redemption_input_names_who_it_is_for() {
        assert_eq!(parse_input("Mira"), ("Mira".into(), String::new()));
        assert_eq!(parse_input("review for Mira - check alchemy"), ("Mira".into(), "check alchemy".into()));
        assert_eq!(parse_input("For John Doe, main account"), ("John Doe".into(), "main account".into()));
        assert_eq!(parse_input("Mira (second account)"), ("Mira".into(), "second account".into()));
        assert_eq!(parse_input("  "), (String::new(), String::new()));
        assert_eq!(parse_input("review"), ("review".into(), String::new()));
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn requests_are_added_once() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate("reviews", MIGRATIONS).unwrap();
        let thread = Request { name: "AnthoMain".into(), user_id: Some(10), thread_id: Some(500), requested_at: 1, source: "thread", ..Request::default() };
        let Added::New(id) = add(&ctx, 1, &thread).unwrap() else { panic!("first add is new") };
        assert_eq!(add(&ctx, 1, &thread).unwrap(), Added::Known);
        // The same thread in another server is another request.
        assert!(matches!(add(&ctx, 2, &thread).unwrap(), Added::New(_)));
        // A redemption for the same account moves it up instead of adding a second one.
        let points = Request { name: "anthomain".into(), twitch_user: Some("antho".into()), reward_id: Some("rw".into()), redemption_id: Some("rd1".into()), redeemed_at: Some(5), requested_at: 5, source: "twitch", ..Request::default() };
        assert_eq!(add(&ctx, 1, &points).unwrap(), Added::Merged(id));
        let r = get(&ctx, 1, id).unwrap().unwrap();
        assert_eq!((r.redeemed_at, r.redemption_id.as_deref(), r.reward_id.as_deref(), r.thread_id), (Some(5), Some("rd1"), Some("rw"), Some(500)));
        // Removed stays as a marker: neither the thread nor the redemption comes back.
        set_status(&ctx, 1, id, "removed", "Antho").unwrap();
        assert_eq!(add(&ctx, 1, &thread).unwrap(), Added::Known);
        assert_eq!(add(&ctx, 1, &points).unwrap(), Added::Known);
        assert!(open(&ctx, 1).unwrap().is_empty());
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn merged_posts_and_redemptions_are_remembered() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate("reviews", MIGRATIONS).unwrap();
        // A second post from the same member merges, and stays known after Complete.
        let post = |thread: u64| Request { name: "AnthoMain".into(), user_id: Some(10), thread_id: Some(thread), requested_at: 1, source: "thread", ..Request::default() };
        let Added::New(id) = add(&ctx, 1, &post(500)).unwrap() else { panic!("first post is new") };
        assert_eq!(add(&ctx, 1, &post(501)).unwrap(), Added::Merged(id));
        set_status(&ctx, 1, id, "done", "Antho").unwrap();
        assert_eq!(add(&ctx, 1, &post(501)).unwrap(), Added::Known);
        assert!(open(&ctx, 1).unwrap().is_empty());
        // A second redemption from the same viewer merges, is known on the next sync, and is
        // fulfilled with the first.
        let points = |rd: &str| Request { name: "Moonfrost".into(), twitch_user: Some("pavel_tv".into()), reward_id: Some("rw".into()), redemption_id: Some(rd.into()), redeemed_at: Some(5), requested_at: 5, source: "twitch", ..Request::default() };
        let Added::New(id) = add(&ctx, 1, &points("rd1")).unwrap() else { panic!("first redemption is new") };
        assert_eq!(add(&ctx, 1, &points("rd2")).unwrap(), Added::Merged(id));
        assert_eq!(add(&ctx, 1, &points("rd2")).unwrap(), Added::Known);
        let ids = |list: Vec<Redemption>| list.into_iter().map(|r| (r.id, r.status)).collect::<Vec<_>>();
        let f = |id: &str| (id.to_owned(), "FULFILLED".to_owned());
        assert_eq!(ids(set_twitch(&ctx, 1, id, "FULFILLED").unwrap()), [f("rd1"), f("rd2")]);
        // One Twitch took stays done; the other is still pending.
        twitch_settled(&ctx, 1, "rd1").unwrap();
        assert_eq!(ids(set_twitch(&ctx, 1, id, "FULFILLED").unwrap()), [f("rd2")]);
        twitch_settled(&ctx, 1, "rd2").unwrap();
        assert!(set_twitch(&ctx, 1, id, "FULFILLED").unwrap().is_empty());
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn existing_requests_fill_the_sources() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate("reviews", &MIGRATIONS[..1]).unwrap();
        ctx.db
            .with(|c| {
                c.execute_batch(
                    "INSERT INTO reviews (guild_id, name, thread_id, reward_id, redemption_id, requested_at, status, source) VALUES
                     (1, 'Kasia', 500, 'rw', 'rd1', 1, 'done', 'thread'), (1, 'Teo', 501, NULL, NULL, 2, 'open', 'thread')",
                )
            })
            .unwrap();
        ctx.db.migrate("reviews", MIGRATIONS).unwrap();
        let req = |thread: Option<u64>, rd: Option<&str>| Request { name: "Someone new".into(), thread_id: thread, redemption_id: rd.map(Into::into), source: "thread", ..Request::default() };
        assert_eq!(add(&ctx, 1, &req(Some(500), None)).unwrap(), Added::Known);
        assert_eq!(add(&ctx, 1, &req(Some(501), None)).unwrap(), Added::Known);
        assert_eq!(add(&ctx, 1, &req(None, Some("rd1"))).unwrap(), Added::Known);
        // A done request's redemption is retried once in case its fulfil failed.
        let pending: i64 = ctx.db.with(|c| c.query_row("SELECT twitch_pending FROM reviews_sources WHERE ext_id = 'rd1'", [], |r| r.get(0))).unwrap();
        assert_eq!(pending, 1);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn channel_points_go_first_then_the_oldest() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.db.migrate("reviews", MIGRATIONS).unwrap();
        for (name, user, at, redeemed) in [("Old", 1, 10, None), ("Newer", 2, 20, None), ("Paid", 3, 30, Some(30))] {
            add(&ctx, 1, &Request { name: name.into(), user_id: Some(user), requested_at: at, redeemed_at: redeemed, source: "thread", ..Request::default() }).unwrap();
        }
        let names: Vec<String> = open(&ctx, 1).unwrap().into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["Paid", "Old", "Newer"]);
    }
}
