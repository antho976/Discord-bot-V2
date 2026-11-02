//! idleontoolbox.com's list of Idleon pets, with their pictures: which pets a server's catalog
//! is missing, and pictures for the pets it has without one.
//!
//! The site is a Next.js app that ships its game data inside its scripts; the pets page itself
//! is drawn in the browser. So the bot reads, from idleontoolbox.com only:
//! - the pets page, for the list of scripts it loads;
//! - the script with the pet list, `JSON.parse('[{"name":"King_Doot","rawName":"babaMummy",...`
//!   (name, sprite name, in-game bonus, its Pet+ bonus, how rare a drop it is, its power);
//! - the script with `"companionGroups"`, the in-game sets (Legacy Pets, Fallen Spirits, ...).
//!   Pets in no set are ones that aren't in the game, so they're left out;
//! - each picture at `/monsters/<rawName>/static.png`, where the site draws it from, or
//!   `/afk_targets/<Name>.png` when that's missing.
//!
//! The site has no rarity, so the bot works it out from the game's data (see [`rarities`]).
//!
//! The list is kept for an hour and pictures until the bot restarts, so a check followed by an
//! add downloads each picture once. A picture that couldn't be read (a timeout, the site's own
//! error) isn't kept, so the next check tries it again. Adding stores the picture like an
//! upload: nothing hotlinks.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use bot_core::axum::Router;
use bot_core::axum::extract::{Path, State};
use bot_core::axum::http::header;
use bot_core::axum::response::{IntoResponse, Redirect};
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::reqwest;
use bot_core::web::ui::{self, Icon};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult};
use bot_core::{Ctx, serde_json, text};
use serde::Deserialize;

use crate::store::{self, Pet};
use crate::{KEY, picture, view};

const SITE: &str = "https://idleontoolbox.com";
const HOST: &str = "idleontoolbox.com";
const PAGE: &str = "/account/prem-currency/pets";
const USER_AGENT: &str = "antho-bot/2 (Discord bot; pet catalog check, run by hand from its dashboard)";
/// The list is read again after this long.
const KEEP: Duration = Duration::from_secs(3600);
/// A whole check, scripts and pictures, gives up after this long.
const DEADLINE: Duration = Duration::from_secs(45);
const PARALLEL: usize = 6;
const CANT_FIND: &str = "idleontoolbox.com answered, but its pet list wasn't where the bot looks for it. The site may have changed how it ships its data.";

/// A pet as idleontoolbox.com lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct SitePet {
    /// "Santas Little Helper"
    pub name: String,
    /// The game's sprite name, "w6c2b": where its picture is.
    pub raw: String,
    /// The in-game set, "Legacy Pets".
    pub group: String,
    /// "+5% All Skill Efficiency"
    pub bonus: String,
    /// What its Pet+ gives instead, "+8% All Skill Efficiency". Empty when it has no Pet+.
    pub plus_bonus: String,
    /// One of [`store::RARITIES`].
    pub rarity: &'static str,
}

#[derive(Clone)]
struct Pic {
    mime: &'static str,
    data: Arc<Vec<u8>>,
}

#[derive(Default)]
struct Cache {
    list: Option<(Instant, i64, Arc<Vec<SitePet>>)>,
    /// By sprite name. `None`: the site has no usable picture, for sure. A picture that
    /// couldn't be read isn't here at all.
    pics: HashMap<String, Option<Pic>>,
    /// The last check's failure per server, shown until the next check.
    errors: HashMap<u64, String>,
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Mutex::default);

fn cache() -> std::sync::MutexGuard<'static, Cache> {
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/pets/catalog/idleon", post(check))
        .route("/g/{guild}/pets/catalog/idleon/add", post(add))
        .route("/g/{guild}/pets/catalog/idleon/pic/{raw}", get(candidate_picture))
}

fn open(ctx: &Ctx, user: &User, guild: u64) -> WebResult<Page> {
    let page = ctx.page(user, guild, KEY, Level::Edit)?;
    store::ensure_seeded(ctx, guild)?;
    Ok(page)
}

// ---- reading the site ----

fn client() -> Result<reqwest::Client, String> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(c) = CLIENT.get() {
        return Ok(c.clone());
    }
    let c = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(8))
        // Only ever idleontoolbox.com, even when it redirects.
        .redirect(reqwest::redirect::Policy::custom(|a| if a.url().host_str() == Some(HOST) && a.previous().len() < 5 { a.follow() } else { a.stop() }))
        .build()
        .map_err(|e| format!("The bot couldn't set up the request ({e})."))?;
    Ok(CLIENT.get_or_init(|| c).clone())
}

fn why(e: reqwest::Error) -> String {
    if e.is_timeout() {
        "idleontoolbox.com didn't answer in time. Try again in a minute.".into()
    } else if e.is_connect() {
        "The bot couldn't connect to idleontoolbox.com. Check that the machine it runs on is online.".into()
    } else {
        format!("The bot couldn't read idleontoolbox.com ({}).", e.without_url())
    }
}

/// A GET that didn't bring the file back: what to tell staff, and whether the site said for
/// sure there's nothing usable there (404, 410, or more than the bot accepts).
struct Fail {
    msg: String,
    definite: bool,
}

impl Fail {
    fn unsure(msg: String) -> Self {
        Self { msg, definite: false }
    }
}

impl From<Fail> for String {
    fn from(f: Fail) -> String {
        f.msg
    }
}

/// GET a path on the site, refusing anything larger than `max` bytes.
async fn fetch(client: &reqwest::Client, path: &str, max: usize) -> Result<Vec<u8>, Fail> {
    let mut res = client.get(format!("{SITE}{path}")).send().await.map_err(|e| Fail::unsure(why(e)))?;
    if res.url().host_str() != Some(HOST) {
        return Err(Fail::unsure("idleontoolbox.com sent the bot to another site, so it stopped.".into()));
    }
    let status = res.status().as_u16();
    if !res.status().is_success() {
        return Err(Fail { msg: format!("idleontoolbox.com answered {status} for {path}."), definite: matches!(status, 404 | 410) });
    }
    let too_big = || Fail { msg: format!("idleontoolbox.com sent more for {path} than the bot accepts."), definite: true };
    if res.content_length().is_some_and(|n| n > max as u64) {
        return Err(too_big());
    }
    let mut out = Vec::new();
    while let Some(part) = res.chunk().await.map_err(|e| Fail::unsure(why(e)))? {
        out.extend_from_slice(&part);
        if out.len() > max {
            return Err(too_big());
        }
    }
    Ok(out)
}

/// The scripts a page loads, as site paths. Only the site's own script folder.
fn script_paths(html: &str) -> Vec<String> {
    const DIR: &str = "/_next/static/chunks/";
    let mut out: Vec<String> = Vec::new();
    for (at, _) in html.match_indices(DIR) {
        let rest = &html[at..];
        let end = rest.find(['"', '\'', ' ', '>']).unwrap_or(rest.len());
        let path = &rest[..end];
        let file = &path[DIR.len()..];
        let ok = file.ends_with(".js") && !file.is_empty() && file.bytes().all(|b| b.is_ascii_alphanumeric() || b"._~-".contains(&b));
        if ok && !out.iter().any(|p| p == path) {
            out.push(path.to_owned());
        }
    }
    out
}

