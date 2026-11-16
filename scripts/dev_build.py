"""Development builds of this checkout on a PC several agents share: the build environment set up once and the same for every
checkout (a player's build tools from tools.lock, the shared object and translation stores, nothing that changes the lift key
by accident), one shared base per commit of main, and every step through the PC's job queue (scripts/job_queue.py).
Linux. Python's standard library only. docs/development.md, *Sharing a PC: the job queue* and *Testing a change against a
base*.

    python -I scripts/dev_build.py env [--json]          the build environment, as shell exports (or JSON)
    python -I scripts/dev_build.py base [REV]            the shared base of REV (default: where this checkout left main),
                                                         built when no one has: one per commit, for every checkout
    python -I scripts/dev_build.py pack [NAME] [--base REV] [--prepare | --no-prepare]
                                                         this checkout as a pack over its base (NAME: the checkout's folder)
    python -I scripts/dev_build.py rt [NAME ...] [--base REV] [--out DIR] [--empty]
                                                         a game folder to run: the base with the packs (default this
                                                         checkout's pack; --empty: none, the base alone, for A/B numbers)
    python -I scripts/dev_build.py build [--out DIR] [--quick] [--prepare] [--timing] [--fsr]
                                                         a whole game folder from this checkout (make_game.py)
    python -I scripts/dev_build.py vk [--copy-to DIR]    the renderer alone (longshot_vk.so), symbols checked
    python -I scripts/dev_build.py ls                    the bases, their packs and when each was last used

Where things go. Nothing on a RAM file system (tmpfs: /tmp on many systems): a build reads and writes tens of gigabytes, and a
game folder there fills the memory. This script refuses one.
  - this checkout's own: build-linux/ (git-ignored), so they go with the checkout: dev-work/ (the work folder of `build`, kept
    between builds: the next compiles only what changed), game/ (`build`), vk-work/ (`vk`), rt-<name>/ (`rt`)
  - shared: LONGSHOT_DEV_HOME (~/.cache/longshot-dev): bases/<commit>/ (dev_packs.py base folders: a base is built once per
    commit, whoever asks first, and the next asks wait for it), builder/ (the build tools, per tools.lock); the object store
    LONGSHOT_OBJ_STORE (~/.cache/longshot-objects) and the translation store LONGSHOT_LIFT_STORE (~/.cache/longshot-lifts),
    as dev_packs.py's, on the same drive as the bases so objects are hard links

The environment (`env`):
  - Python: LONGSHOT_DEV_PYTHON, else build-linux/deps/python of this checkout or of one beside it (scripts/linux/deps.sh
    makes it): one with the build's packages (requirements.txt)
  - the build tools: what a player's build uses, tools.lock's linux-x64 lines, installed once per tools.lock under
    LONGSHOT_DEV_HOME/builder by builder/linux/linux_tools.py (downloads on first use). LONGSHOT_DEV_DEPS=<folder> uses an
    older build-linux/deps folder instead (EMSDK, RX_CPM, LONGSHOT_FFMPEG, LONGSHOT_SDL2 under it)
  - DLSS: the launcher's NGX SDK when LONGSHOT_NGX_SDK is unset and the launcher has one
  - "env" in ~/.config/longshot/dev_build.json, then the environment you run it in; "romfs", "exefs", "python" and "deps"
    there too. The game's unpacked files: LONGSHOT_DEV_ROMFS / LONGSHOT_DEV_EXEFS, else the newest game folder in ~/Games
    with romfs/ and .longshot_exefs/ (never read from anywhere else, never copied off the PC)
  - make_game.py lift_key hashes every A64_, OOT_ and RX_ variable but four compile-only ones: one set by hand gives the
    build its own translation (no other build's from the store). This says which it saw, and drops OOT_STATUS_* (an older
    queue's, naming the job) and the queue's own LONGSHOT_JOB*.
"""
import argparse
import hashlib
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
import job_queue  # noqa: E402  (same folder; python -I does not add it to the path)

