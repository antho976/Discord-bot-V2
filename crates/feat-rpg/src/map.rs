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
use sha2::{Digest, Sha256};

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
    fn body(&self) -> String {
        format!("{}.{}.{}", self.open, self.expedition, u8::from(self.titan))
    }
    pub fn url(&self, ctx: &Ctx) -> String {
        let b = self.body();
        format!("{}/p/rpg/map/{b}.{}.png", ctx.cfg.public_url.trim_end_matches('/'), sign(&ctx.cfg.discord_client_secret, &b))
    }
    fn parse(ctx: &Ctx, spec: &str) -> Option<Spec> {
        let spec = spec.strip_suffix(".png")?;
        let (body, sig) = spec.rsplit_once('.')?;
        if sign(&ctx.cfg.discord_client_secret, body) != sig {
            return None;
        }
        let mut p = body.split('.');
        Some(Spec { open: p.next()?.parse().ok()?, expedition: p.next()?.parse().ok()?, titan: p.next()? == "1" })
    }
}

fn cache() -> &'static Mutex<HashMap<String, Vec<u8>>> {
    static C: OnceLock<Mutex<HashMap<String, Vec<u8>>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

async fn serve(State(ctx): State<Ctx>, Path(spec): Path<String>) -> Response {
    let Some(s) = Spec::parse(&ctx, &spec) else { return StatusCode::NOT_FOUND.into_response() };
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
    ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "public, max-age=31536000, immutable")], png).into_response()
}

fn label(x: i32, y: i32, text: &str, sub: &str, open: bool) -> String {
    let fill = if open { "#F4F1EA" } else { "#8A8F99" };
    format!(
        r##"<text x="{x}" y="{y}" font-family="Geist" font-weight="600" font-size="22" fill="{fill}" text-anchor="middle">{text}</text><text x="{x}" y="{}" font-family="Geist" font-size="15" fill="#9AA6BE" text-anchor="middle">{sub}</text>"##,
        y + 20
    )
}

