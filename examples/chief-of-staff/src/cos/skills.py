"""Approved task Skills and their project-scope handoff (spec section 9.6).

Only the host reads source paths. The Jevscript policy sees IDs and descriptions;
an answer can never supply a source, destination, command, or download URL.
The catalog has no size cap: a dispatch judges only the shortlist that
``search`` returns (``skill_search``), so its cost follows the shortlist.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import tempfile
from pathlib import Path
from typing import Any

from . import skill_search
from .state.home import Home, read_json, write_json

ID = re.compile(r"[a-z0-9][a-z0-9_-]*\Z")
DIGEST = re.compile(r"[a-f0-9]{64}\Z")
MAX_DESCRIPTION = 1024
MAX_KEYWORDS = 32
MAX_BYTES = 65536
MAX_FILES = 128
MAX_TOTAL_BYTES = 8 * 1024 * 1024
_FROM_HOME = object()


class SkillError(ValueError):
    """An approved Skill or worker destination failed section 9.6 verification."""


class Skills:
    """Resolve approved files and reconcile a section 9.6 dispatch receipt."""

    def __init__(self, home: Home) -> None:
        self.home = home
        self._verified: tuple[str, list[dict[str, Any]]] | None = None

    @staticmethod
    def _frontmatter(text: str, skill_id: str) -> dict[str, str]:
        lines = text.splitlines()
        if not lines or lines[0] != "---":
            raise SkillError(f"skill {skill_id}: SKILL.md needs frontmatter")
        try:
            end = lines.index("---", 1)
        except ValueError as error:
            raise SkillError(f"skill {skill_id}: SKILL.md needs frontmatter") from error
        fields: dict[str, str] = {}
        index = 1
        while index < end:
            match = re.fullmatch(r"([a-z_]+):\s*(.*)", lines[index])
            if match is None:
                index += 1
                continue
            key, value = match.groups()
            if value in (">", ">-", ">+", "|", "|-", "|+"):
                body: list[str] = []
                index += 1
                while index < end and (not lines[index].strip() or lines[index][0].isspace()):
                    if lines[index].strip():
                        body.append(lines[index].strip())
                    index += 1
                value = " ".join(body)
            else:
                value = value.strip().strip("\"'")
                index += 1
            fields[key] = value
        if fields.get("name") != skill_id or not fields.get("description"):
            raise SkillError(f"skill {skill_id}: name or description frontmatter is missing or mismatched")
        return fields

    def _source(self, entry: dict[str, Any]) -> tuple[Path, dict[str, bytes]]:
        if not isinstance(entry.get("path"), str):
            raise SkillError(f"skill {entry['id']}: path must name a local SKILL.md")
        source = Path(entry["path"]).expanduser()
        if not source.is_absolute() or source.name != "SKILL.md" or source.is_symlink() or not source.is_file():
            raise SkillError(f"skill {entry['id']}: source must be an existing regular SKILL.md")
        try:
            if source.stat().st_size > MAX_BYTES:
                raise SkillError(f"skill {entry['id']}: SKILL.md exceeds {MAX_BYTES} bytes")
        except OSError as error:
            raise SkillError(f"skill {entry['id']}: cannot read source: {error}") from error
        manifest = entry.get("files", {"SKILL.md": entry["sha256"]})
        if not isinstance(manifest, dict) or not manifest or len(manifest) > MAX_FILES:
            raise SkillError(f"skill {entry['id']}: files must be a bounded digest manifest")
        if manifest.get("SKILL.md") != entry["sha256"]:
            raise SkillError(f"skill {entry['id']}: SKILL.md digest disagrees with files manifest")
        for relative, digest in manifest.items():
            path = Path(relative) if isinstance(relative, str) else Path("/")
            if path.is_absolute() or ".." in path.parts or not path.parts or not isinstance(digest, str) or not DIGEST.fullmatch(digest):
                raise SkillError(f"skill {entry['id']}: invalid pinned file {relative!r}")
        actual: dict[str, bytes] = {}
        try:
            for child in source.parent.rglob("*"):
                if child.is_symlink():
                    raise SkillError(f"skill {entry['id']}: source contains a symlink: {child}")
                if child.is_dir():
                    continue
                if not child.is_file():
                    raise SkillError(f"skill {entry['id']}: source contains an unsupported file: {child}")
                relative = child.relative_to(source.parent).as_posix()
                actual[relative] = child.read_bytes()
                if len(actual) > MAX_FILES or sum(len(content) for content in actual.values()) > MAX_TOTAL_BYTES:
                    raise SkillError(f"skill {entry['id']}: source tree exceeds its file or byte limit")
        except OSError as error:
            raise SkillError(f"skill {entry['id']}: cannot read source: {error}") from error
        if set(actual) != set(manifest):
            raise SkillError(f"skill {entry['id']}: source files differ from the pinned manifest")
        if any(hashlib.sha256(actual[relative]).hexdigest() != digest for relative, digest in manifest.items()):
            raise SkillError(f"skill {entry['id']}: pinned source digest changed")
        return source, actual

    def catalog(self, configured: Any = _FROM_HOME) -> list[dict[str, Any]]:
        """Validate the whole configured catalog and re-read every pinned byte."""
        if configured is _FROM_HOME:
            configured = self.home.config.get("skill_catalog", [])
        if not isinstance(configured, list):
            raise SkillError("skill_catalog must be a list")
        entries: list[dict[str, Any]] = []
        seen: set[str] = set()
        for raw in configured:
            if not isinstance(raw, dict):
                raise SkillError("each skill_catalog entry must be a record")
            entry = dict(raw)
            skill_id = entry.get("id")
            if not isinstance(skill_id, str) or not ID.fullmatch(skill_id) or skill_id in seen:
                raise SkillError(f"invalid or duplicate skill id: {skill_id!r}")
            if not isinstance(entry.get("sha256"), str) or not DIGEST.fullmatch(entry["sha256"]):
                raise SkillError(f"skill {skill_id}: sha256 must be a lowercase SHA-256 digest")
            deps = entry.get("dependencies", [])
            if not isinstance(deps, list) or any(not isinstance(dep, str) or not ID.fullmatch(dep) for dep in deps):
                raise SkillError(f"skill {skill_id}: invalid dependencies")
            keywords = entry.get("keywords", [])
            if not isinstance(keywords, list) or len(keywords) > MAX_KEYWORDS or any(not isinstance(word, str) or not 0 < len(word) <= skill_search.MAX_TERM_CHARS for word in keywords):
                raise SkillError(f"skill {skill_id}: keywords must be at most {MAX_KEYWORDS} texts of 1 to {skill_search.MAX_TERM_CHARS} characters")
            playbooks = entry.get("playbooks", [])
            if not isinstance(playbooks, list) or any(not isinstance(name, str) or not name for name in playbooks):
                raise SkillError(f"skill {skill_id}: playbooks must be a list of playbook names")
            if not isinstance(entry.get("pinned", False), bool):
                raise SkillError(f"skill {skill_id}: pinned must be true or false")
            source, files = self._source(entry)
            try:
                text = files["SKILL.md"].decode("utf-8")
            except UnicodeDecodeError as error:
                raise SkillError(f"skill {skill_id}: SKILL.md is not UTF-8") from error
            fields = self._frontmatter(text, skill_id)
            if len(fields["description"]) > MAX_DESCRIPTION:
                raise SkillError(f"skill {skill_id}: description exceeds {MAX_DESCRIPTION} characters")
            seen.add(skill_id)
            manifest = entry.get("files", {"SKILL.md": entry["sha256"]})
            tree_sha256 = hashlib.sha256(json.dumps(sorted(manifest.items()), separators=(",", ":")).encode()).hexdigest()
            entries.append({"id": skill_id, "path": str(source), "sha256": entry["sha256"], "files": manifest, "tree_sha256": tree_sha256, "description": fields["description"], "dependencies": deps,
                            "keywords": keywords, "playbooks": playbooks, "pinned": entry.get("pinned", False)})
        ids = {entry["id"] for entry in entries}
        if any(dep not in ids for entry in entries for dep in entry["dependencies"]):
            raise SkillError("skill_catalog has a dependency outside the approved catalog")
        self._closure([entry["id"] for entry in entries], entries)
        if self.home.config.get("skill_catalog", []) == configured:
            self._verified = (json.dumps(configured, sort_keys=True), entries)
        return entries

    def _current(self) -> list[dict[str, Any]]:
        """The catalog this session last verified in full, while the config is unchanged.

        A dispatch snapshot verifies every byte once; the verbs that follow it
        re-read only the entries they return or install.
        """
        key = json.dumps(self.home.config.get("skill_catalog", []), sort_keys=True)
        if self._verified is not None and self._verified[0] == key:
            return self._verified[1]
        return self.catalog()

    @staticmethod
    def _limits(policy: dict[str, Any]) -> tuple[int, int]:
        k, terms = policy.get("skill_shortlist"), policy.get("skill_terms")
        if not isinstance(k, int) or isinstance(k, bool) or k < 1:
            raise SkillError("policy.skill_shortlist must be a positive whole number")
        if not isinstance(terms, int) or isinstance(terms, bool) or not 1 <= terms <= 64:
            raise SkillError("policy.skill_terms must be a whole number from 1 to 64")
        return k, terms

    def _scope(self, item: dict[str, Any], k: int) -> tuple[list[dict[str, Any]], list[str], bool]:
        """The catalog, the ids that bypass the filter, and whether the filter drops anything.

        Pinned Skills and those of the playbook that routed the item are always
        judged. Once they fill ``k`` places, nothing is searched for.
        """
        catalog = self._current()
        playbook = item.get("playbook")
        always = [e["id"] for e in catalog if e["pinned"] or (playbook and playbook in e["playbooks"])]
        return catalog, always, len(catalog) > k and len(always) < k

    def warnings(self, policy: dict[str, Any]) -> list[str]:
        k, _ = self._limits(policy)
        pinned = [e["id"] for e in self._current() if e["pinned"]]
        if len(pinned) > k:
            return [f"{len(pinned)} pinned Skills exceed policy.skill_shortlist ({k}); every dispatch judges all of them"]
        return []

    def plan(self, item: dict[str, Any], policy: dict[str, Any], questions: int, scout_enabled: bool) -> dict[str, Any]:
        """Nominal model calls for one dispatch: writer, scout if the filter drops anything, Jev chunks.

        Static arithmetic under the selected profile's question cap; a retry or
        a longer answer can still cost more, so the program keeps its budgets.
        """
        k, _ = self._limits(policy)
        catalog, always, scout = self._scope(item, k)
        judged = len(always) if len(always) >= k else min(len(catalog), k)
        return {"always": always, "scout": scout,
                "calls": 1 + int(scout and scout_enabled) + -(-judged // questions),
                "always_calls": 1 + -(-len(always) // questions)}

    def search(self, item: dict[str, Any], scout_text: str, request: str, k: int, policy: dict[str, Any], *, no_model: bool = False, scout_failure: str | None = None) -> dict[str, Any]:
        """Rank the catalog for one item and return the shortlist Jev will judge.

        The scout's text is model-written, so it is parsed against a fixed
        schema; anything else falls back to the request's own words and says so.
        It narrows which Skills are judged and can never add an id.
        """
        limit, max_terms = self._limits(policy)
        if k != limit:
            raise SkillError(f"skill search asked for {k} Skills; policy.skill_shortlist is {limit}")
        catalog, always, needed = self._scope(item, limit)
        query, terms, kind, fallback = request, [], None, None
        if needed and scout_failure is not None:
            fallback = scout_failure
        elif needed and no_model:
            fallback = "no scout model for this harness"
        elif needed and not (scout_text or "").strip():
            fallback = "the scout wrote nothing"
        elif needed:
            try:
                parsed = skill_search.parse_scout(scout_text or "", max_terms)
            except ValueError as error:
                fallback = f"scout output rejected: {error}"
            else:
                query, terms, kind = f"{parsed.text()} {request}", list(parsed.terms), parsed.kind
        if needed:
            ids = skill_search.rank(catalog, always, query, terms, limit)
        else:  # the whole catalog fits, or the always-included Skills fill it
            ids = always if len(always) >= limit else always + [e["id"] for e in catalog if e["id"] not in always]
        by_id = {entry["id"]: entry for entry in catalog}
        for skill_id in ids:
            self._source(by_id[skill_id])
        return {
            "skills": [{"id": i, "description": by_id[i]["description"], "dependencies": by_id[i]["dependencies"]} for i in ids],
            "ids": ids,
            "always": always,
            "scanned": len(catalog),
            "kept": len(ids),
            "filtered_out": len(catalog) - len(ids),
            "scouted": needed and not no_model,
            "kind": kind,
            "terms": terms,
            "fallback": fallback is not None,
            "fallback_reason": fallback,
        }

    @staticmethod
    def _closure(ids: list[str], catalog: list[dict[str, Any]]) -> list[dict[str, Any]]:
        by_id = {entry["id"]: entry for entry in catalog}
        ordered: list[dict[str, Any]] = []
        visited: set[str] = set()
        visiting: set[str] = set()

        for skill_id in ids:
            if not isinstance(skill_id, str):
                raise SkillError("selected skill ids must be strings")
            stack = [(skill_id, False)]
            while stack:
                current, complete = stack.pop()
                if complete:
                    visiting.remove(current)
                    visited.add(current)
                    ordered.append(by_id[current])
                    continue
                if current not in by_id:
                    raise SkillError(f"unknown selected skill id: {current!r}")
                if current in visiting:
                    raise SkillError(f"skill_catalog dependency cycle at {current}")
                if current in visited:
                    continue
                visiting.add(current)
                stack.append((current, True))
                stack.extend((dep, False) for dep in reversed(by_id[current]["dependencies"]))
        return ordered

    @staticmethod
    def _directory(path: Path) -> None:
        if path.is_symlink() or (path.exists() and not path.is_dir()):
            raise SkillError(f"skill destination collision: {path}")

    @staticmethod
    def _tracked(root: Path) -> set[str]:
        result = subprocess.run(["git", "-C", str(root), "ls-files", "-z"], capture_output=True, check=True)
        return {path.decode("utf-8", "surrogateescape") for path in result.stdout.split(b"\0") if path}

    @staticmethod
    def _atomic_bytes(path: Path, content: bytes, mode: int) -> None:
        fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
        try:
            with os.fdopen(fd, "wb") as handle:
                handle.write(content)
            os.chmod(temporary, mode)
            os.replace(temporary, path)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)

    def ensure(self, task_id: str, worktree: str, harness: str, ids: list[str], status: str, uncertain: list[str]) -> dict[str, Any]:
        """Claim owned paths before writing; replay a prior claim on retry (§9.6).

        The first durable selection wins if Jev answers differently after a
        crash. This keeps a restarted dispatch from stranding a live worker.
        """
        if not isinstance(ids, list) or not isinstance(uncertain, list):
            raise SkillError("selected and uncertain skill ids must be lists")
        if status not in ("none", "selected", "uncertain"):
            raise SkillError(f"invalid Skill selection status: {status!r}")
        root = Path(worktree)
        if not root.is_absolute() or root.is_symlink() or not root.is_dir():
            raise SkillError("worker worktree is missing or not a directory")
        receipt_path = self.home.task_dir(task_id) / "skill-selection.json"
        previous = read_json(receipt_path, None)
        if previous is not None:
            if previous.get("worktree") != str(root) or previous.get("harness") != harness:
                raise SkillError(f"skill selection target changed after an earlier ensure for {task_id}")
            ids = [entry["id"] for entry in previous["selected"]]
            status = previous["selection_status"]
            uncertain = previous["uncertain"]
        catalog = self._current()
        selected = self._closure(ids, catalog)
        by_id = {entry["id"]: entry for entry in catalog}
        if any(skill_id not in by_id for skill_id in uncertain):
            raise SkillError("unknown uncertain skill id")
        if harness not in ("codex", "claude", "claude-code") and selected:
            raise SkillError(f"no project Skill location is approved for harness {harness!r}")
        key_input = [task_id, str(root), harness, "project", status, uncertain, [[e["id"], e["tree_sha256"]] for e in selected]]
        key = hashlib.sha256(json.dumps(key_input, separators=(",", ":")).encode()).hexdigest()
        if previous is not None and previous["key"] != key:
            raise SkillError(f"pinned Skill selection changed for {task_id}")
        result: dict[str, Any] = {
            "key": key,
            "worktree": str(root),
            "harness": harness,
            "selection_status": status,
            "uncertain": uncertain,
            "uncertain_skills": [{"id": skill_id, "description": by_id[skill_id]["description"]} for skill_id in uncertain],
            "status": "pending",
            "selected": [],
            "owned": list(previous.get("owned", [])) if previous else [],
        }
        if not selected:
            result["status"] = "ensured"
            write_json(receipt_path, result)
            return result

        canonical = root / ".agents" / "skills"
        claude = root / ".claude" / "skills"
        for parent in (root / ".agents", canonical):
            self._directory(parent)
        shared_claude = False
        if harness in ("claude", "claude-code"):
            self._directory(root / ".claude")
            if claude.is_symlink():
                if claude.resolve() != canonical.resolve():
                    raise SkillError(f"skill destination collision: {claude}")
                shared_claude = True
            else:
                self._directory(claude)

        planned: list[tuple[dict[str, Any], dict[str, bytes], dict[str, int], Path, Path | None]] = []
        tracked = self._tracked(root)
        claimed = {owned["path"] for owned in result["owned"]}
        for entry in selected:
            source, files = self._source(entry)
            modes = {relative: (source.parent / relative).stat().st_mode & 0o777 for relative in files}
            target_dir = canonical / entry["id"]
            self._directory(target_dir)
            for child in target_dir.rglob("*") if target_dir.exists() else []:
                relative = child.relative_to(target_dir).as_posix()
                if child.is_symlink() or (child.is_file() and relative not in files):
                    raise SkillError(f"skill destination collision: {child}")
            for relative, content in files.items():
                target = target_dir / relative
                for parent in target.parents:
                    if parent == target_dir:
                        break
                    self._directory(parent)
                if target.is_symlink() or (target.exists() and (not target.is_file() or target.read_bytes() != content)):
                    raise SkillError(f"skill destination collision: {target}")
                if target.exists() and str(target.relative_to(root)) not in claimed | tracked:
                    raise SkillError(f"skill destination collision: {target}")
            link = claude / entry["id"] if harness in ("claude", "claude-code") else None
            if link is not None and not shared_claude and (link.exists() or link.is_symlink()):
                if not link.is_symlink() or link.resolve() != target_dir.resolve():
                    raise SkillError(f"skill destination collision: {link}")
                if str(link.relative_to(root)) not in claimed | tracked:
                    raise SkillError(f"skill destination collision: {link}")
            planned.append((entry, files, modes, target_dir, link))
            result["selected"].append({"id": entry["id"], "path": str((link or target_dir) / "SKILL.md"), "sha256": entry["sha256"], "tree_sha256": entry["tree_sha256"], "files": entry["files"], "description": entry["description"]})
            if previous is None:
                for relative in files:
                    target = target_dir / relative
                    if not target.exists():
                        result["owned"].append({"path": str(target.relative_to(root)), "kind": "file", "sha256": entry["files"][relative]})
            if previous is None and link is not None and not shared_claude and not link.is_symlink():
                result["owned"].append({"path": str(link.relative_to(root)), "kind": "link", "target": str(target_dir.relative_to(root))})
        if previous is not None:
            self._verify_owned(root, result, allow_missing=True)
        write_json(receipt_path, result)
        for _, files, modes, target_dir, link in planned:
            for relative, content in files.items():
                target = target_dir / relative
                if not target.exists():
                    target.parent.mkdir(parents=True, exist_ok=True)
                    self._atomic_bytes(target, content, modes[relative])
            if link is not None and not shared_claude and not link.is_symlink():
                link.parent.mkdir(parents=True, exist_ok=True)
                os.symlink(os.path.relpath(target_dir, link.parent), link)
        result["status"] = "ensured"
        write_json(receipt_path, result)
        return result

    @staticmethod
    def _verify_owned(root: Path, receipt: dict[str, Any], *, allow_missing: bool = False) -> list[str]:
        paths: list[str] = []
        for entry in receipt.get("selected", []):
            skill_id = entry["id"]
            for directory in (root / ".agents", root / ".agents" / "skills", root / ".agents" / "skills" / skill_id):
                Skills._directory(directory)
            if Path(entry["path"]).is_relative_to(root / ".claude" / "skills"):
                Skills._directory(root / ".claude")
                shared = root / ".claude" / "skills"
                if shared.is_symlink() and shared.resolve() != (root / ".agents" / "skills").resolve():
                    raise SkillError(f"installed Skill link changed: {shared}")
                elif not shared.is_symlink():
                    Skills._directory(shared)
        for owned in receipt.get("owned", []):
            relative = owned["path"]
            if not isinstance(relative, str) or Path(relative).is_absolute() or ".." in Path(relative).parts:
                raise SkillError("invalid owned Skill path in receipt")
            path = root / relative
            for parent in path.parents:
                if parent == root:
                    break
                Skills._directory(parent)
            if allow_missing and not (path.exists() or path.is_symlink()):
                paths.append(relative)
                continue
            if owned["kind"] == "file":
                if path.is_symlink() or not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != owned["sha256"]:
                    raise SkillError(f"installed Skill changed: {path}")
            elif owned["kind"] == "link":
                expected = root / owned["target"]
                if not path.is_symlink() or path.resolve() != expected.resolve():
                    raise SkillError(f"installed Skill link changed: {path}")
            else:
                raise SkillError("invalid owned Skill type in receipt")
            paths.append(relative)
        for entry in receipt.get("selected", []):
            for relative, digest in entry.get("files", {"SKILL.md": entry["sha256"]}).items():
                path = root / ".agents" / "skills" / entry["id"] / relative
                for parent in path.parents:
                    if parent == root:
                        break
                    Skills._directory(parent)
                if allow_missing and not path.exists():
                    continue
                if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != digest:
                    raise SkillError(f"installed Skill changed: {path}")
        return paths

    def owned_paths(self, task_id: str, worktree: str) -> list[str]:
        """Only exact receipt-owned, unchanged paths can be ignored by Git checks."""
        receipt = read_json(self.home.task_dir(task_id) / "skill-selection.json", None)
        if receipt is None:
            return []
        if receipt.get("worktree") != worktree or receipt.get("status") != "ensured":
            raise SkillError(f"Skill receipt for {task_id} is incomplete or points elsewhere")
        root = Path(worktree)
        if not root.is_dir():
            return []
        paths = self._verify_owned(root, receipt, allow_missing=True)
        if paths:
            tracked = subprocess.run(["git", "-C", worktree, "ls-files", "--", *paths], capture_output=True, text=True, check=True).stdout.strip()
            if tracked:
                raise SkillError(f"worker commit contains host Skill files: {tracked}")
        return paths

    def head_owned_paths(self, task_id: str, worktree: str, head_oid: str | None) -> list[str]:
        """Read receipt-owned paths from the actual forge head tree (§9.6)."""
        receipt = read_json(self.home.task_dir(task_id) / "skill-selection.json", None)
        if receipt is None:
            return []
        if receipt.get("worktree") != worktree or receipt.get("status") != "ensured":
            raise SkillError(f"Skill receipt for {task_id} is incomplete or points elsewhere")
        root = Path(worktree)
        if not root.is_dir():
            raise SkillError("the isolated copy is gone; cannot verify pull request head")
        paths = self._verify_owned(root, receipt, allow_missing=True)
        if not paths:
            return []
        if not isinstance(head_oid, str) or not re.fullmatch(r"[a-f0-9]{40}|[a-f0-9]{64}", head_oid):
            raise SkillError("pull request headRefOid is missing or invalid")
        exists = subprocess.run(["git", "-C", worktree, "cat-file", "-e", f"{head_oid}^{{commit}}"], capture_output=True, check=False)
        if exists.returncode != 0:
            raise SkillError("the pull request head is not in the isolated copy; fetch or push the branch")
        result = subprocess.run(["git", "-C", worktree, "ls-tree", "-r", "-z", "--name-only", head_oid, "--", *paths], capture_output=True, check=False)
        if result.returncode != 0:
            raise SkillError("cannot inspect the pull request head tree")
        return [name.decode("utf-8", "surrogateescape") for name in result.stdout.split(b"\0") if name]

    def remove_owned(self, task_id: str, worktree: str, *, keep_tracked: bool = False, allow_pending: bool = False) -> list[str]:
        """Remove only verified host files after landing, before worktree cleanup."""
        if keep_tracked or allow_pending:
            receipt = read_json(self.home.task_dir(task_id) / "skill-selection.json", None)
            if receipt is None:
                return []
            allowed = ("pending", "ensured") if allow_pending else ("ensured",)
            if receipt.get("worktree") != worktree or receipt.get("status") not in allowed:
                raise SkillError(f"Skill receipt for {task_id} is incomplete or points elsewhere")
            paths = self._verify_owned(Path(worktree), receipt, allow_missing=True)
        else:
            paths = self.owned_paths(task_id, worktree)
        root = Path(worktree)
        tracked = self._tracked(root) & set(paths)
        if tracked and not keep_tracked:
            raise SkillError(f"worker commit contains host Skill files: {', '.join(sorted(tracked))}")
        for relative in sorted(paths, key=lambda path: len(Path(path).parts), reverse=True):
            if relative in tracked:
                continue
            path = root / relative
            if not (path.exists() or path.is_symlink()):
                continue
            path.unlink()
            parent = path.parent
            while parent != root:
                try:
                    parent.rmdir()
                except OSError:
                    break
                parent = parent.parent
        return sorted(tracked)
