#!/usr/bin/env bash
# Auto-update for the self-hosted bot (README, "Self-hosting"). Cron runs it every 2 minutes.
# When main has a new commit and every GitHub check on it passed, it pulls the commit, builds the
# new image while the old bot keeps running, and restarts onto it. If the new bot doesn't answer
# /healthz within 2 minutes, the previous image goes back up. It logs to deploy.log only when
# something happens.
set -euo pipefail
cd "$(dirname "$0")/.."

exec 9>/tmp/discord-bot-auto-update.lock
flock -n 9 || exit 0 # the previous run is still building

repo=antho976/Discord-bot-V2
min_checks=2 # CI's two jobs: "Clippy and tests" and "Docker image"
state=.deploy-state
log() { echo "$(date -u +%FT%TZ) $*" >>deploy.log; }

git fetch -q origin main
head=$(git rev-parse HEAD)
new=$(git rev-parse origin/main)
[[ "$head" == "$new" ]] && exit 0
short=${new:0:7}

# Deploy only once GitHub's checks on that commit have all finished and passed.
runs=$(curl -fsS -m 20 -H 'Accept: application/vnd.github+json' \
  "https://api.github.com/repos/$repo/commits/$new/check-runs?per_page=100")
total=$(jq '.check_runs | length' <<<"$runs")
pending=$(jq '[.check_runs[] | select(.status != "completed")] | length' <<<"$runs")
failed=$(jq '[.check_runs[] | select(.status == "completed"
  and (.conclusion | IN("success", "skipped", "neutral") | not))] | length' <<<"$runs")
if ((failed > 0)); then
  if [[ "$(cat "$state" 2>/dev/null)" != "ci-failed $new" ]]; then
    log "$short: CI failed, not deploying; still running ${head:0:7}"
    echo "ci-failed $new" >"$state"
  fi
  exit 0
fi
((total < min_checks || pending > 0)) && exit 0

if ! git merge -q --ff-only origin/main; then
  log "$short: can't fast-forward (local changes on the server?), not deploying"
  exit 1
fi
log "$short: deploying \"$(git log -1 --format=%s)\""
docker image tag discord-bot-v2:local discord-bot-v2:previous 2>/dev/null || true
if ! docker compose build -q bot >>deploy.log 2>&1; then
  log "$short: build failed; still running ${head:0:7}"
  exit 1
fi
docker compose up -d >>deploy.log 2>&1

url="$(sed -n 's/^PUBLIC_URL=//p' bot.env)/healthz"
healthy() { curl -fsS -m 5 -o /dev/null "$url"; }
for _ in $(seq 24); do
  sleep 5
  if healthy && sleep 10 && healthy; then
    log "$short: up"
    rm -f "$state"
    docker image prune -f >/dev/null
    exit 0
  fi
done

log "$short: no answer from $url after 2 minutes; putting the previous version back"
docker image tag discord-bot-v2:previous discord-bot-v2:local
docker compose up -d --no-build >>deploy.log 2>&1
exit 1
