//! The rules of asking to connect, with no IO, ported from antho-s-bot
//! (`src/server/features/connect/domain/`). A device makes a random verifier, keeps it, and sends
//! only its sha256 (the challenge); an approver allows the request on Discord; the device then
//! shows the verifier to collect its pairing link. Someone who sees the request, or the DM, cannot.

use std::net::{IpAddr, Ipv4Addr};

use sha2::{Digest, Sha256};

/// A request is good for 15 minutes.
pub const TTL_MS: i64 = 15 * 60_000;
/// Open requests at once, across every device: past this, a new one waits for one to settle.
pub const MAX_PENDING: i64 = 3;
/// One request per address in this window, so a loop cannot fire DMs back to back.
pub const IP_COOLDOWN_MS: i64 = 5_000;
/// Rounds of approval DMs in a rolling hour. Past this, requests wait on the dashboard only.
pub const MAX_DM_ROUNDS_PER_HOUR: i64 = 6;
/// Pairing links one allowed request may collect: a device whose first attempt failed retries.
pub const MAX_CLAIMS: i64 = 5;
/// Old requests are history nobody reads: keep a month.
pub const KEEP_MS: i64 = 30 * 86_400_000;

/// Who asks. `tally`, `avex` and `pc` pair with Relay's cloud copy beside the bot; `relay` is the
/// Relay phone app asking for the owner's PC, which opens the window itself.
pub const KINDS: [&str; 4] = ["tally", "avex", "pc", "relay"];

pub fn is_kind(kind: &str) -> bool {
    KINDS.contains(&kind)
}

/// Kinds that pair with the cloud copy on this server.
pub fn pairs_with_cloud(kind: &str) -> bool {
    matches!(kind, "tally" | "avex" | "pc")
}

/// What a device polling its request is told. `waiting_pc`: allowed, and waiting for the owner's
/// PC to open a pairing window (a `relay` request only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimState {
    Waiting,
    WaitingPc,
    Denied,
    Expired,
    Ready,
    Spent,
}

impl ClaimState {
    pub fn as_str(self) -> &'static str {
        match self {
            ClaimState::Waiting => "waiting",
            ClaimState::WaitingPc => "waiting_pc",
            ClaimState::Denied => "denied",
            ClaimState::Expired => "expired",
            ClaimState::Ready => "ready",
            ClaimState::Spent => "spent",
        }
    }
}

/// Where a request stands for the device polling it.
pub fn claim_state(kind: &str, status: &str, expires_at: i64, claims: i64, has_pc_link: bool, now: i64) -> ClaimState {
    if status == "denied" {
        return ClaimState::Denied;
    }
    if expires_at <= now {
        return ClaimState::Expired;
    }
    if status == "pending" {
        return ClaimState::Waiting;
    }
    if claims >= MAX_CLAIMS {
        return ClaimState::Spent;
    }
    if kind == "relay" && !has_pc_link {
        return ClaimState::WaitingPc;
    }
    ClaimState::Ready
}

/// A name the device chose, safe to show in a DM and store: no control characters, no markdown,
/// nothing that makes a link (brackets, parentheses, ":" and "/"), 48 characters at most. The
/// name comes from an unauthenticated request and lands in the approvers' DMs.
pub fn clean_device_name(raw: &str) -> String {
    let visible: String = raw
        .chars()
        .map(|c| if (c as u32) < 0x20 || c as u32 == 0x7f { ' ' } else { c })
        .filter(|c| !matches!(c, '`' | '*' | '_' | '~' | '|' | '<' | '>' | '@' | '#' | '[' | ']' | '(' | ')' | ':' | '/' | '\\'))
        .collect();
    let collapsed = visible.split_whitespace().collect::<Vec<_>>().join(" ");
    let name: String = collapsed.chars().take(48).collect();
    let name = name.trim();
    if name.is_empty() { "a device".into() } else { name.to_owned() }
}

pub fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The four characters the device and the DM both show, so the approver can tell it is theirs.
pub fn display_code(challenge: &str) -> String {
    challenge.chars().take(4).collect::<String>().to_uppercase()
}