/// The text of a single-quoted JavaScript string starting at `s` (just after the quote).
fn js_string(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'v' => out.push('\u{b}'),
                '0' => out.push('\0'),
                'x' => {
                    let h: String = chars.by_ref().take(2).collect();
                    out.push(char::from_u32(u32::from_str_radix(&h, 16).ok()?)?);
                }
                'u' => {
                    let h: String = chars.by_ref().take(4).collect();
                    out.push(char::from_u32(u32::from_str_radix(&h, 16).ok()?).unwrap_or('\u{fffd}'));
                }
                '\n' => {}
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

#[derive(Debug, Default, Deserialize)]
struct RawPet {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "rawName")]
    raw: Option<String>,
    #[serde(default)]
    effect: String,
    /// The Pet+ bonus.
    #[serde(default, rename = "upgradedEffect")]
    upgraded: String,
    /// What a Pet+ costs; none for pets that can't be upgraded (tournament prizes).
    #[serde(default, rename = "upgradeCost")]
    upgrade_cost: serde_json::Value,
    /// For pets from eggs, how rare the drop is: 5 for the commonest, 2000 for King Doot. The
    /// site ships it as text.
    #[serde(default)]
    filler: serde_json::Value,
    /// The pet's power in the game's tournaments.
    #[serde(default, rename = "tourPower")]
    power: serde_json::Value,
}

/// A number the site sends as a number or as text.
fn num(v: &serde_json::Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str()?.trim().parse().ok()).filter(|n: &f64| n.is_finite())
}

impl RawPet {
    fn has_plus(&self) -> bool {
        num(&self.upgrade_cost).is_some_and(|c| c > 0.0) && !self.upgraded.trim().is_empty()
    }
}

/// The pet list, in the site's order (the sets point into it by position).
fn parse_pets(js: &str) -> Option<Vec<RawPet>> {
    const OPEN: &str = "JSON.parse('[{";
    for (at, _) in js.match_indices(OPEN) {
        let start = at + OPEN.len() - 2;
        // A cut inside a character (é, or the U+FFFD of a lossy read) would panic.
        let head = &js[start..js.floor_char_boundary(start + 400)];
        if !head.contains("\"rawName\"") || !head.contains("\"effect\"") {
            continue;
        }
        let Some(json) = js_string(&js[start..]) else { continue };
        if let Ok(list) = serde_json::from_str::<Vec<RawPet>>(&json)
            && list.len() >= 20
            && list.iter().take(5).all(|p| p.name.is_some() && p.raw.is_some())
        {
            return Some(list);
        }
    }
    None
}

#[derive(Debug, Deserialize)]
struct Group {
    name: String,
    indices: Vec<usize>,
}

/// The in-game sets: `"companionGroups":[{"name":"Legacy Pets","indices":[0,1,...]},...]`.
fn parse_groups(js: &str) -> Option<Vec<Group>> {
    const KEY: &str = "\"companionGroups\":";
    let at = js.find(KEY)? + KEY.len();
    let rest = &js[at..];
    let first = serde_json::Deserializer::from_str(rest).into_iter::<Vec<Group>>().next();
    match first {
        Some(Ok(groups)) if !groups.is_empty() => Some(groups),
        // Inside a JavaScript string the quotes may be escaped; read the string's text instead.
        _ => {
            let window = &rest[..rest.floor_char_boundary(64 * 1024)];
            let text = js_string(&format!("{window}'"))?;
            serde_json::Deserializer::from_str(&text).into_iter::<Vec<Group>>().next()?.ok().filter(|g| !g.is_empty())
        }
    }
}

