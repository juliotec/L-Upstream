"""This checkout's development builds on a PC that several agents share: one shared base per commit of main, this checkout
as a pack over it, a runtime to play, every step through the job queue (jobqueue.py) in the build environment a player's
build has: longshot-builder (docs/dev/reference/builder.md) and tools.lock's build tools. Linux and Windows; Python's
standard library only.
docs/dev/developing.md, *Test a change against a base*.

    dev.py base [REV]           the shared base of REV (default: where this checkout left main), built once by whoever
                                asks first (`dev.py packs base`, run by a checkout of that commit)
    dev.py pack                 this checkout as a pack over the nearest base in its history (`dev.py packs pack`)
    dev.py rt [--empty]         build-linux/rt-<checkout>/, a game folder to run: the base with that pack (--empty: none,
                                build-linux/rt-base/, the other half of an A/B)
    dev.py game [--quick | --timing] [--fsr]
                                a whole game folder, build-linux/game/ (--quick: -O1 hot units, never measured;
                                --timing: a measured build)
    dev.py vk                   the renderer alone, its exports checked
    dev.py env                  the build environment as shell lines, for builds of your own
    dev.py bases                the shared bases, their packs and when each was last used

(build-windows\\ instead of build-linux/ on Windows; both git-ignored.) Nothing goes on a RAM file system (tmpfs): a build
there fills the memory, so this refuses one.

The environment:
  - longshot-builder: the checkout's own, compiled with cargo (mise.toml pins Rust) into the development folder's cargo/
    target folder (`dev.py packs run`); LONGSHOT_BUILD names a program to use instead
  - the build tools a player's build uses: tools.lock's lines for this system, installed once per tools.lock into the
    development folder's tools-store/ (`longshot-builder dev tools install`), shared by every checkout, and named to longshot-builder
    by their variables (TOOL_ENV) with LONGSHOT_OWN_TOOLS=1 (a checkout carries no tools/ of its own: build-driver's
    tools.rs). A variable you set yourself wins (RX_CPM=<a CPM cache> ...)
  - DLSS: when LONGSHOT_NGX_SDK is unset, the development folder's sdk/ngx-<linux|windows> (scripts/fetch.py dlss); with
    an SDK, the builds have DLSS (--upscaler dlss)
  - the development folder's config/dev_build.json: "env" (variables for every build), "game", "fsr" (always FSR 4). The
    game: LONGSHOT_DEV_GAME, else "game", else the newest .nup in ~/Games (Windows also ~/Games/Longshot), a folder or
    two down; a .nup or a dump folder with romfs/ and exefs/
  - the stores every build shares: the development folder's store/ (LONGSHOT_STORE moves it)
"""
import contextlib
import hashlib
import os
import re
import shlex
import shutil
import subprocess
import sys
import time

from . import common, jobqueue, packs, vcs

ROOT = common.ROOT
WHO = "dev.py"
HOT_QUICK = {"RX_HOT_OPT": "O1"}   # (docs/dev/developing.md, Build quickly: the hot units' level, in their objects' keys)
# (each tool's variable, build-driver's tools/mod.rs TOOLS: the folder longshot-builder takes it from, with LONGSHOT_OWN_TOOLS=1)
TOOL_ENV = {"windows-x64": {"llvm-mingw": "LONGSHOT_MINGW", "cpm": "RX_CPM", "sdl": "LONGSHOT_SDL_DIR"},
            "linux-x64": {"llvm": "LONGSHOT_LLVM", "sysroot": "LONGSHOT_SYSROOT", "ffmpeg": "LONGSHOT_FFMPEG",
                          "sdl2": "LONGSHOT_SDL2", "cpm": "RX_CPM"}}
# (what host/rx_backend.c asks the renderer's DLL for: without mkwvk_init the game draws nothing)
RENDERER_EXPORTS = ("mkwvk_init",)
CONFIG = "dev_build.json"   # (in the development folder's config/, beside the job queue's job_queue.json)


def die(text):
    common.die(WHO, text)


def say(text):
    common.say(WHO, text)


def system():
    """tools.lock's name for this system"""
    return "windows-x64" if common.WINDOWS else "linux-x64"


