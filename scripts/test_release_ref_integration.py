"""Exercise the staging and publishing ref gate through real Git history."""

from __future__ import annotations

from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


CHECKER = Path(__file__).resolve().parent / "check_release_ref.py"


class ReleaseRefIntegrationTests(unittest.TestCase):
    """A matching tag and digest cannot release an unmerged commit."""

    def test_tagged_merged_commit_required(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def git(*args: str) -> str:
                return subprocess.check_output(["git", *args], cwd=root, text=True).strip()

            def run(commit: str, ref_type: str = "tag") -> subprocess.CompletedProcess[str]:
                return subprocess.run([sys.executable, str(CHECKER), "--commit", commit,
                                       "--tag", "v0.1.0", "--ref-type", ref_type],
                                      cwd=root, capture_output=True, text=True)

            git("init", "-b", "main")
            git("config", "user.email", "release-test@example.test")
            git("config", "user.name", "Release Test")
            (root / "Cargo.toml").write_text('[workspace.package]\nversion = "0.1.0"\n')
            git("add", "Cargo.toml")
            git("commit", "-m", "merged release source")
            merged = git("rev-parse", "HEAD")
            git("update-ref", "refs/remotes/origin/main", merged)
            git("tag", "v0.1.0")
            self.assertEqual(run(merged).returncode, 0)
            branch = run(merged, ref_type="branch")
            self.assertNotEqual(branch.returncode, 0)
            self.assertIn("tag ref", branch.stderr)

            git("checkout", "-b", "unmerged")
            (root / "unreviewed.txt").write_text("unmerged source")
            git("add", "unreviewed.txt")
            git("commit", "-m", "unmerged source")
            unmerged = git("rev-parse", "HEAD")
            git("tag", "-f", "v0.1.0")
            result = run(unmerged)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("not merged into main", result.stderr)


if __name__ == "__main__":
    unittest.main()
