# Improvement sweep

Make what exists better. Not new features: better code, fewer bugs, a more polished UI, and
deeper versions of the features already here.

## Scope

**In:**
- Code review: dead code, duplication, unclear names, wrong error handling, missing tests.
- Bugs: wrong results, panics, races, edge cases (empty server, deleted role or channel, member
  left, Discord errors, restarts, timezones, very long names or text).
- UI polish against `docs/DESIGN.md` and the screenshots in `docs/ui/`: spacing, alignment,
  empty and loading states, phone width, contrast, copy, keyboard access.
- Deeper existing features: a missing obvious function of something that's already here, e.g.
  reliable data behind a reading, an edit where only delete exists, a filter on a list.

**Out:**
- Anything on the Later or Cut lists, and anything new: no new commands or pages for things the
  bot doesn't do yet.
- No new automod. Scam protection and the honeypot are the only moderation features; make those
  better, add nothing next to them.
- No API, webhooks or themes. Black only.
- Nothing that costs memory or writes on every message without a strong reason. Lean stays lean.

## How to work

- **Stay in your area.** Edit only the crates listed for your session below. If something in
  another area needs changing, message that session or the lead instead of editing it.
- **Branch:** `sweep/<area>` from the latest `origin/main`. Keep one pull request open for it
  and push to it as you go. Merge `origin/main` into your branch when it moves; never force-push.
- **Commits:** one commit per one or two changes, with a message that says what changed and why.
  End each with the `Co-Authored-By` line your harness gives you.
- **Before every commit:**
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - For UI changes, `cargo run -p bot -- preview <port>` with your own port, then screenshot the
    page at 1440x900 and at 390x844 and look at it. See `docs/FEATURES.md` for the command.
- **Pull requests:** CI must pass. **Don't merge**: the owner merges in the morning, because
  merging deploys to the live bot.
- **Data:** a schema change is a NEW migration, never an edit to a shipped one. Keep old URLs
  working when a route moves.
- **Tick items here** in your branch as you finish them. Add what you find under your area.

## Areas

Ports for `bot preview`: core 3201, B 3202, C 3203, D 3204, E 3205.

### Core: brisk-heron
`crates/core`, `crates/app`, `crates/render`, `app.css`, `app.js`, the preview seed.
- [x] Move repeated helpers into the kit: the status pill, the header popover (`details.pop`
      with a button), tabs with counts, a compact switch row for rails, a mini bar chart, and
      "account made N ago" (duplicated in guard and audit). Then point the features at them, in
      small commits that coordinate with each area's session.
      Kit: `ui::status`, `ui::pop`, `ui::tabs_counted`, `ui::switch_row`, `ui::mini_bars`,
      `discord::snowflake_ms`, `text::made_ago`. The core pages use them; each feature session
      has a message listing the swaps for its crates.
- [x] `ui::action_button` with `data-confirm` replaces the button text, which breaks icon-only
      buttons.
- [x] Overview: the feed takes the 6 newest items, so a busy feature hides the others. Mix the
      sources fairly.
- [x] Overview: "Messages today" and "On pace" use the UTC day while the page shows local time.
      Say so or align them. (Said: "days start at 20:00 your time" under the chart.)
