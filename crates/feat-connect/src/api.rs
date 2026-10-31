//! The machine endpoints the apps call, kept exactly as antho-s-bot answered them
//! (`src/app/api/connect/**`, `src/app/api/tally/**`): paths, JSON field names, status codes and
//! error words. Clients: Relay's `crates/relay-remote/src/connect.rs`, the Relay phone app's
//! `ServerConnect.kt`, Tally and Avex.

use std::time::Duration;

use bot_core::axum::Router;
use bot_core::axum::body::Bytes;
use bot_core::axum::extract::State;
use bot_core::axum::http::{HeaderMap, StatusCode};
use bot_core::axum::response::{IntoResponse, Json, Response};
use bot_core::axum::routing::{get, post};
use bot_core::serde_json::{Value, json};
use bot_core::{Ctx, now_ms};

use crate::domain::{self, ClaimState};
use crate::store::{self, NewRequest, Refusal};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/api/connect", get(hello))
        .route("/api/connect/enroll", post(enroll))
        .route("/api/connect/claim", post(claim))
        .route("/api/connect/host/phones", post(phones))
        .route("/api/connect/host/answer", post(answer))
        // Tally's first addresses, kept in the shape it knows.
        .route("/api/tally", get(tally_hello))
        .route("/api/tally/enroll", post(enroll))
        .route("/api/tally/claim", post(claim))
}

/// A JSON answer with a status.
pub type Answer = (StatusCode, Value);

fn respond(a: Answer) -> Response {
    (a.0, Json(a.1)).into_response()
}

fn bad_request() -> Answer {
    (StatusCode::BAD_REQUEST, json!({ "error": "bad_request" }))
}

/// The body as a JSON object, whatever the content type says (as Next's `req.json()` did).
fn body(bytes: &[u8]) -> Option<bot_core::serde_json::Map<String, Value>> {
    match bot_core::serde_json::from_slice(bytes) {
        Ok(Value::Object(o)) => Some(o),
        _ => None,
    }
}

fn hex_field<'a>(o: &'a bot_core::serde_json::Map<String, Value>, key: &str, len: usize) -> Option<&'a str> {
    o.get(key).and_then(Value::as_str).filter(|s| domain::is_hex(s, len))
}

/// Whether this server carries Relay's cloud copy (RELAY_PUBLIC_URL is set).
pub fn cloud(ctx: &Ctx) -> bool {
    ctx.cfg.relay_public_url.as_deref().is_some_and(|u| !u.trim().trim_end_matches('/').is_empty())
}

// ---- GET /api/connect, GET /api/tally ----

async fn hello(State(ctx): State<Ctx>) -> Response {
    respond(hello_answer(&ctx))
}

/// What a device reads first: whether this server takes requests, carries the cloud copy, and
/// has a PC checking in for Relay phones. Public: it says nothing about the data.
pub fn hello_answer(ctx: &Ctx) -> Answer {
    let on = store::enabled_anywhere(ctx);
    let pc = on && store::count_hosts(ctx).unwrap_or(0) > 0;
    (
        StatusCode::OK,
        json!({ "service": "relay-connect", "v": 2, "kinds": domain::KINDS, "cloud": on && cloud(ctx), "pc": pc, "approval": "discord" }),
    )
}

async fn tally_hello(State(ctx): State<Ctx>) -> Response {
    respond(tally_hello_answer(&ctx))
}

pub fn tally_hello_answer(ctx: &Ctx) -> Answer {
    let on = store::enabled_anywhere(ctx);
    (StatusCode::OK, json!({ "service": "tally-cloud", "v": 1, "cloud": on && cloud(ctx), "approval": "discord" }))
}

// ---- POST /api/connect/enroll (and /api/tally/enroll) ----

async fn enroll(State(ctx): State<Ctx>, headers: HeaderMap, bytes: Bytes) -> Response {
    // Several X-Forwarded-For headers read as one list, in order.
    let forwarded: Vec<&str> = headers.get_all("x-forwarded-for").iter().filter_map(|v| v.to_str().ok()).collect();
    let ip = domain::client_ip((!forwarded.is_empty()).then(|| forwarded.join(",")).as_deref());
    respond(enroll_answer(&ctx, &bytes, ip.as_deref()).await)
}

