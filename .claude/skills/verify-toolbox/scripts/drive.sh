#!/usr/bin/env bash
# Run the boundary suite and the browser pass, logging each into the evidence directory.
# Usage: drive.sh [evidence-dir]
set -uo pipefail
root=$(git rev-parse --show-toplevel)
evidence=${1:-${TMPDIR:-/tmp}/jevs-toolbox-verify/$(date +%s)}
mkdir -p "$evidence"
evidence=$(cd "$evidence" && pwd -P)
cd "$root/apps/toolbox"
: > "$evidence/summary.txt"
step() {
  local name=$1; shift
  # The keys stay out so nothing can reach a live service; the tests bring their own fixtures.
  env -u TYPESAFE_API_KEY -u ANTHROPIC_API_KEY -u ANTHROPIC_BASE_URL -u JEVSCRIPT_PROFILES "$@" > "$evidence/$name.log" 2>&1
  local code=$?
  echo "$name $code" | tee -a "$evidence/summary.txt"
}
step typecheck pnpm run typecheck
step boundary pnpm test
step browser env JEVS_EVIDENCE="$evidence/screenshots" pnpm test:browser
git -C "$root" rev-parse HEAD > "$evidence/commit.txt"
echo "evidence: $evidence"
! grep -qv ' 0$' "$evidence/summary.txt"
