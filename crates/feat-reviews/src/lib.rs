//! Idleon account reviews: a queue of review requests from a forum channel's posts and from
//! Twitch channel points, worked through on the dashboard. A queue only; reviews happen in
//! the threads.

pub mod store;
pub mod twitch;
mod web;
pub mod words;

use bot_core::anyhow::Result;
use bot_core::twilight_http::request::channel::reaction::RequestReactionType;
use bot_core::twilight_model::channel::message::MessageType;
use bot_core::twilight_model::channel::{Channel, Message};
use bot_core::twilight_model::gateway::event::Event;
use bot_core::twilight_model::guild::Permissions;
use bot_core::twilight_model::id::Id;
use bot_core::twilight_model::id::marker::TagMarker;
use bot_core::{ComponentCall, Ctx, Feature, FeatureMeta, Group, Icon, Migration, NavItem, Need, Overview, Reply, feature_async, now_ms, text};
use serde::{Deserialize, Serialize};

use store::{Added, Request, Tally};

pub const KEY: &str = "reviews";
/// Forum tags that mean a post was dealt with; a scan skips those posts.
const DONE_TAGS: &[&str] = &["done", "closed", "completed", "finished", "resolved"];
pub const DEFAULT_PING: &str = "{mention}, it's your turn for an account review. Answer here when you're ready.";
/// A scan takes archived posts created this recently. Forums archive idle posts, so older
/// archived ones are mostly reviews that were finished without a tag.
const SCAN_ARCHIVED_MS: i64 = 30 * 86_400_000;
pub const DEFAULT_DONE: &str = "Your account review is done. Thanks for waiting.";
pub const DEFAULT_NUDGE: &str = "{mention}, please share your IdleonToolbox profile link in this post so it's ready for your review. It looks like <https://idleontoolbox.com/?profile=YourName>.";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The forum (or text channel) whose new posts are requests.
    pub channel: Option<u64>,
    /// Posted in the thread by Ping. `{mention}` and `{name}`.
    pub ping_message: String,
    /// Filled in when completing a request; it can be changed or cleared each time.
    pub done_message: String,
    /// Posted once in a new post with no IdleonToolbox link. Empty asks nothing.
    pub nudge_message: String,
    /// The channel points reward whose redemptions are requests.
    pub reward_id: Option<String>,
    pub reward_title: String,
    /// Hours after a review before the bot locks its post, so the thread can't fill with chatter
    /// later. The member can lock it sooner with the button under the closing message. 0 leaves
    /// posts open.
    pub lock_hours: i64,
    /// Days after someone's last completed review before a new request of theirs joins the
    /// queue; until then it waits on the waitlist. 0 turns the waitlist off.
    pub wait_days: i64,
}

impl Settings {
    pub fn wait_ms(&self) -> i64 {
        self.wait_days.max(0) * 86_400_000
    }

    pub fn lock_ms(&self) -> i64 {
        self.lock_hours.max(0) * 3_600_000
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            channel: None,
            ping_message: DEFAULT_PING.into(),
            done_message: DEFAULT_DONE.into(),
            nudge_message: DEFAULT_NUDGE.into(),
            reward_id: None,
            reward_title: String::new(),
            lock_hours: 24,
            wait_days: 7,
        }
    }
}

pub struct Reviews;