/// A device asks to connect. Public by design: the device has no account here. What lets it in
/// is an approver's tap, and only the device holding the verifier can collect the link.
pub async fn enroll_answer(ctx: &Ctx, bytes: &[u8], ip: Option<&str>) -> Answer {
    let Some(o) = body(bytes) else { return bad_request() };
    // `kind` defaults to Tally, the first app that asked, which still leaves it out.
    let kind = match o.get("kind") {
        None => "tally",
        Some(Value::String(k)) if domain::is_kind(k) => k.as_str(),
        Some(_) => return bad_request(),
    };
    let Some(device) = o.get("device").and_then(Value::as_str).filter(|d| (1..=100).contains(&domain::js_len(d))) else {
        return bad_request();
    };
    let Some(challenge) = hex_field(&o, "challenge", 64) else { return bad_request() };

    let refuse = |error: &str, status: u16| (StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST), json!({ "error": error }));
    if !store::enabled_anywhere(ctx) {
        return refuse("off", 404);
    }
    if domain::pairs_with_cloud(kind) && !cloud(ctx) {
        return refuse("cloud_off", 503);
    }
    // A Relay phone needs a PC that checks in here to open its window.
    if kind == "relay" && store::count_hosts(ctx).unwrap_or(0) == 0 {
        return refuse("no_pc", 409);
    }
    let req = match store::create(ctx, &NewRequest { kind, device, challenge, ip }, now_ms()) {
        Ok(Ok(req)) => req,
        Ok(Err(Refusal::Busy)) => return refuse("busy", 429),
        Ok(Err(Refusal::SlowDown)) => return refuse("slow_down", 429),
        Err(e) => {
            bot_core::tracing::error!(error = %e, "couldn't store a request to connect");
            return (StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": "internal" }));
        }
    };
    let notified = crate::ask_approvers(ctx, &req).await;
    (
        StatusCode::CREATED,
        json!({ "id": req.id, "code": req.code, "expiresAt": req.expires_at, "notified": notified }),
    )
}

// ---- POST /api/connect/claim (and /api/tally/claim) ----

async fn claim(State(ctx): State<Ctx>, bytes: Bytes) -> Response {
    respond(claim_answer(&ctx, &bytes).await)
}

/// A device polling its request, proving it is the one that made it. Once allowed, the answer
/// carries a pairing link: a fresh one from the cloud engine, or the one the owner's PC opened
/// for a Relay phone. A PC also gets its token for this server. A request unknown or not this
/// device's reads as expired.
pub async fn claim_answer(ctx: &Ctx, bytes: &[u8]) -> Answer {
    let Some(o) = body(bytes) else { return bad_request() };
    let (Some(id), Some(verifier)) = (hex_field(&o, "id", 32), hex_field(&o, "verifier", 64)) else { return bad_request() };
    let status = |s: ClaimState| (StatusCode::OK, json!({ "status": s.as_str() }));

    let row = match store::get(ctx, id) {
        Ok(r) => r,
        Err(e) => {
            bot_core::tracing::error!(error = %e, "couldn't read a request to connect");
            return (StatusCode::SERVICE_UNAVAILABLE, json!({ "status": "error", "reason": "The server could not answer. Try again in a minute." }));
        }
    };
    let Some(row) = row.filter(|r| domain::verifier_matches(verifier, &r.challenge)) else { return status(ClaimState::Expired) };
    let now = now_ms();
    let state = domain::claim_state(&row.kind, &row.status, row.expires_at, row.claims, row.pc_link.is_some(), now);
    if state != ClaimState::Ready {
        return status(state);
    }
    let busy = || (StatusCode::SERVICE_UNAVAILABLE, json!({ "status": "error", "reason": "The server could not answer. Try again in a minute." }));
    // Count the claim first: another claim at the same moment may have taken the last one.
    match store::reserve_claim(ctx, &row.id, now) {
        Ok(true) => {}
        Ok(false) => return status(ClaimState::Spent),
        Err(e) => {
            bot_core::tracing::error!(error = %e, "couldn't count a claim");
            return busy();
        }
    }

    if row.kind == "relay" {
        return (StatusCode::OK, json!({ "status": "ready", "link": row.pc_link }));
    }

    let Some(link) = mint_pair_link(&ctx.cfg.relay_bin, Duration::from_secs(15)).await else {
        bot_core::tracing::error!(bin = %ctx.cfg.relay_bin, "relay remote pair gave no link: is the cloud engine running?");
        let _ = store::release_claim(ctx, &row.id);
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({ "status": "error", "reason": "The server could not open a pairing window. Try again in a minute." }),
        );
    };
    let mut answer = json!({ "status": "ready", "link": link });
    if row.kind == "pc" {
        let (token, hash) = domain::new_host_token();
        match store::upsert_host(ctx, &row.id, &row.device, &hash, now) {
            Ok(true) => answer["hostToken"] = json!(token),
            // Forgotten on the dashboard since it was allowed.
            Ok(false) => return status(ClaimState::Spent),
            Err(e) => {
                bot_core::tracing::error!(error = %e, "couldn't store a PC's token");
                let _ = store::release_claim(ctx, &row.id);
                return busy();
            }
        }
    }
    (StatusCode::OK, answer)
}