/// "{5%_All_Skill_Efficiency" reads "+5% All Skill Efficiency", as the site shows it.
fn readable(s: &str) -> String {
    s.replace('{', "+").replace('_', " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Sets bought or won rather than dropped from eggs: their `filler` isn't a drop's rarity.
fn shop_set(group: &str) -> bool {
    let g = group.to_lowercase();
    g.contains("exclusive") || g.contains("special")
}

/// A drop's rarity, as a place in [`store::RARITIES`]. The egg sets have three pets each at 5
/// and 10, three at 100 to 150, two or three at 250 to 450 and one at 1000 or more.
fn drop_rarity(weight: f64) -> usize {
    match weight {
        w if w >= 1000.0 => 4,
        w if w >= 250.0 => 3,
        w if w >= 100.0 => 2,
        w if w >= 10.0 => 1,
        _ => 0,
    }
}

/// How the egg sets split, commonest first, when the site has no egg pets to count.
const EGG_SHAPE: [f64; 5] = [10.0, 9.0, 8.0, 5.0, 3.0];

/// Each pet's rarity, read from the game:
/// - a pet from an egg (Legacy Pets, Fallen Spirits, Shallow Waters) has the rarity of its drop,
///   which gives each set one legendary, one or two epic and three of each of the rest;
/// - pets bought or won (Exclusive and Special Pets) have no drop, so they're ranked by power
///   into the same shape: the strongest twelfth or so is legendary, and so on down;
/// - a prize with no Pet+ and less power than any egg pet (Glunko Supreme, the tournament
///   champions) is legendary: only a few people can ever have one.
fn rarities(pets: &[(&RawPet, &str)]) -> Vec<&'static str> {
    let power = |p: &RawPet| num(&p.power).unwrap_or(0.0);
    let mut shape = [0.0; 5];
    let mut weakest_egg = f64::INFINITY;
    for (p, group) in pets {
        if let (false, Some(w)) = (shop_set(group), num(&p.filler)) {
            shape[drop_rarity(w)] += 1.0;
            weakest_egg = weakest_egg.min(power(p));
        }
    }
    if shape.iter().sum::<f64>() == 0.0 {
        shape = EGG_SHAPE;
    }
    let total: f64 = shape.iter().sum();
    let weakest_egg = if weakest_egg.is_finite() { weakest_egg } else { 30.0 };
    let prize = |p: &RawPet| !p.has_plus() && power(p) < weakest_egg;
    let ranked: Vec<f64> = pets.iter().filter(|(p, g)| shop_set(g) && !prize(p)).map(|(p, _)| power(p)).collect();
    // Pets as strong as each other get the same rarity.
    let by_power = |pw: f64| {
        let above = ranked.iter().filter(|x| **x > pw).count() as f64 / ranked.len().max(1) as f64;
        let mut upto = 0.0;
        for r in (0..5).rev() {
            upto += shape[r] / total;
            if above < upto - 1e-9 {
                return r;
            }
        }
        0
    };
    pets.iter()
        .map(|(p, group)| {
            let rank = match (shop_set(group), num(&p.filler)) {
                (false, Some(w)) => drop_rarity(w),
                (false, None) => 0,
                (true, _) if prize(p) => 4,
                (true, _) => by_power(power(p)),
            };
            store::RARITIES[rank]
        })
        .collect()
}

/// Pets in a set, each once, in the sets' order.
fn join(pets: &[RawPet], groups: &[Group]) -> Vec<SitePet> {
    let mut picked: Vec<(&RawPet, &str)> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for g in groups {
        for &i in &g.indices {
            let Some(p) = pets.get(i) else { continue };
            let (Some(name), Some(raw)) = (&p.name, &p.raw) else { continue };
            if !safe(raw) || readable(name).is_empty() || !seen.insert(raw.as_str()) {
                continue;
            }
            picked.push((p, g.name.as_str()));
        }
    }
    let rarity = rarities(&picked);
    picked
        .iter()
        .zip(rarity)
        .map(|((p, group), rarity)| SitePet {
            name: text::clip(&readable(p.name.as_deref().unwrap_or_default()), 60),
            raw: p.raw.clone().unwrap_or_default(),
            group: text::clip(&readable(group), 60),
            bonus: text::clip(&readable(&p.effect), 200),
            plus_bonus: if p.has_plus() { text::clip(&readable(&p.upgraded), 200) } else { String::new() },
            rarity,
        })
        .collect()
}

/// Letters, digits and underscores only: these go into a path on the site.
fn safe(s: &str) -> bool {
    !s.is_empty() && s.len() <= 60 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Read the list from the site: the page, then its scripts a few at a time until both the
/// pet list and the sets have turned up.
async fn read_list() -> Result<Vec<SitePet>, String> {
    let client = client()?;
    let html = String::from_utf8_lossy(&fetch(&client, PAGE, 2 << 20).await?).into_owned();
    let scripts = script_paths(&html);
    if scripts.is_empty() {
        return Err(CANT_FIND.into());
    }
    let mut queue = scripts.into_iter();
    let mut set = bot_core::tokio::task::JoinSet::new();
    let spawn = |set: &mut bot_core::tokio::task::JoinSet<Result<Vec<u8>, String>>, path: String| {
        let client = client.clone();
        set.spawn(async move { fetch(&client, &path, 12 << 20).await.map_err(String::from) });
    };
    for path in queue.by_ref().take(PARALLEL) {
        spawn(&mut set, path);
    }
    let (mut pets, mut groups, mut failed) = (None, None, None);
    while let Some(done) = set.join_next().await {
        match done {
            Ok(Ok(body)) => {
                let js = String::from_utf8_lossy(&body);
                if pets.is_none() {
                    pets = parse_pets(&js);
                }
                if groups.is_none() {
                    groups = parse_groups(&js);
                }
            }
            Ok(Err(e)) => failed = failed.or(Some(e)),
            Err(_) => {}
        }
        if pets.is_some() && groups.is_some() {
            break;
        }
        if let Some(path) = queue.next() {
            spawn(&mut set, path);
        }
    }
    set.abort_all();
    match (pets, groups) {
        (Some(p), Some(g)) => {
            let list = join(&p, &g);
            if list.is_empty() { Err(CANT_FIND.into()) } else { Ok(list) }
        }
        _ => Err(failed.unwrap_or_else(|| CANT_FIND.into())),
    }
}

/// The list, from the last hour's read or the site.
async fn list() -> Result<Arc<Vec<SitePet>>, String> {
    if let Some(l) = cached() {
        return Ok(l);
    }
    let fresh = Arc::new(read_list().await?);
    cache().list = Some((Instant::now(), bot_core::now_ms(), fresh.clone()));
    Ok(fresh)
}

fn cached() -> Option<Arc<Vec<SitePet>>> {
    cache().list.as_ref().filter(|(at, _, _)| at.elapsed() < KEEP).map(|(_, _, l)| l.clone())
}

fn read_at() -> Option<i64> {
    cache().list.as_ref().filter(|(at, _, _)| at.elapsed() < KEEP).map(|(_, ms, _)| *ms)
}

/// What asking the site for a pet's picture came to.
enum Found {
    Pic(Pic),
    /// The site has nothing usable, for sure: a 404, or a file that isn't a picture.
    Missing,
    /// It couldn't be read right now (a timeout, no connection, the site's own error).
    Unsure,
}

/// One answer for a picture, checked the way an upload is.
fn judge(got: Result<Vec<u8>, Fail>) -> Found {
    match got {
        Ok(bytes) => match picture::check(&bytes) {
            Ok(mime) => Found::Pic(Pic { mime, data: Arc::new(bytes) }),
            Err(_) => Found::Missing,
        },
        Err(f) if f.definite => Found::Missing,
        Err(_) => Found::Unsure,
    }
}

/// A pet's picture from the site: where the site draws it from, then its other folder.
async fn read_picture(client: &reqwest::Client, p: &SitePet) -> Found {
    let mut paths = vec![format!("/monsters/{}/static.png", p.raw)];
    let underscored = p.name.replace(' ', "_");
    if safe(&underscored) {
        paths.push(format!("/afk_targets/{underscored}.png"));
    }
    let mut unsure = false;
    for path in paths {
        match judge(fetch(client, &path, picture::MAX_BYTES).await) {
            Found::Pic(pic) => return Found::Pic(pic),
            Found::Missing => {}
            Found::Unsure => unsure = true,
        }
    }
    if unsure { Found::Unsure } else { Found::Missing }
}

/// Download the pictures of these pets that aren't held yet.
async fn read_pictures(want: Vec<SitePet>) -> Result<(), String> {
    let client = client()?;
    let todo: Vec<SitePet> = {
        let c = cache();
        want.into_iter().filter(|p| !c.pics.contains_key(&p.raw)).collect()
    };
    let mut queue = todo.into_iter();
    let mut set = bot_core::tokio::task::JoinSet::new();
    let spawn = |set: &mut bot_core::tokio::task::JoinSet<(String, Found)>, p: SitePet| {
        let client = client.clone();
        set.spawn(async move { (p.raw.clone(), read_picture(&client, &p).await) });
    };
    for p in queue.by_ref().take(PARALLEL + 2) {
        spawn(&mut set, p);
    }
    while let Some(done) = set.join_next().await {
        if let Ok((raw, found)) = done {
            match found {
                Found::Pic(pic) => {
                    cache().pics.insert(raw, Some(pic));
                }
                Found::Missing => {
                    cache().pics.insert(raw, None);
                }
                // Not kept, so the next check tries it again.
                Found::Unsure => {}
            }
        }
        if let Some(p) = queue.next() {
            spawn(&mut set, p);
        }
    }
    Ok(())
}

fn held_picture(raw: &str) -> Option<Pic> {
    cache().pics.get(raw).cloned().flatten()
}

/// The last check couldn't read this picture: it isn't known to be missing.
fn picture_unread(raw: &str) -> bool {
    !cache().pics.contains_key(raw)
}

// ---- comparing with a catalog ----

/// Two spellings of a name that should match: letters and digits only, and the same with
/// the words sorted ("Deckhand Pirate" and "Pirate Deckhand").
fn keys(name: &str) -> [String; 2] {
    let words: Vec<String> = name
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase())
        .filter(|w| !w.is_empty())
        .collect();
    let mut sorted = words.clone();
    sorted.sort();
    [words.concat(), sorted.join(" ")]
}

/// What the site has that the catalog doesn't.
pub struct Diff<'a, 'b> {
    /// Pets the catalog lacks.
    pub missing: Vec<&'a SitePet>,
    /// Pictures for its pets that have none.
    pub pictures: Vec<(&'b Pet, &'a SitePet)>,
    /// Its pets whose rarity, bonus or Pet+ bonus isn't the game's.
    pub updates: Vec<(&'b Pet, &'a SitePet)>,
}

/// The site's pets against the catalog, matched by name.
pub fn compare<'a, 'b>(site: &'a [SitePet], catalog: &'b [Pet]) -> Diff<'a, 'b> {
    let mut have: HashMap<String, &Pet> = HashMap::new();
    for p in catalog {
        for k in keys(&p.name) {
            have.entry(k).or_insert(p);
        }
    }
    let mut d = Diff { missing: Vec::new(), pictures: Vec::new(), updates: Vec::new() };
    let (mut pictured, mut updated): (HashSet<&str>, HashSet<&str>) = (HashSet::new(), HashSet::new());
    for s in site {
        let [a, b] = keys(&s.name);
        let Some(p) = have.get(&a).or_else(|| have.get(&b)) else {
            d.missing.push(s);
            continue;
        };
        if !p.has_picture() && pictured.insert(p.id.as_str()) {
            d.pictures.push((*p, s));
        }
        if !news(p, s).is_empty() && updated.insert(p.id.as_str()) {
            d.updates.push((*p, s));
        }
    }
    d
}

/// What taking the site's details would change on a catalog pet: "Common → Epic", "bonus",
/// "Pet+ bonus". A bonus the site doesn't have is left alone.
fn news(p: &Pet, s: &SitePet) -> Vec<String> {
    let mut out = Vec::new();
    if store::clean_rarity(&p.rarity) != s.rarity {
        out.push(format!("{} → {}", store::rarity_label(&p.rarity), store::rarity_label(s.rarity)));
    }
    if !s.bonus.is_empty() && p.bonus.trim() != s.bonus {
        out.push("bonus".into());
    }
    if !s.plus_bonus.is_empty() && p.plus_bonus.trim() != s.plus_bonus {
        out.push("Pet+ bonus".into());
    }
    out
}

/// A catalog pet with the site's rarity and bonuses.
fn updated(p: &Pet, s: &SitePet) -> Pet {
    let mut new = p.clone();
    new.rarity = s.rarity.into();
    if !s.bonus.is_empty() {
        new.bonus = s.bonus.clone();
    }
    if !s.plus_bonus.is_empty() {
        new.plus_bonus = s.plus_bonus.clone();
    }
    new
}

/// The catalog category for a set on the site: the server's own when the name, or its first
/// word, matches ("Legacy Pets" goes to "Legacy Companions"); otherwise the set's name.
fn category_for(group: &str, cats: &[String]) -> String {
    let g = group.to_lowercase();
    if let Some(c) = cats.iter().find(|c| c.to_lowercase() == g) {
        return c.clone();
    }
    let first = |s: &str| s.split_whitespace().next().unwrap_or_default().to_lowercase();
    cats.iter().find(|c| first(c) == first(group)).cloned().unwrap_or_else(|| group.to_owned())
}

/// The catalog entries for the ticked site pets. Names the catalog has are skipped, a name the
/// site lists twice goes in once, and each id is free in the catalog and among the new ones.
/// The catalog is read once for all of them.
fn new_pets<'a>(ticked: impl IntoIterator<Item = &'a SitePet>, catalog: &[Pet], cats: &[String], guild: u64) -> Vec<(Pet, &'a SitePet)> {
    let mut names: HashSet<String> = catalog.iter().map(|p| store::name_key(&p.name)).collect();
    let mut ids: HashSet<String> = catalog.iter().map(|p| p.id.clone()).collect();
    let mut out = Vec::new();
    for s in ticked {
        if !names.insert(store::name_key(&s.name)) {
            continue;
        }
        let id = store::free_id(&s.name, &ids);
        ids.insert(id.clone());
        let pet = Pet {
            id,
            name: s.name.clone(),
            emoji: String::new(),
            category: category_for(&s.group, cats),
            rarity: s.rarity.into(),
            description: String::new(),
            bonus: s.bonus.clone(),
            plus_bonus: s.plus_bonus.clone(),
            image: String::new(),
            guild,
            picture: None,
        };
        out.push((pet, s));
    }
    out
}

