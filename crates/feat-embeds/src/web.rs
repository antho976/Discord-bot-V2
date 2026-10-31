use bot_core::axum::Router;
use bot_core::axum::extract::{Path, Query, State};
use bot_core::axum::http::StatusCode;
use bot_core::axum::response::IntoResponse;
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::twilight_http::error::ErrorType;
use bot_core::twilight_model::channel::message::{AllowedMentions, Embed, MentionType};
use bot_core::twilight_model::id::Id;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, text};
use serde::Deserialize;

use crate::model::{self, EmbedData, FORM_FIELDS, Field};
use crate::{KEY, Sent};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/embeds", get(compose))
        .route("/g/{guild}/embeds/send", post(send))
        .route("/g/{guild}/embeds/sent/{id}/update", post(update))
        .route("/g/{guild}/embeds/sent/{id}/delete", post(forget))
        .route("/g/{guild}/embeds/templates", post(save_template))
        .route("/g/{guild}/embeds/templates/{id}/delete", post(delete_template))
        .route("/g/{guild}/embeds/templates/{id}/duplicate", post(duplicate_template))
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
    let can_edit = page.can_edit();
    let rows: Vec<&Field> = d.fields.iter().take(FORM_FIELDS).collect();
    // The optional parts open on their own when a loaded embed uses any of them.
    let more = !d.content.is_empty() || !d.url.is_empty() || !d.author.is_empty() || !d.footer.is_empty() || !d.thumbnail.is_empty() || !d.fields.is_empty();

    let chips = html! {
        nav.eb-chips aria-label="Saved embeds" {
            @for t in &templates {
                span.eb-chip-wrap {
                    a.eb-chip.on[t.name == v.template_name] href=(page.path(&format!("embeds?template={}", t.id))) title=(t.data.summary()) { (t.name) }
                    @if can_edit {
                        form.inline method="post" action=(page.path(&format!("embeds/templates/{}/delete", t.id))) {
                            button.eb-x type="submit" data-confirm="Delete?" aria-label={ "Delete " (t.name) } title="Delete this template" { (ui::icon(Icon::Close)) }
                        }
                    }
                }
            }
            a.eb-chip.new href=(page.path("embeds")) { (ui::icon(Icon::Plus)) "New" }
            @if let Some(t) = templates.iter().find(|t| t.name == v.template_name && v.editing.is_none()) {
                span.muted.small { "Saved " (text::ago(t.updated_at)) }
                @if can_edit { (ui::action_button(&page.path(&format!("embeds/templates/{}/duplicate", t.id)), "Duplicate", Tone::Muted, false)) }
            }
        }
    };

    let body = html! {
        style { (PreEscaped(CSS)) }
        (chips)
        form #eb-form method="post" action=(page.path("embeds/send")) data-max-fields=(FORM_FIELDS) {
            fieldset.eb disabled[!can_edit] {
                div.eb-card {
                    @if let Some(s) = v.editing {
                        div.eb-note {
                            p {
                                "Editing the message in " strong { (channel_name(s.channel_id)) } ", sent " (text::ago(s.sent_at)) " by " (s.sent_by) ". "
                                "Update changes it in place."
                            }
                            a.btn.small href=(page.path("embeds")) { "New embed" }
                        }
                    }
                    @if let Some(e) = &v.error {
                        div.eb-error role="alert" { (e) }
                    }
                    (text_field("title", "Title", "", &d.title, false, Some(model::MAX_TITLE), 1))
                    (text_field("description", "Description", "Supports **bold**, *italic*, `code`, ~~strike~~ and [links](https://example.com).", &d.description, true, Some(model::MAX_DESCRIPTION), 6))
                    div.eb-f {
                        div.eb-lab { label for="color" { "Colour" } }
                        div.eb-colours {
                            @for c in SWATCHES {
                                button.eb-sw type="button" data-swatch=(c) style={ "--c:" (c) } aria-label={ "Colour " (c) } aria-pressed="false" {}
                            }
                            input.eb-swatch #eb-swatch type="color" value=(color_ok.as_deref().unwrap_or("#2b2d31")) aria-label="Pick any colour";
                            input.input.num.eb-hex #color type="text" name="color" value=(d.color) placeholder="No colour" autocomplete="off" maxlength="7" aria-label="Colour as hex";
                        }
                    }
                    div.eb-f {
                        div.eb-lab { label for="image" { "Image" } }
                        input.input #image type="text" name="image" value=(d.image) placeholder="https://cdn.discordapp.com/.../banner.png" autocomplete="off";
                    }
                    details.eb-more open[more] {
                        summary { "More options" (ui::icon(Icon::Chevron)) }
                        div.eb-more-body {
                            (text_field("content", "Message text", "Shown above the embed. Optional.", &d.content, true, Some(model::MAX_CONTENT), 3))
                            (text_field("url", "Title link", "", &d.url, false, None, 1))
                            div.eb-2 {
                                (text_field("author", "Author", "", &d.author, false, Some(model::MAX_AUTHOR), 1))
                                (text_field("footer", "Footer", "", &d.footer, false, Some(model::MAX_FOOTER), 1))
                            }
                            (text_field("thumbnail", "Thumbnail address", "", &d.thumbnail, false, None, 1))
                            div.eb-fblock {
                                div.eb-fhead {
                                    h3 { "Fields" }
                                    span.muted.small #eb-fcount {}
                                }
                                div #eb-rows {
                                    @for (i, f) in rows.iter().enumerate() { (field_row(&i.to_string(), f)) }
                                }
                                div { button.btn.small #eb-add type="button" { (ui::icon(Icon::Plus)) "Add field" } }
                                template #eb-ftpl { (field_row("__I__", &Field::default())) }
                            }
                        }
                    }
                    div.eb-error #eb-limit role="alert" hidden {}
                    @if can_edit {
                        div.eb-foot {
                            div.eb-chan {
                                label for="channel" { @if v.editing.is_some() { "Send as new to" } @else { "Channel" } }
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
                aside.eb-side {
                    span.muted.small { "Preview" }
                    div.dc {
                        @if let Some(url) = &bot_avatar {
                            img.dc-av src=(url) alt="" width="40" height="40" loading="lazy";
                        } @else {
                            span.dc-av.letter { (bot_name.chars().next().unwrap_or('B')) }
                        }
                        div.dc-main {
                            div.dc-head { b.dc-name { (bot_name) } span.dc-tag { "APP" } span.dc-time { "Today" } }
                            div.dc-content #p-content {}
                            div #p-embed {}
                        }
                    }
                    span.muted.small.eb-total #eb-total {}
                }
            }
        }
        (ui::section("Sent", Some("Embeds posted from here. Edit one to change the message in Discord."), html! {
            @if sent.is_empty() {
                (ui::empty(Icon::Log, "Nothing sent yet", "Messages you send from the composer are listed here so you can edit them later."))
            } @else {
                div.table-wrap { table.table.eb-tbl {
                    thead { tr { th { "Channel" } th { "Message" } th.m-hide { "Sent" } th {} } }
                    tbody { @for s in &sent {
                        @let current = v.editing.is_some_and(|e| e.id == s.id);
                        tr.eb-cur[current] {
                            td { (channel_name(s.channel_id)) }
                            td { span.clip.cell { (s.summary) } @if let Some(t) = &s.template { " " span.muted.small { "from " (t) } } }
                            td.muted.m-hide { (text::ago(s.edited_at.unwrap_or(s.sent_at))) @if s.edited_at.is_some() { " (edited)" } }
                            td.actions {
                                @if current {
                                    span.muted.small { "Editing" }
                                } @else {
                                    a.btn.small href=(page.path(&format!("embeds?sent={}", s.id))) { "Edit" }
                                }
                                @if can_edit { a.btn.small href=(page.path(&format!("embeds?sent={}&copy=1", s.id))) title="Start a new embed from this one" { "Duplicate" } }
                                a.btn.small href={ "https://discord.com/channels/" (guild) "/" (s.channel_id) "/" (s.message_id) } target="_blank" rel="noreferrer noopener" { "Open" }
                                @if can_edit { (ui::action_button(&page.path(&format!("embeds/sent/{}/delete", s.id)), "Forget", Tone::Bad, true)) }
                            }
                        }
                    } }
                } }
            }
        }))
        script { (PreEscaped(JS)) }
    };
    Ok(ui::layout(ctx, page, "Embed builder", None, body))
}

/// A labelled input or textarea with a live character count when it has a limit.
fn text_field(name: &str, label: &str, help: &str, value: &str, long: bool, max: Option<usize>, rows: u8) -> Markup {
    html! {
        div.eb-f {
            div.eb-lab {
                label for=(name) { (label) }
                @if let Some(m) = max { span.eb-count data-count=(name) data-max=(m) {} }
            }
            @if long {
                textarea.input #(name) name=(name) rows=(rows) { (value) }
            } @else {
                input.input #(name) type="text" name=(name) value=(value) autocomplete="off";
            }
            @if !help.is_empty() { p.help { (help) } }
        }
    }
}

fn field_row(i: &str, f: &Field) -> Markup {
    html! {
        div.eb-frow data-i=(i) {
            input.input.fn-name type="text" name={ "field_name_" (i) } value=(f.name) placeholder="Field name" aria-label="Field name" autocomplete="off";
            textarea.input.fn-value name={ "field_value_" (i) } rows="2" placeholder="Field value" aria-label="Field value" { (f.value) }
            div.eb-fbar {
                label.eb-check { input.fn-inline type="checkbox" name={ "field_inline_" (i) } checked[f.inline]; span { "Inline" } }
                button.btn.small type="button" data-remove="1" { "Remove" }
            }
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
.page-body:has(.eb-chips) { max-width: 1180px; }
.eb-chips { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
.eb-chip-wrap { display: inline-flex; align-items: center; gap: 2px; }
.eb-chip { display: inline-flex; align-items: center; gap: 6px; height: 30px; padding: 0 13px; border-radius: 15px; background: var(--control); color: var(--ink-2); font-size: 13px; white-space: nowrap; }
a.eb-chip:not(.new) { display: inline-block; line-height: 30px; max-width: 24ch; overflow: hidden; text-overflow: ellipsis; }
.eb-chip-wrap { min-width: 0; max-width: 100%; }
.eb-chip:hover { background: var(--hover); color: var(--ink); }
.eb-chip.on { background: var(--ink); color: var(--ground); font-weight: 500; }
.eb-chip.new { background: transparent; box-shadow: inset 0 0 0 1px var(--edge-2); }
.eb-chip.new .icon { width: 14px; height: 14px; }
.eb-x { display: inline-grid; place-items: center; height: 22px; min-width: 22px; padding: 0 4px; border: 0; border-radius: 11px; background: transparent; color: var(--ink-3); cursor: pointer; opacity: 0; transition: opacity .15s; font: 500 11px var(--sans); }
.eb-x .icon { width: 12px; height: 12px; }
.eb-x:hover, .eb-x:focus-visible, .eb-x.armed { opacity: 1; color: var(--held); }
.eb-x.armed { background: rgba(236, 106, 94, 0.14); padding: 0 8px; }
.eb-chip-wrap:hover .eb-x { opacity: 1; }
@media (hover: none) { .eb-x { opacity: 1; } }

fieldset.eb { border: 0; margin: 0; padding: 0; min-width: 0; display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 32px; align-items: start; }
.eb-card { display: flex; flex-direction: column; gap: 16px; min-width: 0; padding: 20px; border-radius: var(--r-card); background: var(--veil); border: 1px solid var(--edge-1); }
.eb-f { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.eb-lab { display: flex; justify-content: space-between; align-items: baseline; gap: 8px; }
.eb-lab label, .eb-f > label, .eb-chan > label { font-size: 12.5px; font-weight: 500; color: var(--ink-2); }
.eb-count { font: 11px var(--mono); color: var(--ink-3); }
.eb-count.over { color: var(--held); }
.eb-f .help { margin-top: 0; }
.eb-2 { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 12px; }
.eb-colours { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.eb-sw { width: 26px; height: 26px; padding: 0; border: 0; border-radius: 50%; background: var(--c); cursor: pointer; flex: none; }
.eb-sw:hover { box-shadow: 0 0 0 2px var(--page), 0 0 0 3px var(--edge-3); }
.eb-sw[aria-pressed="true"] { box-shadow: 0 0 0 2px var(--page), 0 0 0 3.5px var(--ink-2); }
.eb-swatch { width: 26px; height: 26px; padding: 0; border: 1px solid var(--edge-2); border-radius: 50%; background: var(--well); cursor: pointer; flex: none; overflow: hidden; }
.eb-hex { width: 112px; height: 28px; margin-left: 6px; }
.eb-fhead { display: flex; align-items: baseline; justify-content: space-between; gap: 8px; }
.eb-fhead h3 { font-size: 13px; font-weight: 500; }
.eb-fblock { display: flex; flex-direction: column; gap: 10px; }
.eb-more { border-top: 1px solid var(--edge-1); padding-top: 14px; }
.eb-more > summary { list-style: none; cursor: pointer; display: inline-flex; align-items: center; gap: 6px; font-size: 13px; color: var(--ink-2); }
.eb-more > summary::-webkit-details-marker { display: none; }
.eb-more > summary:hover { color: var(--ink); }
.eb-more > summary .icon { color: var(--ink-3); transition: transform .15s; }
.eb-more[open] > summary .icon { transform: rotate(180deg); }
.eb-more-body { display: flex; flex-direction: column; gap: 16px; padding-top: 16px; }
.eb-frow { display: flex; flex-direction: column; gap: 8px; padding: 14px 0; border-top: 1px solid var(--edge-1); }
.eb-frow:first-child { border-top: 0; padding-top: 0; }
.eb-frow textarea.input { min-height: 58px; }
.eb-check input { accent-color: var(--signal); }
.eb-fbar { display: flex; justify-content: space-between; align-items: center; }
.eb-check { display: inline-flex; align-items: center; gap: 8px; font-size: 12.5px; color: var(--ink-2); cursor: pointer; }
.eb-note { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 10px 14px; border-radius: var(--r-key); background: var(--well); border: 1px solid var(--edge-1); font-size: 13px; }
.eb-note strong { font-weight: 500; }
.eb-error { padding: 10px 14px; border-radius: var(--r-key); background: rgba(236, 106, 94, 0.1); border: 1px solid rgba(236, 106, 94, 0.35); color: var(--ink); font-size: 13px; }
.eb-foot { display: flex; align-items: flex-end; justify-content: space-between; flex-wrap: wrap; gap: 12px; padding-top: 16px; border-top: 1px solid var(--edge-1); }
.eb-chan { display: flex; flex-direction: column; gap: 6px; min-width: 160px; flex: 1; max-width: 220px; }
.eb-btns { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
.eb-savetpl .pop-body { width: 290px; }
.eb-savetpl .pop-body .eb-btns { justify-content: flex-end; }
.eb-side { position: sticky; top: 16px; display: flex; flex-direction: column; gap: 12px; min-width: 0; }
.eb-total { align-self: flex-start; }
.eb-total.over { color: var(--held); }
td .cell { display: inline-block; max-width: 38ch; vertical-align: bottom; }
.eb-tbl tr.eb-cur td { background: var(--selected); }
.eb-tbl td.actions .muted { display: inline-block; padding: 0 10px; }

.dc { display: flex; gap: 14px; color: #dbdee1; font: 15px/1.4 var(--sans); min-width: 0; }
.dc-av { flex: none; width: 40px; height: 40px; border-radius: 50%; object-fit: cover; }
.dc-av.letter { background: var(--signal); color: var(--signal-ink); display: grid; place-items: center; font-weight: 600; font-size: 17px; text-transform: uppercase; }
.dc-main { min-width: 0; flex: 1; }
.dc-head { display: flex; align-items: center; flex-wrap: wrap; gap: 8px; }
.dc-name { color: #f2f3f5; font-weight: 500; }
.dc-tag { background: #5865f2; color: #fff; font-size: 10px; font-weight: 600; border-radius: 3px; padding: 1px 4px; line-height: 12px; }
.dc-time { color: #949ba4; font-size: 12px; }
.dc-content { white-space: pre-wrap; overflow-wrap: anywhere; }
.dc-content:empty { display: none; }
.dc-embed { margin-top: 6px; display: grid; grid-template-columns: minmax(0, 1fr) auto; column-gap: 16px; max-width: 480px; background: #2b2d31; border: 1px solid rgba(255, 255, 255, 0.06); border-left: 4px solid var(--c, #1e1f22); border-radius: 4px; padding: 0 16px 16px 12px; font-size: 14px; }
.dc-embed > * { grid-column: 1; min-width: 0; margin-top: 8px; overflow-wrap: anywhere; }
.dc-embed > .dc-th { grid-column: 2; grid-row: 1 / span 4; width: 80px; height: 80px; object-fit: cover; border-radius: 4px; }
.dc-author { font-weight: 600; color: #f2f3f5; }
.dc-title { font-size: 16px; font-weight: 600; color: #f2f3f5; }
.dc-title a { color: #00a8fc; }
.dc-desc { white-space: pre-wrap; }
.dc-fields { display: grid; grid-template-columns: repeat(12, minmax(0, 1fr)); gap: 8px; }
.dc-field { grid-column: span 12; }
.dc-field.in { grid-column: span 4; }
.dc-embed.th .dc-field.in { grid-column: span 6; }
.dc-fn { font-weight: 600; color: #f2f3f5; }
.dc-fv { white-space: pre-wrap; }
.dc-embed > .dc-img { margin-top: 16px; max-width: 100%; max-height: 300px; border-radius: 4px; }
.dc-foot { font-size: 12px; color: #b5bac1; }
.dc a { color: #00a8fc; }
.dc code { background: #1e1f22; border-radius: 3px; padding: 0 4px; font: 13px var(--mono); }
.dc pre { margin: 4px 0 0; background: #1e1f22; border: 1px solid rgba(255, 255, 255, 0.06); border-radius: 4px; padding: 8px; font: 13px var(--mono); white-space: pre-wrap; }
.dc-sp { background: #1e1f22; color: transparent; border-radius: 3px; }
.dc-sp:hover { color: inherit; background: rgba(255, 255, 255, 0.06); }
.dc-empty { color: #949ba4; }

@media (max-width: 960px) {
  fieldset.eb { grid-template-columns: minmax(0, 1fr); }
  .eb-side { position: static; }
}
@media (max-width: 520px) {
  .eb-2 { grid-template-columns: minmax(0, 1fr); }
  .eb-hex { margin-left: 0; }
  .eb-card { padding: 16px; }
  .eb-chan { max-width: none; }
  .eb-foot { flex-direction: column; align-items: stretch; }
  .eb-btns .btn { flex: 1; }
  .eb-savetpl { align-self: stretch; }
  .eb-savetpl > summary { width: 100%; }
  .dc { gap: 10px; }
  .eb-tbl .m-hide { display: none; }
  .eb-tbl .cell { white-space: normal; display: inline; max-width: none; }
  .eb-tbl td.actions { display: flex; flex-direction: column; align-items: flex-end; gap: 4px; white-space: normal; }
  .eb-tbl td { padding-left: 8px; }
  .eb-tbl td.actions > * + * { margin-left: 0; }
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
  const rows = $("eb-rows");
  const swatches = [...f.querySelectorAll("[data-swatch]")];

  const fieldRows = () => [...rows.querySelectorAll(".eb-frow")].map((r) => ({
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
  const limit = $("eb-limit");
  const show = (msg) => { limit.textContent = msg; limit.hidden = !msg; };

  function render() {
    const title = val("title"), desc = val("description"), author = val("author"), footer = val("footer");
    const image = web(val("image")), thumb = web(val("thumbnail")), link = web(val("url"));
    const fields = fieldRows();
    $("p-content").innerHTML = md(val("content"));
    const has = title || desc || author || footer || image || thumb || fields.length;
    const hex = /^#?[0-9a-f]{6}$/i.test(val("color")) ? "#" + val("color").replace("#", "").toLowerCase() : "";
    let h = "";
    if (has) {
      h += '<div class="dc-embed' + (thumb ? " th" : "") + '"' + (hex ? ' style="--c:' + hex + '"' : "") + ">";
      if (thumb) h += '<img class="dc-th" alt="" src="' + esc(thumb) + '">';
      if (author) h += '<div class="dc-author">' + esc(author) + "</div>";
      if (title) h += '<div class="dc-title">' + (link ? '<a href="' + esc(link) + '" target="_blank" rel="noreferrer noopener">' + esc(title) + "</a>" : esc(title)) + "</div>";
      if (desc) h += '<div class="dc-desc">' + md(desc) + "</div>";
      if (fields.length) {
        h += '<div class="dc-fields">';
        for (const x of fields) h += '<div class="dc-field' + (x.inline ? " in" : "") + '"><div class="dc-fn">' + md(x.n) + '</div><div class="dc-fv">' + md(x.v) + "</div></div>";
        h += "</div>";
      }
      if (image) h += '<img class="dc-img" alt="" src="' + esc(image) + '">';
      if (footer) h += '<div class="dc-foot">' + esc(footer) + "</div>";
      h += "</div>";
    } else if (!val("content")) {
      h = '<div class="dc-empty">Nothing to show yet.</div>';
    }
    $("p-embed").innerHTML = h;
    $("p-embed").querySelectorAll("img").forEach((i) => i.addEventListener("error", () => { i.style.display = "none"; }));

    swatches.forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.swatch === hex)));
    f.querySelectorAll("[data-count]").forEach((c) => {
      const el = f.elements[c.dataset.count];
      const n = len(el.value.trim());
      c.textContent = n + " / " + c.dataset.max;
      c.classList.toggle("over", n > +c.dataset.max);
    });
    const total = len(title) + len(desc) + len(author) + len(footer) + fields.reduce((a, x) => a + len(x.n) + len(x.v), 0);
    const t = $("eb-total");
    t.textContent = total.toLocaleString("en-US") + " / " + LIMIT.toLocaleString("en-US") + " characters";
    t.classList.toggle("over", total > LIMIT);
    const n = rows.children.length;
    $("eb-fcount").textContent = n + " of " + MAX_ROWS;
    $("eb-add").disabled = n >= MAX_ROWS;
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
  // Enter inside a one-line input shouldn't send or save by accident.
  f.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && e.target.tagName === "INPUT" && e.target.type === "text") e.preventDefault();
  });
  $("eb-add").addEventListener("click", () => {
    const used = new Set([...rows.children].map((r) => r.dataset.i));
    let i = 0;
    while (used.has(String(i))) i++;
    if (i >= MAX_ROWS) return;
    rows.insertAdjacentHTML("beforeend", $("eb-ftpl").innerHTML.replaceAll("__I__", i));
    rows.lastElementChild.querySelector(".fn-name").focus();
    render();
  });
  rows.addEventListener("click", (e) => {
    const b = e.target.closest("[data-remove]");
    if (b) { b.closest(".eb-frow").remove(); render(); }
  });
  const swatch = $("eb-swatch"), color = f.elements["color"];
  swatch.addEventListener("input", () => { color.value = swatch.value; render(); });
  color.addEventListener("input", () => {
    const v = color.value.trim().replace("#", "");
    if (/^[0-9a-f]{6}$/i.test(v)) swatch.value = "#" + v.toLowerCase();
  });
  swatches.forEach((b) => b.addEventListener("click", () => {
    color.value = b.dataset.swatch;
    swatch.value = b.dataset.swatch;
    render();
  }));
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
