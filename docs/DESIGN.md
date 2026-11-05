# Dashboard design

The dashboard was redesigned on 2026-10-31 after the owner found the calm V1 look dull. The
reference is Relay design 16, artboard **A3 · Pulse, with life**: the server's banner fading into
the page, faces wherever people appear. V1 is kept on the `design-v1` branch and in Relay design
17. Run `bot preview` to see every page with sample pictures; `docs/ui/` holds a screenshot of
each (1440x900, plus the overview at 2560 and 390 px). Glows were taken out and the layout made to fill
wide screens on the same day, after the owner's first look.

**2026-11-02, the house style.** The owner moved the dashboard to their `house-style` plugin
(near-black graphite, one accent, colour only in data, no boxes around passive content) and kept,
on purpose, the server's own pictures: the banner faded behind the overview header and on the
sidebar's server card, and the square server icon with its accent ring. The concepts are Claude
artifacts APKpxjRsuPrBvnTuwvDu3F (agreed) and WXt8yQnp3jnXDaE2r6m1Fc (an earlier polish pass).
Where this file and the plugin differ, this file wins for this project.

## The look in one paragraph

A graphite ground, darkest in the sidebar, off-white ink. The server's own colour (`--accent`,
sampled in the browser from the server icon) marks only what asks for you: the primary button,
counts that wait on someone, the icon's ring. Lilac (`--data`) carries every chart and meter.
Pictures are content: the banner (Discord's, an uploaded one, or art drawn from the icon), the
server icon, members' faces, stream thumbnails. Icons take their text's colour. Flat surfaces,
no glow, air instead of lines. Motion answers the person: menus open from their trigger, rows
and buttons press, the chart swaps with a short blur; the overview's chart draws in once. Copy
stays dry and specific.

## Tokens (`crates/core/src/web/assets/app.css`)

| Token | Use |
| --- | --- |
| `--ground` #0f0f10, `--page` #141415, `--raised` #19191b, `--solid` #232325 | Sidebar, page, raised blocks, menus and the lit row |
| `--ink` #ededed, `--ink-2` #a1a1a6, `--ink-3` #85858b | Text: primary, secondary, captions (`--ink-3` is the lowest that passes contrast) |
| `--well` `--veil` `--control` `--hover` `--selected` | Veils of ink (inputs, cards, keys, hover, selected) |
| `--edge-1` `--edge-2` `--edge-3` | Hairlines, faint to strong |
| `--accent`, `--accent-hi`, `--accent-lo`, `--accent-wash` | The server's colour: primary buttons, active states, switches, focus |
| `--hue` | The current group's colour. Set by `data-hue="streams|community|members|safety|pets|tools|core"` on any element; the page sets it on `main.page` |
| `--signal` | Now the accent: counts that wait on someone, the bell's count |
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

- **Sidebar** (256 px, the only column left of the page; the Discord-style server rail is gone):
  the server's card on top (its banner or generated art, icon, name, online state, member count),
  which opens the server switcher; the search button; Overview and the pages in groups, each
  group a plain sentence-case label that folds like a Discord category (the browser remembers
  folds); then the foot: your avatar and role (opens People, Activity and Sign out), the **bell**
  and, for admins, the **gear** (server settings, the Features page). Bell and gear draw at one
  optical size. Page icons take the text colour; the current page is a lit row (`--solid` with a
  hairline highlight), no pill and no group hue.
- **Search palette**: ⌘K / Ctrl K / `/` or the search button. Pages, servers and actions, fuzzy
  matched with the matched letters lit, recent pages first, arrow keys and Enter.
- **Bell** (`.side-foot`, beside the gear at the bottom of the sidebar): a dropdown of the
  newest notices that opens upward, loaded when opened (`/g/{id}/notifications/peek`, which marks them seen), with "See all".
- **Phone** (under 920 px): a top bar (menu, server, search, bell); the sidebar slides in as a
  drawer.
- **Page header** (`ui::layout`): a 44 px neutral icon tile, the title (24 px, 600),
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
- **Overview** (`ui::layout_custom`): the banner fades into the page behind the square server
  icon (accent ring), the server's name and "date · members · here today". Then the answer: four
  readings as tabs (Messages, Active members, Voice hours, Joins; each shows its number for the
  range except the chosen one), one hero figure with its change against the period before, 7 /
  30 days, and the chart (`ui::area_chart_vs`: lilac line and soft fill over a dashed grey line
  for the period before). Tabs and range switch it in place (`data-swap`). Under it, "Around the
  server" beside "Needs you" (five rows, the rest folded under "Show N more"). The right column
  takes the features' panel blocks in order: Live now (stream row with a small thumbnail, next
  stream), Most XP this week (ranked list with thin bars), Running (giveaways and polls as rows).
  Running rows carry no icon; the kind is the first word of their second line ("Giveaway · ends
  in 39 min · 9 entries"). For admins the banner itself is the upload control: clicking it picks a
  picture, "Change banner" (and "Remove upload" when there is one) shows while the pointer is on
  it. No separate banner button. Rows share one shape: 28 px face or icon, a 14 px title, a 13 px muted line, and in Needs you a
  drawn button of one width (76 px), the whole row being the link. Under 1180 px it is one
  column: chart, Live now, Needs you and the feed, then the rest.

- **Stream alerts, Twitch tab** (the second page done in the house style): the answer first and
  unboxed, the stream's picture beside who, the live line, where it posts and three figures, then
  hours streamed as a figure beside its 12 weekly bars; the week's schedule on the right. Below,
  the settings beside one Discord preview, switched between "When you go live" and "When it ends"
  (radio `.seg`, CSS `:has`). "Twitch not connected" is the header pill, which opens how to
  connect; no setup box above the page. The other tabs keep their layout, unboxed: schedule days
  and rows, the next slot lit in the data colour, figures without tiles.
  **Owner's call the same day: on the Stream alerts pages every section and side block sits in
  one bordered panel** (`--raised`, 1 px `--edge-1`, 14 px radius; unboxed felt "all over the
  place"), and Discord previews draw at 75% (`zoom`) so they don't dominate. This overrides the
  house style's "no boxes" for these pages; ask before carrying it to others.
  The Twitch tab's placement after that: one top panel in three columns (picture, who with four
  numbers and the 12-week bars, the week split off by a hairline), then the settings panel (Save
  inside it) beside the preview in a fixed 500 px column. Panels in a row match in height.
  Then "flat and plain" (owner, same day): the top panel now leads with the stream. The current or
  last stream's picture fades in behind the streamer's face (64 px, a purple ring while live),
  name and live line, with one big number on the right (watching now, or the last stream's
  average). Under it, "Average viewers per stream" for the last 20 streams as an area chart
  (`ui::area_chart_series`, last point keeps its label). The week and the four small numbers with
  the 12-week bars share the right column. **Stream pages use Twitch purple `#a970ff` as their
  data colour** (`main.page[data-hue="streams"]` redefines `--data`), for the chart, the bars and
  the next stream in the week; the server's accent stays for Save and what needs you.
  Viewer counts are stored per stream (peak, average, current), not per 5-minute reading, so a
  chart of viewers within one stream needs a new table first.
  **Sub-tabs (Schedule, Clips, YouTube, Stats) get the panels and the purple but no lead**: no
  stream picture header, no big number (owner: "don't put an emphasis like overview and stream
  alert since they are subpages"). Schedule: the next stream's date in purple, a quiet countdown,
  scheduled slots tinted purple, "Coming up" beside its side panels, then the settings full width,
  each group beside its preview (500 px). Clips and YouTube: gallery panel, then settings with the
  side blocks under them two or three across, the preview in its own column. Stats: the two
  charts end at the same height. The Clips and YouTube galleries are one strip of equal tiles about 200 px wide (no
  double tile for the newest): the owner found them taking a whole 1080p screen. Their numbers
  ride in the gallery panel's head (clips in 30 days, views, Posting; claims, XP given, Rewards,
  with the YouTube channels and where they post under the head). Below, three columns: the
  people (Who clips, Claimed the newest) on the left, the settings, the preview (500 px).
  Revised the same day (owner): two columns. Left, the gallery (cut short, tiles ~150 px, one-line
  captions) and the settings under it; right (500 px), one panel with the numbers and the people
  (Who clips / Claimed the newest, plus YouTube's channels), then the preview.
- **Giveaways** (main page, keeps its lead): the running giveaway ending soonest in a bordered
  panel, the prize's emoji on a flat `--solid` tile (no gradient, no spin or float), the clock
  changing in place each second (no digit roll), neutral role pill and "Ends in". "Also running"
  and the ended cards fill their row (`auto-fit`, 300 px minimum); the rail blocks are panels.
  Placement (owner preferred the Ended tab's): the Running tab is the same two columns. Left, the
  giveaway ending soonest (picture tile, prize, clock, time bar, entrants, actions) and "Also
  running"; right, one column of equal panels: its post as members see it, Recent winners, This
  month. The post is no longer a third column inside the lead panel.
- `ui::readings` figures and `.card` are flat everywhere now: figures stand on the page, and a
  settings card is `--raised` with a hairline, no gradient.

- **Placement pass over every other page (2026-11-02, done by six agents from one brief while the
  owner was away; Pets and Account reviews skipped, other sessions own them).** One pattern
  everywhere: two columns, the work on the left and a fixed right column (380-500 px) of equal
  bordered panels; columns end near the same height; settings beside a small (75%) preview with
  Save inside the settings panel; a lead only on a feature's main tab (a big number with a lilac
  line chart against the period before on Tickets, Modmail, Scam protection, Audit log; the podium
  on Levels; the top post on Starboard; today's joins on Welcome); rows 28/14/13 px; row buttons
  one width; nothing moves by itself, nothing counts up. Two previews that used to stack (Welcome,
  Modmail) are one at a time with a switch. Per-page notes live in the agents' reports; open
  questions: Embed builder and the custom command edit pages stretch the Discord window to the
  form's height (the alternative is a short preview that stays in view).
- **Suggestions are a box** (owner, 2026-11-02: "people send, they don't vote, I decide"). `/suggest`
  sends one to the chosen channel (meant to be staff-only) with only a Review button; the member is
  told it went to staff and gets a message when it's decided; vote buttons on older posts just say
  voting is off. The page: All leads with suggestions sent this month (waiting for you in the
  accent, approved, done) and a weekly line; Waiting for review leads with the one that has waited
  longest, large, with Approve / Considering / Deny and a reply box on it, then the rest oldest
  first. Right column: who sends the most (30 days), then the month or the newest post.
- **Moving inside a feature is quiet** (owner, 2026-11-02: changing sub-page "makes the whole page
  move"). app.js `visit` marks a swap `html.vt-soft` when the target belongs to the same sidebar
  page (`navFor`, the longest-matching link) or is the same page with another query (a tab, a
  filter, All time / This week): the page's view-transition group snaps to its new size and only
  its content cross-fades (140/200 ms), so the header, tabs and sidebar hold still; charts, bars and
  meters don't draw in again. Another page from the sidebar keeps the page-to-page transition.
- **The bell's dropdown** (owner: "this looks bad, I like it there"): 400 px, a head with the new
  count and See all, compact rows (`peek_row`: neutral icon, two lines of text at most, feature and
  "10 min ago" under it, an accent dot when new), no chevrons.
- **Round of owner notes, 2026-11-02 evening:** Starboard's lead is compact (a flat gold star
  `#f2b544` with the count, or the post's picture; gold is the starboard's own colour; the figures
  ride in the "Also starred" head). Welcome's settings are two sections (Welcome, Goodbye), each
  opened by its switch and a heading, channel beside message, card options grouped under it.
  Pets: the public Stats page is two columns (giveaways line over 12 weeks with the figures in its
  head, rarity rows comparing given and held; top givers and most given on the right); every
  catalog pet has an Edit button and collection tiles open the edit form for editors; members have
  `/pet mine` (their pets by status, withdraw or remove their own with a confirm, staff notified),
  which the submit flow points to. Custom commands take up to 4 pictures per reply (8 MB, 50 MB a
  server), sent as attachments for /slash, !text and triggers, added and removed on the edit page.
- **Server** (2026-11-04, owner: "a usage dashboard like Render's"; `docs/ui/server.webp`): one
  page for the owner and Admin+ at `/server`, linked from the account menu beside People and
  Activity, not tied to a Discord server and not in the sidebar. A bordered panel of the latest
  reading (CPU with the load, memory and disk with their totals and a thin lilac meter that turns
  amber from 90%, uptime with the day it started, CPU temperature when a sensor reads), then
  "Over the last 24 hours / 7 days" (`.seg`, swapped in place) over four equal chart panels: Server
  CPU, Server memory, Bot CPU, Bot memory, each with its average and peak in the head. Server
  memory scales to all the memory there is; CPU scales to its peak but never under 10% (1% for the
  bot). The bot's numbers are its container's (Caddy and Relay share it) and say so. One quiet line
  at the end points to Beszel for per-container detail and alerts. Not a Beszel clone.
- Shared since then: `.rail` is a plain column of bordered panels (`.rail > .rail-block`), the
  `ui::empty` icon is still and neutral, `[data-count]` numbers no longer count up, and the bell's
  notice icons are neutral with no tint on new ones.

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
| `ui::area_chart_readings` | The area chart for readings over time (CPU, memory): the hover shows a formatted value ("7.4%", "3.6 GB"), the chart names itself for screen readers, and the scale can be fixed (100%, all the memory) instead of following the peak |
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

**Current policy (2026-11-02, supersedes the entrance and loop rules below):** one block at the end
of app.css decides what moves, and wins over feature styles. Nothing enters or idles on a working
page (no rising blocks, arriving previews, pulsing or blinking dots, drifting art). Kept: charts
drawing once (line 0.9 s, fill 0.7 s, bars 0.5 s with 12 ms steps, meters 0.6 s), the levels
podium rising, the app-connection countdown, busy spinners, a refused delete shaking, popovers
growing from their trigger in 0.16 s. Between pages: a 120/180 ms cross-fade, the sidebar still.
Between tabs of one page: only what changed cross-fades and the scroll position stays. Buttons
press (scale 0.97), never lift; toggles slide without bounce; the search palette doesn't animate;
hover never zooms pictures. A feature that wants motion adds it to the policy's kept list.

- The overview is opened every day: no block entrance, no count-up, no parallax. Only its chart
  draws in on the first load.
- Entrances elsewhere: page blocks rise 10 px out of a 3 px blur, about 60 ms apart (CSS on
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