#[feature_async]
impl Feature for Reviews {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Account reviews",
            description: "A queue of Idleon account review requests from a forum channel and Twitch channel points.",
            group: Group::Pets,
            nav: vec![NavItem { label: "Account reviews", path: "reviews", icon: Icon::Review, about: "Review requests from your forum and channel points, in the order you take them." }],
        }
    }

    fn migrations(&self) -> &'static [Migration] {
        store::MIGRATIONS
    }

    fn routes(&self) -> bot_core::axum::Router<Ctx> {
        web::routes()
    }

    async fn on_event(&self, ctx: &Ctx, event: &Event) -> Result<()> {
        if let Event::MessageCreate(m) = event {
            return on_message(ctx, m).await;
        }
        let Event::ThreadCreate(t) = event else { return Ok(()) };
        let Some(guild) = t.guild_id.map(|g| g.get()) else { return Ok(()) };
        if t.newly_created != Some(true) {
            return Ok(());
        }
        let s = ctx.settings::<Settings>(guild, KEY)?;
        if s.channel.is_none() || t.parent_id.map(|p| p.get()) != s.channel {
            return Ok(());
        }
        let Some(req) = request_from_thread(ctx, &t.0, "thread") else { return Ok(()) };
        let added = store::add(ctx, guild, &req, s.wait_ms())?;
        tag_from_title(ctx, guild, added, &req.name)?;
        // A waitlisted request gets no notice; it joins the queue quietly when its time comes.
        if let Added::New(_) = added {
            let who = req.user_id.map(|u| ctx.display_name(guild, u)).unwrap_or_else(|| "Someone".into());
            ctx.notify(Some(guild), KEY, format!("{who} asked for an account review: {}", req.name), Some(format!("/g/{guild}/reviews")));
        }
        // The post's first message can arrive before the request was stored, and be passed
        // over, so it's read here too. A forum post's first message has the post's id.
        if let (Added::New(id) | Added::Waiting(id, _) | Added::Merged(id), Some(member), Some(thread)) = (added, req.user_id, Id::new_checked(t.id.get())) {
            match ctx.http.message(thread, thread.cast()).await {
                Ok(res) => {
                    let m = res.model().await?;
                    if m.author.id.get() == member {
                        take_in(ctx, guild, id, &m.content)?;
                    }
                }
                Err(e) => bot_core::tracing::debug!(guild, error = %e, "no first message to read in a review post"),
            }
        }
        Ok(())
    }

    /// Fetches channel points redemptions every 5 minutes for servers that connected Twitch,
    /// in the background so a slow Twitch doesn't hold up the other features' minute.
    /// Also moves waitlisted requests whose time has come into the queue, and checks each
    /// stored Twitch sign-in once an hour so one Twitch revoked is noticed within the hour.
    async fn tick(&self, ctx: &Ctx) -> Result<()> {
        let now = now_ms();
        store::promote_due(ctx, now)?;
        for guild in ctx.enabled_guilds(KEY) {
            let s = ctx.settings::<Settings>(guild, KEY)?;
            if s.lock_hours > 0 {
                let due = store::lock_due(ctx, guild, now, s.lock_ms())?;
                if !due.is_empty() {
                    let ctx = ctx.clone();
                    bot_core::tokio::spawn(async move {
                        for r in &due {
                            if let Err(e) = lock_post(&ctx, guild, r).await {
                                // Not every minute: an hour on, so a missing permission doesn't
                                // keep the other posts waiting.
                                if let Err(e) = store::lock_failed(&ctx, guild, r.id) {
                                    bot_core::tracing::warn!(guild, review = r.id, error = %e, "couldn't note a failed lock");
                                }
                                bot_core::tracing::warn!(guild, review = r.id, error = %e, "couldn't lock a finished review post");
                                ctx.notify_once(guild, KEY, "lock", format!("The bot couldn't lock {}'s finished review post: {e}", r.name), Some(format!("/g/{guild}/reviews/done")), std::time::Duration::from_secs(86_400));
                            }
                        }
                    });
                }
            }
            if s.nudge_message.trim().is_empty() {
                continue;
            }
            let due = store::nudge_due(ctx, guild, now)?;
            if due.is_empty() {
                continue;
            }
            let ctx = ctx.clone();
            let template = s.nudge_message.clone();
            bot_core::tokio::spawn(async move {
                for r in &due {
                    if let Err(e) = nudge(&ctx, guild, r, &template).await {
                        bot_core::tracing::warn!(guild, review = r.id, error = %e, "couldn't ask for a toolbox link");
                    }
                }
            });
        }
        for guild in twitch::due_checks(ctx, now) {
            let ctx = ctx.clone();
            bot_core::tokio::spawn(async move { twitch::keep_alive(&ctx, guild).await });
        }
        for guild in ctx.enabled_guilds(KEY) {
            let s = ctx.settings::<Settings>(guild, KEY)?;
            let Some(reward) = s.reward_id.clone() else { continue };
            let wait = s.wait_ms();
            if !twitch::due(ctx, guild, now) {
                continue;
            }
            let ctx = ctx.clone();
            bot_core::tokio::spawn(async move {
                if let Err(e) = twitch::sync(&ctx, guild, &reward, wait).await {
                    bot_core::tracing::warn!(guild, error = %e, "couldn't sync channel points redemptions");
                    twitch::note_error(&ctx, guild, Some(&e.to_string()));
                }
            });
        }
        Ok(())
    }

    /// "Lock post" under the closing message: the member who asked, or staff who can manage
    /// threads, close the post to new messages before the timer does.
    async fn on_component(&self, ctx: &Ctx, call: &ComponentCall) -> Result<()> {
        let Some(guild) = call.guild_id() else { return Ok(()) };
        let args = call.args();
        let ["lock", id] = args.as_slice() else { return Ok(()) };
        let say = |t: &str| Reply::text(t).ephemeral();
        let Some(r) = id.parse::<i64>().ok().map(|id| store::get(ctx, guild, id)).transpose()?.flatten() else {
            return ctx.reply(&call.interaction, say("This review isn't in the queue any more.")).await;
        };
        if r.user_id != Some(call.user_id()) && !call.member_has(Permissions::MANAGE_THREADS) {
            return ctx.reply(&call.interaction, say("Only the person who asked for this review, or staff, can lock the post.")).await;
        }
        if r.status != "done" {
            return ctx.reply(&call.interaction, say("This review is back in the queue, so the post stays open.")).await;
        }
        if r.locked_at.is_some() {
            return ctx.reply(&call.interaction, say("This post is already locked.")).await;
        }
        // Locking takes two requests to Discord; the answer can't wait that long.
        ctx.defer(&call.interaction, true).await?;
        match lock_post(ctx, guild, &r).await {
            Ok(()) => ctx.edit_original(&call.interaction, Reply::text("Locked. Only staff can post here now.")).await,
            Err(e) => ctx.edit_original(&call.interaction, Reply::text(format!("The bot couldn't lock this post: {e}"))).await,
        }
    }

    fn overview(&self, ctx: &Ctx, page: &bot_core::web::Page) -> Overview {
        let guild = page.guild_id;
        let mut o = Overview::default();
        let open = store::open(ctx, guild).unwrap_or_default();
        let Some(oldest) = open.iter().map(|r| r.requested_at).min() else { return o };
        let people: Vec<u64> = open.iter().filter_map(|r| r.user_id).take(2).collect();
        let names: Vec<String> = open.iter().take(2).map(|r| r.name.clone()).collect();
        let more = open.len().saturating_sub(names.len());
        let n = open.len();
        o.needs.push(Need {
            since: oldest,
            people: people.iter().map(|u| ctx.member_avatar(guild, *u)).collect(),
            icon: Icon::Review,
            title: format!("{n} account {} waiting", if n == 1 { "review" } else { "reviews" }),
            detail: format!("{}{} · oldest {}", names.join(", "), if more > 0 { format!(" and {more} more") } else { String::new() }, text::ago(oldest)),
            href: page.path("reviews"),
            action: "Review",
            alert: false,
        });
        o
    }

    fn seed_preview(&self, ctx: &Ctx, guild: u64) -> Result<()> {
        web::seed(ctx, guild)
    }
}

