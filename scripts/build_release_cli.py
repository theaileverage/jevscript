"""Build and validate a release CLI on its matching CI host."""

from __future__ import annotations

import argparse
import os
import pathlib
import platform
import shutil
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]


def host_target() -> str:
    """Name the host using the package target vocabulary."""
    system = platform.system().lower()
    machine = platform.machine().lower()
    if machine in ("x86_64", "amd64"):
        machine = "x64"
    elif machine in ("arm64", "aarch64"):
        machine = "arm64"
    if system == "darwin" and machine in ("arm64", "x64"):
        return f"darwin-{machine}"
    if system == "windows" and machine == "x64":
        return "win32-x64"
    if system == "linux" and machine in ("arm64", "x64") and platform.libc_ver()[0] == "glibc":
        return f"linux-{machine}-gnu"
    raise SystemExit(f"unsupported release host: {system}/{machine}")


def main() -> None:
    """Reject stale/wrong-architecture binaries and personal build paths."""
    from stage_release import version

    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True)
    args = parser.parse_args()
    if args.target != host_target():
        raise SystemExit(f"runner is {host_target()}, not {args.target}")
    env = os.environ.copy()
    remap = f"--remap-path-prefix={pathlib.Path.home()}=/build"
    env["RUSTFLAGS"] = f"{env.get('RUSTFLAGS', '')} {remap}".strip()
    subprocess.run(["cargo", "build", "--release", "-p", "jevscript-cli", "--locked"], cwd=ROOT, env=env, check=True)
    name = "jevscript.exe" if os.name == "nt" else "jevscript"
    source = ROOT / "target" / "release" / name
    result = subprocess.check_output([str(source), "--version"], text=True).strip()
    if result != f"jevscript {version()}":
        raise SystemExit(f"unexpected CLI version: {result}")
    data = source.read_bytes()
    for needle in (b"/Users/", b"/home/runner/", b"C:\\Users\\", b"TYPESAFE_API_KEY="):
        if needle in data:
            raise SystemExit(f"release CLI embeds a private path or credential marker: {needle!r}")
    destination = ROOT / ".release-tmp" / "binaries" / args.target / name
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination)
    print(f"validated {args.target} {result} ({len(data)} bytes)")


if __name__ == "__main__":
    main()
