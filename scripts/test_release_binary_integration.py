"""Exercise the release binary floor check through its CLI and otool boundary."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


CHECKER = Path(__file__).resolve().parent / "check_macos_binary.py"


class MacosMinimumIntegrationTests(unittest.TestCase):
    """A mislabeled wheel must fail before its binary is staged."""

    def test_final_macho_floor(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "jevscript"
            binary.write_bytes(b"fixture binary")
            tool = root / "otool"
            tool.write_text("#!/usr/bin/env python3\nimport os\nprint(os.environ['OTOOL_OUTPUT'])\n")
            tool.chmod(0o755)
            env = {**os.environ, "PATH": f"{root}{os.pathsep}{os.environ['PATH']}"}

            def run(target: str, minimum: str) -> subprocess.CompletedProcess[str]:
                env["OTOOL_OUTPUT"] = f"{binary}:\nLoad command 0\n      cmd LC_BUILD_VERSION\n    minos {minimum}\n"
                return subprocess.run([sys.executable, str(CHECKER), "--target", target, str(binary)],
                                      env=env, capture_output=True, text=True)

            self.assertEqual(run("darwin-arm64", "11.0").returncode, 0)
            self.assertEqual(run("darwin-x64", "10.15.0").returncode, 0)
            too_new = run("darwin-x64", "11.0")
            self.assertNotEqual(too_new.returncode, 0)
            self.assertIn("wheel advertises", too_new.stderr)


if __name__ == "__main__":
    unittest.main()
