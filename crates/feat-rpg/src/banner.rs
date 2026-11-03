//! The overview's banner: the realm as a scene, your name, level and Power, and your patron's
//! emblem on the side. Served from a signed address so Discord loads it straight away and a
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
    h.update(b"|rpg-banner|");
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

/// The scene per patron: sky top and bottom, ridge colours, emblem colour.
fn palette(g: God) -> (&'static str, &'static str, [&'static str; 3], &'static str) {
    match g {
        God::Thor => (
            "#2A4170",
            "#121A30",
            ["#1D2C52", "#152040", "#0C1122"],
            "#E4EEFF",
        ),
        God::Athena => (
            "#3B3462",
            "#17142A",
            ["#2A2550", "#1E1A3C", "#100E1E"],
            "#EEE8FF",
        ),
        God::Hades => (
            "#1D4A47",
            "#0D1A1E",
            ["#173A3A", "#10292B", "#081314"],
            "#DDFBF2",
        ),
    }
}

fn emblem(g: God) -> &'static str {
    crate::emoji::ICONS
        .iter()
        .find(|i| i.0 == g.id())
        .map(|i| i.2)
        .unwrap_or("")
}

/// The banner at 932 x 264 (twice its shown size).
pub fn svg(s: &Spec) -> String {
    let (top, bottom, ridges, glow) = palette(s.patron);
    let esc = bot_render::escape;
    let name = esc(&crate::ui::truncate(&s.name, 22));
    let eyebrow = if s.title.is_empty() {
        format!("MIDGARD · SWORN TO {}", s.patron.name().to_uppercase())
    } else {
        format!(
            "MIDGARD · {}",
            esc(&crate::ui::truncate(&s.title, 28)).to_uppercase()
        )
    };
    let xp_w = 400 * s.xp10.min(10) / 10;
    let accent = match s.patron {
        God::Thor => "#9CC2FF",
        God::Athena => "#C9B8FF",
        God::Hades => "#8FD6C1",
    };
    let extra = match s.patron {
        // A lightning bolt striking the hammer.
        God::Thor => r##"<g fill="none" stroke-linejoin="round" stroke-linecap="round"><polyline points="856,0 820,40 842,42 800,80" stroke="#9CC2FF" stroke-width="14" stroke-opacity="0.12"/><polyline points="856,0 820,40 842,42 800,80" stroke="#9CC2FF" stroke-width="8" stroke-opacity="0.25"/><polyline points="856,0 820,40 842,42 800,80" stroke="#E8F0FF" stroke-width="3.5"/></g>"##.to_string(),
        // Stars over the owl's hill.
        God::Athena => (0..14)
            .map(|i| format!(r##"<circle cx="{}" cy="{}" r="{}" fill="#EEE8FF" opacity="{}"/>"##, 420 + (i * 37) % 480, 18 + (i * 23) % 90, 1.5 + (i % 3) as f32 * 0.6, 0.35 + (i % 4) as f32 * 0.15))
            .collect(),
        // The river Styx, faint mist.
        God::Hades => r##"<path d="M0,236 C150,222 300,250 466,236 S780,222 932,238" fill="none" stroke="#8FD6C1" stroke-width="3" opacity="0.35"/>"##.to_string(),
    };
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="932" height="264" viewBox="0 0 932 264">
<defs>
  <linearGradient id="sky" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{top}"/><stop offset="1" stop-color="{bottom}"/></linearGradient>
  <radialGradient id="halo" cx="0.82" cy="0.3" r="0.55"><stop offset="0" stop-color="{accent}" stop-opacity="0.35"/><stop offset="1" stop-color="{accent}" stop-opacity="0"/></radialGradient>
  <linearGradient id="shade" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="#0B1020" stop-opacity="0.7"/><stop offset="0.55" stop-color="#0B1020" stop-opacity="0"/></linearGradient>
  <clipPath id="r"><rect width="932" height="264" rx="12"/></clipPath>
</defs>
<g clip-path="url(#r)">
  <rect width="932" height="264" fill="url(#sky)"/>
  <rect width="932" height="264" fill="url(#halo)"/>
  {extra}
  <polygon points="0,228 100,180 200,212 320,152 440,204 580,136 700,192 820,156 932,184 932,264 0,264" fill="{r0}"/>
  <polygon points="0,244 140,208 260,236 400,192 540,232 680,200 800,228 932,208 932,264 0,264" fill="{r1}"/>
  <polygon points="0,256 180,236 360,252 520,228 700,248 932,232 932,264 0,264" fill="{r2}"/>
  <rect width="932" height="264" fill="url(#shade)"/>
  <g transform="translate(652,44) rotate(-24 104 104) scale(8.6)" fill="none" stroke-linecap="round" stroke-linejoin="round">
    <path d="{emblem}" stroke="{accent}" stroke-width="5" stroke-opacity="0.08"/>
    <path d="{emblem}" stroke="{accent}" stroke-width="3.4" stroke-opacity="0.14"/>
    <path d="{emblem}" stroke="{accent}" stroke-width="2.4" stroke-opacity="0.25"/>
    <path d="{emblem}" stroke="{glow}" stroke-width="1.5"/>
  </g>
  <text x="36" y="62" font-family="Geist" font-weight="600" font-size="22" letter-spacing="2.6" fill="{accent}">{eyebrow}</text>
  <text x="36" y="118" font-family="Geist" font-weight="600" font-size="56" fill="#F4F1EA">{name}</text>
  <text x="36" y="164" font-family="Geist" font-size="28" fill="#C9D3E6">Level <tspan font-weight="600" fill="#F4F1EA">{level}</tspan>   ·   <tspan font-weight="600" fill="#F4F1EA">{power}</tspan> Power</text>
  <rect x="36" y="188" width="400" height="8" rx="4" fill="#FFFFFF" fill-opacity="0.16"/>
  <rect x="36" y="188" width="{xp_w}" height="8" rx="4" fill="{accent}"/>
</g>
</svg>"##,
        r0 = ridges[0],
        r1 = ridges[1],
        r2 = ridges[2],
        emblem = emblem(s.patron),
        level = s.level,
        power = s.power,
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
        }
    }
}
