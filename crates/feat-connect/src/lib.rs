//! App approvals: Relay (desktop and phone), Tally, Avex and the owner's PCs ask to connect to
//! this server over `/api/connect`, every approver gets a DM with Allow and Deny, and the first
//! answer counts. A port of antho-s-bot's connect feature: same routes, same JSON, same limits.
//! What changed is who answers: a list of approvers set on the dashboard, instead of the Discord
//! server's owner.

pub mod api;
pub mod domain;
mod import;
pub mod store;
mod web;

use std::time::Duration;

use bot_core::anyhow::Result;
use bot_core::discord::{ComponentCall, Reply, button_row};
use bot_core::rusqlite::OptionalExtension;
use bot_core::twilight_model::channel::message::component::ButtonStyle;
use bot_core::twilight_model::id::Id;
use bot_core::{Ctx, Feature, FeatureMeta, Group, Icon, Migration, NavItem, Need, Overview, feature_async, now_ms, rusqlite, text};
use serde::{Deserialize, Serialize};

pub const KEY: &str = "connect";

/// Global, stored under server id 0: who approves app connections isn't a per-server matter.
pub const GLOBAL: u64 = 0;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Discord user ids. None until someone edits the list: then the bot's owner approves.
    pub approvers: Option<Vec<u64>>,
}

/// Who gets the DM and may answer it. Never empty: the bot's owner when nobody is set.
pub fn approvers(ctx: &Ctx) -> Vec<u64> {
    ctx.settings::<Settings>(GLOBAL, KEY)
        .ok()
        .and_then(|s| s.approvers.clone())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vec![ctx.cfg.owner_id])
}

pub fn save_approvers(ctx: &Ctx, list: Vec<u64>) -> Result<()> {
    ctx.save_settings(GLOBAL, KEY, &Settings { approvers: Some(list) })
}

#[derive(Default)]
pub struct Connect;

#[feature_async]
impl Feature for Connect {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "App approvals",
            description: "Lets Relay, Tally, Avex and your PCs connect after an approver allows them in Discord.",
            group: Group::Tools,
            nav: vec![NavItem { label: "App approvals", path: KEY, icon: Icon::Plug, about: "Relay, Tally, Avex and your PCs ask here before they connect." }],
        }
    }

    fn migrations(&self) -> &'static [Migration] {
        store::MIGRATIONS
    }

    /// App connections are shared by every server, so a per-server grant mustn't reach them:
    /// only dashboard admins see the page, answer requests, or edit approvers and PCs.
    fn admin_only(&self) -> bool {
        true
    }

    fn routes(&self) -> bot_core::axum::Router<Ctx> {
        api::routes().merge(web::routes())
    }

    fn overview(&self, ctx: &Ctx, page: &bot_core::web::Page) -> Overview {
        if !page.user.admin {
            return Overview::default();
        }
        let now = now_ms();
        let needs = store::pending(ctx, now)
            .unwrap_or_default()
            .into_iter()
            .take(3)
            .map(|r| Need {
                since: r.created_at,
                people: Vec::new(),
                icon: Icon::Plug,
                title: format!("{} on {} wants to connect", domain::kind_name(&r.kind), r.device),
                detail: format!("Waiting {}", text::duration((now - r.created_at) / 1000)),
                href: page.path(KEY),
                action: "Review",
                alert: false,
            })
            .collect();
        Overview { needs, ..Overview::default() }
    }

    async fn on_component(&self, ctx: &Ctx, call: &ComponentCall) -> Result<()> {
        let Some((allow, id)) = domain::parse_button(&call.args()) else { return Ok(()) };
        let user = call.user_id();
        if !approvers(ctx).contains(&user) {
            ctx.reply(&call.interaction, Reply::text("You're not an approver for app connections.").ephemeral()).await?;
            return Ok(());
        }
        // Ack inside Discord's three seconds; deciding edits every approver's DM with the outcome.
        ctx.defer_update(&call.interaction).await?;
        let name = call
            .interaction
            .author()
            .map(|u| u.global_name.clone().unwrap_or_else(|| u.name.clone()))
            .unwrap_or_else(|| name_of(ctx, user));
        if decide(ctx, &id, allow, user, Via::Discord(name)).await?.is_none() {
            ctx.followup(&call.interaction, Reply::text("This request was already answered or has expired.").ephemeral()).await?;
        }
        Ok(())
    }

    async fn tick(&self, ctx: &Ctx) -> Result<()> {
        let now = now_ms();
        // Requests that ran out unanswered: their DMs lose the buttons and say so.
        for req in store::expired_unsettled(ctx, now)? {
            store::mark_settled(ctx, &req.id)?;
            let text = domain::settled_text(&req.kind, &req.device, "expired");
            settle_dms(ctx, &req.id, &text).await;
        }
        store::prune(ctx, now)
    }

    fn seed_preview(&self, ctx: &Ctx, _guild: u64) -> Result<()> {
        seed(ctx)
    }
}

