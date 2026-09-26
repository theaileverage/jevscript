"""Resolve and verify the packaged runtime binary (spec section 11.5)."""

from __future__ import annotations

import hashlib
import json
import os
import platform
from importlib.resources import files
from pathlib import Path


def _target() -> str:
    system = platform.system().lower()
    machine = platform.machine().lower()
    if machine in ("amd64", "x86_64"):
        machine = "x64"
    elif machine in ("aarch64", "arm64"):
        machine = "arm64"
    if system == "darwin" and machine in ("x64", "arm64"):
        return f"darwin-{machine}"
    if system == "windows" and machine == "x64":
        return "win32-x64"
    if system == "linux" and machine in ("x64", "arm64"):
        if platform.libc_ver()[0] != "glibc":
            raise RuntimeError("Jevscript does not yet ship a Linux musl/Alpine CLI")
        return f"linux-{machine}-gnu"
    raise RuntimeError(f"Jevscript has no CLI for {system}/{machine}")


def packaged_binary() -> str:
    """Check package version and SHA-256 before running the CLI."""
    target = _target()
    root = files("jevscript")
    try:
        manifest = json.loads(root.joinpath("_bin", "manifest.json").read_text())
        from importlib.metadata import version

        if manifest["version"] != version("jevscript"):
            raise RuntimeError("Jevscript CLI version does not match the SDK")
        expected = manifest["targets"][target]["sha256"]
        path = Path(str(root.joinpath("_bin", "jevscript.exe" if os.name == "nt" else "jevscript")))
        actual = hashlib.sha256(path.read_bytes()).hexdigest()
    except (FileNotFoundError, KeyError, json.JSONDecodeError) as exc:
        raise RuntimeError(f"Jevscript CLI for {target} is missing; reinstall the matching wheel") from exc
    if actual != expected:
        raise RuntimeError(f"Jevscript CLI for {target} failed integrity check; reinstall the wheel")
    if not path.is_file() or (os.name != "nt" and not os.access(path, os.X_OK)):
        raise RuntimeError(f"Jevscript CLI for {target} is not executable")
    return str(path)


def resolve_binary(explicit: str | None = None) -> str:
    """Explicit host settings win over the wheel's binary."""
    return explicit or os.environ.get("JEVSCRIPT_BIN") or packaged_binary()