// ---- routes ----

async fn check(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = open(&ctx, &user, guild)?;
    let catalog = store::catalog(&ctx, guild)?;
    let run = async {
        let site = list().await?;
        let d = compare(&site, &catalog);
        let want: Vec<SitePet> = d.missing.into_iter().chain(d.pictures.into_iter().map(|(_, s)| s)).cloned().collect();
        read_pictures(want).await
    };
    let outcome = match bot_core::tokio::time::timeout(DEADLINE, run).await {
        Ok(r) => r,
        Err(_) => Err(format!("idleontoolbox.com took longer than {} s. Try again later.", DEADLINE.as_secs())),
    };
    match outcome {
        Ok(()) => {
            cache().errors.remove(&guild);
        }
        Err(e) => {
            bot_core::tracing::warn!(guild, error = %e, "idleontoolbox check failed");
            cache().errors.insert(guild, e);
        }
    }
    Ok(Redirect::to(&format!("{}?idleon=1#idleon", page.path("pets/catalog"))).into_response())
}

async fn add(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = open(&ctx, &user, guild)?;
    let Some(site) = cached() else {
        return Err(WebError::Bad("The check is over an hour old. Run it again, then add.".into()));
    };
    let new: HashSet<&str> = form.all("new").into_iter().collect();
    let pics: HashSet<&str> = form.all("pic").into_iter().collect();
    let upd: HashSet<&str> = form.all("upd").into_iter().collect();
    if new.is_empty() && pics.is_empty() && upd.is_empty() {
        return Err(WebError::Bad("Tick at least one pet first.".into()));
    }
    let catalog = store::catalog(&ctx, guild)?;
    let cats = store::categories(&ctx, guild)?;
    // Compared with the catalog as it is now, whatever the list said a moment ago.
    let d = compare(&site, &catalog);
    let mut added: Vec<String> = Vec::new();
    for (pet, s) in new_pets(d.missing.into_iter().filter(|s| new.contains(s.raw.as_str())), &catalog, &cats, guild) {
        store::save_pet(&ctx, guild, &pet)?;
        if let Some(p) = held_picture(&s.raw) {
            store::set_picture(&ctx, guild, &pet.id, p.mime, &p.data)?;
        }
        added.push(pet.name);
    }
    let mut given = 0;
    for (pet, s) in d.pictures.into_iter().filter(|(p, _)| pics.contains(p.id.as_str())) {
        if let Some(p) = held_picture(&s.raw) {
            store::set_picture(&ctx, guild, &pet.id, p.mime, &p.data)?;
            given += 1;
        }
    }
    let mut changed: Vec<String> = Vec::new();
    for (pet, s) in d.updates.into_iter().filter(|(p, _)| upd.contains(p.id.as_str())) {
        let new = updated(pet, s);
        store::save_pet(&ctx, guild, &new)?;
        // Giveaways keep a copy of the rarity.
        store::sync_giveaways(&ctx, guild, &new)?;
        changed.push(format!("{} ({})", pet.name, news(pet, s).join(", ")));
    }
    match added.as_slice() {
        [] => {}
        [one] => ctx.audit(&page.user, Some(guild), KEY, format!("Added the pet {one} from idleontoolbox.com")),
        many => ctx.audit(&page.user, Some(guild), KEY, format!("Added {} pets from idleontoolbox.com: {}", many.len(), text::clip(&many.join(", "), 300))),
    }
    if given > 0 {
        ctx.audit(&page.user, Some(guild), KEY, format!("Added pictures from idleontoolbox.com to {given} {}", if given == 1 { "pet" } else { "pets" }));
    }
    if !changed.is_empty() {
        ctx.audit(
            &page.user,
            Some(guild),
            KEY,
            format!("Took the game's rarity and bonuses from idleontoolbox.com for {}: {}", plural(changed.len(), "pet", "pets"), text::clip(&changed.join(", "), 400)),
        );
    }
    Ok(bot_core::web::saved(&page.path("pets/catalog")))
}

