//! The dashboard's building blocks. Features compose their pages from these so every page has
//! the same shell, spacing and controls. Classes are defined in `assets/app.css`.

use std::collections::HashMap;

use maud::{DOCTYPE, Markup, PreEscaped, html};

use super::auth::{Level, Page};
use crate::{Ctx, Group};

/// The dashboard's icons. The ones that stand for a page are the custom set drawn for the
/// dashboard on a 24-unit grid; small utility glyphs stay on a 16-unit grid. Both draw at the
/// same weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Home,
    Live,
    Calendar,
    Play,
    Trophy,
    Log,
    Wave,
    Door,
    Gift,
    Poll,
    Bulb,
    Ticket,
    Star,
    Clock,
    Afk,
    Moon,
    Paw,
    Shield,
    Plug,
    Brush,
    Chart,
    Users,
    Toggle,
    Activity,
    Bell,
    Plus,
    Trash,
    Check,
    Close,
    Chevron,
    External,
    Menu,
    Logout,
    Hash,
    Search,
    Mail,
    Rules,
    /// Custom commands: a key with a slash.
    Slash,
    /// Account reviews: a clipboard with a ticked line.
    Review,
    /// A picture: the overview's banner.
    Image,
    /// Server settings.
    Gear,
    /// "Open": an arrow into a row.
    Enter,
    /// People online, members.
    Member,
}