/// Where an answer came from, with the name of the person who gave it.
pub enum Via {
    Discord(String),
    Dashboard(String),
}

/// An answer from a DM button or the dashboard. None when the request isn't pending any more
/// (someone answered first, or it ran out).
pub async fn decide(ctx: &Ctx, id: &str, allow: bool, by: u64, via: Via) -> Result<Option<store::Request>> {
    let (on, who) = match &via {
        Via::Discord(n) => ("discord", n.clone()),
        Via::Dashboard(n) => ("dashboard", format!("{n} on the dashboard")),
    };
    let Some(req) = store::decide(ctx, id, allow, by, on, now_ms())? else { return Ok(None) };
    let verb = if allow { "Allowed" } else { "Denied" };
    let text = format!("{}\n{verb} by {who}.", domain::settled_text(&req.kind, &req.device, &req.status));
    settle_dms(ctx, &req.id, &text).await;
    bot_core::tracing::info!(kind = %req.kind, device = %req.device, by, "{verb} an app connection");
    Ok(Some(req))
}

/// Replace the buttons on every approver's DM with `text`. Best effort: a deleted DM or an
/// unreachable Discord leaves the dashboard as the record.
async fn settle_dms(ctx: &Ctx, id: &str, text: &str) {
    let Ok(dms) = store::dms(ctx, id) else { return };
    for (_, channel, message) in dms {
        let res = ctx.http.update_message(Id::new(channel), Id::new(message)).content(Some(text)).components(Some(&[])).await;
        if let Err(e) = res {
            bot_core::tracing::debug!(error = %e, "couldn't edit an approval DM");
        }
    }
}

/// DM every approver the request with Allow and Deny. True when at least one DM went out; when
/// none did, the dashboard bell says so instead.
pub async fn ask_approvers(ctx: &Ctx, req: &store::Request) -> bool {
    // Anyone can ask, so DMs are rationed: past the hourly allowance requests still wait on the
    // dashboard, and the bell rings once for the lot instead of DMs for each.
    match store::take_dm_round(ctx, &req.id, now_ms()) {
        Ok(store::Round::Dm) => {}
        Ok(store::Round::Quiet { ring }) => {
            bot_core::tracing::warn!(kind = %req.kind, device = %req.device, "too many requests to connect this hour: not DMing approvers");
            if ring {
                ctx.notify(
                    None,
                    KEY,
                    "Requests to connect are arriving faster than usual, so approvers aren't being DMed for now. Answer them here.",
                    store::some_enabled_guild(ctx).map(|g| format!("/g/{g}/{KEY}")),
                );
            }
            return false;
        }
        Err(e) => {
            bot_core::tracing::warn!(error = %e, "couldn't count approval DMs");
            return false;
        }
    }
    let mut sent = false;
    if !ctx.cfg.preview {
        let text = domain::ask_text(&req.kind, &req.device, &req.code);
        let row = button_row(&[
            (&domain::button_id(true, &req.id), "Allow", ButtonStyle::Success),
            (&domain::button_id(false, &req.id), "Deny", ButtonStyle::Danger),
        ]);
        let mut set = bot_core::tokio::task::JoinSet::new();
        for user in approvers(ctx) {
            let (ctx, text, row) = (ctx.clone(), text.clone(), row.clone());
            set.spawn(async move {
                let dm = bot_core::tokio::time::timeout(Duration::from_secs(10), ctx.dm(user, &text, &[], &[row])).await;
                (user, dm.ok().and_then(Result::ok).flatten())
            });
        }
        while let Some(done) = set.join_next().await {
            if let Ok((user, Some((channel, message)))) = done {
                sent = true;
                if let Err(e) = store::add_dm(ctx, &req.id, user, channel, message) {
                    bot_core::tracing::warn!(error = %e, "couldn't store an approval DM");
                }
            }
        }
    }
    if sent {
        let _ = store::set_notified(ctx, &req.id);
    } else {
        bot_core::tracing::warn!(kind = %req.kind, device = %req.device, "couldn't DM any approver about an app connection");
        ctx.notify(
            None,
            KEY,
            format!("{} asks to connect (code {}). No approver could be DMed, so answer it here.", capitalized(&req.label()), req.code),
            store::some_enabled_guild(ctx).map(|g| format!("/g/{g}/{KEY}")),
        );
    }
    sent
}

