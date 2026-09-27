#!/usr/bin/env bash
# Remove staged package trees; keep the cargo download cache and all evidence.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
rm -rf .release-tmp/binaries .release-tmp/wheelhouse .release-tmp/*.tgz \
  sdk/js/native sdk/js/examples sdk/js/skills \
  sdk/python/src/jevscript/_bin sdk/python/src/jevscript/examples sdk/python/src/jevscript/skills
echo "left in .release-tmp: $(ls -A .release-tmp 2>/dev/null | tr "
" " ")"