fn pin(x: i32, y: i32, colour: &str) -> String {
    format!(r##"<g transform="translate({x},{y})"><path d="M0,0 C-12,-16 -12,-30 0,-34 C12,-30 12,-16 0,0Z" fill="{colour}" stroke="#0E1120" stroke-width="2"/><circle cx="0" cy="-22" r="5" fill="#0E1120"/></g>"##)
}

/// The map at 932 x 520.
pub fn svg(s: &Spec) -> String {
    let open = |i: u8| s.open & (1 << i) != 0;
    let dim = |i: u8| if open(i) { "" } else { r##" opacity="0.45""## };
    let mut out = String::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="932" height="520" viewBox="0 0 932 520">
<defs>
  <linearGradient id="sea" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="#101A30"/><stop offset="1" stop-color="#18284A"/></linearGradient>
  <linearGradient id="land" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#2A2F2A"/><stop offset="1" stop-color="#1F231F"/></linearGradient>
  <clipPath id="r"><rect width="932" height="520" rx="14"/></clipPath>
</defs>
<g clip-path="url(#r)">
<rect width="932" height="520" fill="url(#sea)"/>
<path d="M210,0 C250,80 190,150 240,220 C280,280 220,350 260,420 C280,470 250,500 270,520 L932,520 L932,0 Z" fill="url(#land)"/>
"##,
    );
    // Sea waves.
    for (x, y) in [(60, 120), (110, 260), (50, 380), (140, 460), (90, 60)] {
        out.push_str(&format!(r##"<path d="M{x},{y} q12,-8 24,0 t24,0" fill="none" stroke="#2D4A7A" stroke-width="2"/>"##));
    }
    // Mountains along the top, the Loom's hall above them.
    out.push_str(r##"<path d="M330,110 L380,40 L420,95 L470,30 L530,100 L580,50 L640,110 Z" fill="#363C44"/><path d="M380,40 L392,58 L368,58 Z M470,30 L484,50 L456,50 Z M580,50 L592,66 L568,66 Z" fill="#C8D2E0" opacity="0.7"/>"##);
    // Grey Shore: the coastline strip.
    out.push_str(&format!(r##"<g{}><path d="M240,220 C280,280 220,350 260,420 L330,420 C300,350 350,290 320,220 Z" fill="#3A4448"/>"##, dim(0)));
    for (x, y) in [(270, 300), (285, 360), (300, 250)] {
        out.push_str(&format!(r##"<circle cx="{x}" cy="{y}" r="4" fill="#8FB8D8"/>"##));
    }
    out.push_str("</g>");
    // Skagi village.
    out.push_str(r##"<g transform="translate(330,170)"><path d="M-22,10 L0,-12 L22,10 Z" fill="#D2A75E"/><rect x="-16" y="10" width="32" height="18" fill="#8C6A3A"/><path d="M14,4 L28,-8 L42,4 Z" fill="#B88A48"/><rect x="18" y="4" width="20" height="14" fill="#6E5430"/></g>"##);
    out.push_str(&label(352, 228, "Skagi", "the village", true));
    // Wolfwood: trees.
    out.push_str(&format!("<g{}>", dim(1)));
    for (x, y) in [(470, 250), (500, 285), (530, 245), (555, 300), (480, 320), (520, 330), (590, 270), (450, 290), (560, 225)] {
        out.push_str(&format!(r##"<path d="M{x},{} L{},{} L{},{} Z" fill="#2F5A3A"/><path d="M{x},{} L{},{} L{},{} Z" fill="#3E7A4C"/>"##, y - 34, x - 16, y, x + 16, y, y - 46, x - 11, y - 20, x + 11, y - 20));
    }
    out.push_str("</g>");
    // Barrow Fields: mounds.
    out.push_str(&format!("<g{}>", dim(2)));
    for (x, y, w) in [(720, 300, 70), (800, 260, 55), (780, 350, 60), (860, 320, 45), (700, 230, 40)] {
        out.push_str(&format!(r##"<path d="M{},{y} Q{x},{} {},{y} Z" fill="#454B55"/><rect x="{}" y="{}" width="4" height="14" fill="#8A93A0"/>"##, x - w / 2, y - w / 2, x + w / 2, x - 2, y - w / 2 - 10));
    }
    out.push_str("</g>");
    // The Labyrinth's mouth, under the mountains.
    out.push_str(r##"<g transform="translate(470,150)"><circle r="26" fill="#1A1626" stroke="#C9B8FF" stroke-width="2"/><path d="M0,-16 A16,16 0 1,1 -16,0 A10,10 0 1,1 0,-8 A4,4 0 1,1 -4,0" fill="none" stroke="#C9B8FF" stroke-width="2.5"/></g>"##);
    out.push_str(&label(470, 205, "The Labyrinth", "under the Loom", true));
    // Dungeon doors.
    for (i, (x, y)) in [(0u8, (300, 445)), (1, (600, 345)), (2, (860, 400))] {
        if open(i) {
            out.push_str(&format!(r##"<g transform="translate({x},{y})"><path d="M-10,12 L-10,-4 A10,10 0 0,1 10,-4 L10,12 Z" fill="#1A1626" stroke="#E0C47A" stroke-width="2"/></g>"##));
        }
    }
    out.push_str(&label(288, 470, "Grey Shore", if open(0) { "nixies, herring, amber" } else { "locked" }, open(0)));
    out.push_str(&label(520, 380, "Wolfwood", if open(1) { "wolves, yew, fangs" } else { "needs 90 Power" }, open(1)));
    out.push_str(&label(780, 420, "Barrow Fields", if open(2) { "the dead, iron, frost hides" } else { "needs 150 Power" }, open(2)));
    // Where the expedition is.
    let pos = [(0, 0), (290, 330), (520, 300), (780, 300)];
    if (1..=3).contains(&s.expedition) {
        let (x, y) = pos[s.expedition as usize];
        out.push_str(&pin(x, y, "#D2A75E"));
    }
    // A Titan rising from the sea.
    if s.titan {
        out.push_str(r##"<g transform="translate(120,190)"><path d="M-40,60 C-30,0 -10,-30 0,-40 C10,-30 30,0 40,60 Z" fill="#5A1E24" opacity="0.9"/><circle cx="-10" cy="-8" r="5" fill="#F0866B"/><circle cx="10" cy="-8" r="5" fill="#F0866B"/></g>"##);
        out.push_str(r##"<text x="120" y="280" font-family="Geist" font-weight="600" font-size="18" fill="#F0866B" text-anchor="middle">A Titan rises</text>"##);
    }
    out.push_str(r##"<text x="902" y="40" font-family="Geist" font-weight="600" font-size="15" letter-spacing="3" fill="#9AA6BE" text-anchor="end">MIDGARD</text>"##);
    out.push_str("</g></svg>");
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_state_draws() {
        for open in [0u8, 1, 3, 7] {
            for exp in 0..=3 {
                for titan in [false, true] {
                    let png = bot_render::png(&super::svg(&super::Spec { open, expedition: exp, titan })).unwrap();
                    assert!(png.len() > 1000);
                }
            }
        }
    }
}
