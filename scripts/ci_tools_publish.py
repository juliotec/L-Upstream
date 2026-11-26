"""The Make tools workflow's publish job (.forgejo/workflows/tools.yml): the archives its make job made, twice and the
same, uploaded into Longshot's store and written as their tools.lock lines, on the deploy runner, which builds and runs
no program of the checkout's (this is Python's standard library only, read from the commit the workflow runs on).

    python3 -I scripts/ci_tools_publish.py <made> [--lock tools.lock] [--write-lock <file>]

<made>/<system>/<name>.tar.zst with <name>.version beside it (builder/tools/make.sh --out <made>): each its tools.lock
line, the version the one its recipe gave, the dest tools/<name>, the prefix -, the URLs the store's copy (the generic
package <name>-<system>, its version the line's, the file <sha256>-<name>.tar.zst: build-driver's tools/lock.rs
Tool::store_url). A version tools.lock names already with other bytes is refused before anything is uploaded (a package
version's files never change: a new version, its *_TRIM in builder/tools/pins.sh). Each is PUT to FORGEJO_UPLOAD_API's
server when it is set (past Cloudflare's 100 MB limit), with RELEASE_TOKEN (a token that may write the organization's
packages); a copy there already is kept when it is the same. Then each is checked to be in the store with its size and
SHA-256, its line printed, and --write-lock writes the lock with those lines in place of their tools' (a pull request's
tools.lock). Exit status 1 when something is wrong, 2 for a command line it does not take.
"""
import argparse
import hashlib
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request

OWNER = "MagicBeanDev"
SERVER = "https://git.magicbean.dev"
CDN = "https://git-cdn.magicbean.dev"
EXT = ".tar.zst"
SYSTEMS = ("linux-x64", "windows-x64")
NAME = re.compile(r"^[a-z0-9][a-z0-9._-]{0,63}$")
VERSION = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._+-]{0,127}$")
SHA = re.compile(r"^[0-9a-f]{64}$")
AGENT = "longshot-tools"


def fail(text):
    print("::error::ci_tools_publish: %s" % text, flush=True)
    sys.exit(1)


