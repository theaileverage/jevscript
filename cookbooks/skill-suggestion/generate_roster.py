#!/usr/bin/env python3
"""Rebuild the cookbook's explicitly licensed Hermes subset from its pinned source."""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import tarfile
import tempfile
import urllib.request
from pathlib import Path

try:
    import yaml
except ImportError as exc:  # pragma: no cover - provenance utility only
    raise SystemExit("generate_roster.py requires PyYAML") from exc


COMMIT = "e444d165807f489b5c1ab8e4a612c8d09c2e67a2"
ARCHIVE_URL = f"https://codeload.github.com/NousResearch/hermes-agent/tar.gz/{COMMIT}"
ALLOWED_LICENSES = frozenset({"MIT", "Apache-2.0"})
EXPECTED_COUNT = 157
EXPECTED_CATEGORIES = 29
EXPECTED_PROMPT_CHARS = 14_168

IDENTITY = (
    "You are Hermes, a capable AI assistant with access to tools and a library "
    "of skills. You help the user with coding, research, and everyday tasks.\n\n"
)
PREAMBLE = (
    "## Skills (mandatory)\n"
    "Before replying, scan the skills below. If a skill matches or is even partially relevant "
    "to your task, you MUST load it with skill_view(name) and follow its instructions. "
    "Err on the side of loading — it is always better to have context you don't need "
    "than to miss critical steps, pitfalls, or established workflows. "
    "Skills contain specialized knowledge — API endpoints, tool-specific commands, "
    "and proven workflows that outperform general-purpose approaches. Load the skill "
    "even if you think you could handle the task with basic tools like web_search or terminal. "
    "Skills also encode the user's preferred approach, conventions, and quality standards "
    "for tasks like code review, planning, and testing — load them even for tasks you "
    "already know how to do, because the skill defines how it should be done here.\n"
    "Whenever the user asks you to configure, set up, install, enable, disable, modify, "
    "or troubleshoot Hermes Agent itself — its CLI, config, models, providers, tools, "
    "skills, voice, gateway, plugins, or any feature — load the `hermes-agent` skill "
    "first. It has the actual commands (e.g. `hermes config set …`, `hermes tools`, "
    "`hermes setup`) so you don't have to guess or invent workarounds.\n"
    "If a skill has issues, fix it with skill_manage(action='patch').\n"
    "After difficult/iterative tasks, offer to save as a skill. "
    "If a skill you loaded was missing steps, had wrong commands, or needed "
    "pitfalls you discovered, update it before finishing.\n\n"
)
FOOTER = "\n\nOnly proceed without loading a skill if genuinely none are relevant to the task."


def _category(path: Path, root: Path) -> str:
    relative = path.relative_to(root)
    if relative.parts[0] in {"skills", "optional-skills"}:
        inner = relative.parts[1:]
        return "/".join(inner[:-2]) if len(inner) > 2 else inner[0]
    if relative.parts[0] == "plugins":
        return relative.parts[1]
    raise ValueError(f"unexpected skill path: {relative}")


def build(source_root: Path) -> tuple[list[dict[str, str]], dict[str, str]]:
    roster: list[dict[str, str]] = []
    licenses: dict[str, str] = {}
    for path in sorted(source_root.rglob("SKILL.md")):
        text = path.read_text(encoding="utf-8")
        if not text.startswith("---\n") or "\n---\n" not in text[4:]:
            raise ValueError(f"missing YAML frontmatter: {path}")
        frontmatter_text, body = text[4:].split("\n---\n", 1)
        frontmatter = yaml.safe_load(frontmatter_text) or {}
        license_name = str(frontmatter.get("license") or "not declared").strip()
        if license_name not in ALLOWED_LICENSES:
            continue
        name = str(frontmatter.get("name") or path.parent.name)
        description_full = str(frontmatter.get("description") or "").strip().strip("'\"")
        description = (
            description_full
            if len(description_full) <= 60
            else description_full[:57] + "..."
        )
        roster.append(
            {
                "name": name,
                "category": _category(path, source_root),
                "description": description,
                "description_full": description_full,
                "body": body.strip()[:1600],
            }
        )
        licenses[name] = license_name
    return roster, licenses


def render_prompt(roster: list[dict[str, str]]) -> str:
    categories: dict[str, list[dict[str, str]]] = {}
    for skill in roster:
        categories.setdefault(skill["category"], []).append(skill)
    lines: list[str] = []
    for category in sorted(categories):
        lines.append(f"  {category}:")
        for skill in sorted(categories[category], key=lambda item: item["name"]):
            lines.append(f"    - {skill['name']}: {skill['description']}")
    return (
        IDENTITY
        + PREAMBLE
        + "<available_skills>\n"
        + "\n".join(lines)
        + "\n</available_skills>"
        + FOOTER
    )


def verify(roster: list[dict[str, str]], licenses: dict[str, str]) -> None:
    names = [skill["name"] for skill in roster]
    if len(names) != len(set(names)) or set(names) != set(licenses):
        raise ValueError("roster and license declarations must have the same unique names")
    if any(license_name not in ALLOWED_LICENSES for license_name in licenses.values()):
        raise ValueError("roster contains a skill without a permitted license declaration")
    categories = {skill["category"] for skill in roster}
    widths = [len(skill["description"]) for skill in roster]
    actual = (len(roster), len(categories), len(render_prompt(roster)), round(sum(widths) / len(widths)), max(widths))
    expected = (EXPECTED_COUNT, EXPECTED_CATEGORIES, EXPECTED_PROMPT_CHARS, 54, 60)
    if actual != expected:
        raise ValueError(f"roster drift: expected {expected}, got {actual}")


def write_json(path: Path, value: object) -> str:
    payload = (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(payload)
    return hashlib.sha256(payload).hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-dir", type=Path)
    parser.add_argument("--output", type=Path, default=Path("data/hermes_roster.json"))
    parser.add_argument("--licenses", type=Path, default=Path("data/hermes_roster.licenses.json"))
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="hermes-roster-") as temp:
        temp_path = Path(temp)
        if args.source_dir:
            source_root = args.source_dir.resolve()
        else:
            archive = temp_path / "hermes.tar.gz"
            with urllib.request.urlopen(ARCHIVE_URL, timeout=120) as response, archive.open("wb") as output:
                shutil.copyfileobj(response, output)
            with tarfile.open(archive, "r:gz") as bundle:
                bundle.extractall(temp_path, filter="data")
            source_root = next(path for path in temp_path.iterdir() if path.is_dir())
        roster, licenses = build(source_root)
        verify(roster, licenses)
        roster_sha = write_json(args.output, roster)
        license_sha = write_json(args.licenses, licenses)
    print(f"roster_sha256={roster_sha}")
    print(f"licenses_sha256={license_sha}")


if __name__ == "__main__":
    main()
