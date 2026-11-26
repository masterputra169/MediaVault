#!/usr/bin/env bash
set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
python="$root/backend/.venv/bin/python"

if [[ ! -x "$python" || ! -d "$root/frontend/node_modules/next" ]]; then
  echo 'Pasang dependency backend dan frontend sesuai README.md.' >&2
  exit 1
fi

export NEXT_TELEMETRY_DISABLED=1
(cd "$root/backend" && exec "$python" -m app.serve) &
backend=$!
(cd "$root/frontend" && exec pnpm dev) &
frontend=$!
cleanup() {
  kill "$backend" "$frontend" 2>/dev/null || true
  wait "$backend" "$frontend" 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 130' INT TERM
wait -n "$backend" "$frontend"
