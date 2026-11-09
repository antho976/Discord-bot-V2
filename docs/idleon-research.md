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

## Account-wide (zesty-llama)