/// A picture the check downloaded, for the list of what can be added.
async fn candidate_picture(State(ctx): State<Ctx>, user: User, Path((guild, raw)): Path<(u64, String)>) -> WebResult {
    open(&ctx, &user, guild)?;
    let Some(p) = held_picture(&raw) else { return Err(WebError::NotFound) };
    Ok(([(header::CONTENT_TYPE, p.mime.to_owned()), (header::CACHE_CONTROL, "private, max-age=3600".to_owned())], p.data.to_vec()).into_response())
}

// ---- on the catalog page ----

/// The rail block that starts a check. Editors only.
pub fn rail_block(page: &Page) -> Markup {
    html! {
        div.rail-block {
            h2 { "idleontoolbox.com" }
            p.muted.small { "Compare the catalog with the site's list of Idleon pets: pets you're missing, and pictures for pets without one. You pick what to add." }
            form method="post" action=(page.path("pets/catalog/idleon")) {
                button.btn.small.outline type="submit" { (ui::icon(Icon::Search)) "Check idleontoolbox" }
            }
        }
    }
}

/// What the last check found, at the top of the catalog: pets to add and pictures to take,
/// all ticked, with one button.
pub fn panel(page: &Page, catalog: &[Pet]) -> Markup {
    let close = page.path("pets/catalog");
    let again = page.path("pets/catalog/idleon");
    let error = cache().errors.get(&page.guild_id).cloned();
    if let Some(e) = error {
        return html! {
            section.pets-io.bad #idleon {
                div.pets-io-head {
                    div.grow { h2 { "Couldn't check idleontoolbox.com" } p { (e) } }
                    form method="post" action=(again) { button.btn.small type="submit" { "Try again" } }
                    a.btn.small.ghost href=(close) { "Close" }
                }
            }
        };
    }
    let (Some(site), Some(at)) = (cached(), read_at()) else {
        return html! {
            section.pets-io #idleon {
                div.pets-io-head {
                    div.grow { h2 { "The last check is over an hour old" } p { "Run it again to see what idleontoolbox.com has." } }
                    form method="post" action=(again) { button.btn.small type="submit" { "Check again" } }
                    a.btn.small.ghost href=(close) { "Close" }
                }
            }
        };
    };
    let Diff { missing, pictures, updates } = compare(&site, catalog);
    let src = |raw: &str| held_picture(raw).map(|_| page.path(&format!("pets/catalog/idleon/pic/{raw}")));
    let title = match (missing.len(), updates.len(), pictures.len()) {
        (0, 0, 0) => "Your catalog matches idleontoolbox.com".to_owned(),
        (0, 0, _) => "Every pet on idleontoolbox.com is in your catalog".to_owned(),
        (0, n, _) => format!("{} match the game's data", plural(n, "pet doesn't", "pets don't")),
        (1, _, _) => "1 pet isn't in your catalog".to_owned(),
        (n, _, _) => format!("{n} pets aren't in your catalog"),
    };
    let checked = format!("Checked {} on idleontoolbox.com.", text::ago(at));
    let unread = missing.iter().copied().chain(pictures.iter().map(|(_, s)| *s)).filter(|s| picture_unread(&s.raw)).count();
    // Only pictures the check has can be taken; the rest show without a tick box.
    let takeable = pictures.iter().filter(|(_, s)| held_picture(&s.raw).is_some()).count();
    html! {
        section.pets-io #idleon {
            div.pets-io-head {
                div.grow {
                    h2 { (title) }
                    p {
                        (checked)
                        @if !missing.is_empty() || !updates.is_empty() { " " (RARITY_LINE) }
                        @if unread > 0 { " " (unread_line(unread)) }
                    }
                }
                @if unread > 0 { form method="post" action=(again) { button.btn.small type="submit" { "Check again" } } }
                a.btn.small.ghost href=(close) { "Close" }
            }
            @if !missing.is_empty() || !pictures.is_empty() || !updates.is_empty() {
                form.pets-io-form method="post" action=(page.path("pets/catalog/idleon/add")) {
                    @if !missing.is_empty() {
                        div.pets-io-sub {
                            h3 { "New pets" }
                            label.pets-check { input type="checkbox" data-all="new" checked; "Select all" }
                        }
                        div.pets-picks {
                            @for s in &missing {
                                label class={ "pets-pick r-" (s.rarity) } data-tip=(site_tip(s)) {
                                    input type="checkbox" name="new" value=(s.raw) checked;
                                    (pick_art(src(&s.raw).as_deref(), picture_unread(&s.raw)))
                                    span.pets-pick-name { (s.name) }
                                    (view::rarity(s.rarity))
                                }
                            }
                        }
                    }
                    @if !updates.is_empty() {
                        div.pets-io-sub {
                            h3 { "Rarity and bonuses from the game" }
                            label.pets-check { input type="checkbox" data-all="upd" checked; "Select all" }
                        }
                        div.pets-picks {
                            @for (p, s) in &updates {
                                @let what = news(p, s);
                                label class={ "pets-pick r-" (s.rarity) } data-tip=(site_tip(s)) {
                                    input type="checkbox" name="upd" value=(p.id) checked;
                                    span.pets-pick-art aria-hidden="true" {
                                        @match p.image_url() {
                                            Some(u) => img src=(u) alt="" loading="lazy";,
                                            None => span.pets-pick-emoji { (p.icon()) },
                                        }
                                    }
                                    span.pets-pick-name { (p.name) }
                                    span.pets-pick-what { (what.join(", ")) }
                                }
                            }
                        }
                    }
                    @if !pictures.is_empty() {
                        div.pets-io-sub {
                            h3 { @if pictures.len() == 1 { "1 of your pets has no picture" } @else { (pictures.len()) " of your pets have no picture" } }
                            @if takeable > 0 { label.pets-check { input type="checkbox" data-all="pic" checked; "Select all" } }
                        }
                        div.pets-picks {
                            @for (p, s) in &pictures {
                                @let art = src(&s.raw);
                                @if art.is_some() {
                                    label.pets-pick data-tip={ "Picture for " (p.name) } {
                                        input type="checkbox" name="pic" value=(p.id) checked;
                                        (pick_art(art.as_deref(), false))
                                        span.pets-pick-name { (p.name) }
                                    }
                                } @else {
                                    @let failed = picture_unread(&s.raw);
                                    div.pets-pick.off data-tip=(if failed { "Its picture couldn't be fetched right now" } else { "idleontoolbox.com has no picture for it" }) {
                                        (pick_art(None, failed))
                                        span.pets-pick-name { (p.name) }
                                    }
                                }
                            }
                        }
                    }
                    div.pets-io-foot {
                        button.btn.primary type="submit" data-label { (add_label(missing.len(), takeable, updates.len())) }
                    }
                }
                script { (PreEscaped(PANEL_JS)) }
            }
        }
    }
}

