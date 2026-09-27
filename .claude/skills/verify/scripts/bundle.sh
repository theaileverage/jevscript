#!/usr/bin/env bash
# Run the upload-package-bundle action's assemble-and-verify step, byte for
# byte as CI runs it, on the host target's staged tarball and wheel.
set -euo pipefail
root=$(git rev-parse --show-toplevel)
target=$1
outputs=$2
script=$(python3 - "$root/.github/actions/upload-package-bundle/action.yml" <<'PY'
import sys
import yaml
steps = yaml.safe_load(open(sys.argv[1]))["runs"]["steps"]
print(next(step["run"] for step in steps if step.get("id") == "assemble"))
PY
)
cd "$root/.release-tmp"
TARGETS=$target GITHUB_SHA=$(git rev-parse HEAD) GITHUB_OUTPUT=$outputs bash --noprofile --norc -eo pipefail -c "$script"
