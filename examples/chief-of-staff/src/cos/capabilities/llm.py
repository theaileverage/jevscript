"""The built-in `llm` capability: a writer that needs no model.

``write`` is deterministic and offline: with a playbook spec as ``using`` it
renders the playbook from its evidence (``learning.render_playbook``); with a
``{lines: [...]}`` record it joins the facts into short sentences; anything
else comes back as its plain text. A person who wants a model to write binds
an external `llm` adapter instead (``adapters.writer`` in config.json); its
playbook drafts still have to compile and pass replay verification.
"""

from __future__ import annotations

from typing import Any


class TemplateWriter:
    kind = "llm"

    def call(self, verb: str, args: dict[str, Any], capability: str | None = None) -> Any:
        from ..learning import render_playbook

        if verb != "write":
            raise LookupError(f"`writer` has no verb `{verb}`")
        using = (args.get("named") or {}).get("using")
        if isinstance(using, dict) and "category" in using and "evidence" in using:
            return render_playbook(using)
        if isinstance(using, dict) and isinstance(using.get("lines"), list):
            return " ".join(str(line).rstrip(".") + "." for line in using["lines"] if line) or "Nothing changed."
        if using is not None:
            return using if isinstance(using, str) else str(using)
        positional = list(args.get("positional") or [])
        return str(positional[0]) if positional else ""
