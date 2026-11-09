//! The public page: the paste box, then the review.
//!
//! The review leads with the answer: how far the account is overall with one bar per world,
//! then "Not started yet" and "Should have by now" (what the ranking expects by your world), then every mechanic by world (tabs switched in the page, no reload), each a
//! card that opens to its items, least done first. Worlds wear their own colour (data colour,
//! not decoration): the bars, the meters and the world tags.

use bot_core::Ctx;
use bot_core::maud::{DOCTYPE, Markup, PreEscaped, html};
use bot_core::web::ui;

use crate::expected::Pace;
use crate::game::GAME;
use crate::pictures;
use crate::review::{Count, Item, Mechanic, Packs, Review, State as Step, Tier};

/// Who's looking, as the closed test sees it.
pub struct Access {
    /// The page is in closed testing.
    pub closed: bool,
    /// This viewer may use it (always, when it isn't closed).
    pub allowed: bool,
    /// A dashboard user with this page.
    pub staff: bool,
    /// The name of whoever signed in with Discord (on this page or the dashboard).
    pub member: Option<String>,
    /// May open a test account (everything at 0): staff, and the local preview.
    pub test: bool,
}

pub enum State<'a> {
    /// The paste box, with what went wrong. `retry`: the paste was fine and only has to be sent
    /// again (the page was busy), so the box gets it back.
    Ask { error: Option<&'a str>, retry: bool },
    Done(&'a Review),
}

/// The page in a sentence, for link previews.
const ABOUT: &str = "Paste your Idleon save and see how far each mechanic is from the game's max, and what to work on next.";

pub const WORLD_NAMES: [&str; 8] = ["Account", "Blunder Hills", "Yum-Yum Desert", "Frostbite Tundra", "Hyperion Nebula", "Smolderin' Plateau", "Spirited Valley", "Shimmerfin Deep"];

fn world_tag(w: u8) -> String {
    if w == 0 { "All".into() } else { format!("W{w}") }
}

pub fn render(ctx: &Ctx, guild: u64, access: &Access, state: State) -> Markup {
    let name = ctx.guild_name(guild);
    let icon = ctx.guild_icon_large(guild);
    let banner = ctx.guild_banner_url(guild);
    let frame = bot_core::web::banner::frame(ctx, guild);
    let initial: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    let title = match &state {
        State::Done(r) if r.test => "Test account".to_string(),
        State::Done(r) if !r.name.is_empty() => format!("{}'s account", r.name),
        _ => "Account check".to_string(),
    };
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                (ui::head(&title))
                // What a link to the page shows when it's posted in Discord.
                meta name="description" content=(ABOUT);
                meta property="og:title" content={ "Idleon account check · " (name) };
                meta property="og:description" content=(ABOUT);
                @if let Some(i) = &icon { meta property="og:image" content=(i); }
            }
            body.ia-pub data-guild=(guild) data-icon=[icon.as_deref()] data-hue="pets" {
                style { (PreEscaped(CSS)) }
                header.ov-hero.ia-hero {
                    div.ov-bg {
                        @match (&banner, &icon) {
                            (Some(b), _) => img src=(b) alt="" style=[frame.map(|f| f.style())];,
                            (None, i) => (ui::art(i.as_deref(), false)),
                        }
                    }
                    div.ov-hello {
                        @match &icon {
                            Some(i) => img.ov-icon src=(i) alt="";,
                            None => span.ov-icon.letter { (initial) },
                        }
                        div {
                            h1 { (title) }
                            p { span { (name) } span { "Idleon account check" } }
                        }
                    }
                }
                main.ia-page {
                    @if !access.allowed {
                        (closed_door(guild, access))
                    } @else {
                        @if access.closed {
                            p.ia-test role="note" {
                                b { "Closed test." }
                                " This is a test version: some numbers or advice may be off. If something looks wrong, use Report a problem at the bottom of your results."
                            }
                        }
                        @match state {
                            State::Ask { error, retry } => (ask(guild, error, retry, access.test)),
                            State::Done(r) => {
                                (done(guild, r))
                                (report_box(guild, r, access.member.as_deref()))
                            },
                        }
                    }
                    footer.ia-foot {
                        span { "Maxes and names come from the game's own data, from " (GAME.updated) ". Your save is read once and not kept. This tab remembers the last one until you close it." }
                    }
                }
                script { (PreEscaped(JS)) }
            }
        }
    }
}

/// The page itself, for a sign-in to come back to.
fn here(guild: u64) -> String {
    format!("/p/{guild}/idleon")
}

/// "Signed in as Mira · Sign out": the sign-out is a form, so a link elsewhere can't do it.
fn signed_in_as(guild: u64, name: &str) -> Markup {
    html! {
        form.ia-signout method="post" action="/auth/signout" {
            input type="hidden" name="to" value=(here(guild));
            span { "Signed in as " b { (name) } }
            button type="submit" { "Sign out" }
        }
    }
}

/// The page while it's closed, for someone who isn't a tester.
fn closed_door(guild: u64, a: &Access) -> Markup {
    html! {
        section.ia-panel.ia-door {
            h2 { "The account check is in closed testing" }
            @if let Some(name) = &a.member {
                p { "Only staff and testers can use it for now. Ask staff if you'd like to help test it." }
                (signed_in_as(guild, name))
            } @else {
                p { "Staff and testers can try it now; everyone else soon. Testers, sign in with Discord to use it." }
                a.btn.primary href=(bot_core::web::auth::member_sign_in(&here(guild))) { "Sign in with Discord" }
            }
        }
    }
}

/// The longest report text: the suggestion box's limit less the longest header the report gets
/// ("Account check report (40 characters), 60 characters:").
const REPORT_TEXT: usize = 1370;

/// "Report a problem": a note to staff about what looks wrong, sent to the suggestion box.
fn report_box(guild: u64, r: &Review, member: Option<&str>) -> Markup {
    html! {
        section.ia-panel.ia-report #ia-report {
            div.ia-ask-text {
                h2 { "Report a problem" }
                p { "A number that's wrong, advice that doesn't make sense, something missing: tell staff here. It goes to the suggestion box with your name, and says which account and card it's about." }
                @if let Some(name) = member { (signed_in_as(guild, name)) }
            }
            @if member.is_none() {
                // Sending it needs a name to send it under: sign in with Discord (any member can;
                // the page comes back with your results), or copy it for /suggest.
                form.ia-report-copy data-account=(r.name) {
                    p.ia-note { "Sign in with Discord to send it from here, or copy it and send it with /suggest in the server." }
                    label.ia-report-card {
                        span { "Card (optional)" }
                        select name="card" {
                            option value="" { "Anything" }
                            @for m in &r.mechanics { option value=(m.name) { (m.name) } }
                        }
                    }
                    textarea name="text" rows="4" maxlength=(REPORT_TEXT) required placeholder="What looks wrong, and what you expected" {}
                    div.ia-ask-actions {
                        span.ia-report-msg role="status" {}
                        button.btn type="submit" { "Copy for /suggest" }
                        a.btn.primary.ia-sign-in href=(bot_core::web::auth::member_sign_in(&format!("{}?from=signin", here(guild)))) { "Sign in with Discord to send" }
                    }
                }
            } @else {
            form #ia-report-send method="post" action={ "/p/" (guild) "/idleon/report" } data-full {
                input type="hidden" name="account" value=(r.name);
                label.ia-report-card {
                    span { "Card (optional)" }
                    select name="card" {
                        option value="" { "Anything" }
                        @for m in &r.mechanics { option value=(m.name) { (m.name) } }
                    }
                }
                textarea name="text" rows="4" maxlength=(REPORT_TEXT) required placeholder="What looks wrong, and what you expected" {}
                div.ia-ask-actions {
                    span.ia-report-msg role="status" {}
                    button.btn.primary type="submit" { "Send to staff" }
                }
            }
            }
        }
    }
}

fn ask(guild: u64, error: Option<&str>, retry: bool, test: bool) -> Markup {
    html! {
        section.ia-panel.ia-ask {
            div.ia-ask-text {
                h2 { "Paste your save" }
                p { "Copy your account's JSON from IdleonToolbox or Idleon Efficiency and paste it here, or open the file. You get how far each mechanic is from the game's max, and what to work on next." }
            }
            // Sent as plain text: URL-encoding a save triples its size and was a fifth of the
            // server's work per check.
            form method="post" action={ "/p/" (guild) "/idleon" } enctype="text/plain" data-full {
                // First in the form so it's the first line of the plain-text body (the server
                // reads it before the save); shown under the box (CSS order). The page script
                // remembers the choice in this browser.
                div.ia-packs role="radiogroup" aria-label="Packs" {
                    span { "Packs" }
                    @for (p, tip) in [
                        (Packs::Buy, "Every tier counts everything."),
                        (Packs::NoMore, "Leaves out what only a pack you don't own gives: the Potluck Pack's vault levels, pack-only Gallery items."),
                        (Packs::Free, "That, and the Gem Shop leaves Should have by now."),
                    ] {
                        label title=(tip) {
                            input type="radio" name="packs" value=(p.code()) checked[p == Packs::Buy];
                            span { (p.label()) }
                        }
                    }
                }
                textarea #ia-save name="save" rows="7" spellcheck="false" autocomplete="off" autocapitalize="off" autocorrect="off" placeholder="{\"data\": {...}, \"charNames\": [...]}" required {}
                p.ia-err #ia-err role="alert" hidden[error.is_none()] data-retry[retry] { (error.unwrap_or_default()) }
                p.ia-note #ia-last hidden { "Filled in with the save you last checked in this tab." }
                div.ia-ask-actions {
                    label.btn {
                        (ui::icon(bot_core::Icon::Enter))
                        "Open a file"
                        input #ia-file type="file" accept=".json,.txt,application/json" data-max=(crate::save::MAX_BYTES) hidden;
                    }
                    // Shown by the page script where the browser can read the clipboard: on a
                    // phone, pressing and holding in the box to paste a megabyte is the hard part.
                    button.btn #ia-paste type="button" hidden { "Paste from clipboard" }
                    button.btn.primary type="submit" { "Check my account" }
                }
            }
            @if test {
                p.ia-try {
                    span { "Test account, everything at 0, seen as a player in" }
                    @for w in 1..=7u8 { a href={ "/p/" (guild) "/idleon/test?world=" (w) } { "World " (w) } }
                }
            }
        }
    }
}

