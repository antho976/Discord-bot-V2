//! The welcome card: a 960 x 320 image drawn as SVG and rendered to PNG.

use std::time::Duration;

use bot_core::Ctx;
use bot_render::escape;

use crate::banners::Kind;

const W: u32 = 960;
const H: u32 = 320;
const TEXT_X: f64 = 312.0;
const TEXT_W: f64 = 592.0;
/// The server line may run a little further, to 40 px from the edge.
const SERVER_W: f64 = 608.0;

/// The characters Geist has: Latin with its accents (Vietnamese too), Cyrillic, and a few others.
fn geist_has(c: char) -> bool {
    matches!(c as u32,
        0x20..=0x7e | 0xa0..=0xac | 0xae..=0x113 | 0x116..=0x12b | 0x12e..=0x137 | 0x139..=0x13e | 0x141..=0x148
        | 0x14a..=0x14d | 0x150..=0x17e | 0x18f | 0x192 | 0x1a0..=0x1a1 | 0x1af..=0x1b0 | 0x1cd..=0x1ce | 0x1e4..=0x1e9
        | 0x218..=0x21b | 0x259 | 0x300..=0x304 | 0x306..=0x30c | 0x323 | 0x326..=0x328 | 0x400..=0x45f | 0x490..=0x493
        | 0x4ae..=0x4b3 | 0x4d8..=0x4d9 | 0x4e8..=0x4e9 | 0x1e80..=0x1e85 | 0x1e9e | 0x1ea0..=0x1ef9 | 0x2026)
}

/// A name as Geist can draw it. Emoji and symbols are dropped. A name with letters Geist lacks
/// (Greek, Arabic, Hebrew, CJK and so on) is None: drawn, it would be empty boxes or half a name.
fn drawable(name: &str) -> Option<String> {
    if name.chars().any(|c| c.is_alphanumeric() && !geist_has(c)) {
        return None;
    }
    let kept: String = name.chars().filter(|c| geist_has(*c)).collect();
    let kept = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    (!kept.is_empty()).then_some(kept)
}

/// Geist SemiBold's advance widths for ASCII 32 to 126, in hundredths of an em.
const ADVANCE: [u8; 95] = [
    24, 24, 38, 55, 66, 82, 68, 20, 31, 31, 42, 57, 22, 42, 22, 51, 68, 43, 64, 64, 64, 66, 62, 54, 64, 62, 31, 31, 55, 55, 55, 58,
    94, 71, 70, 72, 71, 62, 60, 73, 72, 29, 62, 67, 59, 90, 75, 76, 66, 76, 69, 67, 58, 70, 71, 99, 66, 61, 58, 38, 48, 38, 45, 56,
    27, 58, 62, 58, 62, 59, 43, 62, 60, 27, 31, 63, 30, 89, 60, 60, 62, 62, 41, 55, 43, 60, 58, 84, 63, 57, 57, 40, 28, 40, 52,
];

/// How wide `text` runs in Geist SemiBold, in ems. Outside ASCII it takes a capital's width
/// for capitals and a wide lowercase letter's otherwise, so it errs on the wide side.
fn em_width(text: &str) -> f64 {
    text.chars()
        .map(|c| match c as u32 {
            32..=126 => ADVANCE[c as usize - 32] as f64 / 100.0,
            0x300..=0x36f => 0.0,
            _ if c.is_uppercase() => 0.76,
            _ => 0.62,
        })
        .sum()
}

