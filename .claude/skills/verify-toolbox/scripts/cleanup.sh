#!/usr/bin/env bash
# Stop what launch.sh started. Keeps the evidence directory.
# Usage: cleanup.sh <evidence-dir>
set -uo pipefail
for pid in "$1"/server.pid "$1"/fixtures.pid; do
  [ -f "$pid" ] && kill "$(cat "$pid")" 2>/dev/null && rm "$pid"
done
echo "stopped; evidence kept in $1"