/// Opens a ten-minute, single-use pairing window on the cloud engine and returns its link: what
/// `relay remote pair --via-only` does in a shell. `--no-confirm` because the approver already
/// answered on Discord. None when the binary is missing, fails, times out or prints no link.
pub async fn mint_pair_link(bin: &str, timeout: Duration) -> Option<String> {
    use std::process::Stdio;
    let mut cmd = bot_core::tokio::process::Command::new(bin);
    cmd.args(["--instance", "stable", "remote", "pair", "--via-only", "--no-confirm"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match bot_core::tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(out)) if out.status.success() => domain::parse_pair_link(&String::from_utf8_lossy(&out.stdout)),
        Ok(Ok(out)) => {
            bot_core::tracing::warn!(status = %out.status, "relay remote pair failed");
            None
        }
        Ok(Err(e)) => {
            bot_core::tracing::warn!(error = %e, bin, "couldn't run relay");
            None
        }
        Err(_) => {
            bot_core::tracing::warn!("relay remote pair timed out");
            None
        }
    }
}

// ---- POST /api/connect/host/phones, POST /api/connect/host/answer ----

async fn phones(State(ctx): State<Ctx>, bytes: Bytes) -> Response {
    respond(phones_answer(&ctx, &bytes))
}

/// The owner's PC asking which Relay phones an approver allowed and are waiting for it.
pub fn phones_answer(ctx: &Ctx, bytes: &[u8]) -> Answer {
    let Some(o) = body(bytes) else { return bad_request() };
    let Some(token) = hex_field(&o, "token", 64) else { return bad_request() };
    let now = now_ms();
    let host = domain::host_token_hash(token).and_then(|h| store::host_by_token(ctx, &h, now).ok().flatten());
    if host.is_none() {
        return (StatusCode::UNAUTHORIZED, json!({ "error": "unknown_pc" }));
    }
    let phones: Vec<Value> = store::open_relay_requests(ctx, now)
        .unwrap_or_default()
        .into_iter()
        .map(|r| json!({ "id": r.id, "device": r.label(), "code": r.code, "expiresAt": r.expires_at }))
        .collect();
    (StatusCode::OK, json!({ "phones": phones }))
}

async fn answer(State(ctx): State<Ctx>, bytes: Bytes) -> Response {
    respond(answer_answer(&ctx, &bytes))
}

