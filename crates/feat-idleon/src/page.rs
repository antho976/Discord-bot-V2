//! The public page: the paste box, then the review.
//!
//! The review leads with the answer: how far the account is overall with one bar per world,
//! then "Do next", then every mechanic by world (tabs switched in the page, no reload), each a
//! card that opens to its items, least done first. Worlds wear their own colour (data colour,
//! not decoration): the bars, the meters and the world tags.

use bot_core::Ctx;
use bot_core::maud::{DOCTYPE, Markup, PreEscaped, html};
use bot_core::web::ui;

use crate::game::GAME;
use crate::pictures;
use crate::review::{Count, Item, Mechanic, Review, Why};

pub enum State<'a> {
    Ask { error: Option<&'a str> },
    Done(&'a Review),
}

pub const WORLD_NAMES: [&str; 8] = ["Account", "Blunder Hills", "Yum-Yum Desert", "Frostbite Tundra", "Hyperion Nebula", "Smolderin' Plateau", "Spirited Valley", "Shimmerfin Deep"];

fn world_tag(w: u8) -> String {
    if w == 0 { "All".into() } else { format!("W{w}") }
}

pub fn render(ctx: &Ctx, guild: u64, state: State) -> Markup {
    let name = ctx.guild_name(guild);
    let icon = ctx.guild_icon_large(guild);
    let banner = ctx.guild_banner_url(guild);
    let frame = bot_core::web::banner::frame(ctx, guild);
    let initial: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    let title = match &state {
        State::Done(r) if !r.name.is_empty() => format!("{}'s account", r.name),
        _ => "Account check".to_string(),
    };
    html! {
        (DOCTYPE)
        html lang="en" {
            head { (ui::head(&title)) }
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
                    @match state {
                        State::Ask { error } => (ask(guild, error)),
                        State::Done(r) => (done(guild, r)),
                    }
                    footer.ia-foot {
                        span { "Maxes and names come from the game's own data, from " (GAME.updated) ". Your save is read once and not kept." }
                    }
                }
                script { (PreEscaped(JS)) }
            }
        }
    }
}

fn ask(guild: u64, error: Option<&str>) -> Markup {
    html! {
        section.ia-panel.ia-ask {
            div.ia-ask-text {
                h2 { "Paste your save" }
                p { "Copy your account's JSON from IdleonToolbox or Idleon Efficiency and paste it here, or open the file. You get how far each mechanic is from the game's max, and what to work on next." }
            }
            form method="post" action={ "/p/" (guild) "/idleon" } data-full {
                textarea #ia-save name="save" rows="7" spellcheck="false" autocomplete="off" placeholder="{\"data\": {...}, \"charNames\": [...]}" required {}
                @if let Some(e) = error { p.ia-err role="alert" { (e) } }
                div.ia-ask-actions {
                    label.btn {
                        (ui::icon(bot_core::Icon::Enter))
                        "Open a file"
                        input #ia-file type="file" accept=".json,.txt,application/json" hidden;
                    }
                    button.btn.primary type="submit" { "Check my account" }
                }
            }
        }
    }
}

