#!/usr/bin/env bash
set -euo pipefail
root=$(git rev-parse --show-toplevel)
cd "$root/.release-tmp"
rm -rf package-bundle
mkdir -p package-bundle/wheelhouse
cp jevscript-*.tgz package-bundle/
cp wheelhouse/*.whl package-bundle/wheelhouse/
cd package-bundle
shasum -a 256 jevscript-*.tgz wheelhouse/*.whl > SHA256SUMS
python3 ../../scripts/verify_release_bundle.py . --commit "$GITHUB_SHA" ${TARGETS:+--targets "$TARGETS"}
echo "sums-sha256=$(shasum -a 256 SHA256SUMS | cut -d' ' -f1)" >> "$GITHUB_OUTPUT"
echo "SHA256SUMS digest: $(shasum -a 256 SHA256SUMS | cut -d' ' -f1)"
