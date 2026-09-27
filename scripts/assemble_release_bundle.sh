#!/usr/bin/env bash
set -euo pipefail
root=$(git rev-parse --show-toplevel)
cd "$root/.release-tmp"
rm -rf package-bundle
mkdir -p package-bundle/wheelhouse
python3 - "${TARGETS:-}" <<'PY'
import shutil
import sys
from pathlib import Path

sys.path.insert(0, "../scripts")
from stage_release import PLATFORMS, version

current = version()
targets = sys.argv[1].split(",") if sys.argv[1] else PLATFORMS
shutil.copy2(f"jevscript-{current}.tgz", "package-bundle")
for target in targets:
    tag = PLATFORMS[target]
    shutil.copy2(Path("wheelhouse") / f"jevscript-{current}-py3-none-{tag}.whl", "package-bundle/wheelhouse")
PY
cd package-bundle
shasum -a 256 jevscript-*.tgz wheelhouse/*.whl > SHA256SUMS
python3 ../../scripts/verify_release_bundle.py . --commit "$GITHUB_SHA" ${TARGETS:+--targets "$TARGETS"}
echo "sums-sha256=$(shasum -a 256 SHA256SUMS | cut -d' ' -f1)" >> "$GITHUB_OUTPUT"
echo "SHA256SUMS digest: $(shasum -a 256 SHA256SUMS | cut -d' ' -f1)"
