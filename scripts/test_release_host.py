"""The install smoke can identify its host without Python 3.11 modules."""

from __future__ import annotations

import subprocess
import sys
import unittest
from pathlib import Path


class HostTargetTests(unittest.TestCase):
    def test_import_and_target_without_tomllib(self) -> None:
        script = """
import builtins
original = builtins.__import__
def without_tomllib(name, *args, **kwargs):
    if name == 'tomllib':
        raise ModuleNotFoundError(name)
    return original(name, *args, **kwargs)
builtins.__import__ = without_tomllib
from build_release_cli import host_target
print(host_target())
"""
        result = subprocess.run([sys.executable, "-c", script], cwd=Path(__file__).parent, capture_output=True, text=True, check=True)
        self.assertIn(result.stdout.strip(), {"darwin-arm64", "darwin-x64", "linux-arm64-gnu", "linux-x64-gnu", "win32-x64"})


if __name__ == "__main__":
    unittest.main()
