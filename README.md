# Bot V2

The Discord bot for Antho's servers, rewritten in Rust to use as little memory as possible. One
binary runs the Discord connection and the dashboard; Caddy and Relay's cloud engine run beside
it in the same container, like antho-s-bot.

## What it does

Each feature is switched on per server on the dashboard (Admin, Features). Everything starts off.

| Feature | What it does |
| --- | --- |
| XP and levels | XP from messages, voice and reactions; level-up messages; reward roles; `/level rank`, `/level top`, `/level prestige`, and `/xp` for staff. Imports XP from antho-s-bot. |
| Stream alerts | Twitch live and offline posts (EventSub), schedule reminders and a weekly schedule image, YouTube uploads, a small stream history. |
| Scam protection | Scam-message detection and a honeypot channel: deletes the messages, times the member out, alerts staff with Release and Ban buttons. |
| Welcome | Welcome and goodbye messages with a card, auto-roles, roles back on rejoin, invite tracking. |
| Audit log | Edits, deletes, joins, leaves, role and nickname changes, bans and timeouts, posted to a channel. |
| Community | Giveaways, native Discord polls, suggestions with votes and statuses. |
| Tickets and starboard | Support tickets as private threads; a starboard for popular messages. |
| Pets | Idleon companion collection (pets and their Pet+) with approvals, pet giveaways, public pet pages filterable by rarity; rarity and bonuses come from the game via idleontoolbox.com. |
| App approvals | Relay, Tally, Avex and your PCs connect after an approver allows them by DM. |
| Server | CPU, memory, disk, uptime and temperature of the machine the bot runs on, and the bot container's own CPU and memory, read every minute and kept for a week. One page for you and Admin+, in the account menu; nothing to switch on. |
| AFK and timezones | `/afk`, and `/time now`, `/time set`, `/time clear`. |
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
| `PUBLIC_URL` | The dashboard's address, e.g. `https://anthobot.bid`. On Render it's picked up automatically. (`PUBLIC_BASE_URL` is read too.) |
| `DATABASE_PATH` | Default `data/bot-v2.db`; `/data/bot-v2.db` in the image. Separate from antho-s-bot's `/data/bot.db`. |
| `TWITCH_CLIENT_ID`, `TWITCH_CLIENT_SECRET` | From dev.twitch.tv. Needed for Twitch alerts and the schedule. |
| `TWITCH_EVENTSUB_SECRET` | Any random string of 10 to 100 characters. With it and an https `PUBLIC_URL`, Twitch pushes live events; without it the bot checks every 2 minutes. |
| `RELAY_PUBLIC_URL` | Relay's public address; `off` runs the bot without Relay. Defaults to `wss://<PUBLIC_URL host>/relay` when `PUBLIC_URL` is https (on Render, the Render host). |
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

## Self-hosting (moving off Render)

The home server (`server1` on Tailscale) runs the same image with Docker Compose (`compose.yaml`),
behind a Cloudflare Tunnel. No port is published: Docker-published ports bypass ufw, and the tunnel
dials out. The server's shell is fish, so the commands below are fish.

Never run Render and the server at the same time: two bots on one token answer every command twice
and fight over the gateway.

### Before the switch (nothing goes down)

1. **Domain and tunnel.** Buy the domain on Cloudflare Registrar. In Cloudflare Zero Trust,
   Networks, Tunnels, create a tunnel (Cloudflared) and copy its token. Add a public hostname,
   e.g. `bot.<domain>`, with service type **HTTP** and URL `bot:3000`. WebSockets (for `/relay`)
   pass through on their own.
2. **Discord.** In the application's OAuth2 Redirects, add `https://bot.<domain>/auth/callback`.
   Keep the Render one until Render is deleted.
