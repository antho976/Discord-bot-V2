# Dashboard design

The dashboard was redesigned on 2026-10-31 after the owner found the calm V1 look dull. The
reference is Relay design 16, artboard **A3 · Pulse, with life**: the server's banner fading into
the page, faces wherever people appear. V1 is kept on the `design-v1` branch and in Relay design
17. Run `bot preview` to see every page with sample pictures; `docs/ui/` holds a screenshot of
each (1440x900, plus the overview at 2560 and 390 px). Glows were taken out and the layout made to fill
wide screens on the same day, after the owner's first look.

## The look in one paragraph

The dashboard wears the server. A blue-black ground; the server's own colour (`--accent`, sampled
in the browser from the server icon) lights the active page, the primary button, switches, focus
and the generated banner art; lilac (`--data`) carries every chart and meter; each sidebar group
has its own hue, so a page is found by colour: its header tile and icon tiles take it. Pictures
everywhere: the banner (Discord's, an uploaded one, or art drawn from the icon),
members' faces, stream thumbnails. Motion is part of the material: pages slide between each other
(cross-document view transitions), charts draw in, bars grow, numbers count up, buttons lift and
press. Everything respects `prefers-reduced-motion`. Copy stays dry and specific.

## Tokens (`crates/core/src/web/assets/app.css`)

| Token | Use |
| --- | --- |
| `--ground` #07070b, `--page` #0c0c12, `--raised`, `--solid` | Window, page surface, raised blocks, menus |
| `--ink`, `--ink-2`, `--ink-3` | Text: primary, secondary, captions (`--ink-3` is the lowest that passes contrast) |
| `--well` `--veil` `--control` `--hover` `--selected` | Veils of ink (inputs, cards, keys, hover, selected) |
| `--edge-1` `--edge-2` `--edge-3` | Hairlines, faint to strong |
| `--accent`, `--accent-hi`, `--accent-lo`, `--accent-wash` | The server's colour: primary buttons, active states, switches, focus |
| `--hue` | The current group's colour. Set by `data-hue="streams|community|members|safety|pets|tools|core"` on any element; the page sets it on `main.page` |
| `--signal` | Coral: counts that wait on someone, the "Needs you" pill |
| `--live` `--waiting` `--held` | Green, amber, red: state only |
| `--data`, `--data-2`, `--data-dim`, `--data-wash` | Lilac to violet: charts, meters, podium, levels |
| `--ease` (expo out), `--spring` (overshoot) | Every transition uses one of these |
| `--sans` Geist (400 to 700), `--mono` Geist Mono | Mono only for figures, times, ids, code |
| `--gutter` | Page padding, 24 to 44 px with the window |
| `--z` | The page zoom on big screens (below); divide any `vh` height by it |

No glows: no `box-shadow`, `drop-shadow` or `text-shadow` blur in a hue (dark depth shadows and
0-blur rings are fine), no radial colour washes behind cards or headers, no gradient or
rainbow meters. Cards are flat. The owner found the glows too much, twice. Pictures (the
banner, generated server art, the giveaway art tile) are the only soft colour.

Never a raw grey in feature CSS. A colour that isn't a token is fine when it is the subject's own
(Discord's embed grey `#2b2d31`, Twitch's live red, a role's colour).

## Width and big screens

The page fills the window; nothing is capped at a fixed width and parked at the left.

- From 2200 px the whole page is zoomed (`zoom` on `:root`): 1.125, then 1.25 from 2800 px and 1.5
  from 3300 px, so a 1440p or 4K screen doesn't show it tiny. Each step still lays out wider
  than the one below. `--z` holds the factor: heights in `vh` are written `calc(100vh / var(--z))`
  because zoom scales `vh` too. app.js measures the factor before placing fixed tooltips.
- `.page-body` and each non-rail column of `.with-rail` are size containers named `col`. A block
  rearranges with `@container col (min-width: ...)` for the width it really has.
- `.with-rail`: the right rail is `clamp(300px, 24%, 420px)`.
- `.compose`: settings beside the post or preview they shape (`.compose-preview`, which takes
  the right column and stays in view). Two columns from 1000 px of column, stacked below.
- `.tiles` (`--tile`): cards that multiply as the width grows instead of stretching.
- Hierarchy: header, then the page's answer full width, then the work. Edges line up; an action
  sits on the block it acts on. A Discord preview keeps Discord's width, so something useful
  sits beside it rather than a void.

## Shell

- **Server rail** (72 px, far left): the bot's mark, then each server's icon as Discord draws its
  guild bar (round, squircle on hover, a white pill for the current one), then your avatar,
  which opens Features, People, Activity and Sign out.
- **Sidebar** (252 px): the server's banner card (name, online state, member count), the search
  button, then Overview and the pages in groups. Each group has a hue swatch and folds like a
  Discord category; the browser remembers folds. Each page has a 26 px icon tile tinted in its
  group's hue; the current page's tile fills with the hue and a pill in the hue marks it (the pill
  glides to the next page during a view transition).
- **Search palette**: ⌘K / Ctrl K / `/` or the search button. Pages, servers and actions, fuzzy
  matched with the matched letters lit, recent pages first, arrow keys and Enter.
- **Bell** (`.page-corner`, top right of every page): a dropdown of the newest notices, loaded
  when opened (`/g/{id}/notifications/peek`, which marks them seen), with "See all".
- **Phone** (under 920 px): a top bar (menu, server, search, bell); the rail and sidebar slide in
  as a drawer.
- **Page header** (`ui::layout`): a 48 px icon tile filled with the hue, the title (24 px, 600),
  the page's one line, then actions, left of the bell. One primary button at most.
- **Page swaps**: links and forms inside the dashboard fetch their page and replace only
  `<main>` (app.js `visit`). The sidebar, rail and their scroll stay; the pill glides; counts
  update; a refused save is a toast. Anything else (sign-in, another server, a file, a link
  with `target`, `download` or `data-full`) is an ordinary visit. Page setup lives in app.js
  `mount`. A feature's inline script runs again after a swap, so it never waits for
  `DOMContentLoaded` and passes `window.botPage.signal` to timers and document listeners.
- **Save** sits at the end of its form (`.save-bar`). While a settings form has changes and its
  Save is out of view, a Discord-style bar at the bottom offers Reset and Save.
- **Refusals**: a post the server turns down (`WebError::Bad`, or any error status) shows its
  reason in a popup by the button that asked, never on a page of its own. Write the reason as
  what to do: "Pick a channel for reminders and save first."
- **Send a test**: wherever the bot posts on its own, the block showing that post has a test
  button (POST route, `Level::Edit`, audited, marked as a test, nobody pinged, saved settings).
  Inside a settings form it's a `formaction` button. It returns with `?sent=1`; render
  `ui::sent_toast(sent, "Test sent")`.
- A form that changes the sidebar (switching a feature, the banner) carries `data-full` so it
  loads the whole page.
- **Overview** (`ui::layout_custom`) draws the A3 hero: banner photo (or `ui::art`), server icon
  in a ring of its colour, the greeting, then stat cards that count up, the area chart, "Needs
  you" and "Around the server" (side by side when wide), with the right column panel. The stat
  cards and 7 / 30 days switch the chart in place (`data-swap`, below), so only the chart redraws.

## Building blocks

| Block | Notes |
| --- | --- |
| `ui::tabs` / `ui::tabs_counted` | Underline tabs; the lit line glides between tabs across pages |
| `ui::readings` | Reading cards with a hue hairline on top; numbers count up when given `data-count` |
| `b data-count` | Any number whose text is like `1,284`, `66 h`, `+42` counts up when it scrolls into view. The attribute is reserved for this: app.js rewrites the element's text, so never use `data-count` to carry other data |
| `.card` | Groups settings rows or a block of related things. Never nest cards |
| `div.with-rail` + `aside.rail` + `div.rail-block` | The right column: totals, who, what's next |
| `div.mini` | Rail line: avatar or icon, name and detail, figure |
| `.need` | A row that waits on you: avatar(s) or a hue icon tile, text, an outlined button (primary when it's an alert). Hover slides it right |
| `ui::area_chart`, `ui::bar_chart`, `ui::column_bars`, `ui::mini_bars`, `.meter` | Charts draw in; bars grow one after another; hovering any bar or day shows a tooltip (`data-tip="Label: value"`, read by app.js) |
| `img.avatar`, `ui::avatars` | Faces; overlapping stacks fan out on hover. Always `ctx.member_avatar` |
| `ui::art(icon, still)` | Generated server art when there's no banner |
| `ui::empty` | Icon in a hue tile that floats gently, title, one line on how it fills |
| `.badge.ok/.warn/.bad/.accent` | Tinted pills for state |
| `.status > .dot.live` | Connection pill; a live dot pings |
| `.live-tag` | Twitch's red LIVE tag over a stream thumbnail |
| `.stream-card` | 16:9 thumbnail with a caption gradient; zooms slowly on hover |
| `.btn`, `.btn.primary`, `.btn.outline`, `.btn.ghost`, `.btn.danger`, `.btn.small` | Lift on hover, press on click; primary is the server's colour with a sheen; a submitted form's button spins (`.busy`) |
| `details.pop` + `.pop-body` | Small popover forms; they open with a short scale-in |
| `.seg` | Segmented choice; the lit part slides |
| `.switch` | Springy thumb; on is the server's colour |
| `.chip` | Toggle chips; checked ones take an accent wash |
| `[data-tip]` | Any element: a tooltip on hover (`Label: value` puts the value in bold) |
| `a[data-swap="#a, #b"]` | A link that changes part of the page: app.js fetches its page, replaces `#a` and `#b`, updates the other `data-swap` links' `href` and `class`, and keeps the address. Anything unexpected falls back to a normal visit |

Feature-specific CSS lives in the feature crate (`fn style()`), classes prefixed by the feature
(`.gv-`, `.cm-`, `.au-`). Shared patterns belong in `app.css` and `ui.rs`.

## Motion rules

- Entrances: page blocks rise 10 px out of a 3 px blur, about 60 ms apart (CSS on
  `.page-body > *`). A feature adds its own entrance only for its signature moment (the podium
  growing, a preview card arriving), not on every row.
- Use `var(--ease)` for movement, `var(--spring)` for things that pop (counts, toggles, tiles).
- Animate transform, opacity, filter, clip-path; never layout properties.
- Loops (pings, drifting art) only on things that are live or decorative backgrounds, slow and
  quiet.
- Sidebar groups animate only when opened by hand, never on a page load.
- Coming back to the same page (another filter in the address, or after a save) is `html.same`:
  no page slide and no entrance, numbers don't count again, and only charts draw. The head
  script in `ui.rs` decides it from the referrer.
- `prefers-reduced-motion` turns all of it off (already handled globally).

## Rules that hold everywhere

- One answer per page, above the fold. Readings first when numbers are the answer.
- Faces and pictures wherever people or streams appear. A list of people without avatars is
  unfinished.
- Show the reading with the verdict: "Timed out 2 h ago · link imitates steamcommunity.com".
- Honest at zero: every empty state drawn. Real data only, never invented (preview seeds are
  labelled sample data).
- Undo over confirm: destructive buttons ask for a second click (`data-confirm`), no dialogs.
- Times in the viewer's timezone (`page.user.local(ms)`).
- Works at 390 px wide: the rail stacks under the main column, secondary columns hide.
- Text contrast at least 4.5:1, meaningful marks at least 3:1.
- No gradient text, no eyebrow labels above headings, no colored side stripes on cards, no emoji
  as icons.
- Copy: dry, specific, in "you". No exclamation marks, no em dashes, no hype.
