//! Idleon companion collection, pet giveaways and public pet pages.
//!
//! Members submit pets they gave the server with `/pet add`, the pet or its Pet+ (the upgraded
//! version); staff approve them on the dashboard and they join the collection. `/pet mine` shows
//! a member their own pets and submissions with their status, and lets them remove their own
//! (withdraw a waiting submission, or take their copies out) without asking staff. Moderators record pet giveaways (giver to winner, with an
//! expiry), the bot DMs both, warns 15 minutes before expiry and cancels on expiry, and an editor
//! confirms the hand-over. Three public pages show the collection, giveaways and stats.

mod idleon;
mod picture;
mod public;
pub mod store;
mod view;
mod web;

use bot_core::anyhow::Result;
use bot_core::discord::{CommandCall, ComponentCall, Reply, button_row};
use bot_core::twilight_model::application::command::{Command, CommandType};
use bot_core::twilight_model::channel::message::component::{ActionRow, ButtonStyle, Component, SelectMenu, SelectMenuOption, SelectMenuType};
use bot_core::twilight_model::channel::message::{AllowedMentions, Embed};
use bot_core::twilight_model::guild::Permissions;
use bot_core::twilight_model::id::Id;
use bot_core::twilight_util::builder::command::{BooleanBuilder, CommandBuilder, IntegerBuilder, StringBuilder, SubCommandBuilder};
use bot_core::twilight_util::builder::embed::{EmbedBuilder, EmbedFieldBuilder, EmbedFooterBuilder, ImageSource};
use bot_core::{Ctx, Feature, FeatureMeta, Group, Happening, Icon, Migration, NavItem, Need, Overview, feature_async, now_ms, text};
use serde::{Deserialize, Serialize};

use store::{Giveaway, MyCopies, Pending, Pet, Status};

pub const KEY: &str = "pets";
const COLOR: u32 = 0xf0806a;
/// A `/pet add` or `/pet mine` confirmation button stops working after this long.
const CONFIRM_SECS: i64 = 15 * 60;
const WARN_BEFORE_MS: i64 = 15 * 60 * 1000;
const DAY_MS: i64 = 24 * 3_600_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Confirmed giveaways are posted here, mentioning the giver and the winner.
    pub announce_channel: Option<u64>,
    /// Default time a giveaway stays open, in hours. 0 means no expiry.
    pub expiry_hours: i64,
    /// DM members when their submission is approved or rejected.
    pub dm_reviews: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { announce_channel: None, expiry_hours: 24, dm_reviews: true }
    }
}

#[derive(Default)]
pub struct Pets;