def own_dir():
    """This checkout's own build files (git-ignored)"""
    return os.path.join(ROOT, "build-windows" if common.WINDOWS else "build-linux")


def renderer_name():
    return "longshot_vk.dll" if common.WINDOWS else "longshot_vk.so"


def config():
    return common.read_config(CONFIG)


def not_on_ram(path, what):
    if common.on_ram(path if os.path.exists(path) else os.path.dirname(os.path.abspath(path))):
        die("%s %s is on a RAM file system (tmpfs): a build or a game folder there fills the memory. Put it on a disk" % (what, path))


def touch(d):
    try:
        os.utime(d)   # (`dev.py bases` shows a base's time as its last use)
    except OSError:
        pass


# ---- the checkout ----

def jj_repo():
    return vcs.is_jj(ROOT)


def out(cmd, cwd=ROOT):
    return common.output(WHO, cmd, cwd)


def resolve(rev):
    """A revision (jj or git) as its full commit id"""
    try:
        return vcs.resolve(ROOT, rev)
    except subprocess.CalledProcessError as e:
        die("no commit %s: %s" % (rev, (e.stderr or e.stdout or "").strip()[-400:]))


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
    return vcs.is_ancestor(ROOT, commit)


def distance(commit):
    return vcs.distance(ROOT, commit)


# ---- the environment ----

def tools_lock_key(tree=ROOT):
    """tree's tools.lock, its lines for this system, as a short key: the install's folder name"""
    with open(os.path.join(tree, "tools.lock"), encoding="utf-8") as f:
        lines = [l for l in f if l.startswith("tool ") and " %s " % system() in l]
    return hashlib.sha1("".join(lines).encode()).hexdigest()[:12]


def builder_dir(tree=ROOT):
    # (one install per tools.lock, shared by every checkout: two on other tools.lock never unpack over each other's tools)
    return os.path.join(common.dev_path("tools-store"), tools_lock_key(tree))


def tool_dirs(tree=ROOT):
    """{tool: its folder}: this system's tools at their dest under the builder folder, tools/<name> (every line's, a rule
    `longshot-builder dev tools check` and build-driver's tools/lock.rs test keep)"""
    b = builder_dir(tree)
    return {name: os.path.join(b, "tools", name) for name in TOOL_ENV[system()]}


def toolchain(tree=ROOT):
    """tree's tools.lock's tools for this system (this checkout's, or the tree `dev.py packs run --tree` builds with),
    installed once (`longshot-builder dev tools install`, by this checkout's own builder: downloaded on first use,
    checked by size and SHA-256), as {variable: path}"""
    b = builder_dir(tree)
    with contextlib.redirect_stdout(sys.stderr):   # (on stderr: `env` prints the environment alone)
        exe = packs.builder(ROOT, os.path.join(own_dir(), "longshot-builder-tools" + (".exe" if common.WINDOWS else "")))
    with common.Lock(b + ".lock"):   # (two checkouts at once: the second waits, then finds them there)
        r = subprocess.run([exe, "dev", "tools", "--lock", os.path.join(tree, "tools.lock"), "install", b, "--system",
                            system(), "--cache", os.path.join(common.dev_path("tools-store"), "downloads")],
                           capture_output=True, text=True)
        for line in (r.stdout + r.stderr).splitlines():
            if not line.endswith("there already"):
                common.say(WHO, "build tools: " + line, err=True)
        if r.returncode:
            die("the build tools could not be installed into %s" % b)
    dirs, env = tool_dirs(tree), {"LONGSHOT_OWN_TOOLS": "1"}
    for name, var in TOOL_ENV[system()].items():
        if not os.path.isdir(dirs.get(name, "")):
            die("tools.lock has no %s for %s: its tool folder %s is not there" % (name, system(), dirs.get(name)))
        env[var] = dirs[name]
    return env


def game_roots():
    """The folders whose game folders may hold the game: ~/Games; on Windows also ~/Games/Longshot, the Longshot Launcher's
    builds folder (LauncherSettings.DefaultBuildsFolder)"""
    games = os.path.expanduser(os.path.join("~", "Games"))
    return [games, os.path.join(games, "Longshot")] if common.WINDOWS else [games]


