//! The overview's banner: the realm as a scene with your patron's emblem in it, then who you're
//! sworn to, your name, the title you wear, level and Power. Served from a signed address so Discord loads it straight away and a
//! click never waits on drawing it; the same address always draws the same picture, so it's
//! cached for good.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use bot_core::axum::Router;
use bot_core::axum::extract::{Path, State};
use bot_core::axum::http::{StatusCode, header};
use bot_core::axum::response::{IntoResponse, Response};
use bot_core::axum::routing::get;
use bot_core::{Ctx, tokio};
use sha2::{Digest, Sha256};

use crate::data::God;

pub fn routes() -> Router<Ctx> {
    Router::new().route("/p/rpg/banner/{spec}", get(serve))
}

/// What a banner shows.
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub patron: God,
    pub name: String,
    pub level: u32,
    pub power: i64,
    /// XP towards the next level, in tenths (0 to 10).
    pub xp10: u32,
    /// A title the player wears, or empty.
    pub title: String,
}

fn sign(secret: &str, body: &str) -> String {
    let mut h = Sha256::new();
    h.update(secret.as_bytes());
    h.update(b"|rpg-banner-2|");
    h.update(body.as_bytes());
    hex::encode(&h.finalize()[..8])
}

impl Spec {
    fn body(&self) -> String {
        format!(
            "{}.{}.{}.{}.{}.{}",
            self.patron.id(),
            self.level,
            self.power,
            self.xp10,
            hex::encode(self.name.as_bytes()),
            hex::encode(self.title.as_bytes())
        )
    }

    /// The banner's address, signed so nobody can make the bot draw other words.
    pub fn url(&self, ctx: &Ctx) -> String {
        let body = self.body();
        format!(
            "{}/p/rpg/banner/{body}.{}.png",
            ctx.cfg.public_url.trim_end_matches('/'),
            sign(&ctx.cfg.discord_client_secret, &body)
        )
    }

    fn parse(ctx: &Ctx, spec: &str) -> Option<Spec> {
        let spec = spec.strip_suffix(".png")?;
        let (body, sig) = spec.rsplit_once('.')?;
        if sign(&ctx.cfg.discord_client_secret, body) != sig {
            return None;
        }
        let mut p = body.split('.');
        let patron = God::from_id(p.next()?)?;
        let level = p.next()?.parse().ok()?;
        let power = p.next()?.parse().ok()?;
        let xp10 = p.next()?.parse().ok()?;
        let name = String::from_utf8(hex::decode(p.next()?).ok()?).ok()?;
        let title = String::from_utf8(hex::decode(p.next().unwrap_or("")).ok()?).ok()?;
        Some(Spec {
            patron,
            name,
            level,
            power,
            xp10,
            title,
        })
    }
}

fn cache() -> &'static Mutex<HashMap<String, Vec<u8>>> {
    static C: OnceLock<Mutex<HashMap<String, Vec<u8>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn serve(State(ctx): State<Ctx>, Path(spec): Path<String>) -> Response {
    let Some(s) = Spec::parse(&ctx, &spec) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let hit = cache().lock().unwrap().get(&spec).cloned();
    let png = match hit {
        Some(p) => p,
        None => {
            let svg = svg(&s);
            match tokio::task::spawn_blocking(move || bot_render::png(&svg)).await {
                Ok(Ok(p)) => {
                    let mut c = cache().lock().unwrap();
                    if c.len() > 300 {
                        c.clear();
                    }
                    c.insert(spec, p.clone());
                    p
                }
                _ => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
    };
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        png,
    )
        .into_response()
}

/// The scene per patron, drawn once by `assets/banner/gen.py`: the realm, the light and the
/// patron's emblem on the right, with the gradients the words below use (`#bar`).
fn scene(g: God) -> &'static str {
    match g {
        God::Thor => include_str!("../assets/banner/thor.svg"),
        God::Athena => include_str!("../assets/banner/athena.svg"),
        God::Hades => include_str!("../assets/banner/hades.svg"),
    }
}

/// The patron's colour: the eyebrow, the title mark and the hub's container edge.
pub fn accent(g: God) -> u32 {
    match g {
        God::Thor => 0x9CC2FF,
        God::Athena => 0xCDBBFF,
        God::Hades => 0xF07A6A,
    }
}

/// Ink for the name, and a softer one for the Level line.
fn inks(g: God) -> (&'static str, &'static str) {
    match g {
        God::Thor => ("#F1F4FA", "#C3CDE0"),
        God::Athena => ("#F3F0FA", "#C9C2DE"),
        God::Hades => ("#F6EEE9", "#CDBDB6"),
    }
}

/// The banner at 932 x 264 (twice its shown size). The top line always says who you're sworn
/// to; a title you wear gets its own line under your name instead of replacing it.
pub fn svg(s: &Spec) -> String {
    let esc = bot_render::escape;
    let accent = format!("#{:06X}", accent(s.patron));
    let (ink, sub) = inks(s.patron);
    let name = esc(&crate::ui::truncate(&s.name, 22));
    let eyebrow = format!("MIDGARD · SWORN TO {}", s.patron.name().to_uppercase());
    let level = s.level;
    let power = s.power;
    let stats = format!(
        r##"Level <tspan font-weight="600" fill="{ink}">{level}</tspan>  ·  <tspan font-weight="600" fill="{ink}">{power}</tspan> Power"##
    );
    let (title, stats_y, bar_y) = if s.title.is_empty() {
        (String::new(), 166, 190)
    } else {
        let t = esc(&crate::ui::truncate(&s.title, 28));
        (
            format!(
                r##"<path d="M48,141 L55,149 L48,157 L41,149 Z" fill="{accent}"/>
<text x="66" y="158" font-family="Geist" font-size="26" fill="{ink}" fill-opacity="0.9">{t}</text>"##
            ),
            204,
            221,
        )
    };
    let xp_w = 360 * s.xp10.min(10) / 10;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="932" height="264" viewBox="0 0 932 264">
<g clip-path="url(#r)">
{scene}
<text x="40" y="60" font-family="Geist" font-weight="600" font-size="20" letter-spacing="2.4" fill="{accent}">{eyebrow}</text>
<text x="38" y="118" font-family="Geist" font-weight="600" font-size="54" letter-spacing="-0.8" fill="{ink}">{name}</text>
{title}
<text x="40" y="{stats_y}" font-family="Geist" font-size="26" fill="{sub}">{stats}</text>
<rect x="40" y="{bar_y}" width="360" height="6" rx="3" fill="#FFFFFF" fill-opacity="0.14"/>
<rect x="40" y="{bar_y}" width="{xp_w}" height="6" rx="3" fill="url(#bar)"/>
</g>
</svg>"##,
        scene = scene(s.patron),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_patron_banner_draws() {
        for g in crate::data::GODS {
            let s = Spec {
                patron: g,
                name: "Antho & <friends>".into(),
                level: 14,
                power: 312,
                xp10: 6,
                title: "Wolf-friend".into(),
            };
            let png = bot_render::png(&svg(&s)).unwrap();
            assert!(png.len() > 1000);
            assert!(svg(&s).contains(&format!("SWORN TO {}", g.name().to_uppercase())));
            let plain = Spec {
                title: String::new(),
                ..s
            };
            assert!(bot_render::png(&svg(&plain)).unwrap().len() > 1000);
        }
    }
}
