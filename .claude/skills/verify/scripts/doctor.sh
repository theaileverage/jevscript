#!/usr/bin/env bash
# Read-only: is this checkout and machine able to build and smoke the host packages?
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
target=$(python3 -c 'import sys; sys.path.insert(0, "scripts"); from build_release_cli import host_target; print(host_target())')
rust=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])')
npm=$(node -p 'require("./sdk/js/package.json").version')
pypi=$(python3 -c 'import tomllib; print(tomllib.load(open("sdk/python/pyproject.toml", "rb"))["project"]["version"])')
echo "host target: $target"
echo "toolchain:   $(rustup show active-toolchain)"
echo "node:        $(node --version)  pnpm: $(pnpm --version)  uv: $(uv --version)  python: $(python3 --version)"
echo "versions:    cargo $rust  npm $npm  pypi $pypi"
[ "$rust" = "$npm" ] && [ "$rust" = "$pypi" ] || { echo "versions disagree" >&2; exit 1; }
[ "$(node -p 'process.versions.node.split(".")[0]')" -ge 22 ] || { echo "node >= 22 required" >&2; exit 1; }
