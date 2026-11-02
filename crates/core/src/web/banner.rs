//! The server's banner on the overview and at the top of the sidebar. Discord only gives
//! banners to boosted servers, so admins can upload one here; the browser resizes it before
//! sending, so a row stays a few hundred kilobytes. Without either, `ui::art` draws one from
//! the server icon.

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use rusqlite::{OptionalExtension, params};

use super::auth::User;
use super::{WebError, WebResult, saved};
use crate::Ctx;

/// Larger than the browser ever sends after resizing; a guard, not a target.
const MAX_BYTES: usize = 6 * 1024 * 1024;

pub(super) fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/banner", get(serve).post(upload))
        .route("/g/{guild}/banner/remove", post(remove))
}

/// When the server's uploaded banner last changed, for its cache-busting address.
pub fn uploaded_at(ctx: &Ctx, guild: u64) -> Option<i64> {
    ctx.db
        .with(|c| c.query_row("SELECT updated_at FROM guild_banners WHERE guild_id = ?1", [guild as i64], |r| r.get(0)).optional())
        .ok()
        .flatten()
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

/// Public, like Discord's own banners: the public pets pages show it to visitors who aren't
/// signed in. Only for servers the bot is in.
async fn serve(State(ctx): State<Ctx>, Path(guild): Path<u64>) -> WebResult {
    if !ctx.guild_ids().contains(&guild) {
        return Err(WebError::NotFound);
    }
    let row: Option<(String, Vec<u8>)> = ctx
        .db
        .with(|c| c.query_row("SELECT mime, data FROM guild_banners WHERE guild_id = ?1", [guild as i64], |r| Ok((r.get(0)?, r.get(1)?))).optional())?;
    let Some((mime, data)) = row else { return Err(WebError::NotFound) };
    // The address carries the upload time, so a new banner is a new address.
    Ok(([(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "public, max-age=31536000, immutable".to_owned())], data).into_response())
}

/// The body is the image itself (app.js sends the resized file), not a form. It's read only
/// after the access check, so someone without access can't make the bot buffer uploads.
async fn upload(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, body: Body) -> WebResult {
    if !user.admin || !ctx.guilds_for(&user).contains(&guild) {
        return Err(WebError::Forbidden);
    }
    let Ok(body) = axum::body::to_bytes(body, MAX_BYTES).await else {
        return Err(WebError::Bad("That image is too large. Pick one under 6 MB.".into()));
    };
    let Some(mime) = sniff(&body) else {
        return Err(WebError::Bad("That file isn't a PNG, JPEG, WebP or GIF image.".into()));
    };
    ctx.db.with(|c| {
        c.execute(
            "INSERT INTO guild_banners (guild_id, mime, data, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (guild_id) DO UPDATE SET mime = excluded.mime, data = excluded.data, updated_at = excluded.updated_at",
            params![guild as i64, mime, body.as_ref(), crate::now_ms()],
        )
    })?;
    ctx.audit(&user, Some(guild), "overview", "Uploaded a new server banner");
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn remove(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult<Response> {
    if !user.admin || !ctx.guilds_for(&user).contains(&guild) {
        return Err(WebError::Forbidden);
    }
    let gone = ctx.db.with(|c| c.execute("DELETE FROM guild_banners WHERE guild_id = ?1", [guild as i64]))?;
    if gone > 0 {
        ctx.audit(&user, Some(guild), "overview", "Removed the uploaded server banner");
    }
    Ok(saved(&format!("/g/{guild}")))
}

#[cfg(test)]
mod tests {
    use super::sniff;

    #[test]
    fn sniffs_images_and_refuses_the_rest() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(sniff(b"\xFF\xD8\xFF\xE0...."), Some("image/jpeg"));
        assert_eq!(sniff(b"RIFF\x10\x00\x00\x00WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff(b"GIF89a.."), Some("image/gif"));
        assert_eq!(sniff(b"<svg xmlns="), None);
        assert_eq!(sniff(b""), None);
    }
}
