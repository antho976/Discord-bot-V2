use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::http::StatusCode;
use bot_core::axum::response::{IntoResponse, Redirect};
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::twilight_http::error::ErrorType;
use bot_core::twilight_model::channel::message::{AllowedMentions, Embed, MentionType};
use bot_core::twilight_model::id::Id;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::rusqlite::OptionalExtension;
use bot_core::{Ctx, Icon, now_ms, text};
use serde::Deserialize;

use crate::model::{self, EmbedData, FORM_FIELDS, Field};
use crate::{KEY, Sent};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/embeds", get(compose))
        .route("/g/{guild}/embeds/send", get(back).post(send))
        .route("/g/{guild}/embeds/sent/{id}/update", get(back_to_sent).post(update))
        .route("/g/{guild}/embeds/sent/{id}/delete", post(forget))
        .route("/g/{guild}/embeds/templates", get(back).post(save_template))
        .route("/g/{guild}/embeds/templates/{id}/delete", post(delete_template))
        .route("/g/{guild}/embeds/templates/{id}/duplicate", post(duplicate_template))
}

/// A refused send or save answers with the composer at its own address. Opened again (a reload,
/// or Back after leaving), that address goes to the composer instead of a 405.
async fn back(Path(guild): Path<u64>) -> Redirect {
    Redirect::to(&format!("/g/{guild}/embeds"))
}

async fn back_to_sent(Path((guild, id)): Path<(u64, i64)>) -> Redirect {
    Redirect::to(&format!("/g/{guild}/embeds?sent={id}"))
}

/// What the composer shows: the embed, where it goes, and what it's replacing.
struct View<'a> {
    data: EmbedData,
    channel: Option<u64>,
    template_name: String,
    editing: Option<&'a Sent>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct ComposeQuery {
    template: Option<i64>,
    sent: Option<i64>,
    /// With `sent`: load that message as a new embed instead of editing it.
    copy: Option<u8>,
}

/// The preset colours offered under the editor. The native picker and the hex field take any other.
const SWATCHES: [&str; 5] = ["#f0806a", "#c3a6ff", "#4fc38a", "#5865f2", "#e3a948"];

async fn compose(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, Query(q): Query<ComposeQuery>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let mut view = View {
        data: EmbedData { color: model::DEFAULT_COLOR.into(), ..Default::default() },
        channel: None,
        template_name: String::new(),
        editing: None,
        error: None,
    };
    let sent;
    if let Some(id) = q.sent {
        sent = crate::sent_get(&ctx, guild, id)?.ok_or(WebError::NotFound)?;
        view.data = sent.data.clone();
        view.channel = Some(sent.channel_id);
        if q.copy.is_none() {
            view.template_name = sent.template.clone().unwrap_or_default();
            view.editing = Some(&sent);
        }
    } else if let Some(id) = q.template {
        let t = crate::template(&ctx, guild, id)?.ok_or(WebError::NotFound)?;
        view.data = t.data;
        view.template_name = t.name;
    }
    Ok(render(&ctx, &page, view)?.into_response())
}

