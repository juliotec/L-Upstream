#!/usr/bin/env python3
"""A pull request's built website (dist/), as ci.yml's website job built and kept it, for website.yml's preview.

ci.yml builds every pull request's site once and keeps its dist/ as the artifact website-dist-<head commit>. The
preview's jobs (pull_request_target, main's code) take it from there instead of building the pull request again:

  python3 -I scripts/ci_website_dist.py wait --sha SHA [--timeout SECONDS]      waits for the artifact; prints found=true
                                                                                 or found=false (to $GITHUB_OUTPUT too)
  python3 -I scripts/ci_website_dist.py fetch --sha SHA --out DIR               downloads and unpacks it into DIR

An artifact counts only when its run is ci.yml's, of a pull_request, at that very commit (the run's commit_sha): an
artifact of the same name from any other run is ignored. Unpacked as files only: a name that is absolute, has .. or
is a link is refused. Standard library only; it runs nothing of the pull request's. FORGEJO_TOKEN (the job's token)
reads the API. No game data is involved.
"""

from __future__ import annotations

import argparse
import io
import json
import os
import stat
import sys
import time
import urllib.request
import zipfile
from pathlib import Path, PurePosixPath


def api(path: str, raw: bool = False):
    server = os.environ.get("GITHUB_SERVER_URL", "https://git.magicbean.dev")
    repo = os.environ.get("GITHUB_REPOSITORY", "MagicBeanDev/Longshot")
    url = path if path.startswith("http") else f"{server}/api/v1/repos/{repo}{path}"
    req = urllib.request.Request(url)
    token = os.environ.get("FORGEJO_TOKEN")
    if token:
        req.add_header("Authorization", f"token {token}")
    with urllib.request.urlopen(req, timeout=120) as r:
        data = r.read()
    return data if raw else json.loads(data)


def find(sha: str) -> dict | None:
    """The newest artifact website-dist-<sha> of ci.yml's pull_request run at sha, or None"""
    name = f"website-dist-{sha}"
    found = api(f"/actions/artifacts?name={name}")
    items = found.get("artifacts", []) if isinstance(found, dict) else found
    for a in sorted((a for a in items if a.get("name") == name and not a.get("expired")), key=lambda a: -a["id"]):
        run = api(f"/actions/runs/{a['run_id']}")
        if run.get("workflow_id") == "ci.yml" and run.get("event") == "pull_request" and run.get("commit_sha") == sha:
            return a
    return None


def unpack(data: bytes, out: Path) -> int:
    out.mkdir(parents=True, exist_ok=True)
    n = 0
    with zipfile.ZipFile(io.BytesIO(data)) as z:
        for info in z.infolist():
            p = PurePosixPath(info.filename)
            if p.is_absolute() or ".." in p.parts or stat.S_ISLNK(info.external_attr >> 16):
                raise SystemExit(f"ci_website_dist: refused {info.filename!r}")
            if info.is_dir():
                continue
            dest = out.joinpath(*p.parts)
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_bytes(z.read(info))
            n += 1
    return n


def output(line: str) -> None:
    print(line)
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as f:
            f.write(line + "\n")


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("what", choices=("wait", "fetch"))
    ap.add_argument("--sha", required=True)
    ap.add_argument("--timeout", type=int, default=1800)
    ap.add_argument("--out", type=Path)
    a = ap.parse_args(argv)
    if len(a.sha) != 40 or any(c not in "0123456789abcdef" for c in a.sha):
        ap.error("--sha: a full commit id")
    if a.what == "wait":
        start = time.time()
        end = start + a.timeout
        print(f"::notice::waiting for ci.yml's website job to keep website-dist-{a.sha} (the pull request's built site), "
              f"up to {a.timeout // 60} minutes", flush=True)
        last = 0
        while True:
            try:
                if find(a.sha):
                    output("found=true")
                    return 0
            except OSError as e:   # (the API busy: asked again)
                print(f"ci_website_dist: {e}", file=sys.stderr)
            waited = int(time.time() - start) // 60
            if waited > last:
                last = waited
                print(f"still waiting for website-dist-{a.sha[:12]}: {waited} of {a.timeout // 60} minutes", flush=True)
            if time.time() > end:
                print(f"::warning::no website-dist-{a.sha} from ci.yml after {a.timeout} s: its website job failed or "
                      "was cancelled, so there is no preview", file=sys.stderr)
                output("found=false")
                return 0
            time.sleep(20)
    if not a.out:
        ap.error("fetch needs --out")
    art = find(a.sha)
    if not art:
        raise SystemExit(f"ci_website_dist: no website-dist-{a.sha} from ci.yml")
    n = unpack(api(art["archive_download_url"], raw=True), a.out)
    print(f"ci_website_dist: {n} files of run {art['run_id']} in {a.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
