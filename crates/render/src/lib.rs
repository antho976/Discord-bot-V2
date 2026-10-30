//! Draw images from SVG: welcome cards, the weekly stream schedule. Fonts are built in (Geist
//! and Geist Mono), so the output looks the same on any host.
//!
//! Write the image as an SVG string with `font-family="Geist"` or `"Geist Mono"`, then call
//! [`png`]. Remote images (avatars) must be embedded first: fetch the bytes and use
//! [`data_uri`] in an `<image href="...">`.

use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use resvg::{tiny_skia, usvg};

static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();

fn fonts() -> Arc<usvg::fontdb::Database> {
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_font_data(include_bytes!("../fonts/Geist-Regular.ttf").to_vec());
            db.load_font_data(include_bytes!("../fonts/Geist-SemiBold.ttf").to_vec());
            db.load_font_data(include_bytes!("../fonts/GeistMono-Medium.ttf").to_vec());
            db.set_sans_serif_family("Geist");
            db.set_monospace_family("Geist Mono");
            Arc::new(db)
        })
        .clone()
}

/// Render an SVG document to PNG bytes at its own width and height. CPU-bound: call it from
/// `spawn_blocking`.
pub fn png(svg: &str) -> Result<Vec<u8>> {
    let opt = usvg::Options { font_family: "Geist".into(), fontdb: fonts(), ..Default::default() };
    let tree = usvg::Tree::from_str(svg, &opt).context("parsing the SVG")?;
    let size = tree.size().to_int_size();
    let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height()).context("image size is zero")?;
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
    pixmap.encode_png().context("encoding the PNG")
}

/// Escape text for use inside SVG/XML.
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A `data:` URI for image bytes, for `<image href>`. Sniffs PNG, JPEG, GIF and WebP.
pub fn data_uri(bytes: &[u8]) -> String {
    let mime = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF") {
        "image/gif"
    } else {
        "image/webp"
    };
    format!("data:{mime};base64,{}", base64(bytes))
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn renders_text() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="60"><rect width="200" height="60" fill="#0f0f0f"/><text x="10" y="38" font-family="Geist" font-weight="600" font-size="24" fill="#ededed">Welcome</text></svg>"##;
        let png = super::png(svg).unwrap();
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn base64_matches() {
        assert_eq!(super::base64(b"hi!"), "aGkh");
        assert_eq!(super::base64(b"hi"), "aGk=");
    }
}
