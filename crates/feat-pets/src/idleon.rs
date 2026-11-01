//! idleontoolbox.com's list of Idleon pets, with their pictures: which pets a server's catalog
//! is missing, and pictures for the pets it has without one.
//!
//! The site is a Next.js app that ships its game data inside its scripts; the pets page itself
//! is drawn in the browser. So the bot reads, from idleontoolbox.com only:
//! - the pets page, for the list of scripts it loads;
//! - the script with the pet list, `JSON.parse('[{"name":"King_Doot","rawName":"babaMummy",...`
//!   (name, sprite name, in-game bonus);
//! - the script with `"companionGroups"`, the in-game sets (Legacy Pets, Fallen Spirits, ...).
//!   Pets in no set are ones that aren't in the game, so they're left out;
//! - each picture at `/monsters/<rawName>/static.png`, where the site draws it from, or
//!   `/afk_targets/<Name>.png` when that's missing.
//!
//! The list is kept for an hour and pictures until the bot restarts, so a check followed by an
//! add downloads each picture once. Adding stores the picture like an upload: nothing hotlinks.

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
use crate::{KEY, picture};

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
}

#[derive(Clone)]
struct Pic {
    mime: &'static str,
    data: Arc<Vec<u8>>,
}

#[derive(Default)]
struct Cache {
    list: Option<(Instant, i64, Arc<Vec<SitePet>>)>,
    /// By sprite name. `None`: tried, and the site had no usable picture.
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

/// GET a path on the site, refusing anything larger than `max` bytes.
async fn fetch(client: &reqwest::Client, path: &str, max: usize) -> Result<Vec<u8>, String> {
    let mut res = client.get(format!("{SITE}{path}")).send().await.map_err(why)?;
    if res.url().host_str() != Some(HOST) {
        return Err("idleontoolbox.com sent the bot to another site, so it stopped.".into());
    }
    if !res.status().is_success() {
        return Err(format!("idleontoolbox.com answered {} for {path}.", res.status().as_u16()));
    }
    let too_big = || format!("idleontoolbox.com sent more for {path} than the bot accepts.");
    if res.content_length().is_some_and(|n| n > max as u64) {
        return Err(too_big());
    }
    let mut out = Vec::new();
    while let Some(part) = res.chunk().await.map_err(why)? {
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

#[derive(Debug, Deserialize)]
struct RawPet {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "rawName")]
    raw: Option<String>,
    #[serde(default)]
    effect: String,
}

/// The pet list, in the site's order (the sets point into it by position).
fn parse_pets(js: &str) -> Option<Vec<RawPet>> {
    const OPEN: &str = "JSON.parse('[{";
    for (at, _) in js.match_indices(OPEN) {
        let start = at + OPEN.len() - 2;
        let head = &js[start..js.len().min(start + 400)];
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
            let window = &rest[..rest.len().min(64 * 1024)];
            let text = js_string(&format!("{window}'"))?;
            serde_json::Deserializer::from_str(&text).into_iter::<Vec<Group>>().next()?.ok().filter(|g| !g.is_empty())
        }
    }
}

