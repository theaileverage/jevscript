"""Recall of the Skill text filter (S6) over labelled dispatches.

For each labelled dispatch the shortlist must contain every Skill a
full-catalog judgment should select (recall 1.0 at the default k). The
comparison runs the host's own search twice: with the request's words alone
(the fallback) and with the scout's terms added.

The scout outputs in ``fixtures/skill_scout_outputs.json`` were written by
the real scout models through ``HarnessScout``, one sample per model and
round, keyed by request; a sample covers only the dispatches that existed
when it was captured. ``COS_LIVE_SCOUT=claude`` or ``codex`` (marked
``live``) asks the model again.
"""

from __future__ import annotations

import json
import os
import re
from pathlib import Path
from typing import Any

import pytest

from cos.capabilities.scout import HarnessScout
from cos.skills import Skills
from cos.state.home import DEFAULT_CONFIG, Home
from skill_fixture import DISPATCHES, catalog_texts, write_catalog

OUTPUTS = Path(__file__).parent / "fixtures" / "skill_scout_outputs.json"
JEV = Path(__file__).resolve().parents[1] / "jev" / "skills.jev"


def scout_instruction() -> str:
    """The exact instruction ``skills.jev`` sends, so a live check asks what dispatch asks."""
    literal = re.search(r'scout\.write "((?:[^"\\]|\\.)*)" using', JEV.read_text()).group(1)
    return literal.encode().decode("unicode_escape")


@pytest.fixture(scope="module")
def skills(tmp_path_factory: pytest.TempPathFactory) -> Skills:
    root = tmp_path_factory.mktemp("recall")
    home = Home(root / "home").init()
    home.set_config("skill_catalog", write_catalog(root / "catalog", catalog_texts()))
    skills = Skills(home)
    assert len(skills.catalog()) == 1000
    return skills


def recall(skills: Skills, outputs: dict[str, str] | None) -> list[dict[str, Any]]:
    """One row per labelled dispatch the outputs cover; ``None`` searches with request words alone."""
    policy = DEFAULT_CONFIG["policy"]
    rows = []
    for index, dispatch in enumerate(DISPATCHES):
        if outputs is not None and dispatch["request"] not in outputs:
            continue
        text = "" if outputs is None else outputs[dispatch["request"]]
        found = skills.search({"id": f"t{index}"}, text, dispatch["request"], policy["skill_shortlist"], policy)
        missed = [skill_id for skill_id in dispatch["relevant"] if skill_id not in found["ids"]]
        rows.append({"dispatch": dispatch, "missed": missed, "terms": found["terms"], "fallback": found["fallback_reason"]})
    return rows


def report(label: str, rows: list[dict[str, Any]]) -> float:
    relevant = sum(len(row["dispatch"]["relevant"]) for row in rows)
    found = relevant - sum(len(row["missed"]) for row in rows)
    parts = []
    for name, key in (("held out", "held_out"), ("paraphrased", "paraphrase")):
        group = [row for row in rows if row["dispatch"].get(key)]
        if group:
            parts.append(f"{name} {sum(len(r['dispatch']['relevant']) - len(r['missed']) for r in group)}/{sum(len(r['dispatch']['relevant']) for r in group)}")
    print(f"\n{label}: {len(rows)} dispatches, recall {found}/{relevant} = {found / relevant:.3f}" + "".join(f"; {part}" for part in parts))
    for row in rows:
        if row["missed"]:
            print(f"  missed {row['missed']} for {row['dispatch']['request']!r} with terms {row['terms']}")
    return found / relevant


def test_recorded_scout_terms_reach_full_recall(skills: Skills) -> None:
    """S6 recall check: at the default k every labelled Skill reaches Jev for
    each recorded scout sample. Request words alone are reported beside it,
    over the same dispatches and over all of them."""
    report("request words only, all dispatches", recall(skills, None))
    for sample in json.loads(OUTPUTS.read_text())["samples"]:
        rows = recall(skills, sample["outputs"])
        assert not any(row["fallback"] for row in rows), rows
        covered = {row["dispatch"]["request"]: "" for row in rows}
        report(f"request words only, {sample['round']} dispatches", recall(skills, covered))
        assert report(f"{sample['round']} {sample['harness']} scout ({sample['model']})", rows) == 1.0


@pytest.mark.live
@pytest.mark.skipif(not os.environ.get("COS_LIVE_SCOUT"), reason="set COS_LIVE_SCOUT=claude or codex to ask the real scout model")
def test_live_scout_recall(skills: Skills) -> None:
    """Asks the real scout once per labelled dispatch; ``COS_CAPTURE_SCOUT=<round>`` appends the sample."""
    harness = os.environ["COS_LIVE_SCOUT"]
    models = DEFAULT_CONFIG["scout"]["models"]
    scout = HarnessScout(models)
    outputs = {}
    for dispatch in DISPATCHES:
        using = {"task": dispatch["request"], "request": dispatch["request"], "harness": harness, "model": models[harness], "max_terms": DEFAULT_CONFIG["policy"]["skill_terms"]}
        outputs[dispatch["request"]] = scout.call("write", {"positional": [scout_instruction()], "named": {"using": using}})
    if os.environ.get("COS_CAPTURE_SCOUT"):
        recorded = json.loads(OUTPUTS.read_text())
        recorded["samples"].append({"round": os.environ["COS_CAPTURE_SCOUT"], "harness": harness, "model": models[harness], "outputs": outputs})
        OUTPUTS.write_text(json.dumps(recorded, indent=2) + "\n")
    assert report(f"live {harness} scout ({models[harness]})", recall(skills, outputs)) == 1.0