pub fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// Whether `verifier` is the secret behind `challenge`, compared in constant time.
pub fn verifier_matches(verifier: &str, challenge: &str) -> bool {
    if !is_hex(verifier, 64) || !is_hex(challenge, 64) {
        return false;
    }
    let a = Sha256::digest(verifier.as_bytes());
    let Ok(b) = hex::decode(challenge) else { return false };
    a.len() == b.len() && a.iter().zip(&b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// How a request reads to people: the device, in words.
pub fn device_label(kind: &str, device: &str) -> String {
    match kind {
        "pc" => format!("Relay on {device}"),
        "relay" => format!("the Relay app on {device}"),
        "avex" => format!("Avex on {device}"),
        _ => format!("Tally on {device}"),
    }
}

/// The app's name as the dashboard shows it in a column.
pub fn kind_name(kind: &str) -> &'static str {
    match kind {
        "pc" => "PC",
        "relay" => "Relay app",
        "avex" => "Avex",
        _ => "Tally",
    }
}

/// The DM asking an approver: what wants in, the code to compare, and what allowing it gives.
pub fn ask_text(kind: &str, device: &str, code: &str) -> String {
    let who = device_label(kind, device);
    let head = match kind {
        "relay" => format!("🔐 **{who}** wants to pair with your PC."),
        "pc" => format!("🔐 **{who}** wants to link your PC with your server."),
        _ => format!("🔐 **{who}** wants to sync with your server."),
    };
    let gives = match kind {
        "avex" => "Allowed, it sends your training history to your server.",
        "pc" => "Allowed, your PC and your server keep your budget and training history in step.",
        "relay" => "Allowed, your PC asks you once more on its screen. Accepted there, the phone gets full control of your PC as you.",
        _ => "Allowed, it can read and change your budget.",
    };
    let shows = if kind == "pc" { "terminal" } else { "app" };
    format!("{head}\nThe {shows} shows the code **{code}**: allow it only if yours does.\n{gives} The request expires in 15 minutes.")
}

/// The DM's line once someone answered (or the request ran out).
pub fn settled_text(kind: &str, device: &str, status: &str) -> String {
    let who = device_label(kind, device);
    match status {
        "allowed" | "claimed" if kind == "relay" => format!("✅ Allowed. **{who}** now waits for your PC: accept it there too."),
        "allowed" | "claimed" => format!("✅ Allowed. **{who}** is connecting to your server and will sync on its own from now on."),
        "denied" => format!("🚫 Denied. **{who}** was not let in."),
        _ => format!("⌛ This request from **{who}** expired. Open the app again to ask once more."),
    }
}

/// `connect:allow:<id>` and `connect:deny:<id>`.
pub fn button_id(allow: bool, id: &str) -> String {
    format!("{}:{}:{id}", crate::KEY, if allow { "allow" } else { "deny" })
}

/// The parts after the `connect:` prefix, back to `(allow, id)`.
pub fn parse_button(args: &[&str]) -> Option<(bool, String)> {
    match args {
        ["allow", id] if is_hex(id, 32) => Some((true, (*id).to_owned())),
        ["deny", id] if is_hex(id, 32) => Some((false, (*id).to_owned())),
        _ => None,
    }
}

/// `n` random bytes as lowercase hex.
pub fn random_hex(n: usize) -> String {
    use rand::RngExt as _;
    let mut rng = rand::rng();
    let bytes: Vec<u8> = (0..n).map(|_| rng.random()).collect();
    hex::encode(bytes)
}

/// A PC's token for this server: 32 random bytes, hex. Only its sha256 is stored.
pub fn new_host_token() -> (String, String) {
    let token = random_hex(32);
    let hash = sha256_hex(&token);
    (token, hash)
}

pub fn host_token_hash(token: &str) -> Option<String> {
    is_hex(token, 64).then(|| sha256_hex(token))
}

/// What a PC may hand a phone: a `relay://pair` link, nothing else, of a sane length.
pub fn is_pair_link(link: &str) -> bool {
    link.starts_with("relay://pair?") && link.len() <= 2048 && !link.chars().any(char::is_whitespace)
}

/// The `relay://pair` link in what `relay remote pair` printed, if it printed one.
pub fn parse_pair_link(stdout: &str) -> Option<String> {
    let at = stdout.find("relay://pair?")?;
    let link: String = stdout[at..].chars().take_while(|c| !c.is_whitespace()).collect();
    (link.len() > "relay://pair?".len()).then_some(link)
}

/// The client's address from X-Forwarded-For: the rightmost entry that isn't a private,
/// loopback or link-local address. Entries to its left were written by the client and can say
/// anything; entries to its right are our own proxies (Caddy trusts private ranges and appends
/// Render's edge). With no public entry (a local setup), the rightmost entry. X-Real-IP isn't
/// read: nothing in front of the bot sets it, so a client could.
pub fn client_ip(forwarded_for: Option<&str>) -> Option<String> {
    let entries: Vec<IpAddr> = forwarded_for?.split(',').filter_map(parse_ip).collect();
    let pick = entries.iter().rev().find(|ip| is_public(ip)).or(entries.last())?;
    Some(pick.to_string())
}

/// An address as a proxy writes it: bare, with a port, or in brackets.
fn parse_ip(entry: &str) -> Option<IpAddr> {
    let e = entry.trim();
    e.parse::<IpAddr>().ok().or_else(|| e.parse::<std::net::SocketAddr>().ok().map(|s| s.ip())).or_else(|| {
        e.strip_prefix('[').and_then(|r| r.strip_suffix(']')).and_then(|r| r.parse().ok())
    })
}

fn is_public(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => public_v4(&v4),
            None => {
                let first = v6.segments()[0];
                !(v6.is_loopback() || v6.is_unspecified() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80)
            }
        },
    }
}

