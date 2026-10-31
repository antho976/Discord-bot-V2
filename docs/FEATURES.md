# Writing a feature

The bot is one Rust binary (`crates/app`) built from a shared core (`crates/core`, crate
`bot-core`) and one crate per feature (`crates/feat-*`). `crates/feat-levels` is the reference
implementation: read it before writing a feature, and copy its shape.

## The shape of a feature crate

- `src/lib.rs`: a struct implementing `bot_core::Feature`. Every hook has a no-op default.
  - `meta()`: key, name, one-line description, sidebar group, sidebar links (`NavItem`).
  - `migrations()`: `&'static [Migration]`, plain SQL, applied once in order. Never edit a shipped
    migration; add a new one. Name tables after the feature (`pets_catalog`, `tickets`).
  - `commands()`: slash commands built with `twilight_util::builder::command`. They're
    registered per server where the feature is on. Use `default_member_permissions` for
    staff-only commands.
  - `guild_commands(ctx, guild)`: commands that differ per server (ones people define on the
    dashboard, or one a setting switches on). Read cached settings only. After changing what it
    returns, call `bot_core::discord::register_commands(ctx, guild)`; Discord allows about 200
    re-registrations a day per server, so only when names or descriptions change.
  - `reserved_command_names()`: names `guild_commands` may return in some server, even where it
    doesn't now (a setting that's off), so custom commands can't take them.
  - `on_event()`: every gateway event for servers where the feature is on.
  - `on_command()` / `on_autocomplete()`: only for commands this feature declared.
  - `on_component()` / `on_modal()`: buttons, selects and modals whose custom id starts with
    `"{KEY}:"`. Always prefix custom ids with the key, e.g. `"pets:approve:42"`; read the parts
    after the prefix with `call.args()`.
  - `tick()`: once a minute. Keep your own slower cadence by checking times stored in the DB.
  - `start()`: once at boot, before the gateway connects.
  - `routes()`: dashboard pages and endpoints (axum `Router<Ctx>`).
  - `seed_preview()`: sample rows for `bot preview` (use ids from `bot_core::preview::MEMBERS`).
- `src/web.rs`: the dashboard pages.
- `examples/preview.rs`: already there. `cargo run -p feat-x --example preview -- 3101` serves
  only your feature's pages with the sample servers. Open
  `http://localhost:3101/dev/login?to=/g/1100000000000000001/<your path>` (it signs you in).

## The core, briefly

- `Ctx` (cheap to clone) holds everything:
  - `ctx.http`: twilight's Discord REST client.
  - `ctx.cache`: guilds, channels, roles, members, users, the last 25 messages per channel, voice
    states.
  - `ctx.db`: one SQLite connection. Use `ctx.db.with(|c| ...)` for one statement, or
    `ctx.db.lock()` plus a transaction for several. Never hold the lock across an `.await`.
  - `ctx.web`: reqwest for outside HTTP.
- **Settings:** one serde struct per feature with `#[serde(default)]` and a `Default` impl.
  Read it with `ctx.settings::<S>(guild, KEY)?`, which is cached. Write it with
  `ctx.save_settings(guild, KEY, &s)?`. Use your own tables for lists that grow (giveaways,
  tickets).
- **Interactions:** `ctx.reply(&call.interaction, Reply::text("..").ephemeral())`, plus
  `ctx.update` (edit the message a button is on), `ctx.defer` with `ctx.edit_original` or
  `ctx.followup`, `ctx.modal`, and `ctx.autocomplete`. Read options with `call.opts().str("name")`,
  `.int`, `.user`, `.channel`, `.role`, `.bool`, and `.sub()` for subcommands. Build buttons with
  `bot_core::discord::button_row`.
  - Every interaction must be answered within 3 seconds. Defer if you'll be slower.
  - Mentions are off in `Reply` unless you pass `AllowedMentions` with `.mentions(...)`. Do the
    same on `create_message`.
- **Messages:**
  - `ctx.say(channel, text)` sends a plain message with mentions off.
  - `ctx.dm(user, text, embeds, components)` returns `None` when DMs are closed.
  - For anything else, use `ctx.http.create_message(..)` and similar calls directly.
- **Helpers:**
  - `ctx.display_name(guild, user)`, `ctx.text_channels(guild)`, `ctx.roles(guild)` and
    `ctx.categories(guild)`.
  - `ctx.enabled_guilds(KEY)`: the servers to loop over in `tick`.
  - `bot_core::text`: `fill` for `{placeholders}`, `clip`, `ago`, `duration`,
    `parse_duration` ("2h", "1d 12h") and `num` (12,345).
