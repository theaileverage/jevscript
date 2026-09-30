"""State and persistence: one CoS home on disk.

A home keeps ``data/`` for durable records the person cares about (projects,
backlog, memory, ledger, playbooks), while ``state/`` holds
runtime records (workers, status logs, inboxes, decisions, recordings), and
``config.json`` holds operating choices. Every write is atomic (write a
sibling, then rename), so a crash never leaves a half-written record, and the
next session reconciles from disk rather than from anything held in memory.
"""

from __future__ import annotations

import contextlib
import copy
import json
import os
import secrets
import tempfile
import time
from pathlib import Path
from typing import Any, Iterator

NATIVE_SCOUT_MODELS = {"claude": "claude-haiku-4-5-20251001", "claude-code": "claude-haiku-4-5-20251001", "codex": "gpt-6-luna"}

#: Defaults for ``config.json``.
DEFAULT_CONFIG: dict[str, Any] = {
    # How the CoS names itself, and how it names the person it works for, on
    # every surface a person or a worker reads.
    "identity": {"name": "Chief of Staff", "principal": "the principal"},
    "backend": "auto",
    "jev": {
        "mode": "live",
        "model": "jev-latest",
        "keychain_service": "typesafe-api-key",
        "keychain_account": None,
    },
    "writer": "auto",
    "skill_catalog": [],
    # The cheaper model that writes Skill search terms follows the dispatch
    # harness; a harness missing here searches with the request's own words.
    "scout": {
        "models": dict(NATIVE_SCOUT_MODELS),
    },
    "poll_seconds": 30,
    "max_wait_seconds": 120,
    "heartbeat_minutes": 30,
    "learn_every": 5,
    "forge_merge_flags": ["--squash"],
    "policy": {
        "capacity": 1000,
        "min_kind_confidence": 0.35,
        "min_route_confidence": 0.45,
        "max_ambiguity": 0.8,
        "urgent": 0.8,
        "risk_hold": 0.7,
        "playbook_confidence": 0.6,
        "playbook_routine": 0.5,
        "thread_confidence": 0.65,
        "stall_minutes": 20,
        "pause_minutes": 240,
        "looping": 0.75,
        "prompt": 0.8,
        "needs_owner": 0.5,
        "max_nudges": 2,
        "max_relaunches": 2,
        "rering_minutes": 10,
        "min_category_confidence": 0.5,
        "min_repeats": 3,
        "min_success": 0.66,
        "automatable": 0.7,
        "repeated_strong": 0.85,
        "review_confidence": 0.6,
        "skill_fit": 0.7,
        "skill_uncertain": 0.35,
        "skill_shortlist": 32,
        "skill_terms": 24,
        "min_evidence": 2,
        "min_recall": 1.0,
    },
}


def now() -> float:
    """Wall-clock seconds. One seam so tests can move time."""
    return time.time()


def request_id() -> str:
    """A request id that sorts by arrival and never collides."""
    return f"r{int(now() * 1000)}-{secrets.token_hex(2)}"


def iso(ts: float | None = None) -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(now() if ts is None else ts))


def atomic_write(path: Path, text: str, mode: int | None = None) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(text)
        if mode is not None:
            os.chmod(tmp, mode)
        os.replace(tmp, path)
    except BaseException:
        with contextlib.suppress(FileNotFoundError):
            os.unlink(tmp)
        raise


def read_json(path: Path, default: Any) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return copy.deepcopy(default)


def write_json(path: Path, value: Any) -> None:
    atomic_write(path, json.dumps(value, indent=2, sort_keys=True) + "\n")


def append_jsonl(path: Path, record: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(record, sort_keys=True) + "\n")


def read_jsonl(path: Path) -> list[Any]:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except FileNotFoundError:
        return []
    return [json.loads(line) for line in lines if line.strip()]


def _merge(base: dict[str, Any], over: dict[str, Any]) -> dict[str, Any]:
    out = copy.deepcopy(base)
    for key, value in over.items():
        if isinstance(value, dict) and isinstance(out.get(key), dict):
            out[key] = _merge(out[key], value)
        else:
            out[key] = value
    return out


