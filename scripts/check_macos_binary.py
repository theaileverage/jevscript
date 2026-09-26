"""Reject a macOS release binary newer than its advertised wheel floor."""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import subprocess


FLOORS = {"darwin-arm64": (11, 0), "darwin-x64": (10, 15)}


def version_tuple(text: str) -> tuple[int, ...]:
    """Compare Mach-O version components numerically."""
    return tuple(int(part) for part in text.split("."))


def check(binary: Path, target: str) -> None:
    """Inspect the final Mach-O load command, not the build runner's OS."""
    if target not in FLOORS:
        raise ValueError(f"not a macOS release target: {target}")
    output = subprocess.check_output(["otool", "-l", str(binary)], text=True)
    minimums = []
    for block in re.split(r"(?=Load command \d+)", output):
        if "cmd LC_BUILD_VERSION" in block:
            match = re.search(r"\bminos ([0-9.]+)", block)
        elif "cmd LC_VERSION_MIN_MACOSX" in block:
            match = re.search(r"\bversion ([0-9.]+)", block)
        else:
            continue
        if match is None:
            raise ValueError("Mach-O minimum OS command has no version")
        minimums.append(version_tuple(match.group(1)))
    if len(minimums) != 1:
        raise ValueError(f"expected one Mach-O minimum OS command, found {len(minimums)}")
    actual = minimums[0]
    floor = FLOORS[target]
    width = max(len(actual), len(floor))
    if actual + (0,) * (width - len(actual)) > floor + (0,) * (width - len(floor)):
        raise ValueError(f"{target} binary requires macOS {actual}, wheel advertises {floor}")
    print(f"validated {target} Mach-O minimum macOS {'.'.join(map(str, actual))}")


def main() -> None:
    """Check a final release binary against its wheel platform tag."""
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", choices=FLOORS, required=True)
    parser.add_argument("binary", type=Path)
    args = parser.parse_args()
    check(args.binary, args.target)


if __name__ == "__main__":
    main()
