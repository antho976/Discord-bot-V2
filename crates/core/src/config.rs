//! Settings read once from the environment at boot.

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub discord_token: String,
    pub discord_client_id: String,
    pub discord_client_secret: String,
    /// Public address of the dashboard, e.g. `https://bot.example.com`. No trailing slash.
    pub public_url: String,
    /// The dashboard's first admin. Always has full access and can't be removed.
    pub owner_id: u64,
    pub database_path: String,
    pub port: u16,
    pub twitch_client_id: Option<String>,
    pub twitch_client_secret: Option<String>,
    pub twitch_eventsub_secret: Option<String>,
    pub relay_bin: String,
    pub relay_public_url: Option<String>,
    /// Local preview with sample data: no Discord connection, a sign-in link that skips OAuth.
    pub preview: bool,
}

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let need = |name: &str| var(name).with_context(|| format!("{name} is not set"));
        // PUBLIC_BASE_URL and TALLY_OWNER_ID are antho-s-bot's names, accepted so the same
        // Render service can switch over without renaming its variables.
        let public_url = var("PUBLIC_URL")
            .or_else(|| var("PUBLIC_BASE_URL"))
            .or_else(|| var("RENDER_EXTERNAL_URL"))
            .unwrap_or_else(|| "http://localhost:3001".into())
            .trim_end_matches('/')
            .to_owned();
        Ok(Self {
            discord_token: need("DISCORD_TOKEN")?,
            discord_client_id: need("DISCORD_CLIENT_ID")?,
            discord_client_secret: need("DISCORD_CLIENT_SECRET")?,
            public_url,
            owner_id: var("OWNER_ID")
                .or_else(|| var("TALLY_OWNER_ID"))
                .context("OWNER_ID is not set (your Discord user id)")?
                .parse()
                .context("OWNER_ID must be a Discord user id")?,
            database_path: var("DATABASE_PATH").unwrap_or_else(|| "data/bot-v2.db".into()),
            port: var("PORT").and_then(|p| p.parse().ok()).unwrap_or(3001),
            twitch_client_id: var("TWITCH_CLIENT_ID"),
            twitch_client_secret: var("TWITCH_CLIENT_SECRET"),
            twitch_eventsub_secret: var("TWITCH_EVENTSUB_SECRET"),
            relay_bin: var("RELAY_BIN").unwrap_or_else(|| "relay".into()),
            relay_public_url: var("RELAY_PUBLIC_URL").filter(|v| v != "off"),
            preview: false,
        })
    }

    pub fn preview(port: u16) -> Self {
        Self {
            discord_token: "preview".into(),
            discord_client_id: "0".into(),
            discord_client_secret: "preview".into(),
            public_url: format!("http://localhost:{port}"),
            owner_id: 1,
            database_path: ":memory:".into(),
            port,
            twitch_client_id: None,
            twitch_client_secret: None,
            twitch_eventsub_secret: None,
            // Never a real binary: claiming in the preview must not open a pairing window.
            relay_bin: "/nonexistent/relay-preview".into(),
            relay_public_url: None,
            preview: true,
        }
    }

    pub fn secure_cookies(&self) -> bool {
        self.public_url.starts_with("https://")
    }
}