/// A user's name without a server: their Discord name from the cache, their dashboard name, or
/// their id.
pub fn name_of(ctx: &Ctx, user: u64) -> String {
    if let Some(u) = ctx.cache.user(Id::new(user)) {
        return u.global_name.clone().unwrap_or_else(|| u.name.clone());
    }
    ctx.db
        .with(|c| c.query_row("SELECT name FROM dash_users WHERE id = ?1 AND name != ''", [user as i64], |r| r.get::<_, String>(0)).optional())
        .ok()
        .flatten()
        .unwrap_or_else(|| format!("User {user}"))
}

pub fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// Preview rows: two requests waiting (one no approver could be DMed about), a few answered, a PC, and an approver besides the owner.
/// The PC's token is `sha256("preview-pc")` and the allowed `pc` request's verifier is
/// `sha256("preview-verifier")` (its id: the first 32 of `sha256("preview-request-1")`), so the
/// endpoints can be tried with curl.
fn seed(ctx: &Ctx) -> Result<()> {
    use bot_core::preview::MEMBERS;
    let now = now_ms();
    let min = 60_000;
    let verifier = domain::sha256_hex("preview-verifier");
    // (kind, device, challenge, status, answered by and where, minutes ago)
    type Sample<'a> = (&'a str, &'a str, &'a str, &'a str, Option<(u64, &'a str)>, i64);
    let rows: [Sample; 7] = [
        ("relay", "Pixel 8", &domain::sha256_hex("preview-1"), "pending", None, 2),
        ("pc", "antho-desktop", &domain::sha256_hex(&verifier), "allowed", Some((ctx.cfg.owner_id, "dashboard")), 4),
        ("tally", "Galaxy S23", &domain::sha256_hex("preview-3"), "claimed", Some((ctx.cfg.owner_id, "discord")), 95),
        ("avex", "Pixel 8", &domain::sha256_hex("preview-4"), "claimed", Some((MEMBERS[0].0, "discord")), 60 * 26),
        ("tally", "old tablet", &domain::sha256_hex("preview-5"), "denied", Some((ctx.cfg.owner_id, "discord")), 60 * 50),
        ("relay", "Work phone", &domain::sha256_hex("preview-6"), "pending", None, 60 * 74),
        ("avex", "Galaxy Tab", &domain::sha256_hex("preview-7"), "pending", None, 6),
    ];
    let conn = ctx.db.lock();
    for (i, (kind, device, challenge, status, decided, ago_min)) in rows.into_iter().enumerate() {
        let created = now - ago_min * min;
        conn.execute(
            "INSERT INTO connect_requests (id, kind, device_name, challenge, code, status, notified, decided_by, decided_on,
               claims, created_at, expires_at, decided_at, dm_settled)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 1)",
            rusqlite::params![
                &domain::sha256_hex(&format!("preview-request-{i}"))[..32],
                kind,
                device,
                challenge,
                domain::display_code(challenge),
                status,
                status != "pending" || ago_min < 5,
                decided.map(|d| d.0 as i64),
                decided.map(|d| d.1),
                (status == "claimed") as i64,
                created,
                created + domain::TTL_MS,
                decided.map(|_| created + min),
            ],
        )?;
    }
    conn.execute(
        "INSERT INTO connect_hosts (id, request_id, name, token_hash, created_at, last_seen_at) VALUES ('a1b2c3d4e5f60718', 'seed', 'antho-desktop', ?1, ?2, ?3)",
        rusqlite::params![domain::sha256_hex(&domain::sha256_hex("preview-pc")), now - 9 * 86_400_000, now - 20_000],
    )?;
    drop(conn);
    save_approvers(ctx, vec![ctx.cfg.owner_id, MEMBERS[0].0])
}

#[cfg(test)]
pub(crate) mod testing {
    use bot_core::{Config, Ctx, Db};

