"""Finding the Jevscript binary, the module directory, and the Jev key.

- ``find_jevscript``: ``JEVSCRIPT_BIN``, then this repository's own
  ``target/`` build, then ``PATH``, else a readable setup error. The source
  checkout's build comes before ``PATH`` because the editable SDK puts a
  wheel-only ``jevscript`` wrapper, with no packaged CLI, on the venv ``PATH``.
- ``keychain_key``: the TypeSafe key from the macOS keychain, read at run time
  with ``security find-generic-password``. It is only ever placed in the
  environment of a ``jevscript serve`` child while that child is spawned
  (``episode.JevSession``), never written to disk or to a log.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

JEV_KEY_ENV = "TYPESAFE_API_KEY"


class SetupError(RuntimeError):
    """Something the person must fix before the Chief of Staff can run."""


def _sdk_path() -> None:
    """Use the in-repo SDK when the package was not installed with it."""
    try:
        import jevscript  # noqa: F401
    except ImportError:
        sdk = Path(__file__).resolve().parents[4] / "sdk" / "python" / "src"
        if sdk.exists():
            sys.path.insert(0, str(sdk))


_sdk_path()
from jevscript import load  # noqa: E402,F401 - re-exported for episode.py


def repo_root() -> Path | None:
    for parent in Path(__file__).resolve().parents:
        if (parent / "Cargo.toml").exists() and (parent / "crates" / "jevscript-cli").exists():
            return parent
    return None


def find_jevscript() -> str:
    explicit = os.environ.get("JEVSCRIPT_BIN")
    if explicit:
        if os.access(explicit, os.X_OK):
            return explicit
        raise SetupError(f"JEVSCRIPT_BIN is set to {explicit}, which is not an executable file.")
    root = repo_root()
    if root:
        for profile in ("release", "debug"):
            candidate = root / "target" / profile / "jevscript"
            if candidate.exists():
                return str(candidate)
    found = shutil.which("jevscript")
    if found:
        return found
    raise SetupError(
        "Cannot find the `jevscript` binary. Build it with `cargo build` at the repository root, "
        "put it on PATH, or set JEVSCRIPT_BIN to its path."
    )


def jev_dir() -> Path:
    here = Path(__file__).resolve().parent
    for candidate in (here / "jev", here.parents[1] / "jev"):
        if (candidate / "cos.jev").exists():
            return candidate
    raise SetupError("Cannot find the Chief of Staff .jev modules next to the package.")


def keychain_key(service: str, account: str | None = None, run: Any = subprocess.run) -> str:
    """The Jev key from the macOS keychain; raises a readable error, never the key."""
    if not shutil.which("security"):
        raise SetupError("The macOS `security` tool is not available, so the keychain cannot be read.")
    argv = ["security", "find-generic-password", "-s", service, "-w"]
    if account:
        argv += ["-a", account]
    result = run(argv, capture_output=True, text=True, check=False)
    key = (result.stdout or "").strip()
    if result.returncode != 0 or not key:
        raise SetupError(
            f"No TypeSafe key in the keychain under service `{service}`. Add one with "
            f"`security add-generic-password -s {service} -a \"$USER\" -w` (it prompts for the key), "
            "or set jev.keychain_service in config.json."
        )
    return key