def validated_identity(configured: Any) -> dict[str, str]:
    if not isinstance(configured, dict):
        raise ValueError("identity must be an object")
    values = {**DEFAULT_CONFIG["identity"], **configured}
    names = {}
    for field in ("name", "principal"):
        value = values[field]
        if not isinstance(value, str) or not value.strip():
            raise ValueError(f"identity.{field} must be nonempty text")
        names[field] = value.strip()
    return names


def native_scout_models(configured: Any) -> dict[str, str]:
    if not isinstance(configured, dict):
        raise ValueError("scout.models must be a map of native model IDs or null")
    enabled = {}
    for harness, model in configured.items():
        if harness not in NATIVE_SCOUT_MODELS:
            raise ValueError(f"scout.models has no native binding for {harness!r}")
        if model is not None and model != NATIVE_SCOUT_MODELS[harness]:
            raise ValueError(f"scout.models.{harness} must be {NATIVE_SCOUT_MODELS[harness]!r} or null")
        if model is not None:
            enabled[harness] = model
    return enabled


def _validate_scout_config(config: dict[str, Any]) -> None:
    scout = config.get("scout")
    native_scout_models(scout.get("models") if isinstance(scout, dict) else None)
    if "timeout" in scout:
        raise ValueError("scout.timeout is unsupported")
    adapters = config.get("adapters", {})
    if isinstance(adapters, dict) and adapters.get("scout") is not None:
        raise ValueError("adapters.scout is unsupported; native scout models are fixed")


class LockHeld(RuntimeError):
    """Another live session owns this home."""


