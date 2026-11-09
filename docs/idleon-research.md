# Idleon research

What players do first, per card, with sources. One section per lane (see IDLEON-FINISH.md).

## W1–2 (cobalt-gannet)

Early-world tiers for these are in `crates/feat-idleon/crowd-research.txt`. The World 7 columns
still come from the saves.

### Post Office
- **Get first:** Non Predatory Loot Box (drop rate) on every character. Then Civil War Memory Box
  (base damage), Deaths Storage Unit (weapon power) and Utilitarian Capsule (multikill, sample
  size). Construction Container (build rate, shrine charge) is worth having on every character.
  After that come the class boxes for each character's skills, then the rest. Box of Gosh,
  Scurvy C'arr'ate and the Myriad Crate come last.
- **Why:** Levels 1 to 200 give most of a box's bonus, so put core boxes at 200 before taking
  any to 400. Every character gets the same points, so a character that's behind can catch up
  right away.
- **How the card reads it:** one row per box, at its lowest level on any character. In 72 of
  the 119 saves, every character has put some points into the post office. Most World 7 players
  still leave points unspent on a few characters, so "on every character" asks for something
  they can do now.
- **Calibration:** the World 5–6 saves have only about 100 to 600 box points. The early tiers
  ask for about 550 points per character by Late World 6.