COMPILE_ONLY_ENV = ("RX_HOT_OPT", "RX_PGO", "RX_VK_PGO", "RX_VK_PGO_FILE")   # (make_game.py's: left out of the lift key)
PY_PACKAGES = ("capstone", "unicorn", "zstandard", "numpy")
HOT_QUICK = {"RX_HOT_OPT": "O1"}   # (docs/development.md, A faster build for development: compile-only, never in the lift key)


def die(text):
    sys.exit("dev_build: " + text)


def say(text):
    print("dev_build: " + text, flush=True)


def home():
    return os.environ.get("LONGSHOT_DEV_HOME") or os.path.join(os.environ.get("XDG_CACHE_HOME") or os.path.expanduser("~/.cache"),
                                                               "longshot-dev")


def cache(name):
    return os.path.join(os.environ.get("XDG_CACHE_HOME") or os.path.expanduser("~/.cache"), name)


def config():
    p = os.path.join(os.path.dirname(job_queue.config_path()), "dev_build.json")
    try:
        with open(p, encoding="utf-8") as f:
            c = json.load(f)
        return c if isinstance(c, dict) else {}
    except (OSError, ValueError):
        return {}


def not_on_ram(path, what):
    if job_queue.on_ram(os.path.dirname(os.path.abspath(path)) if not os.path.exists(path) else path):
        die("%s %s is on a RAM file system (tmpfs): a build or a game folder there fills the memory. Put it on a disk" % (what, path))


# ---- the checkout ----

def jj_repo(root=ROOT):
    d = root
    while True:
        if os.path.isdir(os.path.join(d, ".jj")):
            return True
        up = os.path.dirname(d)
        if up == d:
            return False
        d = up


def out(cmd, cwd=ROOT, check=True):
    r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, encoding="utf-8", errors="replace")
    if check and r.returncode:
        die("%s failed: %s" % (" ".join(cmd[:3]), (r.stderr or r.stdout).strip()[-400:]))
    return r.stdout.strip()


def resolve(rev):
    """A revision (jj or git) as its full commit id"""
    if jj_repo():
        return out(["jj", "log", "--no-graph", "-r", rev, "-T", 'commit_id ++ "\\n"', "--limit", "1"]).splitlines()[0]
    return out(["git", "rev-parse", "--verify", rev + "^{commit}"])


def fork_point():
    """Where this checkout left main: the newest commit of main in its history"""
    if jj_repo():
        return resolve("heads(::@ & ::trunk())")
    for main in ("forge/main", "origin/main", "upstream/main", "main"):
        r = subprocess.run(["git", "merge-base", "HEAD", main], cwd=ROOT, capture_output=True, text=True)
        if r.returncode == 0 and r.stdout.strip():
            return r.stdout.strip()
    die("no main to start from (fetch it, or name a revision)")


def is_ancestor(commit):
    return subprocess.run(["git", "merge-base", "--is-ancestor", commit, "HEAD"], cwd=ROOT,
                          capture_output=True).returncode == 0


def distance(commit):
    try:
        return int(out(["git", "rev-list", "--count", "%s..HEAD" % commit]))
    except ValueError:
        return 1 << 30


# ---- the environment ----

def find_python(c):
    """A Python with the build's packages: checked once, the first that has them"""
    tried = []
    named = os.environ.get("LONGSHOT_DEV_PYTHON") or c.get("python")
    cands = [named] if named else []
    cands.append(os.path.join(ROOT, "build-linux", "deps", "python", "bin", "python3"))
    parent = os.path.dirname(ROOT)
    try:
        for n in sorted(os.listdir(parent)):
            cands.append(os.path.join(parent, n, "build-linux", "deps", "python", "bin", "python3"))
    except OSError:
        pass
    for p in cands:
        if not p or not os.path.isfile(p) or p in tried:
            continue
        tried.append(p)
        if subprocess.run([p, "-I", "-c", "import " + ", ".join(PY_PACKAGES)], capture_output=True).returncode == 0:
            return p
        if named and p == named:
            die("%s lacks the build's packages (%s)" % (p, ", ".join(PY_PACKAGES)))
    die("no Python with the build's packages (%s): run scripts/linux/deps.sh once, or set LONGSHOT_DEV_PYTHON" %
        ", ".join(PY_PACKAGES))


