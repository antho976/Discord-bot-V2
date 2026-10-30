//! The welcome card: a 960 x 320 image drawn as SVG and rendered to PNG.

use std::time::Duration;

use bot_core::Ctx;
use bot_render::escape;

const W: u32 = 960;
const H: u32 = 320;
const TEXT_X: f64 = 312.0;
const TEXT_W: f64 = 592.0;

/// Keep what Geist can draw. Emoji and symbols come out as empty boxes, so they're dropped.
fn drawable(name: &str) -> String {
    let kept: String = name.chars().filter(|c| (*c as u32) < 0x2000 && !c.is_control()).collect();
    let kept = kept.trim();
    if kept.is_empty() { "new member".into() } else { bot_core::text::clip(kept, 24) }
}

/// The card as an SVG document. `avatar` is a `data:` URI; without one the card draws the
/// member's initial instead.
pub fn svg(name: &str, server: &str, number: Option<i64>, avatar: Option<&str>) -> String {
    let name = drawable(name);
    let title = format!("Welcome, {name}");
    // Geist semibold runs 0.55 to 0.7 em per character (wide capitals the most). Shrink long names to fit the line.
    let size = (TEXT_W / (title.chars().count() as f64 * 0.64)).clamp(30.0, 56.0);
    let server = escape(&drawable(server));
    let title = escape(&title);

    let portrait = match avatar {
        Some(uri) => format!(
            r##"<clipPath id="a"><circle cx="176" cy="160" r="88"/></clipPath>
  <image href="{uri}" x="88" y="72" width="176" height="176" preserveAspectRatio="xMidYMid slice" clip-path="url(#a)"/>"##
        ),
        None => {
            let initial = escape(&name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default());
            format!(
                r##"<circle cx="176" cy="160" r="88" fill="#1c1c1c"/>
  <text x="176" y="188" text-anchor="middle" font-family="Geist" font-weight="600" font-size="80" fill="#ededed">{initial}</text>"##
            )
        }
    };
    let member = number
        .map(|n| {
            format!(
                r##"<text x="{TEXT_X}" y="244" font-family="Geist Mono" font-weight="500" font-size="28" fill="#f0806a">Member #{}</text>"##,
                bot_core::text::num(n)
            )
        })
        .unwrap_or_default();

    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}">
  <rect width="{W}" height="{H}" fill="#0f0f0f"/>
  <rect x="0.5" y="0.5" width="{}" height="{}" fill="none" stroke="#262626"/>
  {portrait}
  <circle cx="176" cy="160" r="89.5" fill="none" stroke="#2a2a2a" stroke-width="3"/>
  <text x="{TEXT_X}" y="112" font-family="Geist" font-weight="400" font-size="24" fill="#8c8c8c">{server}</text>
  <text x="{TEXT_X}" y="184" font-family="Geist" font-weight="600" font-size="{size:.1}" fill="#ededed">{title}</text>
  {member}
</svg>"##,
        W - 1,
        H - 1
    )
}

/// Download an avatar. `None` when it can't be fetched in a few seconds.
pub async fn fetch_avatar(ctx: &Ctx, url: &str) -> Option<Vec<u8>> {
    let res = ctx.web.get(url).timeout(Duration::from_secs(5)).send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    let bytes = res.bytes().await.ok()?;
    (bytes.len() < 4 * 1024 * 1024).then(|| bytes.to_vec())
}

/// Draw the card and return PNG bytes. The avatar is fetched here; a missing one is not an error.
pub async fn render(ctx: &Ctx, name: &str, server: &str, number: Option<i64>, avatar_url: &str) -> bot_core::anyhow::Result<Vec<u8>> {
    let big = avatar_url.replace("size=128", "size=256");
    let avatar = fetch_avatar(ctx, &big).await.map(|b| bot_render::data_uri(&b));
    let svg = svg(name, server, number, avatar.as_deref());
    bot_core::tokio::task::spawn_blocking(move || bot_render::png(&svg)).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_renders_without_an_avatar() {
        let png = bot_render::png(&svg("Mira", "Antho's server", Some(1284), None)).unwrap();
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn names_are_cleaned_and_escaped() {
        assert_eq!(drawable("Mira \u{1f389}"), "Mira");
        assert_eq!(drawable("\u{1f389}"), "new member");
        let s = svg("a<b>&", "x", None, None);
        assert!(s.contains("a&lt;b&gt;&amp;"));
        assert!(!s.contains("Member #"));
    }
}