/// A request from a new post (thread): its title is the name, its starter the member.
fn request_from_thread(ctx: &Ctx, t: &Channel, source: &'static str) -> Option<Request> {
    let owner = t.owner_id.map(|o| o.get());
    if owner.is_some() && owner == ctx.bot_id() {
        return None;
    }
    let name = text::clip(t.name.as_deref().unwrap_or_default().trim(), 100);
    if name.is_empty() {
        return None;
    }
    let at = created_ms(t);
    Some(Request { name, user_id: owner, thread_id: Some(t.id.get()), requested_at: at, source, ..Request::default() })
}

/// When a post was made.
fn created_ms(t: &Channel) -> i64 {
    t.thread_metadata.as_ref().and_then(|m| m.create_timestamp).map(|ts| ts.as_micros() / 1000).unwrap_or_else(|| bot_core::discord::snowflake_ms(t.id.get()))
}

/// Look through the channel's open posts, and up to 50 archived ones made in the last 30 days,
/// for requests missed while the bot was away, skipping posts tagged as done. Someone reviewed
/// within `wait_ms` goes on the waitlist.
pub async fn scan(ctx: &Ctx, guild: u64, channel: u64, wait_ms: i64) -> Result<Tally> {
    let done: Vec<Id<TagMarker>> = ctx
        .cache
        .channel(Id::new(channel))
        .and_then(|c| c.available_tags.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|t| DONE_TAGS.contains(&t.name.trim().to_lowercase().as_str()))
        .map(|t| t.id)
        .collect();
    let mut threads: Vec<Channel> = ctx
        .http
        .active_threads(Id::new(guild))
        .await?
        .model()
        .await?
        .threads
        .into_iter()
        .filter(|t| t.parent_id.map(|p| p.get()) == Some(channel))
        .collect();
    match ctx.http.public_archived_threads(Id::new(channel)).limit(50).await {
        Ok(r) => {
            let since = now_ms() - SCAN_ARCHIVED_MS;
            threads.extend(r.model().await?.threads.into_iter().filter(|t| created_ms(t) >= since));
        }
        Err(e) => bot_core::tracing::warn!(guild, channel, error = %e, "couldn't list archived posts"),
    }
    let mut tally = Tally::default();
    for t in &threads {
        if t.applied_tags.as_ref().is_some_and(|tags| tags.iter().any(|id| done.contains(id))) {
            continue;
        }
        let Some(req) = request_from_thread(ctx, t, "scan") else { continue };
        let added = store::add(ctx, guild, &req, wait_ms)?;
        tag_from_title(ctx, guild, added, &req.name)?;
        tally.count(added);
    }
    Ok(tally)
}

