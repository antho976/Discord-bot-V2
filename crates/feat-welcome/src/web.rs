use bot_core::axum::Router;
use bot_core::axum::extract::{Path, State};
use bot_core::axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use bot_core::axum::response::{IntoResponse, Redirect};
use bot_core::axum::routing::{get, post};
use bot_core::maud::{Markup, PreEscaped, html};
use bot_core::rusqlite::params;
use bot_core::twilight_model::id::Id;
use bot_core::web::ui::{self, Tone};
use bot_core::web::{FormData, Level, Page, User, WebError, WebResult, saved};
use bot_core::{Ctx, Icon, text};

use crate::{DEFAULT_GOODBYE, DEFAULT_WELCOME, KEY, Settings, card, invites, roles};

pub fn routes() -> Router<Ctx> {
    Router::new()
        .route("/g/{guild}/welcome", get(messages_page).post(save_messages))
        .route("/g/{guild}/welcome/card.png", get(card_png))
        .route("/g/{guild}/welcome/test", post(send_test))
        .route("/g/{guild}/welcome/roles", get(roles_page).post(save_roles))
        .route("/g/{guild}/welcome/invites", get(invites_page).post(save_invites))
}

fn tabs(page: &Page, active: &str) -> Markup {
    ui::tabs(&[("Messages", page.path("welcome")), ("Roles", page.path("welcome/roles")), ("Invites", page.path("welcome/invites"))], active)
}

fn avatar(ctx: &Ctx, user: u64) -> String {
    let hash = ctx.cache.user(Id::new(user)).and_then(|u| u.avatar.map(|a| a.to_string()));
    bot_core::discord::avatar_url(user, hash.as_deref())
}

fn who(ctx: &Ctx, guild: u64, user: u64) -> Markup {
    ui::person(&ctx.display_name(guild, user), &avatar(ctx, user), None)
}

fn channel_name(ctx: &Ctx, guild: u64, id: Option<u64>) -> String {
    id.and_then(|id| ctx.text_channels(guild).into_iter().find(|(c, _)| *c == id)).map(|(_, n)| format!("#{n}")).unwrap_or_else(|| "Off".into())
}

