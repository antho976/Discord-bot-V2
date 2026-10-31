//! A local preview of the dashboard with sample servers and no Discord connection. Used to look
//! at pages while building them: `bot preview`, then open the printed address.

use anyhow::Result;
use serde_json::json;
use twilight_gateway::Event;
use twilight_model::gateway::payload::incoming::GuildCreate;

use crate::Ctx;

pub const GUILD_A: u64 = 1_100_000_000_000_000_001;
pub const GUILD_B: u64 = 1_100_000_000_000_000_002;

/// Sample members present in both preview servers: `(user id, name)`.
pub const MEMBERS: &[(u64, &str)] = &[
    (2_000_000_000_000_000_001, "Mira"),
    (2_000_000_000_000_000_002, "Oskar"),
    (2_000_000_000_000_000_003, "juniper.fern"),
    (2_000_000_000_000_000_004, "Teo"),
    (2_000_000_000_000_000_005, "Kasia"),
    (2_000_000_000_000_000_006, "bramble"),
    (2_000_000_000_000_000_007, "Noor"),
    (2_000_000_000_000_000_008, "Lu"),
    (2_000_000_000_000_000_009, "Ines"),
    (2_000_000_000_000_000_010, "Pavel"),
];

fn guild(id: u64, name: &str, members: u64) -> serde_json::Value {
    let ch = |n: u64, name: &str, kind: u8, pos: i64| {
        json!({"id": (id + n * 10).to_string(), "type": kind, "name": name, "position": pos, "guild_id": id.to_string(), "permission_overwrites": []})
    };
    let role = |n: u64, name: &str, color: u32, pos: i64| {
        json!({"id": (id + n * 100).to_string(), "name": name, "color": color, "colors": {"primary_color": color}, "hoist": false, "managed": false, "mentionable": true, "permissions": "0", "position": pos, "flags": 0})
    };
    json!({
        "id": id.to_string(), "name": name, "icon": null, "owner_id": "1", "afk_timeout": 300,
        "default_message_notifications": 1, "explicit_content_filter": 0, "features": [],
        "mfa_level": 0, "nsfw_level": 0, "preferred_locale": "en-US", "premium_tier": 1,
        "system_channel_flags": 0, "verification_level": 1, "premium_progress_bar_enabled": false, "member_count": members,
        "roles": [
            {"id": id.to_string(), "name": "@everyone", "color": 0, "colors": {"primary_color": 0}, "hoist": false, "managed": false, "mentionable": false, "permissions": "0", "position": 0, "flags": 0},
            role(1, "Mods", 0x5865f2, 5), role(2, "Streamer", 0xf0806a, 6), role(3, "Regulars", 0x4fc38a, 3),
            role(4, "Live pings", 0xe3a948, 2), role(5, "Idleon", 0x9b84ee, 1)
        ],
        "channels": [
            ch(1, "Info", 4, 0), ch(2, "welcome", 0, 1), ch(3, "announcements", 5, 2), ch(4, "general", 0, 3),
            ch(5, "stream-alerts", 0, 4), ch(6, "pets", 0, 5), ch(7, "suggestions", 0, 6), ch(8, "mod-log", 0, 7),
            ch(9, "starboard", 0, 8), ch(10, "tickets", 0, 9)
        ],
        "emojis": [], "stickers": [], "members": MEMBERS.iter().map(|(uid, name)| json!({
            "user": {"id": uid.to_string(), "username": name.to_lowercase(), "global_name": name, "discriminator": "0", "avatar": null, "bot": false},
            "roles": [], "joined_at": "2025-03-01T00:00:00.000000+00:00", "deaf": false, "mute": false, "flags": 0
        })).collect::<Vec<_>>(), "presences": [], "voice_states": [], "threads": [],
        "stage_instances": [], "guild_scheduled_events": [], "unavailable": false, "large": false,
        "joined_at": "2024-01-01T00:00:00.000000+00:00"
    })
}

