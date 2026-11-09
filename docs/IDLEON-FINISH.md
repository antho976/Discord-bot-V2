# Idleon account check: finishing pass

The owner's goal (2026-11-09) is a finished `/p/{guild}/idleon` page that they skim at the end. The work for that:

- Research what players do first today (Reddit, Steam guides), and use it.
- Fix the tiers.
- Merge cards that belong together, the way Stamps (Levels / Unlock) and Brewing / Pay2Win are.
- Add the mechanics that are still missing.
- Base the endgame tiers on the best saves.

Make one or two changes per commit, so the owner can revert any one of them. Don't let that slow you down: the owner reviews everything afterwards.

The lead and integrator is **sly-puffin**. Everyone else owns one lane.

## How we work

- **Base.** Start from `origin/relay/sly-puffin`, which is PR #41 plus the shared changes. From a clean branch at main, run `git fetch && git rebase origin/relay/sly-puffin`.
- **Hand-off.** When you finish a chunk:
  1. Rebase onto `origin/relay/sly-puffin`.
  2. Run `cargo test -p feat-idleon`.
  3. Run `cargo clippy -p feat-idleon --examples -- -D warnings`.
  4. Push your branch and mail sly-puffin, through Relay `mailbox.send` and SendMessage.

  sly-puffin fast-forwards it into `relay/sly-puffin`. Don't open your own PR: the owner gets one PR with every commit in it.
- **`crowd.txt` and `crowd.gen.txt` are generated.** Never edit them by hand; hand edits there belong to the owner. Change the generator (`examples/crowd.rs`), `crowd-base.txt`, `src/crowd.rs` lists or the checks, then regenerate:

  ```
  cargo run --release -p feat-idleon --example crowd -- ~/Desktop/New\ Folder/*.json
  ```

  Those saves are real players' accounts: never copy one into the repo.
- **Rebase conflicts.**
  - `crowd.txt` / `crowd.gen.txt`: run `git checkout --ours crowd.txt crowd.gen.txt` (the upstream side during a rebase), rerun the generator and continue.
  - Shared one-line lists (`ON_HOLD`, `FOLD`, `FIRST`, `COUNTS`, `BASE_ONLY`, `FITS`, `BEST`, `BY_MAX`): keep both sides.
- **Look at your ladders.**
  - `cargo run -p feat-idleon --example ladders -- <world>` prints every ladder as an account in that world sees it. Check worlds 2, 4 and 6 as well as 7.
  - For the real page, run `./target/debug/bot preview <your port>` and POST a save. Ports: cobalt-gannet 3201, keen-badger 3202, quick-gecko 3203, violet-ocelot 3204, zesty-llama 3205.
- **Keep token use low.** The owner is watching their limits:
  - No more than one subagent per lane at a time, and leave it on Haiku.
  - Run about 10 `WebSearch` calls (standard mode) for your lane's research, not dozens.
  - Read only the parts of big files you need: `page.rs`, `pictures.rs` and `crowd.txt` are large.
- **Sources.**
  - Use search results, Reddit (r/idleon), Steam guides and the game's own data (`crates/feat-idleon/data/game.json`, extracted by `tools/idleon-extract`).
  - idleon.wiki blocks AI crawlers in its robots.txt: don't fetch it.
  - AutoReview (scoli): its authors allow using its tiers as a base if ours aren't identical. Learn from it; don't copy it.
  - Write what you learned, with links, under your lane in `docs/idleon-research.md`. Keep a short section per mechanic: what to get first, and why.

## The owner's rules (from past rounds)

- **Ladder shape.**
  - A tier never asks more than the card can show. Use `crowd::FITS` to split big steps into parts of about 8.
  - A tier that names items makes the player chase them. Where what you can level depends on luck or a rotation, tier on counts: `COUNTS` / `@any L` rows, as vials and the arcade do.
  - Bonuses worth getting as soon as possible go first, by name: `FIRST` in `examples/crowd.rs`.
- **Wording.**
  - Unlocks say "Unlock", never "Lv 0 → Lv 1".
  - Use plain words. Nothing that reads as AI-written.
- **When the order is real.** Once a card's tiers are in a real order, take it off `ON_HOLD` in `page.rs`. While it's on the list, the card says "Order coming later".
- **Merged cards** use `FOLD` in `page.rs`, a switch between sides as on Brewing / Pay2Win. Only merge what a player thinks of as one thing.
- **UI work.** Load the `house-style` skill before any change to how the page looks, and keep `docs/DESIGN.md`'s phone rules.

## Lanes

Each lane covers the files and cards listed below. The work in each lane:

1. Research what players do first for each card.
2. Fix every card's tiers: order, size and early worlds, using the research.
3. Take cards off `ON_HOLD`.
4. Fold cards that belong together.
5. Add the mechanics the game has that we don't check yet.

| Lane | Session | Files | Cards |
|---|---|---|---|
| Shared | sly-puffin | `examples/crowd.rs` core, `src/crowd.rs`, `src/review.rs`, `FOLD` code in `page.rs` | The endgame from the best saves, World 6 / early-world gaps on every card, folds of 3 or more, integration |
| W1–2 | cobalt-gannet | `checks/w1.rs`, `checks/w2.rs` | statues, statue-flair, bribes, star-signs, stamps, owl, forge, anvil, vault, alchemy, sigils, post-office, islands, fractal-island, killroy |
| W3–4 | keen-badger | `checks/w3.rs`, `checks/w4.rs` | construction, salt lick, prayers, atoms, death note, equinox (2), library, refinery, sampling, worship, armor sets, meals, kitchens, lab (2), breeding (4), rift (2), cape-vine |
| W5–6 | quick-gecko | `checks/w5.rs`, `checks/w6.rs`, `checks/caverns.rs` | sailing (3), divinity (2), slab, gaming, snail, farming (4), jade, sneaking, emperor, summoning (3), caverns (8) |
| W7 | violet-ocelot | `checks/w7.rs` | spelunking (4), clamworks, coral, gallery (3), menagerie (4), research/observations, minehead, zenith, legend talents, the rest |
| Account-wide | zesty-llama | `checks/general.rs`, plus new account-wide checks | cards, achievements, gem shop, event shop, greenstacks, secret path, class levels, storage, bags, carry capacity, and missing account-wide systems: Tome, obols, family, guild, star talents, companions and the like |

Status lives in Relay (`session.intent`) and in the commits. When you finish your lane, tell sly-puffin, then pick the next undone item from your own list ("added later" ideas). Don't start on another lane's files.
