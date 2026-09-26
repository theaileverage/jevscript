"""``cos demo``: the whole loop offline, in a scratch directory.

A throwaway git project, a home bound to the offline Jev and the fake agent
adapter, and a handful of requests: three routine documentation fixes that
land, a learning pass that turns them into a playbook, and a fourth request
that the playbook handles. Every wake is a real Jevscript episode with a
real recording; only the model and the agent are stand-ins.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

from .commands import Commands
from .state.home import Home

RULES = [
    # Intake: these requests are routine documentation changes.
    {"match": {"id": "^work$"}, "answer": {"choice": "ship"}},
    {"match": {"id": "^project$"}, "answer": {"choice": "i0"}},
    {"match": {"id": "^kinds\\["}, "answer": {"choice": "docs_change"}},
    {"match": {"id": "^routine$", "text": "(?i)readme|docs|typo"}, "answer": {"noul": 0.9}},
    {"match": {"id": "^pb$"}, "answer": {"item": "documentation"}},
    {"match": {"id": "^applies$", "text": "(?i)readme|docs|typo"}, "answer": {"noul": 0.92}},
    {"match": {"id": "^repeated$"}, "answer": {"noul": 0.9}},
    {"match": {"id": "^nature$"}, "answer": {"choice": "automatable"}},
    {"match": {"id": "^efficiency$"}, "answer": {"score": 1}},
]


def make_project(root: Path) -> Path:
    project = root / "notes-site"
    project.mkdir(parents=True, exist_ok=True)
    run = lambda *a: subprocess.run(["git", "-C", str(project), *a], check=True, capture_output=True)  # noqa: E731
    run("init", "-q", "-b", "main")
    (project / "README.md").write_text("# Notes site\n\nA tiny site.\n")
    run("add", ".")
    run("-c", "user.name=demo", "-c", "user.email=demo@example.invalid", "commit", "-qm", "initial")
    return project


def configure(home: Home, root: Path) -> None:
    rules = root / "fake-jev-rules.json"
    rules.write_text(json.dumps(RULES))
    home.set_config("jev.mode", "fake")
    home.set_config("jev.fake_rules", str(rules))
    home.set_config("adapters.crew", {
        "command": [sys.executable, "-m", "cos.fake_agent"],
        "env": {"COS_FAKE_AGENT_STATE": str(root / "fake-agent-state.json"), "PYTHONPATH": str(Path(__file__).resolve().parents[1])},
    })
    home.set_config("policy.min_repeats", 3)
    home.set_config("learn_every", 3)
    home.set_config("mates_inline", True)


def run_demo(root: Path) -> None:
    root.mkdir(parents=True, exist_ok=True)
    project = make_project(root)
    home = Home(root / "home").init()
    configure(home, root)
    with Commands(home) as cos:
        cos.registry.add_project("notes-site", str(project), "the notes website: pages, README and documentation", mode="local-only", yolo=True)
        print(f"Demo home: {home.root}\nProject: {project}\n")
        for text in ("Fix the typo in the README title", "Add a docs page about installing", "Correct the README link to the docs"):
            print(f"> cos say {text!r}\n{cos.say(text).text}")
            for _ in range(6):
                cos.tick()
            print(cos.status().text + "\n")
        print("> cos learn")
        print(cos.learn().text)
        print(cos.playbooks().text + "\n")
        text = "Fix the broken docs index page"
        print(f"> cos say {text!r}\n{cos.say(text).text}")
        for _ in range(6):
            cos.tick()
        print("\n> cos bearings")
        print(cos.bearings().text)
        log = subprocess.run(["git", "-C", str(project), "log", "--oneline"], capture_output=True, text=True).stdout
        print(f"\nThe project's main branch now reads:\n{log}")
