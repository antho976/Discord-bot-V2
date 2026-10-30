//! The dashboard's building blocks. Features compose their pages from these so every page has
//! the same shell, spacing and controls. Classes are defined in `assets/app.css`.

use maud::{DOCTYPE, Markup, PreEscaped, html};

use super::auth::{Level, Page};
use crate::{Ctx, Group};

/// Icons drawn on one 16-unit grid with a 1.5 stroke.
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
    fn paths(self) -> &'static str {
        match self {
            Icon::Home => r#"<path d="M2.5 7 8 2.5 13.5 7v6a.5.5 0 0 1-.5.5H10V10H6v3.5H3a.5.5 0 0 1-.5-.5z"/>"#,
            Icon::Live => r#"<circle cx="8" cy="8" r="1.5"/><path d="M5.2 5.2a4 4 0 0 0 0 5.6M10.8 5.2a4 4 0 0 1 0 5.6M3.1 3.1a7 7 0 0 0 0 9.8M12.9 3.1a7 7 0 0 1 0 9.8"/>"#,
            Icon::Calendar => r#"<rect x="2.5" y="3.5" width="11" height="10" rx="1.5"/><path d="M2.5 6.5h11M5.5 2v3M10.5 2v3"/>"#,
            Icon::Play => r#"<rect x="1.5" y="3.5" width="13" height="9" rx="2.5"/><path d="m6.75 6 3.25 2-3.25 2z"/>"#,
            Icon::Trophy => r#"<path d="M5 2.5h6v4a3 3 0 0 1-6 0zM5 4H2.5v1a2.5 2.5 0 0 0 2.6 2.5M11 4h2.5v1a2.5 2.5 0 0 1-2.6 2.5M8 9.5v2.5M5.5 13.5h5"/>"#,
            Icon::Log => r#"<path d="M4 2.5h6l2.5 2.5v8.5H4zM6.5 7h4M6.5 9.5h4M6.5 12h2"/>"#,
            Icon::Wave => r#"<path d="M5 8.5V4a1 1 0 0 1 2 0v3.5V3a1 1 0 0 1 2 0v4.5V4a1 1 0 0 1 2 0v5a4.5 4.5 0 0 1-8.2 2.5L1.9 9.8a1 1 0 0 1 1.6-1.1z"/>"#,
            Icon::Door => r#"<path d="M3.5 13.5V3a.5.5 0 0 1 .5-.5h6v11M10 4.5h2.5v9M2 13.5h12M7.5 8.25v.5"/>"#,
            Icon::Gift => r#"<rect x="2" y="5.5" width="12" height="3" rx=".5"/><path d="M3 8.5v5h10v-5M8 5.5v8M8 5.5C7 3 4.5 2.5 4.5 4s2 1.5 3.5 1.5ZM8 5.5C9 3 11.5 2.5 11.5 4S9.5 5.5 8 5.5Z"/>"#,
            Icon::Poll => r#"<path d="M3 13.5V8M8 13.5V3M13 13.5V6"/>"#,
            Icon::Bulb => r#"<path d="M6 11.5h4M6.5 13.5h3M5.75 9.5a4 4 0 1 1 4.5 0V11h-4.5z"/>"#,
            Icon::Ticket => r#"<path d="M2.5 4.5h11v2a1.5 1.5 0 0 0 0 3v2h-11v-2a1.5 1.5 0 0 0 0-3zM9.5 4.5v7" stroke-dasharray="1.5 1.5"/>"#,
            Icon::Star => r#"<path d="m8 2 1.8 3.8 4.2.5-3.1 2.9.8 4.1L8 11.3l-3.7 2 .8-4.1L2 6.3l4.2-.5z"/>"#,
            Icon::Clock => r#"<circle cx="8" cy="8" r="5.75"/><path d="M8 4.75V8l2.25 1.5"/>"#,
            Icon::Moon => r#"<path d="M13 9.5A5.5 5.5 0 0 1 6.5 3a5.5 5.5 0 1 0 6.5 6.5Z"/>"#,
            Icon::Paw => r#"<ellipse cx="8" cy="11" rx="2.75" ry="2.25"/><circle cx="4" cy="7.5" r="1.25"/><circle cx="12" cy="7.5" r="1.25"/><circle cx="6.25" cy="4.25" r="1.25"/><circle cx="9.75" cy="4.25" r="1.25"/>"#,
            Icon::Shield => r#"<path d="M8 2 13 4v4c0 3-2.2 5-5 6-2.8-1-5-3-5-6V4z"/><path d="m6 8 1.5 1.5L10.25 6.75"/>"#,
            Icon::Plug => r#"<path d="M6 2v3M10 2v3M4.5 5h7v2.5a3.5 3.5 0 0 1-7 0zM8 11v3"/>"#,
            Icon::Brush => r#"<rect x="2.5" y="2.5" width="11" height="11" rx="1.5"/><path d="M2.5 6h11M6 6v7.5"/>"#,
            Icon::Chart => r#"<path d="M2.5 2.5v11h11"/><path d="m5 10 2.5-3 2 2L13 5"/>"#,
            Icon::Users => r#"<circle cx="6" cy="5.5" r="2.25"/><path d="M2 13c.4-2.2 2-3.5 4-3.5s3.6 1.3 4 3.5"/><path d="M10.5 3.5a2 2 0 0 1 0 4M12 9.75c1 .5 1.8 1.6 2 3.25"/>"#,
            Icon::Toggle => r#"<rect x="1.5" y="4.5" width="13" height="7" rx="3.5"/><circle cx="11" cy="8" r="1.75"/>"#,
            Icon::Activity => r#"<path d="M1.5 8h3l2-4.5 3 9 2-4.5h3"/>"#,
            Icon::Bell => r#"<path d="M4 11V7a4 4 0 0 1 8 0v4l1 1.5H3zM6.5 14h3"/>"#,
            Icon::Plus => r#"<path d="M8 3v10M3 8h10"/>"#,
            Icon::Trash => r#"<path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.5 9h6l.5-9"/>"#,
            Icon::Check => r#"<path d="m3 8.5 3 3 7-7"/>"#,
            Icon::Close => r#"<path d="m4 4 8 8M12 4l-8 8"/>"#,
            Icon::Chevron => r#"<path d="m4.5 6.5 3.5 3.5 3.5-3.5"/>"#,
            Icon::External => r#"<path d="M9 2.5h4.5V7M13.5 2.5 7.5 8.5M11.5 9.5v3a1 1 0 0 1-1 1h-7a1 1 0 0 1-1-1v-7a1 1 0 0 1 1-1h3"/>"#,
            Icon::Menu => r#"<path d="M2.5 4.5h11M2.5 8h11M2.5 11.5h11"/>"#,
            Icon::Logout => r#"<path d="M6.5 13.5h-3a1 1 0 0 1-1-1v-9a1 1 0 0 1 1-1h3M10.5 11 13.5 8l-3-3M13.5 8H6"/>"#,
            Icon::Hash => r#"<path d="M6 2.5 5 13.5M11 2.5l-1 11M2.75 6h11M2.25 10h11"/>"#,
        }
    }
}

