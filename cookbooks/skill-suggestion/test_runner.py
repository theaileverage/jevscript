from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import generate_roster  # noqa: E402
import runner  # noqa: E402
import benchmark  # noqa: E402
import openrouter_agent  # noqa: E402


class SkillSuggestionCookbookTests(unittest.TestCase):
    def test_roster_is_the_pinned_licensed_subset(self) -> None:
        roster = runner.load_roster()
        licenses = json.loads(runner.LICENSES_PATH.read_text(encoding="utf-8"))
        self.assertEqual(len(roster), 157)
        self.assertEqual(len({skill["category"] for skill in roster}), 29)
        self.assertEqual(len(generate_roster.render_prompt(roster)), 14_168)
        self.assertEqual(set(licenses), {skill["name"] for skill in roster})
        self.assertEqual(set(licenses.values()), {"MIT", "Apache-2.0"})
        self.assertFalse({"docx", "pdf", "powerpoint", "xlsx"} & set(licenses))
        by_name = {skill["name"]: skill for skill in roster}
        self.assertEqual(
            by_name["apple-notes"]["description"],
            "Manage Apple Notes via memo CLI: create, search, edit.",
        )
        self.assertEqual(
            by_name["pptx-author"]["description"],
            "Build PowerPoint decks headless with python-pptx.",
        )

    def test_generator_excludes_skills_without_explicit_permitted_license(self) -> None:
        with tempfile.TemporaryDirectory(prefix="skill-license-fixture-") as temp:
            root = Path(temp)
            for name, declaration in (
                ("permitted", "license: MIT\n"),
                ("undeclared", ""),
                ("restricted", "license: Proprietary\n"),
            ):
                directory = root / "skills" / name
                directory.mkdir(parents=True)
                (directory / "SKILL.md").write_text(
                    f"---\nname: {name}\ndescription: Fixture skill\n{declaration}---\n"
                    f"Fixture-only procedure for {name}.\n",
                    encoding="utf-8",
                )
            roster, licenses = generate_roster.build(root)
        self.assertEqual([skill["name"] for skill in roster], ["permitted"])
        self.assertEqual(licenses, {"permitted": "MIT"})

    def test_compiled_main_has_exactly_two_request_groups(self) -> None:
        completed = subprocess.run(
            [str(runner.BINARY), "compile", str(runner.PROGRAM)],
            cwd=runner.REPO,
            check=True,
            capture_output=True,
            text=True,
        )
        ir = json.loads(completed.stdout)
        main = next(task for task in ir["tasks"] if task["name"] == "main")

        def groups(value: object) -> list[int]:
            if isinstance(value, dict):
                found = [value["request_group"]] if "request_group" in value else []
                return found + [group for child in value.values() for group in groups(child)]
            if isinstance(value, list):
                return [group for child in value for group in groups(child)]
            return []

        self.assertEqual(sorted(set(groups(main))), [0, 1])

    def test_scripted_cases_exercise_gate_shortlist_and_abstention(self) -> None:
        results = {row["case"]: row for row in runner.verify()}
        self.assertEqual(results["apple_notes"]["suggestion"], "apple-notes")
        self.assertEqual(results["powerpoint_authoring"]["suggestion"], "pptx-author")
        self.assertIsNone(results["mastodon_no_exact_skill"]["suggestion"])
        self.assertIsNone(results["prose_only"]["suggestion"])
        self.assertEqual(results["apple_notes"]["questions_per_request"], [4, 4])
        self.assertEqual(results["prose_only"]["request_groups"], 1)

    def test_request_state_is_progressively_disclosed(self) -> None:
        _pause, requests = runner.scripted_case("apple_notes")
        self.assertEqual(set(requests[0]["state"]), {"request", "wide_roster"})
        self.assertEqual(
            set(requests[0]["state"]["wide_roster"][0]),
            {"name", "description", "label"},
        )
        self.assertEqual(set(requests[1]["state"]), {"request", "detailed"})
        self.assertEqual(set(requests[1]["state"]["detailed"][0]), {"name", "detail"})
        self.assertEqual([len(request["questions"]) for request in requests], [4, 4])

    def test_recording_replays_without_the_scripted_endpoint(self) -> None:
        sample = next(case for case in runner.load_samples() if case["id"] == "apple_notes")
        with tempfile.TemporaryDirectory(prefix="skill-suggestion-replay-") as temp:
            recording = Path(temp) / "apple-notes.jsonl"
            original, requests = runner.scripted_case("apple_notes", record=str(recording))
            self.assertEqual(len(requests), 2)
            events = [json.loads(line) for line in recording.read_text(encoding="utf-8").splitlines()]
            self.assertEqual(sum(event["event"] == "request" for event in events), 2)
            replayed = runner.replay(recording, sample["text"])
            self.assertEqual(replayed["outputs"], original["outputs"])

    def test_benchmark_metrics_keep_upstream_semantics(self) -> None:
        rows = []
        for arm, positive_load, negative_load in (
            ("baseline", [], ["xurl"]),
            ("jevscript_suggestion", ["apple-notes"], []),
            ("oracle", ["apple-notes"], []),
        ):
            rows.extend(
                [
                    {"arm": arm, "gold": "apple-notes", "loaded": positive_load},
                    {"arm": arm, "gold": None, "loaded": negative_load},
                ]
            )
        scores = benchmark.score(rows)
        self.assertEqual(scores["baseline"]["wrong_load"], 1.0)
        self.assertEqual(scores["baseline"]["needless_load"], 1.0)
        self.assertEqual(scores["jevscript_suggestion"]["wrong_load"], 0.0)
        self.assertEqual(scores["oracle"]["needless_load"], 0.0)

    def test_openrouter_agent_uses_exact_roster_and_requested_model(self) -> None:
        payload = openrouter_agent.request_payload(
            "Save this in Notes", "\n<skill_relevance>apple-notes</skill_relevance>", "test-model"
        )
        self.assertEqual(payload["model"], "test-model")
        self.assertEqual(
            payload["messages"][0]["content"].split("\n<skill_relevance>")[0],
            generate_roster.render_prompt(runner.load_roster()),
        )
        enum = payload["tools"][0]["function"]["parameters"]["properties"]["name"]["enum"]
        self.assertEqual(len(enum), 157)
        self.assertIn("apple-notes", enum)
        self.assertNotIn("powerpoint", enum)

    def test_openrouter_agent_extracts_skill_view_calls(self) -> None:
        response = {
            "choices": [
                {
                    "message": {
                        "tool_calls": [
                            {
                                "function": {
                                    "name": "skill_view",
                                    "arguments": '{"name":"apple-notes"}',
                                }
                            },
                            {"function": {"name": "other", "arguments": "{}"}},
                        ]
                    }
                }
            ]
        }
        self.assertEqual(openrouter_agent._loaded_skills(response), ["apple-notes"])


if __name__ == "__main__":
    unittest.main()
