use std::collections::{HashMap, HashSet};

use bot_core::axum::Router;
use bot_core::axum::extract::{Path, State};
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::{get, post};
use bot_core::jiff::Timestamp;
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::twilight_model::id::Id;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, User, WebResult, saved};
use bot_core::{Ctx, Icon, text};

use crate::{KEY, Settings, afk_clear, afk_list, name_of, name_or_stored, restore_nick, tz_all, tz_guild_users, zones};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/utility", get(page).post(save_settings))
        .route("/g/{guild}/utility/afk/{user}/clear", post(clear_afk))
}

/// Members of this server with a timezone, by what their clock reads, earliest first. Then
/// the members whose saved zone the bot no longer knows (a zone renamed in a tz update).
/// A member counts when the cache has them or they set their zone in this server, so the grid
/// doesn't shrink to whoever has spoken since a restart.
type MemberTimes = (Vec<(u64, String, zones::Local)>, Vec<u64>);

/// Everyone in the server, from the gateway cache.
fn members(ctx: &Ctx, guild: u64) -> HashSet<u64> {
    ctx.cache.guild_members(Id::new(guild)).map(|set| set.iter().map(|u| u.get()).collect()).unwrap_or_default()
}

fn member_times(ctx: &Ctx, members: &HashSet<u64>, set_here: &HashMap<u64, String>) -> bot_core::anyhow::Result<MemberTimes> {
    let now = Timestamp::now();
    let (mut rows, mut unknown) = (Vec::new(), Vec::new());
    for (u, zone) in tz_all(ctx)?.into_iter().filter(|(u, _)| members.contains(u) || set_here.contains_key(u)) {
        match zones::local(&zone, now) {
            Some(l) => rows.push((u, zone, l)),
            None => unknown.push(u),
        }
    }
    rows.sort_by(|a, b| a.2.offset_secs.cmp(&b.2.offset_secs).then_with(|| a.1.cmp(&b.1)));
    Ok((rows, unknown))
}

/// "Mira", "Mira and Teo", "Mira, Teo and 3 others".
fn names_line(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [a, b, c] => format!("{a}, {b} and {c}"),
        [a, b, rest @ ..] => format!("{a}, {b} and {} others", rest.len()),
    }
}

/// Daytime is 06:00 to 18:00 on the member's clock.
fn is_day(hour: u32) -> bool {
    (6..18).contains(&hour)
}

fn member_line(n: usize) -> String {
    match n {
        1 => "1 member sets a timezone with /timezone".into(),
        n => format!("{} members set a timezone with /timezone", text::num(n as i64)),
    }
}

/// Minutes past midnight on a clock that reads "14:32".
fn day_minutes(clock: &str) -> u32 {
    let (h, m) = clock.split_once(':').unwrap_or(("0", "0"));
    (h.parse::<u32>().unwrap_or(0) * 60 + m.parse::<u32>().unwrap_or(0)).min(24 * 60)
}

/// How far apart, in minutes on the band, two pins can sit side by side. Closer ones stack.
const PIN_ROOM: u32 = 115;

/// The row each pin sits in above the band, given where they fall (minutes past midnight).
/// Pins are placed left to right, each on the lowest row with room for it.
fn pin_rows(xs: &[u32]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..xs.len()).collect();
    order.sort_by_key(|&i| xs[i]);
    let mut ends: Vec<u32> = Vec::new();
    let mut rows = vec![0; xs.len()];
    for i in order {
        let row = ends.iter().position(|&end| xs[i] >= end + PIN_ROOM).unwrap_or(ends.len());
        match ends.get_mut(row) {
            Some(end) => *end = xs[i],
            None => ends.push(xs[i]),
        }
        rows[i] = row;
    }
    rows
}

/// One clock: the members whose zone is the same.
struct Clock<'a> {
    zone: &'a str,
    local: &'a zones::Local,
    users: Vec<u64>,
}

/// Times are sorted by offset, then zone, so a zone's members are next to each other.
fn clocks(times: &[(u64, String, zones::Local)]) -> Vec<Clock<'_>> {
    let mut out: Vec<Clock<'_>> = Vec::new();
    for (u, zone, l) in times {
        match out.last_mut() {
            Some(c) if c.zone == zone => c.users.push(*u),
            _ => out.push(Clock { zone, local: l, users: vec![*u] }),
        }
    }
    out
}