pub fn icon(i: Icon) -> Markup {
    html! {
        svg.icon viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" {
            (PreEscaped(i.paths()))
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
/// then the page itself. `actions` sits at the right of the title.
pub fn layout(ctx: &Ctx, page: &Page, title: &str, actions: Option<Markup>, body: Markup) -> Markup {
    let guild = page.guild_id;
    let guilds = ctx.guilds_for(&page.user);
    let unread = unread_count(ctx, page);
    type Link = (&'static str, &'static str, Icon, &'static str);
    let mut groups: Vec<(Group, Vec<Link>)> = Vec::new();
    for f in ctx.features() {
        let meta = f.meta();
        if !ctx.enabled(guild, meta.key) || ctx.level(&page.user, guild, meta.key) == Level::None {
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
                            a.nav.on[current == "overview"] href={ "/g/" (guild) } { (icon(Icon::Home)) span { "Overview" } }
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
                                a.nav.on[current == "features"] href={ "/g/" (guild) "/features" } { (icon(Icon::Toggle)) span { "Features" } }
                                a.nav.on[current == "users"] href="/users" { (icon(Icon::Users)) span { "People" } }
                                a.nav.on[current == "activity"] href="/activity" { (icon(Icon::Activity)) span { "Activity" } }
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
                        header.page-head {
                            h1 { (title) }
                            div.head-actions {
                                @if let Some(a) = actions { (a) }
                                a.icon-btn.bell href={ "/g/" (guild) "/notifications" } title="Notifications" aria-label={ "Notifications, " (unread) " new" } {
                                    (icon(Icon::Bell))
                                    @if unread > 0 { span.count { (if unread > 99 { "99+".to_owned() } else { unread.to_string() }) } }
                                }
                            }
                        }
                        div.page-body { (body) }
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
    rows.iter()
        .filter(|(feature, global)| if *global { page.user.admin } else { ctx.level(&page.user, page.guild_id, feature) >= Level::View })
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
                a.tab.on[*label == active] href=(href) { (label) }
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
