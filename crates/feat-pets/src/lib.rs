//! Idleon companion collection, pet giveaways and public pet pages.
//!
//! Members submit pets they gave the server with `/pet add`; staff approve them on the dashboard
//! and they join the collection. Moderators record pet giveaways (giver to winner, with an
//! expiry), the bot DMs both, warns 15 minutes before expiry and cancels on expiry, and an editor
//! confirms the hand-over. Three public pages show the collection, giveaways and stats.

mod idleon;
mod picture;
mod public;
pub mod store;
mod view;
mod web;

use std::collections::BTreeMap;

use bot_core::anyhow::Result;
use bot_core::discord::{CommandCall, ComponentCall, Reply, button_row};
use bot_core::twilight_model::application::command::{Command, CommandType};
use bot_core::twilight_model::channel::message::component::{ButtonStyle, Component};
use bot_core::twilight_model::channel::message::{AllowedMentions, Embed};
use bot_core::twilight_model::guild::Permissions;
use bot_core::twilight_model::id::Id;
use bot_core::twilight_util::builder::command::{CommandBuilder, IntegerBuilder, StringBuilder, SubCommandBuilder};
use bot_core::twilight_util::builder::embed::{EmbedBuilder, EmbedFieldBuilder, EmbedFooterBuilder, ImageSource};
use bot_core::{Ctx, Feature, FeatureMeta, Group, Happening, Icon, Migration, NavItem, Need, Overview, feature_async, now_ms, text};
use serde::{Deserialize, Serialize};

use store::{Giveaway, Pet, Status};

