# RPG: decisions and build plan

The third attempt at a Discord RPG for the server. The first two (antho976/Discord-bot, then
antho976/antho-s-bot) were dropped: the first sprawled to ~40 systems and lived two weeks, the
second looked right but its idle play, progression and content were shallow and disconnected.

- Concept (what the game is, owner decisions marked): https://claude.ai/artifact/U2qwMshSm4i9HycUKYBsQG
- Screens (approved look): Relay design 19 "RPG hub · directions", page "Screens"

## Settled with the owner (2026-11-02)

- **Setting:** Greek and Norse myth, the server's theme. Realm 1 is Midgard.
- **Patrons instead of classes:** Thor, Athena, Hades at launch. Odin and Hermes are later ideas.
- **Lesser vow** after the first realm boss: a second god's passive at about half strength.
- **Altars:** every gift fills the server's weekly offering *and* your bond with that god.
  Bonds go 1 to 5, capped per realm (Midgard: 2).
- **Skills are fixed** and unlock by bond: Strike, Guard and Potion always; the patron's first
  skill on swearing; the second at bond 2; the merged pair skill at lesser vow + bond 1 with that
  god. The main god decides how a merged skill behaves (Thor damage, Athena defence, Hades drain).
- **Boss fights are turn-based and need thought:** telegraphed attacks, stamina, phases,
  weaknesses that scouting reveals.
- **Expeditions** are idle with decisions: region, danger (Normal ×1 loot/×1 hard, Hard
  ×1.5/×2, Heroic ×2/×3, Mythic ×3/×5), 1/4/8 hours, supplies. Drops are shown, swings are
  small, a rare drop is guaranteed after a set time, and drops match what the forge needs.
- **Play a lot or a little:** hunts for active play, Vigor builds while away and doubles
  rewards, realm caps stop anyone outrunning the content.
- **Guilds stay, without the old abuse:** gear is tradeable but locked to level and realm,
  guild XP comes from play, the treasury only buys upgrades, member cap, small perks.
- **Boss intel can be sold** to other players.
- **Progress is permanent** (no seasons for now). **No stream tie-ins** for now.
- **Look:** the "G" overview: a drawn realm banner (name, level, Power, the patron's emblem on
  the side), then what's happening now, then gold, Vigor and guild. No progress bars on the hub.
  C (the whole hub as one drawn card) was rejected; don't bring it back.
