#!/usr/bin/env bash
# Start the fixture endpoints and the production toolbox server for a manual pass.
# Usage: launch.sh [evidence-dir] [port]
set -euo pipefail
root=$(git rev-parse --show-toplevel)
evidence=${1:-${TMPDIR:-/tmp}/jevs-toolbox-verify/$(date +%s)}
port=${2:-5391}
mkdir -p "$evidence"
cd "$root/apps/toolbox"
pnpm build > "$evidence/build.log" 2>&1
node test/fixtures.ts "$evidence" > "$evidence/fixtures.env" 2> "$evidence/fixtures.log" &
echo $! > "$evidence/fixtures.pid"
for _ in $(seq 50); do grep -q ANTHROPIC_BASE_URL "$evidence/fixtures.env" 2>/dev/null && break; sleep 0.1; done
. "$evidence/fixtures.env"
# Refuse to start unless every service points at a fixture: the server would otherwise read real keys from .env.
[ "${TYPESAFE_API_KEY:-}" = fixture-key ] && [ "${ANTHROPIC_API_KEY:-}" = fixture-key ] && [ -n "${ANTHROPIC_BASE_URL:-}" ] \
  && [ "${JEVSCRIPT_PROFILES:-}" = "$evidence/profiles.json" ] || { echo "fixture environment missing; not starting" >&2; exit 1; }
NODE_ENV=production JEVS_TOOLBOX_HOME="$evidence/home" JEVS_TOOLBOX_PORT="$port" node server/main.ts > "$evidence/server.log" 2>&1 &
echo $! > "$evidence/server.pid"
for _ in $(seq 100); do grep -q 'jevs toolbox on' "$evidence/server.log" 2>/dev/null && break; sleep 0.1; done
cat "$evidence/server.log"
echo "evidence: $evidence"
