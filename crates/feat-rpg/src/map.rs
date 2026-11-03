//! The map of Midgard: a drawn picture of the places, what's open to you, where your
//! expedition is, and whether a Titan is up. Served from a signed address like the banner.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use bot_core::axum::Router;
use bot_core::axum::extract::{Path, State};
use bot_core::axum::http::{StatusCode, header};
use bot_core::axum::response::{IntoResponse, Response};
use bot_core::axum::routing::get;
use bot_core::{Ctx, tokio};

use crate::data;
use sha2::{Digest, Sha256};

/// Bumped whenever the drawing changes.
const REVISION: u8 = 2;

pub fn routes() -> Router<Ctx> {
    Router::new().route("/p/rpg/map/{spec}", get(serve))
}

/// What the map shows: which regions are open (bits), where the expedition is (0 none, 1-3),
/// and whether a Titan is up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spec {
    pub open: u8,
    pub expedition: u8,
    pub titan: bool,
}

fn sign(secret: &str, body: &str) -> String {
    let mut h = Sha256::new();
    h.update(secret.as_bytes());
    h.update(b"|rpg-map|");
    h.update(body.as_bytes());
    hex::encode(&h.finalize()[..6])
}

impl Spec {
    /// The last part is the drawing's revision, so a redrawn map gets a new address and
    /// Discord's image cache doesn't keep showing the old one. Older addresses still parse.
    fn body(&self) -> String {
        format!(
            "{}.{}.{}.{REVISION}",
            self.open,
            self.expedition,
            u8::from(self.titan)
        )
    }
    pub fn url(&self, ctx: &Ctx) -> String {
        let b = self.body();
        format!(
            "{}/p/rpg/map/{b}.{}.png",
            ctx.cfg.public_url.trim_end_matches('/'),
            sign(&ctx.cfg.discord_client_secret, &b)
        )
    }
    fn parse(ctx: &Ctx, spec: &str) -> Option<Spec> {
        let spec = spec.strip_suffix(".png")?;
        let (body, sig) = spec.rsplit_once('.')?;
        if sign(&ctx.cfg.discord_client_secret, body) != sig {
            return None;
        }
        let mut p = body.split('.');
        Some(Spec {
            open: p.next()?.parse().ok()?,
            expedition: p.next()?.parse().ok()?,
            titan: p.next()? == "1",
        })
    }
}

fn cache() -> &'static Mutex<HashMap<String, Vec<u8>>> {
    static C: OnceLock<Mutex<HashMap<String, Vec<u8>>>> = OnceLock::new();
    C.get_or_init(Default::default)
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
                    cache().lock().unwrap().insert(spec, p.clone());
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

/// The drawn map without its live layer: coast, Skagi, the Loom, the Labyrinth, the river,
/// Wolfwood, the Barrow Fields and the three dungeons. Static art, drawn by hand.
const ART: &str = include_str!("map.svg");

/// Where the live layer goes in `ART`.
const LIVE: &str = "<!--live-->";

/// A place name with a dark halo so it reads over the art.
fn label(out: &mut String, x: i32, y: i32, text: &str, sub: &str, open: bool) {
    let (fill, subfill) = if open {
        ("#F4F1EA", "#AEB8CC")
    } else {
        ("#8A8F99", "#8A8F99")
    };
    let halo =
        r##"stroke="#0B0F18" stroke-width="4" stroke-linejoin="round" paint-order="stroke""##;
    out.push_str(&format!(
        r##"<text x="{x}" y="{y}" font-family="Geist" font-weight="600" font-size="19" letter-spacing="0.5" fill="{fill}" text-anchor="middle" {halo}>{text}</text>"##
    ));
    if sub.is_empty() {
        return;
    }
    let sy = y + 18;
    if open {
        out.push_str(&format!(
            r##"<text x="{x}" y="{sy}" font-family="Geist" font-size="13" fill="{subfill}" text-anchor="middle" {halo}>{sub}</text>"##
        ));
    } else {
        // A small padlock before the text; Geist at 13 px runs about 6.6 px a character.
        let w = sub.chars().count() as i32 * 66 / 10;
        let lx = x - w / 2 - 9;
        out.push_str(&format!(
            r##"<g transform="translate({lx},{})" fill="none" stroke="{subfill}" stroke-width="1.4"><rect x="-4.5" y="-1" width="9" height="7" rx="1.5" fill="{subfill}" stroke="none"/><path d="M-2.6,-1 v-2.2 a2.6,2.6 0 0 1 5.2,0 v2.2"/></g>"##,
            sy - 7
        ));
        out.push_str(&format!(
            r##"<text x="{}" y="{sy}" font-family="Geist" font-size="13" fill="{subfill}" text-anchor="middle" {halo}>{sub}</text>"##,
            x + 6
        ));
    }
}

/// A small name for a landmark.
fn note(out: &mut String, x: i32, y: i32, text: &str) {
    out.push_str(&format!(
        r##"<text x="{x}" y="{y}" font-family="Geist" font-size="11" fill="#8C97AC" text-anchor="middle" stroke="#0B0F18" stroke-width="3" stroke-linejoin="round" paint-order="stroke">{text}</text>"##
    ));
}

