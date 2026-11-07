# Idleon account check app: first readings, 2026-11-07

The desktop app (`crates/idleon-app`, iced 0.14) on the same machine as the page's readings:
AMD Ryzen 7 9800X3D, CachyOS, release profile (`opt-level = "s"`, thin LTO). Harness:
`cargo run --release -p idleon-app --example bench -- <save> 30`, which drives the app without a
window: the same `view` the window calls, laid out by iced's own `UserInterface` with a
software renderer, the layout cache kept between steps as the app keeps it. Two runs on the
owner's save (796 KB) differed by up to 8% (11.4 vs 12.3 ms whole read); the numbers below are
the second run, plus the biggest of the 120 saves (1.36 MB).

## Budgets

- A click (open a card, switch world, follow a Should have line, Show all): the window's thread
  builds and lays out the page inside one 60 Hz frame, **under 16.7 ms**, so it never drops a
  frame.
- Reading a save: off the window's thread; the review on screen **within 100 ms** of the paste
  or drop, so it feels instant.

## Readings

| Step (where it runs) | p50 | p95 | max |
|---|---|---|---|
| Read a save, whole (background thread), 796 KB | 12.3 ms | 12.7 ms | 12.8 ms |
| ... of which parse / checks / review | 4.4 / 3.5 / 1.9 ms | | |
| ... of which tiers / Should have by now | 0.24 / 0.88 ms | | |
| Read a save, whole, 1.36 MB | 11.4 ms | 12.1 ms | 16.7 ms |
| First read of a run (game data 2.7 ms, cold caches) | 16.6 ms | | |
| The server's tiers put in place at start (window thread, once) | 2.2 ms | 2.2 ms | 2.3 ms |
| First frame of a review: view / layout | 0.35 / 5.4 ms | | |
| Open a card: view / layout | 0.13 / 0.07 ms | 0.14 / 0.10 ms | 0.17 / 0.13 ms |
| Switch world: view / layout | 0.14 / 0.62 ms | 0.16 / 0.67 ms | |
| Follow a Should have line: view / layout | 0.13 / 0.07 ms | 0.14 / 0.14 ms | 0.15 / 0.67 ms |
| Show all (Stamps, 122 rows): view / layout | 0.19 / 0.74 ms | 0.22 / 0.79 ms | 0.25 / 4.84 ms |
| Peak memory of the bench (reads, game data, software renderer) | 37 MB | | |

Every click is under 1 ms at p95 against a 16.7 ms frame; a save is on screen ~13 ms after it
arrives (plus one frame). The first layout of a review (5.4 ms) shapes every line of text once;
later layouts reuse it.

## What makes it that fast (kept on purpose)

- Reading runs on its own thread (`app::background`), never on the window's.
- Each mechanic's tiers and Should have by now are worked out once per read (`app::Ready`), not
  on every frame: the page's own code calls `Mechanic::tiers()` again for every line.
- A picture's handle is made once and kept (`pics.rs`): iced decodes per handle, so a new handle
  per frame would decode the PNG every frame.
- Show all lists are only built while open, like the page's.

## Not measured, with why

- **Start to first window frame, and drawing.** This machine's Relay session may not open
  windows, and the headless harness has no GPU. The app prints both when run with
  `IDLEON_CHECK_TRACE=1` (`[trace]` lines on stderr: boot, review ready, each frame, view
  times); run it once on the target PC and add the numbers here.
- **Windows.** Built by `.github/workflows/idleon-app.yml` on GitHub's Windows runner, not
  timed.
- **Memory of the windowed app** (GPU buffers, glyph atlas): read it from the trace run's task
  manager.
