//! The RPG's own emoji set: one outline style, coloured by meaning. They're uploaded as the
//! bot's application emojis (no server emoji slots), and a plain emoji stands in until then
//! and in the preview.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use bot_core::anyhow::{Context, Result};
use bot_core::{Ctx, tokio};

/// Bumped when the drawings change, so the new ones are uploaded under new names.
const SET: &str = "rpg1";

/// (name, colour, path on a 24-unit grid, filled, plain emoji stand-in).
pub const ICONS: &[(&str, &str, &str, bool, &str)] = &[
    (
        "thor",
        "#7FB2FF",
        "M6 4h12a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1zM12 10v9M10 21h4",
        false,
        "🔨",
    ),
    (
        "athena",
        "#C9B8FF",
        "M12 3l7 3v6c0 4.5-3 7.5-7 9-4-1.5-7-4.5-7-9V6z",
        false,
        "🛡️",
    ),
    (
        "hades",
        "#8FD6C1",
        "M12 21V9M7 3v4a5 5 0 0 0 10 0V3M12 3v6",
        false,
        "🔱",
    ),
    (
        "expedition",
        "#D2A75E",
        "M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18M15.5 8.5l-2 5-5 2 2-5z",
        false,
        "🧭",
    ),
    (
        "forge",
        "#D2A75E",
        "M3 6h14a4 4 0 0 1-4 4h-1v3l3 4H8l3-4v-3H7a4 4 0 0 1-4-4zM7 20h10",
        false,
        "⚒️",
    ),
    (
        "boss",
        "#F0866B",
        "M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18M12 7a5 5 0 1 0 0 10a5 5 0 1 0 0-10M12 11a1 1 0 1 0 0 2a1 1 0 1 0 0-2",
        false,
        "🎯",
    ),
    (
        "gold",
        "#E2BD5C",
        "M9 4a6 6 0 1 0 0 12a6 6 0 1 0 0-12M15.5 8.6a6 6 0 1 1-6.9 6.9",
        false,
        "🪙",
    ),
    (
        "vigor",
        "#B49BFF",
        "M11 3l1.8 5.2L18 10l-5.2 1.8L11 17l-1.8-5.2L4 10l5.2-1.8zM18.5 15l.7 2 2 .7-2 .7-.7 2-.7-2-2-.7 2-.7z",
        false,
        "✨",
    ),
    (
        "offering",
        "#E0C47A",
        "M12 3c1 3 5 5 5 10a5 5 0 0 1-10 0c0-2.5 1.5-4 2.5-5 0 2 1 3 2 3 0-3-1-5.5.5-8z",
        false,
        "🔥",
    ),
    (
        "power",
        "#F4F1EA",
        "M13 2L4 14h7l-1 8 9-12h-7z",
        false,
        "⚡",
    ),
    (
        "hunt",
        "#DBDEE1",
        "M14.5 3.5H20v5.5L9.5 19.5l-5-5zM4.5 14.5l5 5M6 18l-2.5 2.5",
        false,
        "⚔️",
    ),
    (
        "pack",
        "#D2A75E",
        "M4 8h16v12H4zM8 8V5h8v3M4 13h16",
        false,
        "🎒",
    ),
    (
        "intel",
        "#7FB2FF",
        "M2 12s3.5-6 10-6 10 6 10 6-3.5 6-10 6S2 12 2 12zM12 9a3 3 0 1 0 0 6a3 3 0 1 0 0-6",
        false,
        "👁️",
    ),
    (
        "hp",
        "#F0866B",
        "M12 20s-7-4.4-7-10a4 4 0 0 1 7-2.6A4 4 0 0 1 19 10c0 5.6-7 10-7 10z",
        false,
        "❤️",
    ),
    (
        "iron",
        "#9FB3C8",
        "M3 17l3-8h12l3 8zM8 9l1-3h6l1 3",
        false,
        "⛓️",
    ),
    (
        "yew",
        "#C49A6C",
        "M6 8h12a2 4 0 0 1 0 8H6M6 8a2 4 0 1 0 0 8a2 4 0 1 0 0-8",
        false,
        "🪵",
    ),
    (
        "pelt",
        "#B98B5E",
        "M6 4l3 3h6l3-3v6c0 5-3 10-6 10S6 15 6 10z",
        false,
        "🐺",
    ),
    (
        "herbs",
        "#7FCB8A",
        "M5 19c0-8 5-14 14-14 0 9-6 14-14 14zM5 19l8-8",
        false,
        "🌿",
    ),
    (
        "fish",
        "#8FB8D8",
        "M3 12c3-4 8-5 12-3l4-3v12l-4-3c-4 2-9 1-12-3zM8 11v.5",
        false,
        "🐟",
    ),
    (
        "amber",
        "#E8A33D",
        "M7 4h10l4 6-9 11-9-11zM3 10h18",
        false,
        "🟠",
    ),
    (
        "fang",
        "#E9E4D8",
        "M7 4h10l-1 6c-.5 4-2 8-4 10-2-2-3.5-6-4-10z",
        false,
        "🦷",
    ),
    (
        "frost",
        "#B8D8F0",
        "M12 2v20M3.3 7l17.4 10M3.3 17l17.4-10M9 4l3 2 3-2M9 20l3-2 3 2",
        false,
        "❄️",
    ),
    (
        "rations",
        "#C9A27A",
        "M4 14a8 5 0 0 1 16 0v5H4zM8 11v3M12 10v3M16 11v3",
        false,
        "🍞",
    ),
    (
        "tonic",
        "#8FD0E8",
        "M9 3h6M10 3v6l-5 10h14l-5-10V3",
        false,
        "🧪",
    ),
    (
        "bait",
        "#C97B5A",
        "M14 3v11a4 4 0 0 1-8 0v-3l2 2",
        false,
        "🪝",
    ),
    (
        "potion",
        "#F0866B",
        "M10 3h4M11 3v4.2a6 6 0 1 0 2 0V3",
        false,
        "🧴",
    ),
    (
        "fireoil",
        "#F08A4B",
        "M12 3l5 8a5 5 0 1 1-10 0z",
        false,
        "🔥",
    ),
    (
        "frostoil",
        "#8FC8F0",
        "M12 3l5 8a5 5 0 1 1-10 0z",
        false,
        "💧",
    ),
    (
        "shard",
        "#C9B8FF",
        "M4 18l-1-10 5 4 4-7 4 7 5-4-1 10z",
        false,
        "👑",
    ),
    (
        "pip",
        "#7FB2FF",
        "M12 6a6 6 0 1 0 0 12a6 6 0 1 0 0-12",
        true,
        "🔵",
    ),
    (
        "pipoff",
        "#5D636B",
        "M12 6a6 6 0 1 0 0 12a6 6 0 1 0 0-12",
        false,
        "⚪",
    ),
    (
        "warn",
        "#F0B35C",
        "M12 3l10 18H2zM12 10v5M12 18v.5",
        false,
        "⚠️",
    ),
    ("check", "#7FCB8A", "M5 12l5 5 9-10", false, "✅"),
    (
        "lock",
        "#949BA4",
        "M6 11h12v10H6zM8 11V7a4 4 0 0 1 8 0v4",
        false,
        "🔒",
    ),
    (
        "unknown",
        "#949BA4",
        "M9 9a3 3 0 1 1 4 2.8c-.8.4-1 1-1 2.2M12 18v.5",
        false,
        "❔",
    ),
    (
        "shore",
        "#8FB8D8",
        "M3 15c3 0 3-2 6-2s3 2 6 2 3-2 6-2M3 19c3 0 3-2 6-2s3 2 6 2 3-2 6-2M8 9l4-5 4 5",
        false,
        "🌊",
    ),
    (
        "wolfwood",
        "#7FCB8A",
        "M12 3l6 9h-4l5 7H5l5-7H6z",
        false,
        "🌲",
    ),
    (
        "barrow",
        "#9FB3C8",
        "M3 19c2-8 16-8 18 0zM12 11V6M10 8h4",
        false,
        "🪦",
    ),
    (
        "temple",
        "#E0C47A",
        "M3 9l9-5 9 5M5 9v9M10 9v9M14 9v9M19 9v9M3 20h18",
        false,
        "🏛️",
    ),
    ("goals", "#D2A75E", "M5 21V4h11l-2 4 2 4H5", false, "🚩"),
    (
        "clock",
        "#DBDEE1",
        "M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18M12 7v5l3 2",
        false,
        "⏳",
    ),
    (
        "guard",
        "#DBDEE1",
        "M12 3l7 3v6c0 4.5-3 7.5-7 9-4-1.5-7-4.5-7-9V6z",
        false,
        "🛡️",
    ),
    (
        "spear",
        "#C9B8FF",
        "M5 19L19 5M15 5h4v4M8 16l-3-3",
        false,
        "🔱",
    ),
    (
        "owl",
        "#C9B8FF",
        "M5 5l3 3h8l3-3v9a7 7 0 0 1-14 0zM9.5 11a1.5 1.5 0 1 0 0 3M14.5 11a1.5 1.5 0 1 0 0 3",
        false,
        "🦉",
    ),
    (
        "rend",
        "#8FD6C1",
        "M6 20V10a6 6 0 0 1 12 0v10l-2-2-2 2-2-2-2 2-2-2z",
        false,
        "👻",
    ),
    (
        "bite",
        "#8FD6C1",
        "M7 4h10l-1 6c-.5 4-2 8-4 10-2-2-3.5-6-4-10z",
        false,
        "🐕",
    ),
    (
        "thunder",
        "#7FB2FF",
        "M13 2L4 14h7l-1 8 9-12h-7z",
        false,
        "🌩️",
    ),
    (
        "judgement",
        "#E0C47A",
        "M12 4v16M5 7h14M5 7l-3 6h6zM19 7l-3 6h6zM8 20h8",
        false,
        "⚖️",
    ),
    (
        "saga",
        "#D2A75E",
        "M6 4h11a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6M5 6a2 2 0 0 0-2 2v1h2M9 9h6M9 13h6",
        false,
        "📜",
    ),
    (
        "bounty",
        "#E2BD5C",
        "M12 3l2.6 5.3 5.9.9-4.2 4.1 1 5.8L12 16.4l-5.3 2.7 1-5.8-4.2-4.1 5.9-.9z",
        false,
        "⭐",
    ),
    (
        "elite",
        "#E2BD5C",
        "M4 18l-1-10 5 4 4-7 4 7 5-4-1 10z",
        false,
        "👑",
    ),
    (
        "event",
        "#B49BFF",
        "M12 2v4M12 18v4M4.9 4.9l2.8 2.8M16.3 16.3l2.8 2.8M2 12h4M18 12h4M4.9 19.1l2.8-2.8M16.3 7.7l2.8-2.8",
        false,
        "✨",
    ),
    (
        "labyrinth",
        "#C9B8FF",
        "M3 3h18v18H3zM7 7h10v10H7zM11 11h2v2h-2M3 12h4M17 12h4M12 3v4",
        false,
        "🌀",
    ),
    (
        "thread",
        "#E2BD5C",
        "M4 20c4-4 4-12 8-12s4 8 8 4M18 4l2 2-2 2",
        false,
        "🧵",
    ),
    ("common", "#949BA4", "M12 4l8 8-8 8-8-8z", false, "⚪"),
    ("fine", "#7FCB8A", "M12 4l8 8-8 8-8-8z", true, "🟢"),
    ("rare", "#7FB2FF", "M12 4l8 8-8 8-8-8z", true, "🔵"),
    ("epic", "#C9B8FF", "M12 4l8 8-8 8-8-8z", true, "🟣"),
    (
        "guild",
        "#E0C47A",
        "M6 3h12v8c0 5-6 9-6 9s-6-4-6-9zM12 7v8M9 10h6",
        false,
        "🛡️",
    ),
    (
        "dungeon",
        "#E0C47A",
        "M5 21V10a7 7 0 0 1 14 0v11M9 21v-6h6v6M3 21h18",
        false,
        "🚪",
    ),
    (
        "map",
        "#D2A75E",
        "M3 6l6-3 6 3 6-3v15l-6 3-6-3-6 3zM9 3v15M15 6v15",
        false,
        "🗺️",
    ),
    (
        "market",
        "#E2BD5C",
        "M4 9l2-5h12l2 5M4 9h16v11H4zM4 9c0 2 4 2 4 0 0 2 4 2 4 0 0 2 4 2 4 0 0 2 4 2 4 0M10 20v-5h4v5",
        false,
        "🏪",
    ),
    (
        "quest",
        "#E0C47A",
        "M12 2a4 4 0 1 1 0 8 4 4 0 0 1 0-8M5 21c0-4 3-7 7-7s7 3 7 7",
        false,
        "🗣️",
    ),
    (
        "help",
        "#DBDEE1",
        "M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.6.3-1 .8-1 1.7M12 16.5v.5",
        false,
        "❓",
    ),
];