- **Notices and the log:**
  - `ctx.notify(Some(guild), KEY, "text", Some(link))` puts a notice behind the dashboard bell.
    Use it for things a person must act on (a new pet request, a caught scam, a new ticket).
  - `ctx.audit(&page.user, Some(guild), KEY, "Changed X")` records a dashboard change. Call it
    after every save.
  - `ctx.count(guild, "key", n)` adds to a daily counter. `ctx.record_stat(guild, "key", v)`
    sets today's value.
- **Before snapshots:** the cache is updated before features see an event. Use
  `ctx.before_message(id)` in edit and delete handlers, `ctx.before_member(guild, user)` in
  member update and leave handlers (old roles, nickname, timeout), and
  `ctx.before_voice(guild, user)` for the previous voice channel.
- **Images:** the `bot-render` crate turns an SVG string into a PNG with Geist built in. Call
  it in `spawn_blocking`.
- Re-exports you can use without adding dependencies: `bot_core::{anyhow, rusqlite,
  serde_json, tracing, twilight_model, twilight_http, twilight_util, twilight_mention, jiff,
  reqwest, rand, tokio, axum}`. `maud` and `serde` must be direct dependencies; they already
  are in every crate.

## Dashboard pages

Every page handler starts the same way:

```rust
async fn page(State(ctx): State<Ctx>, user: User, Path(guild): Path<u64>) -> WebResult {
    let page = ctx.page(&user, guild, KEY, Level::View)?;   // Level::Edit for POSTs
    ...
    Ok(ui::layout(&ctx, &page, "Title", None, body).into_response())
}
```

- Paths:
  - Pages: `/g/{guild}/{KEY}...`
  - Public pages (no sign-in): `/p/...`. Wrap them in `ui::public_layout`.
  - Machine endpoints: `/api/...` (exempt from the same-origin check).
- **Forms post back and redirect** with `saved(&page.path("..."))`, which shows a "Saved" toast.
  Read bodies with `FormData`:
  - `.string`, `.text` (keeps newlines), `.bool` for switches, `.int_in(name, min, max, default)`,
    `.id` for selects (empty is `None`), and `.ids` for chip lists.
  - Return `WebError::Bad("what to fix")` for bad input.
- **Build pages from `bot_core::web::ui`:**
  - Structure: `section`, `card`, `settings_form`. `settings_form` disables everything for
    view-only people.
  - Fields: `field_toggle`, `field_text`, `field_textarea`, `field_number`, `field_select`,
    `field_channel`, `field_channels`, `field_role`, `field_roles`, `field_category`,
    `field_custom`.
  - Data: `readings`, `empty`, `badge`, `action_button` (destructive ones need a second click),
    `tabs`, `bar_chart`, `person`, `embed_preview`.
  - Tables: `div.table-wrap > table.table`, with `td.num` for numbers and `td.actions` for row
    buttons.
- View-only people must never see a working control: check `page.can_edit()` before rendering
  buttons and forms outside `settings_form`.

## How it should look and read

The owner's house style applies; `docs/DESIGN.md` has the full reference and `docs/ui/` the screenshots. In short:

- **Look:** near-black and calm. One answer per page, above the fold: lead with the
  `readings` that matter, then the controls. No boxes around passive content; cards only group
  settings rows. Use the coral accent (`var(--signal)`) only for what needs the person. State
  colours (`badge` Ok, Warn, Bad) only mean state. Charts use `var(--data)`.
- **Empty states:** every list has one, drawn with `ui::empty` and the section's icon, with one
  line on how it fills.
- **Copy:** dry, specific, in "you". No exclamation marks, no em dashes, no hype, nothing that
  explains the obvious. A button says what it does ("Approve", "End giveaway"), and a help line
  says what a setting changes.
- **Discord messages:** short, plain, embeds with colour `0xf0806a` where an embed helps.
- **Undo over confirm:** don't add confirm dialogs. Destructive dashboard actions use
  `action_button(..., destructive: true)`.

## Rules for parallel work

- Edit only your own crate (`crates/feat-<name>/`). Don't touch `crates/core`, `crates/app`,
  other features, or the workspace `Cargo.toml`. If the core lacks something, write a local
  helper in your crate. Note anything worth moving into the core in your report.
- Check with `cargo check -p feat-<name>`, `cargo test -p feat-<name>` and
  `cargo clippy -p feat-<name>`. Don't run `cargo build` for the whole workspace: other crates
  may be mid-edit.
- Look at your pages in the preview with headless Chromium:
  `chromium --headless=new --disable-gpu --hide-scrollbars --window-size=1440,900
  --virtual-time-budget=4000 --screenshot=out.png "http://localhost:PORT/dev/login?to=/g/1100000000000000001/<path>"`.
  Add `--window-size=390,844` for a phone. Then read the PNG. Fix what looks wrong before
  reporting.
- Don't commit. The lead commits each feature.
- Keep it lean: no new background loops (use `tick`), no large in-memory caches, and nothing
  that saves on every message unless the feature needs it. Prefer a few hundred clear lines
  over a framework.
