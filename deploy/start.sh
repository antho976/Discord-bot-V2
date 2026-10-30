#!/usr/bin/env bash
# The container's entrypoint: the bot (Discord + dashboard, one Rust binary), Caddy on the public
# port in front of it, and Relay's rendezvous and cloud engine beside them for Tally and Avex.
# RELAY_PUBLIC_URL overrides Relay's public address; RELAY_PUBLIC_URL=off runs the bot alone.
#
# If any process stops, everything stops and the container exits, so Render restarts the whole
# service instead of leaving it half up.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
public_port="${PORT:-3000}"
bot_port=3001
pids=()

if [[ -z "${RELAY_PUBLIC_URL:-}" && -n "${RENDER_EXTERNAL_HOSTNAME:-}" ]]; then
  export RELAY_PUBLIC_URL="wss://${RENDER_EXTERNAL_HOSTNAME}/relay"
fi
if [[ "${RELAY_PUBLIC_URL:-}" == "off" ]] || ! command -v relay >/dev/null; then
  unset RELAY_PUBLIC_URL
fi

if [[ -n "${RELAY_PUBLIC_URL:-}" ]]; then
  mkdir -p "$XDG_DATA_HOME"
  install -d -m 700 "$XDG_RUNTIME_DIR"
  relay remote rendezvous --bind 127.0.0.1:7430 --queue-mib "${RELAY_QUEUE_MIB:-64}" &
  pids+=($!)
  # Idempotent: keeps the room secret, refreshes the public address phones are given.
  relay --instance stable remote via ws://127.0.0.1:7430 --public "${RELAY_PUBLIC_URL%/}" >/dev/null
  relay --instance stable serve --remote --remote-bind 127.0.0.1:7420 --remote-ops cloud &
  pids+=($!)
fi

PORT="$bot_port" bot &
pids+=($!)
PORT="$public_port" caddy run --config "$here/Caddyfile" --adapter caddyfile &
pids+=($!)

trap 'kill "${pids[@]}" 2>/dev/null || true' TERM INT
status=0
wait -n "${pids[@]}" || status=$?
kill "${pids[@]}" 2>/dev/null || true
wait || true
exit "$status"
