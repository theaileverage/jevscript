# Release package feature map

What a user gets from `npm install jevscript` or `pip install jevscript`, and
where each part is proven. Drive them with `../scripts/package-smoke.sh`.

| Feature | File | Proven locally | Proven in CI |
| --- | --- | --- | --- |
| Release CLI build and embedded-path scan | [release-binary.md](release-binary.md) | host target | every target (`ci.yml`, `release.yml`) |
| Installed packages: CLI, SDKs, setup, integrity | [installed-packages.md](installed-packages.md) | host target | Linux x64 and Windows x64 on PRs; all five after a tag |
| CI release lanes per target | [ci-release-lanes.md](ci-release-lanes.md) | no | yes |
