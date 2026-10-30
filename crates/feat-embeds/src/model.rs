//! The embed as composed on the dashboard: parsed from the form, checked against Discord's
//! limits with errors a person can act on, and turned into a twilight embed.

use bot_core::twilight_model::channel::message::Embed;
use bot_core::twilight_util::builder::embed::{EmbedBuilder, EmbedFieldBuilder, EmbedFooterBuilder, ImageSource, EmbedAuthorBuilder};
use bot_core::web::FormData;
use serde::{Deserialize, Serialize};

pub const MAX_CONTENT: usize = 2000;
pub const MAX_TITLE: usize = 256;
pub const MAX_DESCRIPTION: usize = 4096;
pub const MAX_AUTHOR: usize = 256;
pub const MAX_FOOTER: usize = 2048;
pub const MAX_FIELD_NAME: usize = 256;
pub const MAX_FIELD_VALUE: usize = 1024;
pub const MAX_FIELDS: usize = 25;
pub const MAX_TOTAL: usize = 6000;
/// How many field rows the form offers. Discord allows 25; ten is plenty by hand.
pub const FORM_FIELDS: usize = 10;
/// The colour a new embed starts with.
pub const DEFAULT_COLOR: &str = "#f0806a";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Field {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbedData {
    /// Plain message text above the embed.
    pub content: String,
    pub title: String,
    pub description: String,
    /// `#rrggbb`, or empty for none.
    pub color: String,
    pub url: String,
    pub author: String,
    pub footer: String,
    pub image: String,
    pub thumbnail: String,
    pub fields: Vec<Field>,
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

fn too_long(what: &str, s: &str, max: usize) -> Result<(), String> {
    let n = chars(s);
    if n > max { Err(format!("{what} is {n} characters. Discord allows {max}.")) } else { Ok(()) }
}

fn check_url(what: &str, s: &str) -> Result<(), String> {
    if s.is_empty() {
        return Ok(());
    }
    let rest = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://"));
    match rest {
        Some(r) if !r.is_empty() && !s.contains(char::is_whitespace) && chars(s) <= 2000 => Ok(()),
        _ => Err(format!("{what} must be a web address starting with http:// or https://.")),
    }
}

/// `#f0806a` or `f0806a` as a number.
pub fn parse_color(s: &str) -> Option<u32> {
    let hex = s.trim().trim_start_matches('#');
    (hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| u32::from_str_radix(hex, 16).ok()).flatten()
}

impl EmbedData {
    /// Read the composer form. Blank field rows are dropped; half-filled ones are kept so
    /// validation can say what's missing.
    pub fn from_form(form: &FormData) -> Self {
        let fields = (0..FORM_FIELDS)
            .map(|i| Field {
                name: form.text(&format!("field_name_{i}")),
                value: form.text(&format!("field_value_{i}")),
                inline: form.bool(&format!("field_inline_{i}")),
            })
            .filter(|f| !(f.name.is_empty() && f.value.is_empty()))
            .collect();
        Self {
            content: form.text("content"),
            title: form.string("title"),
            description: form.text("description"),
            color: parse_color(&form.string("color")).map(|c| format!("#{c:06x}")).unwrap_or_else(|| form.string("color")),
            url: form.string("url"),
            author: form.string("author"),
            footer: form.string("footer"),
            image: form.string("image"),
            thumbnail: form.string("thumbnail"),
            fields,
        }
    }

    /// True when there's anything to draw as an embed (colour and link alone don't count).
    pub fn has_embed(&self) -> bool {
        !(self.title.is_empty()
            && self.description.is_empty()
            && self.author.is_empty()
            && self.footer.is_empty()
            && self.image.is_empty()
            && self.thumbnail.is_empty()
            && self.fields.is_empty())
    }

    /// Characters Discord counts toward the 6,000 limit.
    pub fn total_chars(&self) -> usize {
        chars(&self.title)
            + chars(&self.description)
            + chars(&self.author)
            + chars(&self.footer)
            + self.fields.iter().map(|f| chars(&f.name) + chars(&f.value)).sum::<usize>()
    }

    /// Everything Discord would refuse, without requiring the embed to be complete. Enough to
    /// save a draft as a template.
    pub fn check_limits(&self) -> Result<(), String> {
        too_long("The message text", &self.content, MAX_CONTENT)?;
        too_long("The title", &self.title, MAX_TITLE)?;
        too_long("The description", &self.description, MAX_DESCRIPTION)?;
        too_long("The author name", &self.author, MAX_AUTHOR)?;
        too_long("The footer", &self.footer, MAX_FOOTER)?;
        if self.fields.len() > MAX_FIELDS {
            return Err(format!("There are {} fields. Discord allows {MAX_FIELDS}.", self.fields.len()));
        }
        for (i, f) in self.fields.iter().enumerate() {
            let n = i + 1;
            if f.name.is_empty() || f.value.is_empty() {
                return Err(format!("Field {n} needs both a name and a value."));
            }
            too_long(&format!("The name of field {n}"), &f.name, MAX_FIELD_NAME)?;
            too_long(&format!("The value of field {n}"), &f.value, MAX_FIELD_VALUE)?;
        }
        let total = self.total_chars();
        if total > MAX_TOTAL {
            return Err(format!("The embed is {total} characters in total. Discord allows {MAX_TOTAL} across the title, description, author, footer and fields."));
        }
        if !self.color.is_empty() && parse_color(&self.color).is_none() {
            return Err("The colour must be a hex code like #f0806a.".into());
        }
        check_url("The title link", &self.url)?;
        check_url("The image address", &self.image)?;
        check_url("The thumbnail address", &self.thumbnail)?;
        Ok(())
    }

    /// Ready to send: within the limits, and not empty.
    pub fn validate(&self) -> Result<(), String> {
        self.check_limits()?;
        if !self.has_embed() && self.content.is_empty() {
            return Err("Add a title, description, field or message text first.".into());
        }
        Ok(())
    }

    /// The embed to send, or `None` when only the message text is set. Call after `validate`.
    pub fn embed(&self) -> Option<Embed> {
        if !self.has_embed() {
            return None;
        }
        let mut b = EmbedBuilder::new();
        if !self.title.is_empty() {
            b = b.title(&self.title);
        }
        if !self.description.is_empty() {
            b = b.description(&self.description);
        }
        if let Some(c) = parse_color(&self.color) {
            b = b.color(c);
        }
        if !self.url.is_empty() {
            b = b.url(&self.url);
        }
        if !self.author.is_empty() {
            b = b.author(EmbedAuthorBuilder::new(&self.author));
        }
        if !self.footer.is_empty() {
            b = b.footer(EmbedFooterBuilder::new(&self.footer));
        }
        if let Ok(src) = ImageSource::url(&self.image) {
            b = b.image(src);
        }
        if let Ok(src) = ImageSource::url(&self.thumbnail) {
            b = b.thumbnail(src);
        }
        for f in &self.fields {
            let field = EmbedFieldBuilder::new(&f.name, &f.value);
            b = b.field(if f.inline { field.inline() } else { field });
        }
        Some(b.build())
    }

    /// A short line for lists: the title, else the start of the description or message.
    pub fn summary(&self) -> String {
        let first = [&self.title, &self.description, &self.content, &self.author, &self.footer]
            .into_iter()
            .find(|s| !s.is_empty())
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_else(|| "Empty embed".into());
        bot_core::text::clip(&first, 80)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic() -> EmbedData {
        EmbedData { title: "Rules".into(), description: "Be kind.".into(), color: "#f0806a".into(), ..Default::default() }
    }

    fn err(d: &EmbedData) -> String {
        d.validate().unwrap_err()
    }

    #[test]
    fn a_simple_embed_is_valid() {
        basic().validate().unwrap();
        let e = basic().embed().unwrap();
        assert_eq!(e.title.as_deref(), Some("Rules"));
        assert_eq!(e.color, Some(0xf0806a));
    }

    #[test]
    fn text_only_sends_no_embed() {
        let d = EmbedData { content: "Hello".into(), color: "#ffffff".into(), url: "https://example.com".into(), ..Default::default() };
        d.validate().unwrap();
        assert!(d.embed().is_none());
    }

    #[test]
    fn empty_is_refused() {
        assert!(err(&EmbedData::default()).starts_with("Add a title"));
        // A colour or link alone isn't content.
        assert!(err(&EmbedData { color: "#ffffff".into(), url: "https://a.b".into(), ..Default::default() }).starts_with("Add a title"));
    }

    #[test]
    fn lengths_name_what_is_too_long() {
        let d = EmbedData { title: "t".repeat(257), ..basic() };
        assert_eq!(err(&d), "The title is 257 characters. Discord allows 256.");
        let d = EmbedData { description: "d".repeat(4097), ..basic() };
        assert!(err(&d).starts_with("The description is 4097"));
        let d = EmbedData { content: "c".repeat(2001), ..basic() };
        assert!(err(&d).starts_with("The message text is 2001"));
        let d = EmbedData { footer: "f".repeat(2049), ..basic() };
        assert!(err(&d).starts_with("The footer is 2049"));
        // Limits count characters, not bytes.
        EmbedData { title: "é".repeat(256), ..basic() }.validate().unwrap();
    }

    #[test]
    fn field_rules() {
        let field = |n: &str, v: &str| Field { name: n.into(), value: v.into(), inline: false };
        let d = EmbedData { fields: vec![field("A", "1"), field("B", "")], ..basic() };
        assert_eq!(err(&d), "Field 2 needs both a name and a value.");
        let d = EmbedData { fields: vec![field("A", &"v".repeat(1025))], ..basic() };
        assert!(err(&d).starts_with("The value of field 1 is 1025"));
        let d = EmbedData { fields: (0..26).map(|_| field("A", "1")).collect(), ..basic() };
        assert_eq!(err(&d), "There are 26 fields. Discord allows 25.");
        let d = EmbedData { fields: (0..25).map(|_| field("A", "1")).collect(), ..basic() };
        d.validate().unwrap();
    }

    #[test]
    fn total_is_capped_across_parts() {
        let field = |v: usize| Field { name: "n".into(), value: "v".repeat(v), inline: true };
        // 4000 + 1000 + 1000 + 3 is just over 6000 even though no single part is.
        let d = EmbedData { description: "d".repeat(4000), fields: vec![field(1000), field(1000)], title: "ttt".into(), ..Default::default() };
        assert_eq!(d.total_chars(), 4000 + 2 + 2000 + 3);
        assert!(err(&d).starts_with("The embed is 6005 characters in total"));
        let ok = EmbedData { fields: vec![field(1000)], ..d };
        ok.validate().unwrap();
    }

    #[test]
    fn colours_and_links() {
        assert_eq!(parse_color("#F0806A"), Some(0xf0806a));
        assert_eq!(parse_color("f0806a"), Some(0xf0806a));
        assert_eq!(parse_color("#fff"), None);
        assert_eq!(parse_color("#gggggg"), None);
        assert_eq!(err(&EmbedData { color: "red".into(), ..basic() }), "The colour must be a hex code like #f0806a.");
        assert!(err(&EmbedData { image: "ftp://x/y.png".into(), ..basic() }).starts_with("The image address must be"));
        assert!(err(&EmbedData { url: "https://".into(), ..basic() }).starts_with("The title link must be"));
        assert!(err(&EmbedData { thumbnail: "https://a b".into(), ..basic() }).starts_with("The thumbnail address must be"));
        EmbedData { image: "https://cdn.example.com/a.png".into(), url: "http://example.com".into(), ..basic() }.validate().unwrap();
    }

    #[test]
    fn form_parsing_keeps_rows_and_drops_blanks() {
        let body = "title=Hi&description=a%0D%0Ab&color=F0806A&content=&field_name_0=A&field_value_0=1&field_inline_0=on&field_name_1=&field_value_1=&field_name_4=Only+name&field_value_4=";
        let d = EmbedData::from_form(&FormData::parse(body.as_bytes()));
        assert_eq!(d.description, "a\nb");
        assert_eq!(d.color, "#f0806a");
        assert_eq!(d.fields.len(), 2);
        assert!(d.fields[0].inline);
        assert_eq!(d.fields[1].name, "Only name");
        assert_eq!(err(&d), "Field 2 needs both a name and a value.");
    }

    #[test]
    fn embed_carries_everything() {
        let d = EmbedData {
            title: "T".into(),
            url: "https://example.com".into(),
            author: "Me".into(),
            footer: "F".into(),
            image: "https://example.com/i.png".into(),
            thumbnail: "https://example.com/t.png".into(),
            fields: vec![Field { name: "a".into(), value: "b".into(), inline: true }],
            ..Default::default()
        };
        d.validate().unwrap();
        let e = d.embed().unwrap();
        assert_eq!(e.url.as_deref(), Some("https://example.com"));
        assert_eq!(e.author.unwrap().name, "Me");
        assert_eq!(e.footer.unwrap().text, "F");
        assert!(e.image.is_some() && e.thumbnail.is_some());
        assert!(e.fields[0].inline);
    }

    #[test]
    fn summary_prefers_the_title() {
        assert_eq!(basic().summary(), "Rules");
        assert_eq!(EmbedData { content: "Hello\nthere".into(), ..Default::default() }.summary(), "Hello there");
        assert_eq!(EmbedData::default().summary(), "Empty embed");
    }
}