/// How the bot works out a rarity, said where it offers one.
const RARITY_LINE: &str = "The site has no rarity, so the bot takes it from the game: egg pets have the rarity of their drop, and shop and event pets are ranked by power into the same shape, one legendary for every eleven or so.";

/// A site pet's hover text: its set, rarity, bonus and Pet+ bonus.
fn site_tip(s: &SitePet) -> String {
    let mut t = format!("{}, {}", s.group, store::rarity_label(s.rarity));
    if !s.bonus.is_empty() {
        t.push_str(&format!(". {}", s.bonus));
    }
    if !s.plus_bonus.is_empty() {
        t.push_str(&format!(". Pet+: {}", s.plus_bonus));
    }
    t
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A pick's picture, or why there's none: the site has none, or it couldn't be read just now.
fn pick_art(src: Option<&str>, unread: bool) -> Markup {
    html! {
        span.pets-pick-art aria-hidden="true" {
            @match src {
                Some(s) => img src=(s) alt="" loading="lazy";,
                None if unread => span.muted.small { "Couldn't fetch" },
                None => span.muted.small { "No picture" },
            }
        }
    }
}

/// The panel's line about pictures the check couldn't read.
fn unread_line(n: usize) -> String {
    if n == 1 {
        "1 picture couldn't be fetched right now. Check again to try it again.".into()
    } else {
        format!("{n} pictures couldn't be fetched right now. Check again to try them again.")
    }
}

/// The button: "Add 2 pets and 1 picture, update 3 pets".
fn add_label(pets: usize, pics: usize, upd: usize) -> String {
    let mut adds = Vec::new();
    if pets > 0 {
        adds.push(plural(pets, "pet", "pets"));
    }
    if pics > 0 {
        adds.push(plural(pics, "picture", "pictures"));
    }
    let add = (!adds.is_empty()).then(|| format!("Add {}", adds.join(" and ")));
    match (add, upd) {
        (None, 0) => "Nothing ticked".into(),
        (None, n) => format!("Update {}", plural(n, "pet", "pets")),
        (Some(a), 0) => a,
        (Some(a), n) => format!("{a}, update {}", plural(n, "pet", "pets")),
    }
}

/// "Select all" boxes, and the button saying what it will add.
const PANEL_JS: &str = r#"(() => {
  const form = document.querySelector("main.page .pets-io-form");
  if (!form) return;
  const btn = form.querySelector("[data-label]");
  const n = (name) => form.querySelectorAll(`input[name="${name}"]:checked`).length;
  const say = (k, one, many) => `${k} ${k === 1 ? one : many}`;
  const sync = () => {
    const a = n("new"), b = n("pic"), u = n("upd");
    const adds = [a && say(a, "pet", "pets"), b && say(b, "picture", "pictures")].filter(Boolean);
    const add = adds.length ? `Add ${adds.join(" and ")}` : "";
    btn.textContent = add && u ? `${add}, update ${say(u, "pet", "pets")}` : add || (u ? `Update ${say(u, "pet", "pets")}` : "Nothing ticked");
    btn.disabled = !a && !b && !u;
    form.querySelectorAll("[data-all]").forEach((all) => {
      const boxes = form.querySelectorAll(`input[name="${all.dataset.all}"]`);
      const on = [...boxes].filter((x) => x.checked).length;
      all.checked = on === boxes.length;
      all.indeterminate = on > 0 && on < boxes.length;
    });
  };
  form.addEventListener("change", (e) => {
    const all = e.target.closest("[data-all]");
    if (all) form.querySelectorAll(`input[name="${all.dataset.all}"]`).forEach((x) => { x.checked = all.checked; });
    sync();
  });
})();"#;

#[cfg(test)]
mod tests {
    use super::*;

    const JS: &str = r#"(globalThis.TURBOPACK||[]).push([553898,(e,a,r)=>{a.exports=JSON.parse('[{"name":"King_Doot","rawName":"babaMummy","effect":"All_Divinities_from_World_5_count_as_Active","bonus":1,"filler":"2000","tourPower":165,"upgradeCost":3000,"upgradedEffect":"All_Divinities_count_as_Active"},{"name":"Santas_Little_Helper","rawName":"w6c2b","effect":"{5%_Santa\'s_bonus","upgradeCost":null,"upgradedEffect":"{5%_Santa\'s_bonus"},{"name":"Pirate_Deckhand","rawName":"w7b11","effect":"x"},{"name":"Slime","rawName":"slimeG","effect":"y"},{"name":"Nutto","rawName":"acorn","effect":"Not_officially_in_the_game_and_may_never_be"},{"name":"A","rawName":"a","effect":""},{"name":"B","rawName":"b","effect":""},{"name":"C","rawName":"c","effect":""},{"name":"D","rawName":"d","effect":""},{"name":"E","rawName":"e","effect":""},{"name":"F","rawName":"f","effect":""},{"name":"G","rawName":"g","effect":""},{"name":"H","rawName":"h","effect":""},{"name":"I","rawName":"i","effect":""},{"name":"J","rawName":"j","effect":""},{"name":"K","rawName":"k","effect":""},{"name":"L","rawName":"l","effect":""},{"name":"M","rawName":"m","effect":""},{"name":"N","rawName":"n","effect":""},{"name":"O","rawName":"o","effect":""},{}]')}]);"#;
    const DATA: &str = r#"x=JSON.parse('{"bundles":[],"companionGroups":[{"name":"Legacy Pets","indices":[0,3]},{"name":"Exclusive Pets","indices":[1,2,99]},{"name":"Special Pets","indices":[1]}],"constellations":[]}')"#;

