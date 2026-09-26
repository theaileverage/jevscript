"""Build only platform wheels with a staged, hash-pinned CLI."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path

from hatchling.builders.hooks.plugin.interface import BuildHookInterface


class CustomBuildHook(BuildHookInterface):
    """Fail closed when a wheel lacks its exact release binary."""

    def initialize(self, version: str, build_data: dict) -> None:
        if version == "editable":
            # A source checkout passes its own CLI through `bin=` or JEVSCRIPT_BIN.
            return
        tag =os.environ.get("JEVSCRIPT_WHEEL_TAG")
        target = os.environ.get("JEVSCRIPT_TARGET")
        if not tag or not target or tag != f"py3-none-{os.environ.get('JEVSCRIPT_PLATFORM_TAG')}":
            raise RuntimeError("set JEVSCRIPT_TARGET, JEVSCRIPT_PLATFORM_TAG and JEVSCRIPT_WHEEL_TAG")
        root = Path(self.root)
        binary = root / "src" / "jevscript" / "_bin" / ("jevscript.exe" if target == "win32-x64" else "jevscript")
        manifest_path = binary.parent / "manifest.json"
        if not binary.is_file() or not manifest_path.is_file():
            raise RuntimeError("stage the release CLI and manifest before building a wheel")
        manifest = json.loads(manifest_path.read_text())
        if manifest["version"] != self.metadata.version:
            raise RuntimeError("CLI and Python SDK versions differ")
        if hashlib.sha256(binary.read_bytes()).hexdigest() != manifest["targets"][target]["sha256"]:
            raise RuntimeError("staged CLI digest mismatch")
        skill = root / "src" / "jevscript" / "skills" / "jevscript" / "SKILL.md"
        if not skill.is_file() or hashlib.sha256(skill.read_bytes()).hexdigest() != manifest["skill_sha256"]:
            raise RuntimeError("staged Skill digest mismatch")
        build_data["tag"] = tag
        build_data["pure_python"] = False
