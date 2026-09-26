"""Memory: the owner's standing preferences and the fleet's learnings.

The owner's preferences and fleet learnings are curated Markdown,
one dated line per fact, never a duplicate. Preferences reach every worker's
instructions; learnings record what the learning loop encoded.
"""

from __future__ import annotations

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
        return self._remember("captain.md", "Preferences", text)

    def remember_learning(self, text: str) -> bool:
        return self._remember("learnings.md", "Learnings", text)

    def preferences(self, limit: int = 2000) -> str:
        path = self.home.data / "captain.md"
        text = path.read_text(encoding="utf-8") if path.exists() else "(none recorded)"
        return text[-limit:]

    def learnings(self) -> str:
        path = self.home.data / "learnings.md"
        return path.read_text(encoding="utf-8") if path.exists() else ""
