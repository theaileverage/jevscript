"""Identity and vocabulary (S2) through the real CoS CLI over fake-backed homes.

Each test names the rule it checks: an old home migrates once and never loses
user content, configured names reach every boundary a person or a worker reads
(help, notifications, briefs, session instructions), and the old verbs and
config key keep working.
"""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any

from conftest import BASE_RULES, SRC, run_until

from cos.state.home import read_json


def cos(home: Path, jevscript_bin: str, *args: str, path: Path | None = None) -> subprocess.CompletedProcess[str]:
    env = {**os.environ, "JEVSCRIPT_BIN": jevscript_bin, "COS_JEV": "fake", "PYTHONPATH": str(SRC)}
    if path is not None:
        env["PATH"] = f"{path}{os.pathsep}{env['PATH']}"
    result = subprocess.run([sys.executable, "-m", "cos", "--home", str(home), *args], env=env, capture_output=True, text=True, check=False)
    assert result.returncode == 0, result.stderr
    return result


def fake_bin(directory: Path, name: str) -> Path:
    """An executable on PATH that records its argv, one JSON line per call."""
    directory.mkdir(exist_ok=True)
    log = directory / f"{name}.jsonl"
    script = directory / name
    script.write_text(f"#!{sys.executable}\nimport json, sys\nwith open({str(log)!r}, 'a') as f:\n    f.write(json.dumps(sys.argv[1:]) + '\\n')\n")
    script.chmod(0o755)
    return log


def fingerprint(path: Path) -> tuple[str, int]:
    return hashlib.sha256(path.read_bytes()).hexdigest(), path.stat().st_mtime_ns


def cli_rules(host, tmp_path: Path, rules: list[dict[str, Any]]) -> None:
    """The CLI's own fake Jev answers with the same rules as the host's."""
    path = tmp_path / "rules.json"
    path.write_text(json.dumps([*rules, *BASE_RULES]))
    host.home.set_config("jev.fake_rules", str(path))


def make_legacy(home: Path) -> None:
    """Rewrite a current home into the layout an older CoS wrote."""
    config = read_json(home / "config.json", {})
    config["adapters"]["crew"] = config["adapters"].pop("staff")
    (home / "config.json").write_text(json.dumps(config, indent=2))
    (home / "data" / "principal.md").unlink(missing_ok=True)
    (home / "data" / "captain.md").write_text("# Preferences\n\n- 2026-01-02: Prefer small commits\n")


def test_an_old_home_migrates_once_keeps_its_crew_adapter_and_replays(make_host, jevscript_bin: str) -> None:
    host = make_host()
    host.submit("Update the guide")
    run_until(host, lambda: bool(host.workers.all()))
    home = host.home.root
    before_migration = sorted((home / "state" / "recordings").glob("*.jsonl"))
    assert before_migration
    make_legacy(home)
    old_preferences = (home / "data" / "captain.md").read_bytes()
    old_adapter = read_json(home / "config.json", {})["adapters"]["crew"]

    cos(home, jevscript_bin, "status")
    assert (home / "data" / "principal.md").read_bytes() == old_preferences
    assert not (home / "data" / "captain.md").exists()
    adapters = read_json(home / "config.json", {})["adapters"]
    assert adapters["staff"] == old_adapter and "crew" not in adapters, "the existing crew adapter is kept, not masked by the default"

    migrated = {name: fingerprint(home / name) for name in ("config.json", "data/principal.md")}
    cos(home, jevscript_bin, "tick")
    first = cos(home, jevscript_bin, "status")
    briefing = cos(home, jevscript_bin, "briefing")
    assert {name: fingerprint(home / name) for name in migrated} == migrated, "a second migration changes nothing"
    assert not (home / "data" / "captain.md").exists()
    assert "migration" not in first.stderr + briefing.stderr
    assert briefing.stdout.splitlines()[0] == f"Briefing: {first.stdout.strip()}"

    recordings = sorted((home / "state" / "recordings").glob("*.jsonl"))
    assert set(before_migration) < set(recordings)
    for path in recordings:
        replay = subprocess.run([jevscript_bin, "replay", str(path)], capture_output=True, text=True, check=False)
        assert replay.returncode == 0, f"{path.name}: {replay.stderr}"