/// Where your expedition is camped.
fn pin(out: &mut String, x: i32, y: i32) {
    out.push_str(&format!(
        r##"<g transform="translate({x},{y})"><ellipse cx="0" cy="1" rx="9" ry="3.5" fill="#000" opacity="0.45"/><circle cx="0" cy="-20" r="16" fill="url(#gold)"/><path d="M0,0 C-10,-13 -11,-26 0,-31 C11,-26 10,-13 0,0Z" fill="#D2A75E" stroke="#0B0F18" stroke-width="2"/><circle cx="0" cy="-20" r="4.5" fill="#0B0F18"/><text x="0" y="-38" font-family="Geist" font-weight="600" font-size="12" fill="#E0C47A" text-anchor="middle" stroke="#0B0F18" stroke-width="3" stroke-linejoin="round" paint-order="stroke">Your expedition</text></g>"##
    ));
}

/// A Titan wading out of the western sea.
const TITAN: &str = r##"<g transform="translate(112,236)">
<ellipse cx="0" cy="40" rx="78" ry="16" fill="#F0866B" opacity="0.10" filter="url(#soft)"/>
<path d="M-64,44 C-60,8 -40,-6 -22,-10 C-18,-30 -14,-46 0,-50 C14,-46 18,-30 22,-10 C40,-6 60,8 64,44 Z" fill="#2A1216"/>
<path d="M-64,44 C-60,8 -40,-6 -22,-10 C-18,-30 -14,-46 0,-50 C14,-46 18,-30 22,-10 C40,-6 60,8 64,44" fill="none" stroke="#F0866B" stroke-width="1.6" opacity="0.7"/>
<path d="M-12,-44 l-10,-14 l14,8 M12,-44 l10,-14 l-14,8" fill="#2A1216" stroke="#F0866B" stroke-width="1.2" stroke-linejoin="round" opacity="0.9"/>
<circle cx="-6" cy="-30" r="3" fill="#FFB49C"/><circle cx="6" cy="-30" r="3" fill="#FFB49C"/>
<circle cx="-6" cy="-30" r="8" fill="#F0866B" opacity="0.25"/><circle cx="6" cy="-30" r="8" fill="#F0866B" opacity="0.25"/>
<path d="M-40,8 l-4,12 M40,8 l4,12 M-24,0 l-2,8" stroke="#F0866B" stroke-width="1.2" stroke-linecap="round" opacity="0.5"/>
<path d="M-84,44 q10,-8 20,0 t20,0 t20,0 t20,0 t20,0 t20,0 t20,0 t20,0" fill="none" stroke="#8FB8D8" stroke-width="2" stroke-linecap="round" opacity="0.6"/>
<path d="M-70,50 q8,-5 16,0 t16,0 t16,0 t16,0 t16,0 t16,0 t16,0 t16,0" fill="none" stroke="#35588A" stroke-width="1.5" stroke-linecap="round"/>
<text x="0" y="76" font-family="Geist" font-weight="600" font-size="16" fill="#F0866B" text-anchor="middle" stroke="#0B0F18" stroke-width="4" stroke-linejoin="round" paint-order="stroke">A Titan rises</text>
</g>"##;

/// The map at 932 x 520.
pub fn svg(s: &Spec) -> String {
    let open = |i: u8| s.open & (1 << i) != 0;
    let mut live = String::new();
    // Fog over the regions you can't reach yet.
    for (i, id) in ["shore", "wolfwood", "barrow"].into_iter().enumerate() {
        if !open(i as u8) {
            live.push_str(&format!(
                r##"<use href="#fog-{id}" fill="#080B12" opacity="0.62" filter="url(#soft)"/>"##
            ));
        }
    }
    note(&mut live, 522, 90, "the Loom");
    note(&mut live, 150, 460, "the Drowned Hall");
    note(&mut live, 606, 432, "the Wolf-King's Den");
    note(&mut live, 820, 480, "Hrolf's Treasury");
    label(&mut live, 340, 272, "Skagi", "the village", true);
    label(&mut live, 472, 204, "The Labyrinth", "under the Loom", true);
    let place = [
        (0u8, 268, 404, "nixies, herring, amber"),
        (1, 520, 328, "wolves, yew, fangs"),
        (2, 790, 352, "the dead, iron, frost hides"),
    ];
    for (i, x, y, what) in place {
        let r = &data::REGIONS[i as usize];
        let sub = if open(i) {
            what.to_owned()
        } else {
            format!("needs {} Power", r.power)
        };
        label(&mut live, x, y, r.name, &sub, open(i));
    }
    let camp = [(0, 0), (246, 372), (566, 306), (790, 318)];
    if (1..=3).contains(&s.expedition) {
        let (x, y) = camp[s.expedition as usize];
        pin(&mut live, x, y);
    }
    if s.titan {
        live.push_str(TITAN);
    }
    ART.replacen(LIVE, &live, 1)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_live_layer_lands_in_the_art() {
        assert_eq!(super::ART.matches(super::LIVE).count(), 1);
        let all = super::svg(&super::Spec {
            open: 7,
            expedition: 2,
            titan: true,
        });
        assert!(!all.contains(super::LIVE));
        assert!(all.contains("Your expedition") && all.contains("A Titan rises"));
        let none = super::svg(&super::Spec {
            open: 1,
            expedition: 0,
            titan: false,
        });
        assert_eq!(
            none.matches("fog-").count() - all.matches("fog-").count(),
            2
        );
        assert!(none.contains("needs 90 Power") && none.contains("needs 150 Power"));
    }

    #[test]
    fn every_state_draws() {
        // Each lock, each camp and the Titan, without rendering all 64 states.
        for (open, expedition, titan) in
            [(0u8, 0, false), (1, 1, false), (3, 2, true), (7, 3, true)]
        {
            let png = bot_render::png(&super::svg(&super::Spec {
                open,
                expedition,
                titan,
            }))
            .unwrap();
            assert!(png.len() > 1000);
        }
    }
}