/// Tag a request from the title of a post that just joined it.
fn tag_from_title(ctx: &Ctx, guild: u64, added: Added, title: &str) -> Result<()> {
    if let Added::New(id) | Added::Waiting(id, _) | Added::Merged(id) = added {
        store::add_tags(ctx, guild, id, &words::tags_in(title))?;
    }
    Ok(())
}

/// Answer to a ping, posted in the thread. `{staff}` is who pressed Ping, the only one pinged.
const READY_REPLY: &str = "{staff}, {name} is ready for their review.";

/// A message in a request's post. From the member, it can bring their toolbox link and tags,
/// and once pinged, an answer that they're ready: that pings whoever pressed Ping, once.
async fn on_message(ctx: &Ctx, m: &Message) -> Result<()> {
    let Some(guild) = m.guild_id.map(|g| g.get()) else { return Ok(()) };
    if m.author.bot || !matches!(m.kind, MessageType::Regular | MessageType::Reply) {
        return Ok(());
    }
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let Some(channel) = s.channel else { return Ok(()) };
    // Most messages are elsewhere; the cache says so without a database lookup.
    if ctx.cache.channel(m.channel_id).is_some_and(|c| c.parent_id.map(|p| p.get()) != Some(channel)) {
        return Ok(());
    }
    let Some(r) = store::by_thread(ctx, guild, m.channel_id.get())? else { return Ok(()) };
    if r.user_id != Some(m.author.id.get()) {
        return Ok(());
    }
    take_in(ctx, guild, r.id, &m.content)?;
    let said_at = m.timestamp.as_micros() / 1000;
    if r.pinged_at.is_none() || r.ready_at.is_some() || !words::says_ready(&m.content) || !store::claim_ready(ctx, guild, r.id, said_at)? {
        return Ok(());
    }
    let who = ctx.display_name(guild, m.author.id.get());
    ctx.notify(Some(guild), KEY, format!("{who} is ready for their account review: {}", r.name), Some(format!("/g/{guild}/reviews")));
    // Pings from before the bot kept who pressed Ping have no one to tell in the thread.
    let Some(staff) = r.pinged_by.filter(|u| *u != 0) else { return Ok(()) };
    let reply = text::fill(READY_REPLY, &[("staff", &format!("<@{staff}>")), ("name", &text::escape_markdown(&who))]);
    if let Err(e) = web::post_in(ctx, m.channel_id.get(), &reply, Some(staff)).await {
        bot_core::tracing::warn!(guild, review = r.id, error = %e, "couldn't tell staff a member is ready");
    }
    Ok(())
}

/// What a member's own message brings to their request: their newest toolbox link, and tags.
fn take_in(ctx: &Ctx, guild: u64, id: i64, content: &str) -> Result<()> {
    if let Some(link) = words::toolbox_link(content) {
        store::set_toolbox(ctx, guild, id, &link)?;
    }
    store::add_tags(ctx, guild, id, &words::tags_in(content))?;
    Ok(())
}

