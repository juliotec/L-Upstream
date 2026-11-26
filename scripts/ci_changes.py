#!/usr/bin/env python3
"""Whether a pull request needs ci.yml's compiling jobs (rust, host-windows) and its renderer build check, from its files.

Those jobs read far more than builder/: the workspace's tests and build scripts read host/, gpu/, game/, assets/,
third_party/, scripts/, tests/, tools.lock and docs/dev/reference/build-events.md. So the rule is the other way round:
they run unless every changed file is in DOCS_ONLY, a place none of them reads. Anything unsure runs them: a push, a
run by hand, an API that does not answer, a pull request of more files than the API lists.

The renderer build check (`longshot-builder compile --only renderer`, without and with AMD FSR 4) runs when a changed
file is one of RENDERER: what that compile reads (build-compile's plan.rs, the renderer's part, and lib.rs's generated
files), the builder that compiles it, the tools it compiles with, and the job itself.

  python3 -I scripts/ci_changes.py --server URL --repo OWNER/NAME --pr N      prints code=true|false and
                                                                              renderer=true|false, and appends them to
                                                                              $GITHUB_OUTPUT when that is set

Standard library only; it reads the pull request's file list from the API (FORGEJO_TOKEN when set, else anonymous) and
runs none of the pull request's code. No game data is involved.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
import sys
import urllib.request

# (what the compiling jobs never read: the site, the docs but the one the builder's tests compare with its codes, the
# other workflows' files, the top-level documents. A file outside these, or one of KEEP, runs them)
DOCS_ONLY = (
    "website/*",
    "docs/*",
    ".forgejo/ISSUE_TEMPLATE/*",
    ".forgejo/pull_request_template.md",
    ".forgejo/workflows/website.yml",
    ".forgejo/workflows/pr-description.yml",
    ".forgejo/workflows/third-party.yml",
    ".forgejo/workflows/ci-image.yml",
    "AGENTS.md",
    "CONTRIBUTING.md",
    "README.md",
    "RELEASING.md",
    "LICENSE",
)
KEEP = ("docs/dev/reference/build-events.md",)
# (what `compile --only renderer` reads: gpu/, the host and game headers it includes, the host files it compiles beside it
# (host/posix/, pgo_rt.c, the MinGW shims), the third-party parts its plan names, the logo and its profile; the builder
# and its Rust, tools.lock's compilers and CPM cache, the CI image's tools; and the job's own files)
RENDERER = (
    "gpu/*",
    "host/*.h",
    "host/posix/*",
    "host/pgo_rt.c",
    "host/mingw_msvc_shims.c",
    "game/*.h",
    "third_party/fidelityfx/*",
    "third_party/fonts/*",
    "third_party/imgui/*",
    "third_party/reshade_fx/*",
    "third_party/stb_image/*",
    "third_party/tracy/*",
    "third_party/xess/*",
    "third_party/zbic/*",
    "assets/longshot.png",
    "assets/profiles/longshot_vk.profdata.xz",
    "builder/*",
    "Cargo.toml",
    "Cargo.lock",
    "mise.toml",
    "tools.lock",
    "scripts/longshot_builder.py",
    "scripts/linux/ci_setup.sh",
    "scripts/linux/checks.Dockerfile",
    "scripts/ci_changes.py",
    ".forgejo/workflows/ci.yml",
)
PAGE = 50
MAX_PAGES = 60  # (3000 files: past that, run everything)


def needs_code(files: list[str]) -> bool:
    """True when any changed file may change what rust or host-windows check (and for no file at all: unsure)."""
    if not files:
        return True
    for f in files:
        if f in KEEP or not any(fnmatch.fnmatchcase(f, p) for p in DOCS_ONLY):
            return True
    return False


def needs_renderer(files: list[str]) -> bool:
    """True when any changed file may change what `compile --only renderer` builds (and for no file at all: unsure)."""
    if not files:
        return True
    return any(fnmatch.fnmatchcase(f, p) for f in files for p in RENDERER)


def pr_files(server: str, repo: str, pr: int, token: str) -> list[str] | None:
    """Every path the pull request changes (renames by both names), or None when the API doesn't say in full."""
    files: list[str] = []
    for page in range(1, MAX_PAGES + 1):
        req = urllib.request.Request(f"{server}/api/v1/repos/{repo}/pulls/{pr}/files?limit={PAGE}&page={page}")
        req.add_header("Accept", "application/json")
        if token:
            req.add_header("Authorization", f"token {token}")
        with urllib.request.urlopen(req, timeout=60) as r:
            batch = json.load(r)
        if not batch:
            return files
        for item in batch:
            files.append(item["filename"])
            if item.get("previous_filename"):
                files.append(item["previous_filename"])
        if len(batch) < PAGE:
            return files
    return None


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--server", required=True)
    ap.add_argument("--repo", required=True)
    ap.add_argument("--pr", type=int, required=True)
    args = ap.parse_args(argv)
    try:
        files = pr_files(args.server.rstrip("/"), args.repo, args.pr, os.environ.get("FORGEJO_TOKEN", ""))
    except Exception as e:  # (any failure: unsure, so everything runs)
        print(f"::warning::ci_changes: the pull request's files are unknown ({e}): every job runs")
        files = None
    code = True if files is None else needs_code(files)
    renderer = True if files is None else needs_renderer(files)
    if files is not None:
        print(f"ci_changes: {len(files)} paths; the compiling jobs {'run' if code else 'are not needed (docs only)'}")
        print(f"ci_changes: the renderer build check {'runs' if renderer else 'is not needed (no renderer file)'}")
    lines = [f"code={'true' if code else 'false'}", f"renderer={'true' if renderer else 'false'}"]
    print("\n".join(lines))
    out = os.environ.get("GITHUB_OUTPUT")
    if out:
        with open(out, "a", encoding="utf-8") as f:
            f.write("".join(line + "\n" for line in lines))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