/// Fill the cache with two sample servers, switch on every feature in the first and seed a
/// little history so charts aren't empty.
pub fn seed(ctx: &Ctx) -> Result<()> {
    for (id, name, members) in [(GUILD_A, "Antho's server", 1284u64), (GUILD_B, "Idleon guild", 212)] {
        let g: GuildCreate = serde_json::from_value(guild(id, name, members))?;
        ctx.cache.update(&Event::GuildCreate(Box::new(g)));
    }
    for f in ctx.features() {
        ctx.set_enabled(GUILD_A, f.meta().key, true)?;
    }
    for f in ctx.features() {
        if let Err(e) = f.seed_preview(ctx, GUILD_A) {
            tracing::warn!(feature = f.meta().key, error = %e, "preview seed failed");
        }
    }
    let today = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC).date();
    let mut conn = ctx.db.lock();
    let tx = conn.transaction()?;
    for back in 0..30i64 {
        let day = today.checked_sub(jiff::Span::new().days(back))?.to_string();
        let wave = ((back as f64) * 0.7).sin().abs();
        for (key, base, spread) in [("messages", 380.0, 420.0), ("joins", 3.0, 9.0), ("leaves", 1.0, 4.0)] {
            let v = (base + spread * wave + ((back * 37) % 11) as f64 * spread / 20.0) as i64;
            tx.execute(
                "INSERT OR REPLACE INTO daily_stats (guild_id, day, key, value) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![GUILD_A as i64, day, key, v],
            )?;
        }
    }
    let now = crate::now_ms();
    let owner = ctx.cfg.owner_id as i64;
    tx.execute(
        "INSERT OR IGNORE INTO dash_users (id, name, avatar, admin, created_at) VALUES (?1, 'Antho', NULL, 1, ?2)",
        rusqlite::params![owner, now - 40 * 86_400_000],
    )?;
    // People with dashboard access, and what they changed, for the People and Activity pages.
    let h = 3_600_000;
    let people = [(MEMBERS[0], true, 2 * h), (MEMBERS[1], false, h / 5), (MEMBERS[3], false, 26 * h), (MEMBERS[6], false, 0)];
    for ((id, name), admin, seen) in people {
        tx.execute(
            "INSERT OR IGNORE INTO dash_users (id, name, avatar, admin, added_by, created_at) VALUES (?1, ?2, NULL, ?3, ?4, ?5)",
            rusqlite::params![id as i64, name, admin, owner, now - if seen == 0 { h } else { 20 * 86_400_000 }],
        )?;
        if seen > 0 {
            tx.execute(
                "INSERT OR IGNORE INTO dash_sessions (token_hash, user_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?3)",
                rusqlite::params![format!("preview-{id}"), id as i64, now - seen],
            )?;
        }
    }
    for (g, page, level) in [(GUILD_A, "guard", 2), (GUILD_A, "tickets", 2), (GUILD_A, "levels", 1), (GUILD_A, "welcome", 2), (GUILD_A, "community", 1), (GUILD_A, "pets", 1), (GUILD_A, "audit", 1)] {
        tx.execute(
            "INSERT OR IGNORE INTO dash_perms (user_id, guild_id, page, level) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![MEMBERS[1].0 as i64, g as i64, page, level],
        )?;
    }
    for page in ["levels", "pets"] {
        tx.execute(
            "INSERT OR IGNORE INTO dash_perms (user_id, guild_id, page, level) VALUES (?1, ?2, ?3, 1)",
            rusqlite::params![MEMBERS[3].0 as i64, GUILD_A as i64, page],
        )?;
    }
    let changes = [
        (h / 3, 1, "guard", "Released Ines from a scam timeout"),
        (2 * h, 9, "levels", "Imported XP from the old bot: 261 members, 26 reward roles"),
        (3 * h, 0, "pets", "Approved 3 pet requests"),
        (4 * h, 9, "users", "Gave Oskar edit access to Scam protection and Tickets"),
        (24 * h, 9, "features", "Turned Starboard off"),
        (25 * h, 1, "welcome", "Changed the welcome message"),
        (26 * h, 0, "levels", "Set the level 25 reward to @Veterans"),
        (50 * h, 1, "community", "Ended the Spring Idleon giveaway early"),
        (53 * h, 9, "guard", "Set #honeypot as the trap channel"),
        (56 * h, 0, "embeds", "Sent the rules embed to #welcome"),
    ];
    for (back, who, feature, action) in changes {
        let (id, name) = if who == 9 { (owner, "Antho") } else { (MEMBERS[who].0 as i64, MEMBERS[who].1) };
        tx.execute(
            "INSERT INTO dash_audit (at, user_id, user_name, guild_id, feature, action) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![now - back, id, name, GUILD_A as i64, feature, action],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Serve a preview of the dashboard with only these features, on `port`. Feature crates call
/// this from `examples/preview.rs` so each can be looked at on its own:
/// `cargo run -p feat-x --example preview -- 3101`, then open `http://localhost:3101/dev/login`.
pub async fn run(features: Vec<std::sync::Arc<dyn crate::Feature>>, port: u16) -> Result<()> {
    let cfg = crate::Config::preview(port);
    let db = crate::Db::open(&cfg.database_path)?;
    let ctx = Ctx::new(cfg, db)?;
    ctx.install_features(features)?;
    seed(&ctx)?;
    println!("Preview: http://localhost:{port}/dev/login?to=/g/{GUILD_A}");
    let (_stop, rx) = tokio::sync::watch::channel(false);
    crate::web::serve(ctx, rx).await
}

/// Entry point for a feature crate's `examples/preview.rs`: reads the port from the first
/// argument (default 3100) and runs [`run`] on a small runtime.
pub fn main(features: Vec<std::sync::Arc<dyn crate::Feature>>) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let port = std::env::args().nth(1).and_then(|p| p.parse().ok()).unwrap_or(3100);
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?.block_on(run(features, port))
}

#[cfg(test)]
mod tests {
    #[test]
    fn sample_guild_parses() {
        let v = super::guild(super::GUILD_A, "x", 1);
        let r: Result<twilight_model::guild::Guild, _> = serde_json::from_value(v);
        r.unwrap();
    }
}