fn public_v4(ip: &Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    // 100.64.0.0/10 is carrier-grade NAT space, never a client's own public address.
    let shared = a == 100 && (64..128).contains(&b);
    !(ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_broadcast() || shared)
}

/// Length as JavaScript counts it, which is what the old bot's limits were written against.
pub fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

#[cfg(test)]
mod tests {

    #[test]
    fn device_names_cant_carry_a_link() {
        assert_eq!(clean_device_name("[Verify here](https://evil.example/x)"), "Verify herehttpsevil.examplex");
        assert_eq!(clean_device_name("Antho's PC"), "Antho's PC");
        assert!(!clean_device_name("https://evil.example").contains("://"));
    }
    use super::*;

    const V: &str = "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0";

    #[test]
    fn the_verifier_opens_only_its_own_challenge() {
        let challenge = sha256_hex(V);
        assert!(verifier_matches(V, &challenge));
        let other = sha256_hex("other");
        assert!(!verifier_matches(V, &other));
        assert!(!verifier_matches("short", &challenge));
        assert!(!verifier_matches(&V.to_uppercase(), &challenge));
        assert!(!verifier_matches(V, "zz"));
    }

    #[test]
    fn the_code_is_the_first_four_of_the_challenge_upper_case() {
        assert_eq!(display_code("ab12cdef"), "AB12");
        // Same as ServerConnect.codeOf on the phone.
        let challenge = sha256_hex(V);
        assert_eq!(display_code(&challenge), challenge[..4].to_uppercase());
    }