def tools_lock_key():
    with open(os.path.join(ROOT, "tools.lock"), encoding="utf-8") as f:
        lines = [l for l in f if l.startswith("tool ") and " linux-x64 " in l]
    return hashlib.sha1("".join(lines).encode()).hexdigest()[:12]


def toolchain(py, c):
    """{variable: path} of the build tools"""
    deps = os.environ.get("LONGSHOT_DEV_DEPS") or c.get("deps")
    if deps:
        deps = os.path.realpath(os.path.expanduser(deps))
        return {"EMSDK": os.path.join(deps, "emsdk"), "RX_CPM": os.path.join(deps, "cpm"),
                "LONGSHOT_FFMPEG": os.path.join(deps, "ffmpeg"), "LONGSHOT_SDL2": os.path.join(deps, "sdl2", "libSDL2-2.0.so.0")}
    # (one install per tools.lock: two checkouts on other tools.lock never unpack over each other's tools mid-build)
    b = os.path.join(home(), "builder", tools_lock_key())
    r = subprocess.run([py, "-I", os.path.join(ROOT, "builder", "linux", "linux_tools.py"), "env", "--install"], cwd=ROOT,
                       env=dict(os.environ, LONGSHOT_BUILDER_HOME=b), stdout=subprocess.PIPE, text=True)
    if r.returncode:
        die("the build tools could not be installed (above); LONGSHOT_DEV_DEPS=<build-linux/deps folder> uses an older set")
    env = {}
    for line in r.stdout.splitlines():
        parts = shlex.split(line)
        if len(parts) == 2 and parts[0] == "export" and "=" in parts[1]:
            k, v = parts[1].split("=", 1)
            env[k] = v
    return env


def game_files(c):
    """(romfs, exefs) of the game, unpacked on this PC"""
    romfs = os.environ.get("LONGSHOT_DEV_ROMFS") or c.get("romfs")
    exefs = os.environ.get("LONGSHOT_DEV_EXEFS") or c.get("exefs")
    if romfs and exefs:
        return os.path.expanduser(romfs), os.path.expanduser(exefs)
    games = os.path.expanduser("~/Games")
    found = []
    for n in os.listdir(games) if os.path.isdir(games) else ():
        g = os.path.join(games, n)
        if os.path.isdir(os.path.join(g, "romfs")) and os.path.isdir(os.path.join(g, ".longshot_exefs")):
            found.append((os.path.getmtime(os.path.join(g, ".longshot_exefs")), g))
    if not found:
        die("no unpacked game here: set LONGSHOT_DEV_ROMFS and LONGSHOT_DEV_EXEFS (a built game folder's romfs/ and "
            ".longshot_exefs/), or \"romfs\" and \"exefs\" in ~/.config/longshot/dev_build.json")
    g = max(found)[1]
    return os.path.join(g, "romfs"), os.path.join(g, ".longshot_exefs")


def environment(quiet=False):
    """(python, env, notes): the build's environment"""
    c = config()
    py = find_python(c)
    env = {k: v for k, v in os.environ.items() if not k.startswith(("OOT_STATUS_", "LONGSHOT_JOB"))}
    env.update(toolchain(py, c))
    ngx = os.path.expanduser("~/.local/share/longshot/launcher/ngx-sdk")
    if "LONGSHOT_NGX_SDK" not in env and os.path.isfile(os.path.join(ngx, "lib", "libnvsdk_ngx.a")):
        env["LONGSHOT_NGX_SDK"] = ngx
    for k, v in (c.get("env") or {}).items():
        if k not in os.environ:
            env[k] = str(v)
    env.setdefault("LONGSHOT_OBJ_STORE", cache("longshot-objects"))
    env.setdefault("LONGSHOT_LIFT_STORE", cache("longshot-lifts"))
    env.setdefault("LONGSHOT_LIFT_STORE_KEEP", "6")   # (3 is one per checkout of main; agents build several branches at once)
    for k in ("LONGSHOT_OBJ_STORE", "LONGSHOT_LIFT_STORE"):
        if env[k] not in ("", "0"):
            not_on_ram(env[k], k)
    keyed = sorted(k for k in env if k.startswith(job_queue.LIFT_KEY_PREFIXES) and k not in COMPILE_ONLY_ENV and k != "RX_CPM")
    if keyed and not quiet:
        say("these enter make_game.py's lift key, so this build takes no other build's translation unless it had them "
            "too: %s" % ", ".join(keyed))
    return py, env