- Source: [Idleon Post Office Guide – Box Priority Order](https://gameslikefinder.com/article/idleon-post-office-guide/)
  (per-class order: skilling, then DMG / Fight / Utility2, then Drop / Divine / Gaming / Scurvy /
  Cons / Myriad). I matched those short names to boxes using the bonuses in `PostOffUpgradeInfo`.

### Arcade
- **Get first:** Gold Ball Gain, because it makes every later buy cheaper. After that, Drop
  Rate and Refinery Speed (S tier). Late: Prima Bubble, Emperor, Lumi, Meritocracy.
- **Why:** the shop sells a rotation of nine upgrades, so the tiers count levels across any
  upgrades rather than naming them (`COUNTS`). This matches the #41 ladder, so the card is off hold.
- Source: [Idleon Arcade Shop Priority](https://gameslikefinder.com/article/idleon-arcade-shop-priority/)

### Sigils
- **Get first:** sigils show one after another, and each costs more than the one before
  (`SigilDesc`: 2 to 30,000 hours to unlock, 100 to 250,000 more for Yellow). The tiers go in
  game order, with Yellow on the cheap early sigils before you unlock the late ones. Red needs
  Ionized Sigils in the Jade Emporium, and costs 50,000 or more, so it starts in World 7.
- **Calibration:** at Early World 7, half the saves have 21 sigils unlocked and about 13 Yellow.
  The World 4–6 tiers climb toward that, starting at Mid World 4 when the lab node opens.
- I found no guide that ranks sigils. The searches turned up only how they level.
  [DigitalTQ alchemy](https://www.digitaltq.com/wiki/idleon/alchemy).

### Islands and Fractal Island
- **Order:** Trash (more bottles), Crystal (Time Candy, class XP), Shimmer (weekly trials),
  Seasalt (World 6 fishing spot), Rando, then Fractal. All of them open in World 2 once you have
  Fishing 30.
- **Fractal:** the Master of Nothing trophy chance (24 hours) is the only early goal. The rest
  is endgame: half of Early World 7 saves have about 320 hours.
- **Folded** into one card: Islands / Fractal Island.
- Source: [Idleon Island Expeditions Guide](https://gameslikefinder.com/article/idleon-island-expeditions-guide/).
  [DigitalTQ](https://www.digitaltq.com/wiki/idleon/islands-yum-yum-desert) says the order
  doesn't matter much.

### Killroy
- **Order:** one point in each fight as it shows, then Timer (toward 160), then Skulls and
  Respawn evenly. Opens in World 2. The second room comes in World 4.
- **Skull shop:** of the permanent buys we check, Artifact Find Chance (World 5 sailing) comes
  first. The guide's S tier (3rd room, Gallery grade) is World 7 or a weekly buy.
- **Calibration:** at Early World 7, half the saves have Timer 19 and Skulls 2. The early tiers
  stay under that.
- Source: [Idleon Killroy Guide](https://gameslikefinder.com/article/idleon-killroy-guide/)

### Bribes
- Bought with coins, one set at a time (each "Art of the..." bribe opens the next set). The
  tiers ask for a set per world, by its prices: Set 4 is 100M to 20B, Set 5 ends with the Jade
  Emporium's New Bribes (World 6), and Set 6 runs to 1e23. A 2023 note from the developer says
  to buy the new bribes early if you see random events often.

### Star Signs
- Chronus Cosmos first (two signs at once), then unlock everything: points now cover every
  sign. The current base (AutoReview) already asks for this.
- Source: [Idleon Best Star Sign](https://gameslikefinder.com/article/idleon-best-star-sign/)

### Not W1–2
- Bubba (meat slices, Megaflesh) is a World 3 system, so it belongs in the W3–4 lane.
  [Idleon Bubba Guide](https://gameslikefinder.com/article/idleon-bubba-guide/)

## W3–4 (keen-badger)

## W5–6 and caverns (quick-gecko)

Few guides rank World 5 and 6 upgrades; the ones that do are below. Where none does, the order
comes from the game itself (fixed unlock orders) or from the 2 World 5 and 7 World 6 saves we have.

### Sailing (artifacts, islands, boats, boat upgrades)

- **Artifacts:** no guide ranks them. Which artifact a chest drops, and in which form, is luck, so
  the tiers count artifacts at a form (any of them). Fauxory Tusk (artifact find chance, from the
  first island) and Genie Lamp (chest value) are asked for by name first: both speed up the rest.
  Forms open in order: Ancient, Eldritch (30 rifts), Sovereign (Jade Emporium), then World 7 ones.
  ([DigitalTQ, Sailing](https://www.digitaltq.com/wiki/idleon/sailing))
- **Islands:** uncovered in order of distance; the tiers ask for the first N.
- **Boats:** more boats, then speed before loot ("faster trips mean more chests"), and keep the
  loot pile from filling. Boat Upgrades is a new card: loot and speed levels over every boat.
  ([DigitalTQ, Sailing](https://www.digitaltq.com/wiki/idleon/sailing))

### Divinity (gods, blessings)

- **Gods** unlock in a fixed order through offerings. Arctis matters most early: it frees your
  characters from the World 4 Lab; Goharut next, so Divinity and Lab level together. Keep one
  character on Purrmep (2x Divinity EXP and points).
  ([Games Finder, Divinity](https://gameslikefinder.com/article/idleon-divinity-guide/))
- **Blessings:** no guide ranks them. The World 5 and 6 columns follow the saves: Snehebatu,
  Harriep, Arctis and Purrmep highest.

### Gaming (superbits, snail)

- **Superbits** open along a fixed path; Bits Per Achievement is first and boosts bits. The tiers
  ask for the first N. ([DigitalTQ, Gaming](https://www.digitaltq.com/wiki/idleon/gaming))
- **Immortal Snail** costs 611B bits, so it's a World 6 buy, after Little Sprinkler (Gaming 20).
  Imports (Sprinkler, Pong Controller, Shovel, Squirrel, Seashell...) aren't checked yet: their
  maxes aren't in our game data.

### The Hole and the caverns

- **Villagers:** Polonai first: his levels find the caverns that hold the other villagers
  (Motherlode for Kaipu, The Bell for Cosmo, The Lamp for Minau, Evertree for Bolaia).
- **Schematics:** Kaipu unlocks them in a fixed order and most players build them as they open:
  Opal Dividends, Better Buckets, Loadin' some 'Lode, Cavern 'Porting, Bar Expand-o-rama, 2nd Bucket
  first. **Majiks:** Cosmo, Enhance! first (it lifts every majik past level 1), then Opal
  Enthusiasm and Monumental Vibes.
  ([DigitalTQ, The Hole](https://www.digitaltq.com/wiki/idleon/the-hole))

### Farming (crops, market, land rank, exotic)

- Push crop evolutions first, then the Day Market: Land Plots, Stronger Vines, Product Doubler,
  More Beenz, Nutritious Soil, Biology Boost, Rank Boost, Smarter Seeds. Move on when an upgrade
  takes over a day. Night Market: Overgrowth and Land Rank as soon as they open, then OG
  Fertilizer, Speed GMO and Evolution GMO evenly. Land Rank: Production, Soil EXP, Overgrowth,
  Farming EXP, Evolution. Exotic Market (World 7): Freexotic, Better Exotic, Datadigging first.
  ([Games Finder, Farming](https://gameslikefinder.com/article/idleon-farming-guide/))

### Sneaking (upgrades, emporium, charms, gemstones)

- **Upgrades:** nearly all are worth it; buy the cheapest first, skip Star Sweeping, stop Kunai
  Knowledge once every character is untied. **Emporium:** a fixed shop order, three on offer at
  a time; the card now lists it in that order. **Pristine charms:** collect them early, while
  repeats are rare; they drop at random, so the tiers count them. **Gemstones:** to 10K first,
  then toward 101K; Garnet and Topaz last.
  ([Games Finder, Sneaking](https://gameslikefinder.com/article/idleon-sneaking-guide/))

### Summoning (upgrades, battles, stones)

- Cheaper upgrades and essence first: Cost Laundering, Sell Sell Sell, Cost Deflation, Cost
  Crashing, then each colour's Knowledge and Essence rows; for battles, damage before utility
  before health. Stones by boss HP: White, Green, Purple, Blue, Red, Yellow, Cyan.
  ([Games Finder, Summoning](https://gameslikefinder.com/article/idleon-summoning-guide/),
  [DigitalTQ, Summoning](https://www.digitaltq.com/wiki/idleon/summoning))

## W7 (violet-ocelot)

110 of the 119 saves have reached World 7, so every W7 card takes its tiers from the saves (7e, 7m and 7l, then the endgame). The guides below were used to check that order, not to replace it. Reddit threads didn't come up in search, so the sources are guide sites and Steam discussions.

**Research grid.** Get the Research EXP multipliers and the nodes that open the minigames first, then magnifiers, then roll chance. A Games Finder guide's foundation order is: K7 (Research EXP), J6 (Mr Minehead Linguistics), K8, K9 (a big EXP multiplier), J8 (Tiny Cogs), K10/L10/L11 (points and new observations), M9 (Kaleidoscope), then J5/J4/J3 (Glimbo, Sushi), and L8 (Optical Monocle). Other guides name All Night Studying, See 'Em All and Takin' Notes as the early EXP picks, and Sharp Eye, Smart Eye and Observationalistic for rolls once EXP is steady. The 7e saves already ask for these first: See 'Em All, All Night Studying 2, Takin' Notes 3, the three Linguistics nodes and Sharp Eye.
- [Games Finder: Research guide](https://gameslikefinder.com/article/idleon-research-guide/)
- [Treyex: Research skill guide](https://www.treyexgaming.com/idleon-research-skill-guide/)
- [Grind'n'Strat: Research skill guide](https://grindnstrat.com/idleon-research-skill-guide/)

**Observations and insight.** Raise your best observations to insight 2 or 3 with Optical Monocles before you put a Magnifying Glass on them, then spread the rest of the insight evenly. Kaleidoscopes go next to the glasses. Magnifiers come from Minehead opponents 3 and 13, Posty Notes and the event shop.
- [Games Finder: Research guide](https://gameslikefinder.com/article/idleon-research-guide/)

**Spelunking shop.** POW first, then stamina, then amber and discovery upgrades. Within each group, level upgrades until they cost about the same. The POW upgrades sit in the shop's first column, the amber ones in the third. Lore: read chapter one (The Fear Within) every day for efficiency, and chapters two (Decay) and six (Kelp) for POW and amber as you go. Bosses need a lot of POW and stamina (Greg about 200M POW and 500 stamina; the Exiled Throne boss about 3e32 POW and 3,000 stamina).
- [Games Finder: Spelunking guide](https://gameslikefinder.com/article/idleon-spelunking-guide/)
- [shnaps: progression guide (GitHub)](https://gist.github.com/shnaps/161a370ed795e6141e0553eb30ddc8fa)

**Minehead.** Both guides push Miney Farmey I (the currency upgrade) first, along with Golden Tiles and Boom Blocker. One adds Base Damage, Grid Expansion and Legal Cheating Button. Wait for a good round (4–5 Golden Tiles, 2 cheating buttons left, Boom Blocker unused) before pushing. Beating an opponent gives a bonus for the whole game, which is why Opponents is now its own side of the card.
- [Games Finder: Minehead guide](https://gameslikefinder.com/article/idleon-minehead-guide/)
- [DigitalTQ: Mr. Minehead guide](https://www.digitaltq.com/wiki/idleon/mr-minehead-guide)

**Sushi Station and Jelly Operator.** Sushi: Superior Sushi Skillz early, since it lets you cook the next tier straight away. Each new sushi type gives a permanent bonus, so cooking new types is the point (now the Types side). Jelly: Daily Operating Procedure (a research node) for more operations a day, then Daily Transfusion to its max. Each obstruction cleared gives a permanent bonus (now the Obstructions side).
- [DigitalTQ: Sushi Station](https://www.digitaltq.com/wiki/idleon/sushi-station)
- [DigitalTQ: Jelly Operator](https://www.digitaltq.com/wiki/idleon/jelly-operator-guide)
- [Vortex Gaming: Sushi Station update](https://vortexgaming.io/en/postdetail/728609)

**Legend Talents.** Yellow 1 (Super Talent Points) is the safest first pick everywhere. Games Finder adds Yellow 5 and 6 and Brown 1 as high priority. Older guides call the green row the strongest because it has the multipliers, and warn against additive talents when points are few. The shrine talent is the one they disagree on.
- [Games Finder: Legend Talents](https://gameslikefinder.com/article/idleon-legend-talents-guide/)
- [Vortex Gaming: Legend Talent picks](https://vortexgaming.io/en/postdetail/587519)

**Clamworks, Glimbo, Coral.** No guide ranks these. What they do agree on: Clamworks upgrades open as you hold more pearls (Pearl Value at 20, up to Anti Inflation at 2B), and Frugality makes the rest cheaper. Glimbo trades raise Upgrade Vault caps, and every 100 items traded adds 1% drop rate with the i4 research node. The saves' order stands.
- [DigitalTQ: The Clamworks](https://www.digitaltq.com/wiki/idleon/the-clamworks)
- [DigitalTQ: Glimbo's Swap Meet](https://www.digitaltq.com/wiki/idleon/glimbos-swap-meet-trades)

**Menagerie and Gallery.** No priority guide either. One tip: put the Lumi multiplier first, because extra missions can be bought with currency anyway. The Idle Skiller trophy is strong in the Gallery.
- [Games Finder: Event shop tier list](https://gameslikefinder.com/article/idleon-event-shop-guide/)

**Mechanics added.** The game's own tables have three more lists that give a permanent bonus per step, and none of them was checked:
- Minehead Opponents, 32 of them (`Research[7][4]`)
- Sushi Types, 64 (`Sushi[5]`)
- Jelly Operator Obstructions, 72 (`Research[7][9]`)

Each is now a side of its card.

## Account-wide (zesty-llama)

Searches: 9 standard WebSearch calls on 2026-11-09. Reddit and Steam turned up little for these
cards; the guides below are the best sources found. The game's own data (`Tome`, `GuildBonuses`,
`CompanionDB` and the obol tables, from N.js) fills the rest.

### Gem shop

Buy first ([DigitalTQ gem shop tier list](https://www.digitaltq.com/wiki/idleon/gem-shop-tier-list), April 2025):
- Extra Card Slot, all 4, as soon as you can.
- Infinity Hammer (two anvil crafts at once).
- Ivory Bubble Cauldrons and Bleach Liquid Cauldrons, all 4 of each (Bleach in order).
- Crystal 3d Printer once you reach World 3.
- Card Presets.

Then buy where you're stuck: Brimstone Forge Slot, Royal Egg Cap (World 4), Richelin Kitchen
(only the kitchens you use), Souped Up Tube, Divinity Sparkie, Golden Sprinkler, Lava Sprouts,
Plot of Land. Premium stones, card packs and arcade balls come last.

### Event shop

Order from the [gameslikefinder event shop tier list](https://gameslikefinder.com/article/idleon-event-shop-guide/) (2026):
1. Stamp Stack, Bubble Broth, Extra Lumi.
2. Golden Square, Sleepy Joe, King of the Rack, Secret Pouch, Smol Pouch, Magnifying Glass, Graduate Thesis.
3. More Storage, Storage Chest, Dough Roller, Government Subsidy, Coin Stacking.

Golden Tome is better once the Tome score is high. Automated Mail and Extra Page come last.

### Tome (World 4, account-wide)

The game data has 124 lines (`Tome`). Each line:
- has a target and a points max, and is scored on one of five curves (N.js `TomePCT`).
- unlocks at a total of class levels: 350, then 40 more per line in the in-game order
  (`NinjaInfo[32]`), with steeper steps after the 35th, 60th, 80th and 100th lines.

Lines on the "more is better" curve keep giving points past their target, up to 1.45 times their max.

The score unlocks bonuses ([DigitalTQ: The Tome](https://www.digitaltq.com/wiki/idleon/the-tome)):
- skill efficiency past 4,000 points
- drop rate past 8,000 points

Players use it as the one number for a whole account, which makes it the account-wide card most worth having.

### Obols

- Fill the family page first. It opens at 5,000 total class levels.
- Then fill each character's own slots. The last one opens at class level 250.
- Drop rate obols everywhere is the simple plan.

Source: [gameslikefinder obol guide](https://gameslikefinder.com/article/idleon-obol-guide/).

### Star talents

Order from the [gameslikefinder special talents guide](https://gameslikefinder.com/article/idleon-special-talents-guide/):
1. Tick Tock and Crystals 4 Dayys.
2. Stonks!, but only to Lv 30.
3. Supersource, Action Frenzy, Ancient Multitool, Statue Metallurgy, Studious Quester (as far as your quests justify), Frothy Malk.
4. Printer Sampling: 1 point only.

### Cards

- 5 stars open at Rift 46, 6 stars with Spelunking ([gameslikefinder card builds](https://gameslikefinder.com/article/idleon-card-builds/)).
- The Bosses / Nightmares set is worth having from early to the end.
- Which cards drop is luck, so card tiers count stars on any cards rather than naming them.

### Guild

No player guide on guild bonus order turned up. Bonus levels are in the save (`Guild[0]`, against
`GuildBonuses`), but only the guild's owner spends guild points, so a member can't act on it.
Low priority.

### Companions

IdleonToolbox's export has them at the top level (`companion`), outside the game save. The bot's
save reader drops that part today, so a Companions card needs a change there first.

### What players do first, account-wide

A [r/idleon thread](https://cal1.lr.ggtyler.dev/r/idleon/comments/1fkvk50/looking_for_early_w6_account_help/lnzxg5u/?context=3)
(Sep 2024, early World 6) says to clean up first: slab, bubbles, constellations, stamps, obols.
Then push the Rift and farm vials.
