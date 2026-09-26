"""Collect the license notices of every crate linked into a release CLI."""

from __future__ import annotations

import hashlib
import json
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
# Every Rust triple a release package ships, so one notices file covers them all.
RUST_TARGETS = ("aarch64-apple-darwin", "x86_64-apple-darwin", "aarch64-unknown-linux-gnu",
                "x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc")
LICENSE_FILE = re.compile(r"(?i)^(licen[cs]e|copying|notice|unlicense)")
MIT = """Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"""


def linked_crates() -> set[str]:
    """Name every normal (linked) dependency of the CLI on a release target."""
    targets = [arg for target in RUST_TARGETS for arg in ("--target", target)]
    tree = subprocess.check_output(
        ["cargo", "tree", "-p", "jevscript-cli", "-e", "normal", "--prefix", "none", "--locked", *targets],
        cwd=ROOT, text=True)
    return {" ".join(line.split()[:2]) for line in tree.splitlines() if line.strip()}


def license_texts(package: dict) -> list[str]:
    """Read a crate's shipped license files, or fill the MIT template from its authors."""
    directory = Path(package["manifest_path"]).parent
    files = sorted(path for path in directory.iterdir() if path.is_file() and LICENSE_FILE.match(path.name))
    if files:
        return [path.read_text(encoding="utf-8", errors="replace").strip() + "\n" for path in files]
    if "MIT" in re.split(r"[\s()/]+", package["license"] or ""):
        holders = ", ".join(package["authors"]) or f"the {package['name']} authors"
        return [f"Copyright (c) {holders}\n\n{MIT}"]
    raise SystemExit(f"{package['name']} {package['version']} ships no license text")


def render() -> str:
    """Group crates by identical license text so each text appears once."""
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked"], cwd=ROOT))
    wanted = linked_crates()
    groups: dict[str, tuple[str, list[str]]] = {}
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if package["source"] is None or f"{package['name']} v{package['version']}" not in wanted:
            continue
        line = f"{package['name']} {package['version']} ({package['license']})"
        if package.get("repository"):
            line += f" {package['repository']}"
        for text in license_texts(package):
            groups.setdefault(hashlib.sha256(text.encode()).hexdigest(), (text, []))[1].append(line)
    parts = ["The jevscript CLI binary links the following third-party Rust crates. Each crate",
             "is listed with its declared license, followed by the license texts it ships.", ""]
    for text, crates in groups.values():
        parts += ["=" * 78, *crates, "-" * 78, text]
    return "\n".join(parts)


if __name__ == "__main__":
    print(render(), end="")
