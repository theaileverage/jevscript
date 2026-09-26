"""Compare published wheel digests with the verified release bundle."""

from __future__ import annotations

import argparse
import hashlib
import json
import urllib.error
import urllib.request
from pathlib import Path


def check(bundle: Path, *, require_all: bool = False, registry: str = "https://pypi.org") -> None:
    wheels = sorted((bundle / "wheelhouse").glob("*.whl"))
    if not wheels:
        raise ValueError("verified bundle has no wheels")
    versions = {wheel.name.split("-", 2)[1] for wheel in wheels}
    if len(versions) != 1:
        raise ValueError("verified wheels have different versions")
    version = versions.pop()
    url = f"{registry.rstrip('/')}/pypi/jevscript/{version}/json"
    try:
        with urllib.request.urlopen(url, timeout=30) as response:
            published = json.load(response)["urls"]
    except urllib.error.HTTPError as error:
        if error.code != 404:
            raise
        published = []
    by_name = {entry["filename"]: entry for entry in published}
    unexpected = set(by_name) - {wheel.name for wheel in wheels}
    if unexpected:
        raise ValueError(f"PyPI has unreviewed distributions: {sorted(unexpected)}")
    for wheel in wheels:
        entry = by_name.get(wheel.name)
        if entry is None:
            if require_all:
                raise ValueError(f"wheel missing from PyPI: {wheel.name}")
            continue
        expected = hashlib.sha256(wheel.read_bytes()).hexdigest()
        actual = entry["digests"]["sha256"]
        if actual != expected:
            raise ValueError(f"PyPI wheel differs from verified bundle: {wheel.name}")
    if require_all and set(by_name) != {wheel.name for wheel in wheels}:
        raise ValueError("PyPI distribution set differs from verified wheels")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("bundle", type=Path)
    parser.add_argument("--require-all", action="store_true")
    parser.add_argument("--registry", default="https://pypi.org")
    args = parser.parse_args()
    check(args.bundle, require_all=args.require_all, registry=args.registry)


if __name__ == "__main__":
    main()
