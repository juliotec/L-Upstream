#!/usr/bin/env python3
"""The workspace's own outputs out of a target folder, before CI saves it as a cache (.forgejo/workflows/ci.yml).

A checkout gives every file a new modification time, and cargo judges a path crate's sources by it: whatever a cache
holds, a run builds every crate of the workspace again. What a run reuses is the registry crates' outputs (judged by their
version and checksum) and the downloads, so only those are kept: the cache stays the size of the dependencies, the same
from one push to the next, instead of growing with every version of the workspace's crates, and target/tmp (tests'
scratch, CARGO_TARGET_TMPDIR) goes too. Removing too much only costs a rebuild, never a wrong build.

  python3 -I scripts/ci_cargo_prune.py [target]      (from the workspace's root; target/ by default)
"""

from __future__ import annotations

import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

# (<name>-<16 hex digits>, then an extension or nothing: cargo's metadata hash in an output's name)
HASHED = re.compile(r"^(?P<stem>.+)-[0-9a-f]{16}(?:\..*)?$")


def workspace_names() -> tuple[set[str], set[str]]:
    """The workspace's package names, and every crate name its targets build under (lib, bins, tests, examples...)."""
    out = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1", "--offline"],
        check=True, capture_output=True, text=True,
    ).stdout
    packages, crates = set(), set()
    for p in json.loads(out)["packages"]:
        packages.add(p["name"])
        crates.add(p["name"].replace("-", "_"))
        for t in p["targets"]:
            crates.add(t["name"].replace("-", "_"))
    return packages, crates


def remove(path: Path) -> None:
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    else:
        path.unlink()


def prune_profile(profile: Path, packages: set[str], crates: set[str]) -> int:
    """One profile's folder (target/debug, target/release, target/<triple>/debug...): what the workspace built goes."""
    n = 0
    for sub in ("incremental", "examples"):
        if (profile / sub).exists():
            shutil.rmtree(profile / sub)
            n += 1
    # (the uplifted programs and libraries, longshot-builder, liblongshot_rs.a, and their .d files)
    for f in profile.iterdir():
        if f.is_file():
            remove(f)
            n += 1
    for sub, names in ((".fingerprint", packages), ("build", packages), ("deps", crates)):
        folder = profile / sub
        if not folder.is_dir():
            continue
        for entry in folder.iterdir():
            m = HASHED.match(entry.name)
            if not m:
                continue
            stem = m["stem"]
            if sub == "deps" and stem.startswith("lib") and stem[3:] in names:
                stem = stem[3:]
            if stem in names:
                remove(entry)
                n += 1
    return n


def main() -> int:
    target = Path(sys.argv[1] if len(sys.argv) > 1 else "target")
    if not target.is_dir():
        print(f"ci_cargo_prune: no {target}: nothing to prune")
        return 0
    packages, crates = workspace_names()
    if (target / "tmp").exists():
        shutil.rmtree(target / "tmp")
    n = 0
    # (a profile's folder is any that has cargo's .fingerprint: target/<profile> or target/<triple>/<profile>)
    for fingerprint in sorted(target.glob(".fingerprint")) + sorted(target.glob("*/.fingerprint")) + sorted(
        target.glob("*/*/.fingerprint")
    ):
        n += prune_profile(fingerprint.parent, packages, crates)
    size = sum(f.stat().st_size for f in target.rglob("*") if f.is_file() and not f.is_symlink())
    print(f"ci_cargo_prune: {n} of the workspace's outputs removed; {target} is {size / 1e6:.0f} MB, the dependencies'")
    return 0


if __name__ == "__main__":
    sys.exit(main())
