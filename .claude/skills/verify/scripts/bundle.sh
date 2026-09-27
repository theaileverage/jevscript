#!/usr/bin/env bash
# Run the upload-package-bundle action's assemble-and-verify step, byte for
# byte as CI runs it, on the host target's staged tarball and wheel.
set -euo pipefail
root=$(git rev-parse --show-toplevel)
target=$1
outputs=$2
outputs=$(cd "$(dirname "$outputs")" && pwd -P)/$(basename "$outputs")
TARGETS=$target GITHUB_SHA=$(git rev-parse HEAD) GITHUB_OUTPUT=$outputs bash "$root/scripts/assemble_release_bundle.sh"