#[feature_async]
impl Feature for Pets {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Pets",
            description: "Idleon companion collection, pet giveaways and public pet pages.",
            group: Group::Pets,
            nav: vec![NavItem { label: "Pets", path: KEY, icon: Icon::Paw, about: "The server's Idleon companion collection." }],
        }
    }

    fn migrations(&self) -> &'static [Migration] {
        store::MIGRATIONS
    }

    fn commands(&self) -> Vec<Command> {
        let pet_opt = |desc: &str| StringBuilder::new("pet", desc).required(true).autocomplete(true);
        let qty_opt = |desc: &str| IntegerBuilder::new("quantity", desc).min_value(1).max_value(25);
        vec![
            CommandBuilder::new("pet", "The server's Idleon pet collection", CommandType::ChatInput)
                .option(
                    SubCommandBuilder::new("add", "Submit a pet you gave the server, for staff to approve")
                        .option(pet_opt("Which pet"))
                        .option(qty_opt("How many, 1 if left out"))
                        .option(BooleanBuilder::new("plus", "Its Pet+, the upgraded version")),
                )
                .option(SubCommandBuilder::new("list", "The server's collection"))
                .option(SubCommandBuilder::new("mine", "Your pets and submissions with their status. Remove your own from here"))
                .option(
                    SubCommandBuilder::new("remove", "Take pets out of the collection (staff)")
                        .option(pet_opt("Which pet"))
                        .option(qty_opt("How many, 1 if left out"))
                        .option(BooleanBuilder::new("plus", "True for Pet+ copies only, false for plain ones only")),
                )
                .build(),
        ]
    }

    fn routes(&self) -> bot_core::axum::Router<Ctx> {
        web::routes().merge(public::routes()).merge(picture::routes()).merge(idleon::routes())
    }

    fn overview(&self, ctx: &Ctx, page: &bot_core::web::Page) -> Overview {
        let guild = page.guild_id;
        let mut o = Overview::default();
        let pending = store::pending(ctx, guild).unwrap_or_default();
        if let Some(oldest) = pending.first() {
            let mut who: Vec<u64> = Vec::new();
            for p in &pending {
                if !who.contains(&p.user_id) {
                    who.push(p.user_id);
                }
            }
            let names: Vec<String> = who.iter().take(2).map(|u| ctx.display_name(guild, *u)).collect();
            let more = who.len().saturating_sub(2);
            let n = pending.len();
            o.needs.push(Need {
                since: oldest.requested_at,
                people: who.iter().take(2).map(|u| ctx.member_avatar(guild, *u)).collect(),
                icon: Icon::Paw,
                title: format!("{n} pet {} waiting", if n == 1 { "request" } else { "requests" }),
                detail: format!("{} · oldest {}", names_line(&names, more), text::ago(oldest.requested_at)),
                href: page.path("pets/approvals"),
                action: "Review",
                alert: false,
            });
        }
        let since = now_ms() - 3 * DAY_MS;
        for (name, by, copies, at) in store::recent_additions(ctx, guild, since, 5).unwrap_or_default() {
            o.feed.push(Happening {
                at,
                avatar: ctx.member_avatar(guild, by),
                who: ctx.display_name(guild, by),
                text: format!("added {}{name} to the pet collection", qty_label(copies)),
            });
        }
        o
    }

    async fn on_command(&self, ctx: &Ctx, call: &CommandCall) -> Result<()> {
        let Some(guild) = call.guild_id() else { return Ok(()) };
        store::ensure_seeded(ctx, guild)?;
        if call.data.name != "pet" {
            return Ok(());
        }
        let Some((sub, opts)) = call.opts().sub() else { return Ok(()) };
        let qty = opts.int("quantity").unwrap_or(1).clamp(1, 25);
        match sub {
            "add" => {
                let Some(pet) = store::find_pet(ctx, guild, opts.str("pet").unwrap_or_default())? else {
                    return ctx.reply(&call.interaction, Reply::text("That pet isn't in the catalog. Pick one from the list.").ephemeral()).await;
                };
                let plus = opts.bool("plus").unwrap_or(false);
                let issued = now_ms() / 1000;
                let confirm = format!("{KEY}:confirm:{}:{qty}:{issued}:{}", pet.id, u8::from(plus));
                let cancel = format!("{KEY}:cancel");
                let mut e = EmbedBuilder::new()
                    .color(COLOR)
                    .title(format!("Submit {}{} {}{}?", qty_label(qty), pet.icon(), pet.name, version(plus)))
                    .description("Staff review submissions before they join the collection. Check on it, or take it back, with /pet mine.")
                    .field(EmbedFieldBuilder::new("Rarity", store::rarity_label(&pet.rarity)).inline())
                    .field(EmbedFieldBuilder::new("Quantity", qty.to_string()).inline())
                    .field(EmbedFieldBuilder::new("Version", if plus { "Pet+" } else { "Plain, not Pet+" }).inline());
                let bonus = if plus && !pet.plus_bonus.is_empty() { &pet.plus_bonus } else { &pet.bonus };
                if !bonus.is_empty() {
                    e = e.field(EmbedFieldBuilder::new(if plus { "Pet+ bonus" } else { "Bonus" }, text::clip(bonus, 1000)));
                }
                if let Some(img) = pet.embed_image(&ctx.cfg.public_url).and_then(|u| ImageSource::url(u).ok()) {
                    e = e.thumbnail(img);
                }
                let row = button_row(&[(&confirm, "Submit", ButtonStyle::Success), (&cancel, "Cancel", ButtonStyle::Secondary)]);
                ctx.reply(&call.interaction, Reply::embed(e.build()).components(vec![row]).ephemeral()).await?;
            }
            "list" => {
                let (embed, rows) = list_page(ctx, guild, 0)?;
                ctx.reply(&call.interaction, Reply::embed(embed).components(rows)).await?;
            }
            "mine" => {
                let (embed, rows) = my_pets(ctx, guild, call.user_id(), None)?;
                ctx.reply(&call.interaction, Reply::embed(embed).components(rows).ephemeral()).await?;
            }
            "remove" => {
                let staff = call
                    .interaction
                    .member
                    .as_ref()
                    .and_then(|m| m.permissions)
                    .is_some_and(|p| p.intersects(Permissions::ADMINISTRATOR | Permissions::MANAGE_GUILD));
                if !staff {
                    return ctx.reply(&call.interaction, Reply::text("Only staff with Manage Server can remove pets.").ephemeral()).await;
                }
                let Some(pet) = store::find_pet(ctx, guild, opts.str("pet").unwrap_or_default())? else {
                    return ctx.reply(&call.interaction, Reply::text("That pet isn't in the catalog.").ephemeral()).await;
                };
                let plus = opts.bool("plus");
                let removed = store::remove_owned(ctx, guild, &pet.id, qty, None, plus)?;
                let which = match plus {
                    Some(true) => " Pet+",
                    Some(false) => " plain",
                    None => "",
                };
                let msg = if removed == 0 {
                    format!("The collection has no{which} {} {}.", pet.icon(), pet.name)
                } else {
                    let left = store::owned_counts(ctx, guild)?.get(&pet.id).copied().unwrap_or(0);
                    let left_plus = store::plus_counts(ctx, guild)?.get(&pet.id).copied().unwrap_or(0);
                    format!(
                        "Removed {}{} {}{}. {} left.",
                        qty_label(removed),
                        pet.icon(),
                        pet.name,
                        if plus == Some(true) { " (Pet+)" } else { "" },
                        copies_line(left, left_plus)
                    )
                };
                ctx.reply(&call.interaction, Reply::text(msg).ephemeral()).await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn on_autocomplete(&self, ctx: &Ctx, call: &CommandCall) -> Result<()> {
        let Some(guild) = call.guild_id() else { return Ok(()) };
        store::ensure_seeded(ctx, guild)?;
        let typed = call.opts().focused().map(|(_, v)| v.to_lowercase()).unwrap_or_default();
        let removing = call.opts().sub().is_some_and(|(s, _)| s == "remove");
        let owned = store::owned_counts(ctx, guild)?;
        let plus = if removing { store::plus_counts(ctx, guild)? } else { Default::default() };
        let choices = store::catalog(ctx, guild)?
            .into_iter()
            .filter(|p| !removing || owned.contains_key(&p.id))
            .filter(|p| typed.is_empty() || p.name.to_lowercase().contains(&typed) || p.category.to_lowercase().contains(&typed) || p.id.contains(&typed))
            .map(|p| {
                let label = match owned.get(&p.id) {
                    Some(n) if removing => format!("{} {} ({} owned)", p.icon(), p.name, copies_line(*n, plus.get(&p.id).copied().unwrap_or(0))),
                    _ => format!("{} {} ({}, {})", p.icon(), p.name, store::rarity_label(&p.rarity), p.category),
                };
                (text::clip(&label, 100), p.id)
            })
            .collect();
        ctx.autocomplete(&call.interaction, choices).await
    }

    async fn on_component(&self, ctx: &Ctx, call: &ComponentCall) -> Result<()> {
        let Some(guild) = call.guild_id() else { return Ok(()) };
        match call.args().as_slice() {
            // Dialogs from before Pet+ have no last part.
            ["confirm", pet_id, qty, issued, rest @ ..] => {
                let plus = rest.first() == Some(&"1");
                let issued: i64 = issued.parse().unwrap_or(0);
                if now_ms() / 1000 - issued > CONFIRM_SECS {
                    return ctx.update(&call.interaction, Reply::text("This expired. Run /pet add again.")).await;
                }
                let Some(pet) = store::pet(ctx, guild, pet_id)? else {
                    return ctx.update(&call.interaction, Reply::text("That pet was removed from the catalog.")).await;
                };
                let qty: i64 = qty.parse::<i64>().unwrap_or(1).clamp(1, 25);
                let user = call.user_id();
                // A double click sends the same dialog twice; submit it once.
                if let Some(m) = call.interaction.message.as_ref()
                    && !first_press(m.id.get())
                {
                    return Ok(());
                }
                store::submit(ctx, guild, &pet.id, user, qty, plus)?;
                ctx.notify(
                    Some(guild),
                    KEY,
                    format!("{} submitted {}{}{} for approval", ctx.display_name(guild, user), qty_label(qty), pet.name, version(plus)),
                    Some(format!("/g/{guild}/{KEY}/approvals")),
                );
                let e = EmbedBuilder::new()
                    .color(COLOR)
                    .title(format!("Submitted {}{} {}{}", qty_label(qty), pet.icon(), pet.name, version(plus)))
                    .description("It joins the collection once staff approve it. You'll get a DM either way if your DMs are open.")
                    .field(EmbedFieldBuilder::new("Status and removal", "Run /pet mine to see whether it's approved, and to withdraw it or remove your pets yourself, no need to ask staff."))
                    .build();
                ctx.update(&call.interaction, Reply::embed(e)).await?;
            }
            ["cancel"] => {
                ctx.update(&call.interaction, Reply::text("Cancelled. Nothing was submitted.")).await?;
            }
            ["list", page] => {
                let (embed, rows) = list_page(ctx, guild, page.parse().unwrap_or(0))?;
                ctx.update(&call.interaction, Reply::embed(embed).components(rows)).await?;
            }
            // `/pet mine`: a pick from the menu asks first.
            ["mine"] => {
                let user = call.user_id();
                let picked = call.data.values.first().map(String::as_str).unwrap_or_default();
                let (embed, rows) = match ask_remove(ctx, guild, user, picked)? {
                    Some(asked) => asked,
                    None => my_pets(ctx, guild, user, Some("That's no longer yours to remove. Here's what you have now."))?,
                };
                ctx.update(&call.interaction, Reply::embed(embed).components(rows)).await?;
            }
            ["back"] => {
                let (embed, rows) = my_pets(ctx, guild, call.user_id(), None)?;
                ctx.update(&call.interaction, Reply::embed(embed).components(rows)).await?;
            }
            ["wd", _, issued] | ["rm", _, _, _, issued] if expired(issued) => {
                let (embed, rows) = my_pets(ctx, guild, call.user_id(), Some("That question expired, so nothing was removed. Pick it again."))?;
                ctx.update(&call.interaction, Reply::embed(embed).components(rows)).await?;
            }
            ["wd", id, issued] => {
                let user = call.user_id();
                if !first_press(nonce(issued)) {
                    return Ok(());
                }
                let note = match store::withdraw(ctx, guild, user, id.parse().unwrap_or(0))? {
                    Some(w) => {
                        let what = submission_name(ctx, guild, &w);
                        ctx.notify(Some(guild), KEY, format!("{} withdrew their {what} submission", ctx.display_name(guild, user)), Some(format!("/g/{guild}/{KEY}/approvals")));
                        format!("Withdrew your {what} submission.")
                    }
                    None => "That submission was already reviewed or withdrawn, so nothing changed.".to_owned(),
                };
                let (embed, rows) = my_pets(ctx, guild, user, Some(&note))?;
                ctx.update(&call.interaction, Reply::embed(embed).components(rows)).await?;
            }
            ["rm", pet_id, plus, qty, issued] => {
                let user = call.user_id();
                if !first_press(nonce(issued)) {
                    return Ok(());
                }
                let plus = *plus == "1";
                let qty: i64 = qty.parse::<i64>().unwrap_or(1).clamp(1, 1000);
                let name = store::pet(ctx, guild, pet_id)?.map(|p| format!("{} {}{}", p.icon(), p.name, version(plus))).unwrap_or_else(|| pet_id.to_string());
                let note = if store::giving_away(ctx, guild, user, pet_id)? {
                    format!("{name} is in a giveaway of yours that waits for its hand-over, so it stays until that's done.")
                } else {
                    match store::remove_mine(ctx, guild, user, pet_id, plus, qty)? {
                        0 => format!("You have no {name} in the collection, so nothing changed."),
                        n => {
                            ctx.notify(
                                Some(guild),
                                KEY,
                                format!("{} took {}{name} out of the collection", ctx.display_name(guild, user), qty_label(n)),
                                Some(format!("/g/{guild}/{KEY}")),
                            );
                            format!("Took {}{name} out of the collection.", qty_label(n))
                        }
                    }
                };
                let (embed, rows) = my_pets(ctx, guild, user, Some(&note))?;
                ctx.update(&call.interaction, Reply::embed(embed).components(rows)).await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn tick(&self, ctx: &Ctx) -> Result<()> {
        for guild in ctx.enabled_guilds(KEY) {
            for g in store::expiring(ctx, guild, WARN_BEFORE_MS)? {
                let Some(at) = g.expires_at else { continue };
                if at <= now_ms() {
                    if store::close_giveaway(ctx, guild, g.id, Status::Expired, "Expired")? {
                        let line = expired_dm(&g.pet_name, &name(ctx, guild, g.giver_id), &name(ctx, guild, g.winner_id), &ctx.guild_name(guild));
                        dm_later(ctx, g.winner_id, line.clone());
                        dm_later(ctx, g.giver_id, line);
                    }
                } else if !g.warned {
                    store::mark_warned(ctx, guild, g.id)?;
                    let line = warning_dm(&g.pet_name, &name(ctx, guild, g.giver_id), &name(ctx, guild, g.winner_id), &ctx.guild_name(guild), at);
                    dm_later(ctx, g.winner_id, line.clone());
                    dm_later(ctx, g.giver_id, line);
                }
            }
        }
        Ok(())
    }

    fn seed_preview(&self, ctx: &Ctx, guild: u64) -> Result<()> {
        preview_rows(ctx, guild)
    }
}

/// The `/pet add` dialogs submitted lately, so a second press of the same Submit is ignored.
static SUBMITTED: std::sync::Mutex<std::collections::VecDeque<u64>> = std::sync::Mutex::new(std::collections::VecDeque::new());
const SUBMITTED_KEEP: usize = 256;

/// True the first time a dialog's message is seen.
fn first_press(message: u64) -> bool {
    let mut seen = SUBMITTED.lock().unwrap_or_else(|e| e.into_inner());
    if seen.contains(&message) {
        return false;
    }
    if seen.len() >= SUBMITTED_KEEP {
        seen.pop_front();
    }
    seen.push_back(message);
    true
}

/// A question's issue time, as the key that keeps a double press from removing twice.
fn nonce(issued: &str) -> u64 {
    issued.parse().unwrap_or(0)
}

/// True when a `/pet mine` question was asked longer ago than buttons stay good.
fn expired(issued: &str) -> bool {
    now_ms() - issued.parse::<i64>().unwrap_or(0) > CONFIRM_SECS * 1000
}

fn qty_label(n: i64) -> String {
    if n > 1 { format!("{n}× ") } else { String::new() }
}

/// After a pet's name: " (Pet+)" for its upgraded version.
pub(crate) fn version(plus: bool) -> &'static str {
    if plus { " (Pet+)" } else { "" }
}

/// "3", or "3, 1 of them Pet+".
fn copies_line(total: i64, plus: i64) -> String {
    match (total, plus) {
        (0, _) => "None".into(),
        (t, 0) => t.to_string(),
        (t, p) if p == t => format!("{t}, all Pet+"),
        (t, p) => format!("{t}, {p} of them Pet+"),
    }
}

/// "Mira and Teo", or "Mira and 2 more" when `more` people are left out.
fn names_line(names: &[String], more: usize) -> String {
    if more == 0 {
        names.join(" and ")
    } else {
        format!("{} and {more} more", names.join(", "))
    }
}

fn name(ctx: &Ctx, guild: u64, user: u64) -> String {
    ctx.display_name(guild, user)
}

/// Send a DM without holding up the caller. Closed DMs are ignored.
pub(crate) fn dm_later(ctx: &Ctx, user: u64, content: String) {
    let ctx = ctx.clone();
    bot_core::tokio::spawn(async move {
        if let Err(e) = ctx.dm(user, &content, &[], &[]).await {
            bot_core::tracing::debug!(user, error = %e, "pet DM failed");
        }
    });
}

/// The public page for this server, when the dashboard has an https address Discord accepts.
fn public_link(ctx: &Ctx, guild: u64, page: &str) -> Option<String> {
    let base = ctx.cfg.public_url.trim_end_matches('/');
    base.starts_with("https://").then(|| format!("{base}/p/{guild}/{page}"))
}

/// One page of `/pet list`: owned pets grouped by category.
fn list_page(ctx: &Ctx, guild: u64, page: i64) -> Result<(Embed, Vec<Component>)> {
    let counts = store::owned_counts(ctx, guild)?;
    let plus = store::plus_counts(ctx, guild)?;
    let total: i64 = counts.values().sum();
    let catalog = store::sorted(store::catalog(ctx, guild)?);
    let mut lines: Vec<String> = Vec::new();
    for cat in store::categories(ctx, guild)? {
        let owned: Vec<&Pet> = catalog.iter().filter(|p| p.category == cat && counts.contains_key(&p.id)).collect();
        if owned.is_empty() {
            continue;
        }
        let kinds = catalog.iter().filter(|p| p.category == cat).count();
        lines.push(format!("**{cat}**  ·  {} of {kinds}", owned.len()));
        for p in owned {
            let n = counts[&p.id];
            let pn = plus.get(&p.id).copied().unwrap_or(0);
            lines.push(format!(
                "{} {}{}{}  ·  {}",
                p.icon(),
                p.name,
                if n > 1 { format!(" ×{n}") } else { String::new() },
                match pn {
                    0 => String::new(),
                    k if k == n => "  ·  Pet+".to_owned(),
                    k => format!("  ·  {k} Pet+"),
                },
                store::rarity_label(&p.rarity)
            ));
        }
    }
    let per = 20;
    let pages = (lines.len() as i64 + per - 1) / per;
    let pages = pages.max(1);
    let page = page.clamp(0, pages - 1);
    let body = if lines.is_empty() {
        "The collection is empty. Use /pet add to submit the first one.".to_owned()
    } else {
        lines.iter().skip((page * per) as usize).take(per as usize).cloned().collect::<Vec<_>>().join("\n")
    };
    let mut e = EmbedBuilder::new()
        .color(COLOR)
        .title(format!("{} pet collection", ctx.guild_name(guild)))
        .description(body)
        .footer(EmbedFooterBuilder::new(format!("{} pets{}", text::num(total), if pages > 1 { format!("  ·  page {} of {pages}", page + 1) } else { String::new() })));
    if let Some(url) = public_link(ctx, guild, "pets") {
        e = e.url(url);
    }
    let mut rows = Vec::new();
    if pages > 1 {
        let prev = format!("{KEY}:list:{}", (page - 1).max(0));
        let next = format!("{KEY}:list:{}", (page + 1).min(pages - 1));
        rows.push(button_row(&[(&prev, "Previous", ButtonStyle::Secondary), (&next, "Next", ButtonStyle::Secondary)]));
    }
    Ok((e.build(), rows))
}

/// "👑 2× King Doot (Pet+)" for a submission.
fn submission_name(ctx: &Ctx, guild: u64, w: &Pending) -> String {
    let (icon, name) = store::pet(ctx, guild, &w.pet_id).ok().flatten().map(|p| (p.icon().to_owned(), p.name)).unwrap_or_else(|| ("🐾".into(), w.pet_id.clone()));
    format!("{icon} {}{name}{}", qty_label(w.quantity), version(w.plus))
}

/// What a member can remove from `/pet mine`, as the menu's values: `w:{id}` withdraws a waiting
/// submission, `c:{pet}:{0|1}` takes their plain or Pet+ copies of a pet out.
fn mine_value_copies(c: &MyCopies) -> String {
    format!("c:{}:{}", c.pet_id, u8::from(c.plus))
}

/// `/pet mine`: a member's pets in the collection, their submissions with each one's status, and
/// a menu to remove any of their own. `note` goes on top, after a removal.
fn my_pets(ctx: &Ctx, guild: u64, user: u64, note: Option<&str>) -> Result<(Embed, Vec<Component>)> {
    let catalog = store::catalog(ctx, guild)?;
    let find = |id: &str| catalog.iter().find(|p| p.id == id);
    let copies = store::my_copies(ctx, guild, user)?;
    let subs = store::submissions_of(ctx, guild, user, 40)?;
    let waiting: Vec<&Pending> = subs.iter().filter(|w| w.status == "pending").collect();
    // Rejections from the last 30 days; older ones are settled.
    let since = now_ms() - 30 * DAY_MS;
    let rejected: Vec<&Pending> = subs.iter().filter(|w| w.status == "rejected" && w.reviewed_at.unwrap_or(0) >= since).collect();
    let mut lines = Vec::new();
    if let Some(n) = note {
        lines.push(format!("**{n}**"));
        lines.push(String::new());
    }
    if !copies.is_empty() {
        lines.push("**Approved, in the collection**".to_owned());
        for c in copies.iter().take(15) {
            let Some(p) = find(&c.pet_id) else { continue };
            lines.push(format!("{} {}{}{}  ·  <t:{}:R>", p.icon(), p.name, if c.count > 1 { format!(" ×{}", c.count) } else { String::new() }, version(c.plus), c.latest / 1000));
        }
        if copies.len() > 15 {
            lines.push(format!("and {} more", copies.len() - 15));
        }
    }
    let mut block = |title: &str, list: &[&Pending], reason: bool| {
        if list.is_empty() {
            return;
        }
        if !lines.is_empty() && lines.last().is_some_and(|l| !l.is_empty()) {
            lines.push(String::new());
        }
        lines.push(format!("**{title}**"));
        for w in list.iter().take(10) {
            let Some(p) = find(&w.pet_id) else { continue };
            let at = if reason { w.reviewed_at.unwrap_or(w.requested_at) } else { w.requested_at };
            let why = if reason && !w.reason.is_empty() { format!("  ·  {}", text::clip(&w.reason, 120)) } else { String::new() };
            lines.push(format!("{} {}{}{}  ·  <t:{}:R>{why}", p.icon(), qty_label(w.quantity), p.name, version(w.plus), at / 1000));
        }
    };
    block("Waiting for approval", &waiting, false);
    block("Rejected", &rejected, true);
    let held: i64 = copies.iter().map(|c| c.count).sum();
    let pending: i64 = waiting.iter().map(|w| w.quantity).sum();
    let nothing = copies.is_empty() && waiting.is_empty() && rejected.is_empty();
    let desc = if nothing && note.is_none() {
        "You haven't added any pets yet. Use /pet add to submit one.".to_owned()
    } else if nothing {
        format!("{}\nYou have no pets in the collection and nothing waiting.", lines.join("\n"))
    } else {
        lines.join("\n")
    };
    let mut e = EmbedBuilder::new()
        .color(COLOR)
        .title(format!("{}'s pets", ctx.display_name(guild, user)))
        .description(text::clip(&desc, 4000))
        .field(EmbedFieldBuilder::new("Approved", text::num(held)).inline())
        .field(EmbedFieldBuilder::new("Waiting", text::num(pending)).inline())
        .field(EmbedFieldBuilder::new("Rejected", text::num(rejected.len() as i64)).inline());
    // The menu: waiting submissions first (they can be withdrawn), then copies.
    let mut options: Vec<SelectMenuOption> = Vec::new();
    for w in waiting.iter().take(10) {
        let Some(p) = find(&w.pet_id) else { continue };
        options.push(SelectMenuOption {
            default: false,
            description: Some(format!("Waiting for approval. Withdraw it, sent {}", text::ago(w.requested_at))),
            emoji: None,
            label: text::clip(&format!("{} {}{}{}", p.icon(), qty_label(w.quantity), p.name, version(w.plus)), 100),
            value: format!("w:{}", w.id),
        });
    }
    for c in &copies {
        if options.len() >= 25 {
            break;
        }
        let Some(p) = find(&c.pet_id) else { continue };
        options.push(SelectMenuOption {
            default: false,
            description: Some(if c.count > 1 { format!("In the collection, {} copies. Take them out", c.count) } else { "In the collection. Take it out".to_owned() }),
            emoji: None,
            label: text::clip(&format!("{} {}{}{}", p.icon(), p.name, if c.count > 1 { format!(" ×{}", c.count) } else { String::new() }, version(c.plus)), 100),
            value: mine_value_copies(c),
        });
    }
    let mut rows = Vec::new();
    if !options.is_empty() {
        e = e.footer(EmbedFooterBuilder::new("Pick one in the menu to remove it. You'll be asked to confirm first."));
        rows.push(Component::ActionRow(ActionRow {
            id: None,
            components: vec![Component::SelectMenu(SelectMenu {
                id: None,
                channel_types: None,
                custom_id: format!("{KEY}:mine"),
                default_values: None,
                disabled: false,
                kind: SelectMenuType::Text,
                max_values: Some(1),
                min_values: Some(1),
                options: Some(options),
                placeholder: Some("Remove one of your pets".into()),
                required: None,
            })],
        }));
    }
    Ok((e.build(), rows))
}

/// The question after a pick in `/pet mine`'s menu, with buttons to remove or keep. None when the
/// pick isn't the member's to remove any more.
fn ask_remove(ctx: &Ctx, guild: u64, user: u64, picked: &str) -> Result<Option<(Embed, Vec<Component>)>> {
    let issued = now_ms();
    let keep = format!("{KEY}:back");
    match picked.split(':').collect::<Vec<_>>().as_slice() {
        ["w", id] => {
            let id: i64 = id.parse().unwrap_or(0);
            let Some(w) = store::submissions_of(ctx, guild, user, 100)?.into_iter().find(|w| w.id == id && w.status == "pending") else { return Ok(None) };
            let what = submission_name(ctx, guild, &w);
            let go = format!("{KEY}:wd:{id}:{issued}");
            let e = EmbedBuilder::new()
                .color(COLOR)
                .title(format!("Withdraw {what}?"))
                .description("Staff won't review it. To send it again later, use /pet add.")
                .build();
            Ok(Some((e, vec![button_row(&[(&go, "Withdraw it", ButtonStyle::Danger), (&keep, "Keep it", ButtonStyle::Secondary)])])))
        }
        ["c", pet_id, plus] => {
            let plus = *plus == "1";
            let Some(c) = store::my_copies(ctx, guild, user)?.into_iter().find(|c| c.pet_id == *pet_id && c.plus == plus) else { return Ok(None) };
            let name = store::pet(ctx, guild, pet_id)?.map(|p| format!("{} {}{}", p.icon(), p.name, version(plus))).unwrap_or_else(|| pet_id.to_string());
            let one = format!("{KEY}:rm:{pet_id}:{}:1:{issued}", u8::from(plus));
            let all = format!("{KEY}:rm:{pet_id}:{}:{}:{issued}", u8::from(plus), c.count);
            let all_label = format!("Remove all {}", c.count);
            let mut buttons: Vec<(&str, &str, ButtonStyle)> = vec![(&one, if c.count > 1 { "Remove 1" } else { "Remove it" }, ButtonStyle::Danger)];
            if c.count > 1 {
                buttons.push((&all, &all_label, ButtonStyle::Danger));
            }
            buttons.push((&keep, "Keep", ButtonStyle::Secondary));
            let held = if c.count > 1 { format!("You have {} copies of it in the collection. ", c.count) } else { String::new() };
            let e = EmbedBuilder::new()
                .color(COLOR)
                .title(format!("Take {name} out of the collection?"))
                .description(format!("{held}Only your own copies are removed. Staff are told, and only they can put it back."))
                .build();
            Ok(Some((e, vec![button_row(&buttons)])))
        }
        _ => Ok(None),
    }
}

// ---- what the bot sends on its own, also sent as tests from the dashboard ----

/// The DM a member gets when staff review their submission. `pet` is "👑 King Doot".
pub(crate) fn review_dm(qty: i64, pet: &str, server: &str, approved: bool, reason: &str) -> String {
    let qty = qty_label(qty);
    if approved {
        format!("Your {qty}{pet} submission in {server} was approved and is in the collection.")
    } else if reason.is_empty() {
        format!("Your {qty}{pet} submission in {server} was rejected.")
    } else {
        format!("Your {qty}{pet} submission in {server} was rejected: {reason}")
    }
}

/// The DMs the winner and the giver get when a giveaway is recorded: `(winner's, giver's)`.
pub(crate) fn recorded_dms(icon: &str, pet: &str, giver: &str, winner: &str, server: &str, expires_at: Option<i64>) -> (String, String) {
    let until = expires_at.map(|t| format!(" It expires <t:{}:R>.", t / 1000)).unwrap_or_default();
    (format!("You won {icon} {pet} from {giver} in {server}.{until}"), format!("You're giving {icon} {pet} to {winner} in {server}.{until}"))
}

/// Sent to both 15 minutes before a giveaway expires.
pub(crate) fn warning_dm(pet: &str, giver: &str, winner: &str, server: &str, at: i64) -> String {
    format!("The {pet} giveaway from {giver} to {winner} in {server} expires <t:{}:R>. Finish the hand-over before then.", at / 1000)
}

/// Sent to both when a giveaway expires unconfirmed.
pub(crate) fn expired_dm(pet: &str, giver: &str, winner: &str, server: &str) -> String {
    format!("The {pet} giveaway from {giver} to {winner} in {server} expired and was cancelled.")
}

/// The post in the announce channel when a giveaway is confirmed.
pub(crate) fn announcement(giver: u64, winner: u64, icon: &str, pet: &str) -> String {
    format!("<@{giver}> gave <@{winner}> {icon} **{pet}**.")
}

/// DM the giver and winner about a new giveaway.
pub(crate) fn announce_recorded(ctx: &Ctx, guild: u64, g: &Giveaway) {
    let (to_winner, to_giver) = recorded_dms(g.icon(), &g.pet_name, &name(ctx, guild, g.giver_id), &name(ctx, guild, g.winner_id), &ctx.guild_name(guild), g.expires_at);
    dm_later(ctx, g.winner_id, to_winner);
    dm_later(ctx, g.giver_id, to_giver);
}

/// Post a confirmed giveaway in the announce channel, mentioning both.
pub(crate) async fn announce_confirmed(ctx: &Ctx, guild: u64, g: &Giveaway) {
    let Ok(s) = ctx.settings::<Settings>(guild, KEY) else { return };
    let Some((channel, to)) = s.announce_channel.and_then(|c| Some((c, Id::new_checked(c)?))) else { return };
    let content = announcement(g.giver_id, g.winner_id, g.icon(), &g.pet_name);
    let mentions = AllowedMentions { users: [g.giver_id, g.winner_id].into_iter().filter_map(Id::new_checked).collect(), ..Default::default() };
    if let Err(e) = ctx.http.create_message(to).content(&content).allowed_mentions(Some(&mentions)).await {
        bot_core::tracing::warn!(guild, channel, error = %e, "couldn't post a confirmed pet giveaway");
    }
}

fn preview_rows(ctx: &Ctx, guild: u64) -> Result<()> {
    use bot_core::rusqlite::params;
    store::ensure_seeded(ctx, guild)?;
    let m: Vec<u64> = bot_core::preview::MEMBERS.iter().map(|(id, _)| *id).collect();
    let now = now_ms();
    let hour = 3_600_000;
    let day = 24 * hour;
    // The collection: (pet, given by, copies, days ago).
    let owned: &[(&str, usize, i64, i64)] = &[
        ("king-doot", 0, 1, 40),
        ("sheepie", 1, 3, 35),
        ("slime", 2, 4, 33),
        ("molti", 0, 2, 30),
        ("bored-bean", 3, 1, 28),
        ("frog", 4, 2, 26),
        ("ancient-golem", 1, 1, 22),
        ("leek-spirit", 5, 1, 20),
        ("mashed-potato", 2, 2, 18),
        ("balloonfish", 6, 1, 15),
        ("pufferblob", 0, 2, 12),
        ("whale", 7, 1, 9),
        ("axolotl", 3, 1, 7),
        ("koi-fish", 1, 1, 5),
        ("glimbo", 8, 1, 3),
        ("cryosnake", 0, 1, 0),
    ];
    {
        let mut conn = ctx.db.lock();
        let tx = conn.transaction()?;
        for (pet, by, n, ago) in owned {
            for _ in 0..*n {
                tx.execute(
                    "INSERT INTO pets_owned (guild_id, pet_id, added_by, given_by, added_at, approved_by) VALUES (?1, ?2, ?3, ?3, ?4, 'Antho')",
                    params![guild as i64, pet, m[*by] as i64, now - ago * day],
                )?;
            }
        }
        // Some copies are Pet+: (pet, how many of its copies).
        for (pet, n) in [("king-doot", 1), ("slime", 2), ("pufferblob", 1), ("whale", 1)] {
            tx.execute(
                "UPDATE pets_owned SET plus = 1 WHERE id IN (SELECT id FROM pets_owned WHERE guild_id = ?1 AND pet_id = ?2 ORDER BY id LIMIT ?3)",
                params![guild as i64, pet, n],
            )?;
        }
        for (pet, by, n, ago_h, plus) in [("rift-slug", 4, 1, 2, true), ("panda", 6, 2, 5, false), ("crystal-capybara", 2, 1, 20, false), ("bunny", 9, 1, 30, true)] {
            tx.execute(
                "INSERT INTO pets_pending (guild_id, pet_id, user_id, quantity, requested_at, plus) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![guild as i64, pet, m[by] as i64, n, now - ago_h * hour, plus],
            )?;
        }
        for (pet, by, status, ago_h, reason) in [("hedgehog", 5, "approved", 26, ""), ("gigafrog", 7, "rejected", 50, "Already counted last week.")] {
            tx.execute(
                "INSERT INTO pets_pending (guild_id, pet_id, user_id, quantity, requested_at, status, reviewed_by, reviewed_at, reason)
                 VALUES (?1, ?2, ?3, 1, ?4, ?5, 'Antho', ?6, ?7)",
                params![guild as i64, pet, m[by] as i64, now - (ago_h + 3) * hour, status, now - ago_h * hour, reason],
            )?;
        }
        tx.commit()?;
    }
    // Giveaways: (pet, giver, winner, status, recorded days ago, expires in ms from now, warned).
    type Row = (&'static str, usize, usize, Status, i64, Option<i64>, bool);
    let rows: &[Row] = &[
        ("sheepie", 1, 5, Status::Pending, 0, Some(10 * 60_000), true),
        ("slime", 2, 7, Status::Pending, 0, Some(20 * hour), false),
        ("mashed-potato", 2, 8, Status::Pending, 1, None, false),
        ("king-doot", 0, 3, Status::Confirmed, 27, None, false),
        ("molti", 0, 4, Status::Confirmed, 24, None, false),
        ("frog", 4, 6, Status::Confirmed, 19, None, false),
        ("slime", 2, 9, Status::Confirmed, 14, None, false),
        ("pufferblob", 0, 1, Status::Confirmed, 10, None, false),
        ("sheepie", 1, 2, Status::Confirmed, 6, None, false),
        ("koi-fish", 1, 0, Status::Confirmed, 3, None, false),
        ("leek-spirit", 5, 6, Status::Expired, 8, None, true),
        ("whale", 7, 3, Status::Cancelled, 4, None, false),
    ];
    for (pet_id, giver, winner, status, ago, expires_in, warned) in rows {
        let Some(pet) = store::pet(ctx, guild, pet_id)? else { continue };
        let at = now - ago * day - 5 * hour;
        let id = store::record_giveaway(
            ctx,
            guild,
            &store::NewGiveaway { pet: &pet, giver: m[*giver], winner: m[*winner], notes: "", recorded_by: "Antho", expires_at: None },
        )?;
        let expires = expires_in.map(|e| now + e).or(Some(at + day));
        let closed = (*status != Status::Pending).then_some(at + 3 * hour);
        ctx.db.with(|c| {
            c.execute(
                "UPDATE pets_giveaways SET recorded_at = ?2, expires_at = ?3, warned = ?4, status = ?5, closed_by = ?6, closed_at = ?7 WHERE id = ?1",
                params![
                    id,
                    at,
                    if *status == Status::Pending && expires_in.is_none() { None } else { expires },
                    warned,
                    status.as_str(),
                    closed.map(|_| if *status == Status::Expired { "Expired" } else { "Antho" }),
                    closed
                ],
            )
        })?;
        if *pet_id == "sheepie" && *status == Status::Pending {
            store::add_comment(ctx, id, "Antho", "Oskar is online after 8pm server time.")?;
            store::add_comment(ctx, id, "Mira", "Kasia confirmed she has a free slot.")?;
        }
        if *pet_id == "whale" {
            store::add_comment(ctx, id, "Antho", "Winner left the server.")?;
        }
    }
    store::add_ban(ctx, guild, m[9], "Took a pet back after the giveaway.", "Antho")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use bot_core::Feature as _;

    #[test]
    fn one_command_with_subcommands() {
        let cmds = super::Pets.commands();
        assert_eq!(cmds.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["pet"]);
        let subs: Vec<&str> = cmds[0].options.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(subs, ["add", "list", "mine", "remove"]);
        assert!(cmds[0].options[2].description.len() <= 100);
        let add: Vec<&str> = cmds[0].options[0].options.as_deref().unwrap_or_default().iter().map(|o| o.name.as_str()).collect();
        assert_eq!(add, ["pet", "quantity", "plus"]);
    }

    #[test]
    fn copies_read_well() {
        assert_eq!(super::copies_line(3, 0), "3");
        assert_eq!(super::copies_line(3, 1), "3, 1 of them Pet+");
        assert_eq!(super::copies_line(2, 2), "2, all Pet+");
        assert_eq!(super::copies_line(0, 0), "None");
        assert_eq!(super::version(true), " (Pet+)");
    }

    #[test]
    fn a_dialog_submits_once() {
        assert!(super::first_press(1));
        assert!(!super::first_press(1));
        for m in 2..300 {
            assert!(super::first_press(m));
        }
        // Old dialogs fall out; the list stays small.
        assert!(super::first_press(1));
        assert!(super::SUBMITTED.lock().unwrap().len() <= super::SUBMITTED_KEEP);
    }

    /// `/pet mine` lists a member's own pets with their status and offers only theirs to remove,
    /// with ids that fit Discord's 100 characters and parse back into the handlers' shapes.
    #[test]
    fn mine_offers_only_your_own() {
        use bot_core::twilight_model::channel::message::component::Component;
        let rt = bot_core::tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let _in = rt.enter();
        let ctx = bot_core::Ctx::new(bot_core::Config::preview(0), bot_core::Db::memory().unwrap()).unwrap();
        ctx.install_features(vec![std::sync::Arc::new(super::Pets)]).unwrap();
        let (g, mira, oskar) = (45, 7, 8);
        super::store::ensure_seeded(&ctx, g).unwrap();
        super::store::add_owned(&ctx, g, "slime", mira, mira, 2, true, "Antho").unwrap();
        super::store::add_owned(&ctx, g, "frog", oskar, oskar, 1, false, "Antho").unwrap();
        let w = super::store::submit(&ctx, g, "king-doot", mira, 1, false).unwrap();
        let r = super::store::submit(&ctx, g, "molti", mira, 1, false).unwrap();
        super::store::review(&ctx, g, &[r], false, "Antho", "Already counted").unwrap();

        let (embed, rows) = super::my_pets(&ctx, g, mira, None).unwrap();
        let desc = embed.description.unwrap();
        assert!(desc.contains("Approved, in the collection") && desc.contains("Slime ×2 (Pet+)"));
        assert!(desc.contains("Waiting for approval") && desc.contains("King Doot"));
        assert!(desc.contains("Rejected") && desc.contains("Already counted"));
        assert!(!desc.contains("Frog"), "Oskar's pet isn't Mira's");
        let Component::ActionRow(row) = &rows[0] else { panic!("a menu row") };
        let Component::SelectMenu(menu) = &row.components[0] else { panic!("a menu") };
        let values: Vec<&str> = menu.options.as_ref().unwrap().iter().map(|o| o.value.as_str()).collect();
        assert_eq!(values, [format!("w:{w}"), "c:slime:1".to_owned()]);

        // Picking asks first; the buttons carry what to remove.
        let (_, rows) = super::ask_remove(&ctx, g, mira, "c:slime:1").unwrap().unwrap();
        let Component::ActionRow(row) = &rows[0] else { panic!("buttons") };
        let ids: Vec<String> = row.components.iter().map(|c| match c { Component::Button(b) => b.custom_id.clone().unwrap(), _ => String::new() }).collect();
        assert!(ids[0].starts_with("pets:rm:slime:1:1:") && ids[1].starts_with("pets:rm:slime:1:2:") && ids[2] == "pets:back");
        assert!(ids.iter().all(|i| i.len() <= 100));
        assert!(super::ask_remove(&ctx, g, mira, "c:frog:0").unwrap().is_none(), "not hers to remove");
        assert!(super::ask_remove(&ctx, g, oskar, &format!("w:{w}")).unwrap().is_none(), "not his to withdraw");
        assert!(super::ask_remove(&ctx, g, mira, &format!("w:{r}")).unwrap().is_none(), "already decided");
        assert!(!super::expired(&bot_core::now_ms().to_string()) && super::expired("0"));
    }
}
