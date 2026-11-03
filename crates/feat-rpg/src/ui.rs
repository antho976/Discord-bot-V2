//! What a screen is made of, independent of Discord: a banner, blocks of text inside the
//! panel, and rows of buttons and menus under it. [`Screen::components`] turns it into
//! Discord's Components V2; [`Screen::html`] draws it on the dashboard.

use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::twilight_model::channel::message::EmojiReactionType;
use bot_core::twilight_model::channel::message::component::{
    ActionRow, Button, ButtonStyle, Component, Container, MediaGallery, MediaGalleryItem, Section, SelectMenu, SelectMenuOption,
    SelectMenuType, Separator, SeparatorSpacingSize, TextDisplay, UnfurledMediaItem,
};
use bot_core::twilight_model::id::Id;

use crate::emoji;

/// The panel's accent: bronze.
pub const ACCENT: u32 = 0xD2A75E;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Primary,
    Secondary,
    Success,
    Danger,
}

#[derive(Debug, Clone)]
pub struct Btn {
    pub id: String,
    pub label: String,
    pub emoji: Option<&'static str>,
    pub style: Style,
    pub disabled: bool,
}

impl Btn {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self { id: id.into(), label: label.into(), emoji: None, style: Style::Secondary, disabled: false }
    }
    pub fn emoji(mut self, e: &'static str) -> Self {
        self.emoji = Some(e);
        self
    }
    pub fn primary(mut self) -> Self {
        self.style = Style::Primary;
        self
    }
    pub fn danger(mut self) -> Self {
        self.style = Style::Danger;
        self
    }
    pub fn success(mut self) -> Self {
        self.style = Style::Success;
        self
    }
    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }
}

#[derive(Debug, Clone)]
pub struct Opt {
    pub label: String,
    pub value: String,
    pub desc: Option<String>,
    pub emoji: Option<&'static str>,
    pub default: bool,
}

impl Opt {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self { label: label.into(), value: value.into(), desc: None, emoji: None, default: false }
    }
    pub fn desc(mut self, d: impl Into<String>) -> Self {
        self.desc = Some(d.into());
        self
    }
    pub fn emoji(mut self, e: &'static str) -> Self {
        self.emoji = Some(e);
        self
    }
    pub fn default(mut self, on: bool) -> Self {
        self.default = on;
        self
    }
}

#[derive(Debug, Clone)]
pub enum Block {
    /// Markdown.
    Text(String),
    /// Markdown with a button beside it.
    Section(String, Btn),
    /// A little space.
    Gap,
    /// A select menu inside the panel.
    Select { id: String, placeholder: String, options: Vec<Opt> },
}

#[derive(Debug, Clone)]
pub enum Row {
    Buttons(Vec<Btn>),
    Select { id: String, placeholder: String, options: Vec<Opt> },
}

#[derive(Debug, Clone, Default)]
pub struct Screen {
    pub banner: Option<String>,
    pub blocks: Vec<Block>,
    pub rows: Vec<Row>,
}

impl Screen {
    pub fn text(&mut self, md: impl Into<String>) -> &mut Self {
        self.blocks.push(Block::Text(md.into()));
        self
    }
    pub fn section(&mut self, md: impl Into<String>, b: Btn) -> &mut Self {
        self.blocks.push(Block::Section(md.into(), b));
        self
    }
    pub fn gap(&mut self) -> &mut Self {
        self.blocks.push(Block::Gap);
        self
    }
    pub fn buttons(&mut self, b: Vec<Btn>) -> &mut Self {
        let b: Vec<Btn> = b.into_iter().take(5).collect();
        if !b.is_empty() {
            self.rows.push(Row::Buttons(b));
        }
        self
    }
    pub fn select(&mut self, id: impl Into<String>, placeholder: impl Into<String>, options: Vec<Opt>) -> &mut Self {
        if !options.is_empty() {
            self.rows.push(Row::Select { id: id.into(), placeholder: placeholder.into(), options: options.into_iter().take(25).collect() });
        }
        self
    }
    pub fn inner_select(&mut self, id: impl Into<String>, placeholder: impl Into<String>, options: Vec<Opt>) -> &mut Self {
        if !options.is_empty() {
            self.blocks.push(Block::Select { id: id.into(), placeholder: placeholder.into(), options: options.into_iter().take(25).collect() });
        }
        self
    }
    /// Put a notice at the very top of the panel.
    pub fn note(mut self, n: Option<String>) -> Self {
        if let Some(n) = n {
            self.blocks.insert(0, Block::Text(format!("{} {n}", emoji::get("warn"))));
        }
        self
    }

