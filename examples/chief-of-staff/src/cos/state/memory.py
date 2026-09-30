"""Memory: the principal's standing preferences and the administration's learnings.

The principal's preferences and the learnings are curated Markdown,
one dated line per fact, never a duplicate. Preferences reach every worker's
instructions; learnings record what the learning loop encoded.
"""

from __future__ import annotations

from pathlib import Path

from .home import Home, atomic_write, iso


class Memory:
    def __init__(self, home: Home) -> None:
        self.home = home

    def _remember(self, name: str, heading: str, text: str) -> bool:
        path = self.home.data / name
        body = path.read_text(encoding="utf-8") if path.exists() else f"# {heading}\n\n"
        line = " ".join(text.split())
        if any(existing.split(": ", 1)[-1] == line for existing in body.splitlines()):
            return False
        atomic_write(path, body.rstrip("\n") + f"\n- {iso()[:10]}: {line}\n")
        return True

    def remember_preference(self, text: str) -> bool:
        return self._remember(self.home.preferences_path.name, "Preferences", text)

    def remember_learning(self, text: str) -> bool:
        return self._remember("learnings.md", "Learnings", text)

    def preferences(self, limit: int = 2000) -> str:
        current = _read(self.home.preferences_path)
        text = current[-limit:] if current else "(none recorded)"
        # A captain.md the migration could not merge still reaches workers,
        # labelled, so no preference is lost while the principal merges it.
        legacy = _read(self.home.legacy_preferences_path)
        if legacy and legacy != current:
            text += f"\n\n### Not yet merged from {self.home.legacy_preferences_path.name}\n{legacy[-limit:]}"
        return text

    def learnings(self) -> str:
        path = self.home.data / "learnings.md"
        return path.read_text(encoding="utf-8") if path.exists() else ""


def _read(path: Path) -> str:
    return path.read_text(encoding="utf-8") if path.exists() else ""
