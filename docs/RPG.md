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
  heals 15%; HP refills in 15 minutes instead of 45; the inn heals at once for gold. Hades'
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
  all three; titles worn on the banner.
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
- **Categories:** the overview has Hunt, Expedition, Labyrinth, Saga, then Hero (stats, gear,
  pack, skills, titles, help) and Skagi (forge, temple, bounty board, goals).
- **Tutorial:** six steps shown on the overview with what, why and how; each pays 25 gold, all
  of them 2 potions, 100 gold and a title. Fights explain themselves until your first win.
  Skippable from Help.
- **Announcements:** elites only on a player's first kill of that elite, and the RPG posts at
  most once per server every 10 minutes.