fn registry() -> &'static RwLock<HashMap<String, u64>> {
    static R: OnceLock<RwLock<HashMap<String, u64>>> = OnceLock::new();
    R.get_or_init(|| RwLock::new(HashMap::new()))
}

fn full_name(name: &str) -> String {
    format!("{SET}_{name}")
}

/// The uploaded emoji's id and full name, once the upload has happened.
pub fn custom(name: &str) -> Option<(u64, String)> {
    let n = full_name(name);
    registry().read().unwrap().get(&n).map(|id| (*id, n))
}

pub fn unicode(name: &str) -> &'static str {
    ICONS
        .iter()
        .find(|i| i.0 == name)
        .map(|i| i.4)
        .unwrap_or("▫️")
}

/// The emoji for message text: `<:rpg1_gold:123>`, or the plain stand-in.
pub fn get(name: &str) -> String {
    match custom(name) {
        Some((id, n)) => format!("<:{n}:{id}>"),
        None => unicode(name).to_owned(),
    }
}

/// For the dashboard preview: an image of the uploaded emoji, else the stand-in.
pub fn html(name: &str) -> String {
    match custom(name) {
        Some((id, _)) => format!(
            "<img class=\"rpg-emoji\" alt=\"\" src=\"https://cdn.discordapp.com/emojis/{id}.png?size=44\">"
        ),
        None => unicode(name).to_owned(),
    }
}

