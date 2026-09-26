"""Stage the same pinned CLI bytes for npm and PyPI packages."""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import tomllib
from pathlib import Path

from third_party_notices import render as third_party_notices

ROOT = Path(__file__).resolve().parents[1]
PLATFORMS = {
    "darwin-arm64": "macosx_11_0_arm64",
    "darwin-x64": "macosx_10_15_x86_64",
    "linux-arm64-gnu": "manylinux_2_39_aarch64",
    "linux-x64-gnu": "manylinux_2_39_x86_64",
    "win32-x64": "win_amd64",
}


def version() -> str:
    """Read the one Rust workspace version."""
    with (ROOT / "Cargo.toml").open("rb") as source:
        return tomllib.load(source)["workspace"]["package"]["version"]


def stage(binary_root: Path, targets: list[str], destination: Path, npm: bool) -> None:
    """Copy named binaries, examples, and the exact bundled Skill into a package tree."""
    current = version()
    package_version = (
        json.loads((ROOT / "sdk/js/package.json").read_text())["version"]
        if npm
        else tomllib.loads((ROOT / "sdk/python/pyproject.toml").read_text())["project"]["version"]
    )
    if current != package_version:
        raise SystemExit("Rust, JS and Python release versions must agree")
    if destination.exists():
        shutil.rmtree(destination)
    destination.mkdir(parents=True)
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    skill = (ROOT / "skills/jevscript/SKILL.md").read_bytes()
    manifest: dict[str, object] = {"version": current, "source_commit": commit, "targets": {},
                                   "skill_sha256": hashlib.sha256(skill).hexdigest()}
    for target in targets:
        if target not in PLATFORMS:
            raise SystemExit(f"unsupported target: {target}")
        name = "jevscript.exe" if target == "win32-x64" else "jevscript"
        source = binary_root / target / name
        if not source.is_file():
            raise SystemExit(f"missing release binary: {source}")
        # Cross-built binaries cannot execute on this host; the matching CI runner
        # runs --version and the full smoke before its artifact is accepted.
        output = destination / target / name if npm else destination / name
        output.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, output)
        if target != "win32-x64":
            output.chmod(output.stat().st_mode | 0o111)
        manifest["targets"][target] = {"sha256": hashlib.sha256(output.read_bytes()).hexdigest()}
    (destination / "manifest.json").write_text(json.dumps(manifest, sort_keys=True, indent=2) + "\n")
    (destination / "THIRD_PARTY_NOTICES.txt").write_text(third_party_notices(), encoding="utf-8")
    example_dir = ROOT / ("sdk/js/examples" if npm else "sdk/python/src/jevscript/examples")
    if example_dir.exists():
        shutil.rmtree(example_dir)
    example_dir.mkdir(parents=True)
    shutil.copy2(ROOT / "examples/inbox_triage.jev", example_dir / "inbox_triage.jev")
    shutil.copy2(ROOT / "sdk/fixtures/package_smoke.jev", example_dir / "package_smoke.jev")
    skill_dir = ROOT / ("sdk/js/skills/jevscript" if npm else "sdk/python/src/jevscript/skills/jevscript")
    skill_dir.mkdir(parents=True, exist_ok=True)
    (skill_dir / "SKILL.md").write_bytes(skill)


def main() -> None:
    """Accept either one platform wheel or an npm bundle target list."""
    parser = argparse.ArgumentParser()
    parser.add_argument("kind", choices=["npm", "wheel"])
    parser.add_argument("--binary-root", type=Path, required=True)
    parser.add_argument("--targets", required=True, help="comma-separated target names")
    args = parser.parse_args()
    targets = args.targets.split(",")
    if args.kind == "wheel" and len(targets) != 1:
        raise SystemExit("a wheel must contain exactly one target binary")
    destination = ROOT / ("sdk/js/native" if args.kind == "npm" else "sdk/python/src/jevscript/_bin")
    stage(args.binary_root, targets, destination, args.kind == "npm")
    if args.kind == "wheel":
        print(f"JEVSCRIPT_TARGET={targets[0]}")
        print(f"JEVSCRIPT_PLATFORM_TAG={PLATFORMS[targets[0]]}")
        print(f"JEVSCRIPT_WHEEL_TAG=py3-none-{PLATFORMS[targets[0]]}")


if __name__ == "__main__":
    main()
