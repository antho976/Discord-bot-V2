//! Pet pictures: checking what staff upload, and serving stored pictures to every page.
//!
//! The browser shrinks a picture before it's sent (to 256 px on its long side), so a stored
//! one is a few kilobytes. It arrives as a `data:` address in an ordinary form field, so the
//! form posts like any other.

use bot_core::Ctx;
use bot_core::axum::Router;
use bot_core::axum::extract::{Path, State};
use bot_core::axum::http::header;
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::get;
use bot_core::web::{WebError, WebResult};

use crate::{KEY, store};

/// Larger than the browser ever sends after shrinking (a GIF is sent as is); a guard.
pub const MAX_BYTES: usize = 1024 * 1024;

pub fn routes() -> Router<Ctx> {
    Router::new().route("/p/{guild}/pet-pictures/{id}", get(serve))
}

/// Public like the collection page: the address carries when it changed, so it caches for good.
async fn serve(State(ctx): State<Ctx>, Path((guild, id)): Path<(u64, String)>) -> WebResult {
    if !ctx.guild_ids().contains(&guild) || !ctx.enabled(guild, KEY) {
        return Err(WebError::NotFound);
    }
    let Some((mime, data)) = store::picture(&ctx, guild, &id)? else { return Err(WebError::NotFound) };
    Ok(([(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "public, max-age=31536000, immutable".to_owned())], data).into_response())
}

/// What kind of image these bytes are, from their first bytes. Anything else is refused.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        _ => None,
    }
}

/// Bytes the dashboard will store, with their type, or why not.
pub fn check(bytes: &[u8]) -> Result<&'static str, String> {
    if bytes.len() > MAX_BYTES {
        return Err("That picture is too large. Pick one under 1 MB.".into());
    }
    sniff(bytes).ok_or_else(|| "That file isn't a PNG, JPEG, WebP or GIF image.".into())
}

/// A picture sent from the form: a `data:image/...;base64,` address. Returns its type and bytes.
pub fn from_data_url(url: &str) -> Result<(&'static str, Vec<u8>), String> {
    let bad = || "The picture didn't come through. Pick it again.".to_owned();
    let rest = url.strip_prefix("data:").ok_or_else(bad)?;
    let (head, body) = rest.split_once(',').ok_or_else(bad)?;
    if !head.ends_with(";base64") {
        return Err(bad());
    }
    // Base64 is a third larger than the bytes; refuse early rather than decode a huge field.
    if body.len() > MAX_BYTES / 3 * 4 + 8 {
        return Err("That picture is too large. Pick one under 1 MB.".into());
    }
    let bytes = base64(body).ok_or_else(bad)?;
    let mime = check(&bytes)?;
    Ok((mime, bytes))
}

/// Standard or URL-safe base64, padding optional, whitespace ignored.
fn base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_images_and_refuses_the_rest() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(sniff(b"\xFF\xD8\xFF\xE0...."), Some("image/jpeg"));
        assert_eq!(sniff(b"RIFF\x10\x00\x00\x00WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff(b"GIF89a.."), Some("image/gif"));
        assert_eq!(sniff(b"<svg xmlns="), None);
        assert_eq!(sniff(b""), None);
        assert!(check(&[0x89; MAX_BYTES + 1]).is_err());
    }

    #[test]
    fn decodes_base64() {
        assert_eq!(base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64("aGVsbG8").unwrap(), b"hello");
        assert_eq!(base64("aGVs\nbG8gd29ybGQ=").unwrap(), b"hello world");
        assert_eq!(base64("+/+/").unwrap(), base64("-_-_").unwrap());
        assert!(base64("a*b").is_none());
    }

    #[test]
    fn reads_data_urls() {
        // "\x89PNG\r\n\x1a\n" then a byte.
        let (mime, bytes) = from_data_url("data:image/png;base64,iVBORw0KGgoA").unwrap();
        assert_eq!((mime, bytes.len()), ("image/png", 9));
        assert!(from_data_url("data:image/png;base64,PHN2Zz4=").is_err(), "an svg named png is refused");
        assert!(from_data_url("https://example.com/a.png").is_err());
        assert!(from_data_url("data:image/png,iVBORw0KGgo").is_err());
    }
}