fn done(guild: u64, r: &Review) -> Markup {
    let worlds: Vec<u8> = (1..=7).chain([0]).filter(|w| r.mechanics.iter().any(|m| m.world == *w)).collect();
    let start = worlds.iter().copied().find(|w| *w == r.next.first().map_or(r.world, |n| r.mechanics[n.mechanic].world)).unwrap_or(1);
    html! {
        section.ia-panel.ia-lead {
            div.ia-lead-who {
                b.ia-big { (pct(r.share())) }
                span { "of the maxes this check reads, across " (r.mechanics.iter().filter(|m| m.world <= r.world).count()) " mechanics" }
                p.ia-lead-line {
                    "World " (r.world) " · " (r.characters) " characters"
                    " · " a href={ "/p/" (guild) "/idleon" } { "Check another account" }
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
        @if !r.next.is_empty() {
            section.ia-panel {
                div.ia-head {
                    h2 { "Do next" }
                    p { "What the rest of your account has left behind, then the quick finishes" }
                }
                ol.ia-next {
                    @for n in &r.next {
                        @let m = &r.mechanics[n.mechanic];
                        li {
                            a.ia-row href={ "#m-" (m.key) } data-go={ "m-" (m.key) } data-w=(m.world) style={ "--w: var(--w" (m.world) ")" } {
                                (mark(m))
                                span.ia-row-text {
                                    b { (m.name) }
                                    span { (why(m, &n.why)) }
                                }
                                span.ia-meter { span style={ "--s:" (m.share()) } {} }
                                span.ia-fig { (fig(m)) }
                                span.ia-tag { (world_tag(m.world)) }
                            }
                        }
                    }
                }
            }
        }
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
            @for w in &worlds {
                div.ia-world data-w=(w) style={ "--w: var(--w" (w) ")" } {
                    div.ia-grid {
                        @for m in r.mechanics.iter().filter(|m| m.world == *w) { (card(m)) }
                    }
                }
            }
        }
    }
}

fn card(m: &Mechanic) -> Markup {
    let open = m.open_items();
    let maxed: Vec<&Item> = m.items.iter().filter(|i| i.done()).collect();
    html! {
        details.ia-card #{ "m-" (m.key) } {
            summary {
                (mark(m))
                span.ia-card-name { b { (m.name) } span { (fig(m)) } }
                span.ia-pct.done[m.done()] { (pct(m.share())) }
                span.ia-meter.wide { span style={ "--s:" (m.share()) } {} }
            }
            div.ia-items {
                @if let Some(n) = &m.note { p.ia-note { (n) } }
                @if open.is_empty() && !m.items.is_empty() { p.ia-note { "Everything here is at max." } }
                @for i in &open { (item(i)) }
                @if !maxed.is_empty() {
                    p.ia-maxed-head { (maxed.len()) " at max" }
                    div.ia-maxed { @for i in &maxed { (item(i)) } }
                }
            }
        }
    }
}

fn item(i: &Item) -> Markup {
    html! {
        div.ia-item.done[i.done()] {
            @if let Some(p) = i.picture.as_deref().filter(|p| pictures::has(p)) {
                img src={ "/idleon/pic/" (p) ".png" } alt="" loading="lazy";
            } @else { span.ia-item-dot {} }
            span.ia-item-name { (i.name) }
            span.ia-meter { span style={ "--s:" (i.share()) } {} }
            span.ia-fig {
                @match &i.level_name {
                    Some(n) => (n),
                    None => { (num(i.level)) " / " (num(i.max)) },
                }
            }
        }
    }
}

/// The mechanic's picture, or a dot in its world's colour.
fn mark(m: &Mechanic) -> Markup {
    html! {
        @match m.picture.as_deref().filter(|p| pictures::has(p)) {
            Some(p) => img.ia-mark src={ "/idleon/pic/" (p) ".png" } alt="";,
            None => span.ia-mark.blank {},
        }
    }
}

fn why(m: &Mechanic, w: &Why) -> String {
    match w {
        Why::Behind { world_avg } => {
            let place = if m.world == 0 { "account".to_string() } else { format!("W{}", m.world) };
            format!("{} done, your {place} average is {}", pct(m.share()), pct(*world_avg))
        }
        Why::NearlyDone { left: 1 } => "One left to max".into(),
        Why::NearlyDone { left } => format!("{left} left to max"),
    }
}

fn fig(m: &Mechanic) -> String {
    match m.count {
        Count::Levels => format!("{} / {} {}", num(m.have()), num(m.total()), m.unit),
        Count::Done => format!("{} of {} {}", num(m.have()), num(m.total()), m.unit),
    }
}

fn pct(s: f64) -> String {
    let p = s * 100.0;
    // 99.6% shouldn't read as done.
    if (99.5..100.0).contains(&p) { "99%".into() } else { format!("{:.0}%", p) }
}

/// Whole numbers with thousands separators; anything else with one decimal.
fn num(n: f64) -> String {
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

const JS: &str = r#"(function(){
var f=document.getElementById("ia-file"),t=document.getElementById("ia-save");
if(f&&t)f.addEventListener("change",function(){var x=f.files&&f.files[0];if(x)x.text().then(function(s){t.value=s;t.form.requestSubmit()})});
document.querySelectorAll("a[data-go]").forEach(function(a){a.addEventListener("click",function(){var w=document.getElementById("w-"+a.dataset.w);if(w)w.checked=true;var m=document.getElementById(a.dataset.go);if(m)m.open=true})});
})()"#;