def read_lock(path):
    """[{name, version, system, sha256, size, urls}] of tools.lock's `tool` lines, each checked for what goes into a URL"""
    tools = []
    with open(path, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            w = line.split()
            if not w or w[0] != "tool":
                continue
            if len(w) < 9:
                fail("%s:%d: a tool line has fewer than 9 words" % (path, n))
            t = {"name": w[1], "version": w[2], "system": w[3], "sha256": w[4], "size": w[5], "urls": w[8:]}
            if not (NAME.match(t["name"]) and VERSION.match(t["version"]) and t["system"] in SYSTEMS
                    and SHA.match(t["sha256"]) and t["size"].isdigit()):
                fail("%s:%d: not a tool line this takes: %s" % (path, n, line.strip()))
            t["size"] = int(t["size"])
            tools.append(t)
    return tools


def store_path(t):
    return "/api/packages/%s/generic/%s-%s/%s/%s-%s%s" % (OWNER, t["name"], t["system"], t["version"], t["sha256"],
                                                          t["name"], EXT)


def upload_server():
    """FORGEJO_UPLOAD_API's server (<server>/api/v1) when it names one, else the store's (build-driver's store::upload_server)"""
    base = (os.environ.get("FORGEJO_UPLOAD_API") or "").strip().rstrip("/")
    return base[:-len("/api/v1")] if base.endswith("/api/v1") else SERVER


def sha256_file(path):
    h, n = hashlib.sha256(), 0
    with open(path, "rb") as f:
        while True:
            b = f.read(1 << 20)
            if not b:
                return h.hexdigest(), n
            h.update(b)
            n += len(b)


def listed(t):
    """(size, sha256) of the line's copy as the package API lists it (anonymous, the server's own address), None when it
    is not there; a list that cannot be read stops"""
    url = "%s/api/v1/packages/%s/generic/%s-%s/%s/files" % (SERVER, OWNER, urllib.parse.quote(t["name"]), t["system"],
                                                            urllib.parse.quote(t["version"]))
    req = urllib.request.Request(url, headers={"User-Agent": AGENT, "Accept": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            files = json.load(r)
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return None
        fail("%s: HTTP %d" % (url, e.code))
    except (urllib.error.URLError, OSError, ValueError) as e:
        fail("%s: %s" % (url, e))
    want = "%s-%s%s" % (t["sha256"], t["name"], EXT)
    for f in files if isinstance(files, list) else ():
        if isinstance(f, dict) and f.get("name") == want:
            return f.get("Size", f.get("size")), f.get("sha256")
    return None


def put(path, t, token):
    to = upload_server() + store_path(t)
    shown = SERVER + store_path(t)
    with open(path, "rb") as body:
        req = urllib.request.Request(to, data=body, method="PUT", headers={
            "User-Agent": AGENT, "Authorization": "token " + token, "Content-Type": "application/zstd",
            "Content-Length": str(t["size"])})
        try:
            with urllib.request.urlopen(req, timeout=1800) as r:
                r.read()
            print("uploaded %s" % shown, flush=True)
            return
        except urllib.error.HTTPError as e:
            if e.code != 409:
                fail("PUT %s: HTTP %d %s" % (shown, e.code, e.read().decode("utf-8", "replace")[:300]))
        except (urllib.error.URLError, OSError) as e:
            fail("PUT %s: %s" % (shown, e))
    # (409: the store has a file of that name already; kept when it is these bytes)
    got = listed(t)
    if got != (t["size"], t["sha256"]):
        fail("%s is in the package registry already with other bytes (%s): a new version, then" % (shown, got))
    print("%s is in the package registry already" % shown, flush=True)


def line(t):
    """The tools.lock line of a tool"""
    return "tool %s %s %s %s %d tools/%s - %s %s" % (t["name"], t["version"], t["system"], t["sha256"], t["size"],
                                                   t["name"], CDN + store_path(t), SERVER + store_path(t))


def made_tools(made, tools):
    """[(archive path, tool)] of <made>/<system>/<name>.tar.zst and its .version, each checked against the lock"""
    out = []
    for system in sorted(os.listdir(made)):
        if not os.path.isdir(os.path.join(made, system)):
            continue   # (make.sh's add.sh)
        for f in sorted(os.listdir(os.path.join(made, system))):
            path = os.path.join(made, system, f)
            if f.endswith(".version"):
                continue
            if system not in SYSTEMS or not f.endswith(EXT) or not os.path.isfile(path) or os.path.islink(path):
                fail("%s: not <system>/<name>%s" % (path, EXT))
            name = f[:-len(EXT)]
            try:
                with open(path[:-len(EXT)] + ".version", encoding="utf-8") as v:
                    version = v.read().strip()
            except OSError:
                fail("%s: no %s.version beside it (builder/tools/make.sh writes it)" % (path, name))
            if not (NAME.match(name) and VERSION.match(version)):
                fail("%s: not a name and version a tools.lock line takes (%r %r)" % (path, name, version))
            sha, size = sha256_file(path)
            t = {"name": name, "version": version, "system": system, "sha256": sha, "size": size}
            for old in tools:
                if (old["name"], old["system"], old["version"]) == (name, system, version) and \
                        (old["sha256"], old["size"]) != (sha, size):
                    fail("%s (%s) %s: made %d bytes, SHA-256 %s; tools.lock names that version as %d bytes, %s. A package "
                         "version's files never change: a new version (its *_TRIM in builder/tools/pins.sh), or the "
                         "recipe's output made as before" % (name, system, version, size, sha, old["size"], old["sha256"]))
            out.append((path, t))
    if not out:
        fail("%s holds no archive" % made)
    return out


def write_lock(path, src, lines):
    """src's text with each tool's line in place of its tool's (name and system), the new ones after the last line"""
    with open(src, encoding="utf-8") as f:
        text = f.read().splitlines()
    todo = {(l.split()[1], l.split()[3]): l for l in lines}
    out = []
    for old in text:
        w = old.split()
        key = (w[1], w[3]) if len(w) > 3 and w[0] == "tool" else None
        out.append(todo.pop(key) if key in todo else old)
    out += list(todo.values())
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(out) + "\n")


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("made", help="the folder of <system>/<name>.tar.zst and .version, the make job's artifact")
    p.add_argument("--lock", default="tools.lock")
    p.add_argument("--write-lock", help="the lock with the made tools' lines written there")
    a = p.parse_args(argv)
    made = made_tools(a.made, read_lock(a.lock))
    token = os.environ.get("RELEASE_TOKEN") or ""
    if not token:
        fail("RELEASE_TOKEN is needed (a token with write access to %s's packages)" % OWNER)
    for path, t in made:
        put(path, t, token)
    missing = [t for _, t in made if listed(t) != (t["size"], t["sha256"])]
    for t in missing:
        print("::error::%s (%s): its copy is not in the store, or not the archive made" % (t["name"], t["system"]))
    if missing:
        return 1
    lines = [line(t) for _, t in made]
    print("every made archive is in the store; its tools.lock lines:", flush=True)
    for l in lines:
        print(l)
    if a.write_lock:
        write_lock(a.write_lock, a.lock, lines)
    return 0


if __name__ == "__main__":
    sys.exit(main())
