# Dashboard design

The dashboard was redesigned on 2026-10-31 after the owner found the calm V1 look dull. The
reference is Relay design 16, artboard **A3 · Pulse, with life**: the server's banner fading into
the page, faces wherever people appear. V1 is kept on the `design-v1` branch and in Relay design
17. Run `bot preview` to see every page with sample pictures.

## The look in one paragraph

The dashboard wears the server. A blue-black ground; the server's own colour (`--accent`, sampled
in the browser from the server icon) lights the active page, the primary button, switches, focus
and the generated banner art; lilac (`--data`) carries every chart and meter; each sidebar group
has its own hue, so a page is found by colour and its header, icon tiles and empty states glow in
it. Pictures everywhere: the banner (Discord's, an uploaded one, or art drawn from the icon),
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
| `--accent`, `--accent-hi`, `--accent-lo`, `--accent-wash`, `--accent-glow` | The server's colour: primary buttons, active states, switches, focus |
| `--hue` | The current group's colour. Set by `data-hue="streams|community|members|safety|pets|tools|core"` on any element; the page sets it on `main.page` |
| `--signal` | Coral: counts that wait on someone, the "Needs you" pill |
| `--live` `--waiting` `--held` | Green, amber, red: state only |
| `--data`, `--data-2`, `--data-dim`, `--data-wash` | Lilac to violet: charts, meters, podium, levels |
| `--ease` (expo out), `--spring` (overshoot) | Every transition uses one of these |
| `--sans` Geist (400 to 700), `--mono` Geist Mono | Mono only for figures, times, ids, code |

Never a raw grey in feature CSS. A colour that isn't a token is fine when it is the subject's own
(Discord's embed grey `#2b2d31`, Twitch's live red, a role's colour).

## Shell

- **Server rail** (72 px, far left): the bot's mark, then each server's icon as Discord draws its
  guild bar (round, squircle on hover, a white pill for the current one), then the bell with its
  count and your avatar, which opens Features, People, Activity and Sign out.
- **Sidebar** (252 px): the server's banner card (name, online state, member count), the search
  button, then Overview and the pages in groups. Each group has a hue swatch and folds like a
  Discord category; the browser remembers folds. Each page has a 26 px icon tile tinted in its
  group's hue; the current page's tile fills with the hue and a glowing pill marks it (the pill
  glides to the next page during a view transition).
- **Search palette**: ⌘K / Ctrl K / `/` or the search button. Pages, servers and actions, fuzzy
  matched with the matched letters lit, recent pages first, arrow keys and Enter.
- **Phone** (under 920 px): a top bar (menu, server, search, bell); the rail and sidebar slide in
  as a drawer.
- **Page header** (`ui::layout`): a soft glow of the group's hue behind it, a 48 px icon tile
  filled with the hue, the title (24 px, 600), the page's one line, then actions. One primary
  button at most.
- **Overview** (`ui::layout_custom`) draws the A3 hero: banner photo (or `ui::art`), server icon
  in a ring of its colour, the greeting, then stat cards that count up, the area chart, "Needs
  you" and "Around the server", with the right column panel.

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
