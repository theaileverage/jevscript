#!/usr/bin/env bash
# Read-only: can this checkout launch and drive apps/toolbox?
set -uo pipefail
cd "$(git rev-parse --show-toplevel)"
fail=0
check() { if eval "$2" >/dev/null 2>&1; then echo "ok    $1"; else echo "MISS  $1${3:+  ($3)}"; fail=1; fi; }
check "node >= 26 ($(node --version 2>/dev/null))" '[ "$(node -p "process.versions.node.split(\".\")[0]")" -ge 26 ]' "the server runs its TypeScript directly"
check "pnpm ($(pnpm --version 2>/dev/null))" 'pnpm --version'
check "target/debug/jevscript" 'target/debug/jevscript --version' "cargo build"
check "sdk/js/dist" '[ -f sdk/js/dist/index.js ]' "(cd sdk/js && pnpm install && pnpm run build)"
check "adapters built" '[ -f adapters/codex/dist/index.js ] && [ -f adapters/claude-code/dist/index.js ] && [ -f adapters/core/dist/index.js ]' "(cd adapters && pnpm install && pnpm run build)"
check "apps/toolbox/node_modules" '[ -d apps/toolbox/node_modules/playwright-core ]' "(cd apps/toolbox && pnpm install)"
check "Chrome for the browser pass" '[ -n "${JEVS_BROWSER:-}" ] || [ -d "/Applications/Google Chrome.app" ] || command -v google-chrome' "or set JEVS_BROWSER"
check "tmux for the pane tail check" 'command -v tmux' "the check is skipped without it"
exit $fail
