"""Installed jevscript command (spec section 11.6)."""

from __future__ import annotations

import os
import subprocess
import sys

from ._native import packaged_binary


def main() -> int:
    """Run the wheel's CLI with the caller's arguments and streams."""
    try:
        binary = packaged_binary()
    except RuntimeError as exc:
        print(f"jevscript: {exc}", file=sys.stderr)
        return 1
    if os.name != "nt":
        os.execv(binary, [binary, *sys.argv[1:]])
        return 1  # pragma: no cover - execv replaces this process
    return subprocess.call([binary, *sys.argv[1:]])