fn done(guild: u64, r: &Review) -> Markup {
    let worlds: Vec<u8> = (1..=7).chain([0]).filter(|w| r.mechanics.iter().any(|m| m.world == *w)).collect();
    let start = worlds.iter().copied().filter(|w| (1..=r.world).contains(w)).max().unwrap_or(1);
    html! {
        @if r.test {
            p.ia-test role="note" {
                b { "Test account." }
                " Everything is at 0, as a World " (r.world) " player would see the page. Paste a real save to check one."
            }
        }
        section.ia-panel.ia-lead {
            div.ia-lead-who {
                b.ia-big { (pct(r.share())) }
                span { "of the maxes this check reads, across " (r.mechanics.iter().filter(|m| m.world <= r.world && m.total() > 0.0).count()) " mechanics" }
                p.ia-lead-line {
                    "World " (r.world) " · " (r.characters) " characters"
                    @if r.packs != Packs::Buy { " · " (r.packs.label()) }
                    " · " a.ia-again href={ "/p/" (guild) "/idleon" } { "Check another account" }
                }
            }
            div.ia-cols role="img" aria-label="How far each world is from max" {
                @for w in (1..=7u8).chain([0]) {
                    @let share = r.world_share(w);
                    @let reached = w == 0 || w <= r.world;
                    div.ia-col.off[!reached || share.is_none()] style={ "--w: var(--w" (w) ")" } data-tip={ (WORLD_NAMES[w as usize]) ": " (share.filter(|_| reached).map_or("not reached".into(), pct)) } {
                        span.ia-col-track { span.ia-col-fill style={ "--s:" (share.filter(|_| reached).unwrap_or(0.0)) } {} }
                        b { (share.filter(|_| reached).map_or("·".into(), |s| format!("{:.0}", s * 100.0))) }
                        span { (world_tag(w)) }
                    }
                }
            }
        }
        // Above Should have by now: a whole mechanic untouched is the biggest gap, and the list is
        // short. Only the first few up front: a returning player can have twenty-odd, one per line
        // on a phone.
        @if !r.not_started.is_empty() {
            section.ia-panel {
                div.ia-head {
                    h2 { "Not started yet" }
                    p { "In worlds you've reached, with no progress at all. New since you last played, or skipped" }
                }
                div.ia-fresh {
                    @for &i in r.not_started.iter().take(FRESH) { (fresh(&r.mechanics[i])) }
                }
                @if r.not_started.len() > FRESH {
                    details.ia-fresh-more {
                        summary { "Show " (r.not_started.len() - FRESH) " more" }
                        div.ia-fresh {
                            @for &i in r.not_started.iter().skip(FRESH) { (fresh(&r.mechanics[i])) }
                        }
                    }
                }
            }
        }
        (should_have(r))
        section.ia-worlds {
            div.ia-tabs role="tablist" {
                @for w in &worlds {
                    label style={ "--w: var(--w" (w) ")" } {
                        input #{ "w-" (w) } type="radio" name="ia-world" value=(w) checked[*w == start];
                        span.dot {}
                        (if *w == 0 { "Account".to_string() } else { format!("World {w}") })
                        @if *w > r.world && *w != 0 { span.ia-tab-note { "not reached" } }
                    }
                }
            }
            (items_data(r))
            @for w in &worlds {
                div.ia-world data-w=(w) style={ "--w: var(--w" (w) ")" } {
                    div.ia-grid {
                        @for m in r.mechanics.iter().filter(|m| m.world == *w && card_key(m.key) == m.key) {
                            (card(m, &folded(r, m.key), r.world))
                        }
                    }
                }
            }
        }
    }
}

/// Not started chips shown before "Show N more".
const FRESH: usize = 8;


/// Boxes Should have by now shows, oldest first: two rows of four on a wide screen, never a
/// third (narrower screens show two rows of what fits, see the CSS).
const OWED_BOXES: usize = 8;

/// Should have by now: a box per stage (Early, Mid and Late of each world), oldest first, side
/// by side like the cards, and in each a line per mechanic: the tier you're on and the tier that
/// stage asks for. A line opens its card; a long box scrolls inside itself. Past Late World 7,
/// the endgame boxes (Endgame 1, 2, Min-max) are goals from the best saves, not counted as owed.
fn should_have(r: &Review) -> Markup {
    let stages = r.should_have();
    if stages.is_empty() {
        return html! {};
    }
    let total: usize = stages.iter().filter(|(s, _)| !crate::crowd::endgame(*s)).map(|(_, v)| v.len()).sum();
    let goals = stages.iter().any(|(s, _)| crate::crowd::endgame(*s));
    html! {
        section.ia-owe {
            div.ia-head {
                h2 { "Should have by now" }
                p {
                    (total) " things players have by your world and you don't, oldest first. "
                    @if goals { "Then the endgame: steps toward the best of every save we have, and Min-max, the best itself. " }
                    "Click one to open its card."
                }
            }
            div.ia-owe-grid {
                @for (stage, v) in stages.iter().take(OWED_BOXES) {
                    @let w = crate::crowd::stage_world(*stage);
                    // A mechanic's items at this stage, in the order the list ranks them.
                    @let rows = {
                        let mut rows: Vec<Vec<&crate::review::Owed>> = Vec::new();
                        for o in v {
                            match rows.iter_mut().find(|g| g[0].mechanic == o.mechanic) {
                                Some(g) => g.push(o),
                                None => rows.push(vec![o]),
                            }
                        }
                        rows
                    };
                    div.ia-owe-box style={ "--w: var(--w" (w) ")" } {
                        div.ia-owe-head {
                            b { (crate::crowd::stage_label(*stage)) }
                            @if crate::crowd::endgame(*stage) { span.ia-chip.goal { "Goal" } }
                            @else if w < r.world { span.ia-chip.miss { "Missed" } } @else { span.ia-chip.next { "Next up" } }
                            span.ia-owe-n { (v.len()) (if v.len() == 1 { " thing" } else { " things" }) }
                        }
                        div.ia-owe-lines { @for g in &rows { (owed(r, g)) } }
                    }
                }
            }
        }
    }
}

/// One mechanic's line at a stage, a link that opens its card: the tier you're on and the one
/// the stage asks for; what's missing on hover. When none of it can move yet, a line under it
/// on what it waits for (every locked item says), named when there are several.
fn owed(r: &Review, g: &[&crate::review::Owed]) -> Markup {
    let m = &r.mechanics[g[0].mechanic];
    let here = m.tiers().iter().position(|t| !t.complete()).unwrap_or(0) + 1;
    let want = g.iter().map(|o| o.tier).max().unwrap_or(0) + 1;
    let items: Vec<&Item> = g.iter().map(|o| &m.items[o.item]).collect();
    let need = items.iter().all(|i| i.state == Step::Locked).then(|| items[0].how.as_deref()).flatten().map(|h| match items.len() {
        1 => h.to_string(),
        _ => format!("{}: {h}", items[0].name),
    });
    let names = items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(", ");
    html! {
        a.ia-owe-line href={ "#m-" (card_key(m.key)) } data-go={ "m-" (card_key(m.key)) } data-side=[side_of(m.key)] data-w=(m.world) title={ (items.len()) " left: " (names) } {
            (mark(m))
            span.ia-owe-name {
                b { (m.name) }
                @if let [i] = items.as_slice() { span { " · " (i.name) } }
            }
            span.ia-owe-t { @if want > here { "T " (here) " ▸ T " (want) } @else { "T " (want) } }
            @if let Some(n) = &need { span.ia-owe-need { (n) } }
        }
    }
}

/// A Not started chip: opens its card.
fn fresh(m: &Mechanic) -> Markup {
    html! {
        a.ia-fresh-item href={ "#m-" (card_key(m.key)) } data-go={ "m-" (card_key(m.key)) } data-side=[side_of(m.key)] data-w=(m.world) style={ "--w: var(--w" (m.world) ")" } {
            (mark(m))
            span { (m.name) }
            span.ia-tag { (world_tag(m.world)) }
        }
    }
}

/// Every card's full item list as compact data, for the page script to draw when a "Show
/// all" opens: per mechanic, the items not at max (in the card's order) and those at max, each
/// `[name, picture or null, share 0-1, figure text, 1 when it has a max, 1 when the picture is
/// drawn as its left square]`. Character names come from the paste, so the
/// JSON is made safe for a script tag and the script only ever sets text, never HTML.
fn items_data(r: &Review) -> Markup {
    use bot_core::serde_json::{Map, Value, json};
    let row = |i: &Item| {
        let pic = i.picture.as_deref().filter(|p| pictures::has(p));
        json!([i.name, pic, (i.share() * 1000.0).round() / 1000.0, item_fig(i), u8::from(i.max > 0.0), u8::from(pic.is_some_and(left_square))])
    };
    let mut all = Map::new();
    for m in &r.mechanics {
        let open: Vec<Value> = m.open_items().into_iter().map(row).collect();
        let maxed: Vec<Value> = m.items.iter().filter(|i| i.done()).map(row).collect();
        all.insert(m.key.to_string(), json!([open, maxed]));
    }
    let text = Value::Object(all).to_string().replace('<', "\\u003c").replace('>', "\\u003e").replace('&', "\\u0026");
    html! { script #ia-items type="application/json" { (PreEscaped(text)) } }
}

/// What an item's row says on the right: its named level, "17 / 40", or "Lv 17".
fn item_fig(i: &Item) -> String {
    match &i.level_name {
        Some(n) => n.clone(),
        None if i.max > 0.0 => format!("{} / {}", num(i.level), num(i.max)),
        None => format!("Lv {}", num(i.level)),
    }
}

/// Where a mechanic stands against its tiers' worlds, in a line: what world it's at, and the
/// first tier overdue for the world you're in.
fn pace_line(p: &Pace, reached: u8) -> Option<String> {
    let at = p.at.map(|w| format!("Where a World {w} player would be."));
    match (at, p.due.and_then(|k| p.worlds[k].map(|w| (k, w)))) {
        (at, Some((k, w))) => Some(format!("{}Tier {} is expected by World {w}, and you're in World {reached}.", at.map(|a| a + " ").unwrap_or_default(), k + 1)),
        (Some(at), None) => Some(format!("{at} Nothing here is overdue for World {reached}.")),
        (None, None) => p.worlds.iter().flatten().min().map(|w| format!("Its first tier is expected by World {w}.")),
    }
}

/// Mechanics drawn as one card behind a switch, like Stamps' Levels / Unlock (owner,
/// 2026-11-08: Cauldron Upgrades and Pay2Win, "fold that in"): per card, each side's mechanic
/// and the side's name, the card's own mechanic first, then the card's title ("" keeps the
/// first mechanic's name). The card sits in its first mechanic's world tab. A side whose
/// mechanic the save doesn't have is left out.
const FOLD: &[(&[(&str, &str)], &str)] = &[
    (&[("cauldron-upgrades", "Brewing"), ("pay2win", "Pay2Win")], ""),
    (&[("islands", "Islands"), ("fractal-island", "Fractal Island")], "Islands"),
    // World 5 and 6 (2026-11-09): one card per skill, as the game draws it.
    (&[("artifacts", "Artifacts"), ("sailing-islands", "Islands"), ("boats", "Boats"), ("boat-upgrades", "Upgrades")], "Sailing"),
    (&[("blessings", "Blessings"), ("gods", "Gods")], "Divinity"),
    (&[("superbits", "Superbits"), ("snail", "Snail")], "Gaming"),
    (&[("cavern-villagers", "Villagers"), ("cavern-schematics", "Schematics"), ("cavern-majiks", "Majiks"), ("cavern-measurements", "Measurements"), ("cavern-studies", "Studies")], "The Hole"),
    (&[("cavern-shallow", "Shallow"), ("cavern-glowshroom", "Glowshroom"), ("cavern-overgrowth", "Overgrowth")], "Caverns"),
    (&[("crops", "Crops"), ("farm-market", "Market"), ("land-rank", "Land Rank"), ("exotic-market", "Exotic")], "Farming"),
    (&[("ninja-upgrades", "Upgrades"), ("jade-emporium", "Emporium"), ("pristine-charms", "Charms"), ("gemstones", "Gemstones")], "Sneaking"),
    (&[("summoning", "Upgrades"), ("summoning-battles", "Battles"), ("summoning-stones", "Stones")], "Summoning"),
    // World 3 and 4 (2026-11-09).
    (&[("equinox-upgrades", "Upgrades"), ("equinox-dreams", "Dreams")], "Equinox"),
    (&[("meals", "Meals"), ("kitchens", "Kitchens")], "Cooking"),
    (&[("jewels", "Jewels"), ("lab-chips", "Chips")], "Lab"),
    (&[("breeding-pets", "Pets"), ("territories", "Territories"), ("breeding-upgrades", "Upgrades"), ("shiny", "Shiny")], "Breeding"),
    (&[("rift", "Rifts"), ("rift-bonuses", "Bonuses")], "Rift"),
    (&[("worship", "Totems"), ("worship-skulls", "Skulls")], "Worship"),
];

/// Most sides a switch has (the page's CSS has a rule per side).
const SIDES: usize = 6;

/// The key of the card a mechanic is drawn in: its own, or the one it's folded into.
fn card_key(key: &str) -> &str {
    FOLD.iter().find(|f| f.0[1..].iter().any(|s| s.0 == key)).map_or(key, |f| f.0[0].0)
}

/// For a folded mechanic, the side of its card's switch it's on (links open that side).
fn side_of(key: &str) -> Option<usize> {
    FOLD.iter().find_map(|f| f.0.iter().position(|s| s.0 == key).filter(|&k| k > 0))
}

/// The other mechanics folded into `key`'s card, with their sides' names.
fn folded<'a>(r: &'a Review, key: &str) -> Vec<(&'static str, &'a Mechanic)> {
    FOLD.iter()
        .find(|f| f.0[0].0 == key)
        .map(|f| f.0[1..].iter().filter_map(|s| r.mechanics.iter().find(|p| p.key == s.0).map(|p| (s.1, p))).collect())
        .unwrap_or_default()
}