3. **On the server:**

   ```fish
   sudo apt install -y gh; and gh auth login
   gh repo clone antho976/Discord-bot-V2 ~/apps/discord-bot
   cd ~/apps/discord-bot
   cp bot.env.example bot.env; chmod 600 bot.env   # fill in from Render's Environment tab
   echo "TUNNEL_TOKEN=<token>" > .env; chmod 600 .env
   docker compose build                             # a few minutes; do it now, not at the switch
   ```

   In `bot.env`, set `PUBLIC_URL=https://bot.<domain>`. Relay's address follows from it. Leave out
   `DATABASE_URL`, `AUTH_SECRET` and anything Render sets itself (`RENDER_*`, `PORT`).
4. **Render SSH.** Add your PC's public key in Render, Account Settings, SSH Public Keys. The
   service's SSH address is on its page under **Connect, SSH** (`srv-…@ssh.virginia.render.com`).

### The switch (a few minutes down)

1. **Snapshot on Render.** In the service's **Shell** tab (the image has no `sqlite3`, so add it
   for this session):

   ```sh
   apt-get update && apt-get install -y sqlite3
   sqlite3 /data/bot-v2.db ".backup /tmp/bot-v2.db"
   cd /data && tar -czf /tmp/data.tgz -C /tmp bot-v2.db -C /data relay $(ls bot.db 2>/dev/null)
   ```

   `.backup` copies the live database consistently. `relay` is Relay's room and cloud state;
   `bot.db` is antho-s-bot's old database, if it's still there.
2. **Copy it to the server**, from the PC:

   ```fish
   ssh srv-XXXX@ssh.virginia.render.com 'cat /tmp/data.tgz' > data.tgz
   tar -tzf data.tgz                                # bot-v2.db and relay/ should be listed
   scp data.tgz server1:apps/discord-bot/
   ```

3. **Suspend the Render service** (Settings, Suspend). Don't delete it: it's the fallback for a
   week. Anything that happened between the snapshot and now (a few minutes of XP) is lost.
4. **Start on the server:**

   ```fish
   cd ~/apps/discord-bot
   mkdir -p data; and tar -xzf data.tgz -C data; and rm data.tgz
   docker compose up -d
   docker compose logs -f bot
   ```

5. **Check:** a slash command and a button in Discord; sign in at `https://bot.<domain>`; Stream
   alerts shows the Twitch subscriptions as enabled (the bot subscribes again for the new address;
   Twitch drops the old Render ones on its own); Tally and Avex reach Relay at
   `wss://bot.<domain>/relay` (re-pair them if they still dial the Render address).

### Going back

`docker compose down` on the server, then resume the Render service. Render still has its disk
from before the switch, so anything since then is only on the server (`~/apps/discord-bot/data`).

### Updating

The server updates itself. `deploy/auto-update.sh` runs from cron every 2 minutes. When `main` has
a new commit and both CI jobs passed on it, it pulls, builds while the old bot keeps running, and
restarts onto the new image (a few seconds down). If the new bot doesn't answer `/healthz` within 2
minutes, the previous image goes back up. It never deploys a commit whose CI failed. What it did
is in `~/apps/discord-bot/deploy.log`.

The cron line (`crontab -e` on the server):

```
*/2 * * * * $HOME/apps/discord-bot/deploy/auto-update.sh
```

To update by hand instead: `cd ~/apps/discord-bot; and git pull; and docker compose up -d --build`.
The server checks GitHub rather than GitHub reaching the server: the repository is public, and a
self-hosted Actions runner on a public repository would let a pull request run code on the server.

## CI and deploys

GitHub Actions (`.github/workflows/ci.yml`) runs clippy with warnings as errors, the tests and a
Docker image build on every pull request and every push to `main`. The home server deploys `main`
once both jobs pass (see Updating above), so a red build never ships. The Render service is
suspended as a fallback; `render.yaml` goes when it's deleted.

## Code

- `crates/core` is the shared runtime: config, SQLite, the gateway, interaction helpers, and the
  dashboard framework and UI kit.
- `crates/feat-*` holds one crate per feature.
- `crates/render` turns SVG into PNG for the generated images.
- `crates/app` is the binary.

`docs/FEATURES.md` explains how to write a feature.
