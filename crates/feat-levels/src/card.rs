//! `/level rank` and `/level top` as pictures: drawn as SVG and rendered to PNG, like the
//! welcome card. Numbers are the member's own; nothing here is made up.

use std::time::Duration;

use bot_core::Ctx;
use bot_render::escape;

/// The ground, text and lines, chosen for Discord's dark theme.
const GROUND: &str = "#111117";
const INK: &str = "#f3f2f8";
const INK_2: &str = "#b5b2c4";
const INK_3: &str = "#8f8ca0";
const TRACK: &str = "#25242e";
/// Lilac, the dashboard's data colour, for a member without a coloured role.
const LILAC: u32 = 0xb99cff;

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
pub fn drawable(name: &str) -> Option<String> {
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

/// How wide `text` runs in Geist SemiBold, in ems. Outside ASCII it takes a capital's width for
/// capitals and a wide lowercase letter's otherwise, so it errs on the wide side. Geist Regular
/// is a little narrower, so this holds for it too.
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

/// A role colour bright enough to read on the dark ground: dark ones are mixed toward white
/// until they reach a lightness that holds 3:1 there. No colour, or none at all, is lilac.
pub fn accent(color: Option<u32>) -> String {
    let c = color.filter(|c| *c != 0).unwrap_or(LILAC);
    let ch = |shift: u32| ((c >> shift) & 0xff) as f64 / 255.0;
    let (mut r, mut g, mut b) = (ch(16), ch(8), ch(0));
    let lum = |r: f64, g: f64, b: f64| {
        let lin = |v: f64| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    };
    let mut steps = 0;
    while lum(r, g, b) < 0.2 && steps < 20 {
        r += (1.0 - r) * 0.12;
        g += (1.0 - g) * 0.12;
        b += (1.0 - b) * 0.12;
        steps += 1;
    }
    format!("#{:02x}{:02x}{:02x}", (r * 255.0).round() as u8, (g * 255.0).round() as u8, (b * 255.0).round() as u8)
}

/// A round face at (`cx`, `cy`): the picture when there is one, the name's initial otherwise.
fn face(id: &str, cx: f64, cy: f64, r: f64, avatar: Option<&str>, name: &str) -> String {
    match avatar {
        Some(uri) => format!(
            r##"<clipPath id="{id}"><circle cx="{cx}" cy="{cy}" r="{r}"/></clipPath><circle cx="{cx}" cy="{cy}" r="{r}" fill="#23222c"/><image href="{uri}" x="{x}" y="{y}" width="{d}" height="{d}" preserveAspectRatio="xMidYMid slice" clip-path="url(#{id})"/>"##,
            x = cx - r,
            y = cy - r,
            d = r * 2.0,
        ),
        None => {
            let initial = escape(&name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default());
            format!(
                r##"<circle cx="{cx}" cy="{cy}" r="{r}" fill="#23222c"/><text x="{cx}" y="{ty:.1}" text-anchor="middle" font-family="Geist" font-weight="600" font-size="{fs:.1}" fill="{INK_2}">{initial}</text>"##,
                ty = cy + r * 0.36,
                fs = r * 1.0,
            )
        }
    }
}

/// What the rank card shows. `prestige` and `streak` are 0 where they're off or unearned.
pub struct Rank {
    pub name: String,
    pub avatar: Option<String>,
    pub accent: String,
    pub level: u32,
    pub place: Option<i64>,
    pub xp: i64,
    pub into: i64,
    pub needed: i64,
    pub prestige: i64,
    pub streak: i64,
    pub messages: i64,
}

const RANK_W: f64 = 1000.0;
const RANK_H: f64 = 300.0;
/// The text column's edges.
const R_X0: f64 = 296.0;
const R_X1: f64 = 948.0;

/// The rank card as an SVG document.
pub fn rank_svg(r: &Rank) -> String {
    let num = bot_core::text::num;
    // Top right: the place and the level, label then figure, on the name's baseline. A member
    // with no XP has no place yet, and says so.
    let level = r.level.to_string();
    let (place, place_w) = match r.place {
        Some(p) => {
            let p = format!("#{}", num(p));
            let w = em_width("Rank") * 20.0 + 8.0 + em_width(&p) * 40.0;
            (format!(r##"<tspan font-size="20" font-weight="400" fill="{INK_3}">Rank</tspan><tspan dx="8" font-size="40" fill="{INK}">{p}</tspan>"##), w)
        }
        None => (format!(r##"<tspan font-size="20" font-weight="400" fill="{INK_3}">Unranked</tspan>"##), em_width("Unranked") * 20.0),
    };
    let corner = place_w + 32.0 + em_width("Level") * 20.0 + 8.0 + em_width(&level) * 40.0;
    let name_room = (R_X1 - R_X0 - corner - 28.0).max(160.0);
    let size = (name_room / em_width(&r.name)).clamp(30.0, 46.0);
    let name = escape(&fit(&r.name, size, name_room));
    // The line under the name: prestige and streak when there are any, then messages.
    let mut facts = Vec::new();
    if r.prestige > 0 {
        facts.push(format!("Prestige {}", r.prestige));
    }
    if r.streak > 1 {
        facts.push(format!("{}-day streak", r.streak));
    }
    facts.push(format!("{} {}", num(r.messages), if r.messages == 1 { "message" } else { "messages" }));
    let facts = escape(&fit(&facts.join("   \u{b7}   "), 24.0, R_X1 - R_X0));
    // The bar to the next level. A sliver shows as a dot rather than a squashed pill.
    let bar_w = R_X1 - R_X0;
    let share = if r.needed > 0 { (r.into as f64 / r.needed as f64).clamp(0.0, 1.0) } else { 0.0 };
    let fill_w = if share > 0.0 { (bar_w * share).max(24.0) } else { 0.0 };
    let fill = if fill_w > 0.0 { format!(r##"<rect x="{R_X0}" y="212" width="{fill_w:.1}" height="24" rx="12" fill="{a}"/>"##, a = r.accent) } else { String::new() };
    let to_next = format!("{} of {} XP to level {}", num(r.into), num(r.needed), r.level + 1);
    let portrait = face("av", 150.0, 150.0, 92.0, r.avatar.as_deref(), &r.name);
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{RANK_W}" height="{RANK_H}" viewBox="0 0 {RANK_W} {RANK_H}">
  <rect width="{RANK_W}" height="{RANK_H}" rx="28" fill="{GROUND}"/>
  <rect x="0.5" y="0.5" width="999" height="299" rx="27.5" fill="none" stroke="#ffffff" stroke-opacity="0.07"/>
  <circle cx="150" cy="150" r="101" fill="none" stroke="{a}" stroke-width="5"/>
  {portrait}
  <text x="{R_X0}" y="98" font-family="Geist" font-weight="600" font-size="{size:.1}" fill="{INK}">{name}</text>
  <text x="{R_X1}" y="98" text-anchor="end" font-family="Geist" font-weight="600">{place}<tspan dx="32" font-size="20" font-weight="400" fill="{INK_3}">Level</tspan><tspan dx="8" font-size="40" fill="{a}">{level}</tspan></text>
  <text x="{R_X0}" y="140" font-family="Geist" font-size="24" fill="{INK_2}">{facts}</text>
  <text x="{R_X0}" y="194" font-family="Geist" font-weight="600" font-size="26" fill="{INK}">{xp}<tspan dx="7" font-weight="400" font-size="22" fill="{INK_3}">XP</tspan></text>
  <text x="{R_X1}" y="194" text-anchor="end" font-family="Geist" font-size="22" fill="{INK_2}">{to_next}</text>
  <rect x="{R_X0}" y="212" width="{bar_w}" height="24" rx="12" fill="{TRACK}"/>
  {fill}
</svg>"##,
        a = r.accent,
        xp = num(r.xp),
    )
}

/// One place on the board image.
pub struct Row {
    pub place: i64,
    pub name: String,
    pub avatar: Option<String>,
    pub level: u32,
    /// Total XP, or XP gained this week on the weekly board.
    pub figure: i64,
    pub prestige: i64,
    /// The member who asked, picked out with a faint band.
    pub you: bool,
}

/// What the board image shows.
pub struct Board {
    pub title: String,
    pub week: bool,
    pub page: i64,
    pub pages: i64,
    pub rows: Vec<Row>,
}

const BOARD_W: f64 = 1000.0;
const HEAD_H: f64 = 148.0;
const ROW_H: f64 = 76.0;

/// The leaderboard page as an SVG document.
pub fn board_svg(b: &Board) -> String {
    let num = bot_core::text::num;
    let n = b.rows.len().max(1) as f64;
    let h = HEAD_H + ROW_H * n + 28.0;
    let title = escape(&fit(&b.title, 34.0, 640.0));
    let subtitle = if b.week { "XP gained since Monday" } else { "Members by XP" };
    let pages = if b.pages > 1 { format!("Page {} of {}", b.page + 1, b.pages) } else { String::new() };
    let figure_head = if b.week { "This week" } else { "XP" };
    let mut rows = String::new();
    for (i, r) in b.rows.iter().enumerate() {
        let y = HEAD_H + ROW_H * i as f64;
        let cy = y + ROW_H / 2.0;
        if r.you {
            rows.push_str(&format!(r##"<rect x="28" y="{y0:.1}" width="944" height="{hh:.1}" rx="16" fill="#ffffff" fill-opacity="0.045"/>"##, y0 = y + 4.0, hh = ROW_H - 8.0));
        } else if i > 0 && !b.rows[i - 1].you {
            rows.push_str(&format!(r##"<rect x="48" y="{y:.1}" width="904" height="1" fill="#ffffff" fill-opacity="0.07"/>"##));
        }
        let place_fill = if r.place <= 3 { INK } else { INK_3 };
        let place = num(r.place);
        let place_size = if place.chars().count() > 3 { 22.0 } else { 26.0 };
        // The name, with prestige after it in the caption grey when there is one.
        let extra = if r.prestige > 0 { format!("Prestige {}", r.prestige) } else { String::new() };
        let room = 560.0 - em_width(&extra) * 20.0 - 14.0;
        let name = escape(&fit(&r.name, 28.0, room.max(200.0)));
        let figure = if b.week { format!("+{}", num(r.figure)) } else { num(r.figure) };
        rows.push_str(&format!(
            r##"<text x="88" y="{ty:.1}" text-anchor="end" font-family="Geist" font-weight="600" font-size="{place_size}" fill="{place_fill}">{place}</text>
  {face}
  <text x="176" y="{ty:.1}" font-family="Geist" font-weight="600" font-size="28" fill="{INK}">{name}<tspan dx="14" font-weight="400" font-size="20" fill="{INK_3}">{extra}</tspan></text>
  <text x="790" y="{ty:.1}" text-anchor="end" font-family="Geist" font-size="26" fill="{INK_2}">{level}</text>
  <text x="952" y="{ty:.1}" text-anchor="end" font-family="Geist" font-weight="600" font-size="26" fill="{INK}">{figure}</text>
  "##,
            ty = cy + 9.5,
            face = face(&format!("a{i}"), 136.0, cy, 24.0, r.avatar.as_deref(), &r.name),
            level = r.level,
        ));
    }
    if b.rows.is_empty() {
        let empty = if b.week { "Nobody has earned XP since Monday." } else { "Nobody has XP yet." };
        rows.push_str(&format!(r##"<text x="48" y="{y:.1}" font-family="Geist" font-size="24" fill="{INK_2}">{empty}</text>"##, y = HEAD_H + ROW_H / 2.0 + 8.0));
    }
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{BOARD_W}" height="{h}" viewBox="0 0 {BOARD_W} {h}">
  <rect width="{BOARD_W}" height="{h}" rx="28" fill="{GROUND}"/>
  <rect x="0.5" y="0.5" width="999" height="{hb}" rx="27.5" fill="none" stroke="#ffffff" stroke-opacity="0.07"/>
  <text x="48" y="74" font-family="Geist" font-weight="600" font-size="34" fill="{INK}">{title}</text>
  <text x="48" y="108" font-family="Geist" font-size="22" fill="{INK_2}">{subtitle}</text>
  <text x="952" y="74" text-anchor="end" font-family="Geist" font-size="22" fill="{INK_3}">{pages}</text>
  <text x="790" y="{hy}" text-anchor="end" font-family="Geist" font-size="18" fill="{INK_3}">Level</text>
  <text x="952" y="{hy}" text-anchor="end" font-family="Geist" font-size="18" fill="{INK_3}">{figure_head}</text>
  <rect x="48" y="{line}" width="904" height="1" fill="#ffffff" fill-opacity="0.12"/>
  {rows}
</svg>"##,
        hb = h - 1.0,
        hy = HEAD_H - 14.0,
        line = HEAD_H - 1.0,
    )
}

/// Download an avatar as a `data:` URI, at `size` pixels. `None` when it can't be fetched in a
/// few seconds; the card then draws the initial.
pub async fn avatar(ctx: &Ctx, url: &str, size: u32) -> Option<String> {
    let url = url.replace("size=128", &format!("size={size}"));
    // A local picture (the preview's sample faces) is fetched from this same server.
    let url = if url.starts_with('/') { format!("{}{url}", ctx.cfg.public_url.trim_end_matches('/')) } else { url };
    let res = ctx.web.get(&url).timeout(Duration::from_secs(3)).send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    let bytes = res.bytes().await.ok()?;
    (bytes.len() < 4 * 1024 * 1024).then(|| bot_render::data_uri(&bytes))
}

/// Several avatars at once, in order. Whatever hasn't arrived in four seconds is left out.
pub async fn avatars(ctx: &Ctx, urls: Vec<String>, size: u32) -> Vec<Option<String>> {
    let mut set = bot_core::tokio::task::JoinSet::new();
    for (i, url) in urls.iter().cloned().enumerate() {
        let ctx = ctx.clone();
        set.spawn(async move { (i, avatar(&ctx, &url, size).await) });
    }
    let mut out = vec![None; urls.len()];
    let _ = bot_core::tokio::time::timeout(Duration::from_secs(4), async {
        while let Some(Ok((i, a))) = set.join_next().await {
            out[i] = a;
        }
    })
    .await;
    set.abort_all();
    out
}

/// Render an SVG off the async workers.
pub async fn png(svg: String) -> bot_core::anyhow::Result<Vec<u8>> {
    bot_core::tokio::task::spawn_blocking(move || bot_render::png(&svg)).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rank() -> Rank {
        Rank {
            name: "Oskar".into(),
            avatar: None,
            accent: accent(Some(0x3ddcb0)),
            level: 21,
            place: Some(3),
            xp: 24_731,
            into: 1_240,
            needed: 2_000,
            prestige: 2,
            streak: 3,
            messages: 1_177,
        }
    }

    #[test]
    fn the_rank_card_renders_and_escapes() {
        let png = bot_render::png(&rank_svg(&rank())).unwrap();
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
        let s = rank_svg(&Rank { name: "a<b>&".into(), ..rank() });
        assert!(s.contains("a&lt;b&gt;&amp;"));
        // Real numbers only: unranked members say so, and no prestige means no prestige line.
        let s = rank_svg(&Rank { place: None, prestige: 0, streak: 0, ..rank() });
        assert!(s.contains("Unranked") && !s.contains("Prestige"));
    }

    #[test]
    fn the_board_renders_full_and_empty() {
        let rows = (1..=10)
            .map(|i| Row { place: i, name: format!("Member {i}"), avatar: None, level: 30 - i as u32, figure: 50_000 / i, prestige: i % 3, you: i == 4 })
            .collect();
        let png = bot_render::png(&board_svg(&Board { title: "Antho's server".into(), week: false, page: 0, pages: 3, rows })).unwrap();
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
        let empty = board_svg(&Board { title: "x".into(), week: true, page: 0, pages: 1, rows: Vec::new() });
        assert!(empty.contains("Nobody has earned XP since Monday."));
        bot_render::png(&empty).unwrap();
    }

    #[test]
    fn dark_roles_are_lifted_and_bright_ones_kept() {
        assert_eq!(accent(None), "#b99cff");
        assert_eq!(accent(Some(0)), "#b99cff");
        assert_eq!(accent(Some(0xf1c40f)), "#f1c40f");
        // Navy is lifted until it reads on the dark ground.
        let lifted = accent(Some(0x1a1a6e));
        assert_ne!(lifted, "#1a1a6e");
    }

    #[bot_core::tokio::test(crate = "bot_core::tokio")]
    async fn faces_that_cannot_be_fetched_are_left_out() {
        let ctx = Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        let started = std::time::Instant::now();
        let got = avatars(&ctx, vec!["http://127.0.0.1:9/a.png".into(), "http://127.0.0.1:9/b.png".into()], 128).await;
        assert_eq!(got, vec![None, None]);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn long_names_fit() {
        let s = rank_svg(&Rank { name: "Bartholomew Alexander Fitzgerald the Third of Somewhere".into(), ..rank() });
        assert!(s.contains('\u{2026}'));
        assert_eq!(drawable("Mira \u{1f389}").as_deref(), Some("Mira"));
        assert_eq!(drawable("さくら"), None);
    }
}
