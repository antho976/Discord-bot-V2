# Dashboard design

The dashboard's look was agreed with the owner screen by screen (Relay design 16, "Dashboard
overview") and is built in the code as it stands on `main`. `docs/ui/` holds a screenshot of every
page from `bot preview`: that is the reference. Improve within this look; don't replace it.

## The look in one paragraph

Near-black and calm, like Claude's apps. One page surface inset from the window, off-white ink, a
coral accent only for what needs the person, green/amber/red only where they mean state, and
lilac for data (charts, meters). No boxes around passive content: air and hairlines separate
things, and cards only group settings or a right-hand column. Warmth comes from people and
pictures (member avatars, the server banner, stream thumbnails), not from colour. Copy is dry and
specific.

## Tokens (`crates/core/src/web/assets/app.css`)

| Token | Use |
| --- | --- |
| `--ground` #070707, `--page` #0f0f0f | Window and page surface |
| `--ink`, `--ink-2`, `--ink-3` | Text: primary, secondary, captions (`--ink-3` is 55%, the lowest that passes contrast) |
| `--well` `--veil` `--control` `--hover` `--selected` | Veils of ink at fixed strengths (inputs, cards, keys, hover, selected) |
| `--edge-1` `--edge-2` `--edge-3` | Hairlines, from faint to strong |
| `--signal` #f0806a | Coral: only for what needs the person (counts waiting, alerts). Never decoration |
| `--live` `--waiting` `--held` | Green, amber, red: state only |
| `--data` #c3a6ff, `--data-dim`, `--data-wash` | Lilac: charts, meters, the podium |
| `--sans` Geist, `--mono` Geist Mono | Mono for figures, times and ids |

Never a raw grey in feature CSS: use the tokens.

## Shell

- **Sidebar:** the server switcher, a page search (`/` jumps to it; it matches names and each
  page's `about`, Enter opens the first match), Overview, then the pages in folding groups
  (Streams, Community, Members, Safety, Idleon, Tools), 30 px rows. The group holding the current
  page starts open and the browser remembers the others; a folded group shows a coral count of
  unread notices inside it. Features, People and Activity sit in the menu under the signed-in
  person at the bottom, with Sign out. Option A of three, picked by the lead on 2026-10-31 when the owner left it to us (Relay
  design 16, page "Sidebar"), because the flat list ran past the screen.
- **Page header** (`ui::layout`): a 40 px icon tile with the page's own icon, the title (20 px,
  600), one line on what the page does (`NavItem::about`), then actions and the bell. One filled
  primary button at most; status pills (`span.status > span.dot`) for connection states.
- **Overview** (`ui::layout_custom`) draws its own banner header instead.
- **Icons:** the custom set in `ui::Icon`, page icons on a 24-unit grid at 1.75 stroke. A new page
  gets its own icon drawn in the same style, not a near miss.

## Building blocks

| Block | Where it's used |
| --- | --- |
| Underline tabs (`ui::tabs`), with a count where something waits | Every feature with sub-pages |
| `div.with-rail` + `aside.rail` + `div.rail-block` (each starts with an h2) | A quiet right column: totals, who, what's next |
| `div.mini` rows (avatar or icon, name and detail, mono figure) | Rail lists |
| Timeline (`.timeline`, `.change`, day headings in the viewer's timezone) | Activity, audit log |
| `.need` rows with an outlined button | Overview "Needs you" |
| Faces (`img.avatar`, `ui::avatars` overlapping) | Wherever a person appears. Always `ctx.member_avatar` |
| `ui::area_chart` (lilac curve), `.meter` (lilac bar) | Data |
| Discord-style previews (message, embed with coral edge, APP tag) | Live post, welcome card, embeds |
| `details.pop` + `.pop-body` | A header or row action that needs a small form |
| `.seg` | A small segmented choice (None / View / Edit, 7 / 30 days) |
| Settings: `ui::settings_form` with `ui::card` and `field_*` rows | Every settings tab. View-only people get it disabled |
| `ui::empty` with the section's icon and one line on how it fills | Every list |

Feature-specific CSS lives in the feature crate (`fn style()` with classes prefixed by the
feature, e.g. `.gv-`, `.cm-`, `.au-`). Shared patterns belong in `app.css` and `ui.rs`.

## Rules that hold everywhere

- One answer per page, above the fold. Readings first when numbers are the answer.
- Show the reading with the verdict: "Timed out 2 h ago · link imitates steamcommunity.com".
- Honest at zero: every empty state drawn, no "N/A" or dashes. Real data only, never invented.
- Undo over confirm: destructive buttons ask for a second click (`data-confirm`), no dialogs.
- Times in the viewer's timezone (`page.user.local(ms)`).
- Works at 390 px wide: the rail stacks under the main column, secondary columns hide.
- Text contrast at least 4.5:1, meaningful marks at least 3:1.
- Copy: dry, specific, in "you". No exclamation marks, no em dashes, no hype, no explaining the
  obvious. Machine ids never render.
- Leave alone what works: polish and deepen, don't redesign.