    /// Discord refuses a message with two identical custom ids. Two buttons that open the same
    /// screen are common ("Plan one" and "Expedition"), so later twins get a tag in the route's
    /// argument slot, which opening a screen ignores.
    fn unique(&self) -> Screen {
        let mut s = self.clone();
        let mut seen = std::collections::HashSet::new();
        let mut fix = |id: &mut String| {
            let mut n = 1;
            let base = id.clone();
            while !seen.insert(id.clone()) {
                n += 1;
                let parts: Vec<&str> = base.split(':').collect();
                *id = if parts.len() == 6 { format!("{}:{}:{}:{}:{}{}:{}", parts[0], parts[1], parts[2], parts[3], parts[4], n, parts[5]) } else { format!("{base}~{n}") };
            }
        };
        for b in &mut s.blocks {
            match b {
                Block::Section(_, btn) => fix(&mut btn.id),
                Block::Select { id, .. } => fix(id),
                _ => {}
            }
        }
        for r in &mut s.rows {
            match r {
                Row::Buttons(bs) => bs.iter_mut().for_each(|b| fix(&mut b.id)),
                Row::Select { id, .. } => fix(id),
            }
        }
        s
    }

    /// Discord Components V2: one container holding the banner and the blocks, then the rows.
    pub fn components(&self) -> Vec<Component> {
        let this = self.unique();
        let mut inner = Vec::new();
        if let Some(url) = &this.banner {
            inner.push(Component::MediaGallery(MediaGallery {
                id: None,
                items: vec![MediaGalleryItem { media: media(url), description: None, spoiler: None }],
            }));
        }
        // Neighbouring text blocks are joined into one display: fewer components.
        let mut text = String::new();
        let flush = |text: &mut String, inner: &mut Vec<Component>| {
            if !text.is_empty() {
                inner.push(Component::TextDisplay(TextDisplay { id: None, content: std::mem::take(text) }));
            }
        };
        for b in &this.blocks {
            match b {
                Block::Text(t) => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(t);
                }
                Block::Section(t, btn) => {
                    flush(&mut text, &mut inner);
                    inner.push(Component::Section(Section {
                        id: None,
                        components: vec![Component::TextDisplay(TextDisplay { id: None, content: t.clone() })],
                        accessory: Box::new(button(btn)),
                    }));
                }
                Block::Gap => {
                    flush(&mut text, &mut inner);
                    inner.push(Component::Separator(Separator { id: None, divider: Some(false), spacing: Some(SeparatorSpacingSize::Small) }));
                }
                Block::Select { id, placeholder, options } => {
                    flush(&mut text, &mut inner);
                    inner.push(select(id, placeholder, options));
                }
            }
        }
        flush(&mut text, &mut inner);
        let mut out = vec![Component::Container(Container { id: None, accent_color: Some(Some(ACCENT)), spoiler: None, components: inner })];
        for r in &this.rows {
            match r {
                Row::Buttons(bs) => out.push(Component::ActionRow(ActionRow { id: None, components: bs.iter().map(button).collect() })),
                Row::Select { id, placeholder, options } => out.push(select(id, placeholder, options)),
            }
        }
        out
    }

    /// The same screen drawn as HTML, for the dashboard's preview.
    pub fn html(&self) -> Markup {
        html! {
            div.rpg-msg {
                div.rpg-panel {
                    @if let Some(url) = &self.banner { img.rpg-banner src=(url) alt="Banner"; }
                    @for b in &self.blocks {
                        @match b {
                            Block::Text(t) => div.rpg-text { (md(t)) },
                            Block::Section(t, btn) => div.rpg-section { div.rpg-text { (md(t)) } (btn_html(btn)) },
                            Block::Gap => div.rpg-gap {},
                            Block::Select { placeholder, .. } => div.rpg-select { (placeholder) span { "▾" } },
                        }
                    }
                }
                @for r in &self.rows {
                    @match r {
                        Row::Buttons(bs) => div.rpg-row { @for b in bs { (btn_html(b)) } },
                        Row::Select { placeholder, .. } => div.rpg-select { (placeholder) span { "▾" } },
                    }
                }
            }
        }
    }
}