def game_source(c):
    """The game, on this PC: a .nup, or a dump folder with romfs/ and exefs/ (longshot-builder's --game)"""
    named = os.environ.get("LONGSHOT_DEV_GAME") or c.get("game")
    if named:
        g = os.path.expanduser(named)
        if not (os.path.isfile(g) or (os.path.isdir(os.path.join(g, "romfs")) and os.path.isdir(os.path.join(g, "exefs")))):
            die("the game %s is neither a .nup nor a folder with romfs/ and exefs/" % g)
        return g
    found = []
    for games in game_roots():
        for top, dirs, files in os.walk(games) if os.path.isdir(games) else ():
            if top.count(os.sep) - games.count(os.sep) >= 2:   # (~/Games/<name>/<file>, ~/Games/<a>/<b>/<file> at most)
                dirs[:] = []
            found += [(os.path.getmtime(os.path.join(top, f)), os.path.join(top, f))
                      for f in files if f.lower().endswith(".nup")]
    if not found:
        die("no .nup here: set LONGSHOT_DEV_GAME (a .nup, or a dump folder with romfs/ and exefs/), or \"game\" in %s" %
            common.config_file(CONFIG))
    return max(found)[1]


def ngx_sdk():
    """(folder, the library a build links) of the development folder's NGX SDK, where scripts/fetch.py dlss puts it"""
    if common.WINDOWS:
        return common.dev_path("sdk/ngx-windows"), "nvsdk_ngx_s.lib"
    return common.dev_path("sdk/ngx-linux"), "libnvsdk_ngx.a"


def environment(quiet=False, tree=ROOT):
    """The build's environment: this one less the queue's variables, with the build tools' of tree's tools.lock (yours
    win), the NGX SDK, dev_build.json's "env" and the stores"""
    c = config()
    env = jobqueue.without_queue_vars(os.environ)
    for k, v in toolchain(tree).items():
        env.setdefault(k, v)
    ngx, ngx_lib = ngx_sdk()
    if "LONGSHOT_NGX_SDK" not in env and os.path.isfile(os.path.join(ngx, "lib", ngx_lib)):
        env["LONGSHOT_NGX_SDK"] = ngx
    elif "LONGSHOT_NGX_SDK" not in env and not quiet:
        common.say(WHO, "no NVIDIA DLSS in this build: python -I scripts/fetch.py dlss puts its SDK in %s" % ngx, err=True)
    for k, v in (c.get("env") or {}).items():
        if k not in os.environ:
            env[k] = str(v)
    env.setdefault("LONGSHOT_STORE", packs.store())
    not_on_ram(env["LONGSHOT_STORE"], "the store")
    # (longshot-builder's per-user cache, its runtimes: the development folder's, never the player's
    # ~/.cache/longshot-builder or %LOCALAPPDATA%\Longshot\Builder, that the launcher's builds use)
    env.setdefault("LONGSHOT_CACHE", common.dev_path("builder-cache"))
    return env


def upscalers(env, fsr=False):
    """longshot-builder's --upscaler for a build in env: DLSS with an NGX SDK, FSR 4 when asked (--fsr, "fsr" in
    dev_build.json)"""
    names = (["dlss"] if env.get("LONGSHOT_NGX_SDK") else []) + (["fsr4"] if fsr or config().get("fsr") else [])
    return ",".join(names) or "none"


def shell_line(k, v):
    """An assignment for this system's shell: POSIX sh's export, PowerShell's $env: on Windows"""
    if common.WINDOWS:
        return "$env:%s = '%s'" % (k, v.replace("'", "''"))
    return "export %s=%s" % (k, shlex.quote(v))


# ---- the shared bases ----

def short(commit):
    return commit[:12]


def bases():
    """[(folder, its base.json)] of the shared bases"""
    d = common.dev_path("bases")
    found = []
    for n in sorted(os.listdir(d)) if os.path.isdir(d) else ():
        try:
            found.append((os.path.join(d, n), packs.read_base(os.path.join(d, n))))
        except (OSError, ValueError):
            continue
    return found


def base_dir(commit):
    return os.path.join(common.dev_path("bases"), short(commit))


