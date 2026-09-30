#!/usr/bin/env bash
# Start the fixture demo (`pnpm demo`) for a manual pass, with its state in the evidence directory.
# Usage: launch.sh [evidence-dir] [port]
set -euo pipefail
root=$(git rev-parse --show-toplevel)
evidence=${1:-${TMPDIR:-/tmp}/jevs-toolbox-verify/$(date +%s)}
port=${2:-5391}
mkdir -p "$evidence"
evidence=$(cd "$evidence" && pwd -P)
cd "$root/apps/toolbox"
pnpm build > "$evidence/build.log" 2>&1
node --input-type=module - "$evidence" "$port" <<'JS'
import { spawn } from 'node:child_process'
import { openSync, closeSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
const [evidence, port] = process.argv.slice(2)
const log = openSync(join(evidence, 'demo.log'), 'w', 0o600)
const child = spawn(process.execPath, ['demo/main.ts'], {
  detached: true,
  env: { ...process.env, JEVS_TOOLBOX_HOME: join(evidence, 'home'), JEVS_TOOLBOX_PORT: port },
  stdio: ['ignore', log, log],
})
writeFileSync(join(evidence, 'demo.pid'), String(child.pid))
child.unref()
closeSync(log)
JS
for _ in $(seq 100); do grep -q 'jevs toolbox demo:' "$evidence/demo.log" 2>/dev/null && break; sleep 0.1; done
if ! grep -q 'jevs toolbox demo:' "$evidence/demo.log"; then
  cat "$evidence/demo.log"
  echo 'demo did not become ready' >&2
  exit 1
fi
cat "$evidence/demo.log"
echo "evidence: $evidence"