def test_differing_preference_files_are_both_kept_reported_and_briefed(make_host, jevscript_bin: str, tmp_path: Path) -> None:
    host = make_host()
    cli_rules(host, tmp_path, [])
    home = host.home.root
    principal, captain = home / "data" / "principal.md", home / "data" / "captain.md"
    principal.write_text("# Preferences\n\n- 2026-09-01: Prefer small commits\n")
    captain.write_text("# Preferences\n\n- 2025-03-04: Use British spelling\n")
    kept = {path: path.read_bytes() for path in (principal, captain)}

    runs = [cos(home, jevscript_bin, "say", "Update the guide", "--project", "proj"), cos(home, jevscript_bin, "tick")]
    for run in runs:
        assert run.stderr.count("unresolved migration") == 1
        assert str(principal) in run.stderr and str(captain) in run.stderr
    assert {path: path.read_bytes() for path in kept} == kept, "neither file is overwritten, merged or deleted"
    doctor = cos(home, jevscript_bin, "doctor").stdout
    assert f"migration: UNRESOLVED - {captain} and {principal}" in doctor

    [brief] = (home / "data").glob("*/brief.md")
    preferences = brief.read_text().split("## Standing preferences\n", 1)[1].split("\n## Task Skills", 1)[0]
    assert "Prefer small commits" in preferences
    assert "### Not yet merged from captain.md\n# Preferences\n\n- 2025-03-04: Use British spelling" in preferences

    cos(home, jevscript_bin, "remember", "Run the linter first")
    assert "Run the linter first" in principal.read_text() and captain.read_bytes() == kept[captain]


def test_identical_preference_files_are_both_kept_and_briefed_once(make_host, jevscript_bin: str, tmp_path: Path) -> None:
    host = make_host()
    cli_rules(host, tmp_path, [])
    home = host.home.root
    text = "# Preferences\n\n- 2026-09-01: Prefer small commits\n"
    for name in ("principal.md", "captain.md"):
        (home / "data" / name).write_text(text)

    runs = [cos(home, jevscript_bin, "say", "Update the guide", "--project", "proj"), cos(home, jevscript_bin, "tick")]
    assert all("migration" not in run.stderr for run in runs)
    assert all((home / "data" / name).read_text() == text for name in ("principal.md", "captain.md"))
    [brief] = (home / "data").glob("*/brief.md")
    assert brief.read_text().count("Prefer small commits") == 1 and "Not yet merged" not in brief.read_text()


def test_help_names_the_homes_identity_without_creating_or_migrating_it(make_host, jevscript_bin: str, tmp_path: Path) -> None:
    fresh = tmp_path / "never-created"
    default = cos(fresh, jevscript_bin, "--help").stdout
    assert "Chief of Staff runs your administration of agent work" in default
    assert not fresh.exists(), "help does not create a home"

    host = make_host()
    home = host.home.root
    make_legacy(home)
    legacy = {name: fingerprint(home / name) for name in ("config.json", "data/captain.md")}
    cos(home, jevscript_bin, "--help")
    assert {name: fingerprint(home / name) for name in legacy} == legacy and not (home / "data" / "principal.md").exists(), "help does not migrate"

    cos(home, jevscript_bin, "config", "set", "identity.name", "Abigail")
    named = cos(home, jevscript_bin, "--help").stdout
    assert "Abigail runs your administration of agent work" in named and "Chief of Staff" not in named
    assert "hand a request to Abigail" in named and "open an interactive agent session with Abigail" in named
    assert "ministers: instances of Abigail" in named
    for verb in ("briefing", "red-box", "minister"):
        assert verb in named

    config = read_json(home / "config.json", {})
    config["scout"] = {"models": {"old-harness": "old-model"}}
    (home / "config.json").write_text(json.dumps(config))
    before_help = fingerprint(home / "config.json")
    assert "Abigail runs your administration" in cos(home, jevscript_bin, "--help").stdout
    assert fingerprint(home / "config.json") == before_help
    env = {**os.environ, "JEVSCRIPT_BIN": jevscript_bin, "COS_JEV": "fake", "PYTHONPATH": str(SRC)}
    show = subprocess.run([sys.executable, "-m", "cos", "--home", str(home), "config", "show"], env=env, capture_output=True, text=True, check=False)
    assert show.returncode != 0 and "scout.models has no native binding" in show.stderr


def configure_identity(home: Path, jevscript_bin: str) -> None:
    cos(home, jevscript_bin, "config", "set", "identity.name", "Abigail")
    cos(home, jevscript_bin, "config", "set", "identity.principal", "Ankeeth")