/// An icon as a 128 px SVG.
pub fn svg(name: &str) -> Option<String> {
    let (_, colour, path, filled, _) = ICONS.iter().find(|i| i.0 == name)?;
    let paint = if *filled {
        format!("fill=\"{colour}\" stroke=\"none\"")
    } else {
        format!(
            "fill=\"none\" stroke=\"{colour}\" stroke-width=\"2.1\" stroke-linecap=\"round\" stroke-linejoin=\"round\""
        )
    };
    Some(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 24 24"><path d="{path}" {paint}/></svg>"#
    ))
}

/// Load the set from Discord, uploading what's missing. Runs once the bot knows its
/// application id; safe to call again.
pub async fn sync(ctx: &Ctx) -> Result<usize> {
    if !ctx.has_app_id() || ctx.cfg.preview {
        return Ok(0);
    }
    let app = ctx.app_id();
    let list = ctx.http.get_application_emojis(app).await?.model().await?;
    let mut have: HashMap<String, u64> = list
        .items
        .into_iter()
        .map(|e| (e.name, e.id.get()))
        .collect();
    let mut uploaded = 0;
    for (name, ..) in ICONS {
        let full = full_name(name);
        if have.contains_key(&full) {
            continue;
        }
        let Some(svg) = svg(name) else { continue };
        let png = tokio::task::spawn_blocking(move || bot_render::png(&svg)).await??;
        let uri = bot_render::data_uri(&png);
        let e = ctx
            .http
            .add_application_emoji(app, &full, &uri)
            .await
            .with_context(|| format!("uploading {full}"))?
            .model()
            .await?;
        have.insert(full, e.id.get());
        uploaded += 1;
    }
    let mut reg = registry().write().unwrap();
    for (n, id) in have {
        if n.starts_with(SET) {
            reg.insert(n, id);
        }
    }
    Ok(uploaded)
}

pub fn ready() -> bool {
    !registry().read().unwrap().is_empty()
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_icon_draws_and_names_fit_discord() {
        for (name, ..) in super::ICONS {
            let full = super::full_name(name);
            assert!(
                full.len() <= 32 && full.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "{full}"
            );
            let png = bot_render::png(&super::svg(name).unwrap()).unwrap();
            assert!(png.len() > 100, "{name} drew nothing");
        }
    }
}
