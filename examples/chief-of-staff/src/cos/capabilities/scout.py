"""The built-in `scout` capability: a cheaper model writes Skill search terms.

The scout follows the harness the item is dispatched to: Claude Code asks
Haiku and Codex asks Luna, through the same CLI the person already signed in
to. ``config.json`` maps each harness to an exact model id under
``scout.models``; a harness with a null entry or no entry has no scout, and the program falls
back to the request's own words instead of borrowing another family.

``write`` receives the program's ``using`` record (spec section 9.3)
``{task, request, notes, harness, model, max_terms}``. The model id is in the
record so the recording shows which model wrote the terms; the adapter still
refuses any id the host did not configure for that harness. Its text is only
a search query: the host parses it against a fixed schema (``skill_search``).
"""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
from pathlib import Path
from typing import Any

from ..state.home import native_scout_models
from .agent import AdapterError

CONTEXT = ("task", "request", "notes", "max_terms")
SCOUT_FAILURE = "\x1ecos-scout-failure:"
SCOUT_TIMEOUT_SECONDS = 120


class HarnessScout:
    kind = "llm"

    def __init__(self, models: dict[str, str | None]) -> None:
        self.models = native_scout_models(models)

    def _argv(self, harness: str, model: str, workdir: str, out: Path) -> list[str]:
        binary = "codex" if harness == "codex" else "claude"
        executable = shutil.which(binary)
        if executable is None:
            raise FileNotFoundError(f"the scout for harness {harness!r} needs `{binary}` on PATH")
        if binary == "codex":
            return [executable, "exec", "-m", model, "--ephemeral", "--sandbox", "read-only", "--skip-git-repo-check", "-C", workdir, "-o", str(out), "-"]
        return [executable, "-p", "--model", model, "--output-format", "text", "--no-session-persistence", "--tools", ""]

    def call(self, verb: str, args: dict[str, Any], capability: str | None = None) -> str:
        if verb != "write":
            raise LookupError(f"`scout` has no verb `{verb}`")
        using = (args.get("named") or {}).get("using")
        positional = list(args.get("positional") or [])
        if not isinstance(using, dict) or not positional:
            raise AdapterError("the scout needs an instruction and a using record")
        harness, model = using.get("harness"), using.get("model")
        if harness not in self.models or self.models[harness] != model:
            raise AdapterError(f"no scout model {model!r} is configured for harness {harness!r}")
        prompt = "\n".join([str(positional[0]), ""] + [f"{key}: {using[key]}" for key in CONTEXT if using.get(key) not in (None, "")])
        env = {key: value for key, value in os.environ.items() if key != "TYPESAFE_API_KEY"}
        with tempfile.TemporaryDirectory(prefix="cos-scout-") as workdir:
            out = Path(workdir) / "answer.txt"
            try:
                argv = self._argv(harness, model, workdir, out)
            except FileNotFoundError as error:
                return SCOUT_FAILURE + str(error)
            try:
                done = subprocess.run(  # noqa: S603 - the person's own signed-in agent CLI
                    argv, input=prompt, capture_output=True, text=True, cwd=workdir, env=env, timeout=SCOUT_TIMEOUT_SECONDS, check=False
                )
            except FileNotFoundError:
                return SCOUT_FAILURE + f"the {harness} scout executable is unavailable"
            except subprocess.TimeoutExpired:
                return SCOUT_FAILURE + f"the {harness} scout did not answer within {SCOUT_TIMEOUT_SECONDS:g}s"
            if done.returncode != 0:
                detail = " ".join(done.stderr.split())[-300:]
                return SCOUT_FAILURE + f"the {harness} scout exited {done.returncode}: {detail}"
            answer = out.read_text(encoding="utf-8") if harness == "codex" and out.exists() else done.stdout
            return "\n" + answer if answer.startswith(SCOUT_FAILURE) else answer
