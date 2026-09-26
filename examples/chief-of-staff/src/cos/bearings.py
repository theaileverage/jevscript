"""Bearings: gather the fleet, let ``bearings.jev`` compose the digest, render it.

The bearings digest covers what needs the owner, what is under way, what is
queued or held, what finished, plus second mates and learned playbooks. What
goes in each section is decided in Jevscript (``bearings.digest``, code only,
no model); this module only collects the records and prints the sections.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:  # pragma: no cover
    from .host import Host

SECTIONS = [
    ("needs_you", "Needs you"),
    ("under_way", "Under way"),
    ("queued", "Queued and held"),
    ("finished", "Recently finished"),
    ("mates", "Second mates"),
    ("playbooks", "Learned playbooks"),
]


def fleet(host: "Host") -> dict[str, Any]:
    registry = host.learning.playbooks.registry()
    return {
        "mode": host.home.mode,
        "decisions": [{"key": d["key"], "question": d["question"]} for d in host.decisions.open()],
        "workers": [{"id": w["id"], "title": w.get("title", w["id"]), "project": w.get("project"), "phase": w.get("phase", "working")} for w in host.workers.all()],
        "backlog": [{"id": i["id"], "title": i["title"], "status": i["status"], "hold": i.get("hold"), "deps": i.get("deps") or []} for i in host.backlog.items()],
        "finished": [{"id": i["id"], "title": i["title"], "outcome": i.get("outcome", i["status"])} for i in reversed(host.backlog.done_history()[-5:])],
        "mates": host.mates.summaries(),
        "playbooks": [{"name": n, "version": r["version"], "state": "on" if r.get("active") else "off"} for n, r in registry.items()],
    }


def digest(host: "Host") -> dict[str, Any]:
    episode = host.run_task("bearings", {"fleet": fleet(host)}, subject="bearings")
    return episode.result or {}


def render(host: "Host") -> str:
    d = digest(host)
    mode = d.get("mode", "normal")
    out = [f"Bearings{'' if mode == 'normal' else f' ({mode} mode)'}: {d.get('headline', '')}"]
    for key, title in SECTIONS:
        rows = d.get(key) or []
        if not rows and key in ("mates", "playbooks"):
            continue
        out.append(f"\n{title}")
        out += [f"  - {row}" for row in rows] or ["  - nothing"]
    return "\n".join(out)


def headline(host: "Host") -> str:
    return str(digest(host).get("headline", ""))
