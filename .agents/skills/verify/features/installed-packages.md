# Installed packages

## Sub-features

Install npm and wheel archives, run both CLIs and SDKs, verify setup and binary integrity rejection.

## How to get to it (user POV)

Install `jevscript` from npm or PyPI and invoke its command or SDK from outside the source tree.

## Driving it with the package smoke

Build the artifacts using `docs/package-release.md`, then run `python3 scripts/smoke_packages.py --npm .release-tmp/jevscript-0.1.2.tgz --wheel .release-tmp/wheelhouse/jevscript-0.1.2-py3-none-macosx_15_0_arm64.whl` on a macOS 15 arm64 host. Use the x86_64 wheel on a macOS 15 x86_64 host. Require `offline npm and Python artifact smoke passed` and exit 0.

## Gotchas

This is local E2E evidence for one host architecture. The release workflow must pass the same path on both macOS 15 architectures before publication.