    /// A context with an in-memory database, this feature's tables and the feature on in one
    /// server. No Discord: preview mode skips the DMs.
    pub fn ctx() -> Ctx {
        let mut cfg = Config::preview(0);
        // Never run a real `relay` that happens to be on the PATH.
        cfg.relay_bin = "/nonexistent/relay".into();
        let ctx = Ctx::new(cfg, Db::memory().unwrap()).unwrap();
        ctx.install_features(vec![std::sync::Arc::new(super::Connect)]).unwrap();
        ctx.set_enabled(42, super::KEY, true).unwrap();
        ctx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn the_owner_approves_until_the_list_is_set() {
        let ctx = testing::ctx();
        assert_eq!(approvers(&ctx), vec![ctx.cfg.owner_id]);
        save_approvers(&ctx, vec![7, 8]).unwrap();
        assert_eq!(approvers(&ctx), vec![7, 8]);
        save_approvers(&ctx, vec![]).unwrap();
        assert_eq!(approvers(&ctx), vec![ctx.cfg.owner_id]);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn the_first_answer_counts() {
        let ctx = testing::ctx();
        let challenge = domain::sha256_hex("v");
        let req = store::create(&ctx, &store::NewRequest { kind: "tally", device: "phone", challenge: &challenge, ip: None }, now_ms())
            .unwrap()
            .unwrap();
        let first = decide(&ctx, &req.id, true, 7, Via::Discord("Mira".into())).await.unwrap();
        assert_eq!(first.unwrap().status, "allowed");
        assert!(decide(&ctx, &req.id, false, 8, Via::Dashboard("Oskar".into())).await.unwrap().is_none());
        let row = store::get(&ctx, &req.id).unwrap().unwrap();
        assert_eq!((row.status.as_str(), row.decided_by, row.decided_on.as_deref()), ("allowed", Some(7), Some("discord")));
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn one_open_request_per_address() {
        let ctx = testing::ctx();
        let now = now_ms();
        let ask = |n: &str, ip: &str, at: i64| {
            let challenge = domain::sha256_hex(n);
            store::create(&ctx, &store::NewRequest { kind: "relay", device: "x", challenge: &challenge, ip: Some(ip) }, at).unwrap()
        };
        assert!(ask("a", "203.0.113.1", now).is_ok());
        // Well past the 5 s cooldown, but the first is still waiting.
        assert_eq!(ask("b", "203.0.113.1", now + 60_000).unwrap_err(), store::Refusal::SlowDown);
        assert!(ask("c", "203.0.113.2", now + 60_000).is_ok());
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn dm_rounds_are_rationed_and_the_bell_rings_once() {
        let ctx = testing::ctx();
        let now = now_ms();
        let bells = || -> i64 { ctx.db.with(|c| c.query_row("SELECT COUNT(*) FROM notifications WHERE feature = 'connect'", [], |r| r.get(0))).unwrap() };
        for i in 0..domain::MAX_DM_ROUNDS_PER_HOUR + 3 {
            let challenge = domain::sha256_hex(&i.to_string());
            // Answer each so the pending cap doesn't get in the way.
            let req = store::create(&ctx, &store::NewRequest { kind: "tally", device: "x", challenge: &challenge, ip: None }, now).unwrap().unwrap();
            let round = store::take_dm_round(&ctx, &req.id, now).unwrap();
            let want = match i - domain::MAX_DM_ROUNDS_PER_HOUR {
                ..0 => store::Round::Dm,
                0 => store::Round::Quiet { ring: true },
                _ => store::Round::Quiet { ring: false },
            };
            assert_eq!(round, want, "request {i}");
            store::decide(&ctx, &req.id, false, 1, "dashboard", now).unwrap();
        }
        // Through ask_approvers: held back, notified false, and no second bell this hour.
        let challenge = domain::sha256_hex("late");
        let req = store::create(&ctx, &store::NewRequest { kind: "tally", device: "x", challenge: &challenge, ip: None }, now_ms()).unwrap().unwrap();
        let before = bells();
        assert!(!ask_approvers(&ctx, &req).await);
        assert_eq!(bells(), before);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn nobody_dmed_rings_the_bell() {
        let ctx = testing::ctx();
        let challenge = domain::sha256_hex("v");
        let req = store::create(&ctx, &store::NewRequest { kind: "avex", device: "Pixel", challenge: &challenge, ip: None }, now_ms())
            .unwrap()
            .unwrap();
        assert!(!ask_approvers(&ctx, &req).await);
        let (n, link): (i64, Option<String>) = ctx
            .db
            .with(|c| c.query_row("SELECT COUNT(*), MAX(link) FROM notifications WHERE feature = 'connect'", [], |r| Ok((r.get(0)?, r.get(1)?))))
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(link.as_deref(), Some("/g/42/connect"));
    }
}