/// A pin on the band: every zone with the same offset reads the same time.
struct Pin<'a> {
    local: &'a zones::Local,
    users: Vec<u64>,
    places: Vec<String>,
}

fn pins<'a>(clocks: &[Clock<'a>]) -> Vec<Pin<'a>> {
    let mut out: Vec<Pin<'a>> = Vec::new();
    for c in clocks {
        let place = zones::place(c.zone, &c.local.offset);
        match out.last_mut() {
            Some(p) if p.local.offset_secs == c.local.offset_secs => {
                p.users.extend(&c.users);
                if !p.places.contains(&place) {
                    p.places.push(place);
                }
            }
            _ => out.push(Pin { local: c.local, users: c.users.clone(), places: vec![place] }),
        }
    }
    out
}

/// The hands' angles for a clock reading: `--h` and `--m` for the dial.
fn hands(clock: &str) -> String {
    let m = day_minutes(clock);
    let (h, m) = (m / 60, m % 60);
    format!("--h:{}deg;--m:{}deg", (h % 12) as f32 * 30.0 + m as f32 * 0.5, m * 6)
}

/// "Sat" from "Sat 14:32".
fn weekday(l: &zones::Local) -> &str {
    l.time.split_whitespace().next().unwrap_or_default()
}

/// A sun in the same drawing style as the dashboard's own icons.
fn sun() -> Markup {
    html! {
        svg.icon viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" aria-hidden="true" {
            circle cx="8" cy="8" r="2.75" {}
            (PreEscaped(r#"<path d="M8 1.5v1.5M8 13v1.5M1.5 8H3M13 8h1.5M3.4 3.4l1.05 1.05M11.55 11.55l1.05 1.05M3.4 12.6l1.05-1.05M11.55 4.45l1.05-1.05"/>"#))
        }
    }
}

/// Both glyphs: the card shows the sun by day and the moon by night, and the page script
/// flips them as the clock moves on.
fn day_glyphs() -> Markup {
    html! {
        span.au-glyph.au-sun role="img" aria-label="Daytime" { (sun()) }
        span.au-glyph.au-moon role="img" aria-label="Night" { (ui::icon(Icon::Moon)) }
    }
}

/// A clock face: twelve marks, an hour hand and a minute hand turned by `--h` and `--m`.
fn dial() -> Markup {
    let marks: String = (0..12)
        .map(|i| {
            let a = (i as f32 * 30.0).to_radians();
            let (inner, class) = if i % 3 == 0 { (13.2, "tk q") } else { (14.6, "tk") };
            let (s, c) = a.sin_cos();
            format!(
                r#"<line class="{class}" x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}"/>"#,
                20.0 + s * inner,
                20.0 - c * inner,
                20.0 + s * 16.4,
                20.0 - c * 16.4
            )
        })
        .collect();
    html! {
        svg.au-dial viewBox="0 0 40 40" width="56" height="56" aria-hidden="true" {
            circle.face cx="20" cy="20" r="18.6" {}
            (PreEscaped(marks))
            line.hh x1="20" y1="20" x2="20" y2="10.5" {}
            line.mh x1="20" y1="20" x2="20" y2="6" {}
            circle.pivot cx="20" cy="20" r="1.7" {}
        }
    }
}

async fn page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    // Leaving clears someone's AFK, so every row is someone in the server, cached or not.
    let away = afk_list(&ctx, guild)?;
    let set_here = tz_guild_users(&ctx, guild)?;
    let (times, unknown) = member_times(&ctx, &members(&ctx, guild), &set_here)?;
    let who = |u: u64| name_or_stored(&ctx, guild, u, set_here.get(&u).map(String::as_str).unwrap_or_default());
    let unknown: Vec<String> = unknown.into_iter().map(who).collect();
    const SHOWN: usize = 200;
    let shown = &times[..times.len().min(SHOWN)];
    let clocks = clocks(shown);
    let pins = pins(&clocks);
    let xs: Vec<u32> = pins.iter().map(|p| day_minutes(&p.local.clock)).collect();
    let rows = pin_rows(&xs);
    let top_row = rows.iter().copied().max().unwrap_or(0);
    // Pins in the order they arrive on the band: left to right.
    let mut arrival: Vec<usize> = (0..pins.len()).collect();
    arrival.sort_by_key(|&i| xs[i]);
    // Where an away member's clock is, when they set a zone.
    let zone_of: HashMap<u64, (&str, &zones::Local)> = times.iter().map(|(u, z, l)| (*u, (z.as_str(), l))).collect();

    let band = html! {
        div.au-sky {
            div.au-stage style={ "--rows:" (top_row + 1) } {
                div.au-pins {
                    @for (order, &i) in arrival.iter().enumerate() {
                        @let p = &pins[i];
                        @let names: Vec<String> = p.users.iter().map(|u| who(*u)).collect();
                        div.au-track data-offset=(p.local.offset_secs) style={ "--x:" (format!("{:.2}", xs[i] as f32 / 14.4)) ";--i:" (order) ";--row:" (rows[i]) } {
                            div.au-pin data-tip={ (p.places.join(", ")) ": " (names_line(&names)) } {
                                span.au-pin-faces {
                                    @for u in p.users.iter().take(3) {
                                        img.avatar src=(ctx.member_avatar(guild, *u)) alt="" width="28" height="28" loading="lazy";
                                    }
                                    @if p.users.len() > 3 { span.au-more { "+" (p.users.len() - 3) } }
                                }
                                span.au-clock { (p.local.clock) }
                                span.au-stem {}
                            }
                        }
                    }
                }
                div.au-band role="img" aria-label="A day from midnight to midnight, with each member at the time their clock shows" {}
                div.au-ticks aria-hidden="true" {
                    span style="left:0%" { "00:00" }
                    span style="left:25%" { "06:00" }
                    span style="left:50%" { "12:00" }
                    span style="left:75%" { "18:00" }
                    span style="left:100%" { "24:00" }
                }
            }
        }
    };

    let zone_cards = html! {
        div.au-zones {
            @for (n, c) in clocks.iter().enumerate() {
                @let l = c.local;
                div.au-zone.is-day[is_day(l.hour)] data-offset=(l.offset_secs) style={ (hands(&l.clock)) ";--i:" (n) } title={ (c.zone) " · " (l.offset) } {
                    div.au-zone-top {
                        (dial())
                        div.au-zone-time {
                            b.au-clock { (l.clock) }
                            span.au-zone-sub { (day_glyphs()) span.au-day { (weekday(l)) } }
                        }
                    }
                    div.au-place { strong.clip { (zones::place(c.zone, &l.offset)) } span { (l.offset) } }
                    @if c.users.len() <= 4 {
                        div.au-people {
                            @for u in &c.users {
                                span.au-who {
                                    img.avatar src=(ctx.member_avatar(guild, *u)) alt="" width="22" height="22" loading="lazy";
                                    span.clip { (who(*u)) }
                                }
                            }
                        }
                    } @else {
                        @let urls: Vec<String> = c.users.iter().map(|u| ctx.member_avatar(guild, *u)).collect();
                        @let names: Vec<String> = c.users.iter().map(|u| who(*u)).collect();
                        div.au-crowd {
                            (ui::avatars(&urls, 5))
                            span { (names_line(&names)) }
                        }
                    }
                }
            }
        }
    };

    let body = html! {
        style { (PreEscaped(CSS)) }
        div.with-rail.au {
            div.stack.au-main {
                @if times.is_empty() && unknown.is_empty() {
                    (ui::section("Where everyone is right now", None, ui::empty(Icon::Clock, "Nobody has set a timezone", "Members add theirs with /timezone set, and their local time shows here.")))
                } @else {
                    @if !times.is_empty() {
                        (ui::section("Where everyone is right now", Some("Each face sits at the time their own clock shows."), band))
                        (ui::section("By timezone", None, html! {
                            (zone_cards)
                            p.muted.small { (member_line(times.len())) }
                            @if times.len() > SHOWN { p.muted.small { "Showing the first " (SHOWN) " of " (times.len()) "." } }
                        }))
                    }
                    @if !unknown.is_empty() {
                        p.muted.small {
                            (names_line(&unknown)) " saved a timezone the bot no longer recognizes, so "
                            (if unknown.len() == 1 { "it's" } else { "they're" }) " left out. /timezone set fixes it."
                        }
                    }
                }
            }
            aside.rail {
                div.rail-block {
                    h2 {
                        "Away right now"
                        @if !away.is_empty() { span.au-n { (away.len()) } }
                    }
                    @if away.is_empty() {
                        (ui::empty(Icon::Moon, "Nobody is AFK", "Members show up here after they use /afk."))
                    } @else {
                        div.au-aways {
                            @for a in &away {
                                div.au-away {
                                    span.au-face {
                                        img.avatar src=(ctx.member_avatar(guild, a.user_id)) alt="" width="36" height="36" loading="lazy";
                                        span.au-zz aria-hidden="true" { (ui::icon(Icon::Moon)) }
                                    }
                                    div.grow {
                                        strong.clip { (name_or_stored(&ctx, guild, a.user_id, &a.name)) }
                                        @if !a.reason.is_empty() { span.au-reason { (a.reason) } }
                                        span.au-meta {
                                            span { "since " (text::ago(a.since_ms)) }
                                            @if let Some((zone, l)) = zone_of.get(&a.user_id) {
                                                span data-offset=(l.offset_secs) { b.au-clock { (l.clock) } " in " (zones::place(zone, &l.offset)) }
                                            }
                                        }
                                    }
                                    @if page.can_edit() {
                                        (ui::action_button(&page.path(&format!("{KEY}/afk/{}/clear", a.user_id)), "Clear", Tone::Muted, false))
                                    }
                                }
                            }
                        }
                    }
                }
                div.rail-block {
                    h2 { "Options" }
                    (ui::settings_form(&page, &page.path(KEY), html! {
                        div.au-opts {
                            (ui::switch_row("afk_enabled", "AFK", Some("Off: /afk does nothing, mentions get no notice, and anyone away now is back."), s.afk_enabled))
                            (ui::switch_row("nick_prefix", "Put [AFK] in their nickname", Some("Skipped for the owner and for members above the bot's role."), s.nick_prefix))
                            (ui::switch_row("welcome_back", "Say welcome back when they return", Some("A short message that deletes itself after a few seconds."), s.welcome_back))
                            (ui::switch_row("timezones_enabled", "Timezones", Some("Off: /timezone and /time do nothing. Saved zones are kept."), s.timezones_enabled))
                        }
                    }))
                }
            }
        }
        script { (PreEscaped(JS)) }
    };
    Ok(ui::layout(&ctx, &page, "AFK and timezones", None, body).into_response())
}