impl Icon {
    /// `(grid, svg body)`.
    fn draw(self) -> (u8, &'static str) {
        match self {
            Icon::Home => (24, r#"<path d="M5.5 4h4A1.5 1.5 0 0 1 11 5.5v4A1.5 1.5 0 0 1 9.5 11h-4A1.5 1.5 0 0 1 4 9.5v-4A1.5 1.5 0 0 1 5.5 4ZM14.5 4h4A1.5 1.5 0 0 1 20 5.5v4a1.5 1.5 0 0 1-1.5 1.5h-4A1.5 1.5 0 0 1 13 9.5v-4A1.5 1.5 0 0 1 14.5 4ZM5.5 13h4a1.5 1.5 0 0 1 1.5 1.5v4A1.5 1.5 0 0 1 9.5 20h-4A1.5 1.5 0 0 1 4 18.5v-4A1.5 1.5 0 0 1 5.5 13ZM13 17h2l1.5-3 2 6 1.5-3h1"/>"#),
            Icon::Live => (24, r#"<path d="M10.25 9a1.75 1.75 0 1 0 3.5 0 1.75 1.75 0 1 0-3.5 0M12 11v9M9 20h6M8.1 5.1a5.5 5.5 0 0 0 0 7.8M15.9 5.1a5.5 5.5 0 0 1 0 7.8M5.3 2.3a9.5 9.5 0 0 0 0 13.4M18.7 2.3a9.5 9.5 0 0 1 0 13.4"/>"#),
            Icon::Calendar => (16, r#"<rect x="2.5" y="3.5" width="11" height="10" rx="1.5"/><path d="M2.5 6.5h11M5.5 2v3M10.5 2v3"/>"#),
            Icon::Play => (16, r#"<rect x="1.5" y="3.5" width="13" height="9" rx="2.5"/><path d="m6.75 6 3.25 2-3.25 2z"/>"#),
            Icon::Trophy => (24, r#"<path d="m6 19.5 6-4 6 4M6 14l6-4 6 4M12 3l1.2 2.4 2.6.4-1.9 1.8.5 2.6L12 9l-2.4 1.2.5-2.6-1.9-1.8 2.6-.4z"/>"#),
            Icon::Log => (24, r#"<path d="M6.5 3h8l4 4v13a1 1 0 0 1-1 1h-11a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1zM14.5 3v4h4M8.5 14.5s1.5-2.5 3.5-2.5 3.5 2.5 3.5 2.5-1.5 2.5-3.5 2.5-3.5-2.5-3.5-2.5zM11.25 14.5a.75.75 0 1 0 1.5 0 .75.75 0 1 0-1.5 0"/>"#),
            Icon::Wave => (24, r#"<path d="M8 13.5V7a1.5 1.5 0 0 1 3 0v4.5V5a1.5 1.5 0 0 1 3 0v6.5-5a1.5 1.5 0 0 1 3 0v7c0 4-2.5 7-6.5 7-2.4 0-4-1.2-5.5-3.4l-1.8-2.7a1.5 1.5 0 0 1 2.5-1.7L8 15M19.5 3.5c.8.6 1.3 1.5 1.5 2.5M3 8c.3-1 .9-1.8 1.7-2.4"/>"#),
            Icon::Door => (16, r#"<path d="M3.5 13.5V3a.5.5 0 0 1 .5-.5h6v11M10 4.5h2.5v9M2 13.5h12M7.5 8.25v.5"/>"#),
            Icon::Gift => (24, r#"<path d="M6 11h12a1.5 1.5 0 0 1 1.5 1.5v6A1.5 1.5 0 0 1 18 20H6a1.5 1.5 0 0 1-1.5-1.5v-6A1.5 1.5 0 0 1 6 11ZM4 7.5h16a1 1 0 0 1 1 1V10a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V8.5a1 1 0 0 1 1-1ZM12 7.5V20M12 7.5C10.6 4.6 7.2 4 7.2 5.8S10.2 7.5 12 7.5zM12 7.5c1.4-2.9 4.8-3.5 4.8-1.7S13.8 7.5 12 7.5z"/>"#),
            Icon::Poll => (24, r#"<path d="M5 4.5h14A1.5 1.5 0 0 1 20.5 6v9.5A1.5 1.5 0 0 1 19 17h-9l-4.5 3.5V17H5a1.5 1.5 0 0 1-1.5-1.5V6A1.5 1.5 0 0 1 5 4.5zM8.5 13.5V11M12 13.5V8M15.5 13.5v-4"/>"#),
            Icon::Bulb => (24, r#"<path d="M9.5 18h5M10.5 21h3M8.6 14.6a5.5 5.5 0 1 1 6.8 0c-.8.6-1.4 1.5-1.4 2.4h-4c0-.9-.6-1.8-1.4-2.4zM19.5 2.5v3M18 4h3"/>"#),
            Icon::Ticket => (24, r#"<path d="M3.5 7.5A1.5 1.5 0 0 1 5 6h14a1.5 1.5 0 0 1 1.5 1.5v2.25a2.25 2.25 0 0 0 0 4.5v2.25A1.5 1.5 0 0 1 19 18H5a1.5 1.5 0 0 1-1.5-1.5v-2.25a2.25 2.25 0 0 0 0-4.5zM15 6.5V8M15 11v2M15 16v1.5M7 10.5h4.5M7 13.5h3"/>"#),
            Icon::Star => (24, r#"<path d="m10.5 5 2 4.4 4.8.6-3.6 3.3 1 4.7-4.2-2.4-4.2 2.4 1-4.7L3.7 10l4.8-.6zM19 3v3M17.5 4.5h3M19.5 14.5v2M18.5 15.5h2"/>"#),
            Icon::Clock => (16, r#"<circle cx="8" cy="8" r="5.75"/><path d="M8 4.75V8l2.25 1.5"/>"#),
            Icon::Afk => (24, r#"<path d="M3.5 13.5a7 7 0 1 0 14 0 7 7 0 1 0-14 0M10.5 10v3.5l2.5 1.5M19 2.5a3 3 0 1 0 2.5 4.6A2.6 2.6 0 0 1 19 2.5z"/>"#),
            Icon::Moon => (16, r#"<path d="M13 9.5A5.5 5.5 0 0 1 6.5 3a5.5 5.5 0 1 0 6.5 6.5Z"/>"#),
            Icon::Paw => (24, r#"<path d="M4.9 10.5a1.6 2.1 0 1 0 3.2 0 1.6 2.1 0 1 0-3.2 0M8.3 6.9a1.6 2.1 0 1 0 3.2 0 1.6 2.1 0 1 0-3.2 0M12.5 6.9a1.6 2.1 0 1 0 3.2 0 1.6 2.1 0 1 0-3.2 0M15.9 10.5a1.6 2.1 0 1 0 3.2 0 1.6 2.1 0 1 0-3.2 0M12 12.5c-2.6 0-5 2.7-5 4.8 0 1.4 1.1 2.4 2.4 2.4 1 0 1.6-.5 2.6-.5s1.6.5 2.6.5c1.3 0 2.4-1 2.4-2.4 0-2.1-2.4-4.8-5-4.8z"/>"#),
            Icon::Shield => (24, r#"<path d="m12 3 7 3v5c0 4.6-3 8-7 10-4-2-7-5.4-7-10V6zM12.5 7v6a2.25 2.25 0 0 1-4.5 0v-1M12.5 7H14"/>"#),
            Icon::Plug => (24, r#"<path d="M6 3h6a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2ZM8 17.5h2M16 11.5l2 2 4-4"/>"#),
            Icon::Brush => (24, r#"<path d="M5.5 4h13A1.5 1.5 0 0 1 20 5.5v13a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 18.5v-13A1.5 1.5 0 0 1 5.5 4ZM7.5 4v16M10.5 9h6M10.5 12.5h4M10.5 16h5"/>"#),
            Icon::Chart => (16, r#"<path d="M2.5 2.5v11h11"/><path d="m5 10 2.5-3 2 2L13 5"/>"#),
            Icon::Users => (24, r#"<path d="M6 8.5a3 3 0 1 0 6 0 3 3 0 1 0-6 0M3 20c.5-3.5 3-5.5 6-5.5s5.5 2 6 5.5M14.5 5.9a3 3 0 1 1 1.5 5.6M17.6 14.6c1.8.8 3 2.6 3.4 5.4"/>"#),
            Icon::Toggle => (24, r#"<path d="M6 4.5h8a3 3 0 0 1 0 6H6a3 3 0 0 1 0-6ZM10 13.5h8a3 3 0 0 1 0 6h-8a3 3 0 0 1 0-6ZM12.75 7.5a1.25 1.25 0 1 0 2.5 0 1.25 1.25 0 1 0-2.5 0M8.75 16.5a1.25 1.25 0 1 0 2.5 0 1.25 1.25 0 1 0-2.5 0"/>"#),
            Icon::Activity => (24, r#"<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8M3 3v5h5M12 7v5l4 2"/>"#),
            Icon::Bell => (24, r#"<path d="M6 16.5V11a6 6 0 0 1 12 0v5.5l1.5 2h-15l1.5-2M10 21h4"/>"#),
            Icon::Plus => (16, r#"<path d="M8 3v10M3 8h10"/>"#),
            Icon::Trash => (16, r#"<path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.5 9h6l.5-9"/>"#),
            Icon::Check => (16, r#"<path d="m3 8.5 3 3 7-7"/>"#),
            Icon::Close => (16, r#"<path d="m4 4 8 8M12 4l-8 8"/>"#),
            Icon::Chevron => (16, r#"<path d="m4.5 6.5 3.5 3.5 3.5-3.5"/>"#),
            Icon::External => (16, r#"<path d="M9 2.5h4.5V7M13.5 2.5 7.5 8.5M11.5 9.5v3a1 1 0 0 1-1 1h-7a1 1 0 0 1-1-1v-7a1 1 0 0 1 1-1h3"/>"#),
            Icon::Search => (16, r#"<path d="M3 7a4 4 0 1 0 8 0 4 4 0 1 0-8 0M10 10l3.5 3.5"/>"#),
            Icon::Menu => (16, r#"<path d="M2.5 4.5h11M2.5 8h11M2.5 11.5h11"/>"#),
            Icon::Logout => (24, r#"<path d="M10 4.5H6.5A1.5 1.5 0 0 0 5 6v12a1.5 1.5 0 0 0 1.5 1.5H10M14.5 8l4 4-4 4M18.5 12h-9"/>"#),
            Icon::Hash => (16, r#"<path d="M6 2.5 5 13.5M11 2.5l-1 11M2.75 6h11M2.25 10h11"/>"#),
            Icon::Mail => (24, r#"<path d="M5 5.5h14A1.5 1.5 0 0 1 20.5 7v10a1.5 1.5 0 0 1-1.5 1.5H5A1.5 1.5 0 0 1 3.5 17V7A1.5 1.5 0 0 1 5 5.5zM4 7l8 6 8-6"/>"#),
            Icon::Rules => (24, r#"<path d="M10 4 8.25 20M16.5 4l-.9 8.5M4.5 9h15M4 15h8.5M14 17.5l2 2 4-4"/>"#),
            Icon::Slash => (24, r#"<path d="M6 4.5h12A1.5 1.5 0 0 1 19.5 6v12a1.5 1.5 0 0 1-1.5 1.5H6A1.5 1.5 0 0 1 4.5 18V6A1.5 1.5 0 0 1 6 4.5ZM14 8.5l-4 7"/>"#),
            Icon::Review => (24, r#"<path d="M8.5 5H7a1.5 1.5 0 0 0-1.5 1.5v13A1.5 1.5 0 0 0 7 21h10a1.5 1.5 0 0 0 1.5-1.5v-13A1.5 1.5 0 0 0 17 5h-1.5M9.5 3.5h5a1 1 0 0 1 1 1V6a1 1 0 0 1-1 1h-5a1 1 0 0 1-1-1V4.5a1 1 0 0 1 1-1ZM8.75 12.25l1.5 1.5 2.75-2.75M14.75 12.5h.5M8.75 17h6.5"/>"#),
            Icon::Image => (24, r#"<path d="M5.5 4h13A1.5 1.5 0 0 1 20 5.5v13a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 18.5v-13A1.5 1.5 0 0 1 5.5 4ZM4 16l4.5-4.5a1.5 1.5 0 0 1 2.1 0L16 17M14 14.5l1.4-1.4a1.5 1.5 0 0 1 2.1 0L20 15.5M14 8.75a1.25 1.25 0 1 0 2.5 0 1.25 1.25 0 1 0-2.5 0"/>"#),
            Icon::Gear => (24, r#"<path d="M9.6 4.2 10.2 2h3.6l.6 2.2 1.9 1.1 2.2-.6 1.8 3.1-1.6 1.6v2.2l1.6 1.6-1.8 3.1-2.2-.6-1.9 1.1-.6 2.2h-3.6l-.6-2.2-1.9-1.1-2.2.6-1.8-3.1 1.6-1.6v-2.2L3.7 8.4l1.8-3.1 2.2.6zM9.25 12a2.75 2.75 0 1 0 5.5 0 2.75 2.75 0 1 0-5.5 0"/>"#),
            Icon::Enter => (16, r#"<path d="M3 8h9.5M9 4.5 12.5 8 9 11.5"/>"#),
            Icon::Member => (16, r#"<circle cx="8" cy="5.5" r="2.5"/><path d="M3 13.5c.5-2.5 2.5-4 5-4s4.5 1.5 5 4"/>"#),
        }
    }
}

pub fn icon(i: Icon) -> Markup {
    let (grid, body) = i.draw();
    let (view, stroke) = if grid == 24 { ("0 0 24 24", "1.75") } else { ("0 0 16 16", "1.3") };
    html! {
        svg.icon viewBox=(view) width="16" height="16" fill="none" stroke="currentColor" stroke-width=(stroke) stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" {
            (PreEscaped(body))
        }
    }
}

/// `<head>` contents shared by every page. The inline script runs before the first paint: it
/// puts back the server colour this browser sampled last time (so the page doesn't flash
/// violet first) and notes whether a view transition brought the page in.
pub fn head(title: &str) -> Markup {
    html! {
        meta charset="utf-8";
        meta name="viewport" content="width=device-width, initial-scale=1";
        meta name="robots" content="noindex";
        meta name="color-scheme" content="dark";
        meta name="theme-color" content="#07070b";
        title { (title) " · Bot" }
        link rel="icon" href="/static/icon.svg";
        link rel="preconnect" href="https://fonts.googleapis.com";
        link rel="preconnect" href="https://fonts.gstatic.com" crossorigin;
        link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Geist:wght@400;500;600;700&family=Geist+Mono:wght@400;500;600&display=swap";
        link rel="stylesheet" href="/static/app.css";
        script { (PreEscaped(EARLY_JS)) }
        script src="/static/app.js" defer {}
    }
}

const EARLY_JS: &str = r#"(function(){var d=document.documentElement;try{var m=location.pathname.match(/^\/g\/(\d+)/),a=m&&localStorage.getItem("accent:"+m[1]);if(a)d.style.setProperty("--accent",a)}catch(e){}addEventListener("pagereveal",function(e){if(e.viewTransition)d.classList.add("vt")});addEventListener("pageswap",function(){d.classList.remove("vt")})})()"#;

/// The signed-in shell: sidebar with the server switcher and the pages this person can open,
/// then the page with its header (the page's icon, title, one line on what it does).
/// `actions` sits at the right of the title.
pub fn layout(ctx: &Ctx, page: &Page, title: &str, actions: Option<Markup>, body: Markup) -> Markup {
    let (ic, line, hue) = identity(ctx, page, title);
    let unread = unread_by_feature(ctx, page);
    let head = html! {
        div.page-glow aria-hidden="true" {}
        header.page-head {
            span.head-icon { (icon(ic)) }
            div.head-text {
                h1 { (title) }
                @if let Some(l) = line { p { (l) } }
            }
            @if let Some(a) = actions { div.head-actions { (a) } }
        }
    };
    shell(ctx, page, title, hue, &unread, head, html! { div.page-body { (body) } })
}

/// The shell with a page that draws its own header (the overview's banner).
pub fn layout_custom(ctx: &Ctx, page: &Page, title: &str, body: Markup) -> Markup {
    shell(ctx, page, title, "core", &unread_by_feature(ctx, page), html! {}, body)
}

/// The notification bell with its unread count. The server rail carries it on every page;
/// this is for a page that wants one of its own.
pub fn bell(ctx: &Ctx, page: &Page) -> Markup {
    bell_count(page, unread_by_feature(ctx, page).values().sum(), "icon-btn bell")
}

fn bell_count(page: &Page, unread: i64, class: &str) -> Markup {
    html! {
        a class=(class) href={ "/g/" (page.guild_id) "/notifications" } data-tip="Notifications" aria-label={ "Notifications, " (unread) " new" } {
            (icon(Icon::Bell))
            @if unread > 0 { span.count { (if unread > 99 { "99+".to_owned() } else { unread.to_string() }) } }
        }
    }
}

/// The page's icon, the one line under its title, and its group's hue: the feature's own, or
/// the core page's.
fn identity(ctx: &Ctx, page: &Page, title: &str) -> (Icon, Option<&'static str>, &'static str) {
    match page.key {
        "overview" => (Icon::Home, None, "core"),
        "features" => (Icon::Toggle, Some("A feature that's off ignores the server, and its commands disappear there."), "core"),
        "users" => (Icon::Users, Some("Only people you add can open the dashboard, and they see only the pages you give them."), "core"),
        "activity" => (Icon::Activity, Some("Every change made on the dashboard, with who made it."), "core"),
        "notifications" => (Icon::Bell, Some("What features flagged for you, newest first."), "core"),
        key => match ctx.feature(key) {
            Some(f) => {
                let meta = f.meta();
                let nav = meta.nav.iter().find(|n| n.label == title).or(meta.nav.first());
                let ic = nav.map(|n| n.icon).unwrap_or(Icon::Toggle);
                (ic, Some(nav.map(|n| n.about).filter(|a| !a.is_empty()).unwrap_or(meta.description)), meta.group.hue())
            }
            None => (Icon::Home, None, "core"),
        },
    }
}

/// The hue of a feature's group, for markup outside the shell that wants to match the
/// sidebar (notification rows, needs on the overview). "core" when the feature is unknown.
pub fn feature_hue(ctx: &Ctx, key: &str) -> &'static str {
    ctx.feature(key).map(|f| f.meta().group.hue()).unwrap_or("core")
}

#[allow(clippy::too_many_arguments)]
fn shell(ctx: &Ctx, page: &Page, title: &str, hue: &str, unread: &HashMap<String, i64>, top: Markup, body: Markup) -> Markup {
    let guild = page.guild_id;
    let guilds = ctx.guilds_for(&page.user);
    let levels = ctx.levels(&page.user, guild);
    // label, path, icon, about, unread count (on a feature's first page only, so a feature with
    // several pages counts once)
    type Link = (&'static str, &'static str, Icon, &'static str, i64);
    let mut groups: Vec<(Group, Vec<Link>)> = Vec::new();
    // The page this is, marked by the server so the indicator is in place on the first paint.
    let mut current: Option<&'static str> = None;
    for f in ctx.features() {
        let meta = f.meta();
        if !ctx.enabled(guild, meta.key) || levels.get(meta.key) == Level::None {
            continue;
        }
        let waiting = unread.get(meta.key).copied().unwrap_or(0);
        let items: Vec<Link> = meta.nav.iter().enumerate().map(|(i, n)| (n.label, n.path, n.icon, n.about, if i == 0 { waiting } else { 0 })).collect();
        if items.is_empty() {
            continue;
        }
        if meta.key == page.key {
            current = meta.nav.iter().find(|n| n.label == title).or(meta.nav.first()).map(|n| n.path);
        }
        match groups.iter_mut().find(|(g, ..)| *g == meta.group) {
            Some((_, list)) => list.extend(items),
            None => groups.push((meta.group, items)),
        }
    }
    groups.sort_by_key(|(g, ..)| *g);
    let key = page.key;
    let total_unread: i64 = unread.values().sum();
    let name = ctx.guild_name(guild);
    let members = ctx.member_count(guild);
    let banner = ctx.guild_banner_url(guild);
    let icon_url = ctx.guild_icon_large(guild);
    let me = page.user.avatar_url();

    html! {
        (DOCTYPE)
        html lang="en" {
            head { (head(title)) }
            body data-guild=(guild) data-icon=[icon_url.as_deref()] {
                a.skip href="#main" { "Skip to the page" }
                header.topbar {
                    button.icon-btn.menu-btn type="button" aria-label="Menu" aria-expanded="false" { (icon(Icon::Menu)) }
                    (guild_avatar(ctx, guild))
                    strong.grow.clip { (name) }
                    button.icon-btn type="button" data-palette aria-label="Search" { (icon(Icon::Search)) }
                    (bell_count(page, total_unread, "icon-btn bell"))
                }
                div.scrim {}
                div.shell {
                    nav.guilds aria-label="Servers" {
                        a.guild.mark href="/" data-tip="Dashboard" aria-label="Dashboard home" { (mark()) }
                        span.guilds-sep {}
                        div.guilds-list {
                            @for g in &guilds {
                                a.guild.on[*g == guild] href={ "/g/" (g) } data-tip=(ctx.guild_name(*g)) aria-label=(ctx.guild_name(*g)) aria-current=[(*g == guild).then_some("true")] {
                                    @match ctx.guild_icon_url(*g) {
                                        Some(u) => img src=(u) alt="" width="46" height="46";,
                                        None => (initial(&ctx.guild_name(*g))),
                                    }
                                }
                            }
                        }
                        div.guilds-foot {
                            (bell_count(page, total_unread, "rail-btn bell"))
                            details.pop.account {
                                summary aria-label="Your account" { img src=(me) alt="" width="38" height="38"; }
                                div.pop-body.account-menu {
                                    div.account-who {
                                        img src=(me) alt="";
                                        div { strong { (page.user.name) } span.muted.small { @if page.user.admin { "Admin" } @else { "Member of the team" } } }
                                    }
                                    @if page.user.admin {
                                        a.menu-item.on[key == "features"] href={ "/g/" (guild) "/features" } { (icon(Icon::Toggle)) span { "Features" } }
                                        a.menu-item.on[key == "users"] href="/users" { (icon(Icon::Users)) span { "People" } }
                                        a.menu-item.on[key == "activity"] href="/activity" { (icon(Icon::Activity)) span { "Activity" } }
                                    }
                                    form method="post" action="/logout" {
                                        button.menu-item type="submit" data-signout { (icon(Icon::Logout)) span { "Sign out" } }
                                    }
                                }
                            }
                        }
                    }
                    nav.side aria-label="Dashboard" {
                        a.side-head href={ "/g/" (guild) } aria-label={ (name) ", overview" } {
                            @match (&banner, &icon_url) {
                                (Some(b), _) => img.cover src=(b) alt="";,
                                (None, i) => (art(i.as_deref(), true)),
                            }
                            div.side-head-name { (guild_avatar(ctx, guild)) strong.clip { (name) } }
                            div.side-head-meta {
                                @if ctx.cfg.preview { span.dot.waiting {} "Preview" }
                                @else if ctx.bot_id().is_some() { span.dot.live {} "Bot online" }
                                @else { span.dot.waiting {} "Connecting" }
                                @if let Some(m) = members { " · " (crate::text::num(m as i64)) " members" }
                            }
                        }
                        button.side-search type="button" data-palette {
                            (icon(Icon::Search))
                            span.grow { "Search" }
                            kbd.mac-only { "⌘K" }
                        }
                        div.side-links {
                            a.nav.on[key == "overview"] data-hue="core" href={ "/g/" (guild) } data-about="home summary overview" aria-current=[(key == "overview").then_some("page")] {
                                @if key == "overview" { span.nav-pill {} }
                                span.nav-ic { (icon(Icon::Home)) }
                                span.grow { "Overview" }
                            }
                            @for (group, items) in &groups {
                                @let count: i64 = items.iter().map(|l| l.4).sum();
                                details.nav-fold open data-group=(group.label()) data-hue=(group.hue()) {
                                    summary {
                                        span.swatch {}
                                        span.grow { (group.label()) }
                                        @if count > 0 { span.nav-count.folded { (count) span.sr { " unread" } } }
                                        (icon(Icon::Chevron))
                                    }
                                    @for (label, path, ic, about, waiting) in items {
                                        @let here = current == Some(*path);
                                        a.nav.on[here] href={ "/g/" (guild) "/" (path) } data-path={ "/g/" (guild) "/" (path) } data-about=(about) data-group=(group.label()) aria-current=[here.then_some("page")] {
                                            @if here { span.nav-pill {} }
                                            span.nav-ic { (icon(*ic)) }
                                            span.grow { (label) }
                                            @if *waiting > 0 { span.nav-count { (waiting) span.sr { " unread" } } }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    main.page #main tabindex="-1" data-hue=(hue) {
                        (top)
                        (body)
                    }
                }
                (palette())
            }
        }
    }
}

/// The search palette (⌘K, Ctrl+K or `/`). app.js fills it from the sidebar, the server rail
/// and the account menu, so it offers exactly what this person can open.
fn palette() -> Markup {
    html! {
        div.palette #palette role="dialog" aria-modal="true" aria-label="Search the dashboard" hidden {
            div.pal-box {
                label.pal-field {
                    (icon(Icon::Search))
                    input #pal-q type="text" placeholder="Search pages, servers and actions" aria-label="Search" autocomplete="off" spellcheck="false" aria-controls="pal-list" aria-activedescendant="";
                    kbd { "Esc" }
                }
                div.pal-list #pal-list role="listbox" {}
                footer.pal-foot aria-hidden="true" {
                    span { kbd { "↑" } kbd { "↓" } "move" }
                    span { kbd { "↵" } "open" }
                    span { kbd { "Esc" } "close" }
                }
            }
        }
    }
}

/// Generated artwork for a server with no banner: its icon, huge and blurred, under drifting
/// light in the server's colours. `still` stops the drift (the sidebar).
pub fn art(icon_url: Option<&str>, still: bool) -> Markup {
    html! {
        div.art.still[still] aria-hidden="true" {
            @if let Some(i) = icon_url { div.art-icon style={ "background-image:url(" (i) ")" } {} }
            span.art-blob.a {} span.art-blob.b {} span.art-blob.c {}
        }
    }
}

/// The bot's mark: a speech bubble with two eyes, drawn in the current colour. It sits on a
/// tile of the server's colour in the rail and on the sign-in card.
pub fn mark() -> Markup {
    html! {
        svg.icon viewBox="5 4.5 22 22" width="26" height="26" fill="none" aria-hidden="true" {
            path d="M8 11a3.5 3.5 0 0 1 3.5-3.5h9A3.5 3.5 0 0 1 24 11v6.5a3.5 3.5 0 0 1-3.5 3.5H15l-4.5 3.5V21A2.5 2.5 0 0 1 8 18.5z" stroke="currentColor" stroke-width="2" stroke-linejoin="round" {}
            circle cx="13" cy="14.25" r="1.6" fill="currentColor" {}
            circle cx="19" cy="14.25" r="1.6" fill="currentColor" {}
        }
    }
}

fn initial(name: &str) -> String {
    name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default()
}

/// A minimal public page with no sidebar, for pages anyone can see (pet collection, stats).
pub fn public_layout(title: &str, subtitle: Option<&str>, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head { (head(title)) }
            body.public {
                main.public-page {
                    header.public-head {
                        h1 { (title) }
                        @if let Some(s) = subtitle { p.muted { (s) } }
                    }
                    (body)
                }
            }
        }
    }
}

/// Unread notices this person may see, by feature key ("" for notices about no server).
fn unread_by_feature(ctx: &Ctx, page: &Page) -> HashMap<String, i64> {
    let seen = notifications_seen(ctx, page.user.id, page.guild_id);
    let global_seen = notifications_seen_anywhere(ctx, page.user.id);
    let rows: Vec<(String, bool)> = ctx
        .db
        .with(|c| {
            let mut s = c.prepare(
                "SELECT n.feature, n.guild_id IS NULL FROM notifications n
                 WHERE (n.guild_id = ?1 AND n.at > ?2) OR (n.guild_id IS NULL AND n.at > ?3) LIMIT 500",
            )?;
            let rows = s
                .query_map(rusqlite::params![page.guild_id as i64, seen, global_seen], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap_or_default();
    let levels = ctx.levels(&page.user, page.guild_id);
    let mut out = HashMap::new();
    for (feature, global) in rows {
        if if global { page.user.admin } else { levels.get(&feature) >= Level::View } {
            *out.entry(if global { String::new() } else { feature }).or_insert(0) += 1;
        }
    }
    out
}

/// When this person last opened any server's notifications. Notices with no server (app
/// connections, Twitch) show on every server's bell, so reading them once clears them all.
pub(super) fn notifications_seen_anywhere(ctx: &Ctx, user: u64) -> i64 {
    ctx.db
        .with(|c| {
            c.query_row(
                "SELECT MAX(COALESCE((SELECT MAX(seen_at) FROM dash_notif_seen WHERE user_id = ?1), 0),
                            COALESCE((SELECT notif_seen FROM dash_users WHERE id = ?1), 0))",
                [user as i64],
                |r| r.get(0),
            )
        })
        .unwrap_or(0)
}

/// When this person last opened the server's notifications, in epoch milliseconds.
pub(super) fn notifications_seen(ctx: &Ctx, user: u64, guild: u64) -> i64 {
    ctx.db
        .with(|c| {
            c.query_row(
                "SELECT COALESCE((SELECT seen_at FROM dash_notif_seen WHERE user_id = ?1 AND guild_id = ?2),
                                 (SELECT notif_seen FROM dash_users WHERE id = ?1), 0)",
                rusqlite::params![user as i64, guild as i64],
                |r| r.get(0),
            )
        })
        .unwrap_or(0)
}

pub fn guild_avatar(ctx: &Ctx, guild: u64) -> Markup {
    let name = ctx.guild_name(guild);
    let initial: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    html! {
        @match ctx.guild_icon_url(guild) {
            Some(url) => img.gicon src=(url) alt="" width="22" height="22" loading="lazy";,
            None => span.gicon.letter { (initial) },
        }
    }
}

// ---- page structure ----

/// A titled block on a page. No box: the title and spacing set it apart.
pub fn section(title: &str, hint: Option<&str>, body: Markup) -> Markup {
    html! {
        section.block {
            div.block-head {
                h2 { (title) }
                @if let Some(h) = hint { p.muted { (h) } }
            }
            (body)
        }
    }
}

/// A group of settings rows on a glass card.
pub fn card(body: Markup) -> Markup {
    html! { div.card { (body) } }
}

/// A settings form. View-only people see the values with every control disabled and no save
/// button. `body` is usually one or more [`card`]s of fields.
pub fn settings_form(page: &Page, action: &str, body: Markup) -> Markup {
    html! {
        form.settings method="post" action=(action) {
            fieldset disabled[!page.can_edit()] {
                (body)
            }
            @if page.can_edit() {
                div.save-bar { button.btn.primary type="submit" { "Save" } }
            } @else {
                p.muted.small { "You can view these settings. Ask an admin for edit access to change them." }
            }
        }
    }
}

fn row(label: &str, help: Option<&str>, for_id: &str, control: Markup) -> Markup {
    html! {
        div.row {
            div.row-text {
                label for=(for_id) { (label) }
                @if let Some(h) = help { p.help { (h) } }
            }
            div.row-control { (control) }
        }
    }
}

/// A row whose control spans the full width under the label (long text, lists).
fn row_wide(label: &str, help: Option<&str>, for_id: &str, control: Markup) -> Markup {
    html! {
        div.row.wide {
            div.row-text {
                label for=(for_id) { (label) }
                @if let Some(h) = help { p.help { (h) } }
            }
            div.row-control { (control) }
        }
    }
}

pub fn field_toggle(name: &str, label: &str, help: Option<&str>, on: bool) -> Markup {
    row(label, help, name, html! {
        label.switch {
            input type="checkbox" id=(name) name=(name) checked[on];
            span.track { span.thumb {} }
        }
    })
}

pub fn field_text(name: &str, label: &str, help: Option<&str>, value: &str, placeholder: &str) -> Markup {
    row(label, help, name, html! {
        input.input type="text" id=(name) name=(name) value=(value) placeholder=(placeholder) autocomplete="off";
    })
}

pub fn field_textarea(name: &str, label: &str, help: Option<&str>, value: &str, placeholder: &str) -> Markup {
    row_wide(label, help, name, html! {
        textarea.input id=(name) name=(name) rows="4" placeholder=(placeholder) { (value) }
    })
}

pub fn field_number(name: &str, label: &str, help: Option<&str>, value: i64, min: i64, max: i64, unit: Option<&str>) -> Markup {
    row(label, help, name, html! {
        div.with-unit {
            input.input.num type="number" id=(name) name=(name) value=(value) min=(min) max=(max) step="1";
            @if let Some(u) = unit { span.unit { (u) } }
        }
    })
}

pub fn field_select(name: &str, label: &str, help: Option<&str>, options: &[(String, String)], selected: &str) -> Markup {
    row(label, help, name, select(name, options, selected))
}

pub fn select(name: &str, options: &[(String, String)], selected: &str) -> Markup {
    html! {
        div.select-wrap {
            select.input id=(name) name=(name) {
                @for (value, text) in options {
                    option value=(value) selected[value == selected] { (text) }
                }
            }
            (icon(Icon::Chevron))
        }
    }
}

/// Pick one text channel, or none.
pub fn field_channel(ctx: &Ctx, guild: u64, name: &str, label: &str, help: Option<&str>, selected: Option<u64>) -> Markup {
    let mut opts = vec![(String::new(), "None".to_owned())];
    opts.extend(ctx.text_channels(guild).into_iter().map(|(id, n)| (id.to_string(), format!("#{n}"))));
    let sel = selected.map(|s| s.to_string()).unwrap_or_default();
    field_select(name, label, help, &opts, &sel)
}

/// Pick one category, or none.
pub fn field_category(ctx: &Ctx, guild: u64, name: &str, label: &str, help: Option<&str>, selected: Option<u64>) -> Markup {
    let mut opts = vec![(String::new(), "None".to_owned())];
    opts.extend(ctx.categories(guild).into_iter().map(|(id, n)| (id.to_string(), n)));
    let sel = selected.map(|s| s.to_string()).unwrap_or_default();
    field_select(name, label, help, &opts, &sel)
}

/// Pick one role, or none.
pub fn field_role(ctx: &Ctx, guild: u64, name: &str, label: &str, help: Option<&str>, selected: Option<u64>) -> Markup {
    let mut opts = vec![(String::new(), "None".to_owned())];
    opts.extend(ctx.roles(guild).into_iter().map(|(id, n, _)| (id.to_string(), format!("@{n}"))));
    let sel = selected.map(|s| s.to_string()).unwrap_or_default();
    field_select(name, label, help, &opts, &sel)
}

/// Pick any number of roles, shown as a list of chips you switch on.
pub fn field_roles(ctx: &Ctx, guild: u64, name: &str, label: &str, help: Option<&str>, selected: &[u64]) -> Markup {
    row_wide(label, help, name, chips(name, &ctx.roles(guild).into_iter().collect::<Vec<_>>(), selected))
}

/// Pick any number of channels.
pub fn field_channels(ctx: &Ctx, guild: u64, name: &str, label: &str, help: Option<&str>, selected: &[u64]) -> Markup {
    row_wide(label, help, name, chips(name, &ctx.text_channels(guild).into_iter().map(|(id, n)| (id, format!("#{n}"), 0)).collect::<Vec<_>>(), selected))
}

fn chips(name: &str, items: &[(u64, String, u32)], selected: &[u64]) -> Markup {
    html! {
        div.chips id=(name) {
            @if items.is_empty() { span.muted.small { "Nothing to pick from yet." } }
            @for (id, label, color) in items {
                label.chip {
                    input type="checkbox" name=(name) value=(id) checked[selected.contains(id)];
                    @if *color != 0 { span.dot style={ "background:#" (format!("{color:06x}")) } {} }
                    span { (label) }
                }
            }
        }
    }
}

/// A custom control in a standard row.
pub fn field_custom(label: &str, help: Option<&str>, control: Markup) -> Markup {
    row(label, help, "", control)
}

pub fn field_custom_wide(label: &str, help: Option<&str>, control: Markup) -> Markup {
    row_wide(label, help, "", control)
}

// ---- data display ----

/// A row of readings: `(label, value)`. Use for the few numbers that answer the page.
pub fn readings(items: &[(&str, String)]) -> Markup {
    html! {
        div.readings {
            @for (label, value) in items {
                div.reading { b.mono { (value) } span { (label) } }
            }
        }
    }
}

/// Up to three small figures side by side in a rail block: `(label, value)`, like
/// ("companions", "61"). The page's own answer goes in [`readings`]; these are the rail's.
pub fn rail_numbers(items: &[(&str, String)]) -> Markup {
    html! {
        div.rail-nums {
            @for (label, value) in items {
                div { b.mono { (value) } span { (label) } }
            }
        }
    }
}

/// What an empty list looks like: the section's own icon and one line on how it fills.
pub fn empty(ic: Icon, title: &str, text: &str) -> Markup {
    html! {
        div.empty {
            span.empty-icon { (icon(ic)) }
            div { strong { (title) } p.muted { (text) } }
        }
    }
}

#[derive(Clone, Copy)]
pub enum Tone {
    Ok,
    Warn,
    Bad,
    Accent,
    Muted,
}

pub fn badge(text: &str, tone: Tone) -> Markup {
    let class = match tone {
        Tone::Ok => "badge ok",
        Tone::Warn => "badge warn",
        Tone::Bad => "badge bad",
        Tone::Accent => "badge accent",
        Tone::Muted => "badge",
    };
    html! { span class=(class) { (text) } }
}

/// A one-button form, for actions on a row (approve, end, delete). Destructive ones ask for a
/// second click instead of a dialog.
pub fn action_button(action: &str, label: &str, tone: Tone, destructive: bool) -> Markup {
    let class = match tone {
        Tone::Bad => "btn small danger",
        Tone::Accent => "btn small primary",
        _ => "btn small",
    };
    html! {
        form.inline method="post" action=(action) {
            button class=(class) type="submit" data-confirm=[destructive.then_some("Click again to confirm")] { (label) }
        }
    }
}

/// Sub-page switcher inside a feature: `(label, href)`.
pub fn tabs(items: &[(&str, String)], active: &str) -> Markup {
    let items: Vec<_> = items.iter().map(|(l, h)| (*l, h.clone(), None)).collect();
    tabs_counted(&items, active)
}

/// [`tabs`] with a count after a label where something waits: `(label, href, count)`, e.g.
/// ("Running", href, Some(3)). `active` is the open tab's label, without the count.
pub fn tabs_counted(items: &[(&str, String, Option<i64>)], active: &str) -> Markup {
    html! {
        nav.tabs {
            @for (label, href, count) in items {
                a.tab.on[*label == active] href=(href) aria-current=[(*label == active).then_some("page")] {
                    (label)
                    @if let Some(n) = count { span.tab-count { (crate::text::num(*n)) } }
                    @if *label == active { span.tab-line {} }
                }
            }
        }
    }
}

/// How a [`status`] pill's dot reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dot {
    /// Green: connected, running, live.
    Live,
    /// Amber: partly on, connecting, waiting.
    Waiting,
    /// Grey: off.
    Off,
}

/// A state pill for a page header or a rail heading: "Twitch connected · instant alerts".
pub fn status(dot: Dot, label: &str) -> Markup {
    let class = match dot {
        Dot::Live => "dot live",
        Dot::Waiting => "dot waiting",
        Dot::Off => "dot",
    };
    html! { span.status { span class=(class) {} (label) } }
}

/// A button that opens a small form under it, for a header or a row action. `button` is the
/// button's classes after `btn` ("primary", "small outline"); `class` goes on the popover for
/// a feature's own styling ("" for none). With an icon the label hides on a phone and stays
/// the button's name. app.js closes it on a click outside or Escape.
pub fn pop(class: &str, label: &str, ic: Option<Icon>, button: &str, body: Markup) -> Markup {
    html! {
        details class={ "pop " (class) } {
            summary class={ "btn " (button) } aria-label=[ic.is_some().then_some(label)] {
                @if let Some(i) = ic { (icon(i)) span.pop-label { (label) } } @else { (label) }
            }
            div.pop-body { (body) }
        }
    }
}

/// A switch with its label and help, for a rail or a short list of options outside a card.
/// Submits with the form around it, like [`field_toggle`].
pub fn switch_row(name: &str, label: &str, help: Option<&str>, on: bool) -> Markup {
    html! {
        div.switch-row {
            div.grow {
                label for=(name) { (label) }
                @if let Some(h) = help { p.help { (h) } }
            }
            label.switch {
                input type="checkbox" id=(name) name=(name) checked[on];
                span.track { span.thumb {} }
            }
        }
    }
}

/// Small bars for a short series in a rail, `(label, value)`, oldest first: the latest in lilac,
/// the rest dimmer. Hovering a bar shows its label and value with `unit`. A `None` value leaves
/// its bar blank and says nothing was read, so a gap doesn't pass for a zero. `about` names the
/// chart for screen readers ("Hours streamed in each of the last 12 weeks").
pub fn mini_bars(series: &[(String, Option<i64>)], unit: &str, about: &str) -> Markup {
    html! { div.mini-bars { (bars_svg(series, unit, about)) } }
}

/// [`mini_bars`] sized for a page's main column, with the first, middle and last labels under it.
pub fn column_bars(series: &[(String, Option<i64>)], unit: &str, about: &str) -> Markup {
    let n = series.len();
    html! {
        figure.chart {
            (bars_svg(series, unit, about))
            @if n > 1 {
                div.axis { span { (series[0].0) } span { (series[n / 2].0) } span { (series[n - 1].0) } }
            }
        }
    }
}

fn bars_svg(series: &[(String, Option<i64>)], unit: &str, about: &str) -> Markup {
    let max = series.iter().filter_map(|(_, v)| *v).max().unwrap_or(0).max(1) as f64;
    let n = series.len();
    html! {
        svg viewBox={ "0 0 " ((n * 10).saturating_sub(4)) " 48" } preserveAspectRatio="none" role="img" aria-label=(about) {
            defs {
                linearGradient id="bars-hi" x1="0" y1="0" x2="0" y2="1" {
                    stop offset="0" stop-color="var(--data)" {}
                    stop offset="1" stop-color="var(--data-2)" {}
                }
            }
            @for (i, (label, v)) in series.iter().enumerate() {
                @let v = *v;
                @let h = v.map_or(0.0, |v| (v as f64 / max * 46.0).max(if v > 0 { 2.0 } else { 0.0 }));
                @let tip = match v { Some(v) => format!("{label}: {} {unit}", crate::text::num(v)), None => format!("{label}: nothing read") };
                rect.bar x=(i * 10) y=(48.0 - h) width="6" height=(h) rx="1.2" style={ "--i:" (i) } fill=(if i + 1 == n { "url(#bars-hi)" } else { "var(--data-dim)" }) data-tip=(tip) {}
            }
        }
    }
}

/// Vertical bars for a daily series, `(label, value)`, oldest first. The latest bar is drawn
/// brighter. `color` is a CSS colour for the latest bar; the rest use a dimmer version.
pub fn bar_chart(series: &[(String, i64)], color: &str, unit: &str) -> Markup {
    let max = series.iter().map(|(_, v)| *v).max().unwrap_or(0).max(1);
    let n = series.len().max(1);
    let (w, h) = (600.0, 150.0);
    let gap = 3.0;
    let bw = (w - gap * (n as f64 - 1.0)) / n as f64;
    let total: i64 = series.iter().map(|(_, v)| v).sum();
    let dim = if color == "var(--data)" { "var(--data-dim)".to_owned() } else { format!("color-mix(in oklab, {color} 45%, transparent)") };
    html! {
        figure.chart {
            svg viewBox={ "0 0 " (w) " " (h) } preserveAspectRatio="none" role="img" aria-label={ (crate::text::num(total)) " " (unit) " in this period" } {
                line x1="0" y1=(h - 0.5) x2=(w) y2=(h - 0.5) stroke="var(--edge-2)" stroke-width="1" vector-effect="non-scaling-stroke" {}
                @for (i, (label, v)) in series.iter().enumerate() {
                    @let bh = (*v as f64 / max as f64) * (h - 4.0);
                    @let x = i as f64 * (bw + gap);
                    rect.bar x=(x) y=(h - bh) width=(bw.max(1.0)) height=(bh.max(if *v > 0 { 1.5 } else { 0.0 })) rx="2" style={ "--i:" (i) } fill=(if i + 1 == n { color } else { dim.as_str() }) data-tip={ (label) ": " (crate::text::num(*v)) " " (unit) } {}
                }
            }
            @if let (Some(first), Some(last)) = (series.first(), series.last()) {
                div.axis { span { (first.0) } span { (last.0) } }
            }
            figcaption.muted.small { "Peak " (crate::text::num(max)) " · total " (crate::text::num(total)) " " (unit) }
        }
    }
}

/// A smooth area chart for a daily series, `(label, value)`, oldest first, in the data colour.
/// The line draws itself in, and hovering reads any day's value off a cursor. Labels the first
/// and middle day under it, and the last one "Today", so the series must end today (the
/// overview's do).
pub fn area_chart(series: &[(String, i64)], unit: &str) -> Markup {
    let n = series.len();
    let max = series.iter().map(|(_, v)| *v).max().unwrap_or(0).max(1) as f64;
    let (w, h) = (1000.0, 190.0);
    let step = if n > 1 { w / (n - 1) as f64 } else { w };
    let pts: Vec<(f64, f64)> = series.iter().enumerate().map(|(i, (_, v))| (i as f64 * step, h - 3.0 - (*v as f64 / max) * (h - 22.0))).collect();
    // Catmull-Rom through the points, as cubic Béziers.
    let mut line = String::new();
    for (i, p) in pts.iter().enumerate() {
        if i == 0 {
            line.push_str(&format!("M{:.1} {:.1}", p.0, p.1));
            continue;
        }
        let p0 = if i >= 2 { pts[i - 2] } else { pts[i - 1] };
        let p1 = pts[i - 1];
        let p3 = pts.get(i + 1).copied().unwrap_or(*p);
        let c1 = (p1.0 + (p.0 - p0.0) / 6.0, p1.1 + (p.1 - p0.1) / 6.0);
        let c2 = (p.0 - (p3.0 - p1.0) / 6.0, p.1 - (p3.1 - p1.1) / 6.0);
        line.push_str(&format!(" C{:.1} {:.1} {:.1} {:.1} {:.1} {:.1}", c1.0, c1.1.min(h), c2.0, c2.1.min(h), p.0, p.1));
    }
    let area = format!("{line} L{w} {h} L0 {h} Z");
    let total: i64 = series.iter().map(|(_, v)| v).sum();
    // Each day as [x %, y %, label, value] for the hover cursor.
    let points = serde_json::to_string(
        &series
            .iter()
            .zip(&pts)
            .map(|((label, v), (x, y))| serde_json::json!([(x / w * 1000.0).round() / 10.0, (y / h * 1000.0).round() / 10.0, label, crate::text::num(*v)]))
            .collect::<Vec<_>>(),
    )
    .unwrap_or_default();
    let last = pts.last().copied().unwrap_or((w, h));
    html! {
        figure.area data-points=(points) data-unit=(unit) {
            div.area-plot {
                svg viewBox={ "0 0 " (w) " " (h) } preserveAspectRatio="none" role="img" aria-label={ (crate::text::num(total)) " " (unit) " in " (n) " days" } {
                    defs {
                        linearGradient id="area-fill" x1="0" y1="0" x2="0" y2="1" {
                            stop offset="0" stop-color="var(--data)" stop-opacity="0.42" {}
                            stop offset="0.6" stop-color="var(--data-2)" stop-opacity="0.1" {}
                            stop offset="1" stop-color="var(--data-2)" stop-opacity="0" {}
                        }
                        linearGradient id="area-line" x1="0" y1="0" x2="1" y2="0" {
                            stop offset="0" stop-color="var(--data-2)" {}
                            stop offset="0.7" stop-color="var(--data)" {}
                            stop offset="1" stop-color="#e9ddff" {}
                        }
                    }
                    @for f in [0.25, 0.5, 0.75] {
                        line.grid x1="0" x2=(w) y1=(h * f) y2=(h * f) vector-effect="non-scaling-stroke" {}
                    }
                    path.fill d=(area) fill="url(#area-fill)" {}
                    path.line d=(line) fill="none" stroke="url(#area-line)" stroke-width="2.5" vector-effect="non-scaling-stroke" stroke-linejoin="round" stroke-linecap="round" {}
                }
                span.area-cursor {}
                span.area-dot {}
                span.area-end style={ "left:" (format!("{:.2}", last.0 / w * 100.0)) "%;top:" (format!("{:.2}", last.1 / h * 100.0)) "%" } {}
            }
            @if n > 1 {
                div.axis { span { (series[0].0) } span { (series[n / 2].0) } span { "Today" } }
            }
        }
    }
}

/// Up to `max` overlapping avatars.
pub fn avatars(urls: &[String], max: usize) -> Markup {
    html! {
        span.avatars {
            @for u in urls.iter().take(max) { img.avatar src=(u) alt="" width="26" height="26" loading="lazy"; }
        }
    }
}

/// A member line: avatar, name and a muted detail.
pub fn person(name: &str, avatar_url: &str, detail: Option<&str>) -> Markup {
    html! {
        span.person {
            img.avatar src=(avatar_url) alt="" width="22" height="22" loading="lazy";
            span.clip { (name) }
            @if let Some(d) = detail { span.muted.small { (d) } }
        }
    }
}

/// A Discord-style embed preview for the embed builder and message settings.
pub fn embed_preview(color: &str, title: &str, description: &str) -> Markup {
    html! {
        div.embed-preview style={ "--embed:" (color) } {
            @if !title.is_empty() { strong { (title) } }
            @if !description.is_empty() { p { (description) } }
        }
    }
}