/// "{5%_All_Skill_Efficiency" reads "+5% All Skill Efficiency", as the site shows it.
fn readable(s: &str) -> String {
    s.replace('{', "+").replace('_', " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Pets in a set, each once, in the sets' order.
fn join(pets: &[RawPet], groups: &[Group]) -> Vec<SitePet> {
    let mut out: Vec<SitePet> = Vec::new();
    for g in groups {
        for &i in &g.indices {
            let Some(p) = pets.get(i) else { continue };
            let (Some(name), Some(raw)) = (&p.name, &p.raw) else { continue };
            if !safe(raw) || out.iter().any(|o| o.raw == *raw) {
                continue;
            }
            let name = text::clip(&readable(name), 60);
            if name.is_empty() {
                continue;
            }
            out.push(SitePet { name, raw: raw.clone(), group: text::clip(&readable(&g.name), 60), bonus: text::clip(&readable(&p.effect), 200) });
        }
    }
    out
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
        set.spawn(async move { fetch(&client, &path, 12 << 20).await });
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

/// A pet's picture from the site, stored the way an upload is checked.
async fn read_picture(client: &reqwest::Client, p: &SitePet) -> Option<Pic> {
    let mut paths = vec![format!("/monsters/{}/static.png", p.raw)];
    let underscored = p.name.replace(' ', "_");
    if safe(&underscored) {
        paths.push(format!("/afk_targets/{underscored}.png"));
    }
    for path in paths {
        if let Ok(bytes) = fetch(client, &path, picture::MAX_BYTES).await
            && let Ok(mime) = picture::check(&bytes)
        {
            return Some(Pic { mime, data: Arc::new(bytes) });
        }
    }
    None
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
    let spawn = |set: &mut bot_core::tokio::task::JoinSet<(String, Option<Pic>)>, p: SitePet| {
        let client = client.clone();
        set.spawn(async move { (p.raw.clone(), read_picture(&client, &p).await) });
    };
    for p in queue.by_ref().take(PARALLEL + 2) {
        spawn(&mut set, p);
    }
    while let Some(done) = set.join_next().await {
        if let Ok((raw, pic)) = done {
            cache().pics.insert(raw, pic);
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

/// What the site has that the catalog doesn't: pets it lacks, and pictures for its pets that
/// have none, as `(catalog pet, site pet)`.
pub fn compare<'a, 'b>(site: &'a [SitePet], catalog: &'b [Pet]) -> (Vec<&'a SitePet>, Vec<(&'b Pet, &'a SitePet)>) {
    let mut have: HashMap<String, &Pet> = HashMap::new();
    for p in catalog {
        for k in keys(&p.name) {
            have.entry(k).or_insert(p);
        }
    }
    let mut missing = Vec::new();
    let mut pictures = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for s in site {
        let [a, b] = keys(&s.name);
        match have.get(&a).or_else(|| have.get(&b)) {
            None => missing.push(s),
            Some(p) if !p.has_picture() && seen.insert(p.id.as_str()) => pictures.push((*p, s)),
            Some(_) => {}
        }
    }
    (missing, pictures)
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

// ---- routes ----

async fn check(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = open(&ctx, &user, guild)?;
    let catalog = store::catalog(&ctx, guild)?;
    let run = async {
        let site = list().await?;
        let (missing, pictures) = compare(&site, &catalog);
        let want: Vec<SitePet> = missing.into_iter().chain(pictures.into_iter().map(|(_, s)| s)).cloned().collect();
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
    if new.is_empty() && pics.is_empty() {
        return Err(WebError::Bad("Tick at least one pet first.".into()));
    }
    let catalog = store::catalog(&ctx, guild)?;
    let cats = store::categories(&ctx, guild)?;
    let (missing, pictured) = compare(&site, &catalog);
    let mut added: Vec<String> = Vec::new();
    for s in missing.into_iter().filter(|s| new.contains(s.raw.as_str())) {
        // Names that already exist are skipped, whatever the list said a moment ago.
        if store::name_taken(&ctx, guild, &s.name, None)?.is_some() {
            continue;
        }
        let pet = Pet {
            id: store::new_pet_id(&ctx, guild, &s.name)?,
            name: s.name.clone(),
            emoji: String::new(),
            category: category_for(&s.group, &cats),
            rarity: "common".into(),
            description: String::new(),
            bonus: s.bonus.clone(),
            image: String::new(),
            guild,
            picture: None,
        };
        store::save_pet(&ctx, guild, &pet)?;
        if let Some(p) = held_picture(&s.raw) {
            store::set_picture(&ctx, guild, &pet.id, p.mime, &p.data)?;
        }
        added.push(pet.name);
    }
    let mut given = 0;
    for (pet, s) in pictured.into_iter().filter(|(p, _)| pics.contains(p.id.as_str())) {
        if let Some(p) = held_picture(&s.raw) {
            store::set_picture(&ctx, guild, &pet.id, p.mime, &p.data)?;
            given += 1;
        }
    }
    match added.as_slice() {
        [] => {}
        [one] => ctx.audit(&page.user, Some(guild), KEY, format!("Added the pet {one} from idleontoolbox.com")),
        many => ctx.audit(&page.user, Some(guild), KEY, format!("Added {} pets from idleontoolbox.com: {}", many.len(), text::clip(&many.join(", "), 300))),
    }
    if given > 0 {
        ctx.audit(&page.user, Some(guild), KEY, format!("Added pictures from idleontoolbox.com to {given} {}", if given == 1 { "pet" } else { "pets" }));
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
    let (missing, pictures) = compare(&site, catalog);
    let src = |raw: &str| held_picture(raw).map(|_| page.path(&format!("pets/catalog/idleon/pic/{raw}")));
    let title = match (missing.len(), pictures.len()) {
        (0, 0) => "Your catalog has every pet idleontoolbox.com lists".to_owned(),
        (0, _) => "Every pet on idleontoolbox.com is in your catalog".to_owned(),
        (1, _) => "1 pet isn't in your catalog".to_owned(),
        (n, _) => format!("{n} pets aren't in your catalog"),
    };
    let checked = format!("Checked {} on idleontoolbox.com.", text::ago(at));
    html! {
        section.pets-io #idleon {
            div.pets-io-head {
                div.grow {
                    h2 { (title) }
                    p {
                        (checked)
                        @if !missing.is_empty() { " The site doesn't list rarity, so new pets start as Common. Change it from each pet." }
                    }
                }
                a.btn.small.ghost href=(close) { "Close" }
            }
            @if !missing.is_empty() || !pictures.is_empty() {
                form.pets-io-form method="post" action=(page.path("pets/catalog/idleon/add")) {
                    @if !missing.is_empty() {
                        div.pets-io-sub {
                            label.pets-check { input type="checkbox" data-all="new" checked; "Select all" }
                        }
                        div.pets-picks {
                            @for s in &missing {
                                label.pets-pick data-tip={ (s.group) @if !s.bonus.is_empty() { ": " (s.bonus) } } {
                                    input type="checkbox" name="new" value=(s.raw) checked;
                                    (pick_art(src(&s.raw).as_deref()))
                                    span.pets-pick-name { (s.name) }
                                }
                            }
                        }
                    }
                    @if !pictures.is_empty() {
                        div.pets-io-sub {
                            h3 { @if pictures.len() == 1 { "1 of your pets has no picture" } @else { (pictures.len()) " of your pets have no picture" } }
                            label.pets-check { input type="checkbox" data-all="pic" checked; "Select all" }
                        }
                        div.pets-picks {
                            @for (p, s) in &pictures {
                                label.pets-pick data-tip={ "Picture for " (p.name) } {
                                    input type="checkbox" name="pic" value=(p.id) checked;
                                    (pick_art(src(&s.raw).as_deref()))
                                    span.pets-pick-name { (p.name) }
                                }
                            }
                        }
                    }
                    div.pets-io-foot {
                        button.btn.primary type="submit" data-label { (add_label(missing.len(), pictures.len())) }
                    }
                }
                script { (PreEscaped(PANEL_JS)) }
            }
        }
    }
}

fn pick_art(src: Option<&str>) -> Markup {
    html! {
        span.pets-pick-art aria-hidden="true" {
            @match src {
                Some(s) => img src=(s) alt="" loading="lazy";,
                None => span.muted.small { "No picture" },
            }
        }
    }
}

fn add_label(pets: usize, pics: usize) -> String {
    let p = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    match (pets, pics) {
        (0, 0) => "Add".into(),
        (n, 0) => format!("Add {}", p(n, "pet", "pets")),
        (0, m) => format!("Add {}", p(m, "picture", "pictures")),
        (n, m) => format!("Add {} and {}", p(n, "pet", "pets"), p(m, "picture", "pictures")),
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
    const a = n("new"), b = n("pic");
    btn.textContent = a && b ? `Add ${say(a, "pet", "pets")} and ${say(b, "picture", "pictures")}` : a ? `Add ${say(a, "pet", "pets")}` : b ? `Add ${say(b, "picture", "pictures")}` : "Nothing ticked";
    btn.disabled = !a && !b;
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

    const JS: &str = r#"(globalThis.TURBOPACK||[]).push([553898,(e,a,r)=>{a.exports=JSON.parse('[{"name":"King_Doot","rawName":"babaMummy","effect":"All_Divinities_from_World_5_count_as_Active","bonus":1},{"name":"Santas_Little_Helper","rawName":"w6c2b","effect":"{5%_Santa\'s_bonus"},{"name":"Pirate_Deckhand","rawName":"w7b11","effect":"x"},{"name":"Slime","rawName":"slimeG","effect":"y"},{"name":"Nutto","rawName":"acorn","effect":"Not_officially_in_the_game_and_may_never_be"},{"name":"A","rawName":"a","effect":""},{"name":"B","rawName":"b","effect":""},{"name":"C","rawName":"c","effect":""},{"name":"D","rawName":"d","effect":""},{"name":"E","rawName":"e","effect":""},{"name":"F","rawName":"f","effect":""},{"name":"G","rawName":"g","effect":""},{"name":"H","rawName":"h","effect":""},{"name":"I","rawName":"i","effect":""},{"name":"J","rawName":"j","effect":""},{"name":"K","rawName":"k","effect":""},{"name":"L","rawName":"l","effect":""},{"name":"M","rawName":"m","effect":""},{"name":"N","rawName":"n","effect":""},{"name":"O","rawName":"o","effect":""},{}]')}]);"#;
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
            image: image.into(),
            guild: 1,
            picture: None,
        }
    }

    #[test]
    fn compares_by_name() {
        let site: Vec<SitePet> = [("King Doot", "babaMummy"), ("Santas Little Helper", "w6c2b"), ("Pirate Deckhand", "w7b11"), ("Glimbo", "w7b3b"), ("Slime", "slimeG")]
            .iter()
            .map(|(n, r)| SitePet { name: (*n).into(), raw: (*r).into(), group: "Legacy Pets".into(), bonus: String::new() })
            .collect();
        let mut doot = pet("king-doot", "King Doot", "");
        doot.picture = Some(5);
        let catalog = [doot, pet("helper", "Santa's Little Helper", ""), pet("deck", "Deckhand Pirate", ""), pet("slime", "slime", "https://x/s.png")];
        let (missing, pictures) = compare(&site, &catalog);
        assert_eq!(missing.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Glimbo"]);
        assert_eq!(pictures.iter().map(|(p, _)| p.id.as_str()).collect::<Vec<_>>(), ["helper", "deck"], "pets with a picture keep it");
    }

    #[test]
    fn sets_go_to_the_servers_categories() {
        let cats = ["Legacy Companions".to_owned(), "Fallen Spirits".to_owned(), "Exclusive Companions".to_owned()];
        assert_eq!(category_for("Legacy Pets", &cats), "Legacy Companions");
        assert_eq!(category_for("fallen spirits", &cats), "Fallen Spirits");
        assert_eq!(category_for("Special Pets", &cats), "Special Pets");
        assert_eq!(add_label(2, 1), "Add 2 pets and 1 picture");
    }
}