async fn save_settings(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let was_on = ctx.settings::<Settings>(guild, KEY)?.afk_enabled;
    let s = Settings {
        afk_enabled: form.bool("afk_enabled"),
        nick_prefix: form.bool("nick_prefix"),
        welcome_back: form.bool("welcome_back"),
        timezones_enabled: form.bool("timezones_enabled"),
    };
    ctx.save_settings(guild, KEY, &s)?;
    // With AFK off nobody's next message ends it, so end everyone's now and put nicknames back.
    if was_on && !s.afk_enabled {
        for a in afk_list(&ctx, guild)? {
            if let Some(was) = afk_clear(&ctx, guild, a.user_id)? {
                restore_nick(&ctx, guild, a.user_id, &was).await;
            }
        }
    }
    ctx.audit(&page.user, Some(guild), KEY, "Changed AFK and timezone settings");
    Ok(saved(&page.path(KEY)))
}

async fn clear_afk(State(ctx): State<Ctx>, user: User, Path((guild, member)): Path<(u64, u64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    if let Some(was) = afk_clear(&ctx, guild, member)? {
        restore_nick(&ctx, guild, member, &was).await;
        ctx.audit(&page.user, Some(guild), KEY, format!("Cleared AFK for {}", name_of(&ctx, guild, member)));
    }
    Ok(saved(&page.path(KEY)))
}

/// Keeps every clock on the page true while it stays open: the times, the hands, the pins on
/// the band and day or night.
const JS: &str = r#"(() => {
  const els = document.querySelectorAll(".au [data-offset]");
  if (!els.length) return;
  const days = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
  const pad = (n) => String(n).padStart(2, "0");
  const tick = () => {
    const now = Date.now();
    els.forEach((el) => {
      const off = Number(el.dataset.offset);
      if (!Number.isFinite(off)) return;
      const d = new Date(now + off * 1000);
      const h = d.getUTCHours(), m = d.getUTCMinutes();
      const clock = pad(h) + ":" + pad(m);
      el.querySelectorAll(".au-clock").forEach((c) => { if (c.textContent !== clock) c.textContent = clock; });
      const day = el.querySelector(".au-day");
      if (day) day.textContent = days[d.getUTCDay()];
      el.style.setProperty("--h", ((h % 12) * 30 + m * 0.5) + "deg");
      el.style.setProperty("--m", (m * 6) + "deg");
      if (el.classList.contains("au-track")) el.style.setProperty("--x", ((h * 60 + m) / 14.4).toFixed(2));
      if (el.classList.contains("au-zone")) el.classList.toggle("is-day", h >= 6 && h < 18);
    });
  };
  setInterval(tick, 30000);
})();"#;