/// The owner's PC handing a waiting Relay phone the pairing window it opened for it.
pub fn answer_answer(ctx: &Ctx, bytes: &[u8]) -> Answer {
    let Some(o) = body(bytes) else { return bad_request() };
    let (Some(token), Some(id)) = (hex_field(&o, "token", 64), hex_field(&o, "id", 32)) else { return bad_request() };
    let Some(link) = o.get("link").and_then(Value::as_str).filter(|l| (1..=2048).contains(&domain::js_len(l))) else {
        return bad_request();
    };
    let now = now_ms();
    let err = |status: StatusCode, e: &str| (status, json!({ "error": e }));
    let Some(host) = domain::host_token_hash(token).and_then(|h| store::host_by_token(ctx, &h, now).ok().flatten()) else {
        return err(StatusCode::UNAUTHORIZED, "unknown_pc");
    };
    if !domain::is_pair_link(link) {
        return err(StatusCode::BAD_REQUEST, "bad_link");
    }
    if !store::set_pc_link(ctx, id, &host.id, link, now).unwrap_or(false) {
        return err(StatusCode::CONFLICT, "gone");
    }
    bot_core::tracing::info!(pc = %host.name, request = id, "a PC opened a pairing window for a phone");
    (StatusCode::OK, json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    fn verifier(n: u8) -> String {
        domain::sha256_hex(&format!("verifier-{n}"))
    }

    fn enroll_body(kind: &str, n: u8) -> Vec<u8> {
        json!({ "kind": kind, "device": "Pixel 8", "challenge": domain::sha256_hex(&verifier(n)) }).to_string().into_bytes()
    }

    fn claim_body(id: &str, n: u8) -> Vec<u8> {
        json!({ "id": id, "verifier": verifier(n) }).to_string().into_bytes()
    }

    fn add_host(ctx: &Ctx) -> String {
        let (token, hash) = domain::new_host_token();
        store::upsert_host(ctx, "r", "desk", &hash, now_ms()).unwrap();
        token
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn hello_has_the_shape_the_apps_read() {
        let ctx = testing::ctx();
        let (s, v) = hello_answer(&ctx);
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v, json!({ "service": "relay-connect", "v": 2, "kinds": ["tally", "avex", "pc", "relay"], "cloud": false, "pc": false, "approval": "discord" }));
        add_host(&ctx);
        assert_eq!(hello_answer(&ctx).1["pc"], true);
        assert_eq!(tally_hello_answer(&ctx).1, json!({ "service": "tally-cloud", "v": 1, "cloud": false, "approval": "discord" }));
        ctx.set_enabled(42, crate::KEY, false).unwrap();
        assert_eq!(hello_answer(&ctx).1["pc"], false);
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn enroll_refusals_keep_their_words_and_codes() {
        let ctx = testing::ctx();
        let bad = |s: &str| s.as_bytes().to_vec();
        for body in [bad("nope"), bad("[]"), bad(r#"{"device":"x"}"#), bad(r#"{"kind":"fridge","device":"x","challenge":"00"}"#)] {
            assert_eq!(enroll_answer(&ctx, &body, None).await, bad_request());
        }
        let long = json!({ "device": "x".repeat(101), "challenge": "a".repeat(64) }).to_string();
        assert_eq!(enroll_answer(&ctx, long.as_bytes(), None).await, bad_request());
        // No RELAY_PUBLIC_URL here: the cloud kinds, Tally by default, are refused.
        let no_kind = json!({ "device": "x", "challenge": "a".repeat(64) }).to_string();
        assert_eq!(enroll_answer(&ctx, no_kind.as_bytes(), None).await, (StatusCode::SERVICE_UNAVAILABLE, json!({ "error": "cloud_off" })));
        assert_eq!(enroll_answer(&ctx, &enroll_body("relay", 1), None).await, (StatusCode::CONFLICT, json!({ "error": "no_pc" })));
        ctx.set_enabled(42, crate::KEY, false).unwrap();
        assert_eq!(enroll_answer(&ctx, &enroll_body("relay", 1), None).await, (StatusCode::NOT_FOUND, json!({ "error": "off" })));
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn enroll_limits_per_address_and_in_total() {
        let ctx = testing::ctx();
        add_host(&ctx);
        let (s, v) = enroll_answer(&ctx, &enroll_body("relay", 1), Some("1.2.3.4")).await;
        assert_eq!(s, StatusCode::CREATED);
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["code", "expiresAt", "id", "notified"]);
        assert_eq!(v["code"], domain::display_code(&domain::sha256_hex(&verifier(1))));
        assert_eq!(v["notified"], false);
        assert!(domain::is_hex(v["id"].as_str().unwrap(), 32));
        assert_eq!(enroll_answer(&ctx, &enroll_body("relay", 2), Some("1.2.3.4")).await, (StatusCode::TOO_MANY_REQUESTS, json!({ "error": "slow_down" })));
        assert_eq!(enroll_answer(&ctx, &enroll_body("relay", 2), Some("1.2.3.5")).await.0, StatusCode::CREATED);
        assert_eq!(enroll_answer(&ctx, &enroll_body("relay", 3), None).await.0, StatusCode::CREATED);
        assert_eq!(enroll_answer(&ctx, &enroll_body("relay", 4), None).await, (StatusCode::TOO_MANY_REQUESTS, json!({ "error": "busy" })));
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn a_relay_phone_goes_through_the_pc() {
        let ctx = testing::ctx();
        let token = add_host(&ctx);
        let (_, v) = enroll_answer(&ctx, &enroll_body("relay", 1), None).await;
        let id = v["id"].as_str().unwrap().to_owned();

        assert_eq!(claim_answer(&ctx, &claim_body(&id, 1)).await, (StatusCode::OK, json!({ "status": "waiting" })));
        // Someone else's verifier, or an unknown id, reads as expired.
        assert_eq!(claim_answer(&ctx, &claim_body(&id, 2)).await, (StatusCode::OK, json!({ "status": "expired" })));
        assert_eq!(claim_answer(&ctx, &claim_body(&"0".repeat(32), 1)).await, (StatusCode::OK, json!({ "status": "expired" })));
        assert_eq!(claim_answer(&ctx, b"{}").await, bad_request());

        let phones = json!({ "token": token }).to_string();
        assert_eq!(phones_answer(&ctx, phones.as_bytes()), (StatusCode::OK, json!({ "phones": [] })));
        crate::decide(&ctx, &id, true, 1, crate::Via::Dashboard("Antho".into())).await.unwrap().unwrap();
        assert_eq!(claim_answer(&ctx, &claim_body(&id, 1)).await, (StatusCode::OK, json!({ "status": "waiting_pc" })));

        let (s, v) = phones_answer(&ctx, phones.as_bytes());
        assert_eq!(s, StatusCode::OK);
        let phone = &v["phones"][0];
        assert_eq!(phone["id"], id.as_str());
        assert_eq!(phone["device"], "the Relay app on Pixel 8");
        assert_eq!(phone["code"].as_str().unwrap().len(), 4);
        assert!(phone["expiresAt"].as_i64().unwrap() > now_ms());

        let unknown = json!({ "token": "f".repeat(64) }).to_string();
        assert_eq!(phones_answer(&ctx, unknown.as_bytes()), (StatusCode::UNAUTHORIZED, json!({ "error": "unknown_pc" })));

        let answer = |link: &str| json!({ "token": token, "id": id, "link": link }).to_string();
        assert_eq!(answer_answer(&ctx, answer("https://x").as_bytes()), (StatusCode::BAD_REQUEST, json!({ "error": "bad_link" })));
        assert_eq!(answer_answer(&ctx, answer("relay://pair?v=1&code=AB").as_bytes()), (StatusCode::OK, json!({ "ok": true })));
        assert_eq!(answer_answer(&ctx, answer("relay://pair?v=1&code=CD").as_bytes()), (StatusCode::CONFLICT, json!({ "error": "gone" })));

        let ready = (StatusCode::OK, json!({ "status": "ready", "link": "relay://pair?v=1&code=AB" }));
        for _ in 0..crate::domain::MAX_CLAIMS {
            assert_eq!(claim_answer(&ctx, &claim_body(&id, 1)).await, ready);
        }
        assert_eq!(claim_answer(&ctx, &claim_body(&id, 1)).await, (StatusCode::OK, json!({ "status": "spent" })));
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn a_denied_request_says_so() {
        let ctx = testing::ctx();
        add_host(&ctx);
        let (_, v) = enroll_answer(&ctx, &enroll_body("relay", 1), None).await;
        let id = v["id"].as_str().unwrap().to_owned();
        crate::decide(&ctx, &id, false, 1, crate::Via::Discord("Antho".into())).await.unwrap().unwrap();
        assert_eq!(claim_answer(&ctx, &claim_body(&id, 1)).await, (StatusCode::OK, json!({ "status": "denied" })));
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn a_cloud_claim_without_the_relay_binary_is_a_503_the_apps_read() {
        let ctx = testing::ctx();
        let challenge = domain::sha256_hex(&verifier(1));
        let req = store::create(&ctx, &NewRequest { kind: "pc", device: "desk", challenge: &challenge, ip: None }, now_ms()).unwrap().unwrap();
        crate::decide(&ctx, &req.id, true, 1, crate::Via::Discord("Antho".into())).await.unwrap().unwrap();
        // The test context points relay_bin at a path that doesn't exist.
        let (s, v) = claim_answer(&ctx, &claim_body(&req.id, 1)).await;
        assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(v, json!({ "status": "error", "reason": "The server could not open a pairing window. Try again in a minute." }));
        // Nothing was handed out, so the device can try again.
        assert_eq!(store::get(&ctx, &req.id).unwrap().unwrap().claims, 0);
        assert_eq!(store::count_hosts(&ctx).unwrap(), 0);
    }

    /// A stand-in for `relay` that prints a pairing link, in a directory of its own.
    fn fake_relay() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("connect-test-{}", domain::random_hex(4)));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("relay");
        std::fs::write(&fake, "#!/bin/sh\necho 'relay://pair?v=1&room=r&code=ZZ99'\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        fake
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn a_forgotten_pc_cant_claim_its_way_back() {
        let fake = fake_relay();
        let mut cfg = bot_core::Config::preview(0);
        cfg.relay_bin = fake.to_str().unwrap().into();
        let ctx = Ctx::new(cfg, bot_core::Db::memory().unwrap()).unwrap();
        ctx.install_features(vec![std::sync::Arc::new(crate::Connect)]).unwrap();
        let challenge = domain::sha256_hex(&verifier(1));
        let req = store::create(&ctx, &NewRequest { kind: "pc", device: "desk", challenge: &challenge, ip: None }, now_ms()).unwrap().unwrap();
        crate::decide(&ctx, &req.id, true, 1, crate::Via::Discord("Antho".into())).await.unwrap().unwrap();

        let (s, v) = claim_answer(&ctx, &claim_body(&req.id, 1)).await;
        assert_eq!((s, v["status"].as_str()), (StatusCode::OK, Some("ready")));
        assert!(v["hostToken"].is_string());
        let host = store::hosts(&ctx).unwrap().remove(0);
        // A retry inside the window still works and replaces the token.
        assert!(claim_answer(&ctx, &claim_body(&req.id, 1)).await.1["hostToken"].is_string());

        store::revoke_host(&ctx, &host.id, now_ms()).unwrap().unwrap();
        assert_eq!(claim_answer(&ctx, &claim_body(&req.id, 1)).await, (StatusCode::OK, json!({ "status": "spent" })));
        assert_eq!(store::count_hosts(&ctx).unwrap(), 0);
        // Even with the request's count put back, the host isn't revived.
        let (_, hash) = domain::new_host_token();
        assert!(!store::upsert_host(&ctx, &req.id, "desk", &hash, now_ms()).unwrap());
        assert_eq!(store::count_hosts(&ctx).unwrap(), 0);
        let _ = std::fs::remove_dir_all(fake.parent().unwrap());
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn claims_are_counted_before_a_link_goes_out() {
        let ctx = testing::ctx();
        let challenge = domain::sha256_hex(&verifier(1));
        let req = store::create(&ctx, &NewRequest { kind: "tally", device: "x", challenge: &challenge, ip: None }, now_ms()).unwrap().unwrap();
        let now = now_ms();
        // Not allowed yet: nothing to claim.
        assert!(!store::reserve_claim(&ctx, &req.id, now).unwrap());
        crate::decide(&ctx, &req.id, true, 1, crate::Via::Discord("Antho".into())).await.unwrap().unwrap();
        ctx.db.with(|c| c.execute("UPDATE connect_requests SET claims = ?2 WHERE id = ?1", bot_core::rusqlite::params![req.id, domain::MAX_CLAIMS - 1])).unwrap();
        // Two claims at once for the last link: one gets it.
        assert!(store::reserve_claim(&ctx, &req.id, now).unwrap());
        assert!(!store::reserve_claim(&ctx, &req.id, now).unwrap());
        // A claim given back from the first link puts the request back to allowed.
        ctx.db.with(|c| c.execute("UPDATE connect_requests SET claims = 1 WHERE id = ?1", [&req.id])).unwrap();
        store::release_claim(&ctx, &req.id).unwrap();
        let row = store::get(&ctx, &req.id).unwrap().unwrap();
        assert_eq!((row.status.as_str(), row.claims), ("allowed", 0));
        // Past the window, no claim.
        assert!(!store::reserve_claim(&ctx, &req.id, row.expires_at).unwrap());
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn the_pair_link_comes_from_the_relay_output() {
        let dir = std::env::temp_dir().join(format!("connect-test-{}", domain::random_hex(4)));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("relay");
        std::fs::write(&fake, "#!/bin/sh\necho \"args: $*\" >&2\necho 'Open relay://pair?v=1&room=r&code=ZZ99 on the device.'\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let link = mint_pair_link(fake.to_str().unwrap(), Duration::from_secs(5)).await;
        assert_eq!(link.as_deref(), Some("relay://pair?v=1&room=r&code=ZZ99"));
        let slow = dir.join("slow");
        std::fs::write(&slow, "#!/bin/sh\nsleep 5\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&slow, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(mint_pair_link(slow.to_str().unwrap(), Duration::from_millis(200)).await, None);
        assert_eq!(mint_pair_link("/nonexistent/relay", Duration::from_secs(1)).await, None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