def test_configured_names_reach_notifications_briefs_and_the_session(make_host, jevscript_bin: str, tmp_path: Path) -> None:
    status_rule = [{"match": {"id": "^work$", "text": "going on"}, "answer": {"choice": "status"}}]
    host = make_host()
    cli_rules(host, tmp_path, status_rule)
    home = host.home.root
    host.home.set_config("desktop_notifications", True)
    configure_identity(home, jevscript_bin)
    bin_dir = tmp_path / "bin"
    osascript, claude = fake_bin(bin_dir, "osascript"), fake_bin(bin_dir, "claude")

    status_say = cos(home, jevscript_bin, "say", "What's going on?", path=bin_dir)
    status_tick = cos(home, jevscript_bin, "tick", path=bin_dir)
    outbox = [json.loads(line) for line in (home / "state" / "outbox.jsonl").read_text().splitlines()]
    assert outbox[-1]["name"] == "Abigail"
    assert outbox[-1]["message"] == cos(home, jevscript_bin, "status").stdout.strip()
    assert f"cos (Abigail): {outbox[-1]['message']}" in status_say.stderr + status_tick.stderr
    [call] = [json.loads(line) for line in osascript.read_text().splitlines()]
    assert call[0] == "-e" and call[1].endswith(' with title "Abigail"'), call

    cos(home, jevscript_bin, "say", "Update the guide", "--project", "proj", path=bin_dir)
    cos(home, jevscript_bin, "tick", path=bin_dir)
    [brief] = (home / "data").glob("*/brief.md")
    text = brief.read_text()
    assert "You are working for Abigail on behalf of Ankeeth." in text
    assert "4. If a decision belongs to Ankeeth, append needs-decision" in text
    assert "Chief of Staff" not in text and "the person who owns the work" not in text

    cos(home, jevscript_bin, "session", "--agent", "claude", "--no-watch", path=bin_dir)
    [argv] = [json.loads(line) for line in claude.read_text().splitlines()]
    assert argv[0] == "--append-system-prompt" and argv[2] == "Start with the briefing and the red box."
    prompt = argv[1]
    assert prompt.startswith("# Abigail agent session\n")
    assert f"Ankeeth's conversational front door to Abigail, whose home is\n`{home}`" in prompt
    assert "Never write\nAbigail's state files directly." in prompt
    assert "Chief of Staff" not in prompt and "{" not in prompt


def test_new_minister_inherits_the_named_parent_in_its_worker_brief(make_host, jevscript_bin: str, tmp_path: Path) -> None:
    host = make_host()
    home = host.home.root
    cli_rules(host, tmp_path, [])
    configure_identity(home, jevscript_bin)
    cos(home, jevscript_bin, "minister", "add", "ops", "--portfolio", "operations")
    child = home / "mates" / "ops"
    raw = read_json(child / "config.json", {})
    assert raw["identity"] == {"name": "Abigail", "principal": "Ankeeth"}
    parent_config = read_json(home / "config.json", {})
    for key, value in (("jev.mode", "fake"), ("jev.fake_rules", parent_config["jev"]["fake_rules"]), ("adapters.staff", parent_config["adapters"]["staff"])):
        cos(child, jevscript_bin, "config", "set", key, json.dumps(value))
    cos(child, jevscript_bin, "say", "Update the guide", "--project", "proj")
    cos(child, jevscript_bin, "tick")
    [brief] = (child / "data").glob("*/brief.md")
    assert "You are working for Abigail on behalf of Ankeeth." in brief.read_text()


def test_existing_minister_acquires_missing_identity_once(make_host, jevscript_bin: str) -> None:
    host = make_host()
    home = host.home.root
    configure_identity(home, jevscript_bin)
    cos(home, jevscript_bin, "minister", "add", "ops", "--portfolio", "operations")
    child = home / "mates" / "ops"
    child_config = read_json(child / "config.json", {})
    child_config.pop("identity")
    (child / "config.json").write_text(json.dumps(child_config))
    assert "identity" not in read_json(child / "config.json", {})
    cos(child, jevscript_bin, "status")
    assert read_json(child / "config.json", {})["identity"] == {"name": "Abigail", "principal": "Ankeeth"}
    migrated = fingerprint(child / "config.json")
    cos(child, jevscript_bin, "status")
    assert fingerprint(child / "config.json") == migrated