    #[test]
    fn reads_the_site_data() {
        let pets = parse_pets(JS).expect("pet list");
        assert_eq!(pets.len(), 21);
        assert_eq!(pets[1].effect, "{5%_Santa's_bonus");
        assert!(parse_pets(DATA).is_none());
        let groups = parse_groups(DATA).expect("groups");
        assert_eq!(groups.len(), 3);
        assert!(parse_groups(JS).is_none());
        let list = join(&pets, &groups);
        let names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
        // Only pets in a set, once each; Nutto isn't in the game.
        assert_eq!(names, ["King Doot", "Slime", "Santas Little Helper", "Pirate Deckhand"]);
        assert_eq!(list[2].bonus, "+5% Santa's bonus");
        assert_eq!(list[2].group, "Exclusive Pets");
        // Pet+: its bonus, and none for a pet that can't be upgraded.
        assert_eq!((list[0].plus_bonus.as_str(), list[0].rarity), ("All Divinities count as Active", "legendary"));
        assert_eq!(list[2].plus_bonus, "");
    }

    fn raw(name: &str, filler: f64, power: f64, plus: bool) -> RawPet {
        RawPet {
            name: Some(name.into()),
            raw: Some(name.into()),
            effect: "x".into(),
            upgraded: if plus { "y".into() } else { String::new() },
            upgrade_cost: if plus { serde_json::json!(100) } else { serde_json::Value::Null },
            filler: serde_json::json!(filler.to_string()),
            power: serde_json::json!(power),
        }
    }

    /// Egg pets by their drop; shop pets by power into the eggs' shape; prizes are legendary.
    #[test]
    fn rarity_comes_from_the_game() {
        let eggs = [(2000.0, 165.0), (450.0, 125.0), (150.0, 120.0), (150.0, 110.0), (10.0, 80.0), (10.0, 70.0), (10.0, 65.0), (5.0, 50.0), (5.0, 50.0), (5.0, 50.0), (5.0, 40.0)];
        let mut pets: Vec<RawPet> = eggs.iter().enumerate().map(|(i, (f, p))| raw(&format!("egg{i}"), *f, *p, true)).collect();
        // Twenty-two shop pets, powers 100 to 310.
        for i in 0..22 {
            pets.push(raw(&format!("shop{i}"), 1.0, 100.0 + 10.0 * i as f64, true));
        }
        pets.push(raw("champion", 1.0, 10.0, false));
        pets.push(raw("weak-but-upgradable", 1.0, 10.0, true));
        let sets: Vec<(&RawPet, &str)> = pets.iter().map(|p| (p, if p.name.as_deref().unwrap().starts_with("egg") { "Legacy Pets" } else { "Exclusive Pets" })).collect();
        let got = rarities(&sets);
        assert_eq!(&got[..11], ["legendary", "epic", "rare", "rare", "uncommon", "uncommon", "uncommon", "common", "common", "common", "common"]);
        let shop = &got[11..33];
        let count = |r: &str| shop.iter().filter(|x| **x == r).count();
        assert_eq!(shop[21], "legendary", "the strongest");
        assert_eq!(shop[0], "common", "the weakest");
        // 23 ranked (the weak one with a Pet+ too) in the eggs' 1:1:2:3:4.
        assert_eq!([count("legendary"), count("epic"), count("rare"), count("uncommon"), count("common")], [3, 2, 4, 6, 7], "the eggs' shape");
        assert!(shop.windows(2).all(|w| store::rarity_rank(w[0]) <= store::rarity_rank(w[1])), "stronger is never less rare");
        assert_eq!(got[33], "legendary", "a prize with no Pet+");
        assert_eq!(got[34], "common", "weak, but it has a Pet+");
    }

    /// Equal power, equal rarity; no egg pets at all still gives a rarity.
    #[test]
    fn rarity_ties_and_no_eggs() {
        let pets: Vec<RawPet> = (0..10).map(|i| raw(&format!("s{i}"), 1.0, if i < 5 { 100.0 } else { 200.0 }, true)).collect();
        let sets: Vec<(&RawPet, &str)> = pets.iter().map(|p| (p, "Special Pets")).collect();
        let got = rarities(&sets);
        assert!(got[..5].iter().all(|r| *r == got[0]) && got[5..].iter().all(|r| *r == got[5]));
        assert_eq!(got[5], "legendary");
        assert_eq!(num(&serde_json::json!(" 250 ")), Some(250.0));
        assert_eq!(num(&serde_json::json!("lots")), None);
    }

    /// A character of two or three bytes across the 400-byte look-ahead used to panic and take
    /// the bot down: é from the site, or the U+FFFD a lossy read puts in for bad bytes.
    #[test]
    fn a_character_across_the_look_ahead_is_fine() {
        let start = JS.find("JSON.parse('[{").unwrap() + "JSON.parse('".len();
        let at = JS.find("All_Divinities").unwrap() - start;
        for c in ['é', '\u{fffd}'] {
            for back in 1..c.len_utf8() {
                let js = JS.replacen("All_Divinities", &format!("{}{c}All_Divinities", "x".repeat(400 - at - back)), 1);
                assert!(!js.is_char_boundary(start + 400), "the character sits across the cut");
                let pets = parse_pets(&js).expect("pet list");
                assert!(pets[0].effect.contains(c));
            }
        }
    }

    /// The same for the 64 KiB window the sets are read from when their quotes are escaped.
    #[test]
    fn a_character_across_the_sets_window_is_fine() {
        let head = r#"x=JSON.parse('{"companionGroups":"#;
        let groups = r#"[{"name":"Santa\'s Pets","indices":[0,1]}],"pad":""#;
        for c in ['é', '\u{fffd}'] {
            for back in 1..c.len_utf8() {
                let js = format!("{head}{groups}{}{c}\"}}')", "x".repeat(64 * 1024 - groups.len() - back));
                assert!(!js.is_char_boundary(head.len() + 64 * 1024), "the character sits across the cut");
                let got = parse_groups(&js).expect("groups");
                assert_eq!((got[0].name.as_str(), got[0].indices.as_slice()), ("Santa's Pets", &[0, 1][..]));
            }
        }
    }