fn render(ctx: &Ctx, page: &Page, v: View<'_>) -> bot_core::anyhow::Result<Markup> {
    let guild = page.guild_id;
    let templates = crate::templates(ctx, guild)?;
    let sent = crate::sent_list(ctx, guild)?;
    let channels = ctx.text_channels(guild);
    let channel_name = |id: u64| channels.iter().find(|(c, _)| *c == id).map(|(_, n)| format!("#{n}")).unwrap_or_else(|| "a deleted channel".into());
    let bot = ctx.cache.current_user();
    let bot_name = bot.as_ref().map(|u| u.name.clone()).unwrap_or_else(|| "Bot".into());
    let bot_avatar = bot.as_ref().map(|u| bot_core::discord::avatar_url(u.id.get(), u.avatar.map(|a| a.to_string()).as_deref()));
    let d = &v.data;
    let color_ok = model::parse_color(&d.color).map(|c| format!("#{c:06x}"));
    let mut options = vec![(String::new(), "Pick a channel".to_owned())];
    options.extend(channels.iter().map(|(id, n)| (id.to_string(), format!("#{n}"))));
    let selected = v.channel.map(|c| c.to_string()).unwrap_or_default();
    let picked = v.channel.and_then(|c| channels.iter().find(|(id, _)| *id == c)).map(|(_, n)| n.clone());
    let can_edit = page.can_edit();
    let rows: Vec<&Field> = d.fields.iter().take(FORM_FIELDS).collect();
    let time = page.user.local(now_ms()).strftime("%-I:%M %p").to_string();
    let current = templates.iter().find(|t| t.name == v.template_name && v.editing.is_none());

    let strip = html! {
        nav.eb-tpls aria-label="Saved embeds" {
            @for t in &templates {
                span.eb-tpl-wrap {
                    a.eb-tpl.on[t.name == v.template_name] href=(page.path(&format!("embeds?template={}", t.id))) title=(t.data.summary()) aria-current=[(t.name == v.template_name).then_some("true")] {
                        span.eb-dot style={ "--c:" (swatch(&t.data.color)) } {}
                        span.eb-tpl-name { (t.name) }
                    }
                    @if can_edit {
                        form.inline method="post" action=(page.path(&format!("embeds/templates/{}/delete", t.id))) {
                            button.eb-x type="submit" data-confirm="Delete?" aria-label={ "Delete " (t.name) } title="Delete this template" { (ui::icon(Icon::Close)) }
                        }
                    }
                }
            }
            a.eb-tpl.new href=(page.path("embeds")) { (ui::icon(Icon::Plus)) "New" }
            @if let Some(t) = current {
                span.eb-saved {
                    span.muted.small { "Saved " (text::ago(t.updated_at)) }
                    @if can_edit { (ui::action_button(&page.path(&format!("embeds/templates/{}/duplicate", t.id)), "Duplicate", Tone::Muted, false)) }
                }
            }
        }
    };

    let body = html! {
        style { (PreEscaped(CSS)) }
        (strip)
        form #eb-form method="post" action=(page.path("embeds/send")) data-max-fields=(FORM_FIELDS) {
            fieldset.eb.compose disabled[!can_edit] {
                div.eb-edit {
                    @if let Some(s) = v.editing {
                        div.eb-note {
                            span.eb-note-ic { (ui::icon(Icon::Brush)) }
                            p {
                                "Editing the message in " strong { (channel_name(s.channel_id)) } ", sent " (text::ago(s.sent_at)) " by " (s.sent_by) ". "
                                "Update changes it in place."
                            }
                            a.btn.small href=(page.path("embeds")) { "New embed" }
                        }
                    }
                    @if let Some(e) = &v.error {
                        div.eb-error role="alert" { (ui::icon(Icon::Close)) span { (e) } }
                    }
                    div.eb-card {
                        div.eb-col {
                            section.eb-grp {
                                (group_head(chat_icon(), "Message text", None))
                                (text_field("content", "Text above the embed", "Optional. Mentions of members and roles ping them; @everyone and @here don't.", &d.content, true, Some(model::MAX_CONTENT), 2, "content", ""))
                            }
                            section.eb-grp {
                                (group_head(ui::icon(Icon::Brush), "Embed", None))
                                div.eb-f data-part="bar" {
                                    div.eb-lab { label for="color" { "Colour" } }
                                    div.eb-colours {
                                        @for c in SWATCHES {
                                            button.eb-sw type="button" data-swatch=(c) style={ "--c:" (c) } aria-label={ "Colour " (c) } aria-pressed="false" {}
                                        }
                                        label.eb-pick title="Pick any colour" {
                                            input.eb-swatch #eb-swatch type="color" value=(color_ok.as_deref().unwrap_or("#2b2d31")) aria-label="Pick any colour";
                                            (ui::icon(Icon::Plus))
                                        }
                                        input.input.num.eb-hex #color type="text" name="color" value=(d.color) placeholder="No colour" autocomplete="off" maxlength="7" aria-label="Colour as hex";
                                    }
                                }
                                (text_field("author", "Author", "", &d.author, false, Some(model::MAX_AUTHOR), 1, "author", "A name over the title"))
                                div.eb-2 {
                                    (text_field("title", "Title", "", &d.title, false, Some(model::MAX_TITLE), 1, "title", ""))
                                    (text_field("url", "Title link", "", &d.url, false, None, 1, "title", "https://"))
                                }
                                (text_field("description", "Description", "Supports **bold**, *italic*, `code`, ~~strike~~, ||spoilers|| and [links](https://example.com).", &d.description, true, Some(model::MAX_DESCRIPTION), 6, "desc", ""))
                            }
                        }
                        div.eb-col {
                            section.eb-grp.eb-fblock {
                                (group_head(fields_icon(), "Fields", Some(html! { span.muted.small #eb-fcount {} })))
                                div #eb-rows {
                                    @for (i, f) in rows.iter().enumerate() { (field_row(&i.to_string(), f)) }
                                }
                                div { button.btn.small.outline #eb-add type="button" { (ui::icon(Icon::Plus)) "Add a field" } }
                                template #eb-ftpl { (field_row("__I__", &Field::default())) }
                            }
                            section.eb-grp {
                                (group_head(ui::icon(Icon::Image), "Pictures and footer", None))
                                div.eb-2 {
                                    (text_field("image", "Image", "Large, under the fields.", &d.image, false, None, 1, "img", "https://cdn.discordapp.com/.../banner.png"))
                                    (text_field("thumbnail", "Thumbnail", "Small, at the top right.", &d.thumbnail, false, None, 1, "th", "https://"))
                                }
                                (text_field("footer", "Footer", "", &d.footer, false, Some(model::MAX_FOOTER), 1, "foot", "Small print at the bottom"))
                            }
                        }
                    }
                    div.eb-error #eb-limit role="alert" hidden {}
                }
                aside.eb-side.compose-preview {
                    div.eb-win {
                        div.eb-win-bar {
                            (ui::icon(Icon::Hash))
                            strong #eb-chan-name data-none="no channel picked" { (picked.as_deref().unwrap_or("no channel picked")) }
                            span.eb-win-tag { "Preview" }
                        }
                        div.eb-win-body {
                            div.dc #eb-msg {
                                @if let Some(url) = &bot_avatar {
                                    img.dc-av src=(url) alt="" width="40" height="40" loading="lazy";
                                } @else {
                                    span.dc-av.mark { img src="/static/icon.svg" alt="" width="22" height="22"; }
                                }
                                div.dc-main {
                                    div.dc-head { b.dc-name { (bot_name) } span.dc-tag { "APP" } span.dc-time { "Today at " (time) } }
                                    div.dc-content #p-content {}
                                    div.dc-slot #p-embed {}
                                }
                            }
                        }
                    }
                    div.eb-total {
                        span.eb-total-bar aria-hidden="true" { span #eb-total-fill {} }
                        span.small #eb-total {}
                    }
                    @if can_edit {
                        div.eb-send {
                            div.eb-chan {
                                label for="channel" { @if v.editing.is_some() { "Send as new to" } @else { "Send to" } }
                                (ui::select("channel", &options, &selected))
                            }
                            div.eb-btns {
                                @if let Some(s) = v.editing {
                                    button.btn.primary type="submit" formaction=(page.path(&format!("embeds/sent/{}/update", s.id))) { "Update message" }
                                    button.btn type="submit" formaction=(page.path("embeds/send")) { "Send as new" }
                                } @else {
                                    button.btn.primary type="submit" formaction=(page.path("embeds/send")) { "Send" }
                                }
                                details.pop.eb-savetpl {
                                    summary.btn { "Save template" }
                                    div.pop-body {
                                        div.eb-f {
                                            label for="template_name" { "Template name" }
                                            input.input #template_name type="text" name="template_name" value=(v.template_name) placeholder="Name it" maxlength="60" autocomplete="off";
                                        }
                                        div.eb-btns {
                                            button.btn.primary type="submit" formaction=(page.path("embeds/templates")) { "Save template" }
                                        }
                                        p.help { "Saved embeds show up as chips above the composer." }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        (ui::section("Sent", Some("Embeds posted from here. Edit one to change the message in Discord."), html! {
            @if sent.is_empty() {
                (ui::empty(Icon::Log, "Nothing sent yet", "Messages you send from the composer are listed here so you can edit them later."))
            } @else {
                div.eb-sent {
                    @for s in &sent {
                        @let current = v.editing.is_some_and(|e| e.id == s.id);
                        @let pic = [&s.data.image, &s.data.thumbnail].into_iter().find(|u| u.starts_with("https://") || u.starts_with("http://"));
                        div.eb-srow.cur[current] {
                            div.eb-mini style={ "--c:" (swatch(&s.data.color)) } {
                                div.eb-mini-text {
                                    strong.clip { (s.summary) }
                                    @let line = first_line(&s.data);
                                    @if !line.is_empty() { span.clip { (line) } }
                                }
                                @if let Some(u) = pic { img.eb-mini-pic src=(u) alt="" loading="lazy"; }
                            }
                            div.eb-smeta {
                                span.eb-chan-chip { (ui::icon(Icon::Hash)) (channel_name(s.channel_id).trim_start_matches('#')) }
                                span.eb-by {
                                    @if let Some(face) = face_of(ctx, page, &s.sent_by) { img.avatar src=(face) alt="" width="20" height="20"; }
                                    span.clip { (s.sent_by) " · " (text::ago(s.edited_at.unwrap_or(s.sent_at))) @if s.edited_at.is_some() { " (edited)" } }
                                }
                                @if let Some(t) = &s.template { span.muted.small.clip { "from " (t) } }
                            }
                            div.eb-sacts {
                                @if current {
                                    span.badge.accent { "Editing" }
                                } @else {
                                    a.btn.small href=(page.path(&format!("embeds?sent={}", s.id))) { "Edit" }
                                }
                                // On a phone the row keeps Edit and Forget; Duplicate and Open would crowd it.
                                @if can_edit { a.btn.small.ghost.m-hide href=(page.path(&format!("embeds?sent={}&copy=1", s.id))) title="Start a new embed from this one" { "Duplicate" } }
                                a.icon-btn.m-hide href={ "https://discord.com/channels/" (guild) "/" (s.channel_id) "/" (s.message_id) } target="_blank" rel="noreferrer noopener" data-tip="Open in Discord" aria-label="Open in Discord" { (ui::icon(Icon::External)) }
                                @if can_edit { (ui::action_button(&page.path(&format!("embeds/sent/{}/delete", s.id)), "Forget", Tone::Bad, true)) }
                            }
                        }
                    }
                }
            }
        }))
        script { (PreEscaped(JS)) }
    };
    Ok(ui::layout(ctx, page, "Embed builder", None, body))
}

/// A stored colour for a dot or a bar: the embed's own, or Discord's plain edge.
fn swatch(color: &str) -> String {
    model::parse_color(color).map(|c| format!("#{c:06x}")).unwrap_or_else(|| "#4e5058".into())
}

/// The line under a sent embed's title in the list: its description, or the message text.
/// Nothing when there's no title, since the summary already took the first text.
fn first_line(d: &EmbedData) -> String {
    let next = if d.title.is_empty() { "" } else if !d.description.is_empty() { &d.description } else { &d.content };
    text::clip(&next.split_whitespace().collect::<Vec<_>>().join(" "), 90)
}

/// The face of whoever sent a message from the dashboard: the person looking, or anyone with
/// dashboard access by that name.
fn face_of(ctx: &Ctx, page: &Page, name: &str) -> Option<String> {
    if name == page.user.name {
        return Some(page.user.avatar_url());
    }
    let id: Option<i64> = ctx
        .db
        .with(|c| c.query_row("SELECT id FROM dash_users WHERE name = ?1 LIMIT 1", [name], |r| r.get(0)).optional())
        .ok()
        .flatten();
    id.map(|id| ctx.member_avatar(page.guild_id, id as u64))
}

fn group_head(ic: Markup, title: &str, extra: Option<Markup>) -> Markup {
    html! {
        div.eb-grp-head {
            span.eb-grp-ic { (ic) }
            h3 { (title) }
            @if let Some(x) = extra { (x) }
        }
    }
}

/// A speech bubble, for the message text.
fn chat_icon() -> Markup {
    html! {
        svg.icon viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" {
            (PreEscaped(r#"<path d="M5 4.5h14A1.5 1.5 0 0 1 20.5 6v9.5A1.5 1.5 0 0 1 19 17h-9l-4.5 3.5V17H5a1.5 1.5 0 0 1-1.5-1.5V6A1.5 1.5 0 0 1 5 4.5zM8 9h8M8 12.5h5"/>"#))
        }
    }
}

/// Name and value pairs, for the fields.
fn fields_icon() -> Markup {
    html! {
        svg.icon viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" {
            (PreEscaped(r#"<path d="M5.5 4.5h4A1.5 1.5 0 0 1 11 6v4a1.5 1.5 0 0 1-1.5 1.5h-4A1.5 1.5 0 0 1 4 10V6a1.5 1.5 0 0 1 1.5-1.5ZM14.5 4.5h4A1.5 1.5 0 0 1 20 6v4a1.5 1.5 0 0 1-1.5 1.5h-4A1.5 1.5 0 0 1 13 10V6a1.5 1.5 0 0 1 1.5-1.5ZM4 15h16M4 19h11"/>"#))
        }
    }
}

/// A labelled input or textarea with a live character count when it has a limit. `part` names
/// the piece of the preview it fills, which lights while you type in it.
#[allow(clippy::too_many_arguments)]
fn text_field(name: &str, label: &str, help: &str, value: &str, long: bool, max: Option<usize>, rows: u8, part: &str, placeholder: &str) -> Markup {
    html! {
        div.eb-f data-part=(part) {
            div.eb-lab {
                label for=(name) { (label) }
                @if let Some(m) = max { span.eb-count data-count=(name) data-max=(m) {} }
            }
            @if long {
                textarea.input #(name) name=(name) rows=(rows) placeholder=(placeholder) { (value) }
            } @else {
                input.input #(name) type="text" name=(name) value=(value) placeholder=(placeholder) autocomplete="off";
            }
            @if !help.is_empty() { p.help { (help) } }
        }
    }
}

fn field_row(i: &str, f: &Field) -> Markup {
    html! {
        div.eb-frow data-i=(i) {
            div.eb-frow-top {
                input.input.fn-name type="text" name={ "field_name_" (i) } value=(f.name) placeholder="Field name" aria-label="Field name" autocomplete="off";
                label.chip.eb-inline title="Inline fields sit side by side, up to three in a row" {
                    input.fn-inline type="checkbox" name={ "field_inline_" (i) } checked[f.inline];
                    span { "Inline" }
                }
                button.icon-btn.eb-frm type="button" data-remove="1" aria-label="Remove this field" data-tip="Remove" { (ui::icon(Icon::Trash)) }
            }
            textarea.input.fn-value name={ "field_value_" (i) } rows="2" placeholder="Field value" aria-label="Field value" { (f.value) }
        }
    }
}

// ---- actions ----

fn rerender(ctx: &Ctx, page: &Page, view: View<'_>) -> WebResult {
    Ok((StatusCode::UNPROCESSABLE_ENTITY, render(ctx, page, view)?).into_response())
}

fn failed<'a>(data: EmbedData, form: &FormData, editing: Option<&'a Sent>, error: String) -> View<'a> {
    View { data, channel: form.id("channel"), template_name: form.string("template_name"), editing, error: Some(error) }
}

fn explain(e: &bot_core::twilight_http::Error) -> String {
    match e.kind() {
        ErrorType::Response { status, .. } if status.get() == 403 => {
            "Discord refused: the bot needs View Channel, Send Messages and Embed Links in that channel.".into()
        }
        ErrorType::Response { status, .. } if status.get() == 404 => "Discord can't find that message or channel. It may have been deleted.".into(),
        ErrorType::Response { status, body, .. } if status.get() == 400 => {
            let detail = bot_core::serde_json::from_slice::<bot_core::serde_json::Value>(body)
                .ok()
                .and_then(|v| v.get("message").and_then(|m| m.as_str().map(str::to_owned)))
                .unwrap_or_default();
            match detail.trim().trim_end_matches('.') {
                "" => "Discord rejected the embed. Check the image and link addresses.".into(),
                d => format!("Discord rejected the embed: {d}. Check the image and link addresses."),
            }
        }
        _ => format!("Discord refused it: {e}"),
    }
}

/// Users and roles in the text can be pinged. `@everyone` and `@here` can't.
fn mentions() -> AllowedMentions {
    AllowedMentions { parse: vec![MentionType::Users, MentionType::Roles], ..Default::default() }
}

async fn send(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let data = EmbedData::from_form(&form);
    if let Err(e) = data.validate() {
        return rerender(&ctx, &page, failed(data, &form, None, e));
    }
    let Some(channel) = form.id("channel").filter(|c| ctx.text_channels(guild).iter().any(|(id, _)| id == c)) else {
        return rerender(&ctx, &page, failed(data, &form, None, "Pick a channel to send to.".into()));
    };
    let embeds: Vec<Embed> = data.embed().into_iter().collect();
    let allowed = mentions();
    let mut req = ctx.http.create_message(Id::new(channel)).embeds(&embeds).allowed_mentions(Some(&allowed));
    if !data.content.is_empty() {
        req = req.content(&data.content);
    }
    let res = match req.await {
        Ok(r) => r,
        Err(e) => return rerender(&ctx, &page, failed(data, &form, None, explain(&e))),
    };
    // The message is out even if its reply can't be read; an error page here invites a second
    // copy. It just can't be listed for editing.
    match res.model().await {
        Ok(message) => {
            let name = form.string("template_name");
            let template = crate::templates(&ctx, guild)?.into_iter().find(|t| t.name == name).map(|t| t.name);
            crate::sent_add(&ctx, guild, channel, message.id.get(), template.as_deref(), &data, &page.user.name)?;
        }
        Err(e) => bot_core::tracing::warn!(guild, channel, error = %e, "sent an embed but couldn't read Discord's reply; it won't be listed"),
    }
    ctx.audit(&page.user, Some(guild), KEY, format!("Sent an embed to #{}", channel_label(&ctx, guild, channel)));
    Ok(saved(&page.path("embeds")))
}

async fn update(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let sent = crate::sent_get(&ctx, guild, id)?.ok_or(WebError::NotFound)?;
    let data = EmbedData::from_form(&form);
    if let Err(e) = data.validate() {
        return rerender(&ctx, &page, failed(data, &form, Some(&sent), e));
    }
    let embeds: Vec<Embed> = data.embed().into_iter().collect();
    let allowed = mentions();
    let content = Some(data.content.as_str()).filter(|c| !c.is_empty());
    let res = ctx
        .http
        .update_message(Id::new(sent.channel_id), Id::new(sent.message_id))
        .content(content)
        .embeds(Some(&embeds))
        .allowed_mentions(Some(&allowed))
        .await;
    if let Err(e) = res {
        return rerender(&ctx, &page, failed(data, &form, Some(&sent), explain(&e)));
    }
    crate::sent_update(&ctx, guild, id, &data)?;
    ctx.audit(&page.user, Some(guild), KEY, format!("Edited an embed in #{}", channel_label(&ctx, guild, sent.channel_id)));
    Ok(saved(&page.path(&format!("embeds?sent={id}"))))
}

fn channel_label(ctx: &Ctx, guild: u64, channel: u64) -> String {
    ctx.text_channels(guild).into_iter().find(|(id, _)| *id == channel).map(|(_, n)| n).unwrap_or_else(|| channel.to_string())
}

async fn forget(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    crate::sent_delete(&ctx, guild, id)?;
    ctx.audit(&page.user, Some(guild), KEY, "Removed a sent embed from the list");
    Ok(saved(&page.path("embeds")))
}

async fn save_template(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let data = EmbedData::from_form(&form);
    let name = text::clip(&form.string("template_name"), 60);
    let problem = if name.is_empty() {
        Some("Name the template first.".to_owned())
    } else if let Err(e) = data.check_limits() {
        Some(e)
    } else if !data.has_embed() && data.content.is_empty() {
        Some("There's nothing to save yet. Add a title, description, field or message text.".to_owned())
    } else {
        None
    };
    if let Some(e) = problem {
        return rerender(&ctx, &page, failed(data, &form, None, e));
    }
    let Some(id) = crate::template_save(&ctx, guild, &name, &data)? else {
        return rerender(&ctx, &page, failed(data, &form, None, "This server has reached 50 templates. Delete one to save another.".into()));
    };
    ctx.audit(&page.user, Some(guild), KEY, format!("Saved the embed template {name}"));
    Ok(saved(&page.path(&format!("embeds?template={id}"))))
}

/// "Rules" -> "Rules (copy)", then "Rules (copy 2)" and on, within the 60-character limit.
fn copy_name(name: &str, taken: &[String]) -> String {
    let base: String = name.chars().take(48).collect();
    (1..)
        .map(|n| if n == 1 { format!("{base} (copy)") } else { format!("{base} (copy {n})") })
        .find(|c| !taken.contains(c))
        .unwrap_or_default()
}

async fn duplicate_template(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let t = crate::template(&ctx, guild, id)?.ok_or(WebError::NotFound)?;
    let taken: Vec<String> = crate::templates(&ctx, guild)?.into_iter().map(|t| t.name).collect();
    let name = copy_name(&t.name, &taken);
    let Some(new) = crate::template_save(&ctx, guild, &name, &t.data)? else {
        return Err(WebError::Bad("This server has reached 50 templates. Delete one to duplicate another.".into()));
    };
    ctx.audit(&page.user, Some(guild), KEY, format!("Duplicated the embed template {} as {name}", t.name));
    Ok(saved(&page.path(&format!("embeds?template={new}"))))
}

async fn delete_template(State(ctx): State<Ctx>, user: User, Path((guild, id)): Path<(u64, i64)>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    if let Some(name) = crate::template_delete(&ctx, guild, id)? {
        ctx.audit(&page.user, Some(guild), KEY, format!("Deleted the embed template {name}"));
    }
    Ok(saved(&page.path("embeds")))
}



const CSS: &str = r##"
/* Saved embeds: a chip each, with the embed's own colour. */
.eb-tpls { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
.eb-tpl-wrap { position: relative; display: inline-flex; min-width: 0; max-width: 100%; }
.eb-tpl { display: inline-flex; align-items: center; gap: 8px; max-width: 100%; height: 32px; padding: 0 14px 0 11px; border-radius: 16px; background: var(--control); color: var(--ink-2); font-size: 13px; white-space: nowrap; transition: background 0.18s, color 0.18s, box-shadow 0.2s, transform 0.25s var(--spring); }
.eb-tpl:hover { background: var(--hover); color: var(--ink); }
.eb-tpl:active { transform: scale(0.96); }
.eb-tpl-name { overflow: hidden; text-overflow: ellipsis; max-width: 24ch; }
.eb-tpl-wrap:has(form) .eb-tpl { padding-right: 32px; }
.eb-tpl.on { background: color-mix(in oklab, var(--hue) 16%, transparent); color: var(--ink); font-weight: 500; box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--hue) 50%, transparent); }
.eb-dot { width: 10px; height: 10px; flex: none; border-radius: 50%; background: var(--c); box-shadow: 0 0 0 2px rgba(0, 0, 0, 0.25); }
.eb-tpl-wrap > form { position: absolute; right: 4px; top: 4px; }
.eb-x { display: inline-grid; place-items: center; height: 24px; min-width: 24px; padding: 0 5px; border: 0; border-radius: 12px; background: transparent; color: var(--ink-3); font: 600 11px var(--sans); cursor: pointer; opacity: 0; transition: opacity 0.15s, background 0.15s, color 0.15s; }
.eb-x .icon { width: 12px; height: 12px; }
.eb-tpl-wrap:hover .eb-x, .eb-x:focus-visible, .eb-x.armed { opacity: 1; }
.eb-x:hover { color: var(--held); background: color-mix(in oklab, var(--held) 14%, transparent); }
.eb-x.armed { padding: 0 9px; background: var(--held); color: #1a0507; }
@media (hover: none) { .eb-x { opacity: 1; } }
.eb-tpl.new { padding: 0 13px 0 10px; background: transparent; box-shadow: inset 0 0 0 1px var(--edge-2); }
.eb-tpl.new:hover { background: var(--hover); }
.eb-tpl.new .icon { width: 14px; height: 14px; }
.eb-saved { display: inline-flex; align-items: center; gap: 10px; margin-left: auto; }

/* The editor on the left, Discord on the right (`.compose`). Once the page is wide, the preview
   keeps Discord's own width (its embeds stop at 516 px) and the editor takes the rest, splitting
   into two columns: the text and embed, then the fields and pictures. */
fieldset.eb { min-width: 0; margin: 0; padding: 0; border: 0; }
/* One block on the left, so the preview needs one row; a taller span leaves empty gaps below. */
@container col (min-width: 1000px) { fieldset.eb.compose > .compose-preview { grid-row: 1; } }
@container col (min-width: 1400px) { fieldset.eb.compose { grid-template-columns: minmax(0, 1fr) 640px; } }
.eb-edit { container: ebedit / inline-size; display: flex; flex-direction: column; gap: 14px; min-width: 0; }
.eb-card { display: flex; flex-direction: column; padding: 2px 22px; border-radius: var(--r-card); background: linear-gradient(180deg, rgba(243, 242, 248, 0.05), rgba(243, 242, 248, 0.02)); border: 1px solid var(--edge-1); box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.04); }
.eb-col { display: flex; flex-direction: column; min-width: 0; }
.eb-col + .eb-col { border-top: 1px solid var(--edge-1); }
@container ebedit (min-width: 780px) {
  .eb-card { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); padding: 2px 0; }
  .eb-col { padding: 0 24px; }
  .eb-col + .eb-col { border-top: 0; border-left: 1px solid var(--edge-1); }
}
.eb-grp { display: flex; flex-direction: column; gap: 14px; padding: 18px 0 20px; }
.eb-grp + .eb-grp { border-top: 1px solid var(--edge-1); }
.eb-grp-head { display: flex; align-items: center; gap: 10px; }
.eb-grp-head h3 { flex: 1; font-size: 14px; font-weight: 600; }
.eb-grp-ic { display: grid; place-items: center; width: 28px; height: 28px; flex: none; border-radius: 9px; color: var(--hue); background: color-mix(in oklab, var(--hue) 14%, transparent); }
.eb-grp-ic .icon { width: 15px; height: 15px; }
.eb-f { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.eb-lab { display: flex; justify-content: space-between; align-items: baseline; gap: 8px; }
.eb-lab label, .eb-f > label, .eb-chan > label { font-size: 12.5px; font-weight: 500; color: var(--ink-2); }
.eb-count { font: 11px var(--mono); color: var(--ink-3); transition: color 0.2s; }
.eb-count.over { color: var(--held); }
.eb-f .help { margin-top: 0; }
.eb-2 { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 12px; }
.eb-colours { display: flex; align-items: center; gap: 9px; flex-wrap: wrap; }
.eb-sw { width: 28px; height: 28px; flex: none; padding: 0; border: 0; border-radius: 50%; background: var(--c); cursor: pointer; box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.14); transition: transform 0.3s var(--spring), box-shadow 0.25s; }
.eb-sw:hover { transform: scale(1.12); }
.eb-sw[aria-pressed="true"] { transform: scale(1.06); box-shadow: 0 0 0 2px var(--page), 0 0 0 4px var(--c); }
.eb-pick { position: relative; display: grid; place-items: center; width: 28px; height: 28px; flex: none; border-radius: 50%; color: var(--ink); cursor: pointer; background: conic-gradient(#ff6b6b, #ffd93d, #6bff95, #6bd5ff, #9d7bff, #ff6bd5, #ff6b6b); transition: transform 0.3s var(--spring); }
.eb-pick:hover { transform: scale(1.12) rotate(30deg); }
.eb-pick::before { content: ""; position: absolute; inset: 3px; border-radius: 50%; background: var(--page); }
.eb-pick .icon { position: relative; width: 12px; height: 12px; }
.eb-pick input { position: absolute; inset: 0; width: 100%; height: 100%; padding: 0; border: 0; opacity: 0; cursor: pointer; }
.eb-hex { width: 104px; height: 30px; margin-left: 4px; }
#eb-rows { display: flex; flex-direction: column; }
#eb-rows:empty { display: none; }
.eb-frow { display: flex; flex-direction: column; gap: 8px; }
.eb-frow + .eb-frow { margin-top: 12px; padding-top: 12px; border-top: 1px solid var(--edge-1); }
.eb-frow.new { animation: rise 0.4s var(--ease) both; }
.eb-frow-top { display: flex; align-items: center; gap: 8px; }
.eb-frow-top .fn-name { flex: 1; min-width: 0; }
.eb-inline { flex: none; height: 32px; }
.eb-frm { flex: none; }
.eb-frm:hover { color: var(--held); background: color-mix(in oklab, var(--held) 12%, transparent); }
.eb-frow textarea.input { min-height: 58px; }
.eb-f textarea.input[rows="2"] { min-height: 64px; }
.eb-note { display: flex; align-items: center; gap: 12px; padding: 10px 12px; border-radius: 14px; font-size: 13px; background: color-mix(in oklab, var(--hue) 9%, transparent); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--hue) 28%, transparent); }
.eb-note p { flex: 1; min-width: 0; color: var(--ink-2); }
.eb-note strong { color: var(--ink); font-weight: 600; }
.eb-note-ic { display: grid; place-items: center; width: 32px; height: 32px; flex: none; border-radius: 10px; color: #0b0b10; background: var(--hue); box-shadow: none; }
.eb-note-ic .icon { width: 16px; height: 16px; }
.eb-error { display: flex; align-items: flex-start; gap: 10px; padding: 11px 14px; border-radius: 12px; font-size: 13px; color: var(--ink); background: color-mix(in oklab, var(--held) 11%, transparent); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--held) 35%, transparent); animation: shake 0.4s var(--ease); }
.eb-error .icon { flex: none; margin-top: 2px; color: var(--held); }
.eb-error[hidden] { display: none; }

/* The preview: a Discord channel with the bot's message in it, in Discord's own colours. */
.eb-side { display: flex; flex-direction: column; gap: 12px; min-width: 0; }
.eb-win { overflow: hidden; border-radius: 16px; background: #313338; box-shadow: 0 30px 70px -30px rgba(0, 0, 0, 0.9), 0 0 0 1px rgba(0, 0, 0, 0.45), inset 0 0 0 1px rgba(255, 255, 255, 0.06); }
.eb-win-bar { display: flex; align-items: center; gap: 8px; height: 48px; padding: 0 16px; border-bottom: 1px solid #26272b; color: #80848e; }
.eb-win-bar .icon { width: 20px; height: 20px; }
.eb-win-bar strong { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: #f2f3f5; font-size: 15px; font-weight: 600; }
.eb-win-bar strong.none { color: #80848e; font-weight: 500; }
.eb-win-tag { padding: 3px 8px; border-radius: 6px; font-size: 11.5px; color: #b5bac1; background: rgba(255, 255, 255, 0.06); }
.eb-win-body { display: flex; flex-direction: column; min-height: 240px; max-height: calc(100vh / var(--z, 1) - 250px); overflow-y: auto; padding: 18px 12px 22px; scrollbar-color: #1a1b1e transparent; }
.dc { display: flex; gap: 16px; min-width: 0; margin-top: auto; padding: 4px 8px; border-radius: 6px; color: #dbdee1; font: 15px/1.375 var(--sans); animation: dc-arrive 0.7s var(--ease) both 0.3s; }
@keyframes dc-arrive { from { opacity: 0; transform: translateY(16px); filter: blur(4px); } }
.dc-av { flex: none; width: 40px; height: 40px; border-radius: 50%; object-fit: cover; }
.dc-av.mark { display: grid; place-items: center; background: linear-gradient(145deg, var(--accent-hi), var(--accent-lo)); }
.dc-main { flex: 1; min-width: 0; }
.dc-head { display: flex; align-items: center; flex-wrap: wrap; gap: 6px; line-height: 22px; }
.dc-name { color: #f2f3f5; font-weight: 500; font-size: 16px; }
.dc-tag { height: 15px; padding: 0 4.5px; border-radius: 4px; background: #5865f2; color: #fff; font-size: 10px; font-weight: 600; line-height: 15px; }
.dc-time { margin-left: 2px; color: #949ba4; font-size: 12px; }
.dc-content { white-space: pre-wrap; overflow-wrap: break-word; }
.dc-content:empty { display: none; }
@property --eb-c { syntax: "<color>"; inherits: true; initial-value: #1e1f22; }
.dc-slot { --eb-c: #1e1f22; transition: --eb-c 0.5s var(--ease); }
.dc-embed { display: grid; grid-template-columns: minmax(0, 1fr) auto; column-gap: 16px; max-width: 516px; margin-top: 4px; padding: 2px 16px 16px 12px; border-left: 4px solid var(--eb-c); border-radius: 4px; background: #2b2d31; font-size: 14px; line-height: 1.375; transition: box-shadow 0.3s var(--ease); }
.dc-embed.hl-bar, .dc-ghost.hl-bar { box-shadow: none; }
.dc-embed > * { grid-column: 1; min-width: 0; margin-top: 8px; overflow-wrap: break-word; }
.dc-embed > .dc-th { grid-column: 2; grid-row: 1 / span 6; width: 80px; height: 80px; margin-top: 8px; border-radius: 4px; object-fit: cover; }
.dc-author { color: #f2f3f5; font-weight: 600; }
.dc-title { color: #f2f3f5; font-size: 16px; font-weight: 600; }
.dc-desc, .dc-fv { white-space: pre-wrap; }
.dc-fields { display: grid; grid-template-columns: repeat(12, minmax(0, 1fr)); gap: 8px; }
.dc-field { grid-column: span 12; min-width: 0; overflow-wrap: break-word; }
.dc-field.in { grid-column: span 4; }
.dc-embed.th .dc-field.in { grid-column: span 6; }
.dc-field.new { animation: dc-pop 0.5s var(--spring) both; }
@keyframes dc-pop { from { opacity: 0; transform: translateY(6px) scale(0.94); } }
.dc-fn { margin-bottom: 2px; color: #f2f3f5; font-weight: 600; }
.dc-embed > .dc-img { grid-column: 1 / -1; max-width: 100%; max-height: 300px; margin-top: 16px; border-radius: 4px; object-fit: cover; }
.dc-img, .dc-th { opacity: 0; transform: scale(0.97); transition: opacity 0.5s var(--ease), transform 0.6s var(--ease), box-shadow 0.25s; }
.dc-img.in, .dc-th.in { opacity: 1; transform: none; }
.dc-img.bad, .dc-th.bad { display: none; }
.dc-foot { color: #dbdee1; font-size: 12px; font-weight: 500; }
.dc a { color: #00a8fc; }
.dc code { padding: 0 4px; border-radius: 3px; background: #1e1f22; font: 13px var(--mono); }
.dc pre { margin: 4px 0 0; padding: 8px; border-radius: 4px; background: #1e1f22; border: 1px solid rgba(255, 255, 255, 0.06); font: 13px var(--mono); white-space: pre-wrap; }
.dc-sp { border-radius: 3px; background: #1e1f22; color: transparent; }
.dc-sp:hover { color: inherit; background: rgba(255, 255, 255, 0.06); }
/* The part you're typing in lights in the page's hue, so the form and the post line up. */
.dc [data-p] { border-radius: 3px; transition: background 0.25s, box-shadow 0.25s; }
.dc .hl { background: color-mix(in oklab, var(--hue) 16%, transparent); box-shadow: 0 0 0 3px color-mix(in oklab, var(--hue) 16%, transparent); }
.dc img.hl { background: none; box-shadow: 0 0 0 2px var(--hue); }
.dc-ghost { display: flex; flex-direction: column; gap: 9px; max-width: 420px; margin-top: 6px; padding: 14px 16px 16px 12px; border-left: 4px solid var(--eb-c); border-radius: 4px; background: rgba(43, 45, 49, 0.7); transition: box-shadow 0.3s var(--ease); }
.dc-ghost span { height: 9px; border-radius: 5px; background: rgba(255, 255, 255, 0.06); }
.dc-ghost span:nth-child(1) { width: 42%; height: 11px; background: rgba(255, 255, 255, 0.1); }
.dc-ghost span:nth-child(2) { width: 92%; }
.dc-ghost span:nth-child(3) { width: 68%; }
.dc-empty { margin-top: 10px; color: #949ba4; font-size: 13px; }
.eb-total { display: flex; align-items: center; gap: 12px; padding: 0 4px; color: var(--ink-3); }
.eb-total-bar { flex: 1; height: 4px; overflow: hidden; border-radius: 2px; background: var(--edge-1); }
.eb-total-bar > span { display: block; height: 100%; border-radius: 2px; background: linear-gradient(90deg, var(--data-2), var(--data)); transform: scaleX(0); transform-origin: left; transition: transform 0.45s var(--ease), background 0.3s; }
.eb-total.warn .eb-total-bar > span { background: var(--waiting); }
.eb-total.over .eb-total-bar > span { background: var(--held); }
.eb-total.over #eb-total { color: var(--held); }
#eb-total { font: 11.5px var(--mono); white-space: nowrap; }
.eb-send { display: flex; align-items: flex-end; flex-wrap: wrap; gap: 10px; padding: 14px; border-radius: 16px; background: linear-gradient(180deg, rgba(243, 242, 248, 0.05), rgba(243, 242, 248, 0.02)); box-shadow: inset 0 0 0 1px var(--edge-1); }
.eb-chan { display: flex; flex: 1; flex-direction: column; gap: 6px; min-width: 160px; }
.eb-btns { display: flex; align-items: center; flex-wrap: wrap; gap: 8px; }
.eb-btns .btn { height: 36px; }
.eb-savetpl .pop-body { width: 290px; }
.eb-savetpl .pop-body .eb-btns { justify-content: flex-end; }

/* Sent from here: a card each, a small copy of its embed on top, where and who under it, and
   its actions in its own footer. More cards to a row as the page widens. */
.eb-sent { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 400px), 1fr)); gap: 12px; }
.eb-srow { display: flex; flex-direction: column; gap: 12px; min-width: 0; padding: 12px; border-radius: 16px; background: linear-gradient(180deg, rgba(243, 242, 248, 0.045), rgba(243, 242, 248, 0.015)); box-shadow: inset 0 0 0 1px var(--edge-1); transition: transform 0.35s var(--ease), box-shadow 0.25s; }
.eb-srow:hover { transform: translateY(-2px); box-shadow: inset 0 0 0 1px var(--edge-2), var(--shadow-lift); }
.eb-srow.cur { background: color-mix(in oklab, var(--hue) 8%, transparent); box-shadow: inset 0 0 0 1px color-mix(in oklab, var(--hue) 45%, transparent); }
.eb-mini { display: flex; align-items: center; gap: 12px; min-width: 0; padding: 8px 10px 8px 12px; border-left: 3px solid var(--c); border-radius: 5px; background: #2b2d31; }
.eb-mini-text { display: flex; flex: 1; flex-direction: column; gap: 1px; min-width: 0; }
.eb-mini-text strong { color: #f2f3f5; font-size: 13px; font-weight: 600; }
.eb-mini-text span { color: #b5bac1; font-size: 12px; }
.eb-mini-pic { width: 56px; height: 36px; flex: none; border-radius: 4px; object-fit: cover; }
.eb-smeta { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 12px; min-width: 0; padding: 0 2px; font-size: 12.5px; color: var(--ink-2); }
.eb-chan-chip { display: inline-flex; align-items: center; gap: 3px; height: 22px; padding: 0 8px 0 6px; border-radius: 6px; font-size: 12px; font-weight: 500; color: var(--ink); background: color-mix(in oklab, var(--hue) 13%, transparent); }
.eb-chan-chip .icon { width: 12px; height: 12px; color: var(--hue); }
.eb-by { display: flex; align-items: center; gap: 7px; min-width: 0; }
.eb-by .avatar { width: 20px; height: 20px; }
.eb-sacts { display: flex; align-items: center; justify-content: flex-end; gap: 6px; margin-top: auto; padding-top: 10px; border-top: 1px solid var(--edge-1); }
/* Stacked under the editor, the preview grows with its post instead of scrolling. */
@container col (max-width: 999px) { .eb-win-body { max-height: none; } }
@media (max-width: 560px) {
  .eb-2 { grid-template-columns: minmax(0, 1fr); }
  .eb-card { padding: 2px 16px; }
  .eb-saved { margin-left: 0; }
  .eb-send { flex-direction: column; align-items: stretch; }
  .eb-btns .btn, .eb-savetpl { flex: 1 1 auto; }
  .eb-savetpl > summary { width: 100%; }
  .eb-win-body { padding: 14px 6px 18px; }
  .dc { gap: 12px; }
  .dc-av { width: 36px; height: 36px; }
  .dc-embed > .dc-th { width: 64px; height: 64px; }
  .eb-sacts .m-hide { display: none; }
}
"##;

const JS: &str = r##"
(() => {
  const f = document.getElementById("eb-form");
  if (!f) return;
  const $ = (id) => document.getElementById(id);
  const MAX_ROWS = +f.dataset.maxFields || 25, LIMIT = 6000;
  // Discord's limits, worded like the server's own checks.
  const LIMITS = [["title", 256, "The title"], ["description", 4096, "The description"], ["content", 2000, "The message text"], ["author", 256, "The author name"], ["footer", 2048, "The footer"]];
  const esc = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  const md = (s) => esc(s)
    .replace(/```(?:\w+\n)?([\s\S]*?)```/g, "<pre>$1</pre>")
    .replace(/`([^`\n]+)`/g, "<code>$1</code>")
    .replace(/\*\*([^*\n]+)\*\*/g, "<b>$1</b>")
    .replace(/__([^_\n]+)__/g, "<u>$1</u>")
    .replace(/(^|[^*])\*([^*\n]+)\*/g, "$1<i>$2</i>")
    .replace(/~~([^~\n]+)~~/g, "<s>$1</s>")
    .replace(/\|\|([^|\n]+)\|\|/g, '<span class="dc-sp">$1</span>')
    .replace(/\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)/g, '<a href="$2" target="_blank" rel="noreferrer noopener">$1</a>');
  const val = (n) => (f.elements[n] ? f.elements[n].value.trim() : "");
  const web = (s) => (/^https?:\/\/\S+$/.test(s) ? s : "");
  const len = (s) => [...s].length;
  const rows = $("eb-rows"), slot = $("p-embed"), content = $("p-content"), limit = $("eb-limit");
  const swatches = [...f.querySelectorAll("[data-swatch]")];
  if (!rows || !slot || !content || !limit) return;

  const fieldRows = () => [...rows.querySelectorAll(".eb-frow")].map((r, i) => ({
    i,
    n: r.querySelector(".fn-name").value.trim(),
    v: r.querySelector(".fn-value").value.trim(),
    inline: r.querySelector(".fn-inline").checked,
  })).filter((x) => x.n || x.v);

  const over = (what, n, max) => what + " is " + n.toLocaleString("en-US") + " characters. Discord allows " + max.toLocaleString("en-US") + ".";
  // The first thing Discord would refuse. Half-filled fields only count when sending, so a row
  // being typed doesn't flag.
  function problem(sending) {
    for (const [name, max, what] of LIMITS) {
      const n = len(val(name));
      if (n > max) return over(what, n, max);
    }
    const fields = fieldRows();
    if (fields.length > MAX_ROWS) return "There are " + fields.length + " fields. Discord allows " + MAX_ROWS + ".";
    for (const [i, x] of fields.entries()) {
      if (sending && (!x.n || !x.v)) return "Field " + (i + 1) + " needs both a name and a value.";
      if (len(x.n) > 256) return over("The name of field " + (i + 1), len(x.n), 256);
      if (len(x.v) > 1024) return over("The value of field " + (i + 1), len(x.v), 1024);
    }
    const total = ["title", "description", "author", "footer"].reduce((a, n) => a + len(val(n)), 0) + fields.reduce((a, x) => a + len(x.n) + len(x.v), 0);
    if (total > LIMIT) return "The embed is " + total.toLocaleString("en-US") + " characters in total. Discord allows 6,000 across the title, description, author, footer and fields.";
    return "";
  }
  const show = (msg) => { limit.textContent = msg; limit.hidden = !msg; };

  // Pictures keep their element between renders, so typing elsewhere doesn't reload them, and
  // each fades in once it has loaded.
  const pics = new Map();
  const pic = (url, cls) => {
    const key = cls + " " + url;
    let el = pics.get(key);
    if (!el) {
      el = new Image();
      el.alt = "";
      el.className = cls;
      el.decoding = "async";
      el.addEventListener("load", () => el.classList.add("in"));
      el.addEventListener("error", () => el.classList.add("bad"));
      el.src = url;
      pics.set(key, el);
    }
    return el;
  };

  // The part of the post you're typing in, lit in the preview.
  let part = "";
  const light = () => {
    document.querySelectorAll("#eb-msg [data-p]").forEach((el) => el.classList.toggle("hl", !!part && el.dataset.p === part));
    const box = slot.querySelector(".dc-embed, .dc-ghost");
    if (box) box.classList.toggle("hl-bar", part === "bar");
  };

  let shownFields = -1;
  function render() {
    const title = val("title"), desc = val("description"), author = val("author"), footer = val("footer");
    const image = web(val("image")), thumb = web(val("thumbnail")), link = web(val("url"));
    const fields = fieldRows();
    content.innerHTML = md(val("content"));
    const has = title || desc || author || footer || image || thumb || fields.length;
    const hex = /^#?[0-9a-f]{6}$/i.test(val("color")) ? "#" + val("color").replace("#", "").toLowerCase() : "";
    slot.style.setProperty("--eb-c", hex || "#1e1f22");
    let h = "";
    if (has) {
      h += '<div class="dc-embed' + (thumb ? " th" : "") + '">';
      if (thumb) h += '<i data-pic="th"></i>';
      if (author) h += '<div class="dc-author" data-p="author">' + esc(author) + "</div>";
      if (title) h += '<div class="dc-title" data-p="title">' + (link ? '<a href="' + esc(link) + '" target="_blank" rel="noreferrer noopener">' + esc(title) + "</a>" : esc(title)) + "</div>";
      if (desc) h += '<div class="dc-desc" data-p="desc">' + md(desc) + "</div>";
      if (fields.length) {
        h += '<div class="dc-fields">';
        for (const x of fields) h += '<div class="dc-field' + (x.inline ? " in" : "") + '" data-p="f' + x.i + '"><div class="dc-fn">' + md(x.n) + '</div><div class="dc-fv">' + md(x.v) + "</div></div>";
        h += "</div>";
      }
      if (image) h += '<i data-pic="img"></i>';
      if (footer) h += '<div class="dc-foot" data-p="foot">' + esc(footer) + "</div>";
      h += "</div>";
    } else if (!val("content")) {
      h = '<div class="dc-ghost"><span></span><span></span><span></span></div><p class="dc-empty">Nothing to show yet. Start with a title or some text.</p>';
    }
    slot.innerHTML = h;
    slot.querySelectorAll("i[data-pic]").forEach((ph) => {
      const th = ph.dataset.pic === "th";
      const el = pic(th ? thumb : image, th ? "dc-th" : "dc-img");
      el.dataset.p = ph.dataset.pic;
      ph.replaceWith(el);
    });
    // A field that just arrived pops in.
    const shown = slot.querySelectorAll(".dc-field");
    if (shownFields >= 0 && shown.length > shownFields) shown[shown.length - 1].classList.add("new");
    shownFields = shown.length;
    light();

    swatches.forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.swatch === hex)));
    f.querySelectorAll("[data-count]").forEach((c) => {
      const el = f.elements[c.dataset.count];
      if (!el) return;
      const n = len(el.value.trim());
      c.textContent = n + " / " + c.dataset.max;
      c.classList.toggle("over", n > +c.dataset.max);
    });
    const total = len(title) + len(desc) + len(author) + len(footer) + fields.reduce((a, x) => a + len(x.n) + len(x.v), 0);
    const t = $("eb-total"), fill = $("eb-total-fill");
    if (t) {
      t.textContent = total.toLocaleString("en-US") + " / " + LIMIT.toLocaleString("en-US") + " characters";
      t.parentElement.classList.toggle("warn", total > LIMIT * 0.9 && total <= LIMIT);
      t.parentElement.classList.toggle("over", total > LIMIT);
    }
    if (fill) fill.style.transform = "scaleX(" + Math.min(1, total / LIMIT) + ")";
    const n = rows.children.length;
    const fc = $("eb-fcount"), add = $("eb-add");
    if (fc) fc.textContent = n + " of " + MAX_ROWS;
    if (add) add.disabled = n >= MAX_ROWS || f.querySelector("fieldset").disabled;
    show(problem(false));
  }

  f.addEventListener("submit", (e) => {
    const msg = problem(true);
    if (msg) {
      e.preventDefault();
      show(msg);
      limit.scrollIntoView({ block: "nearest" });
      return;
    }
    // Rows are read by number on the server; number them in the order they're shown, so a row
    // added after a removed one isn't sent out of place.
    [...rows.children].forEach((r, i) => {
      r.querySelector(".fn-name").name = "field_name_" + i;
      r.querySelector(".fn-value").name = "field_value_" + i;
      r.querySelector(".fn-inline").name = "field_inline_" + i;
    });
  });

  f.addEventListener("input", render);
  f.addEventListener("change", render);
  f.addEventListener("focusin", (e) => {
    const row = e.target.closest(".eb-frow");
    part = row ? "f" + [...rows.children].indexOf(row) : (e.target.closest("[data-part]")?.dataset.part || "");
    light();
  });
  f.addEventListener("focusout", () => { part = ""; requestAnimationFrame(light); });
  // Enter inside a one-line input shouldn't send or save by accident.
  f.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && e.target.tagName === "INPUT" && e.target.type === "text") e.preventDefault();
  });
  $("eb-add")?.addEventListener("click", () => {
    const used = new Set([...rows.children].map((r) => r.dataset.i));
    let i = 0;
    while (used.has(String(i))) i++;
    if (i >= MAX_ROWS) return;
    rows.insertAdjacentHTML("beforeend", $("eb-ftpl").innerHTML.replaceAll("__I__", i));
    rows.lastElementChild.classList.add("new");
    rows.lastElementChild.querySelector(".fn-name").focus();
    render();
  });
  rows.addEventListener("click", (e) => {
    const b = e.target.closest("[data-remove]");
    if (b) { b.closest(".eb-frow").remove(); render(); }
  });
  const swatch = $("eb-swatch"), color = f.elements["color"];
  swatch?.addEventListener("input", () => { color.value = swatch.value; render(); });
  color?.addEventListener("input", () => {
    const v = color.value.trim().replace("#", "");
    if (/^[0-9a-f]{6}$/i.test(v) && swatch) swatch.value = "#" + v.toLowerCase();
  });
  swatches.forEach((b) => b.addEventListener("click", () => {
    color.value = b.dataset.swatch;
    if (swatch) swatch.value = b.dataset.swatch;
    render();
  }));
  // The channel in the preview's header follows the one you pick.
  const chan = f.elements["channel"], chanName = $("eb-chan-name");
  const showChan = () => {
    if (!chan || !chanName) return;
    const o = chan.options[chan.selectedIndex];
    const named = !!(o && o.value);
    chanName.textContent = named ? o.textContent.replace(/^#/, "") : chanName.dataset.none;
    chanName.classList.toggle("none", !named);
  };
  chan?.addEventListener("change", showChan);
  showChan();
  render();
})();
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_get_free_names() {
        assert_eq!(copy_name("Rules", &[]), "Rules (copy)");
        let taken = vec!["Rules (copy)".to_owned(), "Rules (copy 2)".to_owned()];
        assert_eq!(copy_name("Rules", &taken), "Rules (copy 3)");
        assert!(copy_name(&"n".repeat(60), &[]).chars().count() <= 60);
    }
}
