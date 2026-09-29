#!/usr/bin/env bash
# Start the fixture demo (`pnpm demo`) for a manual pass, with its state in the evidence directory.
# Usage: launch.sh [evidence-dir] [port]
set -euo pipefail
root=$(git rev-parse --show-toplevel)
evidence=${1:-${TMPDIR:-/tmp}/jevs-toolbox-verify/$(date +%s)}
port=${2:-5391}
mkdir -p "$evidence"
cd "$root/apps/toolbox"
pnpm build > "$evidence/build.log" 2>&1
JEVS_TOOLBOX_HOME="$evidence/home" JEVS_TOOLBOX_PORT="$port" node demo/main.ts > "$evidence/demo.log" 2>&1 &
echo $! > "$evidence/demo.pid"
for _ in $(seq 100); do grep -q 'jevs toolbox demo:' "$evidence/demo.log" 2>/dev/null && break; sleep 0.1; done
cat "$evidence/demo.log"
echo "evidence: $evidence"
