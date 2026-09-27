#!/usr/bin/env bash
# Build, stage, pack and install-smoke the host target's npm tarball and wheel.
set -uo pipefail
root=$(git rev-parse --show-toplevel)
cd "$root"
evidence=${1:-${TMPDIR:-/tmp}/jevscript-verify/$(date +%s)}
mkdir -p "$evidence"
echo "evidence: $evidence"
target=$(python3 -c 'import sys; sys.path.insert(0, "scripts"); from build_release_cli import host_target; print(host_target())')
tag=$(python3 -c "import sys; sys.path.insert(0, 'scripts'); from stage_release import PLATFORMS; print(PLATFORMS['$target'])")
version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])')
tgz=".release-tmp/jevscript-$version.tgz"
whl=".release-tmp/wheelhouse/jevscript-$version-py3-none-$tag.whl"

step() {
  local name=$1; shift
  echo "== $name"
  (cd "$root" && "$@") >"$evidence/$name.log" 2>&1
  local code=$?
  echo "$name $code" >>"$evidence/summary.txt"
  if [ $code -ne 0 ]; then
    tail -n 30 "$evidence/$name.log"
    echo "FAILED at $name; evidence in $evidence" >&2
    exit $code
  fi
}

step build python3 scripts/build_release_cli.py --target "$target"
step stage-npm python3 scripts/stage_release.py npm --binary-root .release-tmp/binaries --targets "$target"
step stage-wheel python3 scripts/stage_release.py wheel --binary-root .release-tmp/binaries --targets "$target"
step wheel env -C sdk/python JEVSCRIPT_TARGET="$target" JEVSCRIPT_PLATFORM_TAG="$tag" JEVSCRIPT_WHEEL_TAG="py3-none-$tag" \
  uv build --wheel --out-dir ../../.release-tmp/wheelhouse
step npm-build env -C sdk/js sh -c 'pnpm install --frozen-lockfile && pnpm run build'
step npm-pack env -C sdk/js JEVSCRIPT_PACK_TARGETS="$target" npm pack --pack-destination ../../.release-tmp
step smoke python3 scripts/smoke_packages.py --npm "$tgz" --wheel "$whl"
shasum -a 256 "$tgz" "$whl" >"$evidence/artifacts.sha256"
grep '^validated' "$evidence/build.log"
echo "PASS $target $version; evidence: $evidence"
