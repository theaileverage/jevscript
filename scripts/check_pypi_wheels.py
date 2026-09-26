"""Compare published wheel digests with the verified release bundle."""

from __future__ import annotations

import argparse
import hashlib
import json
import urllib.error
import urllib.request
from pathlib import Path


def check(bundle: Path, *, require_all: bool = False) -> None:
    wheels = sorted((bundle / "wheelhouse").glob("*.whl"))
    if not wheels:
        raise ValueError("verified bundle has no wheels")
    versions = {wheel.name.split("-", 2)[1] for wheel in wheels}
    if len(versions) != 1:
        raise ValueError("verified wheels have different versions")
    version = versions.pop()
    url = f"https://pypi.org/pypi/jevscript/{version}/json"
    try:
        with urllib.request.urlopen(url, timeout=30) as response:
            published = json.load(response)["urls"]
    except urllib.error.HTTPError as error:
        if error.code != 404:
            raise
        published = []
    by_name = {entry["filename"]: entry for entry in published}
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


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("bundle", type=Path)
    parser.add_argument("--require-all", action="store_true")
    args = parser.parse_args()
    check(args.bundle, require_all=args.require_all)


if __name__ == "__main__":
    main()