const CSS: &str = r#"
.au-main { min-width: 0; gap: 30px; }

/* The band: a day from midnight to midnight, everyone pinned at the time their own clock shows. */
.au-sky { overflow-x: auto; overflow-y: hidden; margin: -6px -10px 0; padding: 6px 10px 4px; scrollbar-width: thin; }
.au-stage { position: relative; min-width: 620px; padding: 0 26px; }
.au-pins { position: relative; z-index: 1; height: calc(var(--rows) * 58px + 14px); }
.au-track { position: absolute; inset: 0; pointer-events: none; transform: translateX(calc(var(--x) * 1%)); transition: transform 1.2s var(--ease); animation: au-glide 1.5s var(--ease) backwards; animation-delay: calc(var(--i) * 90ms + 260ms); }
.au-pin { position: absolute; left: 0; bottom: 0; display: flex; flex-direction: column; align-items: center; transform: translateX(-50%); pointer-events: auto; cursor: default; }
.au-pin-faces { display: flex; align-items: center; padding: 3px; border-radius: 20px; background: var(--solid); box-shadow: inset 0 0 0 1px var(--edge-2), 0 12px 26px -12px rgba(0, 0, 0, 0.85); transition: transform 0.35s var(--spring), box-shadow 0.25s; }
.au-pin:hover .au-pin-faces { transform: translateY(-3px) scale(1.07); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--hue) 55%, transparent), 0 14px 30px -12px var(--hue); }
.au-pin-faces .avatar { width: 30px; height: 30px; box-shadow: 0 0 0 2px var(--solid); }
.au-pin-faces .avatar + .avatar, .au-pin-faces .au-more { margin-left: -9px; }
.au-more { display: grid; place-items: center; min-width: 30px; height: 30px; padding: 0 6px; border-radius: 14px; background: var(--control); box-shadow: 0 0 0 2px var(--solid); font: 600 11px var(--mono); color: var(--ink-2); }
.au-pin .au-clock { margin-top: 5px; padding: 2px 6px; border-radius: 6px; background: color-mix(in oklab, var(--page) 70%, transparent); font: 600 11px/1.3 var(--mono); color: var(--ink); font-variant-numeric: tabular-nums; }
.au-stem { width: 1px; height: calc(var(--row) * 58px + 7px); background: linear-gradient(180deg, var(--edge-3), color-mix(in oklab, var(--ink) 55%, transparent)); }
.au-pin::after { content: ""; width: 9px; height: 9px; margin-bottom: -10px; border-radius: 50%; background: var(--ink); box-shadow: 0 0 0 2.5px var(--page), 0 0 12px rgba(255, 255, 255, 0.45); }
.au-band { position: relative; height: 12px; border-radius: 6px; background: linear-gradient(90deg, #121633 0%, #1b2250 18%, #8a5a8c 22.5%, #e98c72 25.5%, #f2c585 29%, #8fc1ec 37%, #9cc9f0 50%, #8fc1ec 63%, #f2bd7c 70.5%, #e5826c 74.5%, #7f4a82 78%, #242456 83%, #121633 100%); box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.08), 0 10px 30px -12px rgba(143, 193, 236, 0.45); }
.au-ticks { position: relative; height: 26px; margin-top: 6px; }
.au-ticks span { position: absolute; top: 6px; transform: translateX(-50%); font: 11px var(--mono); color: var(--ink-3); }
.au-ticks span::before { content: ""; position: absolute; left: 50%; top: -9px; width: 1px; height: 5px; background: var(--edge-3); }
@keyframes au-glide { from { transform: translateX(0); opacity: 0; } 25% { opacity: 1; } }

/* One card per zone: a clock face, the time, the place and the people there. */
.au-zones { display: grid; grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); gap: 12px; }
.au-zone { --tint: var(--hue); position: relative; display: flex; flex-direction: column; gap: 12px; min-width: 0; padding: 16px 16px 15px; border-radius: var(--r-card); background: radial-gradient(130% 120% at 100% 0%, color-mix(in oklab, var(--tint) 14%, transparent), transparent 60%), linear-gradient(180deg, rgba(243, 242, 248, 0.045), rgba(243, 242, 248, 0.015)); box-shadow: inset 0 0 0 1px var(--edge-1); transition: transform 0.35s var(--ease), box-shadow 0.25s; }
.au-zone.is-day { --tint: var(--waiting); }
.au-zone:hover { transform: translateY(-2px); box-shadow: inset 0 0 0 1px var(--edge-2), var(--shadow-lift); }
.au-zone-top { display: flex; align-items: center; gap: 14px; }
.au-dial { flex: none; width: 56px; height: 56px; overflow: visible; }
.au-dial .face { fill: color-mix(in oklab, var(--tint) 11%, var(--raised)); stroke: color-mix(in oklab, var(--tint) 40%, transparent); stroke-width: 1.2; }
.au-dial .tk { stroke: var(--ink-3); stroke-width: 1; stroke-linecap: round; }
.au-dial .tk.q { stroke: var(--ink-2); stroke-width: 1.5; }
.au-dial .hh, .au-dial .mh { stroke-linecap: round; transform-box: view-box; transform-origin: 50% 50%; animation: au-swing 1.5s var(--spring) backwards; animation-delay: calc(var(--i) * 70ms + 520ms); }
.au-dial .hh { stroke: var(--ink); stroke-width: 2.6; transform: rotate(var(--h)); }
.au-dial .mh { stroke: var(--tint); stroke-width: 1.7; transform: rotate(var(--m)); filter: drop-shadow(0 0 3px color-mix(in oklab, var(--tint) 60%, transparent)); }
.au-dial .pivot { fill: var(--tint); stroke: var(--raised); stroke-width: 1; }
@keyframes au-swing { from { transform: rotate(0deg); } }
.au-zone-time { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.au-zone-time .au-clock { font: 600 27px/1.05 var(--mono); letter-spacing: -0.03em; color: var(--ink); font-variant-numeric: tabular-nums; }
.au-zone-sub { display: flex; align-items: center; gap: 5px; font-size: 12px; color: var(--ink-3); white-space: nowrap; }
.au-glyph { display: inline-grid; place-items: center; }
.au-glyph .icon { width: 14px; height: 14px; }
.au-sun { color: var(--waiting); display: none; }
.au-moon { color: var(--hue); }
.au-zone.is-day .au-sun { display: inline-grid; }
.au-zone.is-day .au-moon { display: none; }
.au-place { display: flex; align-items: baseline; gap: 8px; min-width: 0; }
.au-place strong { font-size: 14px; font-weight: 600; letter-spacing: -0.01em; min-width: 0; }
.au-place span { flex: none; font: 11.5px var(--mono); color: var(--ink-3); }
.au-people { display: flex; flex-wrap: wrap; gap: 6px; margin-top: -2px; }
.au-who { display: inline-flex; align-items: center; gap: 7px; max-width: 100%; height: 28px; padding: 0 10px 0 3px; border-radius: 14px; background: var(--control); font-size: 12.5px; color: var(--ink-2); transition: background 0.18s, color 0.18s; }
.au-who:hover { background: var(--hover); color: var(--ink); }
.au-who .avatar { width: 22px; height: 22px; }
.au-who span { min-width: 0; }
.au-crowd { display: flex; align-items: center; gap: 10px; min-width: 0; font-size: 12.5px; color: var(--ink-2); }
.au-crowd .avatar { width: 26px; height: 26px; }

/* The rail: who's away, with the time where they are. */
.au-n { display: inline-grid; place-items: center; min-width: 22px; height: 20px; padding: 0 7px; border-radius: 10px; background: color-mix(in oklab, var(--hue) 18%, transparent); color: var(--hue); font: 600 11px var(--mono); }
.au-aways { display: flex; flex-direction: column; gap: 2px; margin: 0 -10px; }
.au-away { display: flex; align-items: center; gap: 12px; padding: 9px 10px; border-radius: 12px; transition: background 0.18s; }
.au-away:hover { background: var(--well); }
.au-face { position: relative; flex: none; display: block; }
.au-face .avatar { display: block; width: 36px; height: 36px; }
.au-zz { position: absolute; right: -4px; bottom: -3px; display: grid; place-items: center; width: 18px; height: 18px; border-radius: 50%; background: var(--solid); color: var(--hue); box-shadow: 0 0 0 2px color-mix(in oklab, var(--raised) 90%, transparent), inset 0 0 0 1px color-mix(in oklab, var(--hue) 35%, transparent); }
.au-zz .icon { width: 11px; height: 11px; }
.au-away .grow { display: flex; flex-direction: column; gap: 1px; min-width: 0; }
.au-away strong { font-size: 13.5px; font-weight: 600; }
.au-reason { font-size: 12.5px; color: var(--ink-2); line-height: 1.4; overflow-wrap: anywhere; }
.au-meta { display: flex; flex-wrap: wrap; gap: 0 10px; font-size: 12px; color: var(--ink-3); }
.au-meta .au-clock { font: 500 11.5px var(--mono); color: var(--ink-2); font-variant-numeric: tabular-nums; }
.au-away .btn { flex: none; }
.au-opts { display: flex; flex-direction: column; }
.au-opts .switch-row label { font-size: 13px; }
.au-opts .help { font-size: 12px; }
/* The save button sits under the switches here; floating, it would cover them in the rail. */
.au .rail .save-bar { position: static; margin-top: 4px; }
@media (max-width: 920px) {
  .au-sky { -webkit-mask-image: linear-gradient(90deg, #000 82%, transparent); mask-image: linear-gradient(90deg, #000 82%, transparent); }
  .au-zones { grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 10px; }
  .au-zone { padding: 13px 13px 12px; gap: 10px; }
  .au-zone-top { gap: 10px; }
  .au-dial { width: 40px; height: 40px; }
  .au-zone-time .au-clock { font-size: 21px; }
  .au-who { padding-right: 8px; }
}
@media (prefers-reduced-motion: reduce) {
  .au-track, .au-dial .hh, .au-dial .mh { animation: none; transition: none; }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_join_and_count() {
        let n = |v: &[&str]| names_line(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(n(&["Mira"]), "Mira");
        assert_eq!(n(&["Mira", "Teo"]), "Mira and Teo");
        assert_eq!(n(&["Mira", "Teo", "Lu"]), "Mira, Teo and Lu");
        assert_eq!(n(&["Mira", "Teo", "Lu", "Kai"]), "Mira, Teo and 2 others");
    }

    #[test]
    fn clocks_read_as_minutes_and_hands() {
        assert_eq!(day_minutes("00:00"), 0);
        assert_eq!(day_minutes("14:32"), 14 * 60 + 32);
        assert_eq!(hands("15:30"), "--h:105deg;--m:180deg");
        assert_eq!(hands("00:00"), "--h:0deg;--m:0deg");
    }

    #[test]
    fn close_pins_stack_and_far_ones_share_a_row() {
        // Left to right: 08:00 takes the first row, 08:30 the second, 09:00 is too close to
        // both and takes a third, and 14:00 has room next to 08:00 again.
        let rows = pin_rows(&[8 * 60, 9 * 60, 14 * 60, 8 * 60 + 30]);
        assert_eq!(rows, vec![0, 2, 0, 1]);
        assert!(pin_rows(&[]).is_empty());
    }

    #[test]
    fn zones_group_into_clocks_and_offsets_into_pins() {
        let now = Timestamp::now();
        let at = |u: u64, z: &str| (u, z.to_owned(), zones::local(z, now).unwrap());
        let mut times = vec![at(1, "Europe/Warsaw"), at(2, "Europe/Berlin"), at(3, "Europe/Warsaw"), at(4, "Asia/Tokyo")];
        times.sort_by(|a, b| a.2.offset_secs.cmp(&b.2.offset_secs).then_with(|| a.1.cmp(&b.1)));
        let c = clocks(&times);
        assert_eq!(c.iter().map(|c| (c.zone, c.users.len())).collect::<Vec<_>>(), vec![("Europe/Berlin", 1), ("Europe/Warsaw", 2), ("Asia/Tokyo", 1)]);
        let p = pins(&c);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].users.len(), 3);
        assert_eq!(p[0].places, vec!["Berlin".to_owned(), "Warsaw".to_owned()]);
    }
}