fn media(url: &str) -> UnfurledMediaItem {
    UnfurledMediaItem { url: url.to_owned(), proxy_url: None, height: None, width: None, content_type: None }
}

fn emoji_type(name: &str) -> Option<EmojiReactionType> {
    match emoji::custom(name) {
        Some((id, n)) => Some(EmojiReactionType::Custom { animated: false, id: Id::new(id), name: Some(n) }),
        None => Some(EmojiReactionType::Unicode { name: emoji::unicode(name).to_owned() }),
    }
}

fn button(b: &Btn) -> Component {
    Component::Button(Button {
        id: None,
        custom_id: Some(b.id.clone()),
        disabled: b.disabled,
        emoji: b.emoji.and_then(emoji_type),
        label: Some(b.label.clone()),
        style: match b.style {
            Style::Primary => ButtonStyle::Primary,
            Style::Secondary => ButtonStyle::Secondary,
            Style::Success => ButtonStyle::Success,
            Style::Danger => ButtonStyle::Danger,
        },
        url: None,
        sku_id: None,
    })
}

fn select(id: &str, placeholder: &str, options: &[Opt]) -> Component {
    Component::ActionRow(ActionRow {
        id: None,
        components: vec![Component::SelectMenu(SelectMenu {
            id: None,
            channel_types: None,
            custom_id: id.to_owned(),
            default_values: None,
            disabled: false,
            kind: SelectMenuType::Text,
            max_values: Some(1),
            min_values: Some(1),
            options: Some(
                options
                    .iter()
                    .map(|o| SelectMenuOption {
                        default: o.default,
                        description: o.desc.clone().map(|d| truncate(&d, 100)),
                        emoji: o.emoji.and_then(emoji_type),
                        label: truncate(&o.label, 100),
                        value: o.value.clone(),
                    })
                    .collect(),
            ),
            placeholder: Some(truncate(placeholder, 150)),
            required: None,
        })],
    })
}

pub fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_owned() } else { s.chars().take(n - 1).collect::<String>() + "…" }
}

fn btn_html(b: &Btn) -> Markup {
    let class = match b.style {
        Style::Primary => "rpg-btn primary",
        Style::Secondary => "rpg-btn",
        Style::Success => "rpg-btn success",
        Style::Danger => "rpg-btn danger",
    };
    html! {
        span class=(class) data-off[b.disabled] {
            @if let Some(e) = b.emoji { span.rpg-emoji { (PreEscaped(emoji::html(e))) } }
            (b.label)
        }
    }
}

/// The markdown Discord shows, as far as these screens use it: `### headings`, `-# small
/// text`, `**bold**` and custom emojis.
pub fn md(t: &str) -> Markup {
    html! {
        @for line in t.lines() {
            @if let Some(h) = line.strip_prefix("### ") {
                div.rpg-h { (inline(h)) }
            } @else if let Some(s) = line.strip_prefix("-# ") {
                div.rpg-sub { (inline(s)) }
            } @else if line.is_empty() {
                div.rpg-blank {}
            } @else {
                div { (inline(line)) }
            }
        }
    }
}

fn inline(s: &str) -> Markup {
    let esc = s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    // Custom emojis were escaped above: put them back as images.
    let mut out = String::new();
    let mut rest = esc.as_str();
    while let Some(i) = rest.find("&lt;:") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 5..];
        match after.find("&gt;") {
            Some(j) => {
                let inner = &after[..j];
                let id = inner.rsplit(':').next().unwrap_or("");
                out.push_str(&format!("<img class=\"rpg-emoji\" alt=\"\" src=\"https://cdn.discordapp.com/emojis/{id}.png?size=44\">"));
                rest = &after[j + 4..];
            }
            None => {
                out.push_str("&lt;:");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    let mut bolded = String::new();
    for (i, part) in out.split("**").enumerate() {
        if i % 2 == 1 {
            bolded.push_str("<b>");
            bolded.push_str(part);
            bolded.push_str("</b>");
        } else {
            bolded.push_str(part);
        }
    }
    PreEscaped(bolded)
}