pub const KEY: &str = "pets";
const COLOR: u32 = 0xf0806a;
/// A `/pet add` confirmation button stops working after this long.
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
                        .option(qty_opt("How many, 1 if left out")),
                )
                .option(SubCommandBuilder::new("list", "The server's collection"))
                .option(SubCommandBuilder::new("mine", "Pets you've added and what's waiting for approval"))
                .option(
                    SubCommandBuilder::new("remove", "Take pets out of the collection (staff)")
                        .option(pet_opt("Which pet"))
                        .option(qty_opt("How many, 1 if left out")),
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
                let issued = now_ms() / 1000;
                let confirm = format!("{KEY}:confirm:{}:{qty}:{issued}", pet.id);
                let cancel = format!("{KEY}:cancel");
                let mut e = EmbedBuilder::new()
                    .color(COLOR)
                    .title(format!("Submit {}{} {}?", qty_label(qty), pet.icon(), pet.name))
                    .description("Staff review submissions on the dashboard before they join the collection.")
                    .field(EmbedFieldBuilder::new("Rarity", store::rarity_label(&pet.rarity)).inline())
                    .field(EmbedFieldBuilder::new("Quantity", qty.to_string()).inline());
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
                let embed = my_pets(ctx, guild, call.user_id())?;
                ctx.reply(&call.interaction, Reply::embed(embed).ephemeral()).await?;
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
                let removed = store::remove_owned(ctx, guild, &pet.id, qty, None)?;
                let msg = if removed == 0 {
                    format!("The collection has no {} {}.", pet.icon(), pet.name)
                } else {
                    let left = store::owned_counts(ctx, guild)?.get(&pet.id).copied().unwrap_or(0);
                    format!("Removed {}{} {}. {} left.", qty_label(removed), pet.icon(), pet.name, if left == 0 { "None".to_owned() } else { left.to_string() })
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
        let choices = store::catalog(ctx, guild)?
            .into_iter()
            .filter(|p| !removing || owned.contains_key(&p.id))
            .filter(|p| typed.is_empty() || p.name.to_lowercase().contains(&typed) || p.category.to_lowercase().contains(&typed) || p.id.contains(&typed))
            .map(|p| {
                let label = match owned.get(&p.id) {
                    Some(n) if removing => format!("{} {} (×{n} owned)", p.icon(), p.name),
                    _ => format!("{} {} ({})", p.icon(), p.name, p.category),
                };
                (text::clip(&label, 100), p.id)
            })
            .collect();
        ctx.autocomplete(&call.interaction, choices).await
    }

    async fn on_component(&self, ctx: &Ctx, call: &ComponentCall) -> Result<()> {
        let Some(guild) = call.guild_id() else { return Ok(()) };
        match call.args().as_slice() {
            ["confirm", pet_id, qty, issued] => {
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
                store::submit(ctx, guild, &pet.id, user, qty)?;
                ctx.notify(
                    Some(guild),
                    KEY,
                    format!("{} submitted {}{} for approval", ctx.display_name(guild, user), qty_label(qty), pet.name),
                    Some(format!("/g/{guild}/{KEY}/approvals")),
                );
                let e = EmbedBuilder::new()
                    .color(COLOR)
                    .title(format!("Submitted {}{} {}", qty_label(qty), pet.icon(), pet.name))
                    .description("It joins the collection once staff approve it. You'll get a DM either way if your DMs are open.")
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

fn qty_label(n: i64) -> String {
    if n > 1 { format!("{n}× ") } else { String::new() }
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
    let total: i64 = counts.values().sum();
    let catalog = store::catalog(ctx, guild)?;
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
            lines.push(format!("{} {}{}  ·  {}", p.icon(), p.name, if n > 1 { format!(" ×{n}") } else { String::new() }, store::rarity_label(&p.rarity)));
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

/// `/pet mine`: what a member has added or given, and what's waiting.
fn my_pets(ctx: &Ctx, guild: u64, user: u64) -> Result<Embed> {
    let catalog = store::catalog(ctx, guild)?;
    let find = |id: &str| catalog.iter().find(|p| p.id == id);
    let mine = store::contributions(ctx, guild, user)?;
    let waiting = store::pending_for(ctx, guild, user)?;
    // Pet id -> (count, latest), in the order first seen (newest first).
    let mut by_pet: BTreeMap<usize, (String, i64, i64)> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for o in &mine {
        let idx = match order.iter().position(|p| *p == o.pet_id) {
            Some(i) => i,
            None => {
                order.push(o.pet_id.clone());
                order.len() - 1
            }
        };
        let e = by_pet.entry(idx).or_insert_with(|| (o.pet_id.clone(), 0, o.added_at));
        e.1 += 1;
        e.2 = e.2.max(o.added_at);
    }
    let mut lines = Vec::new();
    if !by_pet.is_empty() {
        lines.push("**In the collection**".to_owned());
        for (pet_id, n, at) in by_pet.values().take(20) {
            let Some(p) = find(pet_id) else { continue };
            lines.push(format!("{} {}{}  ·  <t:{}:R>", p.icon(), p.name, if *n > 1 { format!(" ×{n}") } else { String::new() }, at / 1000));
        }
    }
    if !waiting.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("**Waiting for approval**".to_owned());
        for w in waiting.iter().take(15) {
            let Some(p) = find(&w.pet_id) else { continue };
            lines.push(format!("{} {}{}  ·  <t:{}:R>", p.icon(), qty_label(w.quantity), p.name, w.requested_at / 1000));
        }
    }
    let approved = mine.len() as i64;
    let pending: i64 = waiting.iter().map(|w| w.quantity).sum();
    let desc = if lines.is_empty() { "You haven't added any pets yet. Use /pet add to submit one.".to_owned() } else { lines.join("\n") };
    Ok(EmbedBuilder::new()
        .color(COLOR)
        .title(format!("{}'s pets", ctx.display_name(guild, user)))
        .description(desc)
        .field(EmbedFieldBuilder::new("Approved", text::num(approved)).inline())
        .field(EmbedFieldBuilder::new("Waiting", text::num(pending)).inline())
        .field(EmbedFieldBuilder::new("Submitted", text::num(approved + pending)).inline())
        .build())
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
        for (pet, by, n, ago_h) in [("rift-slug", 4, 1, 2), ("panda", 6, 2, 5), ("crystal-capybara", 2, 1, 20), ("bunny", 9, 1, 30)] {
            tx.execute(
                "INSERT INTO pets_pending (guild_id, pet_id, user_id, quantity, requested_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![guild as i64, pet, m[by] as i64, n, now - ago_h * hour],
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
}
