"""Retained, fake-backed S1 CLI acceptance probe."""

import json
import os
import subprocess
import sys
from pathlib import Path

from cos.demo import RULES, configure, make_project
from cos.state.home import Home, read_jsonl
from cos.state.registry import Registry


repo = Path.cwd()
root = Path(sys.argv[1])
home = Home(root / "home").init()
project = make_project(root)
configure(home, root)
rules = [
    {"match": {"id": "^work$", "text": "going on"}, "answer": {"choice": "status"}},
    *RULES,
]
rule_file = root / "status-jev-rules.json"
rule_file.write_text(json.dumps(rules), encoding="utf-8")
home.set_config("jev.fake_rules", str(rule_file))
Registry(home).add_project("notes-site", str(project), "notes and guide", mode="local-only", yolo=True)

env = {**os.environ, "JEVSCRIPT_BIN": str(repo / "target/debug/jevscript"), "COS_JEV": "fake"}
prefix = ["uv", "run", "--project", "examples/chief-of-staff", "--frozen", "--offline", "cos", "--home", str(home.root)]
evidence = {"head": subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, check=True).stdout.strip(), "home": str(home.root), "commands": [], "snapshots": {}}
log = root / "route-cli.log"


def run(label, argv):
    result = subprocess.run(argv, env=env, capture_output=True, text=True, check=False)
    row = {"label": label, "argv": argv, "exit": result.returncode, "stdout": result.stdout, "stderr": result.stderr}
    evidence["commands"].append(row)
    with log.open("a", encoding="utf-8") as out:
        out.write(f"\n[{label}] {' '.join(argv)}\nexit={result.returncode}\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}\n")
    assert result.returncode == 0, f"{label}: {result.stderr}"
    return result.stdout.strip()


def snapshot(label):
    recordings = sorted(str(p) for p in (home.state / "recordings").glob("*.jsonl"))
    episodes = [row for row in read_jsonl(home.data / "ledger.jsonl") if row.get("type") == "episode"]
    outbox = read_jsonl(home.state / "outbox.jsonl")
    row = {"recordings": recordings, "episodes": episodes, "outbox": outbox}
    evidence["snapshots"][label] = row
    with log.open("a", encoding="utf-8") as out:
        out.write(f"[{label}] recordings={len(recordings)} episodes={len(episodes)} outbox={len(outbox)}\n")
    return row


try:
    expected = run("initial status", prefix + ["status"])
    initial = snapshot("initial")
    assert not initial["recordings"] and not initial["episodes"]

    run("status request", prefix + ["say", "What's going on?"])
    run("status tick", prefix + ["tick"])
    after_status = snapshot("after_status")
    assert any("-on_wake-" in Path(path).name for path in after_status["recordings"])
    assert any(row.get("task") == "on_wake" for row in after_status["episodes"])
    assert after_status["outbox"][-1]["message"] == expected

    status = run("status after route", prefix + ["status"])
    before_bearings = snapshot("before_bearings")
    bearings = run("bearings after route", prefix + ["bearings"])
    assert bearings.splitlines()[0] == f"Bearings: {status}"
    after_bearings = snapshot("after_bearings")
    assert len(after_bearings["recordings"]) == len(before_bearings["recordings"])
    assert len(after_bearings["episodes"]) == len(before_bearings["episodes"])

    run("ordinary request", prefix + ["say", "Update the guide", "--project", "notes-site"])
    run("dispatch tick", prefix + ["tick"])
    after_dispatch = snapshot("after_dispatch")
    assert len(after_dispatch["recordings"]) > len(after_bearings["recordings"])
    assert any(row.get("subject") == "backlog" for row in after_dispatch["episodes"])
    assert not any("-bearings-" in Path(path).name for path in after_dispatch["recordings"])

    final_status = run("final status", prefix + ["status"])
    final_before = snapshot("before_final_bearings")
    final_bearings = run("final bearings", prefix + ["bearings"])
    assert final_bearings.splitlines()[0] == f"Bearings: {final_status}"
    final_after = snapshot("after_final_bearings")
    assert len(final_after["recordings"]) == len(final_before["recordings"])
    assert len(final_after["episodes"]) == len(final_before["episodes"])

    for index, path in enumerate(final_after["recordings"], start=1):
        run(f"replay {index}", [env["JEVSCRIPT_BIN"], "replay", path])
    evidence["verdict"] = "pass"
finally:
    (root / "route-evidence.json").write_text(json.dumps(evidence, indent=2), encoding="utf-8")
