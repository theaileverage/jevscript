"""Check the Runme runbooks in runbooks/ through the runme CLI (see runbooks/README.md)."""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RUNBOOKS = ROOT / "runbooks"
TAGS = ("inspection", "network-read", "local-mutation", "external-mutation")
FENCE = re.compile(r"^(`{3,}|~{3,})(.*)$")


def runme(*args: str) -> subprocess.CompletedProcess[str]:
    """Call the runme CLI from the repository root, the way an operator does."""
    # runme's --chdir drops project mode and reads ./README.md alone, so change directory instead.
    command = [os.environ.get("RUNME", "runme"), *args]
    return subprocess.run(command, cwd=ROOT, capture_output=True, text=True, check=False)


def fenced_blocks(path: Path) -> list[tuple[int, str, str]]:
    """Return each fenced block's opening line number, info string and body."""
    blocks = []
    open_fence = None
    for number, line in enumerate(path.read_text().splitlines(), 1):
        match = FENCE.match(line)
        if match is not None and open_fence is None:
            open_fence = match.group(1)
            blocks.append((number, match.group(2).strip(), ""))
        elif match is not None and match.group(1).startswith(open_fence) and not match.group(2).strip():
            open_fence = None
        elif open_fence is not None:
            start, info, body = blocks[-1]
            blocks[-1] = (start, info, body + line + "\n")
    return blocks


def main() -> None:
    """Fail on any cell Run All could misuse, any cell that does not dry-run, and runnable docs."""
    errors = []
    binary = os.environ.get("RUNME", "runme")
    if shutil.which(binary) is None:
        raise SystemExit(f"{binary} is not an executable; install Runme 3.17.5 or set RUNME")
    version = runme("--version")
    cells = {}
    for path in sorted(RUNBOOKS.glob("*.md")):
        relative = path.relative_to(ROOT).as_posix()
        for number, info, body in fenced_blocks(path):
            where = f"{relative}:{number}"
            language, _, attributes = info.partition(" ")
            try:
                parsed = json.loads(attributes) if attributes.strip() else {}
            except json.JSONDecodeError as error:
                errors.append(f"{where}: attributes are not JSON: {error}")
                continue
            name = parsed.get("name")
            if language != "bash" or not name:
                errors.append(f"{where}: every fenced block must be a named bash cell")
                continue
            if name in cells:
                errors.append(f"{where}: cell name {name} repeats {cells[name][0]}")
                continue
            if parsed.get("tag") not in TAGS:
                errors.append(f"{where}: {name} needs a tag from {', '.join(TAGS)}")
            if parsed.get("cwd") != ".." or parsed.get("interpreter") != "bash":
                errors.append(f"{where}: {name} must set \"cwd\":\"..\" and \"interpreter\":\"bash\"")
            if parsed.get("tag") != "inspection" and parsed.get("excludeFromRunAll") != "true":
                errors.append(f"{where}: {name} changes state or reads the network, so it must set excludeFromRunAll")
            # Runme runs cells with `bash -i`, which reports success after a failed `${NAME:?}`.
            if re.search(r"\$\{[^}]*:\?", body):
                errors.append(f"{where}: {name} must stop on missing input with an explicit exit, not ${{NAME:?}}")
            cells[name] = (where, relative, parsed.get("excludeFromRunAll") == "true")

    listed = runme("list", "--project", "runbooks", "--allow-unnamed", "--json")
    if listed.returncode != 0:
        raise SystemExit(f"runme list failed: {listed.stderr.strip()}")
    entries = json.loads(listed.stdout)
    for entry in entries:
        name = entry["name"]
        if not entry["named"] or name not in cells:
            errors.append(f"{entry['file']}: runme found block {name} that the check did not")
            continue
        where, relative, excluded = cells[name]
        if entry["file"] != relative:
            continue  # A repeated name, already reported.
        if entry["run_all"] == excluded:
            errors.append(f"{where}: runme disagrees with the excludeFromRunAll of {name}")
        dry = runme("run", "--filename", relative, "--dry-run", "--skip-prompts", name)
        # runme prints a dry run's script on stderr.
        script = dry.stderr
        header = script.splitlines()[:1]
        if dry.returncode != 0:
            errors.append(f"{where}: dry run of {name} failed: {dry.stderr.strip()}")
        elif not header or not header[0].startswith("#!") or not header[0].endswith("bash"):
            errors.append(f"{where}: dry run of {name} does not start a bash script")
        elif f'# run in "{ROOT}"' not in script:
            errors.append(f"{where}: {name} does not run in the repository root")
    missing = set(cells) - {entry["name"] for entry in entries}
    errors.extend(f"{cells[name][0]}: runme does not list {name}" for name in sorted(missing))

    # With no flags, runme at the root indexes named cells in every tracked Markdown file.
    everywhere = runme("list", "--json")
    if everywhere.returncode != 0:
        raise SystemExit(f"runme list at the root failed: {everywhere.stderr.strip()}")
    for entry in json.loads(everywhere.stdout):
        if not entry["file"].startswith("runbooks/"):
            errors.append(f"{entry['file']}: named cell {entry['name']} is runnable outside runbooks/")

    if errors:
        raise SystemExit("\n".join(errors))
    run_all = sum(1 for entry in entries if entry["run_all"])
    print(f"{version.stdout.strip()}: {len(cells)} cells dry-run in the repository root, {run_all} in Run All")


if __name__ == "__main__":
    main()
