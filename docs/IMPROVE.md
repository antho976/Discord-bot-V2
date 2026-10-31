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
- [ ] Move repeated helpers into the kit: the status pill, the header popover (`details.pop`
      with a button), tabs with counts, a compact switch row for rails, a mini bar chart, and
      "account made N ago" (duplicated in guard and audit). Then point the features at them, in
      small commits that coordinate with each area's session.
- [ ] `ui::action_button` with `data-confirm` replaces the button text, which breaks icon-only
      buttons.
- [ ] Overview: the feed takes the 6 newest items, so a busy feature hides the others. Mix the
      sources fairly.
- [ ] Overview: "Messages today" and "On pace" use the UTC day while the page shows local time.
      Say so or align them.
- [ ] Notifications page in the new style (icons per feature, links, read/unread).
- [ ] Edit access page: group by server, and make it quick to give someone the same access as
      another person.
- [ ] Accessibility pass: `aria-current` on the active nav item, focus rings everywhere, labels
      on icon buttons, contrast of `--ink-3` text on veils.
- [ ] Cost of one overview load: count the queries and keep it small.

### B: brisk-otter
`crates/feat-alerts`, `crates/feat-utility`, `crates/feat-embeds`.
- [ ] Stream alerts: show the real Twitch thumbnail where one is stored, and check every state
      (not connected, polling, push, live, offline, no schedule).
- [ ] Schedule, YouTube and Streams tabs: polish to match the Twitch tab.
- [ ] AFK and timezones: city names for zones that aren't "Region/City", and members whose zone
      is invalid.
- [ ] Embed builder: the preview column is narrow next to the editor. Balance them like the
      design. Check long titles, many fields and editing a sent message.

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
- [ ] Levels: the Rewards tab with role colour dots, editing a reward's level or role in place, and
      a warning on rewards the bot can no longer give (deleted role, or above the bot's role).
- [ ] Welcome: previews and cards with long names, other scripts and no avatar; the Roles tab with
      a deleted saved role.
- [ ] Code review of all three crates: unwraps on Discord data, silent errors that matter, missing
      tests (curve maths, invite attribution when two invites tick at once, ticket numbering).

### D: keen-tapir
`crates/feat-guard`, `crates/feat-audit`, `crates/feat-connect`.
- [ ] Scam protection: store who decided and an optional reason when someone releases or bans,
      and show it. Check the message tester and the settings tab at phone width.
- [ ] Audit log: search reads the JSON summaries. Make search match what the page shows. Add
      "by <moderator>" to the #mod-log posts now that the moderator is known.
- [ ] Audit log: check that "by <moderator>" matching holds when Discord sends the audit entry
      before or after the event, and when several happen at once.
- [ ] App approvals: review the pairing and expiry flows for edge cases.

### E: teal-marten
`crates/feat-community` (giveaways, polls, suggestions), `crates/feat-pets`.
- [ ] Polls: live counts miss votes cast while the bot was offline. On start, and when a poll
      page is opened, catch up from Discord's poll voters endpoint for open polls (rate-limit
      aware).
- [ ] Giveaways: let the creator pick an emoji for the prize tile.
- [ ] Suggestions: vote timestamps so "Most votes given" can be per month.
- [ ] Pets: polish the Approvals, Giveaways, Catalog and Bans tabs to the Collection tab's
      standard. Restyle the public pet page in the same look.

### Lead: sharp-hare
- Review every pull request (correctness, scope, look) and leave comments; check CI.
- Cross-cutting checks: every page at 390 px, copy consistency, dead routes.
