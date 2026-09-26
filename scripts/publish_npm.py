"""Publish the verified npm tarball, or accept the exact bytes already on npm."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tarfile
import time
import urllib.error
import urllib.request


def package_identity(tarball: Path) -> tuple[str, str]:
    """Read the identity npm will publish from the staged archive."""
    with tarfile.open(tarball, "r:gz") as archive:
        package = json.load(archive.extractfile("package/package.json"))
    return package["name"], package["version"]


def published_state(tarball: Path, registry: str) -> str:
    """Compare the registry's exact tarball digest with the staged archive."""
    name, version = package_identity(tarball)
    if name != "jevscript":
        raise ValueError(f"unexpected npm package: {name}")
    url = f"{registry.rstrip('/')}/{name}/{version}"
    try:
        with urllib.request.urlopen(url, timeout=30) as response:
            metadata = json.load(response)
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return "missing"
        raise
    if metadata.get("name") != name or metadata.get("version") != version:
        raise ValueError("npm registry returned a different package identity")
    digest = base64.b64encode(hashlib.sha512(tarball.read_bytes()).digest()).decode("ascii")
    integrity = metadata.get("dist", {}).get("integrity")
    if integrity != f"sha512-{digest}":
        raise ValueError(f"npm {name}@{version} differs from the verified tarball")
    return "match"


def main() -> None:
    """Publish once and recheck npm, or skip only on an exact digest match."""
    parser = argparse.ArgumentParser()
    parser.add_argument("bundle", type=Path)
    parser.add_argument("--registry", default="https://registry.npmjs.org")
    parser.add_argument("--bootstrap", action="store_true")
    parser.add_argument("--require-published", action="store_true")
    args = parser.parse_args()
    tarballs = list(args.bundle.glob("jevscript-*.tgz"))
    if len(tarballs) != 1:
        raise ValueError("verified bundle must contain exactly one npm tarball")
    tarball = tarballs[0]
    state = published_state(tarball, args.registry)
    if state == "match":
        print("npm already holds the exact verified tarball")
        return
    if args.require_published:
        raise ValueError("npm is missing the verified tarball")
    if args.bootstrap and not os.environ.get("NODE_AUTH_TOKEN"):
        raise ValueError("npm bootstrap credential is absent")
    # The explicit relative path matters: npm can parse an unprefixed path as
    # a package or Git specifier. No package build occurs in this job.
    path = os.path.relpath(tarball.resolve(), Path.cwd())
    if not path.startswith("."):
        path = f"./{path}"
    subprocess.run(["npm", "publish", path, "--access", "public", "--provenance"], check=True)
    for attempt in range(6):
        if published_state(tarball, args.registry) == "match":
            print("npm holds the exact verified tarball")
            return
        if attempt < 5:
            time.sleep(5)
    raise ValueError("published npm tarball was not visible with the verified digest")


if __name__ == "__main__":
    main()