    #[test]
    fn only_a_sure_answer_means_no_picture() {
        let fail = |status: Option<u16>, definite| Err(Fail { msg: format!("{status:?}"), definite });
        assert!(matches!(judge(Ok(b"\x89PNG\r\n\x1a\n....".to_vec())), Found::Pic(p) if p.mime == "image/png"));
        assert!(matches!(judge(Ok(b"<!doctype html>".to_vec())), Found::Missing), "a page instead of a picture");
        assert!(matches!(judge(fail(Some(404), true)), Found::Missing));
        assert!(matches!(judge(fail(Some(503), false)), Found::Unsure), "the site's error is tried again");
        assert!(matches!(judge(fail(None, false)), Found::Unsure), "a timeout is tried again");
        assert!(pick_art(None, true).into_string().contains("Couldn't fetch"));
        assert!(pick_art(None, false).into_string().contains("No picture"));
        assert_eq!(unread_line(2), "2 pictures couldn't be fetched right now. Check again to try them again.");
    }

    #[test]
    fn new_pets_read_the_catalog_once() {
        let site = |name: &str, raw: &str| SitePet {
            name: name.into(),
            raw: raw.into(),
            group: "Legacy Pets".into(),
            bonus: "+1% x".into(),
            plus_bonus: "+2% x".into(),
            rarity: "epic",
        };
        let ticked = [site("Glimbo", "a"), site("glimbo", "b"), site("Slime", "c"), site("Glimbo!", "d")];
        let catalog = [pet("glimbo", "Glimbo Classic", ""), pet("slime", "Slime", "")];
        let cats = ["Legacy Companions".to_owned()];
        let out = new_pets(&ticked, &catalog, &cats, 1);
        let got: Vec<(&str, &str, &str)> = out.iter().map(|(p, s)| (p.id.as_str(), p.name.as_str(), s.raw.as_str())).collect();
        // Slime is in the catalog; the second Glimbo is the same name; ids never collide.
        assert_eq!(got, [("glimbo-2", "Glimbo", "a"), ("glimbo-3", "Glimbo!", "d")]);
        assert_eq!((out[0].0.category.as_str(), out[0].0.bonus.as_str()), ("Legacy Companions", "+1% x"));
        assert_eq!((out[0].0.rarity.as_str(), out[0].0.plus_bonus.as_str()), ("epic", "+2% x"));
    }

    /// Reads the live site: `cargo test -p feat-pets live_site -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads idleontoolbox.com"]
    fn live_site() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let rt = bot_core::tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let list = rt.block_on(read_list()).expect("the site's list");
        let n = |r: &str| list.iter().filter(|p| p.rarity == r).count();
        println!("{} pets: {:?}", list.len(), store::RARITIES.map(|r| (r, n(r))));
        println!("with a Pet+ bonus: {}", list.iter().filter(|p| !p.plus_bonus.is_empty()).count());
        for p in list.iter().filter(|p| p.rarity == "legendary") {
            println!("  legendary: {} ({}) {} | Pet+: {}", p.name, p.group, p.bonus, p.plus_bonus);
        }
        assert!(n("legendary") < n("common"));
    }

    #[test]
    fn finds_scripts_on_the_site_only() {
        let html = r#"<script src="/_next/static/chunks/2y135y279kk72.js" async=""></script><script src="/_next/static/chunks/2y135y279kk72.js"></script>
            <script src="https://evil.example/_next/static/chunks/../x.js"></script><script src="/_next/static/chunks/a%2F..js"></script>"#;
        assert_eq!(script_paths(html), ["/_next/static/chunks/2y135y279kk72.js"]);
    }

    #[test]
    fn unescapes_js_strings() {
        assert_eq!(js_string(r"a\'b\\cA\x42'rest").as_deref(), Some("a'b\\cAB"));
        assert_eq!(js_string("no end"), None);
    }

    fn pet(id: &str, name: &str, image: &str) -> Pet {
        Pet {
            id: id.into(),
            name: name.into(),
            emoji: String::new(),
            category: "Legacy Companions".into(),
            rarity: "common".into(),
            description: String::new(),
            bonus: String::new(),
            plus_bonus: String::new(),
            image: image.into(),
            guild: 1,
            picture: None,
        }
    }

    #[test]
    fn compares_by_name() {
        let site: Vec<SitePet> = [("King Doot", "babaMummy"), ("Santas Little Helper", "w6c2b"), ("Pirate Deckhand", "w7b11"), ("Glimbo", "w7b3b"), ("Slime", "slimeG")]
            .iter()
            .map(|(n, r)| SitePet { name: (*n).into(), raw: (*r).into(), group: "Legacy Pets".into(), bonus: String::new(), plus_bonus: String::new(), rarity: "common" })
            .collect();
        let mut doot = pet("king-doot", "King Doot", "");
        doot.picture = Some(5);
        let catalog = [doot, pet("helper", "Santa's Little Helper", ""), pet("deck", "Deckhand Pirate", ""), pet("slime", "slime", "https://x/s.png")];
        let d = compare(&site, &catalog);
        assert_eq!(d.missing.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Glimbo"]);
        assert_eq!(d.pictures.iter().map(|(p, _)| p.id.as_str()).collect::<Vec<_>>(), ["helper", "deck"], "pets with a picture keep it");
        assert!(d.updates.is_empty(), "same rarity, and the site has no bonus to give");
    }

    /// A pet whose rarity or bonuses aren't the game's is offered the game's; taking them keeps
    /// everything else, and a bonus the site lacks stays.
    #[test]
    fn offers_the_games_rarity_and_bonuses() {
        let s = |rarity: &'static str, bonus: &str, plus: &str| SitePet {
            name: "King Doot".into(),
            raw: "babaMummy".into(),
            group: "Legacy Pets".into(),
            bonus: bonus.into(),
            plus_bonus: plus.into(),
            rarity,
        };
        let mut doot = pet("king-doot", "King Doot", "");
        doot.bonus = "+1 thing".into();
        doot.description = "The royal ruler of all doots.".into();
        let site = [s("legendary", "+1 thing", "+2 things")];
        let d = compare(&site, std::slice::from_ref(&doot));
        assert_eq!(news(d.updates[0].0, d.updates[0].1), ["Common → Legendary", "Pet+ bonus"]);
        let new = updated(&doot, &site[0]);
        assert_eq!((new.rarity.as_str(), new.bonus.as_str(), new.plus_bonus.as_str()), ("legendary", "+1 thing", "+2 things"));
        assert_eq!(new.description, doot.description);
        let site = [s("common", "", "")];
        assert!(compare(&site, std::slice::from_ref(&doot)).updates.is_empty(), "nothing to take");
    }

    #[test]
    fn sets_go_to_the_servers_categories() {
        let cats = ["Legacy Companions".to_owned(), "Fallen Spirits".to_owned(), "Exclusive Companions".to_owned()];
        assert_eq!(category_for("Legacy Pets", &cats), "Legacy Companions");
        assert_eq!(category_for("fallen spirits", &cats), "Fallen Spirits");
        assert_eq!(category_for("Special Pets", &cats), "Special Pets");
        assert_eq!(add_label(2, 1, 0), "Add 2 pets and 1 picture");
        assert_eq!(add_label(0, 1, 3), "Add 1 picture, update 3 pets");
        assert_eq!(add_label(0, 0, 1), "Update 1 pet");
    }
}