def test_minister_explicit_identity_survives_migration_and_parent_renames(make_host, jevscript_bin: str) -> None:
    host = make_host()
    home = host.home.root
    configure_identity(home, jevscript_bin)
    cos(home, jevscript_bin, "minister", "add", "ops", "--portfolio", "operations")
    child = home / "mates" / "ops"
    child_config = read_json(child / "config.json", {})
    child_config["identity"] = {"name": "Beatrice"}
    (child / "config.json").write_text(json.dumps(child_config))
    cos(child, jevscript_bin, "status")
    assert read_json(child / "config.json", {})["identity"] == {"name": "Beatrice", "principal": "Ankeeth"}
    cos(home, jevscript_bin, "config", "set", "identity.name", "Charlotte")
    cos(home, jevscript_bin, "config", "set", "identity.principal", "Someone Else")
    cos(child, jevscript_bin, "status")
    assert read_json(child / "config.json", {})["identity"] == {"name": "Beatrice", "principal": "Ankeeth"}
    cos(child, jevscript_bin, "config", "set", "identity.name", "Diana")
    cos(child, jevscript_bin, "status")
    assert read_json(child / "config.json", {})["identity"] == {"name": "Diana", "principal": "Ankeeth"}


def test_minister_keeps_parent_defaults_after_parent_changes(make_host, jevscript_bin: str, tmp_path: Path) -> None:
    host = make_host()
    home = host.home.root
    assert "identity" not in read_json(home / "config.json", {})
    cli_rules(host, tmp_path, [])
    cos(home, jevscript_bin, "minister", "add", "ops", "--portfolio", "operations")
    child = home / "mates" / "ops"
    expected = {"name": "Chief of Staff", "principal": "the principal"}
    assert read_json(child / "config.json", {})["identity"] == expected
    cos(home, jevscript_bin, "config", "set", "identity.name", "Abigail")
    parent_config = read_json(home / "config.json", {})
    for key, value in (("jev.mode", "fake"), ("jev.fake_rules", parent_config["jev"]["fake_rules"]), ("adapters.staff", parent_config["adapters"]["staff"])):
        cos(child, jevscript_bin, "config", "set", key, json.dumps(value))
    cos(child, jevscript_bin, "say", "Update the guide", "--project", "proj")
    cos(child, jevscript_bin, "tick")
    [brief] = (child / "data").glob("*/brief.md")
    assert "You are working for Chief of Staff on behalf of the principal." in brief.read_text()
    child_config = fingerprint(child / "config.json")
    (home / "config.json").write_text("{invalid json")
    cos(child, jevscript_bin, "status")
    assert fingerprint(child / "config.json") == child_config


def test_minister_inherits_identity_despite_unrelated_parent_config_error(make_host, jevscript_bin: str) -> None:
    host = make_host()
    home = host.home.root
    configure_identity(home, jevscript_bin)
    parent_config = read_json(home / "config.json", {})
    parent_config["scout"] = {"models": {"old-harness": "old-model"}}
    (home / "config.json").write_text(json.dumps(parent_config))
    cos(home, jevscript_bin, "minister", "add", "ops", "--portfolio", "operations")
    child = home / "mates" / "ops"
    expected = {"name": "Abigail", "principal": "Ankeeth"}
    assert read_json(child / "config.json", {})["identity"] == expected
    child_config = read_json(child / "config.json", {})
    child_config["identity"] = {"name": "Beatrice"}
    (child / "config.json").write_text(json.dumps(child_config))
    cos(child, jevscript_bin, "status")
    assert read_json(child / "config.json", {})["identity"] == {"name": "Beatrice", "principal": "Ankeeth"}


