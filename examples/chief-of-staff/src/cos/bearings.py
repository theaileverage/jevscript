"""Bearings: gather fleet facts and render a read-only digest.

The bearings digest covers what needs the owner, what is under way, what is
queued or held, what finished, plus second mates and learned playbooks. What
goes in each section is formatting of stored facts; dispatch policy remains
in Jevscript.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from .state.home import iso, now

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
    backlog = host.backlog.items()
    done = host.backlog.done_history()
    statuses = {item["id"]: item["status"] for item in done}
    statuses.update((item["id"], item["status"]) for item in backlog)
    return {
        "mode": host.home.mode,
        "decisions": [{"key": d["key"], "question": d["question"]} for d in host.decisions.open()],
        "workers": [{"id": w["id"], "title": w.get("title", w["id"]), "project": w.get("project"), "phase": w.get("phase", "working")} for w in host.workers.all()],
        "backlog": [{"id": i["id"], "title": i["title"], "status": i["status"], "hold": i.get("hold"), "deps": i.get("deps") or []} for i in backlog],
        "dependency_statuses": statuses,
        "finished": [{"id": i["id"], "title": i["title"], "outcome": i.get("outcome", i["status"])} for i in reversed(done[-5:])],
        "mates": host.mates.summaries(),
        "playbooks": [{"name": n, "version": r["version"], "state": "on" if r.get("active") else "off"} for n, r in registry.items()],
    }


def headline(workers: int, queued: int, decisions: int) -> str:
    if workers == queued == decisions == 0:
        return "All quiet: nothing under way and nothing waiting on you."
    return f"{workers} under way, {queued} queued, {decisions} waiting on you."


def digest(facts: dict[str, Any], at: float) -> dict[str, Any]:
    queued = [item for item in facts["backlog"] if item["status"] == "queued"]
    statuses = facts["dependency_statuses"]
    plain, waiting, held = [], [], []
    for item in queued:
        label = f"{item['title']} [{item['id']}]"
        hold = item.get("hold")
        details = []
        if hold is not None:
            until = hold.get("until")
            if until is None:
                detail = "held"
            elif until > at:
                detail = f"held until {iso(until)}"
            else:
                detail = f"hold expired {iso(until)}"
            details.append(f"{detail}, {hold.get('reason', '')}")
        if item["deps"]:
            deps = ", ".join(f"{dep} ({statuses.get(dep, 'unknown')})" for dep in item["deps"])
            details.append(f"after {deps}")
        row = f"{label}: {'; '.join(details)}" if details else label
        if hold is not None:
            held.append(row)
        elif item["deps"]:
            waiting.append(row)
        else:
            plain.append(row)
    return {
        "headline": headline(len(facts["workers"]), len(queued), len(facts["decisions"])),
        "mode": facts["mode"],
        "needs_you": [f"{d['question']} (answer with: cos answer {d['key']} ...)" for d in facts["decisions"]],
        "under_way": [f"{w['title']} [{w['id']}] in {w['project']}: {w['phase']}" for w in facts["workers"]],
        "queued": plain + waiting + held,
        "finished": [f"{item['title']} [{item['id']}]: {item['outcome']}" for item in facts["finished"]],
        "mates": [f"{mate['name']} ({mate['scope']}): {mate['in_flight']} under way, {mate['open_decisions']} waiting" for mate in facts["mates"]],
        "playbooks": [f"{book['name']} v{book['version']}: {book['state']}" for book in facts["playbooks"]],
    }


def render(host: "Host") -> str:
    d = digest(fleet(host), now())
    mode = d.get("mode", "normal")
    out = [f"Bearings{'' if mode == 'normal' else f' ({mode} mode)'}: {d.get('headline', '')}"]
    for key, title in SECTIONS:
        rows = d.get(key) or []
        if not rows and key in ("mates", "playbooks"):
            continue
        out.append(f"\n{title}")
        out += [f"  - {row}" for row in rows] or ["  - nothing"]
    return "\n".join(out)