/// Ask once for a toolbox link in a new post that has none. The post's messages are read
/// first: the link may have come in before the request was stored, or while the bot was away.
async fn nudge(ctx: &Ctx, guild: u64, r: &store::Review, template: &str) -> Result<()> {
    let (Some(thread), Some(member)) = (r.thread_id, r.user_id) else { return Ok(()) };
    if !store::claim_nudge(ctx, guild, r.id)? {
        return Ok(());
    }
    let Some(thread_id) = Id::new_checked(thread) else { return Ok(()) };
    match ctx.http.channel_messages(thread_id).limit(100).await {
        Ok(res) => {
            // Newest first.
            let mine: Vec<Message> = res.models().await?.into_iter().filter(|m| m.author.id.get() == member).collect();
            for m in mine.iter().rev() {
                store::add_tags(ctx, guild, r.id, &words::tags_in(&m.content))?;
            }
            if let Some(link) = mine.iter().find_map(|m| words::toolbox_link(&m.content)) {
                store::set_toolbox(ctx, guild, r.id, &link)?;
                return Ok(());
            }
        }
        Err(e) if bot_core::discord::is_not_found(&e) => return Ok(()),
        Err(e) => bot_core::tracing::warn!(guild, review = r.id, error = %e, "couldn't read a review post before asking for a toolbox link"),
    }
    web::post_in(ctx, thread, &web::fill(template, r), Some(member)).await.map_err(bot_core::anyhow::Error::msg)
}

/// Lock a finished request's post and close it in the forum, once. A failure lets the lock go,
/// so the button or the dashboard can try again; a post that's gone stays marked.
pub(crate) async fn lock_post(ctx: &Ctx, guild: u64, r: &store::Review) -> Result<()> {
    if !store::claim_lock(ctx, guild, r.id)? || ctx.cfg.preview {
        return Ok(());
    }
    let Some(thread) = r.thread_id.and_then(Id::new_checked) else { return Ok(()) };
    // An archived post takes no other change, so it's opened, locked, then archived again.
    let res = async {
        ctx.http.update_thread(thread).archived(false).locked(true).await?;
        ctx.http.update_thread(thread).archived(true).await
    }
    .await;
    match res {
        Ok(_) => {
            // Sent back to the queue while the lock was on its way: open the post again.
            if let Some(now) = store::get(ctx, guild, r.id)?
                && now.status != "done"
            {
                unlock_post(ctx, guild, &now, true).await;
            }
            Ok(())
        }
        Err(e) if bot_core::discord::is_not_found(&e) => Ok(()),
        Err(e) => {
            store::release_lock(ctx, guild, r.id)?;
            Err(bot_core::anyhow::anyhow!(bot_core::discord::why_refused(&e)))
        }
    }
}

/// Open a request's post again when it goes back to the queue, if the bot locked it (or, with
/// `always`, whatever the row says: a lock that landed after the request left "done").
pub(crate) async fn unlock_post(ctx: &Ctx, guild: u64, r: &store::Review, always: bool) {
    if r.locked_at.is_none() && !always {
        return;
    }
    if let Err(e) = store::release_lock(ctx, guild, r.id) {
        bot_core::tracing::warn!(guild, review = r.id, error = %e, "couldn't clear a review's lock");
    }
    let Some(thread) = r.thread_id.and_then(Id::new_checked).filter(|_| !ctx.cfg.preview) else { return };
    if let Err(e) = ctx.http.update_thread(thread).archived(false).locked(false).await {
        bot_core::tracing::warn!(guild, review = r.id, error = %e, "couldn't unlock a review post");
    }
}

const DONE_MARK: RequestReactionType<'static> = RequestReactionType::Unicode { name: "✅" };

/// A ✅ on a post whose review is done, taken off when it goes back to the queue. A forum post's
/// first message has the post's id; a thread made from a message has that message in the
/// channel. Best effort: a missing permission only loses the mark.
pub(crate) async fn mark_done(ctx: &Ctx, guild: u64, channel: Option<u64>, thread: u64, done: bool) {
    if ctx.cfg.preview {
        return;
    }
    let Some(message) = Id::new_checked(thread) else { return };
    for at in [Some(thread), channel].into_iter().flatten().filter_map(Id::new_checked) {
        let res = if done { ctx.http.create_reaction(at, message, &DONE_MARK).await.map(|_| ()) } else { ctx.http.delete_current_user_reaction(at, message, &DONE_MARK).await.map(|_| ()) };
        match res {
            Ok(()) => return,
            Err(e) if bot_core::discord::is_not_found(&e) => continue,
            Err(e) => {
                bot_core::tracing::warn!(guild, thread, error = %e, "couldn't change the done mark on a review post");
                return;
            }
        }
    }
}