def test_minister_keeps_unmerged_preferences_without_overwriting_child_files(make_host, jevscript_bin: str, tmp_path: Path) -> None:
    host = make_host()
    home = host.home.root
    cli_rules(host, tmp_path, [])
    principal, captain = home / "data" / "principal.md", home / "data" / "captain.md"
    principal.write_bytes(b"# Preferences\n\n- Prefer small commits\n")
    captain.write_bytes(b"# Preferences\n\n- Use British spelling\n")
    cos(home, jevscript_bin, "minister", "add", "ops", "--portfolio", "operations")
    child = home / "mates" / "ops"
    assert (child / "data" / "principal.md").read_bytes() == principal.read_bytes()
    assert (child / "data" / "captain.md").read_bytes() == captain.read_bytes()

    parent_config = read_json(home / "config.json", {})
    for key, value in (("jev.mode", "fake"), ("jev.fake_rules", parent_config["jev"]["fake_rules"]), ("adapters.staff", parent_config["adapters"]["staff"])):
        cos(child, jevscript_bin, "config", "set", key, json.dumps(value))
    cos(child, jevscript_bin, "say", "Update the guide", "--project", "proj")
    cos(child, jevscript_bin, "tick")
    [brief] = (child / "data").glob("*/brief.md")
    preferences = brief.read_text().split("## Standing preferences\n", 1)[1].split("\n## Task Skills", 1)[0]
    assert "Prefer small commits" in preferences
    assert "### Not yet merged from captain.md\n# Preferences\n\n- Use British spelling" in preferences

    child_principal, child_captain = child / "data" / "principal.md", child / "data" / "captain.md"
    child_principal.write_bytes(b"# Child principal\n")
    child_captain.write_bytes(b"# Child captain\n")
    child_projects = child / "data" / "projects.json"
    child_projects.write_bytes(b"[]\n")
    projects_before = child_projects.read_bytes()
    cos(home, jevscript_bin, "minister", "add", "ops", "--portfolio", "operations")
    assert child_principal.read_bytes() == b"# Child principal\n"
    assert child_captain.read_bytes() == b"# Child captain\n"
    assert child_projects.read_bytes() == projects_before

    captain.write_bytes(principal.read_bytes())
    cos(home, jevscript_bin, "minister", "add", "docs", "--portfolio", "documentation")
    docs = home / "mates" / "docs" / "data"
    assert (docs / "principal.md").read_bytes() == principal.read_bytes()
    assert not (docs / "captain.md").exists()


def test_the_reviewer_brief_names_the_cos_and_the_principal(make_host, jevscript_bin: str) -> None:
    host = make_host(
        rules=[{"match": {"id": "^verdict$"}, "answer": {"choice": "approve"}}],
        scripts=[{"match": "^review: ", "steps": ["work", "idle"]}, {"match": ".", "steps": ["work", "commit", "done", "idle"]}],
        config={"review_profile": "lead", "identity": {"name": "Abigail", "principal": "Ankeeth"}},
    )
    host.registry.set_profiles(host.registry.profiles() + [{"name": "lead", "rule": "reviews other workers' changes", "harness": "codex"}])
    host.submit("Add input validation")
    run_until(host, lambda: any((host.home.data).glob("*-review/brief.md")))
    [review] = (host.home.data).glob("*-review/brief.md")
    assert review.read_text().startswith("# Review\nYou are working for Abigail on behalf of Ankeeth.\n")


def test_old_verbs_and_config_key_are_aliases_of_the_new_ones(make_host, jevscript_bin: str) -> None:
    host = make_host()
    home = host.home.root
    host.decisions.record("choice", "Choose a path?", ["A", "B"])
    added = cos(home, jevscript_bin, "minister", "add", "ops", "--portfolio", "operations work").stdout
    assert added.startswith("Minister ops ready at ") and added.strip().endswith("with the portfolio: operations work")
    cos(home, jevscript_bin, "mate", "add", "docs", "--scope", "documentation")

    def same(new: list[str], old: list[str]) -> str:
        out = cos(home, jevscript_bin, *new).stdout
        assert out == cos(home, jevscript_bin, *old).stdout
        return out

    ministers = same(["minister", "list"], ["mate", "list"])
    assert ministers.splitlines() == [f"ops (portfolio: operations work): {home / 'mates' / 'ops'}", f"docs (portfolio: documentation): {home / 'mates' / 'docs'}"]
    assert same(["red-box"], ["decisions"]).strip() == "[choice] Choose a path? (A / B)"
    briefing = same(["briefing"], ["bearings"])
    status = cos(home, jevscript_bin, "status").stdout.strip()
    assert briefing.splitlines()[0] == f"Briefing: {status}"
    assert "\nMinisters\n  - ops (portfolio: operations work): 0 under way, 0 waiting" in briefing
    charter = (home / "mates" / "ops" / "data" / "charter.md").read_text()
    assert f"Minister `ops` of {home}.\nPortfolio: operations work\n" in charter

    entry: dict[str, Any] = {"command": ["legacy-agent"]}
    cos(home, jevscript_bin, "config", "set", "adapters.crew", json.dumps(entry))
    adapters = read_json(home / "config.json", {})["adapters"]
    assert adapters["staff"] == entry and "crew" not in adapters
    doctor = cos(home, jevscript_bin, "doctor").stdout
    assert "staff adapter: ['legacy-agent']" in doctor and "identity: Chief of Staff, working for the principal" in doctor
