# Bot V2

The Discord bot for Antho's servers, rewritten in Rust to use as little memory as possible. One
binary runs the Discord connection and the dashboard; Caddy and Relay's cloud engine run beside
it in the same container, like antho-s-bot.

## What it does

Each feature is switched on per server on the dashboard (Admin, Features). Everything starts off.

| Feature | What it does |
| --- | --- |
| XP and levels | XP from messages, voice and reactions; level-up messages; reward roles; `/rank`, `/leaderboard`, `/xp`. Imports XP from antho-s-bot. |
| Stream alerts | Twitch live and offline posts (EventSub), schedule reminders and a weekly schedule image, YouTube uploads, a small stream history. |
| Scam protection | Scam-message detection and a honeypot channel: deletes the messages, times the member out, alerts staff with Release and Ban buttons. |
| Welcome | Welcome and goodbye messages with a card, auto-roles, roles back on rejoin, invite tracking. |
| Audit log | Edits, deletes, joins, leaves, role and nickname changes, bans and timeouts, posted to a channel. |
| Community | Giveaways, native Discord polls, suggestions with votes and statuses. |
| Tickets and starboard | Support tickets as private threads; a starboard for popular messages. |
| Pets | Idleon companion collection with approvals, pet giveaways, public pet pages. |
| App approvals | Relay, Tally, Avex and your PCs connect after an approver allows them by DM. |
| AFK and timezones | `/afk`, `/timezone`, `/time`. |
| Embed builder | Write embeds on the dashboard and send or edit them in a channel. |

Later: YouTube upload rewards, prestige and streaks, Twitch clips, modmail, channel rules, custom
commands, the Idleon guild tools.

## Dashboard access

Sign in with Discord. Nobody gets in until an admin adds them on **People**, and then they see only
the pages you grant: no access, view or edit, per page and per server. `OWNER_ID` is always an
admin. Every change is listed on **Activity** with who made it.

## Running it

```sh
# set the variables below (a .env file works with your shell's dotenv tool)
cargo run -p bot        # bot + dashboard on http://localhost:3001
cargo run -p bot -- preview   # the dashboard with sample servers, no Discord needed
```

`bot help` lists the maintenance commands.

### Environment

| Variable | |
| --- | --- |
| `DISCORD_TOKEN` | The bot token. |
| `DISCORD_CLIENT_ID`, `DISCORD_CLIENT_SECRET` | The application's OAuth2 id and secret, for dashboard sign-in. |
| `OWNER_ID` | Your Discord user id. Always a dashboard admin, and the first approver for app connections. (`TALLY_OWNER_ID` is read too.) |
| `PUBLIC_URL` | The dashboard's address, e.g. `https://anthos-bot.onrender.com`. On Render it's picked up automatically. (`PUBLIC_BASE_URL` is read too.) |
| `DATABASE_PATH` | Default `data/bot-v2.db`; `/data/bot-v2.db` in the image. Separate from antho-s-bot's `/data/bot.db`. |
| `TWITCH_CLIENT_ID`, `TWITCH_CLIENT_SECRET` | From dev.twitch.tv. Needed for Twitch alerts and the schedule. |
| `TWITCH_EVENTSUB_SECRET` | Any random string of 10 to 100 characters. With it and an https `PUBLIC_URL`, Twitch pushes live events; without it the bot checks every 2 minutes. |
| `RELAY_PUBLIC_URL` | Relay's public address; `off` runs the bot without Relay. Set automatically on Render. |
| `RELAY_BIN` | Path to the relay binary. Default `relay`. |
| `LEGACY_DB_PATH` | Where antho-s-bot's database is, for the one-click XP import. Default `/data/bot.db`. |

### Discord application settings

- **Bot, Privileged Gateway Intents:** turn on Server Members and Message Content.
- **OAuth2, Redirects:** add `<PUBLIC_URL>/auth/callback`.
- **Bot permissions:** Manage Roles, Manage Messages, Moderate Members, Ban Members, Manage
  Server (invite tracking), View Audit Log (who banned, kicked or timed out someone), Create
  Private Threads, Manage Threads, Send Messages, Embed Links, Attach Files, Read Message History,
  Add Reactions, Manage Nicknames (AFK prefix). Keep the bot's role above the roles it hands out.