def base_for():
    """The nearest base in this checkout's history (made by longshot-builder: an older dev.py's base has no game_source)"""
    mine = [(distance(b["commit"]), p) for p, b in bases() if "game_source" in b and is_ancestor(b["commit"])]
    if not mine:
        die("no base in this checkout's history: dev.py base (it builds the one where this checkout left main)")
    return min(mine)[1]


def base_lock(d):
    """One job at a time on a shared base: <base>.lock, waited for (`dev.py packs` refuses a second job on a base, by its
    dev_packs.busy; this waits for the first instead)"""
    d = d.rstrip("/" + os.sep)
    return common.Lock(d + ".lock", waiting="%s: another job uses the base %s: waiting for it" % (WHO, os.path.basename(d)))


def packs_cmd(tree, *args):
    """`dev.py packs` of a checkout"""
    return [sys.executable, "-I", os.path.join(tree, "scripts", "dev.py"), "packs"] + list(args)


def queued(name, mode, cmd, env, cwd=ROOT):
    """cmd through the job queue (directly when this runs inside a job already); a failure stops"""
    rc = jobqueue.run(name, mode, cmd, environ=env, cwd=cwd)
    if rc:
        die("%s failed (exit status %d)" % (name, rc))


def on_base(d, name, mode, cmd, env):
    """A `dev.py packs` step on the shared base d through the queue, holding the base's lock"""
    with base_lock(d):
        touch(d)
        queued(name, mode, cmd, env)


def pack_name():
    """This checkout's pack: its folder's name"""
    return re.sub(r"[^A-Za-z0-9._-]+", "-", os.path.basename(ROOT))


# ---- commands ----

def cmd_base(a):
    commit = resolve(a.rev) if a.rev else fork_point()
    d = base_dir(commit)
    not_on_ram(d, "the base")
    if os.path.isfile(os.path.join(d, "base.json")):
        say("the base of %s is there: %s" % (short(commit), d))
        touch(d)
        return 0
    env = environment()
    game = game_source(config())
    with base_lock(d):
        if os.path.isfile(os.path.join(d, "base.json")):   # (built by whoever held the lock before)
            say("the base of %s is there: %s" % (short(commit), d))
            return 0
        # (a checkout of the commit alone, built by its own `dev.py packs base`: a base has nothing uncommitted)
        src, ws = d + ".src", "base-" + short(commit)
        shutil.rmtree(src, ignore_errors=True)
        if jj_repo():
            subprocess.run(["jj", "workspace", "forget", ws], cwd=ROOT, capture_output=True)
            out(["jj", "workspace", "add", "--name", ws, "-r", commit, src])
        else:
            out(["git", "worktree", "add", "--detach", src, commit])
        try:
            # (the shader caches of the newest base: the preparation then redoes only what changed)
            seed = [g for g in (os.path.join(p, "game") for p, _ in sorted(bases(), key=lambda x: -os.path.getmtime(x[0])))
                    if os.path.isdir(g)][:1]
            cmd = packs_cmd(src, "base", d, "--game", game, "--tree", src, "--upscaler", upscalers(env)) + \
                (["--seed", seed[0]] if seed else [])
            say("building the base of %s in %s%s" % (short(commit), d, " (shader caches from %s)" % seed[0] if seed else ""))
            queued("base-" + short(commit)[:8], "build", cmd, env, cwd=src)
        finally:
            if jj_repo():
                subprocess.run(["jj", "workspace", "forget", ws], cwd=ROOT, capture_output=True)
                shutil.rmtree(src, ignore_errors=True)
                subprocess.run(["git", "worktree", "prune"], cwd=ROOT, capture_output=True)
            else:
                subprocess.run(["git", "worktree", "remove", "--force", src], cwd=ROOT, capture_output=True)
    say("base of %s: %s" % (short(commit), d))
    return 0


def cmd_pack(a):
    d, name = base_for(), pack_name()
    env = environment()
    on_base(d, "pack-" + name, "build", packs_cmd(ROOT, "pack", d, name, "--tree", ROOT), env)
    say("pack %s on the base %s" % (name, d))
    return 0


