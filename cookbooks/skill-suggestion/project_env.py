"""Load cookbook credentials from project env files without extra dependencies."""

from __future__ import annotations

import os
from pathlib import Path


def load_project_env(repo: Path) -> None:
    """Load `.env.local` then `.env`, while preserving the process environment."""
    for path in (repo / ".env.local", repo / ".env"):
        if not path.is_file():
            continue
        for raw_line in path.read_text(encoding="utf-8").splitlines():
            line = raw_line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            if line.startswith("export "):
                line = line.removeprefix("export ").lstrip()
            name, value = line.split("=", 1)
            name = name.strip()
            if not name.isidentifier():
                continue
            value = value.strip()
            if len(value) >= 2 and value[0] == value[-1] and value[0] in {"'", '"'}:
                value = value[1:-1]
            os.environ.setdefault(name, value)
