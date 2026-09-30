"""Project registry, delivery posture, ministers and dispatch profiles.

Projects, ministers (stored as ``mates``) and dispatch profiles are kept as
JSON in a home.
Each list reaches Jevscript as a snapshot field; the program chooses among
the entries with judgments.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
from pathlib import Path
from typing import Any

from .home import Home, atomic_write, read_json, write_json

MODES = ("local-only", "direct-PR", "no-mistakes")

DEFAULT_PROFILES: list[dict[str, Any]] = [
    {
        "name": "default",
        "rule": "any work that no more specific rule covers",
        "harness": "claude",
        "model": None,
        "effort": None,
        "backend": None,
        "command": None,
    }
]


def default_branch(path: Path) -> str:
    """The branch a project lands on: its origin HEAD, else the current branch."""
    for argv in (
        ["git", "-C", str(path), "symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
        ["git", "-C", str(path), "symbolic-ref", "--short", "HEAD"],
    ):
        result = subprocess.run(argv, capture_output=True, text=True, check=False)
        if result.returncode == 0 and result.stdout.strip():
            return result.stdout.strip().split("/", 1)[-1] if argv[-1].startswith("refs/remotes") else result.stdout.strip()
    return "main"


class Registry:
    def __init__(self, home: Home) -> None:
        self.home = home

    # -- projects -----------------------------------------------------------

    @property
    def projects_path(self) -> Path:
        return self.home.data / "projects.json"

    def projects(self) -> list[dict[str, Any]]:
        return read_json(self.projects_path, [])

    def project(self, name: str) -> dict[str, Any]:
        for project in self.projects():
            if project["name"] == name:
                return project
        raise KeyError(f"no project named `{name}`")

    def add_project(
        self,
        name: str,
        path: str,
        description: str,
        mode: str = "local-only",
        yolo: bool = False,
        branch_prefix: str = "cos/",
        review: str | None = None,
    ) -> dict[str, Any]:
        if mode not in MODES:
            raise ValueError(f"mode must be one of {', '.join(MODES)}")
        root = Path(path).expanduser().resolve()
        if not (root / ".git").exists():
            raise ValueError(f"{root} is not a git repository")
        projects = [p for p in self.projects() if p["name"] != name]
        record = {
            "name": name,
            "path": str(root),
            "description": description,
            "mode": mode,
            "yolo": yolo,
            "branch_prefix": branch_prefix,
            "default_branch": default_branch(root),
            "review": review,
        }
        write_json(self.projects_path, projects + [record])
        return record

    def remove_project(self, name: str, in_flight: list[str]) -> None:
        if in_flight:
            raise RuntimeError(f"`{name}` still has work under way: {', '.join(in_flight)}")
        write_json(self.projects_path, [p for p in self.projects() if p["name"] != name])

    # -- ministers -------------------------------------------------------------

    @property
    def mates_path(self) -> Path:
        return self.home.data / "mates.json"

    def mates(self) -> list[dict[str, Any]]:
        return read_json(self.mates_path, [])

    def add_mate(self, name: str, scope: str) -> dict[str, Any]:
        child = Home(self.home.root / "mates" / name)
        mates = self.mates()
        new_minister = not child.root.exists() and not any(m["name"] == name for m in mates)
        write_json(child.data / "parent.json", {"home": str(self.home.root), "name": name})
        child.init()
        record = {"name": name, "scope": scope, "home": str(child.root)}
        write_json(self.mates_path, [m for m in mates if m["name"] != name] + [record])
        charter = f"# Charter\n\nMinister `{name}` of {self.home.root}.\nPortfolio: {scope}\n"
        atomic_write(child.data / "charter.md", charter)
        # Seed only missing child files, including on re-add; a later parent
        # preference conflict must not enter an established minister's home.
        sources = [self.home.data / name for name in ("projects.json", "profiles.json", self.home.preferences_path.name)]
        legacy, current = self.home.legacy_preferences_path, self.home.preferences_path
        if new_minister and legacy.exists() and current.exists() and legacy.read_bytes() != current.read_bytes():
            sources.append(legacy)
        for source in sources:
            target = child.data / source.name
            if source.exists() and not target.exists():
                with tempfile.NamedTemporaryFile(dir=child.data) as staged:
                    shutil.copyfile(source, staged.name)
                    try:
                        os.link(staged.name, target)
                    except FileExistsError:
                        pass
        return record

    def homes(self) -> list[dict[str, Any]]:
        """The routing list intake picks among: this home first, then ministers."""
        return [{"name": "main", "scope": "any work no second mate's scope covers"}] + [
            {"name": m["name"], "scope": m["scope"]} for m in self.mates()
        ]

    # -- dispatch profiles ---------------------------------------------------

    @property
    def profiles_path(self) -> Path:
        return self.home.data / "profiles.json"

    def profiles(self) -> list[dict[str, Any]]:
        profiles = read_json(self.profiles_path, DEFAULT_PROFILES)
        for profile in profiles:
            for key in ("model", "effort", "backend", "command"):
                profile.setdefault(key, None)
        return profiles

    def set_profiles(self, profiles: list[dict[str, Any]]) -> None:
        for profile in profiles:
            missing = {"name", "rule", "harness"} - set(profile)
            if missing:
                raise ValueError(f"profile {profile} lacks {', '.join(sorted(missing))}")
        write_json(self.profiles_path, profiles)
