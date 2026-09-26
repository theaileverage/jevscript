"""Idempotency keys for effects.

A wake that crashes part-way is run again from the start (Jevscript 0.1
cannot resume a run mid-effect), so every effect the run asks for must be
safe to ask for twice. Each effect gets a key from the wake id, the verb, its
arguments and how many identical calls came before it in this wake; the
first completed call records its result under that key, and a re-run returns
the recorded result instead of acting again. Keys of acknowledged wakes are
dropped, since those wakes never run again.
"""

from __future__ import annotations

import hashlib
import json
from typing import Any, Callable

from ..state.home import Home, read_jsonl, write_json


class EffectLog:
    def __init__(self, home: Home) -> None:
        self.path = home.state / "effects.jsonl"
        self.done: dict[str, Any] = {r["key"]: r["result"] for r in read_jsonl(self.path)}
        self.wake = "adhoc"
        self._counts: dict[str, int] = {}

    def begin(self, wake_id: str) -> None:
        self.wake = wake_id
        self._counts = {}

    def key(self, verb: str, args: Any) -> str:
        digest = hashlib.sha256(json.dumps([verb, args], sort_keys=True, default=str).encode()).hexdigest()[:16]
        base = f"{verb}:{digest}"
        self._counts[base] = self._counts.get(base, 0) + 1
        return f"{self.wake}:{base}:{self._counts[base]}"

    def once(self, verb: str, args: Any, act: Callable[[], Any]) -> Any:
        key = self.key(verb, args)
        if key in self.done:
            return self.done[key]
        result = act()
        self.done[key] = result
        with self.path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps({"key": key, "result": result}, default=str) + "\n")
        return result

    def forget(self, wake_id: str) -> None:
        prefix = f"{wake_id}:"
        kept = {k: v for k, v in self.done.items() if not k.startswith(prefix)}
        if len(kept) != len(self.done):
            self.done = kept
            self.path.write_text("".join(json.dumps({"key": k, "result": v}, default=str) + "\n" for k, v in kept.items()), encoding="utf-8")
        _ = write_json