# ---- running a step ----

class BaseLock:
    """One job at a time on a base (dev_packs.py refuses a second; this waits for the first instead)"""

    def __init__(self, d):
        self.path = d.rstrip("/") + ".lock"

    def __enter__(self):
        import fcntl
        os.makedirs(os.path.dirname(self.path), exist_ok=True)
        self.f = open(self.path, "a")
        try:
            fcntl.flock(self.f, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            say("another job uses the base %s: waiting for it" % os.path.basename(self.path)[:-5])
            fcntl.flock(self.f, fcntl.LOCK_EX)
        return self

    def __exit__(self, *exc):
        self.f.close()


def queued(name, mode, cmd, env, cwd=ROOT):
    inside = os.environ.get("LONGSHOT_JOB") or os.environ.get("OOT_STATUS_LOCK")
    if inside:   # (run by a job already, of this queue or the older one: it holds what this needs, so no second ticket)
        say("inside job %s: %s runs directly" % (os.environ.get("LONGSHOT_JOB_NAME") or inside, name))
        if subprocess.call(cmd, cwd=cwd, env=env):
            die("%s failed" % name)
        return
    old = os.getcwd()
    os.chdir(cwd)
    try:
        rc = job_queue.run(name, mode, cmd, environ=env)
    finally:
        os.chdir(old)
    if rc:
        die("%s failed (exit status %d)" % (name, rc))


def short(commit):
    return commit[:12]


def bases():
    d = os.path.join(home(), "bases")
    out_ = []
    for n in sorted(os.listdir(d)) if os.path.isdir(d) else ():
        p = os.path.join(d, n)
        try:
            with open(os.path.join(p, "base.json"), encoding="utf-8") as f:
                b = json.load(f)
        except (OSError, ValueError):
            continue
        out_.append((p, b))
    return out_


def base_dir(commit):
    return os.path.join(home(), "bases", short(commit))


def base_for(rev):
    """The base folder for this checkout: REV's, or the nearest base in its history"""
    if rev:
        d = base_dir(resolve(rev))
        if not os.path.isfile(os.path.join(d, "base.json")):
            die("no base of %s yet: dev_build.py base %s" % (rev, rev))
        return d
    mine = [(distance(b["commit"]), p) for p, b in bases() if is_ancestor(b["commit"])]
    if not mine:
        die("no base in this checkout's history: dev_build.py base (it builds the one where this checkout left main)")
    return min(mine)[1]


def touch(d):
    try:
        os.utime(d)
    except OSError:
        pass


def pack_name(name):
    return re.sub(r"[^A-Za-z0-9._-]+", "-", name or os.path.basename(ROOT))


# ---- commands ----

def cmd_env(a):
    py, env = environment()
    shown = {k: env[k] for k in sorted(env) if k not in os.environ or os.environ[k] != env[k]}
    shown["PY"] = py
    if a.json:
        print(json.dumps(shown, indent=1))
    else:
        for k, v in shown.items():
            print("export %s=%s" % (k, shlex.quote(v)))
    return 0


def cmd_base(a):
    commit = resolve(a.rev) if a.rev else fork_point()
    d = base_dir(commit)
    not_on_ram(d, "the base")
    if os.path.isfile(os.path.join(d, "base.json")):
        say("the base of %s is there: %s" % (short(commit), d))
        touch(d)
        return 0
    py, env = environment()
    romfs, exefs = game_files(config())
    with BaseLock(d):
        if os.path.isfile(os.path.join(d, "base.json")):   # (built by whoever held the lock before)
            say("the base of %s is there: %s" % (short(commit), d))
            return 0
        src = d + ".src"
        shutil.rmtree(src, ignore_errors=True)
        # (a checkout of the commit alone: dev_packs.py base builds a tree with nothing uncommitted)
        if jj_repo():
            name = "base-" + short(commit)
            subprocess.run(["jj", "workspace", "forget", name], cwd=ROOT, capture_output=True)
            out(["jj", "workspace", "add", "--name", name, "-r", commit, src])
        else:
            out(["git", "worktree", "add", "--detach", src, commit])
        try:
            seed = [g for g in (os.path.join(p, "game") for p, _ in sorted(bases(), key=lambda x: -os.path.getmtime(x[0])))
                    if os.path.isdir(g)][:1]
            cmd = [py, "-I", os.path.join(src, "scripts", "dev_packs.py"), "base", d, "--romfs", romfs, "--exefs", exefs,
                   "--tree", src] + (["--seed", seed[0]] if seed else [])
            say("building the base of %s in %s%s" % (short(commit), d, " (shader caches from %s)" % seed[0] if seed else ""))
            queued("base-" + short(commit)[:8], "build", cmd, env, cwd=src)
        finally:
            if jj_repo():
                subprocess.run(["jj", "workspace", "forget", "base-" + short(commit)], cwd=ROOT, capture_output=True)
                shutil.rmtree(src, ignore_errors=True)
                subprocess.run(["git", "worktree", "prune"], cwd=ROOT, capture_output=True)
            else:
                subprocess.run(["git", "worktree", "remove", "--force", src], cwd=ROOT, capture_output=True)
    say("base of %s: %s" % (short(commit), d))
    return 0


def cmd_pack(a):
    d = base_for(a.base)
    name = pack_name(a.name)
    py, env = environment()
    cmd = [py, "-I", os.path.join(ROOT, "scripts", "dev_packs.py"), "pack", d, name, "--tree", ROOT]
    if a.prepare is not None:
        cmd.append("--prepare" if a.prepare else "--no-prepare")
    with BaseLock(d):
        touch(d)
        queued("pack-" + name, "build", cmd, env)
    say("pack %s on the base %s" % (name, d))
    return 0


def cmd_rt(a):
    d = base_for(a.base)
    packs = [] if a.empty else [pack_name(n) for n in (a.packs or [None])]
    rt = os.path.abspath(a.out or os.path.join(ROOT, "build-linux", "rt-" + ("base" if a.empty else "-".join(packs))))
    not_on_ram(rt, "the runtime")
    py, env = environment(quiet=True)
    cmd = [py, "-I", os.path.join(ROOT, "scripts", "dev_packs.py"), "rt", d, rt] + packs
    with BaseLock(d):
        touch(d)
        # (rt may prepare the shaders again, which takes the GPU: a pack that changed the translator or the shader seed)
        queued("rt-" + os.path.basename(rt), "shared", cmd, env)
    say("runtime: %s" % rt)
    return 0


def cmd_build(a):
    out_dir = os.path.abspath(a.out or os.path.join(ROOT, "build-linux", "game"))
    work = os.path.join(ROOT, "build-linux", "dev-work")
    for p, what in ((out_dir, "the game folder"), (work, "the work folder")):
        not_on_ram(p, what)
    if a.quick and (a.timing or a.prepare):
        die("--quick is for checks (its hot code is slower): never with --timing, and prepare in a normal build")
    py, env = environment()
    romfs, exefs = game_files(config())
    env["LONGSHOT_KEEP_BUILD_FILES"] = "1"   # (the next build here compiles only what changed)
    if not a.prepare:
        env.setdefault("LONGSHOT_NO_PREPARE", "1")
    if a.quick:
        env.update(HOT_QUICK)
    cmd = [py, "-I", "nx2/make_game.py", "--romfs", romfs, "--exefs", exefs, "--work", work, "--out", out_dir,
           "--language", a.language]
    if a.fsr or config().get("fsr"):   # (make_game.py decides FSR 4 by --fsr or the folder's mark, not the environment)
        cmd.append("--fsr")
    queued("build-" + pack_name(None), "timing-cpu" if a.timing else "build", cmd, env)
    say("game folder: %s%s" % (out_dir, " (shaders not prepared: its first start prepares them, or --prepare)" if not a.prepare else ""))
    return 0


def cmd_vk(a):
    work = os.path.join(ROOT, "build-linux", "vk-work")
    not_on_ram(work, "the work folder")
    py, env = environment()
    env.update(OOT_WORK=work, RX_VK_ONLY="1", RX_RELEASE="1")
    if "RX_VK_PGO" not in env:   # (the renderer's play profile, as make_game.py gives a game build: scripts/dev_packs.py renderer_profile)
        for n in ("longshot_vk.profdata", "longshot_vk.shipped.profdata"):
            p = os.path.join(ROOT, "build-linux", "dev-work", n)
            if os.path.isfile(p):
                env["RX_VK_PGO"] = p
                break
    so = os.path.join(work, "sa", "build", "longshot_vk.so")
    if os.path.exists(so):
        os.remove(so)
    queued("vk-" + pack_name(None), "light", [py, "-I", "nx2/build.py", "--standalone"], env)
    if not os.path.isfile(so):
        die("no %s after the build" % so)
    # (a renderer function left in an anonymous namespace is an undefined symbol: the game finds no renderer and opens no window)
    nm = subprocess.run("nm -D --undefined-only '%s' | c++filt | grep mkwvk" % so, shell=True, capture_output=True, text=True)
    bad = [l for l in nm.stdout.splitlines() if not l.lstrip().startswith("w ")]
    if bad:
        die("longshot_vk.so has undefined mkwvk symbols:\n" + "\n".join(bad))
    say("renderer: %s (symbols checked)" % so)
    if a.copy_to:
        dst_dir = os.path.join(a.copy_to, "bin") if os.path.isdir(os.path.join(a.copy_to, "bin")) else a.copy_to
        dst = os.path.join(dst_dir, "longshot_vk.so")
        shutil.copyfile(so, dst + ".new")
        os.replace(dst + ".new", dst)
        say("copied to %s: its game must come from the same main (MKWVK_API_VERSION), or no window opens" % dst)
    return 0


def cmd_ls(a):
    now = time.time()
    found = bases()
    if not found:
        print("no bases in %s" % os.path.join(home(), "bases"))
    for p, b in found:
        packs = sorted(os.listdir(os.path.join(p, "packs"))) if os.path.isdir(os.path.join(p, "packs")) else []
        print("%s  %s  used %s ago%s%s" % (short(b["commit"]), p, job_queue.ago(now - os.path.getmtime(p)),
                                         "  (not prepared: %s)" % b["not_prepared"] if b.get("not_prepared") else "",
                                         "  in this checkout's history" if is_ancestor(b["commit"]) else ""))
        for k in packs:
            print("    pack %s" % k)
    return 0


def main(argv):
    if os.name == "nt":
        die("Linux only for now: on Windows run scripts/dev_packs.py and nx2/make_game.py as docs/development.md says")
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = p.add_subparsers(dest="what", required=True)
    s = sub.add_parser("env")
    s.add_argument("--json", action="store_true")
    s = sub.add_parser("base")
    s.add_argument("rev", nargs="?")
    s = sub.add_parser("pack")
    s.add_argument("name", nargs="?")
    s.add_argument("--base")
    s.add_argument("--prepare", action="store_true", default=None)
    s.add_argument("--no-prepare", dest="prepare", action="store_false")
    s = sub.add_parser("rt")
    s.add_argument("packs", nargs="*")
    s.add_argument("--base")
    s.add_argument("--out")
    s.add_argument("--empty", action="store_true")
    s = sub.add_parser("build")
    s.add_argument("--out")
    s.add_argument("--quick", action="store_true")
    s.add_argument("--prepare", action="store_true")
    s.add_argument("--timing", action="store_true")
    s.add_argument("--language", default="en-US")
    s.add_argument("--fsr", action="store_true", help="with AMD FSR 4 (\"fsr\": true in dev_build.json: always)")
    s = sub.add_parser("vk")
    s.add_argument("--copy-to")
    sub.add_parser("ls")
    a = p.parse_args(argv)
    return {"env": cmd_env, "base": cmd_base, "pack": cmd_pack, "rt": cmd_rt, "build": cmd_build, "vk": cmd_vk,
            "ls": cmd_ls}[a.what](a)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