/// `text` shortened with an ellipsis until it fits `width` pixels at `size`.
fn fit(text: &str, size: f64, width: f64) -> String {
    if em_width(text) * size <= width {
        return text.to_owned();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() && em_width(&format!("{}\u{2026}", chars.iter().collect::<String>())) * size > width {
        chars.pop();
    }
    format!("{}\u{2026}", chars.iter().collect::<String>().trim_end())
}

/// The card as an SVG document. `names` are tried in order (display name, then username) and
/// the first Geist can draw is used. `avatar` is a `data:` URI; without one the card draws the
/// member's initial instead. `picture`, a `data:` URI too, is drawn behind everything, darkened
/// on the left where the face and name sit; without one the card has its own lit ground.
pub fn svg(kind: Kind, names: &[&str], server: &str, number: Option<i64>, avatar: Option<&str>, picture: Option<&str>) -> String {
    let fallback = if kind == Kind::Goodbye { "a member" } else { "new member" };
    let name = names.iter().find_map(|n| drawable(n)).unwrap_or_else(|| fallback.into());
    let title = format!("{}, {name}", if kind == Kind::Goodbye { "Goodbye" } else { "Welcome" });
    // Long names shrink to fit the line, down to a size that still reads; past that they're cut.
    let size = (TEXT_W / em_width(&title)).clamp(30.0, 56.0);
    let title = escape(&fit(&title, size, TEXT_W));
    let server = escape(&drawable(server).map(|s| fit(&s, 24.0, SERVER_W)).unwrap_or_default());

    let portrait = match avatar {
        Some(uri) => format!(
            r##"<clipPath id="a"><circle cx="176" cy="160" r="88"/></clipPath>
  <image href="{uri}" x="88" y="72" width="176" height="176" preserveAspectRatio="xMidYMid slice" clip-path="url(#a)"/>"##
        ),
        None => {
            let initial = escape(&name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default());
            format!(
                r##"<circle cx="176" cy="160" r="88" fill="#1b1a26"/>
  <text x="176" y="188" text-anchor="middle" font-family="Geist" font-weight="600" font-size="80" fill="#f3f2f8">{initial}</text>"##
            )
        }
    };
    // The member number sits in a pill sized to its text (Geist Mono runs 0.6 em a character).
    let member = number
        .map(|n| {
            let label = format!("Member #{}", bot_core::text::num(n));
            let w = label.chars().count() as f64 * 0.6 * 24.0 + 36.0;
            format!(
                r##"<rect x="{TEXT_X}" y="212" width="{w:.0}" height="46" rx="23" fill="#3ddcb0" fill-opacity="0.13"/>
  <rect x="{x:.1}" y="212.5" width="{w2:.0}" height="45" rx="22.5" fill="none" stroke="#3ddcb0" stroke-opacity="0.35"/>
  <text x="{tx:.0}" y="243" font-family="Geist Mono" font-weight="500" font-size="24" fill="#62f0c6">{label}</text>"##,
                x = TEXT_X + 0.5,
                w2 = w - 1.0,
                tx = TEXT_X + 18.0,
            )
        })
        .unwrap_or_default();

    // A dark ground lit by the members' teal behind the face, lilac from the top right and a
    // little coral low down; or the picture, under a shade that keeps the text readable on any
    // photo. A gradient ring round the avatar either way.
    let ground = match picture {
        Some(uri) => format!(
            r##"<rect width="{W}" height="{H}" fill="#0c0c14"/>
  <image href="{uri}" width="{W}" height="{H}" preserveAspectRatio="xMidYMid slice"/>
  <rect width="{W}" height="{H}" fill="url(#shade)"/>"##
        ),
        None => format!(
            r##"<rect width="{W}" height="{H}" fill="#0c0c14"/>
  <rect width="{W}" height="{H}" fill="url(#g1)"/>
  <rect width="{W}" height="{H}" fill="url(#g2)"/>
  <rect width="{W}" height="{H}" fill="url(#g3)"/>"##
        ),
    };
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}">
  <defs>
    <radialGradient id="g1" cx="176" cy="160" r="430" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#3ddcb0" stop-opacity="0.34"/><stop offset="1" stop-color="#3ddcb0" stop-opacity="0"/></radialGradient>
    <radialGradient id="g2" cx="930" cy="-20" r="560" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#9d7bff" stop-opacity="0.5"/><stop offset="1" stop-color="#9d7bff" stop-opacity="0"/></radialGradient>
    <radialGradient id="g3" cx="640" cy="380" r="360" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#ff8266" stop-opacity="0.16"/><stop offset="1" stop-color="#ff8266" stop-opacity="0"/></radialGradient>
    <linearGradient id="ring" x1="88" y1="72" x2="264" y2="248" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#62f0c6"/><stop offset="1" stop-color="#b99cff"/></linearGradient>
    <linearGradient id="shade" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="#08080e" stop-opacity="0.82"/><stop offset="0.62" stop-color="#08080e" stop-opacity="0.6"/><stop offset="1" stop-color="#08080e" stop-opacity="0.25"/></linearGradient>
  </defs>
  {ground}
  <circle cx="176" cy="160" r="108" fill="none" stroke="url(#ring)" stroke-opacity="0.22" stroke-width="2"/>
  {portrait}
  <circle cx="176" cy="160" r="91" fill="none" stroke="url(#ring)" stroke-width="6"/>
  <text x="{TEXT_X}" y="112" font-family="Geist" font-weight="500" font-size="24" fill="#c9c4dc">{server}</text>
  <text x="{TEXT_X}" y="184" font-family="Geist" font-weight="600" font-size="{size:.1}" fill="#f6f4fb">{title}</text>
  {member}
</svg>"##
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
/// `picture` is an uploaded picture's bytes, drawn behind the card.
pub async fn render(ctx: &Ctx, kind: Kind, names: &[&str], server: &str, number: Option<i64>, avatar_url: &str, picture: Option<&[u8]>) -> bot_core::anyhow::Result<Vec<u8>> {
    let big = avatar_url.replace("size=128", "size=256");
    // A local picture (the preview's sample faces) is fetched from this same server.
    let big = if big.starts_with('/') { format!("{}{big}", ctx.cfg.public_url.trim_end_matches('/')) } else { big };
    let avatar = fetch_avatar(ctx, &big).await.map(|b| bot_render::data_uri(&b));
    let picture = picture.map(bot_render::data_uri);
    let svg = svg(kind, names, server, number, avatar.as_deref(), picture.as_deref());
    bot_core::tokio::task::spawn_blocking(move || bot_render::png(&svg)).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_renders_without_an_avatar() {
        let png = bot_render::png(&svg(Kind::Welcome, &["Mira"], "Antho's server", Some(1284), None, None)).unwrap();
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn a_goodbye_card_over_a_picture_renders() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../core/preview");
        let picture = bot_render::data_uri(&std::fs::read(format!("{dir}/lake.jpg")).unwrap());
        let s = svg(Kind::Goodbye, &["Mira"], "Antho's server", None, None, Some(&picture));
        assert!(s.contains("Goodbye, Mira") && s.contains("url(#shade)") && !s.contains("url(#g1)"));
        let png = bot_render::png(&s).unwrap();
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn names_are_cleaned_and_escaped() {
        assert_eq!(drawable("Mira \u{1f389}").as_deref(), Some("Mira"));
        assert_eq!(drawable("\u{1f389}"), None);
        assert_eq!(drawable("Nguyễn Thị  Minh").as_deref(), Some("Nguyễn Thị Minh"));
        assert_eq!(drawable("Анастасия").as_deref(), Some("Анастасия"));
        let s = svg(Kind::Welcome, &["a<b>&"], "x", None, None, None);
        assert!(s.contains("a&lt;b&gt;&amp;"));
        assert!(!s.contains("Member #"));
    }

    #[test]
    fn names_geist_cant_draw_fall_back_to_the_username() {
        // Greek has a few letters in Geist (λ, Ω), which would leave half a name.
        for name in ["Αλέξανδρος", "محمد", "さくら", "Mira さくら", "דוד"] {
            assert_eq!(drawable(name), None, "{name}");
        }
        assert!(svg(Kind::Welcome, &["さくら", "sakura_09"], "Antho's server", None, None, None).contains("Welcome, sakura_09"));
        assert!(svg(Kind::Welcome, &["さくら", "\u{1f338}"], "Antho's server", None, None, None).contains("Welcome, new member"));
        // A server name it can't draw leaves the line empty.
        assert!(!svg(Kind::Welcome, &["Mira"], "Ελληνικός σέρβερ", None, None, None).contains('Ε'));
    }

    #[test]
    fn long_names_fit_the_card() {
        for name in ["WWWWWWWWWWWWWWWWWWWWWWWWWWWWWW", "Bartholomew Alexander Fitzgerald III", "ЩЩЩЩЩЩЩЩЩЩЩЩЩЩЩЩЩЩЩЩЩЩЩЩЩЩЩЩ"] {
            let title = format!("Welcome, {name}");
            let size = (TEXT_W / em_width(&title)).clamp(30.0, 56.0);
            let shown = fit(&title, size, TEXT_W);
            assert!(em_width(&shown) * size <= TEXT_W, "{shown}");
            assert!(shown.starts_with("Welcome, "));
        }
        assert_eq!(fit("Welcome, Mira", 56.0, TEXT_W), "Welcome, Mira");
    }
}