fn role_names(ctx: &Ctx, guild: u64, ids: &[u64]) -> String {
    let known = ctx.roles(guild);
    ids.iter()
        .map(|id| known.iter().find(|(r, _, _)| r == id).map(|(_, n, _)| format!("@{n}")).unwrap_or_else(|| "a deleted role".into()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Sum of a daily counter over the last `days` days.
fn stat_sum(ctx: &Ctx, guild: u64, key: &str, days: i64) -> i64 {
    let since = bot_core::jiff::Timestamp::now()
        .to_zoned(bot_core::jiff::tz::TimeZone::UTC)
        .date()
        .checked_sub(bot_core::jiff::Span::new().days(days))
        .map(|d| d.to_string())
        .unwrap_or_default();
    ctx.db
        .with(|c| c.query_row("SELECT COALESCE(SUM(value), 0) FROM daily_stats WHERE guild_id = ?1 AND key = ?2 AND day > ?3", params![guild as i64, key, since], |r| r.get(0)))
        .unwrap_or(0)
}

const CSS: &str = "
.wl-msg { background: #313338; color: #dbdee1; border-radius: 8px; padding: 12px 14px; max-width: 520px; font-size: 14px; line-height: 1.4; display: flex; flex-direction: column; gap: 8px; overflow-wrap: anywhere; }
.wl-msg.off { opacity: .45; }
.wl-mention { background: rgba(88, 101, 242, .3); color: #c9cdfb; border-radius: 3px; padding: 0 2px; }
.wl-card { display: block; width: 100%; height: auto; max-width: 420px; border-radius: 6px; }
.wl-label { font-size: 11.5px; color: var(--ink-3); margin: 14px 0 6px; }
.wl-label:first-child { margin-top: 0; }
";

const SCRIPT: &str = r#"
(() => {
  const box = document.getElementById("wl-preview");
  if (!box) return;
  const v = { name: box.dataset.name, server: box.dataset.server, count: box.dataset.count };
  const $ = (id) => document.getElementById(id);
  const draw = (el, tpl, mention) => {
    el.textContent = "";
    tpl.split(/(\{user\})/).forEach((part) => {
      if (part === "{user}") {
        if (mention) { const s = document.createElement("span"); s.className = "wl-mention"; s.textContent = "@" + v.name; el.append(s); }
        else el.append(v.name);
      } else el.append(part.replace(/\{(name|server|count)\}/g, (_, k) => v[k]));
    });
  };
  const sync = () => {
    draw($("wl-welcome-text"), $("welcome_text").value.trim() || box.dataset.defaultWelcome, true);
    draw($("wl-goodbye-text"), $("goodbye_text").value.trim() || box.dataset.defaultGoodbye, false);
    $("wl-card").hidden = !$("welcome_card").checked;
    $("wl-goodbye").classList.toggle("off", !$("goodbye_on").checked);
    $("wl-welcome").classList.toggle("off", !$("welcome_on").checked);
  };
  ["welcome_text", "goodbye_text", "welcome_card", "goodbye_on", "welcome_on"].forEach((id) => {
    $(id).addEventListener("input", sync);
    $(id).addEventListener("change", sync);
  });
  sync();
  const url = new URL(location.href);
  if (url.searchParams.has("sent")) {
    window.botToast && window.botToast("Test sent");
    url.searchParams.delete("sent");
    history.replaceState(null, "", url.pathname + url.search + url.hash);
  }
})();
"#;

async fn messages_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let count = ctx.member_count(guild).map(|n| text::num(n as i64)).unwrap_or_else(|| "1".into());

    let body = html! {
        style { (PreEscaped(CSS)) }
        (tabs(&page, "Messages"))
        (ui::readings(&[
            ("welcome posts in", if s.welcome_on { channel_name(&ctx, guild, s.welcome_channel) } else { "Off".into() }),
            ("goodbye posts in", if s.goodbye_on { channel_name(&ctx, guild, s.goodbye_channel) } else { "Off".into() }),
            ("joins, 7 days", text::num(stat_sum(&ctx, guild, "joins", 7))),
            ("leaves, 7 days", text::num(stat_sum(&ctx, guild, "leaves", 7))),
        ]))
        (ui::section("Preview", Some("Shown with your name and this server. Updates as you type."), html! {
            div id="wl-preview" data-name=(user.name) data-server=(ctx.guild_name(guild)) data-count=(count)
                data-default-welcome=(DEFAULT_WELCOME) data-default-goodbye=(DEFAULT_GOODBYE) {
                div.wl-label { "Welcome" }
                div.wl-msg id="wl-welcome" {
                    div id="wl-welcome-text" {}
                    img.wl-card id="wl-card" src=(page.path("welcome/card.png")) alt="The welcome card" width="960" height="320";
                }
                div.wl-label { "Goodbye" }
                div.wl-msg id="wl-goodbye" { div id="wl-goodbye-text" {} }
            }
            @if page.can_edit() {
                div.inline-form style="margin-top:14px" {
                    @if s.welcome_channel.is_some() {
                        form.inline method="post" action=(page.path("welcome/test")) {
                            button.btn type="submit" { "Send a test" }
                        }
                        span.muted.small { "Posts the welcome for you in " (channel_name(&ctx, guild, s.welcome_channel)) ", using the saved settings." }
                    } @else {
                        span.muted.small { "Pick a welcome channel and save to send a test." }
                    }
                }
            }
        }))
        (ui::settings_form(&page, &page.path("welcome"), html! {
            (ui::section("Welcome message", None, ui::card(html! {
                (ui::field_toggle("welcome_on", "Welcome new members", None, s.welcome_on))
                (ui::field_channel(&ctx, guild, "welcome_channel", "Channel", None, s.welcome_channel))
                (ui::field_textarea("welcome_text", "Message", Some("{user} mentions them, {name} is their name, {server} the server and {count} their member number. Only they are mentioned."), &s.welcome_text, DEFAULT_WELCOME))
                (ui::field_toggle("welcome_card", "Attach a welcome card", Some("An image with their avatar, name and member number."), s.welcome_card))
            })))
            (ui::section("Goodbye message", None, ui::card(html! {
                (ui::field_toggle("goodbye_on", "Say goodbye", Some("Posts when someone leaves."), s.goodbye_on))
                (ui::field_channel(&ctx, guild, "goodbye_channel", "Channel", None, s.goodbye_channel))
                (ui::field_textarea("goodbye_text", "Message", Some("{name}, {server} and {count} work here. Nobody is mentioned."), &s.goodbye_text, DEFAULT_GOODBYE))
            })))
        }))
        script { (PreEscaped(SCRIPT)) }
    };
    Ok(ui::layout(&ctx, &page, "Welcome", None, body).into_response())
}

async fn save_messages(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let old = ctx.settings::<Settings>(guild, KEY)?;
    let template = |name: &str, default: &str| {
        let t = form.text(name);
        if t.is_empty() { default.to_owned() } else { text::clip(&t, 1000) }
    };
    let s = Settings {
        welcome_on: form.bool("welcome_on"),
        welcome_channel: form.id("welcome_channel"),
        welcome_text: template("welcome_text", DEFAULT_WELCOME),
        welcome_card: form.bool("welcome_card"),
        goodbye_on: form.bool("goodbye_on"),
        goodbye_channel: form.id("goodbye_channel"),
        goodbye_text: template("goodbye_text", DEFAULT_GOODBYE),
        ..(*old).clone()
    };
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed the welcome and goodbye messages");
    Ok(saved(&page.path("welcome")))
}

/// The welcome card for whoever is signed in, as the page's preview.
async fn card_png(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    ctx.page(&user, guild, KEY, Level::View)?;
    let number = ctx.member_count(guild).map(|n| n as i64);
    let png = card::render(&ctx, &user.name, &ctx.guild_name(guild), number, &user.avatar_url()).await?;
    Ok(([(CONTENT_TYPE, "image/png"), (CACHE_CONTROL, "no-store")], png).into_response())
}

async fn send_test(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    crate::post_welcome(&ctx, &s, guild, user.id, &user.name, &user.avatar_url())
        .await
        .map_err(|e| WebError::Bad(e.to_string()))?;
    ctx.audit(&page.user, Some(guild), KEY, "Sent a test welcome");
    Ok(Redirect::to(&format!("{}?sent=1", page.path("welcome"))).into_response())
}

// ---- roles ----

async fn roles_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let days = s.rejoin_days.max(1);
    let waiting = roles::waiting_count(&ctx, guild, days)?;
    let restored = roles::recently_restored(&ctx, guild, 25)?;
    let stuck: Vec<u64> = s.autoroles.iter().copied().filter(|r| !roles::assignable(&ctx, guild, *r)).collect();

    let body = html! {
        (tabs(&page, "Roles"))
        (ui::readings(&[
            ("auto-roles", s.autoroles.len().to_string()),
            ("members with roles saved", text::num(waiting)),
            ("restored recently", restored.len().to_string()),
        ]))
        @if !stuck.is_empty() {
            p.small { (ui::badge("Can't give", Tone::Warn)) " " (role_names(&ctx, guild, &stuck)) ". These sit above the bot's own role, or the server manages them." }
        }
        (ui::settings_form(&page, &page.path("welcome/roles"), html! {
            (ui::section("Auto-roles", None, ui::card(html! {
                (ui::field_roles(&ctx, guild, "autoroles", "Roles for new members", Some("The bot can only give roles below its own role."), &s.autoroles))
                (ui::field_toggle("autorole_bots", "Include bots", Some("Off: bots that join get no auto-roles."), s.autorole_bots))
            })))
            (ui::section("Roles back on rejoin", None, ui::card(html! {
                (ui::field_toggle("rejoin_on", "Give roles back", Some("A member who leaves and returns gets the roles they left with."), s.rejoin_on))
                (ui::field_number("rejoin_days", "Keep for", Some("After this long, a leaver's saved roles are deleted."), days, 1, 3650, Some("days")))
                (ui::field_roles(&ctx, guild, "rejoin_exclude", "Never give back", Some("Useful for staff or punishment roles."), &s.rejoin_exclude))
            })))
        }))
        (ui::section("Recently restored", Some("Members who came back and got their roles."), html! {
            @if restored.is_empty() {
                (ui::empty(Icon::Users, "Nobody restored yet", "Members who leave and return show up here with the roles they got back."))
            } @else {
                div.table-wrap { table.table {
                    thead { tr { th { "Member" } th { "Roles" } th { "When" } } }
                    tbody { @for r in &restored {
                        tr {
                            td { (who(&ctx, guild, r.user)) }
                            td { (role_names(&ctx, guild, &r.roles)) }
                            td.muted { (text::ago(r.at)) }
                        }
                    } }
                } }
            }
        }))
    };
    Ok(ui::layout(&ctx, &page, "Welcome", None, body).into_response())
}

async fn save_roles(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let old = ctx.settings::<Settings>(guild, KEY)?;
    let s = Settings {
        autoroles: form.ids("autoroles"),
        autorole_bots: form.bool("autorole_bots"),
        rejoin_on: form.bool("rejoin_on"),
        rejoin_days: form.int_in("rejoin_days", 1, 3650, 90),
        rejoin_exclude: form.ids("rejoin_exclude"),
        ..(*old).clone()
    };
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed auto-roles and rejoin roles");
    Ok(saved(&page.path("welcome/roles")))
}

// ---- invites ----

async fn invites_page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;
    let s = ctx.settings::<Settings>(guild, KEY)?;
    let now = bot_core::now_ms();
    let month = now - 30 * 86_400_000;
    let (joins, known) = invites::join_counts(&ctx, guild, month)?;
    let top = invites::top_inviters(&ctx, guild, month, 15)?;
    let recent = invites::recent_joins(&ctx, guild, 25)?;
    let state = invites::state(&ctx, guild)?;

    let status = if !s.track_invites {
        html! { (ui::badge("Off", Tone::Muted)) " " span.muted.small { "Invites aren't being read. Joins are still listed, without who invited them." } }
    } else {
        match &state {
            None => html! { (ui::badge("Not read yet", Tone::Muted)) " " span.muted.small { "The bot reads the invites soon after it starts, then every hour." } },
            Some(st) if st.ok => html! { (ui::badge("Reading invites", Tone::Ok)) " " span.muted.small { (st.codes) " invites, last read " (text::ago(st.checked_at)) "." } },
            Some(st) => html! {
                (ui::badge("Can't read invites", Tone::Bad)) " "
                span.small { (st.error.as_deref().unwrap_or("Discord didn't say why.")) }
                " " span.muted.small { "Tried " (text::ago(st.checked_at)) ". It tries again every hour." }
            },
        }
    };

    let body = html! {
        (tabs(&page, "Invites"))
        (ui::readings(&[
            ("joins, 30 days", text::num(joins)),
            ("with a known inviter", if joins > 0 { format!("{}%", known * 100 / joins) } else { "-".into() }),
            ("invites watched", state.as_ref().map(|s| s.codes.to_string()).unwrap_or_else(|| "0".into())),
        ]))
        p { (status) }
        (ui::settings_form(&page, &page.path("welcome/invites"), ui::card(html! {
            (ui::field_toggle("track_invites", "Track invites", Some("Works out which invite each new member used. The bot needs the Manage Server permission to read invites."), s.track_invites))
        })))
        (ui::section("Top inviters", Some("Members whose invites brought people in."), html! {
            @if top.is_empty() {
                (ui::empty(Icon::Users, "No invited joins yet", "Joins through a member's invite are counted here once tracking has taken its first reading."))
            } @else {
                div.table-wrap { table.table {
                    thead { tr { th { "Member" } th.num { "Last 30 days" } th.num { "All time" } } }
                    tbody { @for (inviter, recent, all) in &top {
                        tr { td { (who(&ctx, guild, *inviter)) } td.num { (recent) } td.num { (all) } }
                    } }
                } }
            }
        }))
        (ui::section("Recent joins", None, html! {
            @if recent.is_empty() {
                (ui::empty(Icon::Door, "No joins recorded", "New members appear here with the invite they came through."))
            } @else {
                div.table-wrap { table.table {
                    thead { tr { th { "Member" } th { "Invited by" } th { "Invite" } th { "Joined" } } }
                    tbody { @for j in &recent {
                        tr {
                            td { (who(&ctx, guild, j.user)) }
                            td { @if let Some(i) = j.inviter { (who(&ctx, guild, i)) } @else { span.muted { "Unknown" } } }
                            td { @if let Some(c) = &j.code { code { (c) } } @else { span.muted { "-" } } }
                            td.muted { (text::ago(j.at)) }
                        }
                    } }
                } }
            }
        }))
    };
    Ok(ui::layout(&ctx, &page, "Welcome", None, body).into_response())
}

async fn save_invites(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>, form: FormData) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::Edit)?;
    let old = ctx.settings::<Settings>(guild, KEY)?;
    let s = Settings { track_invites: form.bool("track_invites"), ..(*old).clone() };
    ctx.save_settings(guild, KEY, &s)?;
    ctx.audit(&page.user, Some(guild), KEY, "Changed invite tracking");
    Ok(saved(&page.path("welcome/invites")))
}