def cmd_rt(a):
    d = base_for()
    pack = [] if a.empty else [pack_name()]
    rt = os.path.join(own_dir(), "rt-" + (pack[0] if pack else "base"))
    not_on_ram(rt, "the runtime")
    env = environment(quiet=True)
    # (shared: rt may prepare the shaders again, which takes the GPU, when a pack changed the translator or the shader seed)
    on_base(d, "rt-" + os.path.basename(rt), "shared", packs_cmd(ROOT, "rt", d, rt, *pack), env)
    say("runtime: %s" % rt)
    return 0


def cmd_game(a):
    out_dir, work = os.path.join(own_dir(), "game"), os.path.join(own_dir(), "dev-work")
    not_on_ram(out_dir, "the game folder")
    not_on_ram(work, "the work folder")
    env = environment()
    if a.quick:
        env.update(HOT_QUICK)
    # (its first start prepares the shaders; LONGSHOT_NO_PREPARE=0 prepares them now)
    now = env.get("LONGSHOT_NO_PREPARE", "1") == "0"
    cmd = packs_cmd(ROOT, "run", "--") + ["build", "--game", game_source(config()), "--out", out_dir, "--work", work,
                                          "--language", "en-US", "--upscaler", upscalers(env, a.fsr)]
    queued("build-" + pack_name(), "timing-cpu" if a.timing else "build", cmd + ([] if now else ["--no-prepare-shaders"]), env)
    say("game folder: %s" % out_dir)
    return 0


def cmd_vk(a):
    work = os.path.join(own_dir(), "vk-work")
    not_on_ram(work, "the work folder")
    env = environment()
    env["RX_RELEASE"] = "1"
    so = os.path.join(work, "build", renderer_name())
    if os.path.exists(so):
        os.remove(so)
    cmd = packs_cmd(ROOT, "run", "--") + ["compile", "--only", "renderer", "--work", work, "--upscaler", upscalers(env)]
    queued("vk-" + pack_name(), "light", cmd, env)
    if not os.path.isfile(so):
        die("no %s after the build" % so)
    if common.WINDOWS:
        check_dll_exports(so, env)
    else:
        # (a renderer function left in an anonymous namespace is an undefined symbol: the game finds no renderer, opens no window)
        nm = subprocess.run("nm -D --undefined-only '%s' | c++filt | grep mkwvk" % so, shell=True, capture_output=True, text=True)
        bad = [l for l in nm.stdout.splitlines() if not l.lstrip().startswith("w ")]
        if bad:
            die("longshot_vk.so has undefined mkwvk symbols:\n" + "\n".join(bad))
    say("renderer: %s (symbols checked)" % so)
    return 0


def dll_exports(text):
    """The names in llvm-objdump -p's export table (its lines "  <ordinal> 0x<RVA> <name>" after "Export Table:")"""
    names, inside = set(), False
    for line in text.splitlines():
        if line.startswith("Export Table:"):
            inside = True
        elif inside and line.strip() and not line.startswith((" ", "\t")):
            inside = False
        elif inside:
            w = line.split()
            if len(w) >= 3 and w[0].isdigit() and w[1].lower().startswith("0x"):
                names.add(w[-1])
    return names


def check_dll_exports(dll, env):
    """(Windows) the DLL exports the renderer's entry points. An undefined symbol fails the link there, so nm's check has
    nothing to find; an entry point without __declspec(dllexport) is a DLL the game finds no renderer in"""
    mingw = env.get("LONGSHOT_MINGW", "")
    objdump = os.path.join(mingw, "bin", "llvm-objdump.exe") if mingw else ""
    objdump = objdump if os.path.isfile(objdump) else shutil.which("llvm-objdump")
    if not objdump:
        die("no llvm-objdump to check %s's exports with (LONGSHOT_MINGW's bin)" % dll)
    r = subprocess.run([objdump, "-p", dll], capture_output=True, text=True, encoding="utf-8", errors="replace")
    missing = [n for n in RENDERER_EXPORTS if n not in dll_exports(r.stdout)]
    if r.returncode or missing:
        die("%s does not export %s: the game would find no renderer%s" % (
            os.path.basename(dll), ", ".join(missing or RENDERER_EXPORTS),
            " (%s)" % r.stderr.strip()[-300:] if r.returncode else ""))


