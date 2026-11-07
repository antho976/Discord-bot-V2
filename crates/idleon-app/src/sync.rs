//! What the app keeps on this computer, and what it syncs with the bot's server.
//!
//! Kept, in the system's config folder (`~/.config/idleon-check` on Linux, `%APPDATA%\idleon-check`
//! on Windows): the settings (packs, server, the Discord sign-in for reports), the last save
//! checked, and the server's tiers as last fetched. Nothing about a save ever leaves the computer:
//! the server is asked for its tiers and the servers it has the check on, and it gets the reports
//! you send.
//!
//! Every function here blocks; the app calls them off its window's thread.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The bot's address. `IDLEON_CHECK_SERVER` points the app at another one (a local preview).
pub fn server() -> String {
    std::env::var("IDLEON_CHECK_SERVER")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "https://anthobot.bid".into())
        .trim_end_matches('/')
        .to_string()
}

/// `IDLEON_CHECK_OFFLINE=1`: no network at all (the app works from what it carries and keeps).
pub fn offline() -> bool {
    std::env::var("IDLEON_CHECK_OFFLINE").is_ok_and(|v| v == "1")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The packs choice's code ("buy", "no-more", "free").
    pub packs: String,
    /// The server (Discord guild) reports go to, as the info listed it.
    pub guild: Option<String>,
    /// The Discord sign-in reports are sent with.
    pub session: Option<Session>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub token: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tiers {
    pub version: String,
    pub crowd: String,
    pub expected: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Guild {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    #[serde(default)]
    pub closed: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Info {
    pub guilds: Vec<Guild>,
    pub tiers: String,
    #[allow(dead_code)]
    pub game: String,
}

/// The app's folder, made if it isn't there.
fn dir() -> Option<PathBuf> {
    let d = dirs::config_dir()?.join("idleon-check");
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

/// Writes through a temporary file, so a crash mid-write never leaves half a file.
fn write(name: &str, bytes: &[u8]) {
    let Some(d) = dir() else { return };
    let tmp = d.join(format!("{name}.tmp"));
    if std::fs::write(&tmp, bytes).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        let _ = std::fs::rename(&tmp, d.join(name));
    }
}

fn read(name: &str) -> Option<String> {
    std::fs::read_to_string(dir()?.join(name)).ok()
}

pub fn settings() -> Settings {
    read("settings.json")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_settings(s: &Settings) {
    if let Ok(j) = serde_json::to_vec_pretty(s) {
        write("settings.json", &j);
    }
}

/// The save last checked, to check again on the next start.
pub fn last_save() -> Option<String> {
    read("last-save.json").filter(|s| !s.trim().is_empty())
}

pub fn keep_save(text: &str) {
    write("last-save.json", text.as_bytes());
}

pub fn forget_save() {
    if let Some(d) = dir() {
        let _ = std::fs::remove_file(d.join("last-save.json"));
    }
}

/// The server's tiers as last fetched.
pub fn kept_tiers() -> Option<Tiers> {
    read("tiers.json").and_then(|s| serde_json::from_str(&s).ok())
}

pub fn keep_tiers(t: &Tiers) {
    if let Ok(j) = serde_json::to_vec(t) {
        write("tiers.json", &j);
    }
}

/// Puts tiers in place of the ones the app carries. False when they didn't read (the app keeps
/// its own).
pub fn use_tiers(t: &Tiers) -> bool {
    let crowd = idleon_core::crowd::set(Some(&t.crowd));
    let expected = idleon_core::expected::set(Some(&t.expected));
    let ok = crowd.is_empty() && expected.is_empty();
    if !ok {
        // Half a set is worse than none.
        idleon_core::crowd::set(None);
        idleon_core::expected::set(None);
    }
    ok
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .user_agent(concat!("idleon-check/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn fail(e: ureq::Error) -> String {
    match e {
        ureq::Error::Io(_)
        | ureq::Error::ConnectionFailed
        | ureq::Error::HostNotFound
        | ureq::Error::Timeout(_) => "Couldn't reach the server. Check your connection.".into(),
        e => format!("The server answered something unexpected ({e})."),
    }
}

pub fn info(server: &str) -> Result<Info, String> {
    let mut r = agent()
        .get(format!("{server}/idleon/app/info"))
        .call()
        .map_err(fail)?;
    if !r.status().is_success() {
        return Err(format!("The server said {}.", r.status()));
    }
    r.body_mut()
        .read_json()
        .map_err(|e| format!("The server's answer didn't read ({e})."))
}

/// The server's tiers, or `None` when they're still `have`.
pub fn tiers(server: &str, have: Option<&str>) -> Result<Option<Tiers>, String> {
    let mut req = agent().get(format!("{server}/idleon/app/tiers"));
    if let Some(v) = have {
        req = req.header("If-None-Match", format!("\"{v}\""));
    }
    let mut r = req.call().map_err(fail)?;
    if r.status() == 304 {
        return Ok(None);
    }
    if !r.status().is_success() {
        return Err(format!("The server said {}.", r.status()));
    }
    let t: Tiers = r
        .body_mut()
        .with_config()
        .limit(8 * 1024 * 1024)
        .read_json()
        .map_err(|e| format!("The tiers didn't read ({e})."))?;
    Ok(Some(t))
}

/// A server's icon, for the header.
pub fn icon(url: &str) -> Option<Vec<u8>> {
    let mut r = agent().get(url).call().ok()?;
    r.status().is_success().then_some(())?;
    r.body_mut()
        .with_config()
        .limit(2 * 1024 * 1024)
        .read_to_vec()
        .ok()
}

#[derive(Debug, Clone, Deserialize)]
pub struct Link {
    pub code: String,
    pub secret: String,
}

pub fn link_start(server: &str) -> Result<Link, String> {
    let mut r = agent()
        .post(format!("{server}/idleon/app/link"))
        .send_empty()
        .map_err(fail)?;
    if !r.status().is_success() {
        return Err(r
            .body_mut()
            .read_to_string()
            .unwrap_or_else(|_| format!("The server said {}.", r.status())));
    }
    r.body_mut()
        .read_json()
        .map_err(|e| format!("The server's answer didn't read ({e})."))
}

/// Waits for the browser to confirm the code, up to the code's ten minutes.
pub fn link_wait(server: &str, link: &Link) -> Result<Session, String> {
    let a = agent();
    let until = std::time::Instant::now() + Duration::from_secs(600);
    while std::time::Instant::now() < until {
        match a
            .get(format!("{server}/idleon/app/link/{}", link.code))
            .query("secret", &link.secret)
            .call()
        {
            Ok(mut r) if r.status() == 200 => {
                #[derive(Deserialize)]
                struct Got {
                    session: String,
                    name: String,
                }
                let g: Got = r
                    .body_mut()
                    .read_json()
                    .map_err(|e| format!("The server's answer didn't read ({e})."))?;
                return Ok(Session {
                    token: g.session,
                    name: g.name,
                });
            }
            Ok(r) if r.status() == 202 => {}
            Ok(mut r) => {
                return Err(r
                    .body_mut()
                    .read_to_string()
                    .unwrap_or_else(|_| "That sign-in expired. Start again.".into()));
            }
            // A dropped connection while you're in the browser: keep waiting.
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    Err("That sign-in expired. Start again.".into())
}

/// Sends a report as the signed-in member: the page's own report address, with the sign-in as
/// its cookie. The server's answer is the line to show; `Err` with `true` means the sign-in is
/// gone and should be forgotten.
pub fn report(
    server: &str,
    guild: &str,
    token: &str,
    account: &str,
    card: &str,
    text: &str,
) -> Result<String, (String, bool)> {
    let r = agent()
        .post(format!("{server}/p/{guild}/idleon/report"))
        .header("Cookie", format!("msid={token}"))
        .send_form([("account", account), ("card", card), ("text", text)]);
    match r {
        Ok(mut r) => {
            let ok = r.status().is_success();
            let gone = r.status() == 403;
            let said = r.body_mut().read_to_string().unwrap_or_default();
            // A 403 asking to sign in means the session ended (expired or signed out elsewhere).
            let gone = gone && said.starts_with("Sign in");
            if ok { Ok(said) } else { Err((said, gone)) }
        }
        Err(e) => Err((fail(e), false)),
    }
}

/// Ends the sign-in on the server too.
pub fn sign_out(server: &str, guild: &str, token: &str) {
    let _ = agent()
        .post(format!("{server}/auth/signout"))
        .header("Cookie", format!("msid={token}"))
        .send_form([("to", format!("/p/{guild}/idleon").as_str())]);
}
