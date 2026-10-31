//! The dashboard's building blocks. Features compose their pages from these so every page has
//! the same shell, spacing and controls. Classes are defined in `assets/app.css`.

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
            Icon::Menu => (16, r#"<path d="M2.5 4.5h11M2.5 8h11M2.5 11.5h11"/>"#),
            Icon::Logout => (24, r#"<path d="M10 4.5H6.5A1.5 1.5 0 0 0 5 6v12a1.5 1.5 0 0 0 1.5 1.5H10M14.5 8l4 4-4 4M18.5 12h-9"/>"#),
            Icon::Hash => (16, r#"<path d="M6 2.5 5 13.5M11 2.5l-1 11M2.75 6h11M2.25 10h11"/>"#),
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

/// `<head>` contents shared by every page.
pub fn head(title: &str) -> Markup {
    html! {
        meta charset="utf-8";
        meta name="viewport" content="width=device-width, initial-scale=1";
        meta name="robots" content="noindex";
        meta name="color-scheme" content="dark";
        title { (title) " · Bot" }
        link rel="icon" href="/static/icon.svg";
        link rel="preconnect" href="https://fonts.googleapis.com";
        link rel="preconnect" href="https://fonts.gstatic.com" crossorigin;
        link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Geist:wght@400;500;600&family=Geist+Mono:wght@400;500&display=swap";
        link rel="stylesheet" href="/static/app.css";
        script src="/static/app.js" defer {}
    }
}

/// The signed-in shell: sidebar with the server switcher and the pages this person can open,
/// then the page with its header (the page's icon, title, one line on what it does, the bell).
/// `actions` sits at the right of the title.
pub fn layout(ctx: &Ctx, page: &Page, title: &str, actions: Option<Markup>, body: Markup) -> Markup {
    let (ic, line) = identity(ctx, page, title);
    let head = html! {
        header.page-head {
            span.head-icon { (icon(ic)) }
            div.head-text {
                h1 { (title) }
                @if let Some(l) = line { p { (l) } }
            }
            div.head-actions {
                @if let Some(a) = actions { (a) }
                (bell(ctx, page))
            }
        }
    };
    shell(ctx, page, title, head, html! { div.page-body { (body) } })
}

/// The shell with a page that draws its own header (the overview's banner). Put [`bell`] in it.
pub fn layout_custom(ctx: &Ctx, page: &Page, title: &str, body: Markup) -> Markup {
    shell(ctx, page, title, html! {}, body)
}

/// The notification bell with its unread count.
pub fn bell(ctx: &Ctx, page: &Page) -> Markup {
    let unread = unread_count(ctx, page);
    html! {
        a.icon-btn.bell href={ "/g/" (page.guild_id) "/notifications" } title="Notifications" aria-label={ "Notifications, " (unread) " new" } {
            (icon(Icon::Bell))
            @if unread > 0 { span.count { (if unread > 99 { "99+".to_owned() } else { unread.to_string() }) } }
        }
    }
}

/// The icon and the one line under the title: the feature's own, or the core page's.
fn identity(ctx: &Ctx, page: &Page, title: &str) -> (Icon, Option<&'static str>) {
    match page.key {
        "overview" => (Icon::Home, None),
        "features" => (Icon::Toggle, Some("A feature that's off ignores the server, and its commands disappear there.")),
        "users" => (Icon::Users, Some("Only people you add can open the dashboard, and they see only the pages you give them.")),
        "activity" => (Icon::Activity, Some("Every change made on the dashboard, with who made it.")),
        "notifications" => (Icon::Bell, Some("What features flagged for you, newest first.")),
        key => match ctx.feature(key) {
            Some(f) => {
                let meta = f.meta();
                let nav = meta.nav.iter().find(|n| n.label == title).or(meta.nav.first());
                let ic = nav.map(|n| n.icon).unwrap_or(Icon::Toggle);
                (ic, Some(nav.map(|n| n.about).filter(|a| !a.is_empty()).unwrap_or(meta.description)))
            }
            None => (Icon::Home, None),
        },
    }
}

fn shell(ctx: &Ctx, page: &Page, title: &str, top: Markup, body: Markup) -> Markup {
    let guild = page.guild_id;
    let guilds = ctx.guilds_for(&page.user);
    let levels = ctx.levels(&page.user, guild);
    type Link = (&'static str, &'static str, Icon, &'static str);
    let mut groups: Vec<(Group, Vec<Link>)> = Vec::new();
    for f in ctx.features() {
        let meta = f.meta();
        if !ctx.enabled(guild, meta.key) || levels.get(meta.key) == Level::None {
            continue;
        }
        let items: Vec<_> = meta.nav.iter().map(|n| (n.label, n.path, n.icon, meta.key)).collect();
        if items.is_empty() {
            continue;
        }
        match groups.iter_mut().find(|(g, _)| *g == meta.group) {
            Some((_, list)) => list.extend(items),
            None => groups.push((meta.group, items)),
        }
    }
    groups.sort_by_key(|(g, _)| *g);
    let current = page.key;

    html! {
        (DOCTYPE)
        html lang="en" {
            head { (head(title)) }
            body {
                div.shell {
                    nav.side aria-label="Dashboard" {
                        div.side-top {
                            details.switcher {
                                summary {
                                    (guild_avatar(ctx, guild))
                                    span.grow { (ctx.guild_name(guild)) }
                                    @if guilds.len() > 1 { (icon(Icon::Chevron)) }
                                }
                                @if guilds.len() > 1 {
                                    div.menu {
                                        @for g in &guilds {
                                            a.menu-item.on[*g == guild] href={ "/g/" (g) } {
                                                (guild_avatar(ctx, *g)) span { (ctx.guild_name(*g)) }
                                            }
                                        }
                                    }
                                }
                            }
                            button.icon-btn.menu-btn type="button" aria-label="Menu" aria-expanded="false" { (icon(Icon::Menu)) }
                        }
                        div.side-links {
                            a.nav.on[current == "overview"] aria-current=[(current == "overview").then_some("page")] href={ "/g/" (guild) } { (icon(Icon::Home)) span { "Overview" } }
                            @for (group, items) in &groups {
                                div.nav-group { (group.label()) }
                                @for (label, path, ic, _key) in items {
                                    a.nav href={ "/g/" (guild) "/" (path) } data-path={ "/g/" (guild) "/" (path) } {
                                        (icon(*ic)) span { (label) }
                                    }
                                }
                            }
                            @if page.user.admin {
                                div.nav-group { "Admin" }
                                a.nav.on[current == "features"] aria-current=[(current == "features").then_some("page")] href={ "/g/" (guild) "/features" } { (icon(Icon::Toggle)) span { "Features" } }
                                a.nav.on[current == "users"] aria-current=[(current == "users").then_some("page")] href="/users" { (icon(Icon::Users)) span { "People" } }
                                a.nav.on[current == "activity"] aria-current=[(current == "activity").then_some("page")] href="/activity" { (icon(Icon::Activity)) span { "Activity" } }
                            }
                        }
                        div.side-foot {
                            img.avatar src=(page.user.avatar_url()) alt="" width="24" height="24" loading="lazy";
                            span.grow.clip { (page.user.name) }
                            form method="post" action="/logout" {
                                button.icon-btn type="submit" title="Sign out" aria-label="Sign out" { (icon(Icon::Logout)) }
                            }
                        }
                    }
                    main.page {
                        (top)
                        (body)
                    }
                }
            }
        }
    }
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

fn unread_count(ctx: &Ctx, page: &Page) -> i64 {
    let rows: Vec<(String, bool)> = ctx
        .db
        .with(|c| {
            let mut s = c.prepare(
                "SELECT n.feature, n.guild_id IS NULL FROM notifications n, dash_users u
                 WHERE u.id = ?1 AND n.at > u.notif_seen AND (n.guild_id IS NULL OR n.guild_id = ?2) LIMIT 500",
            )?;
            let rows = s
                .query_map(rusqlite::params![page.user.id as i64, page.guild_id as i64], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap_or_default();
    let levels = ctx.levels(&page.user, page.guild_id);
    rows.iter()
        .filter(|(feature, global)| if *global { page.user.admin } else { levels.get(feature) >= Level::View })
        .count() as i64
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
    html! {
        nav.tabs {
            @for (label, href) in items {
                a.tab.on[*label == active] href=(href) aria-current=[(*label == active).then_some("page")] { (label) }
            }
        }
    }
}

/// Vertical bars for a daily series, `(label, value)`, oldest first. The latest bar is drawn
/// brighter. `color` is a CSS colour for the latest bar; the rest use a dimmer version.
pub fn bar_chart(series: &[(String, i64)], color: &str, unit: &str) -> Markup {
    let max = series.iter().map(|(_, v)| *v).max().unwrap_or(0).max(1);
    let n = series.len().max(1);
    let (w, h) = (600.0, 140.0);
    let gap = 3.0;
    let bw = (w - gap * (n as f64 - 1.0)) / n as f64;
    let total: i64 = series.iter().map(|(_, v)| v).sum();
    let dim = if color == "var(--data)" { "var(--data-dim)".to_owned() } else { color.to_owned() };
    html! {
        figure.chart {
            svg viewBox={ "0 0 " (w) " " (h) } preserveAspectRatio="none" role="img" aria-label={ (crate::text::num(total)) " " (unit) " in this period" } {
                line x1="0" y1=(h - 0.5) x2=(w) y2=(h - 0.5) stroke="var(--edge-2)" stroke-width="1" vector-effect="non-scaling-stroke" {}
                @for (i, (label, v)) in series.iter().enumerate() {
                    @let bh = (*v as f64 / max as f64) * (h - 4.0);
                    @let x = i as f64 * (bw + gap);
                    rect x=(x) y=(h - bh) width=(bw.max(1.0)) height=(bh.max(if *v > 0 { 1.5 } else { 0.0 })) fill=(if i + 1 == n { color } else { dim.as_str() }) {
                        title { (label) ": " (crate::text::num(*v)) " " (unit) }
                    }
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
/// Hovering a day shows its value. Labels the first, middle and last day under it.
pub fn area_chart(series: &[(String, i64)], unit: &str) -> Markup {
    let n = series.len();
    let max = series.iter().map(|(_, v)| *v).max().unwrap_or(0).max(1) as f64;
    let (w, h) = (1000.0, 130.0);
    let step = if n > 1 { w / (n - 1) as f64 } else { w };
    let pts: Vec<(f64, f64)> = series.iter().enumerate().map(|(i, (_, v))| (i as f64 * step, h - 2.0 - (*v as f64 / max) * (h - 14.0))).collect();
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
    html! {
        figure.area {
            svg viewBox={ "0 0 " (w) " " (h) } preserveAspectRatio="none" role="img" aria-label={ (crate::text::num(total)) " " (unit) " in " (n) " days" } {
                defs {
                    linearGradient id="area-fill" x1="0" y1="0" x2="0" y2="1" {
                        stop offset="0" stop-color="var(--data)" stop-opacity="0.32" {}
                        stop offset="1" stop-color="var(--data)" stop-opacity="0.04" {}
                    }
                }
                path d=(area) fill="url(#area-fill)" {}
                path d=(line) fill="none" stroke="var(--data)" stroke-width="2" vector-effect="non-scaling-stroke" stroke-linejoin="round" {}
                @for (i, (label, v)) in series.iter().enumerate() {
                    rect.hit x=((i as f64 * step - step / 2.0).max(0.0)) y="0" width=(step) height=(h) {
                        title { (label) ": " (crate::text::num(*v)) " " (unit) }
                    }
                }
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
