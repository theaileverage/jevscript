"""The `person` capability (spec section 9.2): the person who owns the work.

``notify`` goes to the outbox (and the chat session's wake queue). ``ask``
and ``take_over`` are pauses the runtime raises itself; the episode runner
turns them into durable decisions and parks the run until they are answered.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:  # pragma: no cover
    from ..host import Host


class Person:
    kind = "person"

    def __init__(self, host: "Host") -> None:
        self.host = host

    def call(self, verb: str, args: dict[str, Any], capability: str | None = None) -> Any:
        values = list(args.get("positional") or [])
        if verb == "notify":
            text = str(values[0]) if values else str((args.get("named") or {}).get("text", ""))
            self.host.effects.once("notify", [text], lambda: self.host.decisions.notify(text))
        return None