    #[test]
    fn sha256_matches_the_known_vector() {
        assert_eq!(sha256_hex("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn claim_states_follow_the_old_order() {
        let now = 1_000_000;
        let later = now + 1;
        assert_eq!(claim_state("tally", "denied", now - 5, 0, false, now), ClaimState::Denied);
        assert_eq!(claim_state("tally", "pending", now, 0, false, now), ClaimState::Expired);
        assert_eq!(claim_state("tally", "allowed", now - 1, 0, false, now), ClaimState::Expired);
        assert_eq!(claim_state("tally", "pending", later, 0, false, now), ClaimState::Waiting);
        assert_eq!(claim_state("tally", "allowed", later, 0, false, now), ClaimState::Ready);
        assert_eq!(claim_state("pc", "claimed", later, 4, false, now), ClaimState::Ready);
        assert_eq!(claim_state("pc", "claimed", later, 5, false, now), ClaimState::Spent);
        assert_eq!(claim_state("relay", "allowed", later, 0, false, now), ClaimState::WaitingPc);
        assert_eq!(claim_state("relay", "allowed", later, 0, true, now), ClaimState::Ready);
        assert_eq!(ClaimState::WaitingPc.as_str(), "waiting_pc");
    }

    #[test]
    fn device_names_are_cleaned() {
        assert_eq!(clean_device_name("  Pixel\t8\n**Pro**  "), "Pixel 8 Pro");
        assert_eq!(clean_device_name("<@123> #general"), "123 general");
        assert_eq!(clean_device_name("\u{7f}\u{1}"), "a device");
        assert_eq!(clean_device_name(&"x".repeat(80)).len(), 48);
    }

    #[test]
    fn hex_checks() {
        assert!(is_hex(&"a".repeat(32), 32));
        assert!(!is_hex(&"A".repeat(32), 32));
        assert!(!is_hex(&"g".repeat(32), 32));
        assert!(!is_hex(&"a".repeat(31), 32));
        assert_eq!(random_hex(16).len(), 32);
        assert!(is_hex(&random_hex(16), 32));
    }

    #[test]
    fn host_tokens_are_hashed() {
        let (token, hash) = new_host_token();
        assert!(is_hex(&token, 64));
        assert_eq!(host_token_hash(&token), Some(hash));
        assert_eq!(host_token_hash("nope"), None);
    }

    #[test]
    fn pair_links() {
        assert!(is_pair_link("relay://pair?v=1&code=AB"));
        assert!(!is_pair_link("https://pair?x"));
        assert!(!is_pair_link("relay://pair?a b"));
        assert!(!is_pair_link(&format!("relay://pair?{}", "a".repeat(2048))));
        let out = "Pairing window open for 10 minutes.\nLink: relay://pair?v=1&room=abc&code=XY12\n";
        assert_eq!(parse_pair_link(out).as_deref(), Some("relay://pair?v=1&room=abc&code=XY12"));
        assert_eq!(parse_pair_link("nothing here"), None);
    }

    #[test]
    fn the_client_ip_is_the_rightmost_public_address() {
        // What Caddy passes on: whatever the client wrote, the real client, Render's edge.
        assert_eq!(client_ip(Some("1.1.1.1, 203.0.113.9, 10.0.0.1")).as_deref(), Some("203.0.113.9"));
        assert_eq!(client_ip(Some("203.0.113.9, 10.20.0.5")).as_deref(), Some("203.0.113.9"));
        // A spoofed leftmost entry is ignored, even a private one.
        assert_eq!(client_ip(Some("10.0.0.7, 198.51.100.4, 172.16.3.1")).as_deref(), Some("198.51.100.4"));
        // Ports, brackets, IPv6, mapped IPv4, link-local and CGNAT.
        assert_eq!(client_ip(Some("198.51.100.4:5555, 192.168.1.1")).as_deref(), Some("198.51.100.4"));
        assert_eq!(client_ip(Some("[2001:db8::1]:443, fd00::2")).as_deref(), Some("2001:db8::1"));
        assert_eq!(client_ip(Some("2001:db8::7, fe80::1, ::1")).as_deref(), Some("2001:db8::7"));
        assert_eq!(client_ip(Some("::ffff:10.0.0.1, 203.0.113.2, 100.70.0.1, 169.254.1.1")).as_deref(), Some("203.0.113.2"));
        // Nothing public (a local setup): the rightmost entry.
        assert_eq!(client_ip(Some("10.0.0.1, 127.0.0.1")).as_deref(), Some("127.0.0.1"));
        assert_eq!(client_ip(Some("garbage, , ")), None);
        assert_eq!(client_ip(Some(" ")), None);
        assert_eq!(client_ip(None), None);
    }

    #[test]
    fn buttons_round_trip() {
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(button_id(true, id), format!("connect:allow:{id}"));
        assert_eq!(parse_button(&["allow", id]), Some((true, id.to_owned())));
        assert_eq!(parse_button(&["deny", id]), Some((false, id.to_owned())));
        assert_eq!(parse_button(&["maybe", id]), None);
        assert_eq!(parse_button(&["allow", "x"]), None);
    }

    #[test]
    fn dm_text_names_the_device_and_code() {
        let t = ask_text("relay", "Pixel 8", "AB12");
        assert!(t.contains("**the Relay app on Pixel 8** wants to pair with your PC."));
        assert!(t.contains("The app shows the code **AB12**"));
        assert!(ask_text("pc", "desk", "AB12").contains("The terminal shows"));
        assert!(settled_text("tally", "phone", "denied").contains("Denied. **Tally on phone**"));
        assert!(settled_text("relay", "phone", "allowed").contains("accept it there too"));
        assert!(settled_text("avex", "phone", "pending").contains("expired"));
    }
}