## Switching over from antho-s-bot on Render

V2 is built to replace antho-s-bot on the same Render service, keeping the disk. Relay's data
(`/data/relay`) and the old database (`/data/bot.db`) stay where they are.

1. Point the Render service at this repository. The Dockerfile is at the root.
2. Environment:
   - Keep `DISCORD_TOKEN`, `DISCORD_CLIENT_ID` and `DISCORD_CLIENT_SECRET`.
   - Add `OWNER_ID`, or keep `TALLY_OWNER_ID`.
   - Add `TWITCH_CLIENT_ID`, `TWITCH_CLIENT_SECRET` and `TWITCH_EVENTSUB_SECRET` if you want Twitch.
   - `DATABASE_URL`, `AUTH_SECRET`, `DISCORD_GUILD_IDS` and `DISCORD_DISABLED_FEATURES` are no
     longer used.
3. Add `<your Render URL>/auth/callback` to the Discord application's OAuth2 redirects.
4. Deploy, sign in, and switch features on per server.
5. XP: open **Levels, Import**. Since the old database is on the same disk, there's an
   **Import from the old bot** button. Reward roles and XP settings come too.

### If you'd rather import from a file

- Upload a backup `.db` file from antho-s-bot's dashboard on **Levels, Import**.
- Or run `bot import-xp <file>` locally against the V2 database.
- Without antho-s-bot's dashboard, open its Render **Shell** and print the XP as JSON:

  ```sh
  cd /app && node -e 'require("@libsql/client").createClient({url:"file:/data/bot.db"}).execute("SELECT guild_id, user_id, xp, messages, voice_minutes FROM levels").then(r=>console.log(JSON.stringify(r.rows.map(x=>({guild_id:String(x.guild_id),user_id:String(x.user_id),xp:Number(x.xp),messages:Number(x.messages),voice_minutes:Number(x.voice_minutes)})))))'
  ```

  Then copy the output into a `.json` file and upload that.

### antho-s-bot's "Server error, problem with the server configuration"

That page comes from Auth.js, which antho-s-bot's dashboard uses for sign-in. It means its
configuration is incomplete, almost always one of these:
- `AUTH_SECRET` is missing on Render.
- `DISCORD_CLIENT_ID` or `DISCORD_CLIENT_SECRET` is missing.
- The OAuth2 redirect `<url>/api/auth/callback/discord` isn't registered in the Discord app.

Render's Logs tab shows the exact cause on a line starting with `[auth][error]`. V2 doesn't use
Auth.js.

## Memory

Measured on 2026-10-30, release build in preview mode while serving dashboard pages and rendering the
schedule image: 20 MB resident, 27 MB at peak. For comparison, antho-s-bot's Next.js alone used
220 MB. A live gateway caching two servers' members adds some on top. The binary is 20 MB with the
fonts and the time-zone database built in.

The bot runs as one Rust process with two worker threads and a single SQLite connection. The
gateway cache holds servers, channels, roles, members, voice states and the last 25 messages per
channel. All periodic work runs on one minute timer. Twitch pushes events instead of being polled
when `TWITCH_EVENTSUB_SECRET` is set.

## Not tested against the real services yet

Everything builds, 152 tests pass, and every dashboard page was checked in the preview against the agreed designs. The parts
that talk to Discord, Twitch or YouTube have only run against sample data, so check them first
after deploying:
- Slash commands, buttons and DMs.
- Twitch EventSub. Twitch's follower total needs a user token, so follower stats stay empty.
- The YouTube feed.
- The relay pairing subprocess.
- The Docker image. Docker wasn't available where this was built.

## CI and deploys

GitHub Actions (`.github/workflows/ci.yml`) runs clippy with warnings as errors, the tests and a
Docker image build on every pull request and every push to `main`. Render deploys `main`. In the
service's settings, set Auto-Deploy to **After CI checks pass** so a red build never ships
(`render.yaml` says the same for a Blueprint).

## Code

- `crates/core` is the shared runtime: config, SQLite, the gateway, interaction helpers, and the
  dashboard framework and UI kit.
- `crates/feat-*` holds one crate per feature.
- `crates/render` turns SVG into PNG for the generated images.
- `crates/app` is the binary.

`docs/FEATURES.md` explains how to write a feature.
