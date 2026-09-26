"""Require a versioned release tag on a commit already merged into main."""

from __future__ import annotations

import argparse
from pathlib import Path
import subprocess
import tomllib


def git(*args: str) -> str:
    """Read a Git ref from the checkout used by the release workflow."""
    return subprocess.check_output(["git", *args], text=True).strip()


def main() -> None:
    """Fail before staging or publishing an unmerged or mismatched tag."""
    parser = argparse.ArgumentParser()
    parser.add_argument("--commit", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--ref-type", required=True)
    parser.add_argument("--main-ref", default="refs/remotes/origin/main")
    args = parser.parse_args()
    if args.ref_type != "tag":
        raise ValueError("release workflow must run on a tag ref")
    with Path("Cargo.toml").open("rb") as source:
        version = tomllib.load(source)["workspace"]["package"]["version"]
    if args.tag != f"v{version}":
        raise ValueError(f"release tag must be v{version}")
    tagged = git("rev-parse", f"refs/tags/{args.tag}^{{commit}}")
    if tagged != args.commit or git("rev-parse", "HEAD") != args.commit:
        raise ValueError("release tag, checkout and requested commit differ")
    if subprocess.run(["git", "merge-base", "--is-ancestor", args.commit, args.main_ref],
                      check=False).returncode != 0:
        raise ValueError("release commit is not merged into main")
    print(f"validated {args.tag} at merged main commit {args.commit}")


if __name__ == "__main__":
    main()