def cmd_env(a):
    env = environment()
    for k in sorted(env):
        if os.environ.get(k) != env[k]:
            print(shell_line(k, env[k]))
    return 0


def prunable():
    """[(path, what)]: what nothing reads any more: the bases an older dev.py made (no game_source; base_for skips them),
    and common.old_data()'s folders"""
    old = [(p, "a base of an older dev.py (%s), never used now" % short(b.get("commit", "?")))
           for p, b in bases() if "game_source" not in b]
    return old + common.old_data()


def cmd_prune(a):
    found = prunable()
    if not found:
        print("nothing to prune")
        return 0
    total = 0
    for p, what in found:
        n = common.tree_size(p)
        total += n
        print("%s  %.1f GB  %s" % (p, n / common.GB, what))
    if not a.yes:
        print("%.1f GB in all: `dev.py bases --prune --yes` removes them" % (total / common.GB))
        return 0
    s = jobqueue.settings()
    qdir = jobqueue.queue_dir(s)
    with common.Lock(os.path.join(qdir, ".lock"), waiting="dev.py bases: waiting for the job queue's lock"):
        jobs = jobqueue.read_tickets(qdir, clean=False)
        if jobs:
            die("the job queue has %d job(s): prune when `dev.py queue status` shows none" % len(jobs))
        for p, _ in found:
            if os.path.dirname(p) == common.dev_path("bases"):
                try:
                    with common.Lock(p + ".lock", wait=False):
                        shutil.rmtree(p)
                except BlockingIOError:
                    say("%s is in use: left" % p)
                    continue
                try:
                    os.remove(p + ".lock")
                except OSError:
                    pass
            else:
                shutil.rmtree(p)
            say("removed %s" % p)
    return 0


def cmd_bases(a):
    if a.prune:
        return cmd_prune(a)
    now = time.time()
    found = bases()
    if not found:
        print("no bases in %s" % common.dev_path("bases"))
    for p, b in found:
        print("%s  %s  used %s ago%s%s%s" % (short(b["commit"]), p, common.ago(now - os.path.getmtime(p)),
                                           "  (not prepared: %s)" % b["not_prepared"] if b.get("not_prepared") else "",
                                           "  (an older dev.py's: not used)" if "game_source" not in b else "",
                                           "  in this checkout's history" if is_ancestor(b["commit"]) else ""))
        kept = os.path.join(p, "packs")
        for k in sorted(os.listdir(kept)) if os.path.isdir(kept) else ():
            print("    pack %s" % k)
    return 0


def add_commands(sub):
    """dev.py's build commands: base, pack, rt, game, vk, env, bases"""
    def add(name, func, text):
        p = sub.add_parser(name, help=text, description=text)
        p.set_defaults(func=func)
        return p
    s = add("base", cmd_base, "the shared base of REV, built when no one has")
    s.add_argument("rev", nargs="?", help="a revision of main (default: where this checkout left main)")
    add("pack", cmd_pack, "this checkout as a pack over the nearest base in its history")
    add("rt", cmd_rt, "a game folder to run: the base with this checkout's pack").add_argument(
        "--empty", action="store_true", help="the base alone, the other half of an A/B")
    s = add("game", cmd_game, "a whole game folder from this checkout (longshot-builder)")
    speed = s.add_mutually_exclusive_group()
    speed.add_argument("--quick", action="store_true", help="-O1 hot units: faster to build, slower to run; never measure it")
    speed.add_argument("--timing", action="store_true", help="a measured build (the queue's timing-cpu mode)")
    s.add_argument("--fsr", action="store_true", help="with AMD FSR 4 (\"fsr\": true in dev_build.json: always)")
    add("vk", cmd_vk, "the renderer alone, its exports checked")
    add("env", cmd_env, "the build environment, as shell lines")
    s = add("bases", cmd_bases, "the shared bases, their packs and when each was last used")
    s.add_argument("--prune", action="store_true", help="list what nothing reads any more (an older dev.py's bases, builder/, "
                   "stores/, ~/.cache/longshot-objects and -lifts) and its size; with --yes, remove it")
    s.add_argument("--yes", action="store_true", help="with --prune: remove what it lists")
