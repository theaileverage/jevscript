#!/usr/bin/env python3
"""Score the cookbook's three benchmark arms or run an explicit paid benchmark."""

from __future__ import annotations

import argparse
import json
import shlex
import subprocess
import sys
from pathlib import Path
from typing import Any

import openrouter_agent
import runner


UPSTREAM = {
    "source": "https://docs.typesafe.ai/cookbooks/skill_suggestion",
    "rendered": "2026-07-31",
    "typesafe_model": "jev-1.12",
    "agent_model": "claude-haiku-4-5-20251001",
    "requests": 488,
    "covered": 315,
    "covered_distinct_skills": 171,
    "uncovered": 173,
    "arms": {
        "baseline": {"wrong_load": 0.168, "needless_load": 0.098},
        "typesafe_python_suggestion": {"wrong_load": 0.073, "needless_load": 0.040},
        "oracle": {"wrong_load": 0.025, "needless_load": 0.012},
    },
    "moved": {"fixed": 37, "broken": 7, "covered": 315},
    "classification": "upstream published/static; not reproduced locally",
}


def suggestion_block(name: str | None) -> str:
    body = (
        f"Relevant to the current request: {name}. Ignore this if it does not fit what the user actually asked for."
        if name
        else "No skill in the roster appears relevant to this request."
    )
    return f"\n\n<skill_relevance>\n{body}\n</skill_relevance>"


def score(rows: list[dict[str, Any]]) -> dict[str, dict[str, float]]:
    result: dict[str, dict[str, float]] = {}
    for arm in ("baseline", "jevscript_suggestion", "oracle"):
        arm_rows = [row for row in rows if row["arm"] == arm]
        positives = [row for row in arm_rows if row.get("gold")]
        negatives = [row for row in arm_rows if not row.get("gold")]
        if not positives or not negatives:
            raise ValueError(f"arm {arm!r} needs covered and uncovered rows")
        hits = [row.get("loaded", [])[:1] == [row["gold"]] for row in positives]
        over = [bool(row.get("loaded")) for row in negatives]
        result[arm] = {
            "wrong_load": 1 - sum(hits) / len(hits),
            "needless_load": sum(over) / len(over),
            "covered": len(positives),
            "uncovered": len(negatives),
        }
    return result


def run_agent(command: str, request: str, suggestion: str, arm: str) -> dict[str, Any]:
    try:
        completed = subprocess.run(
            shlex.split(command),
            input=json.dumps({"request": request, "suggestion": suggestion, "arm": arm}),
            check=True,
            capture_output=True,
            text=True,
        )
    except subprocess.CalledProcessError as exc:
        detail = exc.stderr.strip() or "no stderr"
        raise RuntimeError(f"agent command failed for {arm}: {detail}") from exc
    response = json.loads(completed.stdout)
    if not isinstance(response.get("loaded"), list):
        raise ValueError("agent command must return JSON with a `loaded` list")
    return {**response, "loaded": [str(item) for item in response["loaded"]]}


def live_benchmark(args: argparse.Namespace) -> dict[str, Any]:
    if not args.confirm_paid_run:
        raise SystemExit(
            "refusing paid calls; pass --confirm-paid-run after reviewing README.md"
        )
    requests = json.loads(args.requests.read_text(encoding="utf-8"))
    positives = [row for row in requests if row.get("gold")]
    negatives = [row for row in requests if not row.get("gold")]
    if not args.allow_custom_dataset and (len(requests), len(positives), len(negatives)) != (488, 315, 173):
        raise ValueError("the upstream-sized run requires exactly 488/315/173 rows")
    output_rows: list[dict[str, Any]] = []
    jev_results: list[dict[str, Any]] = []
    for index, row in enumerate(requests, start=1):
        jev_result = runner.live(row["text"], args.typesafe_model, None)
        jev_results.append(jev_result)
        decision = jev_result["outputs"]["suggestion"]
        prompts = {
            "baseline": "",
            "jevscript_suggestion": suggestion_block(decision),
            "oracle": suggestion_block(row.get("gold")),
        }
        for arm, prompt in prompts.items():
            agent_result = run_agent(args.agent_command, row["text"], prompt, arm)
            output_rows.append(
                {
                    "arm": arm,
                    "text": row["text"],
                    "gold": row.get("gold"),
                    "suggestion": decision,
                    "jev_usage": jev_result.get("usage", {}),
                    **agent_result,
                }
            )
        print(f"completed {index}/{len(requests)}", flush=True)
    args.output.write_text(
        "".join(json.dumps(row, ensure_ascii=False) + "\n" for row in output_rows),
        encoding="utf-8",
    )
    jev_usage = [result.get("usage", {}) for result in jev_results]
    agent_usage = [row.get("usage", {}) for row in output_rows]
    return {
        "classification": "live local smoke benchmark; not the upstream 488-row benchmark",
        "requests": len(requests),
        "typesafe_model": args.typesafe_model,
        "agent_model": args.agent_model,
        "turns": len(output_rows),
        "typesafe_usage": {
            "calls": sum(int(item.get("calls", 0)) for item in jev_usage),
            "tokens": sum(int(item.get("tokens", 0)) for item in jev_usage),
            "usd": sum(float(item.get("usd", 0)) for item in jev_usage),
            "minutes": sum(float(item.get("minutes", 0)) for item in jev_usage),
        },
        "agent_usage": {
            "prompt_tokens": sum(int(item.get("prompt_tokens", 0)) for item in agent_usage),
            "completion_tokens": sum(
                int(item.get("completion_tokens", 0)) for item in agent_usage
            ),
            "cost": sum(float(item.get("cost", 0)) for item in agent_usage),
            "latency_seconds": sum(
                float(row.get("latency_seconds", 0)) for row in output_rows
            ),
        },
        "scores": score(output_rows),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("published", help="print the upstream published/static results")
    score_parser = sub.add_parser("score", help="score captured JSONL agent turns")
    score_parser.add_argument("turns", type=Path)
    live_parser = sub.add_parser("live", help="opt-in full benchmark; can make up to 976 Jev and 1464 agent calls")
    live_parser.add_argument("--requests", type=Path, required=True)
    live_parser.add_argument("--agent-command")
    live_parser.add_argument("--agent-model", default=openrouter_agent.DEFAULT_MODEL)
    live_parser.add_argument("--output", type=Path, required=True)
    live_parser.add_argument("--typesafe-model", default="jev-latest")
    live_parser.add_argument("--confirm-paid-run", action="store_true")
    live_parser.add_argument("--allow-custom-dataset", action="store_true")
    args = parser.parse_args()
    if args.command == "live" and args.agent_command is None:
        args.agent_command = shlex.join(
            [sys.executable, str(Path(openrouter_agent.__file__).resolve()), "--model", args.agent_model]
        )
    if args.command == "published":
        result = UPSTREAM
    elif args.command == "score":
        rows = [json.loads(line) for line in args.turns.read_text(encoding="utf-8").splitlines() if line]
        result = score(rows)
    else:
        result = live_benchmark(args)
    print(json.dumps(result, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