- **Screens stay short (2026-11-03, task #9):** owner found some "so crowded with info it
  sucks". A line earns its place only if it changes what you do now: menus show live state
  (HP, what's out, what's ready) or just the name, never a standing description; costs read
  as have/need on one line; a reward shared by every row is said once; explanations live in
  Help or on the screen where they matter (dungeon lore when you enter, the townsfolk's words
  when you claim), not repeated on every visit.
- **Banner art (2026-11-03, task #6):** each patron has one colour and a scene drawn by
  `crates/feat-rpg/assets/banner/gen.py` (run it with an output dir and the assets dir to redraw):
  Hades red, the Styx with a bident (two prongs; the old three-pronged one read as Poseidon);
  Thor blue, a storm and Mjolnir; Athena violet, the Acropolis and her owl. The top line always
  says "Sworn to <patron>"; a title never replaces it, it gets its own line under the name.
  The hub's container edge takes the patron's colour. Owner chose A, the drawn scene
  ("A 100%"), over B (seal) and C (no picture); all three are in Relay design 19, page
  "Banner rework". Changing the art means
  changing the signing salt in `banner.rs` too, or Discord keeps the old picture cached.
- **Per-server toggle** like every other feature.
- **Clicks must feel fast:** v2 felt slow because each click made two trips to Discord.

## Rules for the code

1. **The database is the only truth.** No game state in memory between clicks, no per-player
   timers. Idle progress (expeditions, Vigor, regen) is computed from timestamps when read.
2. **One trip per click.** Answer with `UpdateMessage` straight away. Defer only to render a
   new banner, which happens when realm, patron, level or Power changes.
3. **The route is in the custom id:** `rpg:{owner}:{view}:{action}:{arg}:{turn}`. Only the
   owner can click their hub; `{turn}` is the state version, so a stale or double click is
   refused instead of acting twice (no double-collecting an expedition).
4. **Every gold and item movement goes through one `transfer()`** inside a transaction and
   writes a ledger row. Shared values (the server offering) change in one guarded statement.
5. **The engine is plain Rust with no Discord:** combat, expeditions, loot, bonds and Power are
   pure functions with a seeded RNG, so they're tested and replayed exactly.
6. **Nothing exists on its own:** a test fails if an item has no use, or a currency can be
   earned but not spent.

## Build plan

### Phase 0: numbers before code

- One data module for all content and balance: regions and drop tables, monsters, the boss and
  its phases, gear recipes, supplies, skills, bond costs and caps, the XP curve, the Power formula.
- A simulator (`cargo run -p feat-rpg --example sim`) plays 30 days for a light player (2-3
  short visits a day) and a heavy one (hours a day) for each patron and lesser vow. It reports
  the day each reaches the boss, gold earned against spent, and materials short or piling up.
- Targets: heavy player at the boss in about 5 days, light player in about 10.
- Draw the emoji set as SVG, render to PNG with `crates/render`, upload them as the bot's own
  application emojis (Discord allows 2,000, and they use no server slots).

### Phase 1: the engine (`crates/feat-rpg`, no Discord)

- `engine/combat.rs`: `resolve(state, action, seed) -> (state, log)`. Telegraphs, stamina,
  phases, skills (patron, merged, by main god), weaknesses and the "scouted" flags.
- `engine/expedition.rs`: the outcome from region, danger, length, supplies, Power, patron and
  bonds, worked out on collect. The hour-by-hour story comes from templates.
- `engine/economy.rs`: Power, XP and levels, realm caps, bonds, Vigor.
- Tests: every boss against all 6 patron pairings stays inside a win-rate band; the
  no-orphan-items and sources-and-sinks checks; the pace targets from the simulator.

### Phase 2: Discord

- `/rpg` posts the hub in the RPG channel (Components V2) and deletes that player's previous
  hub. A first-time player gets the patron choice instead.
- Screens, in build order: patron choice, overview, expedition plan and report, hunt, forge,
  pack, goals, temple with altars, boss fight.
- Banner: SVG to PNG via `crates/render` in `spawn_blocking`, cached by
  (realm, patron, level, Power) and kept as the message's attachment on normal clicks.
- Click timing (Discord to bot, bot work, reply) logged per click. Budget: bot work under 5 ms
  at p95. A test counts SQL statements per click and fails if one rises (a ratchet).

### Phase 3: dashboard

- On/off per server, the RPG channel, and a tester list (only listed members can play during
  the closed test).
- A test page: players, daily actives, level and Power over time, gold in against gold out,
  materials piling up. Admin tools: reset a player, grant items, read the ledger.
- A few multipliers (XP, drops, gold) editable without a deploy, for tuning during the test.

### Phase 4: closed test week

- 5 to 10 testers. Read the test page daily and tune. Fix before adding.
- Content for the test: Midgard (3 regions, 1 boss), expeditions, hunts and Vigor, the forge,
  the temple with altars and bonds, 2 skills per patron, lesser vows and the 3 merged skills
  (heavy testers reach the boss around day 5).

### After the test, one at a time

Guilds, selling boss intel, trading (level-locked gear, materials with a weekly cap), realms 2
and 3, Titan events, more patrons (only once all their pairs are designed).

## Status (2026-11-03)

Phases 0 to 3 are built in `crates/feat-rpg`. Phase 4 (the closed test) is the owner's to start.

### Starting the closed test

1. Dashboard → Features: switch **RPG** on for the server.
2. RPG → Settings: pick the **RPG channel** and the **tester role**. The closed test is on by
   default: only that role (and anyone with Manage Server) can play.
3. Give testers the role and tell them to use `/rpg` in the channel.
4. The bot uploads its emoji set within a minute of the feature being on somewhere. Plain
   emojis show until then.
5. During the test: RPG → Players (who's where), Economy (gold in and out, materials piling up),
   Screens (exactly what any player sees), and the XP, loot and gold rates in Settings for
   tuning without a deploy.

### What the code does

| Part | Where |
|---|---|
| Content and every balance number | `src/data.rs` |
| Combat, expeditions, progression (no Discord, seeded) | `src/engine/` |
| A whole player played for 30 days | `src/engine/sim.rs`, `examples/sim.rs` |
| Tables, the transaction per click, the ledger | `src/store.rs` |
| Every screen and action | `src/game.rs` |
| Screen model to Components V2 and to HTML | `src/ui.rs` |
| Banner (signed URL, cached), emoji set | `src/banner.rs`, `src/emoji.rs` |
| Dashboard | `src/web.rs` |

Tools: `cargo run --release -p feat-rpg --example sim -- boss` (boss win rates per pairing),
`-- days` (30-day pace and economy), `--example trace -- athena thor 380 3` (one fight turn by
turn), `--example clickbench` (bot work per click), `--example banner -- <dir>` (draw banners),
`--example preview -- 3101` (dashboard with sample players in four states).

### Balance at launch (from the simulator)

- First boss: light player (two long runs and a few fights a day) on day 10, heavy player
  (about six hours a day) on days 6 to 8, for every patron.
- Barrow King at its recommended 420 Power with all four facts scouted: Thor 90%, Athena 85%,
  Hades 100% of fights won; without scouting 14 to 74%; at 340 Power about 0%.
- Every material is used by a recipe; spare materials and gold go to the altars.
- Tests pin these: `boss_pace_for_every_patron`, `lesser_vows_help`, `pace_matches_the_plan`,
  `every_item_has_a_use_and_a_source`, and the click-through in `tests/flow.rs`, which also
  checks every screen against Discord's message limits (40 components, unique custom ids).

### Changed from the concept while building

- Thunderclap stuns a foe that's winding up (cancelling the attack) instead of hitting a pack:
  fights are one foe at a time.
- Athena's Aegis Storm throws the whole blocked hit back, not half: half made the pair weaker
  than Athena alone.
- New players start with an Iron Hand-axe and a Wolf-hide Vest, two potions and two rations:
  with level 1's Power alone even the easiest region was too dangerous.
- Selling boss intel, guilds, trading and Titan events are after the test, as planned. The
  Goals screen points to scouting instead of buying intel.

## Second pass, after the owner's first test (2026-11-03)

Owner: progression felt slow in a way that stopped you playing (one hard-won fight as Hades,
two fights before needing potions, lifesteal barely healing); wanted a story with steps and
some choices, more content, and every button on the overview instead of a menu.

- **Fights are shorter and you keep playing.** Hunt monsters have less HP (`HUNT_HP`); a win
  heals 8% (15% at first, too much for the owner); HP refills in 15 minutes instead of 45; the inn heals at once for gold. Hades'
  lifesteal is 20% (40% of that against the boss) and Soul Rend heals 60%. Thor guards 60%.
  Measured: a new player's fight is 7 to 10 turns and costs 4 to 38% HP before the win heal.
- **Pace after it** (simulator): light player at the boss on day 9, heavy on day 4 or 5.
  Boss at 420 Power with intel: Thor 97%, Athena 85%, Hades 100%.
- **The saga** (`src/quests.rs`): the Loom frame (the Norns and the Fates weave every saga;
  someone cut a thread, you relive the sagas to find who) around Midgard's 12-chapter saga,
  "The King Under the Mound". Goals use what you already do. Three chapters end in a choice,
  never back to back; options are sidegrades (a test checks their worth is within 2×). The
  völva's choice picks a boss ally: Eydis blocks the first slam, the völva reveals a fact, the
  jarl's men add 8% damage. The ending hints at the culprit (a golden apple and mistletoe) for
  the next realm.
- **Content:** four monsters per region; a rare elite per region (6%, triple rewards, a title
  on the first kill, announced in the channel); encounters after hunts (merchant, shrine, lost
  traveller, buried cache), both options worth having; three daily bounties with a bonus for
  all three; titles worn on the banner, on their own line under the name.
- **Overview:** two rows of buttons (Hunt, Expedition, Saga, Forge, Goals / Pack, Temple,
  Skills, Bounties, Help), no menu.
- **Testing:** Settings → Test speed (×10, ×60, ×240 for expeditions, healing and Vigor);
  Players → a player → set level, give items, finish their expedition, make them boss-ready,
  jump their saga to a chapter, heal.

## Third pass: a core loop (2026-11-03)

Owner, after the second test: much better, but it needs a core loop and something to grab
attention besides hunts and expeditions; a tutorial that explains what, why and how; gear is
too basic; too many buttons; elite messages are spammy.

- **The loop:** expeditions and hunts bring materials, the forge makes gear, gear takes you
  deeper into the Labyrinth, the Labyrinth gives loose threads to make gear better. The saga and
  the Barrow King sit on top.
- **The Labyrinth of Daedalus** (`src/labyrinth.rs`): under the Loom, where cut threads fall.
  Pick one of three doors per floor (fight, hard fight for double haul, treasure that may be
  trapped, shrine that heals); the Minotaur guards every fifth floor. Leave any time with
  everything; fall and keep half (gear found is lost). Wins heal only 8%. Weekly server board
  of deepest floors. Simulated depth: a new player about floor 4, mid-game about 7, boss-ready
  about 13 (`--example labsim`).
- **Gear rarity and traits:** Common, Fine, Rare, Epic; each step adds a trait (Fierce,
  Sturdy, Vampiric, Keen, Steady, Warding, Lucky, Seeing, Flame- or Frost-touched) and 4% Power.
  Forged gear rolls rarity; hunts drop gear 3% of the time, elites always, the guardian always,
  the boss always (Rare or Epic). Loose threads refine a piece one rarity up (adds a trait) or
  reforge its traits. Salvaging Fine or better gives threads; a full pack (20 spare pieces)
  salvages new drops into threads.
- **Categories (2026-11-03, task #8):** the overview's "Right now" lines carry their own
  button (Hunt, Plan one, Collect, Continue, Read), and one row underneath opens a menu each:
  Combat (Titan, raid, hunt, expedition, Labyrinth, dungeons, map), Character (stats, gear,
  forge, pack, skills, titles, help), Quests (saga, townsfolk side quests, bounty board,
  goals), Skagi (market, temple; the button opens the temple directly when the market is
  off) and the guild. Screens inside a menu go back to it ("← Combat", "← Quests",
  "← Character", "← Skagi").
- **Tutorial:** six steps shown on the overview with what, why and how; each pays 25 gold, all
  of them 2 potions, 100 gold and a title. Fights explain themselves until your first win.
  Skippable from Help.
- **Announcements:** elites only on a player's first kill of that elite. Every one goes out
  (the old limit of one post per server every 10 minutes hid most of them) and the bot deletes
  it after 30 minutes, so the channel doesn't fill up. "Delete them after" in the RPG settings
  changes that; 0 keeps them.

## Fourth pass: the big build (2026-11-03, overnight)

Owner: make it good enough to ship without shipping; more content, more weight to the
mechanics, guilds, boss battles, server-wide events, dungeons, raids, maybe a map. Bloat is
fine: they test it, cut what they don't like, and we go through what's left.

Every system below has a switch on the dashboard (RPG, Settings, Modules), so cutting one is a
click before it is a code change. Switched off, its buttons and announcements disappear and
its saved progress stays.

| Switch | What it is |
| --- | --- |
| Labyrinth | Floor-by-floor runs (third pass) |
| Dungeons | Three fixed delves, one per region: the Drowned Hall (level 5), the Wolf-King's Den (11), Hrolf's Treasury (17). Four rooms then a boss with its own pattern. First clear each day opens the chest: gear (Fine or better), 2 loose threads, the region's rare material, gold. Fall and keep half of what you carried |
| Guilds | Found one for 500 gold (name and tag in a form), up to 12 members. Guild XP comes only from what members do; levels give small shared perks (up to +5% gold and a title). The treasury only buys upgrades (Mead Hall, War Banner, Longship), never power for one member. Weekly challenge: 12 wins per member pays the treasury. **Controls (task #10):** the leader's Manage page sets Open or Invite only (players ask, the leader accepts or declines, or invites a player who isn't in a guild; asks and invites last 7 days, 3 open asks per player), a weekly guild-XP minimum (none, 50 to 1,000; members who were in all last week and fell short are marked and listed first to kick; nothing is automatic), and kicks (a kicked player can't join or ask that guild for 3 days unless invited). Find a guild lists guilds strongest first, 6 a page, with search by name or tag (a form) and an Open-only filter. The overview shows asks to leaders and invites to players |
| Guild raids | The leader spends 1,500 from the treasury to wake Fafnir. 7,000 HP per member, 72 hours, 2 attempts a day of 10 turns each. Everyone who hit claims when it falls (a smaller reward if it gets away) |
| Titans | Server-wide: one of Jormungandr, Typhon or Hrimthurs rises every 72 hours for 48 (staff can raise one now from the dashboard). 9,000 HP per active player, 3 attempts a day. The one announcement that bypasses the 10-minute quiet gap |
| Map | A drawn map of Midgard on the overview's Map screen: open regions, dungeon doors, where your expedition is, a Titan in the sea when one is up |
| Saga | The Loom saga (second pass) |
| Side quests | Skagi's townsfolk, six short lines with their own small story: Asa the fisherman's widow, Eydis the shieldmaiden, Jarl Sigrun, Old Hakon the hunter, Bragi the skald, the völva. Each step pays gold, XP or supplies; each line ends with a title |
| Bounties | The daily bounty board (second pass) |
| Market | Skagi's daily stall: three lots of materials for the tiers you'll forge next, and supplies with daily limits (no finished gear since the fifth pass) |
| Encounters | Random events on hunts (second pass) |
| Elites | Elite monsters and their once-per-player announcement |

Mechanics given more weight in the same pass:

- **Fights read better:** critical hits say so; guarding a heavy blow from a normal foe is a
  perfect guard and the next hit lands 50% harder. Not against the Barrow King: he is balanced
  around guarding his slam already, and with it Hades won 60 of 60 fights at 80% of the Power
  he needs.
- **Allies:** the saga's choice at the völva picks who stands with you once in the Barrow
  King's fight (the völva, Eydis, or the Jarl's men).
- **Dashboard test tools:** a player view (give gold, make them boss-ready, move their saga chapter, heal); Raise a Titan now; the
  module switches; a preview seeded with a guild (Ravenhold), a Titan, a raid and side quests.

Still untested in Discord itself: the guild form (a modal), the Titan announcement, and the map
image through the public address. The click-through tests cover each flow in the database.

## Fifth pass: the forge, and things opening one at a time (2026-11-04)

Owner: polish and improve, change the core, fix balancing, make the forge more important, and
features should unlock slower, not all at once.

**What opens when** (`src/unlocks.rs`, the table is `data::UNLOCKS`). A new player has hunts,
their character and the saga. The rest opens by level, or sooner when the saga gets there, and
the overview says so once ("New", with a button to take a look); the level-up line names what
just opened, menus end with "Next: … at level N", and Help lists what's still to come.

| Level | Opens |
| --- | --- |
| 2 (or saga chapter "Iron and Bread") | Expeditions and the map |
| 3 (or chapter "The Jarl of Skagi") | The forge; encounters after hunts |
| 4 | The bounty board (it only asks for what's open) |
| 5 | The temple; elites |
| 6 | Dungeons |
| 7 | Skagi's townsfolk |
| 8 | The Labyrinth |
| 9 | The market |
| 10 | Guilds |
| 12 | Titans |

A locked screen or button shows the overview with "X opens at level N". Anything already
started stays usable (an expedition out, a run, a guild joined before this). Players from
before this pass have every unlock marked as seen, so no news cards flood them.

**The forge is where gear comes from.**

- **Smith rank 1 to 10** from everything made at the forge (gear, tempering, supplies,
  upgrades, refining, salvaging, Sigrun's order). Each rank improves the forge's rarity odds
  (rank 1: 60% Common, 1% Epic; rank 10: 6% Common, 36% Rare, 10% Epic, better than any hunt
  drop), raises how far your own work upgrades (+1 at rank 1, +5 at rank 8), and the better
  recipes need a rank. The forge screen shows the rank and what the next one brings.
- **Tempering:** forge a piece with a chosen trait by adding a rare material (wolf fang for
  Fierce or Keen, sea amber for Flame, Lucky, Seeing or Warding, frost hide for Frost or
  Sturdy, loose threads for Vampiric or Steady). The piece is at least Fine, and the chosen
  trait rolls in the top half of its range.
- **Upgrades** go to +5 at 7% each (was +3 at 10%); past +3 they take loose threads too.
- **Found gear** (elites, dungeon chests, Labyrinth guardians, Titans, the boss) upgrades to
  +2 only; the pack says "found". Ordinary hunts no longer drop gear; their rare-material
  chance went from 6% to 9% instead.
- **Eight new pieces** fill the gaps, so there's a next tier every three or four levels per
  slot: Bearded Axe (4), Wolf-tooth Sax (10), Draugr-bane Greataxe (16); Studded Jerkin (4),
  Scale Hauberk (11), Barrow-plate (17); Rune Stone (6), Seidr Bindrune (12). The forge shows
  the next tier above what you wear in each slot.
- **The market** sells three lots of materials a day for pieces you could forge soon, not
  finished gear.
- **Sigrun's order** (`src/orders.rs`): one job a day on the forge screen, supplies or a
  spare piece you made, for gold, smithing and (for a piece) a loose thread.

**Balance.**

- The simulator now counts bounties, the saga and the townsfolk. Without them it said a
  light player hit level 5 on day one; with them the old curve put a heavy player at level 12
  on day one and the level cap on day three, which is why everything opened at once. New XP
  curve `80 × level^1.6` (was `55 × level^1.4`); bounties pay `40 + 15×level` XP (was
  `60 + 25×level`). Simulated: light player level 5 after day one, 8 on day three, 10 on day
  five, boss on day 12 to 16; heavy player level 8 after day one, boss on day 6 or 7.
- Hades hunted for free (0 to 3% HP a fight): lifesteal 20% → 15%, Soul Rend heals 45% (was
  60%) with a one-turn cooldown. Wolfwood at level 8: Hades 13% HP a fight, Athena 23%, Thor
  30%. Barrow King at 420 with intel: Thor 97%, Athena 85%, Hades 96%.
- Judgement with Hades as main heals half the drain (it healed all of it and shielded it):
  Hades+Athena at 340 Power won 94% of King fights, now 42%.

Dashboard: a player's smith rank shows on the Players tab and can be set there.
