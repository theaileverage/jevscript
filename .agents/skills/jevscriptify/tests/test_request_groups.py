import os
from pathlib import Path
import subprocess
import sys
import unittest


ROOT = Path(__file__).resolve().parents[4]
HELPER = ROOT / ".agents/skills/jevscriptify/scripts/request_groups.py"
CLI = Path(os.environ.get("JEVSCRIPT_BIN", ROOT / "target/debug/jevscript"))


class RequestGroupsTest(unittest.TestCase):
    def report(self, source, *options):
        result = subprocess.run(
            [
                sys.executable,
                str(HELPER),
                str(source),
                "--jevscript",
                str(CLI),
                *options,
            ],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=True,
        )
        return result.stdout

    def assert_order(self, output, *parts):
        position = 0
        for part in parts:
            found = output.find(part, position)
            self.assertGreaterEqual(
                found, 0, f"missing or out of order: {part}\n{output}"
            )
            position = found + len(part)

    def test_compiled_nested_effects_and_judgment_logs(self):
        output = self.report(Path(__file__).with_name("request_groups.jev"))
        judgment, task = output.split("\ntask main", 1)
        self.assert_order(
            judgment,
            "first = message feels",
            "many = messages each feels",
            "calls def echo",
        )
        self.assertIn("one logical group", judgment)
        self.assertIn(
            "many = messages each feels (one question per runtime item)", judgment
        )
        self.assertIn("group 0: 2 judgment expressions", judgment)
        self.assertIn(
            "network request count depends on the profile question cap", judgment
        )
        self.assert_order(
            task,
            "effect agent.spawn [agent]",
            "group 0",
            "effect dev.observe [handle]",
            "effect tree.note [tool]",
            "group 1",
            "effect dev.observe [handle]",
            "calls focus (Jev)",
            "group 2",
            "effect tree.diff [tool]",
            "calls def echo",
            "effect dev.send [handle]",
            "calls judgment read (one logical group)",
            "effect dev.stop [handle]",
        )

    def test_existing_shape_effects_are_visible(self):
        output = self.report(ROOT / "examples/fix_issue_inline.jev")
        task = output.split("\ntask main", 1)[1]
        self.assert_order(
            task,
            "effect dev.observe [handle]",
            "calls focus (Jev)",
            "effect tree.diff [tool]",
        )

    def test_imported_units_show_mapped_effects(self):
        output = self.report(ROOT / "examples/fix_issue.jev", "--all")
        self.assertIn("task harness.", output)
        self.assertIn("effect tree.diff [tool]", output)

    def test_comprehension_boundaries_follow_runtime_calls(self):
        source = Path(__file__).with_name("comprehension_order.jev")
        task = self.report(source).split("\ntask main", 1)[1]
        self.assert_order(
            task, "calls def source", "calls def keep", "calls def render"
        )

        run = subprocess.run(
            [str(CLI), "run", str(source)],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=True,
        )
        self.assert_order(run.stderr, "iterable", "filter a", "render a", "filter b")
        self.assertNotIn("render b", run.stderr)

    def test_supervisor_guards_precede_any_approved_action(self):
        source = ROOT / "examples/chief-of-staff/jev/supervise.jev"
        machine = self.report(source, "--all").split("\nmachine lifecycle", 1)[1]
        approved = machine.split("state approved (possible step)", 1)[1].split(
            "state landing (possible step)", 1
        )[0]
        self.assert_order(
            approved,
            "guard land (if evaluated)",
            "guard merge (if evaluated)",
            "guard no_pr (if evaluated)",
            "machine Choice (if any event is enabled)",
            "action land (if chosen)",
            "effect fleet.land [tool]",
        )


if __name__ == "__main__":
    unittest.main()
