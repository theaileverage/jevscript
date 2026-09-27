"""Install built artifacts offline and drive the shipped CLI and SDKs."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import tarfile
import tempfile
import venv
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
FORBIDDEN = (b"/Users/", b"/home/runner/", b"C:\\Users\\", b"docs/handoffs/", b"TYPESAFE_API_KEY=", b"-----BEGIN PRIVATE KEY-----")
EXTERNAL_NAME = bytes.fromhex("46697273746d617465").lower()


def check_source_name() -> None:
    """Keep external supervisor branding out of publishable tracked source."""
    names = subprocess.check_output(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=ROOT
    ).split(b"\0")
    for raw in names:
        if not raw:
            continue
        path = ROOT / os.fsdecode(raw)
        if EXTERNAL_NAME in raw.lower() or (not path.is_symlink() and path.is_file() and EXTERNAL_NAME in path.read_bytes().lower()):
            raise AssertionError(f"external supervisor name in source: {path.relative_to(ROOT)}")


def run(*args: str, cwd: pathlib.Path, env: dict[str, str]) -> None:
    """Run a command in the fresh installation, never the source tree."""
    subprocess.run(args, cwd=cwd, env=env, check=True)


def must_fail_integrity(*args: str, cwd: pathlib.Path, env: dict[str, str]) -> None:
    """A damaged installed CLI must not fall back to one on PATH."""
    result = subprocess.run(args, cwd=cwd, env=env, text=True, capture_output=True, check=False)
    if result.returncode == 0 or "integrity check" not in result.stderr:
        raise AssertionError(f"damaged CLI was not rejected: {result.returncode}: {result.stderr}")


def assert_lf(name: str, data: bytes) -> None:
    """A packaged Skill or example carries LF only, whatever runner built it."""
    if name.endswith((".jev", "/SKILL.md")) and b"\r" in data:
        raise AssertionError(f"carriage return in packaged text: {name}")


def inspect_tarball(path: pathlib.Path) -> tuple[dict, bytes]:
    """Audit exact npm package members and sensitive bytes."""
    allowed = ("package/dist/", "package/native/", "package/bin/", "package/examples/", "package/skills/")
    individual = {"package/package.json", "package/README.md", "package/LICENSE"}
    with tarfile.open(path, "r:gz") as archive:
        names = set()
        for member in archive.getmembers():
            if not member.isfile():
                continue
            names.add(member.name)
            if EXTERNAL_NAME in member.name.lower().encode() or "/.env" in member.name or member.name.endswith((".env", ".replay.jsonl")):
                raise AssertionError(f"environment file in npm package: {member.name}")
            if member.name not in individual and not member.name.startswith(allowed):
                raise AssertionError(f"unexpected npm member: {member.name}")
            data = archive.extractfile(member).read()  # type: ignore[union-attr]
            if any(pattern in data for pattern in FORBIDDEN) or EXTERNAL_NAME in data.lower():
                raise AssertionError(f"sensitive npm member: {member.name}")
            assert_lf(member.name, data)
        for required in ("package/LICENSE", "package/examples/inbox_triage.jev", "package/examples/package_smoke.jev", "package/native/manifest.json", "package/native/THIRD_PARTY_NOTICES.txt", "package/skills/jevscript/SKILL.md"):
            if required not in names:
                raise AssertionError(f"missing npm member: {required}")
        return (json.loads(archive.extractfile("package/native/manifest.json").read()),
                archive.extractfile("package/skills/jevscript/SKILL.md").read())  # type: ignore[union-attr]


def inspect_wheel(path: pathlib.Path, version: str) -> tuple[dict, bytes]:
    """Audit exact wheel members, including generated metadata."""
    if not path.name.startswith(f"jevscript-{version}-"):
        raise AssertionError("wheel filename differs from npm package version")
    with zipfile.ZipFile(path) as archive:
        names = set(archive.namelist())
        for name in names:
            if EXTERNAL_NAME in name.lower().encode() or "/.env" in name or name.endswith((".env", ".replay.jsonl")):
                raise AssertionError(f"environment file in wheel: {name}")
            if not (name.startswith("jevscript/") or name.startswith(f"jevscript-{version}.dist-info/")):
                raise AssertionError(f"unexpected wheel member: {name}")
            data = archive.read(name)
            if any(pattern in data for pattern in FORBIDDEN) or EXTERNAL_NAME in data.lower():
                raise AssertionError(f"sensitive wheel member: {name}")
            assert_lf(name, data)
        for suffix in ("entry_points.txt", "RECORD", "WHEEL"):
            if not any(name.endswith(suffix) for name in names):
                raise AssertionError(f"missing wheel {suffix}")
        if f"jevscript-{version}.dist-info/licenses/LICENSE" not in names:
            raise AssertionError("wheel lacks Apache-2.0 license text")
        if not {"jevscript/examples/inbox_triage.jev", "jevscript/examples/package_smoke.jev", "jevscript/_bin/manifest.json", "jevscript/_bin/THIRD_PARTY_NOTICES.txt", "jevscript/skills/jevscript/SKILL.md"}.issubset(names):
            raise AssertionError("wheel lacks the packaged example, Skill, CLI manifest or third-party notices")
        wheel = next(name for name in names if name.endswith(".dist-info/WHEEL"))
        if "Root-Is-Purelib: false" not in archive.read(wheel).decode():
            raise AssertionError("platform wheel incorrectly marked pure")
        return json.loads(archive.read("jevscript/_bin/manifest.json")), archive.read("jevscript/skills/jevscript/SKILL.md")


def assert_skill(path: pathlib.Path, expected: bytes) -> None:
    """Check installed Skill bytes through the coding-agent location."""
    if path.read_bytes() != expected:
        raise AssertionError(f"installed Skill bytes differ: {path}")


def main() -> None:
    """Exercise both artifacts with no registry or repository runtime path."""
    parser = argparse.ArgumentParser()
    parser.add_argument("--npm", type=pathlib.Path, required=True)
    parser.add_argument("--wheel", type=pathlib.Path, required=True)
    args = parser.parse_args()
    npm = args.npm.resolve()
    wheel = args.wheel.resolve()
    check_source_name()
    npm_manifest, npm_skill = inspect_tarball(npm)
    wheel_manifest, wheel_skill = inspect_wheel(wheel, npm_manifest["version"])
    source_skill = (ROOT / "skills/jevscript/SKILL.md").read_bytes()
    if npm_skill != wheel_skill or npm_skill != source_skill:
        raise AssertionError("npm, wheel and source Skills differ")
    if any(manifest["skill_sha256"] != hashlib.sha256(source_skill).hexdigest()
           for manifest in (npm_manifest, wheel_manifest)):
        raise AssertionError("Skill digest differs from package manifest")
    from build_release_cli import host_target

    target = host_target()
    if target.startswith("darwin-"):
        macos_version = subprocess.check_output(["sw_vers", "-productVersion"], text=True).strip()
        if macos_version.split(".")[0] != "15":
            raise AssertionError(f"macOS 15 installed-package smoke required, runner is {macos_version}")
        print(f"installed-package smoke on macOS {macos_version} {target}")
    if any(npm_manifest[key] != wheel_manifest[key] for key in ("version", "source_commit")):
        raise AssertionError("npm and wheel come from different versions or commits")
    if npm_manifest["targets"].get(target) != wheel_manifest["targets"].get(target):
        raise AssertionError("npm and wheel contain different CLI bytes")
    scratch = ROOT / ".release-tmp"
    scratch.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(dir=scratch) as directory:
        base = pathlib.Path(directory)
        js = base / "js"
        js.mkdir()
        py = base / "py"
        py.mkdir()
        env = os.environ.copy()
        env.pop("JEVSCRIPT_BIN", None)
        env["npm_config_cache"] = str(base / "npm-cache")
        run("npm.cmd" if os.name == "nt" else "npm", "install", "--offline", "--ignore-scripts", "--no-audit", str(npm), cwd=js, env=env)
        command = js / "node_modules" / ".bin" / ("jevscript.cmd" if os.name == "nt" else "jevscript")
        example = js / "node_modules" / "jevscript" / "examples" / "inbox_triage.jev"
        run(str(command), "--version", cwd=js, env=env)
        project = base / "project"
        project.mkdir()
        home = base / "home"
        home.mkdir()
        clean_bin = base / "clean-bin"
        clean_bin.mkdir()
        node = shutil.which("node")
        if node is None:
            raise AssertionError("Node is required for npm smoke")
        shutil.copy2(node, clean_bin / ("node.exe" if os.name == "nt" else "node"))
        setup_env = {**env, "HOME": str(home), "USERPROFILE": str(home), "PATH": str(clean_bin)}
        run(str(command), "setup", "--agent", "codex", "--agent", "claude-code", "--project", str(project), cwd=js, env=setup_env)
        assert_skill(project / ".agents/skills/jevscript/SKILL.md", source_skill)
        assert_skill(project / ".claude/skills/jevscript/SKILL.md", source_skill)
        run(str(command), "check", str(example), cwd=js, env=env)
        run("node", "--input-type=module", "-e", "import { load } from 'jevscript'; const p=await load(process.argv[1]); if (!p.judgments.some(j=>j.name==='triage')) throw Error('missing judgment'); await p.close()", str(example), cwd=js, env=env)
        no_model = js / "node_modules" / "jevscript" / "examples" / "package_smoke.jev"
        run("node", "--input-type=module", "-e", "import { load } from 'jevscript'; const p=await load(process.argv[1]); const pause=await p.task('main').start().next(); if(pause.kind!=='done') throw Error(JSON.stringify(pause)); await p.close()", str(no_model), cwd=js, env=env)
        venv_dir = py / "venv"
        venv.EnvBuilder(with_pip=True).create(venv_dir)
        python = venv_dir / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
        cli = venv_dir / ("Scripts/jevscript.exe" if os.name == "nt" else "bin/jevscript")
        run(str(python), "-m", "pip", "install", "--no-index", "--no-deps", str(wheel), cwd=py, env=env)
        run(str(cli), "--version", cwd=py, env=env)
        run(str(cli), "setup", "--global", "--agent", "codex", cwd=py, env=setup_env)
        assert_skill(home / ".agents/skills/jevscript/SKILL.md", source_skill)
        assert_skill(home / ".codex/skills/jevscript/SKILL.md", source_skill)
        code = "from importlib.resources import files; from jevscript import load; e=files('jevscript').joinpath('examples','inbox_triage.jev'); p=load(str(e)); assert any(j['name'] == 'triage' for j in p.judgments); p.close()"
        run(str(python), "-c", code, cwd=py, env=env)
        task_code = "from importlib.resources import files; from jevscript import load; e=files('jevscript').joinpath('examples','package_smoke.jev'); p=load(str(e)); assert p.task('main').start().next()['kind'] == 'done'; p.close()"
        run(str(python), "-c", task_code, cwd=py, env=env)
        py_example = subprocess.check_output([str(python), "-c", "from importlib.resources import files; print(files('jevscript').joinpath('examples','inbox_triage.jev'))"], cwd=py, env=env, text=True).strip()
        run(str(cli), "check", py_example, cwd=py, env=env)
        native_name = "jevscript.exe" if os.name == "nt" else "jevscript"
        js_native = js / "node_modules" / "jevscript" / "native" / target / native_name
        js_native.write_bytes(b"damaged binary")
        must_fail_integrity(str(command), "--version", cwd=js, env=env)
        must_fail_integrity("node", "--input-type=module", "-e", "import {load} from 'jevscript'; await load(process.argv[1])", str(example), cwd=js, env=env)
        py_native = pathlib.Path(subprocess.check_output([str(python), "-c", "from importlib.resources import files; print(files('jevscript').joinpath('_bin', 'jevscript.exe' if __import__('os').name == 'nt' else 'jevscript'))"], cwd=py, env=env, text=True).strip())
        py_native.write_bytes(b"damaged binary")
        must_fail_integrity(str(cli), "--version", cwd=py, env=env)
        must_fail_integrity(str(python), "-c", "from jevscript import load; load('anything.jev')", cwd=py, env=env)
    print("offline npm and Python artifact smoke passed")


if __name__ == "__main__":
    main()