- [x] Notifications page in the new style (icons per feature, links, read/unread).
- [x] Edit access page: group by server, and make it quick to give someone the same access as
      another person. ("Same as…" copies another person's pages.)
- [x] Accessibility pass: `aria-current` on the active nav item, focus rings everywhere, labels
      on icon buttons, contrast of `--ink-3` text on veils. (`--ink-3` passes at 4.77:1 or
      better everywhere; the focus ring didn't and now does.)
- [x] Cost of one overview load: count the queries and keep it small. (Admin 21 to 16 round
      trips, 15 of them feature panels; non-admins no longer pay one query per feature and per
      unread notice.)

Found and fixed along the way:
- [x] Reading one server's notifications cleared the other server's bell (one seen time for
      all servers). Now per server, new migration `002_notif_seen`.
- [x] Chips and the None/View/Edit choice couldn't be reached by keyboard.
- [x] Popovers didn't close on a click outside or Escape.
- [x] A browser that refuses cookies reloaded forever trying to set the timezone; "Etc/GMT+5"
      style zones fell back to UTC.
- [x] `parse_duration` accepted 0 and negative spans and overflowed on huge numbers.
- [x] A form id of 0 panicked the request (twilight `Id::new(0)`).
- [x] Buttons of a switched-off feature got no answer ("This interaction failed").
- [x] Two quick feature toggles could register commands out of order.
- [x] Activity log entries showed people's ids ("Changed access for 2000…").
- [x] Overview: the 7/30 days switch wrapped at phone width; an empty server showed "On pace
      for 0" over a flat line.
- [x] `display_name` fell back to "User 123…" for members not in the cache, so ids rendered
      on the leaderboard and in notices. Now "Unknown member" (flagged by keen-hare).
- [x] Page headers at phone width squeezed the title beside a status pill (flagged by
      brisk-otter). Actions that don't fit now drop under the title.
- [x] `ui::rail_numbers` for the three-figure rail grid (community, pets; flagged by
      teal-marten).
- [x] `Ctx::notify_once` and `discord::why_refused` / `why_refused_any`, copied into levels,
      welcome and starboard (asked by the lead for keen-hare).
- [x] From keen-tapir's review of #3: header popovers opened off the left edge at 390 px;
      notices with no server had to be cleared once per server; `Ctx::known_name` so stored
      names don't become "Unknown member".

### B: brisk-otter
`crates/feat-alerts`, `crates/feat-utility`, `crates/feat-embeds`.
- [x] Stream alerts: show the real Twitch thumbnail where one is stored, and check every state
      (not connected, polling, push, live, offline, no schedule).
- [x] Schedule, YouTube and Streams tabs: polish to match the Twitch tab.
- [x] AFK and timezones: city names for zones that aren't "Region/City", and members whose zone
      is invalid.
- [x] Embed builder: the preview column is narrow next to the editor. Balance them like the
      design. Check long titles, many fields and editing a sent message.
- [x] Found: the stream `thumbnail` column was never written. Streams now keep the live preview,
      then the VOD's picture (filled in later while Twitch makes it).
- [x] Found: one broadcaster's failing schedule read stopped every other broadcaster's refresh,
      and one server's settings error stopped live-post edits for the rest.
- [x] Found: saving more than 10 YouTube channels looked each up on youtube.com before refusing.
- [x] Found: the overview promised reminder times while live, when reminders are skipped.
- [x] Found: with the [AFK] nickname prefix, notices, "Welcome back" and the dashboard showed
      "[AFK] Mira". Turning AFK off left everyone already away stuck AFK with the prefix.
- [x] Found: `/time` said "hasn't set a timezone" when the saved zone was unknown.
- [x] Twitch tab shows EventSub state: each subscription's status as of the last check, the last
      event, and a quiet warning when Twitch has a stream that no event arrived for (one Helix
      call on page load, push mode only). Polling mode shows when the last check ran.
- [x] Found: on a phone the "Twitch connected · instant alerts" pill overlapped the page title.
      The detail now drops below 600 px.
- [x] YouTube: "Send a test post" posts the newest stored video in the chosen channel, pinging
      nobody, like the Twitch test alert.
- [x] Embed builder: the form checks Discord's limits as you type (each part, field names and
      values, 6,000 in total) and on send (half-filled fields), with the server's wording. It
      offers 25 fields, Discord's limit, instead of 10. Duplicate on a template and on a sent
      message.
- [x] Found: a field added after removing another was sent out of order (rows were read by
      number, not by position).
- [x] Review pass (unwraps, silent errors, tests). Fixed: a failed read or write of "reminder
      sent" could repost a reminder every minute; failed subscription removals and follower
      writes were silent; an embed Discord accepted showed an error page when its reply couldn't
      be read; YouTube handle links with a page (`/@name/videos`), `/c/` and `/user/` links and
      Twitch links with a query didn't resolve. New tests: weeks and months across DST, the
      schedule image across a year end and on a DST Sunday, southern and half-hour DST zones,
      relative times, link parsing. Mutex `unwrap`s and `write!` into strings are left as they
      are.
- [ ] Not done: subscriptions pointing at an old `PUBLIC_URL` are never removed (they could
      belong to another instance sharing the Twitch app, so removing them needs the owner's
      call). AFK still runs one indexed DELETE per message to end someone's AFK; an in-memory
      set of AFK people would avoid it if that ever matters.
- [x] From teal-marten's review: the AFK list and timezone grid no longer depend on the member
      cache (names are kept with the command); no "missed stream" warning for streams from before
      push mode worked, and the VOD lookup is cached; the reminder preview uses the schedule's
      zone; EventSub statuses read as words; Sent rows keep two buttons on a phone.
- [ ] For core (brisk-heron): on a phone the page title wraps beside a header status pill
      ("Stream alerts" next to "Twitch not connected"). The header could drop the pill under the
      title below ~480 px.

### C: keen-hare
`crates/feat-levels`, `crates/feat-welcome`, `crates/feat-tickets` (tickets and starboard).
- [x] Tickets: "Waiting on staff" depends on the message cache. Store the last message author
      per open ticket thread (one small write per message in ticket threads only) so it survives
      restarts, and bring back "no staff reply yet".
- [x] Starboard: images. Discord image links expire, so fetch a fresh one when the page loads
      (cached briefly) rather than storing it.
- [x] Levels: search finds only cached members. Check large servers and the podium with fewer
      than 3 members.
- [x] Welcome: let invites have a friendly name ("Twitch link") set on the Invites tab, and show
      it on "Joined today".
- [x] Starboard: clearing all reactions, or one emoji, from a message left the post with its old
      count. Both now update or take down the post.
- [x] Tickets: staff can reopen a ticket closed in the last 7 days from the Closed tab (unarchives
      the thread). Closed rows show the close reason and who closed it.
- [x] Levels: the Rewards tab with role colour dots, editing a reward's level or role in place, and
      a warning on rewards the bot can no longer give (deleted role, or above the bot's role).
- [x] Welcome: previews and cards with long names, other scripts and no avatar; the Roles tab with
      a deleted saved role.
- [x] Code review of all three crates: unwraps on Discord data, silent errors that matter, missing
      tests (curve maths, invite attribution when two invites tick at once, ticket numbering).
      Found: XP sums overflowed past level ~215 (`/xp set-level`), imported settings unchecked,
      invites guessed when two ticked at once, `<#0>` on a double press, user 0 on a missing
      author, and refused welcome/level-up/starboard posts reaching only the log.

### D: keen-tapir
`crates/feat-guard`, `crates/feat-audit`, `crates/feat-connect`.
- [x] Scam protection: store who decided and an optional reason when someone releases or bans,
      and show it. Check the message tester and the settings tab at phone width.
- [x] Audit log: search reads the JSON summaries. Make search match what the page shows. Add
      "by <moderator>" to the #mod-log posts now that the moderator is known.
- [x] Audit log: check that "by <moderator>" matching holds when Discord sends the audit entry
      before or after the event, and when several happen at once.
- [x] App approvals: review the pairing and expiry flows for edge cases.
- Found and done:
  - [x] Scam protection: bans and timeout lifts done in Discord record the moderator and reason
        from the audit log, and the alert loses its buttons. Two moderators clicking at once
        can't both act.
  - [x] Scam protection: the tester added new-account points when that check was off, and its
        gauge didn't show the score.
  - [x] Scam protection and audit log: say when the honeypot, alert or log channel was deleted.
  - [x] Scam protection: filter the catch list (Waiting, Banned, Released); undecided catches
        could fall out of the newest 50.
  - [x] Audit log: the bot's own actions were credited to a moderator who acted on the same
        member a moment before. They're recorded now and kept out of the weekly rail.
  - [x] App approvals: a PC forgotten on the dashboard could claim its way back within 15
        minutes; two claims at once could pass the 5-link limit; the cloud pill disagreed with
        the API for a blank RELAY_PUBLIC_URL; say when allowing a request won't be enough.
- Known limit: Discord merges repeated message deletions by one moderator into one audit entry,
  so an author deleting their own message in the same channel within a minute is also shown as
  deleted by that moderator.
- Waiting on core PR #3, for the morning once it is on main:
  - [ ] Point guard, audit and connect at `text::made_ago`, `discord::snowflake_ms` and
        `ui::status`.
  - [ ] With #3, `display_name` gives "Unknown member" for someone not cached. Audit and guard
        store that name in rows that outlive the cache (`audit_events.user_name`,
        `guard_catches.user_name`, `acted_by_name`): use the name the event carries instead.

### E: teal-marten
`crates/feat-community` (giveaways, polls, suggestions), `crates/feat-pets`.
- [x] Polls: live counts miss votes cast while the bot was offline. On start, and when a poll
      page is opened, catch up from Discord's poll voters endpoint for open polls (rate-limit
      aware).
- [x] Giveaways: let the creator pick an emoji for the prize tile.
- [x] Suggestions: vote timestamps so "Most votes given" can be per month.
- [x] Pets: polish the Approvals, Giveaways, Catalog and Bans tabs to the Collection tab's
      standard. Restyle the public pet page in the same look.
- [x] Found: an early-ended giveaway's post said "Ended in 2 days" (the scheduled end).
- [x] Found: a second reroll could pick the winners the first reroll replaced.
- [x] Found: the poll avatars and the suggestions top voters shared a class, so phones hid
      the top voters.
- [x] Found: the pets Approvals tab counted quantities (5) where the rail and overview count
      requests (4).
- [x] Found: pets Bans "Given" and public stats "givers" read the top-ten list, so they
      undercounted.
- [x] Found: dashes for an unknown confirmation rate on pets pages; overview polls without
      their live votes; editors had no "Open in Discord" on suggestions.
- [x] Giveaways: let editors change a running giveaway's prize, end time and number of
      winners, and update the Discord message to match.
- [x] Suggestions: let staff edit their reply after deciding (it updates the Discord message).
- [x] Polls: polish the Ended tab to the hero card's standard; check ten answers and long
      answers at phone width.
- [x] Pets: check the public page and the Collection tab with 100+ pets and a member with many
      copies, for speed and layout. Keep queries indexed.
- [x] Code review pass over both crates: unwraps on Discord data, silent errors, the race when
      a giveaway ends while someone enters, missing tests.
- [x] Found: ended polls were listed by when they were posted, not when they ended.
- [x] Found: re-saving a suggestion's status re-DMed the author and reset the decision date;
      Discord had no way to edit a reply.
- [x] Found: an entry could land after a giveaway's winners were drawn, and its message
      update could revive the Enter button; a failed entry read left a giveaway ended with no
      winners.
- [x] Found: "Couldn't post" was shown for giveaways and suggestions that did post; tick read
      errors were swallowed; a double click on /pet add submitted twice.
- [x] Review (keen-hare): poll catch-up could count a vote switched during the read twice; a
      long winning answer broke the polls rail; an edit just before a giveaway ended wasn't the
      one announced, and its refresh could overwrite the ended post; pets copy for viewers and
      the public counts.

### Lead: sharp-hare
- Review every pull request (correctness, scope, look) and leave comments; check CI.
- Cross-cutting checks: every page at 390 px, copy consistency, dead routes.