class Home:
    """Paths and records of one home. Cheap to construct; reads lazily."""

    def __init__(self, root: str | Path) -> None:
        self.root = Path(root).expanduser().resolve()
        self.data = self.root / "data"
        self.state = self.root / "state"

    # -- layout -------------------------------------------------------------

    def init(self) -> "Home":
        for directory in (
            self.data,
            self.data / "playbooks",
            self.state / "workers",
            self.state / "requests",
            self.state / "recordings",
            self.state / "mates",
        ):
            directory.mkdir(parents=True, exist_ok=True)
        if not self.config_path.exists():
            write_json(self.config_path, {})
        validated_identity(read_json(self.config_path, {}).get("identity", {}))
        self.migrate()
        return self

    # -- records renamed by the S2 vocabulary --------------------------------

    @property
    def preferences_path(self) -> Path:
        return self.data / "principal.md"

    @property
    def legacy_preferences_path(self) -> Path:
        return self.data / "captain.md"

    def migrate(self) -> None:
        """Move records an older home wrote under their old names.

        Idempotent, and never overwrites: where the old and the new name both
        exist, both are kept exactly as they are; ``unresolved`` reports pairs
        that differ. The adapter key is decided from the raw file before
        defaults merge in, so no default can mask an existing ``adapters.crew``.
        """
        legacy, current = self.legacy_preferences_path, self.preferences_path
        if legacy.exists() and not current.exists():
            with contextlib.suppress(FileExistsError):
                os.link(legacy, current)  # fails rather than replace a principal.md that appeared meanwhile
                legacy.unlink()
        raw = read_json(self.config_path, {})
        changed = False
        adapters = raw.get("adapters")
        if isinstance(adapters, dict) and "crew" in adapters and "staff" not in adapters:
            adapters["staff"] = adapters.pop("crew")
            changed = True
        child_identity = raw.get("identity", {})
        if isinstance(child_identity, dict):
            missing = [field for field in ("name", "principal") if field not in child_identity]
            if missing:
                parent = read_json(self.data / "parent.json", None)
                if isinstance(parent, dict) and "home" in parent:
                    parent_raw = read_json(Home(parent["home"]).config_path, {})
                    parent_identity = validated_identity(parent_raw.get("identity", {}))
                    raw.setdefault("identity", child_identity)
                    for field in missing:
                        child_identity[field] = parent_identity[field]
                    changed = True
        if changed:
            write_json(self.config_path, raw)

    def unresolved(self) -> list[str]:
        """Old and new records that disagree; only the principal can merge them."""
        notes = []
        legacy, current = self.legacy_preferences_path, self.preferences_path
        if legacy.exists() and current.exists() and legacy.read_bytes() != current.read_bytes():
            notes.append(
                f"{legacy} and {current} both hold standing preferences and differ. Neither was changed; "
                f"workers are briefed with both until you merge {legacy.name} into {current.name} by hand and delete {legacy.name}."
            )
        adapters = read_json(self.config_path, {}).get("adapters")
        if isinstance(adapters, dict) and "crew" in adapters and "staff" in adapters and adapters["crew"] != adapters["staff"]:
            notes.append(f"{self.config_path} sets both adapters.crew and adapters.staff. adapters.staff is used; remove adapters.crew by hand.")
        return notes

    @property
    def config_path(self) -> Path:
        return self.root / "config.json"

    @property
    def config(self) -> dict[str, Any]:
        config = _merge(DEFAULT_CONFIG, read_json(self.config_path, {}))
        config["identity"] = validated_identity(config["identity"])
        _validate_scout_config(config)
        return config

    @property
    def identity(self) -> dict[str, str]:
        """The configured names every surface uses: ``name`` for the CoS and
        ``principal`` for the person it works for."""
        return self.config["identity"]

    def set_config(self, dotted: str, value: Any) -> list[str]:
        if dotted == "adapters.crew" or dotted.startswith("adapters.crew."):
            dotted = "adapters.staff" + dotted[len("adapters.crew") :]  # the old key keeps working
        raw = read_json(self.config_path, {})
        cursor = raw
        parts = dotted.split(".")
        for part in parts[:-1]:
            cursor = cursor.setdefault(part, {})
        cursor[parts[-1]] = value
        candidate = _merge(DEFAULT_CONFIG, raw)
        candidate["identity"] = validated_identity(candidate["identity"])
        _validate_scout_config(candidate)
        if dotted == "identity" or dotted in {"identity.name", "identity.principal"}:
            for field in ("name", "principal"):
                if field in raw["identity"]:
                    raw["identity"][field] = candidate["identity"][field]
        warnings: list[str] = []
        if dotted in {"skill_catalog", "policy", "policy.skill_shortlist", "policy.skill_terms"}:
            from ..skills import Skills

            skills = Skills(self)
            catalog = skills.catalog(configured=candidate["skill_catalog"])
            warnings = skills.warnings(candidate["policy"], catalog)
        write_json(self.config_path, raw)
        return warnings

    def task_dir(self, task_id: str) -> Path:
        return self.data / task_id

    def status_path(self, task_id: str) -> Path:
        return self.state / f"{task_id}.status"

    def inbox_dir(self, task_id: str) -> Path:
        return self.state / f"{task_id}.inbox"

    def worker_path(self, task_id: str) -> Path:
        return self.state / "workers" / f"{task_id}.json"

    def worktrees(self) -> Path:
        return self.root / "worktrees"

    # -- away and quiet modes ------------------------------------------------

    @property
    def mode(self) -> str:
        record = read_json(self.state / "mode.json", {})
        return record.get("mode", "normal")

    def set_mode(self, mode: str, note: str = "") -> None:
        if mode not in {"normal", "away", "quiet"}:
            raise ValueError(f"unknown mode `{mode}`")
        write_json(self.state / "mode.json", {"mode": mode, "since": iso(), "note": note})

    # -- the session lock ----------------------------------------------------

    @contextlib.contextmanager
    def lock(self) -> Iterator[None]:
        """One live session per home. A lock whose process is gone is stale
        and is taken over."""
        path = self.state / "session.lock"
        path.parent.mkdir(parents=True, exist_ok=True)
        while True:
            try:
                fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
            except FileExistsError:
                try:
                    pid = int(path.read_text().split()[0])
                except (ValueError, IndexError, FileNotFoundError):
                    pid = -1
                if pid > 0 and _alive(pid) and pid != os.getpid():
                    raise LockHeld(f"another session (pid {pid}) holds {path}") from None
                with contextlib.suppress(FileNotFoundError):
                    path.unlink()
                continue
            with os.fdopen(fd, "w") as handle:
                handle.write(f"{os.getpid()} {iso()}\n")
            break
        try:
            yield
        finally:
            with contextlib.suppress(FileNotFoundError):
                path.unlink()


def _alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True