fn card(m: &Mechanic, partners: &[(&'static str, &Mechanic)], reached: u8) -> Markup {
    let tiers = m.tiers();
    let current = tiers.iter().position(|t| !t.complete());
    let hold = ON_HOLD.contains(&m.key);
    let fold = (!partners.is_empty()).then(|| FOLD.iter().find(|f| f.0[0].0 == m.key)).flatten();
    let title = fold.map(|f| f.1).filter(|t| !t.is_empty()).unwrap_or(m.name);
    // Sides behind a switch: statues and stamps show the game's tiers and a ladder on levels;
    // a folded card shows its mechanics. (label, the side's mechanic)
    let split: Option<Vec<(&str, Mechanic)>> = match fold {
        // A side with two ladders of its own (Statues: Levels / Tiers) brings both.
        Some(f) => Some(
            std::iter::once((f.0[0].1, m))
                .chain(partners.iter().map(|(l, p)| (*l, *p)))
                .flat_map(|(l, x)| match x.levels_view().zip(x.tiers_view()) {
                    Some((lv, tv)) => vec![("Levels", lv), (x.other_side, tv)],
                    None => vec![(l, x.clone())],
                })
                .take(SIDES)
                .collect(),
        ),
        None => m.levels_view().zip(m.tiers_view()).map(|(lv, tv)| vec![("Levels", lv), (m.other_side, tv)]),
    };
    let all = || std::iter::once(m).chain(partners.iter().map(|p| p.1));
    let (have, total) = all().fold((0.0, 0.0), |(h, t), x| (h + x.have(), t + x.total()));
    let share = if fold.is_some() && total > 0.0 { (have / total).clamp(0.0, 1.0) } else { m.share() };
    let done = if fold.is_some() { all().all(|x| x.done()) } else { m.done() };
    html! {
        details.ia-card #{ "m-" (m.key) } {
            summary {
                (mark(m))
                span.ia-card-name {
                    b { (title) }
                    // The tier you're on, numbered as the ladder numbers it ("Tier 3 · you're here").
                    span {
                        @if fold.is_some() { (fig(m)) } @else if let Some((_, tv)) = split.as_ref().and_then(|s| s.last()) { (ladder_fig(tv)) } @else { (fig(m)) }
                        @match current {
                            Some(k) if fold.is_some() => { " · Tier " (k + 1) "/" (tiers.len()) },
                            Some(k) if split.is_some() => { " · Level tier " (k + 1) "/" (tiers.len()) },
                            Some(k) => { " · Tier " (k + 1) "/" (tiers.len()) },
                            None if !tiers.is_empty() => { " · All tiers" },
                            None => {},
                        }
                        @if let Some(p) = &m.pace {
                            @match (p.at, p.worlds.iter().flatten().min()) {
                                (Some(w), _) => { " · World " (w) " level" },
                                (None, Some(f)) if *f <= reached => { " · Under World " (f) " level" },
                                _ => {},
                            }
                        }
                    }
                }
                @if m.count == Count::Open {
                    span.ia-pct.open { "No max" }
                } @else {
                    span.ia-pct.done[done] { (pct(share)) }
                    span.ia-meter.wide { span style={ "--s:" (share3(share)) } {} }
                }
            }
            div.ia-body {
                @if hold {
                    p.ia-hold { b { "Order coming later." } " The numbers here are right, but these tiers aren't in the order you'd really do them yet. That comes in a later update." }
                }
                @if let Some(sides) = &split {
                    div.ia-split {
                        div.ia-switch role="radiogroup" aria-label="Show" {
                            @for (k, (label, side)) in sides.iter().enumerate() {
                                label {
                                    input type="radio" name={ "v-" (m.key) } value=(k) checked[k == 0];
                                    b { (label) }
                                    span.ia-fig { (ladder_fig(side)) }
                                }
                            }
                        }
                        @for (k, (_, side)) in sides.iter().enumerate() {
                            div.ia-view data-v=(k) { (view(side, reached, &k.to_string())) @if fold.is_some() { (all_items(side)) } }
                        }
                    }
                } @else {
                    div.ia-view { (view(m, reached, "")) }
                }
                @if !m.later.is_empty() {
                    p.ia-note.ia-later-worlds {
                        "Not counted until you get there: "
                        @for (n, i) in m.later.iter().take(6).enumerate() {
                            @if n > 0 { ", " }
                            (i.name) " (World " (i.world.unwrap_or(0)) ")"
                        }
                        @if m.later.len() > 6 { " and " (m.later.len() - 6) " more" }
                        "."
                    }
                }
                @if fold.is_none() { (all_items(m)) }
            }
        }
    }
}

/// A mechanic's full item list behind "Show all", drawn by the page script from the page's
/// item data when first opened: these lists were three quarters of the page and most are
/// never opened.
fn all_items(m: &Mechanic) -> Markup {
    html! {
        @if !m.items.is_empty() {
            details.ia-all {
                summary { "Show all " (m.items.len()) }
                div.ia-items data-key=(m.key) {}
            }
        }
    }
}

/// A card's body under its head: the notes, the ladder, the current tier's tasks and the side
/// panels. Drawn once, or once per side of a switch (`Mechanic::levels_view`, `tiers_view`).
fn view(m: &Mechanic, reached: u8, tag: &str) -> Markup {
    let open = m.open_items();
    let tiers = m.tiers();
    let current = tiers.iter().position(|t| !t.complete());
    // What you hold and what raises the cap only matter while there's a tier to work on.
    let side = current.is_some() && (m.have.is_some() || !m.raises.is_empty());
    html! {
        @if let Some(n) = &m.note { p.ia-note { (n) } }
        @if let Some(line) = m.pace.as_ref().and_then(|p| pace_line(p, reached)) { p.ia-pace { (line) } }
        @if m.count == Count::Open {
            @if let Some([avg, median, top, low]) = m.spread() {
                div.ia-spread {
                    div { b { (num(avg.round())) } span { "Average" } }
                    div { b { (num(median.round())) } span { "Median" } }
                    div { b { (num(top)) } span { "Highest" } }
                    div { b { (num(low)) } span { "Lowest" } }
                }
            }
        }
        @if tiers.is_empty() {
            div.ia-tasks {
                h3 { (m.open_order.unwrap_or("Lowest first")) }
                @if m.open_order.is_none() { p.ia-note { "Nothing here has a max, so these are the ones furthest behind." } }
                div.ia-task-list { @for i in open.iter().take(TASKS) { (item(i)) } }
            }
        } @else {
            (ladder(m, &tiers, current, reached, tag))
        }
        @if side {
            aside.ia-side {
                @if let Some(h) = &m.have {
                    div.ia-side-box {
                        span.ia-side-k { "With what you have" }
                        b.ia-side-big { (h.figure) }
                        p { (h.text) }
                    }
                }
                @if !m.raises.is_empty() {
                    div.ia-side-box {
                        span.ia-side-k { "Raise the cap" }
                        // The ones still to get first: those are the ones to act on.
                        @for r in m.raises.iter().filter(|r| !r.owned).chain(m.raises.iter().filter(|r| r.owned)) {
                            div.ia-raise.owned[r.owned] {
                                span.ia-raise-mark title=(if r.owned { "You have it" } else { "Not yet" }) {
                                    @if r.owned { (ui::icon(bot_core::Icon::Check)) }
                                }
                                span.ia-raise-text {
                                    b { (r.name) }
                                    span { (r.amount) }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Stage rows a ladder shows before "N more stages".
const STAGE_ROWS: usize = 12;

/// Which stage rows show: all when they fit, else `STAGE_ROWS` from a few before the one
/// picked. `(from, to)`, `to` exclusive.
fn stage_window(len: usize, here: usize) -> (usize, usize) {
    if len <= STAGE_ROWS {
        return (0, len);
    }
    let from = here.saturating_sub(3).min(len - STAGE_ROWS);
    (from, from + STAGE_ROWS)
}

/// The world colour of a stage row: its world ("Late World 4"), World 7 past it, otherwise
/// the mechanic's own.
fn stage_world(name: &str, world: u8) -> u8 {
    match name.split_once("World ").and_then(|(_, w)| w.trim().parse::<u8>().ok()) {
        Some(w) => w,
        None if name.starts_with("Endgame") || name.starts_with("Min-max") => 7,
        None => world,
    }
}

/// Tier layout B (Relay design 23, owner 2026-11-09): the stages down the side, a row each
/// with a dash per tier, and the picked tier beside them, a page of its asks with steps to the
/// tier before and after. Every tier's page is in the card; a radio per tier picks one, no
/// script needed (the script only moves the highlight in the stage list).
fn ladder(m: &Mechanic, tiers: &[Tier], current: Option<usize>, reached: u8, tag: &str) -> Markup {
    let id = |k: usize| format!("t-{}-{}-{}", m.key, tag, k);
    let pick = current.unwrap_or(tiers.len() - 1);
    // Stages: runs of tiers named alike before " · " ("Late World 1 · 2 of 4").
    let mut groups: Vec<(&str, Vec<usize>)> = Vec::new();
    for (k, t) in tiers.iter().enumerate() {
        let name = t.label.split(" · ").next().unwrap_or(&t.label);
        match groups.last_mut() {
            Some(g) if g.0 == name => g.1.push(k),
            _ => groups.push((name, vec![k])),
        }
    }
    let here = groups.iter().position(|g| g.1.contains(&pick)).unwrap_or(0);
    let (from, to) = stage_window(groups.len(), here);
    let cut = from > 0 || to < groups.len();
    html! {
        div.ia-b {
            div.ia-stages {
                @if cut { input.ia-allst type="checkbox" id={ "s-" (m.key) "-" (tag) } hidden; }
                @for (g, (name, ks)) in groups.iter().enumerate() {
                    @let first = ks.iter().copied().find(|&k| !tiers[k].complete()).unwrap_or(ks[0]);
                    div.ia-st.done[ks.iter().all(|&k| tiers[k].complete())].picked[g == here].far[g < from || g >= to] style={ "--w: var(--w" (stage_world(name, m.world)) ")" } {
                        label.ia-st-name for=(id(first)) { span.ia-st-dot {} span { (name) } }
                        span.ia-dashes {
                            @for &k in ks {
                                label.ia-dash.done[tiers[k].complete()].here[current == Some(k)].picked[k == pick] for=(id(k)) data-k=(k) title={ "Tier " (k + 1) } {}
                            }
                        }
                    }
                }
                @if cut {
                    label.ia-st-more for={ "s-" (m.key) "-" (tag) } {
                        span.more {
                            @if to < groups.len() { (groups.len() - to) " more stages, to " (groups[groups.len() - 1].0) }
                            @else { "Show the " (from) " stages before" }
                        }
                        span.less { "Show fewer stages" }
                    }
                }
            }
            div.ia-peeks {
                @for k in 0..tiers.len() {
                    input.ia-pick type="radio" name={ "t-" (m.key) "-" (tag) } id=(id(k)) data-k=(k) checked[k == pick] aria-label={ "Tier " (k + 1) ", " (tiers[k].label) };
                    div.ia-peek { (peek(m, tiers, k, current, reached, &id)) }
                }
            }
        }
    }
}

/// Where a side of the switch stands, for its label and the card head: "Onyx 23 / 32",
/// "Tier 10 / 24", "86 / 120 unlocked", "All tiers".
fn ladder_fig(m: &Mechanic) -> String {
    let tiers = m.tiers();
    if m.crowd.is_none() && m.tier_names.is_none() {
        return format!("{} / {} {}", num(m.have()), num(m.total()), m.unit);
    }
    match tiers.iter().position(|t| !t.complete()) {
        Some(k) if m.crowd.is_none() => format!("{} {} / {}", tiers[k].label, tiers[k].met, tiers[k].need),
        Some(k) => format!("Tier {} / {}", k + 1, tiers.len()),
        None => "All tiers".into(),
    }
}

/// Mechanics whose tiers aren't in a real order yet (owner, 2026-11-06): their cards say so.
const ON_HOLD: &[&str] = &[
    "post-office",
    "meals",
    "lab-chips",
    "shiny",
];

/// Tasks shown under the current tier before "and N more".
const TASKS: usize = 8;

/// A level as the mechanic names it: "Gold" for a statue, "Lv 17" otherwise.
fn level_text(m: &Mechanic, level: f64) -> String {
    match &m.tier_names {
        Some(names) if level >= 1.0 => names.get(level as usize - 1).cloned().unwrap_or_else(|| format!("Lv {}", num(level))),
        Some(_) => "None".into(),
        None => format!("Lv {}", num(level)),
    }
}

/// What an ask's figure says: the level it asks ("Lv 40", "Unlock", "Gold") and, under it, where
/// it starts from ("from Lv 30"), when that says something.
fn ask_text(m: &Mechanic, i: &Item, target: f64) -> (String, Option<String>) {
    let once = i.max > 0.0 && i.max <= 1.0 && m.tier_names.is_none();
    let level = m.tier_level(i);
    if once {
        return (if i.raw.is_some() { "Unlock".into() } else { i.level_name.clone().unwrap_or_else(|| "Unlock".into()) }, None);
    }
    if m.crowd.is_some() {
        // Something not had at all (a stamp not found) is "Unlock": its level 0 says nothing.
        return match () {
            _ if level <= 0.0 && target <= 1.0 => ("Unlock".into(), None),
            _ if i.raw.is_some_and(|r| r <= 0.0) => (format!("Lv {}", num(target)), Some("Unlock first".into())),
            _ => (format!("Lv {}", num(target)), (level > 0.0).then(|| format!("from {}", num(level)))),
        };
    }
    (level_text(m, target), (i.level > 0.0).then(|| format!("from {}", level_text(m, i.level))))
}

/// One ask of a tier: the picture, the name and how to do it, then the level it asks with what
/// stands in the way under it (Do now, Locked, or where it starts).
fn task(m: &Mechanic, i: &Item, target: f64) -> Markup {
    let (to, from) = ask_text(m, i, target);
    html! {
        div.ia-task {
            @if let Some(p) = i.picture.as_deref().filter(|p| pictures::has(p)) {
                img.left[left_square(p)] src={ "/idleon/pic/" (p) ".png" } alt="" loading="lazy" style=[(i.state == Step::Locked).then_some("opacity:.55")];
            } @else { span.ia-item-dot {} }
            span.ia-task-text {
                b { (i.name) }
                @if let Some(h) = &i.how { span { (h) } }
            }
            span.ia-fig {
                b { (to) }
                @match i.state {
                    Step::Ready => span.ok { "Do now" },
                    Step::Locked => span.lock { "Locked" },
                    Step::Unknown => @if let Some(f) = from { span { (f) } },
                }
            }
        }
    }
}

/// One tier's page, shown when it's picked: its name with steps to the tiers either side, a
/// line on where you stand, then everything it asks, what's left first.
fn peek(m: &Mechanic, tiers: &[Tier], k: usize, current: Option<usize>, reached: u8, id: &dyn Fn(usize) -> String) -> Markup {
    let t = &tiers[k];
    // A done count tier has nothing to list: which items counted isn't kept.
    let asks = if t.complete() && t.targets.is_none() { Vec::new() } else { m.tier_asks(tiers, k) };
    let left = asks.iter().filter(|a| !a.2).count();
    let ready = asks.iter().filter(|a| !a.2 && a.0.state == Step::Ready).count();
    let expected = m.pace.as_ref().and_then(|p| p.worlds.get(k).copied().flatten());
    html! {
        div.ia-peek-head {
            h3 { "Tier " (k + 1) " · " (t.label) }
            span.ia-steps {
                @if k > 0 { label.ia-step.back for=(id(k - 1)) title="Tier before" { (ui::icon(bot_core::Icon::Chevron)) } }
                @else { span.ia-step.back.off { (ui::icon(bot_core::Icon::Chevron)) } }
                @if k + 1 < tiers.len() { label.ia-step.next for=(id(k + 1)) title="Next tier" { (ui::icon(bot_core::Icon::Chevron)) } }
                @else { span.ia-step.next.off { (ui::icon(bot_core::Icon::Chevron)) } }
            }
        }
        p.ia-note {
            @if current == Some(k) { "You're here. " }
            @if t.complete() {
                "Done." @if current.is_none() && k + 1 == tiers.len() { " Every tier is done." } " "
            } @else if t.targets.is_none() {
                (t.need - t.met) " more"
                @if let Some(l) = t.at.filter(|l| *l > 1.0) { " at Lv " (num(l)) }
                ", any of these. "
            } @else if left == 0 {
                "Everything this tier adds is done; a tier before it isn't yet. "
            } @else {
                (left) " of " (asks.len()) " left"
                @if ready == left { ", all of them you can do now" } @else if ready > 0 { ", " (ready) " you can do now" }
                ". "
            }
            // The tier you're on has this in the line above the ladder already.
            @if let Some(w) = expected.filter(|_| current != Some(k)) { "Expected by World " (w) ", and you're in World " (reached) ". " }
            @if let Some(n) = m.tier_notes.get(k).filter(|n| !n.is_empty() && m.crowd.is_none()) { (n) }
        }
        @if left > 0 {
            div.ia-task-list {
                @for (i, target, _) in asks.iter().take(left.min(TASKS)) { (task(m, i, *target)) }
            }
            @if left > TASKS {
                details.ia-rest {
                    summary { "Show the other " (left - TASKS) }
                    div.ia-task-list { @for (i, target, _) in asks.iter().take(left).skip(TASKS) { (task(m, i, *target)) } }
                }
            }
        }
        // What's done in it: one quiet line of names, not a row each.
        @if left < asks.len() {
            p.ia-done-line {
                span.ia-tick { (ui::icon(bot_core::Icon::Check)) }
                span {
                    "Done: "
                    @for (n, (i, _, _)) in asks[left..].iter().enumerate() {
                        @if n > 0 { ", " }
                        (i.name)
                    }
                }
            }
        }
    }
}

fn item(i: &Item) -> Markup {
    html! {
        div.ia-item.done[i.done()] {
            @if let Some(p) = i.picture.as_deref().filter(|p| pictures::has(p)) {
                img.left[left_square(p)] src={ "/idleon/pic/" (p) ".png" } alt="" loading="lazy";
            } @else { span.ia-item-dot {} }
            span.ia-item-name { (i.name) }
            @if i.max > 0.0 { span.ia-meter { span style={ "--s:" (share3(i.share())) } {} } } @else { span {} }
            span.ia-fig {
                @match &i.level_name {
                    Some(n) => (n),
                    None if i.max > 0.0 => { (num(i.level)) " / " (num(i.max)) },
                    None => { "Lv " (num(i.level)) },
                }
            }
        }
    }
}

/// The mechanic's picture, or a dot in its world's colour.
fn mark(m: &Mechanic) -> Markup {
    html! {
        @match m.picture.as_deref().filter(|p| pictures::has(p)) {
            Some(p) => img.ia-mark.left[left_square(p)] src={ "/idleon/pic/" (p) ".png" } alt="";,
            None => span.ia-mark.blank {},
        }
    }
}

/// Pictures that are a portrait at the left end of a wide name plate (the cavern villagers):
/// drawn as their left square, which is the portrait. Show all rows carry it as a flag.
const LEFT_SQUARE: &[&str] = &["HoleUIvillager"];

fn left_square(id: &str) -> bool {
    LEFT_SQUARE.iter().any(|p| id.starts_with(p))
}

fn fig(m: &Mechanic) -> String {
    match m.count {
        Count::Levels => format!("{} / {} {}", num(m.have()), num(m.total()), m.unit),
        Count::Done => format!("{} of {} {}", num(m.have()), num(m.total()), m.unit),
        // A sum of levels with no max says little: the middle one says more.
        Count::Open => match m.spread() {
            Some([_, median, top, _]) => format!("median {} · highest {}", num(median.round()), num(top)),
            None => format!("{} {}", num(m.have()), m.unit),
        },
    }
}

/// A share for a bar's width, to three places: the bar can't show more.
fn share3(s: f64) -> String {
    format!("{:.3}", s.clamp(0.0, 1.0)).trim_end_matches('0').trim_end_matches('.').to_string()
}

fn pct(s: f64) -> String {
    let p = s * 100.0;
    // 99.6% shouldn't read as done.
    if (99.5..100.0).contains(&p) { "99%".into() } else { format!("{:.0}%", p) }
}

/// Whole numbers with thousands separators; anything else with one decimal. From a quadrillion
/// on, the game's short form ("1.2Qa", "3.4e21"): past 2^53 the digits aren't real, and past
/// 2^64 a plain cast reads 18,446,744,073,709,551,615.
pub(crate) fn num(n: f64) -> String {
    if n.is_nan() {
        return "0".into();
    }
    if n.is_infinite() || n.abs() >= 1e15 {
        return crate::checks::big(n);
    }
    if n.fract().abs() > 1e-9 {
        return format!("{n:.1}");
    }
    let s = format!("{}", n.abs() as u64);
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0.0 { format!("-{out}") } else { out }
}

/// The paste box's file picker, and cards that grow out of their place: when one opens it
/// spans the grid, so every card is measured before and after (FLIP). The opened one is
/// revealed from its old size rather than stretched and draws on top; cards that only moved up
/// or down slide; cards that changed column fade in where they land, since sliding them across
/// their neighbours reads as a double image. Should have by now
/// and Not started links switch the world, open the card the same way and scroll to it.
const JS: &str = r##"(function(){
var f=document.getElementById("ia-file"),t=document.getElementById("ia-save"),pb=document.getElementById("ia-paste"),er=document.getElementById("ia-err");
function say(s){if(er){er.textContent=s;er.hidden=false}}
if(f&&t)f.addEventListener("change",function(){var x=f.files&&f.files[0];if(!x)return;
  if(x.size>+f.dataset.max){say("That file is bigger than any Idleon save. Pick the JSON you saved from IdleonToolbox or Idleon Efficiency.");f.value="";return}
  x.text().then(function(s){t.value=s;t.form.requestSubmit()})});
// Paste from clipboard: one tap on a phone, and the megabyte never has to be drawn in the box.
if(pb&&t&&navigator.clipboard&&navigator.clipboard.readText){
  pb.hidden=false;
  pb.addEventListener("click",function(){
    navigator.clipboard.readText().then(function(s){
      if(!/^\s*\{/.test(s||"")){say("Your clipboard doesn't hold a save. Copy your account's JSON first, then try again.");return}
      t.value=s;t.form.requestSubmit();
    },function(){say("Your browser didn't let the page read the clipboard. Press and hold in the box and choose Paste instead.")});
  });
}
// Results arrive by POST. Made a plain visit, a reload (or a phone bringing a closed tab back)
// opens the paste box instead of asking to send the form again, and the save you last checked
// in this tab is filled in, to check again with one tap. It stays in this tab's own storage.
// Back from signing in to send a report (?from=signin): check the same save again and land on
// the report, with what you'd written put back.
var signin=/[?&]from=signin/.test(location.search);
// The rest of the address stays (the test account's ?world=).
try{history.replaceState(null,"",location.pathname+location.search.replace(/([?&])from=signin(&|$)/,function(m,a,b){return b?a:""})+location.hash)}catch(x){}
var LAST="ia-last-save";
// Packs: the choice on the paste box, remembered in this browser, and passed to the test
// account links.
var PK="ia-packs";
function packsLinks(v){document.querySelectorAll(".ia-try a").forEach(function(a){a.href=a.href.replace(/&packs=[^&]*/,"")+(v&&v!=="buy"?"&packs="+v:"")})}
try{var pv=localStorage.getItem(PK),pr=pv&&document.querySelector('input[name=packs][value="'+pv+'"]');if(pr){pr.checked=true;packsLinks(pv)}}catch(x){}
document.addEventListener("change",function(e){if(e.target.name!=="packs")return;try{localStorage.setItem(PK,e.target.value)}catch(x){}packsLinks(e.target.value)});
function keep(s){try{if(s)sessionStorage.setItem(LAST,s);else sessionStorage.removeItem(LAST)}catch(x){try{sessionStorage.removeItem(LAST)}catch(y){}}}
if(t){
  t.form.addEventListener("submit",function(){keep(t.value)});
  // After a paste that didn't read, there's nothing worth filling back in. After "busy, try
  // again" the paste was fine: it goes back in the box to send again.
  var failed=er&&!er.hidden;
  if(failed&&!er.hasAttribute("data-retry"))keep("");
  else{var last=null;try{last=sessionStorage.getItem(LAST)}catch(x){}
    if(last&&!t.value){t.value=last;var ln=document.getElementById("ia-last");if(ln&&!failed)ln.hidden=false}
    if(signin&&last&&!failed){t.form.action=location.pathname+"#ia-report";t.form.requestSubmit()}}
}
var DRAFT="ia-report-draft";
document.addEventListener("click",function(e){
  if(!e.target.closest("a.ia-sign-in"))return;
  var f=e.target.closest("form");
  try{if(f)sessionStorage.setItem(DRAFT,JSON.stringify({card:f.querySelector("select").value,text:f.querySelector("textarea").value}))}catch(x){}
});
(function(){
  var f=document.getElementById("ia-report-send")||document.querySelector(".ia-report-copy"),d=null;
  if(!f)return;
  try{d=JSON.parse(sessionStorage.getItem(DRAFT)||"null");sessionStorage.removeItem(DRAFT)}catch(x){}
  if(!d)return;
  f.querySelector("select").value=d.card||"";f.querySelector("textarea").value=d.text||"";
})();
document.addEventListener("click",function(e){if(e.target.closest("a.ia-again"))keep("")});
// On a phone the world tabs are one row that scrolls sideways: keep the chosen one in view.
function showTab(smooth){
  var s=document.querySelector(".ia-tabs"),i=s&&s.querySelector("input:checked");
  if(!i||s.scrollWidth<=s.clientWidth)return;
  var a=s.getBoundingClientRect(),b=i.parentElement.getBoundingClientRect();
  s.scrollBy({left:b.left+b.width/2-(a.left+a.width/2),behavior:smooth&&!still.matches?"smooth":"auto"});
}
document.addEventListener("change",function(e){if(e.target.name==="ia-world")showTab(true)});
var still=matchMedia("(prefers-reduced-motion: reduce)"),ease="cubic-bezier(.16,1,.3,1)";
// Bars draw in once, with the page. After that a bar that appears (a card opening, another
// world's tab) is just there, so nothing regrows while a card moves.
setTimeout(function(){document.documentElement.classList.add("ia-settled")},1200);
var busy=new Set(),easeIn="cubic-bezier(.4,0,1,1)";
function rects(cs){return cs.map(function(c){return c.getBoundingClientRect()})}
// After the grid changes shape: keep the clicked card where your eye is (scroll by however far
// the layout moved it), slide cards that only moved up or down, and let cards that changed
// column come up where they land (sliding them across their neighbours reads as a double image).
function settle(cs,was,d){
  var i=cs.indexOf(d),a=was[i],b=d.getBoundingClientRect(),shift=b.top-a.top;
  if(Math.abs(shift)>1)window.scrollBy(0,shift);
  cs.forEach(function(c,k){
    if(c===d)return;
    // Both rects are on screen, so dy also carries the scroll: cards slide from where you saw them.
    var q=c.getBoundingClientRect(),dx=was[k].left-q.left,dy=was[k].top-q.top;
    if(Math.abs(dx)>1)c.animate([{opacity:.25},{opacity:1}],{duration:260,easing:ease});
    else if(Math.abs(dy)>1)c.animate([{transform:"translateY("+dy+"px)"},{transform:"none"}],{duration:420,easing:ease});
  });
  return d.getBoundingClientRect();
}
function flip(d,open){
  if(d.open===open||busy.has(d))return;
  // Closed from its sticky head, far down a long card: the card's top is off screen, so fold it
  // at once and bring its head back into view rather than animating a part you can't see.
  if(!open&&d.getBoundingClientRect().top<-8){
    d.open=false;
    window.scrollBy(0,d.getBoundingClientRect().top-16);
    return;
  }
  var cs=[].slice.call(d.parentElement.children);
  if(still.matches||!d.animate){d.open=open;return}
  busy.add(d);d.style.zIndex=2;
  var fin=function(){d.style.zIndex="";busy.delete(d)},body=d.querySelector(".ia-body"),r="px 0 round 14px)";
  if(open){
    var was=rects(cs),a=was[cs.indexOf(d)];
    d.open=true;
    var b=settle(cs,was,d);
    d.animate([{transform:"translate("+(a.left-b.left)+"px,"+(a.top-b.top)+"px)",clipPath:"inset(0 "+Math.max(0,b.width-a.width)+"px "+Math.max(0,b.height-a.height)+r},{transform:"none",clipPath:"inset(0 0 0 0 round 14px)"}],{duration:420,easing:ease}).finished.then(fin,fin);
    if(body)body.animate([{opacity:0,transform:"translateY(-4px)"},{opacity:1,transform:"none"}],{duration:300,delay:140,easing:ease,fill:"backwards"});
  }else{
    // Fold into the size it will have, then let the grid close up around it.
    var big=d.getBoundingClientRect();
    d.open=false;var small=d.getBoundingClientRect();d.open=true;
    var fade=body&&body.animate([{opacity:1},{opacity:0}],{duration:150,easing:easeIn,fill:"forwards"});
    var fold=d.animate([{clipPath:"inset(0 0 0 0 round 14px)"},{clipPath:"inset(0 "+Math.max(0,big.width-small.width)+"px "+Math.max(0,big.height-small.height)+r}],{duration:260,easing:ease,fill:"forwards"});
    fold.finished.then(function(){
      var was=rects(cs);
      d.open=false;fold.cancel();if(fade)fade.cancel();
      var b=settle(cs,was,d);
      d.animate([{transform:"translate("+(big.left-b.left)+"px,0)"},{transform:"none"}],{duration:360,easing:ease}).finished.then(fin,fin);
    },fin);
  }
}
// "Show all" lists arrive as data and are drawn the first time each opens.
var data=null;
document.addEventListener("toggle",function(e){
  var d=e.target;
  if(!d.classList||!d.classList.contains("ia-all")||!d.open)return;
  var box=d.querySelector(".ia-items");
  if(!box||box.dataset.drawn)return;
  if(!data){var el=document.getElementById("ia-items");try{data=JSON.parse(el?el.textContent:"{}")}catch(x){data={}}}
  var lists=data[box.dataset.key]||[[],[]];
  function row(r,done){
    var it=document.createElement("div");it.className="ia-item"+(done?" done":"");
    if(r[1]){var im=document.createElement("img");im.src="/idleon/pic/"+r[1]+".png";im.alt="";im.loading="lazy";if(r[5])im.className="left";it.appendChild(im)}
    else{var dot=document.createElement("span");dot.className="ia-item-dot";it.appendChild(dot)}
    var nm=document.createElement("span");nm.className="ia-item-name";nm.textContent=r[0];it.appendChild(nm);
    var mt=document.createElement("span");
    if(r[4]){mt.className="ia-meter";var f=document.createElement("span");f.style.setProperty("--s",r[2]);mt.appendChild(f)}
    it.appendChild(mt);
    var fg=document.createElement("span");fg.className="ia-fig";fg.textContent=r[3];it.appendChild(fg);
    return it;
  }
  var frag=document.createDocumentFragment();
  lists[0].forEach(function(r){frag.appendChild(row(r,false))});
  if(lists[1].length){
    var h=document.createElement("p");h.className="ia-maxed-head";h.textContent=lists[1].length+" at max";frag.appendChild(h);
    var mx=document.createElement("div");mx.className="ia-maxed";
    lists[1].forEach(function(r){mx.appendChild(row(r,true))});frag.appendChild(mx);
  }
  box.appendChild(frag);box.dataset.drawn="1";
},true);
// Signed out: the report as text for /suggest, copied to the clipboard.
var rc=document.querySelector(".ia-report-copy");
if(rc)rc.addEventListener("submit",function(e){
  e.preventDefault();
  // The same header budget as a report sent from here (account 40, card 60): it fits /suggest's 1,500.
  var msg=rc.querySelector(".ia-report-msg"),card=rc.querySelector("select").value.slice(0,60),acc=(rc.dataset.account||"").slice(0,40);
  var txt="Account check report"+(acc?" ("+acc+")":"")+(card?", "+card:"")+": "+rc.querySelector("textarea").value.trim();
  var done=function(){msg.textContent="Copied. Now run /suggest in the server and paste it."},fail=function(){msg.textContent="Couldn't copy it here: select the text in the box and copy it yourself."};
  if(navigator.clipboard&&navigator.clipboard.writeText)navigator.clipboard.writeText(txt).then(done,fail);else fail();
});
// Report a problem without leaving your results.
var rf=document.getElementById("ia-report-send");
if(rf)rf.addEventListener("submit",function(e){
  e.preventDefault();
  var msg=rf.querySelector(".ia-report-msg"),b=rf.querySelector("button");
  b.disabled=true;msg.textContent="Sending…";
  fetch(rf.action,{method:"POST",body:new URLSearchParams(new FormData(rf)),headers:{"content-type":"application/x-www-form-urlencoded"}})
    .then(function(r){return r.text().then(function(t){msg.textContent=t;if(r.ok)rf.querySelector("textarea").value=""})})
    .catch(function(){msg.textContent="Couldn't send it. Check your connection and try again."})
    .finally(function(){b.disabled=false});
});
document.addEventListener("click",function(e){
  var s=e.target.closest(".ia-card > summary");
  if(s){e.preventDefault();flip(s.parentElement,!s.parentElement.open);return}
  var a=e.target.closest("a[data-go]");
  if(!a)return;
  var w=document.getElementById("w-"+a.dataset.w),m=document.getElementById(a.dataset.go);
  if(!m)return;
  e.preventDefault();
  var t=m.closest(".ia-world");if(t)w=document.getElementById("w-"+t.dataset.w);
  if(w){w.checked=true;showTab(false)}
  if(a.dataset.side){var v=m.querySelector('.ia-switch input[value="'+a.dataset.side+'"]');if(v)v.checked=true}
  flip(m,true);
  m.scrollIntoView({behavior:still.matches?"auto":"smooth",block:"start"});
});
// Tier layout B: picking a tier moves the highlight in the stage list (the page itself is CSS).
document.addEventListener("change",function(e){
  var p=e.target;if(!p.classList||!p.classList.contains("ia-pick"))return;
  var b=p.closest(".ia-b");if(!b)return;
  b.querySelectorAll(".ia-dash.picked,.ia-st.picked").forEach(function(x){x.classList.remove("picked")});
  var d=b.querySelector('.ia-dash[data-k="'+p.dataset.k+'"]');if(!d)return;
  var s=d.closest(".ia-st");d.classList.add("picked");s.classList.add("picked");s.classList.remove("far");
  stageRow(b.querySelector(".ia-stages"));
});
// On a phone the stage list is one row that scrolls sideways: keep the picked stage in view
// when a card opens or switches side.
function stageRow(r){var s=r&&r.querySelector(".ia-st.picked");if(s&&r.scrollWidth>r.clientWidth)r.scrollLeft+=s.getBoundingClientRect().left-r.getBoundingClientRect().left-14}
document.addEventListener("toggle",function(e){if(e.target.open&&e.target.classList.contains("ia-card"))e.target.querySelectorAll(".ia-stages").forEach(stageRow)},true);
document.addEventListener("change",function(e){var v=e.target.closest&&e.target.closest(".ia-split");if(v&&e.target.closest(".ia-switch"))v.querySelectorAll(".ia-stages").forEach(stageRow)});
showTab(false);
})()"##;

const CSS: &str = r#"
body.ia-pub { background: var(--page); -webkit-tap-highlight-color: transparent;
  --w0: var(--ink-2); --w1: #7fd18b; --w2: #e6c36a; --w3: #79c3f0; --w4: #a88bff; --w5: #ff9a5c; --w6: #f297c6; --w7: #46d1c0; }
.ia-hero { min-height: 280px; border-radius: 0; padding: 20px max(var(--gutter), calc((100% - 1500px) / 2)) 28px; }
.ia-hero .ov-hello h1 { font-size: 40px; }
.ia-page { max-width: calc(1500px + 2 * var(--gutter)); margin: 0 auto; padding: 0 var(--gutter) 72px; display: flex; flex-direction: column; gap: 22px; container: col / inline-size; }
.ia-panel { background: var(--raised); border: 1px solid var(--edge-1); border-radius: 14px; padding: 20px 22px; min-width: 0; }
.ia-head { display: flex; align-items: baseline; justify-content: space-between; gap: 4px 14px; flex-wrap: wrap; margin-bottom: 10px; }
.ia-head h2, .ia-ask h2 { font-size: 15px; font-weight: 600; }
.ia-head p { font-size: 13px; color: var(--ink-3); }

/* The paste box. */
.ia-ask { display: grid; gap: 18px; }
@container col (min-width: 900px) { .ia-ask { grid-template-columns: 320px minmax(0, 1fr); gap: 28px; } }
.ia-ask-text p { margin-top: 6px; color: var(--ink-2); font-size: 14px; line-height: 1.55; }
.ia-ask form { display: flex; flex-direction: column; gap: 12px; min-width: 0; }
.ia-ask textarea { width: 100%; min-height: 150px; resize: vertical; padding: 12px 14px; border-radius: 10px; border: 0; background: var(--well); box-shadow: inset 0 0 0 1px var(--edge-1); color: var(--ink); font: 12.5px/1.5 var(--mono); }
.ia-ask textarea:focus { outline: 2px solid var(--focus); outline-offset: 1px; }
.ia-ask-actions { order: 2; display: flex; gap: 10px; justify-content: flex-end; flex-wrap: wrap; }
.ia-packs { order: 1; display: flex; flex-wrap: wrap; align-items: center; gap: 6px; font-size: 13px; }
.ia-packs > span { margin-right: 4px; color: var(--ink-3); }
.ia-packs label { position: relative; cursor: pointer; }
.ia-packs input { position: absolute; opacity: 0; pointer-events: none; }
.ia-packs label span { display: block; padding: 6px 11px; border-radius: 8px; color: var(--ink-2); box-shadow: inset 0 0 0 1px var(--edge-1); }
.ia-packs label:hover span { color: var(--ink); }
.ia-packs input:checked + span { color: var(--ink); background: color-mix(in oklab, var(--accent) 16%, transparent); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--accent) 55%, transparent); }
.ia-packs input:focus-visible + span { outline: 2px solid var(--focus); outline-offset: 1px; }
.ia-try { display: flex; flex-wrap: wrap; align-items: baseline; gap: 6px 14px; padding-top: 14px; border-top: 1px solid var(--edge-1); color: var(--ink-3); font-size: 13px; }
.ia-try a { color: var(--ink-2); text-underline-offset: 3px; }
.ia-try a:hover { color: var(--ink); }
@media (pointer: coarse) { .ia-try a { padding: 8px 0; } .ia-switch label { min-height: 40px; } }
.ia-err { color: var(--held); font-size: 13.5px; }

/* The answer: the big share and one column per world. */
.ia-lead { display: grid; gap: 22px; align-items: end; }
@container col (min-width: 820px) { .ia-lead { grid-template-columns: minmax(0, 1fr) minmax(360px, 46%); } }
.ia-lead-who { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
.ia-big { font: 650 64px/1 var(--sans); letter-spacing: -0.03em; font-variant-numeric: tabular-nums; }
.ia-lead-who > span { color: var(--ink-2); font-size: 14px; }
.ia-lead-line { margin-top: 10px; color: var(--ink-3); font-size: 13px; }
.ia-lead-line a { color: var(--ink-2); text-decoration: underline; text-decoration-color: var(--edge-3); text-underline-offset: 3px; }
.ia-cols { display: grid; grid-template-columns: repeat(8, minmax(0, 1fr)); gap: 10px; height: 150px; }
.ia-col { display: grid; grid-template-rows: 1fr auto auto; justify-items: center; gap: 5px; min-width: 0; }
.ia-col-track { position: relative; width: 100%; max-width: 34px; border-radius: 6px; background: color-mix(in oklab, var(--w) 12%, var(--well)); overflow: hidden; }
.ia-col-fill { position: absolute; inset: auto 0 0 0; height: calc(var(--s) * 100%); min-height: 3px; background: var(--w); border-radius: 6px 6px 0 0; transform-origin: bottom; animation: ia-grow 0.6s var(--ease) both; }
.ia-col:nth-child(2) .ia-col-fill { animation-delay: 30ms; } .ia-col:nth-child(3) .ia-col-fill { animation-delay: 60ms; }
.ia-col:nth-child(4) .ia-col-fill { animation-delay: 90ms; } .ia-col:nth-child(5) .ia-col-fill { animation-delay: 120ms; }
.ia-col:nth-child(6) .ia-col-fill { animation-delay: 150ms; } .ia-col:nth-child(7) .ia-col-fill { animation-delay: 180ms; }
.ia-col:nth-child(8) .ia-col-fill { animation-delay: 210ms; }
@keyframes ia-grow { from { transform: scaleY(0); } }
.ia-col b { font: 600 13px var(--mono); color: var(--ink); }
.ia-col > span:last-child { font-size: 12px; color: var(--ink-3); }
.ia-col.off .ia-col-track { background: var(--well); }
.ia-col.off b { color: var(--ink-3); }

/* Rows: the items. A 28 px mark, the name and its line, a meter, the figure. */
.ia-mark { width: 28px; height: 28px; object-fit: contain; image-rendering: pixelated; }
body.ia-pub img.left { object-fit: cover; object-position: left center; }
.ia-mark.blank { display: block; width: 10px; height: 10px; margin: 9px; border-radius: 50%; background: var(--w); }
.ia-meter { display: block; height: 5px; border-radius: 3px; background: color-mix(in oklab, var(--w) 14%, var(--well)); overflow: hidden; }
.ia-meter > span { display: block; height: 100%; width: calc(var(--s) * 100%); min-width: 2px; border-radius: 3px; background: var(--w); transform-origin: left; animation: ia-fill 0.6s var(--ease) both; }
@keyframes ia-fill { from { transform: scaleX(0); } }
html.ia-settled .ia-meter > span, html.ia-settled .ia-col-fill { animation: none; }
.ia-fig { font: 500 12.5px var(--mono); color: var(--ink-2); text-align: right; white-space: nowrap; }
.ia-tag { justify-self: end; font: 600 11px var(--mono); color: var(--w); padding: 2px 6px; border-radius: 5px; background: color-mix(in oklab, var(--w) 14%, transparent); }

/* Worlds: tabs switched in the page, then one card per mechanic. */
.ia-tabs { display: flex; gap: 4px; flex-wrap: wrap; margin-bottom: 14px; }
.ia-tabs label { position: relative; display: inline-flex; align-items: center; gap: 8px; height: 34px; padding: 0 13px; border-radius: 9px; color: var(--ink-2); font-size: 13.5px; cursor: pointer; transition: background 0.18s, color 0.18s; }
.ia-tabs input { position: absolute; opacity: 0; width: 1px; height: 1px; pointer-events: none; }
.ia-tabs label:has(input:focus-visible) { outline: 2px solid var(--focus); outline-offset: 2px; }
.ia-tabs label:has(input:checked) { background: color-mix(in oklab, var(--w) 14%, var(--control)); color: var(--ink); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--w) 50%, transparent); }
.ia-tabs .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--w); }
.ia-tab-note { font-size: 11.5px; color: var(--ink-3); }
.ia-switch { display: inline-flex; gap: 2px; padding: 3px; margin: 2px 0 18px; border-radius: 9px; background: var(--well); }
.ia-switch label { position: relative; display: inline-flex; align-items: center; gap: 8px; padding: 6px 12px; border-radius: 7px; color: var(--ink-2); font-size: 13px; cursor: pointer; transition: background 0.18s, color 0.18s; }
.ia-switch label b { font-weight: 500; }
.ia-switch input { position: absolute; opacity: 0; width: 1px; height: 1px; pointer-events: none; }
.ia-switch label:has(input:focus-visible) { outline: 2px solid var(--focus); outline-offset: 2px; }
.ia-switch label:has(input:checked) { background: color-mix(in oklab, var(--ink) 10%, transparent); color: var(--ink); }
.ia-switch label:has(input:checked) b { font-weight: 600; }
.ia-switch label:has(input:checked) .ia-fig { color: var(--ink); }
.ia-view { display: flex; flex-direction: column; gap: 18px; }
.ia-split > .ia-view:not([data-v="0"]), .ia-split:has(> .ia-switch input:not([value="0"]):checked) > .ia-view[data-v="0"] { display: none; }
.ia-split:has(> .ia-switch input[value="1"]:checked) > .ia-view[data-v="1"],
.ia-split:has(> .ia-switch input[value="2"]:checked) > .ia-view[data-v="2"],
.ia-split:has(> .ia-switch input[value="3"]:checked) > .ia-view[data-v="3"],
.ia-split:has(> .ia-switch input[value="4"]:checked) > .ia-view[data-v="4"],
.ia-split:has(> .ia-switch input[value="5"]:checked) > .ia-view[data-v="5"] { display: flex; animation: ia-in 0.18s var(--ease); }
.ia-world { display: none; }
.ia-worlds:has(#w-0:checked) .ia-world[data-w="0"], .ia-worlds:has(#w-1:checked) .ia-world[data-w="1"],
.ia-worlds:has(#w-2:checked) .ia-world[data-w="2"], .ia-worlds:has(#w-3:checked) .ia-world[data-w="3"],
.ia-worlds:has(#w-4:checked) .ia-world[data-w="4"], .ia-worlds:has(#w-5:checked) .ia-world[data-w="5"],
.ia-worlds:has(#w-6:checked) .ia-world[data-w="6"], .ia-worlds:has(#w-7:checked) .ia-world[data-w="7"] { display: block; animation: ia-in 0.18s var(--ease); }
@keyframes ia-in { from { opacity: 0; } }
.ia-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(340px, 1fr)); gap: 12px; align-items: stretch; }
.ia-card { background: var(--raised); border: 1px solid var(--edge-1); border-radius: 14px; scroll-margin-top: 20px; }
.ia-card[open] { grid-column: 1 / -1; }
.ia-card > summary { display: grid; grid-template-columns: 28px minmax(0, 1fr) auto; grid-template-areas: "m n p" "b b b"; align-items: center; gap: 10px 12px; padding: 14px 16px; cursor: pointer; list-style: none; border-radius: 14px; transition: background 0.18s; }
.ia-card > summary::-webkit-details-marker { display: none; }
.ia-card > summary:focus-visible { outline: 2px solid var(--focus); outline-offset: -2px; }
.ia-card > summary .ia-mark { grid-area: m; }
.ia-card-name { grid-area: n; display: flex; flex-direction: column; min-width: 0; }
.ia-card-name b, .ia-card-name span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ia-card-name b { font-size: 14px; font-weight: 500; }
.ia-card-name span { font: 12.5px var(--mono); color: var(--ink-3); }
.ia-pct { grid-area: p; font: 600 15px var(--mono); }
.ia-pct.done { color: var(--live); }
.ia-pct.open { font: 500 12px var(--sans); color: var(--ink-3); }
.ia-fresh { display: flex; flex-wrap: wrap; gap: 8px; }
.ia-fresh-item { display: inline-flex; align-items: center; gap: 10px; height: 40px; padding: 0 8px 0 6px; border-radius: 10px; background: var(--control); color: var(--ink); font-size: 13.5px; transition: background 0.18s; }
.ia-fresh-item:active { transform: scale(0.97); }
.ia-fresh-item .ia-mark { width: 26px; height: 26px; }
.ia-fresh-item .ia-mark.blank { width: 8px; height: 8px; margin: 9px; }
.ia-meter.wide { grid-area: b; }
.ia-body { padding: 4px 16px 18px; display: flex; flex-direction: column; gap: 18px; }
.ia-body .ia-note { padding: 0; }
.ia-b { display: grid; grid-template-columns: 236px minmax(0, 1fr); gap: 28px; align-items: start; }
.ia-stages { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.ia-st { display: grid; grid-template-columns: minmax(0, 1fr) auto; align-items: center; gap: 10px; min-height: 30px; padding: 0 10px; border-radius: 8px; }
.ia-st.far { display: none; }
.ia-stages:has(.ia-allst:checked) .ia-st.far { display: grid; }
.ia-st.picked { background: color-mix(in oklab, var(--ink) 7%, transparent); }
.ia-st-name { display: flex; align-items: center; gap: 10px; min-width: 0; font-size: 13px; color: var(--ink-2); cursor: pointer; }
.ia-st-name span:last-child { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ia-st.picked .ia-st-name { color: var(--ink); }
.ia-st-dot { flex: none; width: 6px; height: 6px; border-radius: 50%; background: var(--w); }
.ia-dashes { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: 4px 3px; max-width: 104px; }
.ia-dash { position: relative; width: 10px; height: 4px; border-radius: 2px; background: color-mix(in oklab, var(--ink) 16%, transparent); cursor: pointer; }
.ia-dash::before { content: ""; position: absolute; inset: -7px -1.5px; }
.ia-dash.done { background: var(--live); }
.ia-dash.here { background: var(--ink); }
.ia-dash.picked:not(.here) { box-shadow: 0 0 0 1.5px var(--ink-2); }
.ia-st-more { padding: 8px 10px 0; font-size: 12px; color: var(--ink-3); cursor: pointer; }
.ia-st-more .less, .ia-stages:has(.ia-allst:checked) .ia-st-more .more { display: none; }
.ia-stages:has(.ia-allst:checked) .ia-st-more .less { display: inline; }
.ia-peeks { min-width: 0; }
.ia-pick { position: absolute; width: 1px; height: 1px; margin: 0; opacity: 0; pointer-events: none; }
.ia-peek { display: none; }
.ia-pick:checked + .ia-peek { display: block; animation: ia-in 0.18s var(--ease); }
.ia-peek-head { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.ia-peek-head h3 { font-size: 15px; font-weight: 600; }
.ia-pick:focus-visible + .ia-peek .ia-peek-head h3 { outline: 2px solid var(--focus); outline-offset: 3px; border-radius: 4px; }
.ia-peek > .ia-note { margin-top: 4px; }
.ia-steps { display: flex; gap: 6px; }
.ia-step { display: grid; place-items: center; width: 28px; height: 28px; border-radius: 8px; background: var(--well); color: var(--ink-2); cursor: pointer; }
.ia-step svg { width: 14px; height: 14px; }
.ia-step.back svg { transform: rotate(90deg); }
.ia-step.next svg { transform: rotate(-90deg); }
.ia-step.off { opacity: 0.35; cursor: default; }
.ia-task > .ia-fig { display: flex; flex-direction: column; align-items: flex-end; gap: 1px; min-width: 80px; }
.ia-task > .ia-fig b { font: 500 13px var(--mono); color: var(--ink); white-space: nowrap; }
.ia-task > .ia-fig span { font-size: 12px; color: var(--ink-3); white-space: nowrap; }
.ia-task > .ia-fig span.ok { color: var(--live); }
.ia-task > .ia-fig span.lock { color: var(--waiting); }
.ia-done-line { display: flex; align-items: flex-start; gap: 10px; margin-top: 10px; font-size: 13px; line-height: 20px; color: var(--ink-3); }
.ia-tick { flex: none; display: grid; place-items: center; width: 20px; height: 20px; border-radius: 50%; color: var(--live); background: color-mix(in oklab, var(--live) 14%, transparent); }
.ia-tick svg { width: 12px; height: 12px; }
.ia-rest > summary { margin-top: 8px; font-size: 13px; color: var(--ink-3); cursor: pointer; }
.ia-pace { margin: 0 0 10px; font-size: 13px; color: var(--ink-2); }
.ia-tasks { min-width: 0; }
.ia-tasks h3 { font-size: 15px; font-weight: 600; }
.ia-task-list { margin-top: 8px; }
.ia-task { display: grid; grid-template-columns: 28px minmax(0, 1fr) auto; align-items: center; gap: 12px; min-height: 50px; padding: 6px 0; border-bottom: 1px solid var(--edge-1); }
.ia-task:last-child { border-bottom: 0; }
.ia-task img { width: 28px; height: 28px; object-fit: contain; image-rendering: pixelated; }
.ia-task-text { display: flex; flex-direction: column; gap: 1px; min-width: 0; }
.ia-task-text b { font-size: 14px; font-weight: 500; }
.ia-task-text span { font-size: 13px; color: var(--ink-3); }
.ia-chip { font: 600 12px var(--sans); padding: 3px 8px; border-radius: 6px; white-space: nowrap; }
.ia-chip.miss { color: var(--waiting); background: color-mix(in oklab, var(--waiting) 14%, transparent); }
.ia-chip.next { color: var(--w); background: color-mix(in oklab, var(--w) 14%, transparent); }
.ia-chip.goal { color: var(--ink-2); background: color-mix(in oklab, var(--ink-2) 12%, transparent); }
/* Should have by now: a box per stage like the cards (three on a wide screen, one on a phone),
   a line per mechanic; a long box scrolls inside itself. */
.ia-owe-grid { display: grid; grid-template-columns: minmax(0, 1fr); gap: 12px; margin-top: 12px; align-items: stretch; }
/* Never more than two rows: as many columns as fit (each at least 340 px), and only twice that
   many boxes; a phone stacks them all. */
@container col (min-width: 692px) { .ia-owe-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); } .ia-owe-box:nth-child(n+5) { display: none; } }
@container col (min-width: 1044px) { .ia-owe-grid { grid-template-columns: repeat(3, minmax(0, 1fr)); } .ia-owe-box:nth-child(n+5) { display: block; } .ia-owe-box:nth-child(n+7) { display: none; } }
@container col (min-width: 1396px) { .ia-owe-grid { grid-template-columns: repeat(4, minmax(0, 1fr)); } .ia-owe-box:nth-child(n+7) { display: block; } }
.ia-owe-box { background: var(--raised); border: 1px solid var(--edge-1); border-radius: 14px; padding: 14px 16px 6px; min-width: 0; }
.ia-owe-head { display: flex; align-items: baseline; gap: 10px; flex-wrap: wrap; }
.ia-owe-head b { font-size: 14px; font-weight: 600; color: var(--w); }
.ia-owe-n { margin-left: auto; font-size: 13px; color: var(--ink-3); }
.ia-owe-lines { margin-top: 6px; max-height: 312px; overflow-y: auto; overscroll-behavior: contain; scrollbar-width: thin; }
.ia-owe-line { display: grid; grid-template-columns: 24px minmax(0, 1fr) auto; align-items: center; column-gap: 10px; row-gap: 2px; padding: 8px 4px 8px 0; color: inherit; text-decoration: none; border-bottom: 1px solid var(--edge-1); }
.ia-owe-line:last-child { border-bottom: 0; }
.ia-owe-line .ia-mark { width: 24px; height: 24px; }
.ia-owe-line .ia-mark.blank { width: 8px; height: 8px; margin: 8px; }
.ia-owe-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 14px; }
.ia-owe-name b { font-weight: 500; }
.ia-owe-name span { color: var(--ink-3); }
.ia-owe-line:hover b, .ia-owe-line:focus-visible b { text-decoration: underline; text-underline-offset: 3px; }
.ia-owe-t { font: 500 12.5px var(--mono); color: var(--ink-2); white-space: nowrap; }
.ia-owe-need { grid-column: 2 / -1; font-size: 12px; line-height: 1.35; color: var(--waiting); display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
.ia-hold { padding: 10px 14px; border-radius: 10px; background: color-mix(in oklab, var(--waiting) 10%, transparent); color: var(--ink-2); font-size: 13px; line-height: 1.45; }
.ia-hold b { color: var(--waiting); font-weight: 600; }
.ia-spread { display: flex; flex-wrap: wrap; gap: 8px 32px; }
.ia-spread div { display: flex; flex-direction: column; gap: 1px; }
.ia-spread b { font: 600 18px var(--mono); }
.ia-spread span { font-size: 12px; color: var(--ink-3); }
.ia-side { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 300px), 1fr)); gap: 12px; align-items: start; }
.ia-side-box { display: flex; flex-direction: column; gap: 6px; padding: 14px 16px; border-radius: 12px; background: var(--solid); }
.ia-side-k { font-size: 12px; color: var(--ink-2); }
.ia-side-big { font: 500 18px/1.3 var(--mono); overflow-wrap: anywhere; }
.ia-side-box p { font-size: 13.5px; line-height: 1.45; color: var(--ink-2); }
/* A raise: what it is, then what it adds, wrapped (some name 30 upgrades). A ring until you have
   it, then a tick. */
.ia-raise { display: grid; grid-template-columns: 16px minmax(0, 1fr); gap: 10px; align-items: start; padding: 4px 0; }
.ia-raise-mark { width: 14px; height: 14px; margin-top: 2px; border-radius: 50%; box-shadow: inset 0 0 0 1.5px var(--waiting); display: grid; place-items: center; }
.ia-raise.owned .ia-raise-mark { box-shadow: none; color: var(--live); }
.ia-raise-mark svg { width: 14px; height: 14px; }
.ia-raise-text { display: flex; flex-direction: column; gap: 1px; min-width: 0; }
.ia-raise-text b { font-size: 13.5px; font-weight: 500; }
.ia-raise-text span { font-size: 13px; line-height: 1.4; color: var(--ink-3); overflow-wrap: anywhere; }
.ia-raise.owned .ia-raise-text b { color: var(--ink-2); font-weight: 400; }
.ia-fresh-more > summary { display: inline-block; margin-top: 10px; cursor: pointer; font-size: 13px; color: var(--ink-2); text-decoration: underline; text-decoration-color: var(--edge-3); text-underline-offset: 3px; list-style: none; }
.ia-fresh-more > summary::-webkit-details-marker { display: none; }
.ia-fresh-more[open] > summary { display: none; }
.ia-fresh-more .ia-fresh { margin-top: 8px; }
.ia-all > summary { display: inline-block; white-space: nowrap; cursor: pointer; font-size: 13px; color: var(--ink-2); text-decoration: underline; text-decoration-color: var(--edge-3); text-underline-offset: 3px; list-style: none; }
.ia-all > summary::-webkit-details-marker { display: none; }
.ia-all[open] > summary { margin-bottom: 8px; }
.ia-all[open] .ia-items { animation: ia-in .22s var(--ease); }
.ia-items { padding: 0; display: grid; grid-template-columns: repeat(auto-fill, minmax(330px, 1fr)); gap: 0 28px; }
.ia-note, .ia-maxed-head { grid-column: 1 / -1; font-size: 13px; color: var(--ink-3); padding: 6px 0 8px; }
.ia-maxed-head { padding-top: 14px; }
.ia-maxed { grid-column: 1 / -1; display: grid; grid-template-columns: repeat(auto-fill, minmax(330px, 1fr)); gap: 0 28px; }
.ia-item { display: grid; grid-template-columns: 24px minmax(0, 1fr) minmax(50px, 26%) minmax(9ch, auto); align-items: center; gap: 10px; min-height: 36px; border-bottom: 1px solid var(--edge-1); font-size: 13.5px; }
.ia-item img { width: 24px; height: 24px; object-fit: contain; image-rendering: pixelated; }
.ia-item-dot { width: 6px; height: 6px; margin: 9px; border-radius: 50%; background: var(--edge-3); }
.ia-item-name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ia-item.done { color: var(--ink-3); }
.ia-item.done .ia-meter > span { background: color-mix(in oklab, var(--w) 45%, var(--well)); }

.ia-test { padding: 12px 16px; border-radius: 12px; background: color-mix(in oklab, var(--waiting) 10%, var(--raised)); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--waiting) 35%, transparent); color: var(--ink-2); font-size: 13.5px; line-height: 1.5; }
.ia-test b { color: var(--waiting); font-weight: 600; }
.ia-door { display: flex; flex-direction: column; align-items: flex-start; gap: 10px; }
.ia-door h2 { font-size: 17px; font-weight: 600; }
.ia-door p { color: var(--ink-2); font-size: 14px; }
.ia-report { display: grid; gap: 18px; }
@container col (min-width: 900px) { .ia-report { grid-template-columns: 320px minmax(0, 1fr); gap: 28px; } }
.ia-report form { display: flex; flex-direction: column; gap: 12px; min-width: 0; }
.ia-report-copy .ia-note { padding: 0; }
.ia-report textarea { width: 100%; resize: vertical; padding: 12px 14px; border-radius: 10px; border: 0; background: var(--well); box-shadow: inset 0 0 0 1px var(--edge-1); color: var(--ink); font: 14px/1.5 var(--sans); }
.ia-report textarea:focus, .ia-report select:focus { outline: 2px solid var(--focus); outline-offset: 1px; }
.ia-report-card { display: flex; align-items: center; gap: 10px; font-size: 13px; color: var(--ink-2); }
.ia-report-card select { flex: 0 1 auto; min-width: 0; }
.ia-report select { height: 34px; padding: 0 10px; border-radius: 9px; border: 0; background: var(--control); color: var(--ink); font: 13px var(--sans); box-shadow: inset 0 0 0 1px var(--edge-1); max-width: 100%; }
.ia-report .ia-ask-actions { align-items: center; }
.ia-report-msg { flex: 1; font-size: 13px; color: var(--ink-2); }
form.ia-signout { display: flex; flex-direction: row; flex-wrap: wrap; align-items: center; gap: 4px 10px; margin-top: 8px; font-size: 13px; color: var(--ink-3); }
form.ia-signout b { color: var(--ink-2); font-weight: 500; }
form.ia-signout button { padding: 0; border: 0; background: none; color: var(--ink-2); font: inherit; text-decoration: underline; text-underline-offset: 3px; cursor: pointer; }
@media (pointer: coarse) { form.ia-signout button { min-height: 40px; } }
.ia-foot { padding-top: 16px; border-top: 1px solid var(--edge-1); color: var(--ink-3); font-size: 12.5px; }

/* An open card keeps its head in view while you read down it, so closing it is one tap away. */
.ia-card[open] > summary { position: sticky; top: 0; z-index: 1; background: var(--raised); border-radius: 14px 14px 0 0; }

/* Hover only where there's a pointer to hover with: a tap would leave it stuck on. */
@media (hover: hover) and (pointer: fine) {
  .ia-lead-line a:hover { color: var(--ink); }
  .ia-tabs label:hover { background: var(--hover); color: var(--ink); }
  .ia-switch label:not(:has(input:checked)):hover { color: var(--ink); }
  .ia-card > summary:hover { background: var(--hover); }
  .ia-card[open] > summary:hover { background: linear-gradient(var(--hover), var(--hover)), var(--raised); }
  .ia-fresh-item:hover { background: var(--hover); }
  .ia-all > summary:hover, .ia-fresh-more > summary:hover { color: var(--ink); }
}
@media (hover: none) {
  .ia-tabs label:active { background: var(--hover); }
  .ia-switch label:active { background: var(--hover); }
  .ia-card > summary:active { background: linear-gradient(var(--hover), var(--hover)), var(--raised); }
}
/* Under 16 px a phone zooms into the box when you tap it. */
@media (pointer: coarse) {
  .ia-ask textarea, .ia-report textarea, .ia-report select { font-size: 16px; }
  /* Fingers, not a pointer: taps at least 40 px tall. */
  .ia-tabs label { height: 40px; }
  .ia-all > summary, .ia-fresh-more > summary { padding: 10px 0; }
  .ia-all[open] > summary { margin-bottom: 0; }
}

@media (max-width: 720px) {
  /* Two halves that share the width, each its name over where it stands: long figures
     ("Pay2Win Tier 26 / 59") ran past the card. */
  .ia-switch { display: flex; }
  .ia-switch label { flex: 1 1 0; min-width: 0; flex-direction: column; align-items: flex-start; gap: 2px; }
  .ia-switch label .ia-fig { font-size: 12px; }
  .ia-hero { min-height: 180px; }
  .ia-hero .ov-hello h1 { font-size: 26px; }
  .ia-page { padding: 0 16px 56px; gap: 14px; }
  .ia-panel { padding: 16px; }
  .ia-big { font-size: 48px; }
  .ia-cols { height: 112px; gap: 6px; }
  .ia-ask-actions .btn { flex: 1 1 auto; justify-content: center; }
  /* World tabs: one row that scrolls sideways instead of three rows of tabs. */
  .ia-tabs { flex-wrap: nowrap; overflow-x: auto; margin: 0 -16px 12px; padding: 0 16px 2px; scrollbar-width: none; scroll-padding: 0 16px; overscroll-behavior-x: contain; }
  .ia-tabs::-webkit-scrollbar { display: none; }
  .ia-tabs label { flex: none; }
  .ia-grid, .ia-items, .ia-maxed { grid-template-columns: minmax(0, 1fr); }
  .ia-card > summary { padding: 12px 14px; }
  .ia-card-name span { white-space: normal; }
  .ia-body { padding: 4px 14px 16px; gap: 16px; }
  /* Tier layout B: the stages one row that scrolls sideways, the picked tier under it. */
  .ia-b { grid-template-columns: minmax(0, 1fr); gap: 14px; }
  .ia-stages { flex-direction: row; overflow-x: auto; gap: 4px; margin: 0 -14px; padding: 0 14px 2px; scrollbar-width: none; }
  .ia-st, .ia-st.far { display: flex; flex: none; flex-direction: column; align-items: flex-start; gap: 5px; padding: 7px 10px; }
  .ia-st-more { display: none; }
  .ia-dashes { flex-wrap: nowrap; justify-content: flex-start; max-width: none; }
  .ia-task > .ia-fig { min-width: 0; }
  .ia-item { grid-template-columns: 24px minmax(0, 1fr) minmax(44px, 22%) minmax(9ch, auto); gap: 8px; }
}
"#;

#[cfg(test)]
mod tests {
    use super::{stage_window, stage_world};

    #[test]
    fn a_ladder_shows_twelve_stages_from_a_few_before_yours() {
        assert_eq!(stage_window(8, 5), (0, 8));
        assert_eq!(stage_window(24, 0), (0, 12));
        assert_eq!(stage_window(24, 8), (5, 17));
        assert_eq!(stage_window(24, 23), (12, 24));
        assert_eq!((stage_world("Late World 4", 2), stage_world("Endgame 1", 2), stage_world("Any 5 of 24", 2)), (4, 7, 2));
    }
}
