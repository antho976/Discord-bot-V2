//! Discord, drawn on the dashboard: a channel, a message from the bot, an embed, buttons. Every
//! feature that previews what the bot posts uses these, so the previews look alike and a change
//! to how Discord looks is one edit. The styles are the `dp-` rules in app.css.
//!
//! A preview is `channel(.., message(..).render(body))`, where `body` is the message's text
//! (`line`), then any `embed(..)` and `buttons(..)`. Inside an embed, use the classes
//! `dp-provider`, `dp-author`, `dp-title`, `dp-desc`, `dp-media`, `dp-thumb` and `dp-foot`, and
//! `fields(..)` for its fields.

use maud::{Markup, html};

use super::auth::Page;
use super::ui::{self, Icon};
use crate::{Ctx, now_ms};

/// The bot's name and avatar address, as Discord shows them. `None` for the avatar before the
/// bot has connected (the preview), when the bot's mark is drawn instead.
pub fn bot_identity(ctx: &Ctx) -> (String, Option<String>) {
    match ctx.cache.current_user() {
        Some(u) => (u.name.clone(), Some(crate::discord::avatar_url(u.id.get(), u.avatar.map(|a| a.to_string()).as_deref()))),
        None => ("Bot".into(), None),
    }
}

/// A channel as Discord draws it: its name in the bar (or `none` when there isn't one), then
/// the messages.
pub fn channel(name: Option<&str>, none: &str, body: Markup) -> Markup {
    html! {
        div.dp {
            div.dp-bar {
                (ui::icon(Icon::Hash))
                @match name {
                    Some(c) => span.clip { (c.trim_start_matches('#')) },
                    None => span.none { (none) },
                }
                span.dp-tag { "Preview" }
            }
            div.dp-body { (body) }
        }
    }
}

/// "Today at 4:05 PM", or "Oct 30 at 4:05 PM" for another day, in the viewer's time zone.
pub fn time(page: &Page, at: i64) -> String {
    let when = page.user.local(at);
    let clock = when.strftime("%-I:%M %p");
    if when.date() == page.user.local(now_ms()).date() { format!("Today at {clock}") } else { format!("{} at {clock}", when.strftime("%b %-d")) }
}

/// A message, as Discord draws it: avatar, name with the APP tag and the time, then the body.
pub struct Message<'a> {
    name: String,
    avatar: Option<String>,
    app: bool,
    time: String,
    used: Option<(&'a Page, &'a str)>,
    id: Option<&'a str>,
    off: bool,
}

/// A message from the bot, sent now.
pub fn message<'a>(ctx: &Ctx, page: &'a Page) -> Message<'a> {
    let (name, avatar) = bot_identity(ctx);
    Message { name, avatar, app: true, time: time(page, now_ms()), used: None, id: None, off: false }
}

impl<'a> Message<'a> {
    /// Sent at this time instead of now.
    pub fn at(mut self, page: &Page, at: i64) -> Self {
        self.time = time(page, at);
        self
    }

    /// The time exactly as given ("Today at 4:05 PM").
    pub fn time_text(mut self, t: impl Into<String>) -> Self {
        self.time = t.into();
        self
    }

    /// From someone other than the bot: no APP tag.
    pub fn from(mut self, name: impl Into<String>, avatar: Option<String>) -> Self {
        self.name = name.into();
        self.avatar = avatar;
        self.app = false;
        self
    }

    /// The answer to a slash command the viewer used, shown above it the way Discord does.
    pub fn used(mut self, page: &'a Page, command: &'a str) -> Self {
        self.used = Some((page, command));
        self
    }

    /// An id on the message, for a page script that rewrites it in place.
    pub fn id(mut self, id: &'a str) -> Self {
        self.id = Some(id);
        self
    }

    /// Drawn faded: a message the current settings won't send.
    pub fn off(mut self, off: bool) -> Self {
        self.off = off;
        self
    }

    pub fn render(self, body: Markup) -> Markup {
        html! {
            div.dp-msg.off[self.off] id=[self.id] {
                @if let Some((page, cmd)) = self.used {
                    div.dp-used { img src=(page.user.avatar_url()) alt=""; b { (page.user.name) } " used " span.cmd { (cmd) } }
                }
                div.dp-post {
                    @match &self.avatar {
                        Some(url) => img.dp-av src=(url) alt="" width="40" height="40" loading="lazy";,
                        None => span.dp-av.mark { img src="/static/icon.svg" alt=""; },
                    }
                    div.dp-main {
                        div.dp-head {
                            b.dp-name { (self.name) }
                            @if self.app { span.dp-app { "APP" } }
                            span.dp-time { (self.time) }
                        }
                        (body)
                    }
                }
            }
        }
    }
}

/// The message's own text. Use `span.dp-ping` for a mention and `span.dp-ts` for a time
/// Discord fills in for each reader.
pub fn line(body: Markup) -> Markup {
    html! { div.dp-line { (body) } }
}

/// An embed with its colour edge (`color` is any CSS colour).
pub fn embed(color: &str, body: Markup) -> Markup {
    html! { div.dp-embed style={ "--embed:" (color) } { (body) } }
}

/// An embed's fields, side by side as Discord lays out inline fields.
pub fn fields<N: maud::Render, V: maud::Render>(items: impl IntoIterator<Item = (N, V)>) -> Markup {
    html! { div.dp-fields { @for (n, v) in items { div { b { (n) } span { (v) } } } } }
}

/// How a button looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Grey, like a link or secondary button.
    Grey,
    /// Discord's blurple.
    Primary,
    Green,
    Red,
}

/// A row of buttons under a message.
pub fn buttons(body: Markup) -> Markup {
    html! { div.dp-buttons { (body) } }
}

/// A button that opens a link, with Discord's arrow.
pub fn link_button(label: &str, url: &str) -> Markup {
    html! { a.dp-btn href=(url) target="_blank" rel="noopener" { (label) (ui::icon(Icon::External)) } }
}

/// A button that does something in Discord; drawn, not pressable.
pub fn button(label: &str, style: Style) -> Markup {
    let class = match style {
        Style::Grey => "dp-btn",
        Style::Primary => "dp-btn primary",
        Style::Green => "dp-btn green",
        Style::Red => "dp-btn red",
    };
    html! { span class=(class) { (label) } }
}
