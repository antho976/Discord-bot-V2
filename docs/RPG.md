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
