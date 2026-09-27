"""Prove a staged release bundle is the exact, complete, single-commit set to publish."""

from __future__ import annotations

import argparse
import hashlib
import json
import tarfile
import zipfile
from pathlib import Path

from stage_release import PLATFORMS, version


def sha256(path: Path) -> str:
    """Hash one artifact file."""
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> None:
    """Check digests, the artifact set, versions, source commit, and cross-registry CLI bytes."""
    parser = argparse.ArgumentParser()
    parser.add_argument("bundle", type=Path, help="directory holding SHA256SUMS, the npm tarball and wheelhouse/")
    parser.add_argument("--commit", required=True, help="source commit every artifact must record")
    parser.add_argument("--sums-sha256", help="reviewed SHA-256 of SHA256SUMS itself")
    parser.add_argument("--targets", default=",".join(PLATFORMS),
                        help="comma-separated targets of a representative CI bundle; a release holds all of them")
    args = parser.parse_args()
    targets = args.targets.split(",")
    if not set(targets) <= set(PLATFORMS):
        raise SystemExit(f"unsupported targets: {sorted(set(targets) - set(PLATFORMS))}")
    current = version()
    sums = args.bundle / "SHA256SUMS"
    if args.sums_sha256 is not None and sha256(sums) != args.sums_sha256:
        raise SystemExit("SHA256SUMS differs from the reviewed digest")
    listed = {}
    for line in sums.read_text().splitlines():
        digest, name = line.split(maxsplit=1)
        listed[name.lstrip("*")] = digest
    tarball = f"jevscript-{current}.tgz"
    wheels = {target: f"wheelhouse/jevscript-{current}-py3-none-{tag}.whl" for target, tag in PLATFORMS.items() if target in targets}
    expected = {tarball, *wheels.values()}
    present = {path.relative_to(args.bundle).as_posix() for path in args.bundle.rglob("*") if path.is_file()} - {"SHA256SUMS"}
    if set(listed) != expected or present != expected:
        raise SystemExit(f"bundle must hold exactly {sorted(expected)}; listed {sorted(listed)}, present {sorted(present)}")
    for name, digest in listed.items():
        if sha256(args.bundle / name) != digest:
            raise SystemExit(f"digest mismatch: {name}")
    with tarfile.open(args.bundle / tarball, "r:gz") as archive:
        package = json.loads(archive.extractfile("package/package.json").read())  # type: ignore[union-attr]
        npm_manifest = json.loads(archive.extractfile("package/native/manifest.json").read())  # type: ignore[union-attr]
    if package["name"] != "jevscript" or package["version"] != current or package.get("private"):
        raise SystemExit(f"npm package is not public jevscript@{current}")
    if set(npm_manifest["targets"]) != set(targets):
        raise SystemExit(f"npm tarball does not carry exactly {sorted(targets)}")
    for target, name in wheels.items():
        with zipfile.ZipFile(args.bundle / name) as archive:
            wheel_manifest = json.loads(archive.read("jevscript/_bin/manifest.json"))
        if list(wheel_manifest["targets"]) != [target]:
            raise SystemExit(f"{name} does not carry exactly the {target} CLI")
        if wheel_manifest["targets"][target] != npm_manifest["targets"][target]:
            raise SystemExit(f"npm and {name} carry different {target} CLI bytes")
        for field in ("version", "source_commit", "skill_sha256"):
            if wheel_manifest[field] != npm_manifest[field]:
                raise SystemExit(f"{name} {field} differs from the npm tarball")
    if npm_manifest["version"] != current or npm_manifest["source_commit"] != args.commit:
        raise SystemExit(f"artifacts record {npm_manifest['version']} at {npm_manifest['source_commit']}, not {current} at {args.commit}")
    print(f"verified jevscript {current} from {args.commit}: {len(listed)} artifacts")
    for name in sorted(listed):
        print(f"{listed[name]}  {name}")


if __name__ == "__main__":
    main()
