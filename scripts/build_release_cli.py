"""Build and validate a release CLI on its matching CI host."""

from __future__ import annotations

import argparse
import os
import pathlib
import platform
import shutil
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]
PATH_MARKERS = (b"/Users/", b"/home/runner/", b"C:\\Users\\")
CREDENTIAL_MARKERS = (b"TYPESAFE_API_KEY=",)
CONTEXT_BYTES = 160
SHOWN_MATCHES = 20


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


def marker_matches(data: bytes) -> list[str]:
    """Describe each marker match so a failed scan explains itself without echoing a credential."""
    matches = []
    for needle in PATH_MARKERS + CREDENTIAL_MARKERS:
        start = data.find(needle)
        while start != -1:
            if needle in CREDENTIAL_MARKERS:
                matches.append(f"offset {start}: {needle!r} followed by a withheld value")
            else:
                # Rust packs string literals without separators, so a credential can follow a path.
                context = data[start:start + CONTEXT_BYTES].split(b"\0", 1)[0]
                for credential in CREDENTIAL_MARKERS:
                    context = context.split(credential, 1)[0]
                matches.append(f"offset {start}: {context!r}")
            start = data.find(needle, start + 1)
    return matches


def main() -> None:
    """Reject stale/wrong-architecture binaries and personal build paths."""
    from stage_release import version

    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True)
    args = parser.parse_args()
    if args.target != host_target():
        raise SystemExit(f"runner is {host_target()}, not {args.target}")
    env = os.environ.copy()
    if args.target.startswith("darwin-"):
        env["MACOSX_DEPLOYMENT_TARGET"] = "15.0"
    # aws-lc-sys strips its C __FILE__ prefix only for GCC and Clang, so under MSVC a
    # registry inside the user profile would embed that profile's path in the binary.
    cargo_home = ROOT / ".release-tmp" / "cargo-home"
    env["CARGO_HOME"] = str(cargo_home)
    # rustc lets the last matching remap win, and cargo_home sits under home on a workstation.
    remaps = [f"--remap-path-prefix={pathlib.Path.home()}=/build", f"--remap-path-prefix={cargo_home}=/cargo"]
    env["RUSTFLAGS"] = " ".join([env.get("RUSTFLAGS", ""), *remaps]).strip()
    subprocess.run(["cargo", "build", "--release", "-p", "jevscript-cli", "--locked"], cwd=ROOT, env=env, check=True)
    name = "jevscript.exe" if os.name == "nt" else "jevscript"
    source = ROOT / "target" / "release" / name
    result = subprocess.check_output([str(source), "--version"], text=True).strip()
    if result != f"jevscript {version()}":
        raise SystemExit(f"unexpected CLI version: {result}")
    if args.target.startswith("darwin-"):
        from check_macos_binary import check

        check(source, args.target)
    data = source.read_bytes()
    matches = marker_matches(data)
    if matches:
        hidden = len(matches) - SHOWN_MATCHES
        more = [f"... and {hidden} more"] if hidden > 0 else []
        raise SystemExit("\n".join([f"release CLI embeds {len(matches)} private path or credential markers:",
                                     *matches[:SHOWN_MATCHES], *more]))
    destination = ROOT / ".release-tmp" / "binaries" / args.target / name
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination)
    print(f"validated {args.target} {result} ({len(data)} bytes)")


if __name__ == "__main__":
    main()
