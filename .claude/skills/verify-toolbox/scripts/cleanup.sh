#!/usr/bin/env bash
# Stop the demo launch.sh started; it closes its fixtures itself. Keeps the evidence directory.
# Usage: cleanup.sh <evidence-dir>
set -uo pipefail
pid="$1/demo.pid"
[ -f "$pid" ] && kill "$(cat "$pid")" 2>/dev/null && rm "$pid"
echo "stopped; evidence kept in $1"