const CSS: &str = r#"
body.ia-pub { background: var(--page);
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
.ia-ask-actions { display: flex; gap: 10px; justify-content: flex-end; flex-wrap: wrap; }
.ia-err { color: var(--held); font-size: 13.5px; }

/* The answer: the big share and one column per world. */
.ia-lead { display: grid; gap: 22px; align-items: end; }
@container col (min-width: 820px) { .ia-lead { grid-template-columns: minmax(0, 1fr) minmax(360px, 46%); } }
.ia-lead-who { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
.ia-big { font: 650 64px/1 var(--sans); letter-spacing: -0.03em; font-variant-numeric: tabular-nums; }
.ia-lead-who > span { color: var(--ink-2); font-size: 14px; }
.ia-lead-line { margin-top: 10px; color: var(--ink-3); font-size: 13px; }
.ia-lead-line a { color: var(--ink-2); text-decoration: underline; text-decoration-color: var(--edge-3); text-underline-offset: 3px; }
.ia-lead-line a:hover { color: var(--ink); }
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

/* Rows: do next and the items. A 28 px mark, the name and its line, a meter, the figure. */
.ia-next { list-style: none; display: flex; flex-direction: column; padding: 0; margin: 0; }
.ia-next li + li { border-top: 1px solid var(--edge-1); }
.ia-row { display: grid; grid-template-columns: 28px minmax(0, 1fr) minmax(80px, 22%) auto 34px; align-items: center; gap: 14px; min-height: 54px; padding: 0 6px; margin: 0 -6px; border-radius: 8px; color: var(--ink); transition: background 0.18s; }
.ia-row:hover { background: var(--hover); }
.ia-row:active { transform: scale(0.99); }
.ia-row-text { display: flex; flex-direction: column; min-width: 0; }
.ia-row-text b { font-size: 14px; font-weight: 500; }
.ia-row-text span { font-size: 13px; color: var(--ink-3); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.ia-mark { width: 28px; height: 28px; object-fit: contain; image-rendering: pixelated; }
.ia-mark.blank { display: block; width: 10px; height: 10px; margin: 9px; border-radius: 50%; background: var(--w); }
.ia-meter { display: block; height: 5px; border-radius: 3px; background: color-mix(in oklab, var(--w) 14%, var(--well)); overflow: hidden; }
.ia-meter > span { display: block; height: 100%; width: calc(var(--s) * 100%); min-width: 2px; border-radius: 3px; background: var(--w); transform-origin: left; animation: ia-fill 0.6s var(--ease) both; }
@keyframes ia-fill { from { transform: scaleX(0); } }
.ia-fig { font: 500 12.5px var(--mono); color: var(--ink-2); text-align: right; white-space: nowrap; }
.ia-tag { justify-self: end; font: 600 11px var(--mono); color: var(--w); padding: 2px 6px; border-radius: 5px; background: color-mix(in oklab, var(--w) 14%, transparent); }

/* Worlds: tabs switched in the page, then one card per mechanic. */
.ia-tabs { display: flex; gap: 4px; flex-wrap: wrap; margin-bottom: 14px; }
.ia-tabs label { position: relative; display: inline-flex; align-items: center; gap: 8px; height: 34px; padding: 0 13px; border-radius: 9px; color: var(--ink-2); font-size: 13.5px; cursor: pointer; transition: background 0.18s, color 0.18s; }
.ia-tabs label:hover { background: var(--hover); color: var(--ink); }
.ia-tabs input { position: absolute; opacity: 0; width: 1px; height: 1px; pointer-events: none; }
.ia-tabs label:has(input:focus-visible) { outline: 2px solid var(--focus); outline-offset: 2px; }
.ia-tabs label:has(input:checked) { background: color-mix(in oklab, var(--w) 14%, var(--control)); color: var(--ink); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--w) 50%, transparent); }
.ia-tabs .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--w); }
.ia-tab-note { font-size: 11.5px; color: var(--ink-3); }
.ia-world { display: none; }
.ia-worlds:has(#w-0:checked) .ia-world[data-w="0"], .ia-worlds:has(#w-1:checked) .ia-world[data-w="1"],
.ia-worlds:has(#w-2:checked) .ia-world[data-w="2"], .ia-worlds:has(#w-3:checked) .ia-world[data-w="3"],
.ia-worlds:has(#w-4:checked) .ia-world[data-w="4"], .ia-worlds:has(#w-5:checked) .ia-world[data-w="5"],
.ia-worlds:has(#w-6:checked) .ia-world[data-w="6"], .ia-worlds:has(#w-7:checked) .ia-world[data-w="7"] { display: block; animation: ia-in 0.18s var(--ease); }
@keyframes ia-in { from { opacity: 0; } }
.ia-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(340px, 1fr)); gap: 12px; align-items: start; }
.ia-card { background: var(--raised); border: 1px solid var(--edge-1); border-radius: 14px; scroll-margin-top: 20px; }
.ia-card[open] { grid-column: 1 / -1; }
.ia-card summary { display: grid; grid-template-columns: 28px minmax(0, 1fr) auto; grid-template-areas: "m n p" "b b b"; align-items: center; gap: 10px 12px; padding: 14px 16px; cursor: pointer; list-style: none; border-radius: 14px; transition: background 0.18s; }
.ia-card summary::-webkit-details-marker { display: none; }
.ia-card summary:hover { background: var(--hover); }
.ia-card summary:focus-visible { outline: 2px solid var(--focus); outline-offset: -2px; }
.ia-card summary .ia-mark { grid-area: m; }
.ia-card-name { grid-area: n; display: flex; flex-direction: column; min-width: 0; }
.ia-card-name b { font-size: 14px; font-weight: 500; }
.ia-card-name span { font: 12.5px var(--mono); color: var(--ink-3); }
.ia-pct { grid-area: p; font: 600 15px var(--mono); }
.ia-pct.done { color: var(--live); }
.ia-meter.wide { grid-area: b; }
.ia-items { padding: 2px 16px 16px; display: grid; grid-template-columns: repeat(auto-fill, minmax(330px, 1fr)); gap: 0 28px; }
.ia-note, .ia-maxed-head { grid-column: 1 / -1; font-size: 13px; color: var(--ink-3); padding: 6px 0 8px; }
.ia-maxed-head { padding-top: 14px; }
.ia-maxed { grid-column: 1 / -1; display: grid; grid-template-columns: repeat(auto-fill, minmax(330px, 1fr)); gap: 0 28px; }
.ia-item { display: grid; grid-template-columns: 24px minmax(0, 1fr) minmax(50px, 26%) auto; align-items: center; gap: 10px; min-height: 36px; border-bottom: 1px solid var(--edge-1); font-size: 13.5px; }
.ia-item img { width: 24px; height: 24px; object-fit: contain; image-rendering: pixelated; }
.ia-item-dot { width: 6px; height: 6px; margin: 9px; border-radius: 50%; background: var(--edge-3); }
.ia-item-name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ia-item.done { color: var(--ink-3); }
.ia-item.done .ia-meter > span { background: color-mix(in oklab, var(--w) 45%, var(--well)); }

.ia-foot { padding-top: 16px; border-top: 1px solid var(--edge-1); color: var(--ink-3); font-size: 12.5px; }

@media (max-width: 720px) {
  .ia-hero { min-height: 200px; }
  .ia-hero .ov-hello h1 { font-size: 28px; }
  .ia-page { padding: 0 16px 56px; gap: 16px; }
  .ia-panel { padding: 16px; }
  .ia-big { font-size: 48px; }
  .ia-cols { height: 120px; gap: 6px; }
  .ia-row { grid-template-columns: 28px minmax(0, 1fr) auto; }
  .ia-row .ia-meter, .ia-row .ia-tag { display: none; }
  .ia-row { padding: 8px 6px; }
  .ia-row-text span { white-space: normal; }
  .ia-grid, .ia-items, .ia-maxed { grid-template-columns: minmax(0, 1fr); }
}
"#;
