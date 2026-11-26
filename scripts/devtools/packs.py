"""A development build as a base and small packs over it, in a base folder you name, without the job queue (`dev.py
packs`): build the game once, then build only what a change touches and run it from a copy of that base. For development
and A/B tests only: it runs longshot-builder (docs/dev/reference/builder.md) as players' builders do, built from the
checkout's own builder/ (docs/dev/developing.md, *Test a change against a base*). `dev.py base`, `pack` and `rt`
(build.py) run these on the PC's shared bases through the queue; the Test Farm runs them on its own.

    dev.py packs base DIR --game G [--tree T] [--seed GAME] [--upscaler LIST]
        Build the checkout T (default this one; a commit: no uncommitted or untracked sources) into DIR/game with
        longshot-builder, its work folder DIR/work. G is the game: a .nup (read in place: the game folder holds its romfs's
        index) or a dump folder with romfs/ and exefs/. --upscaler: longshot-builder's (dlss, fsr4, xess; with none given,
        the ones the environment names, as the Test Farm sets them: env_upscalers), which every pack of the base is built
        with too. --seed copies the caches of an earlier game folder first, as a player's update keeps them: they are
        stamped by the shader translator, so the preparation redoes only what changed.
    dev.py packs pack DIR NAME [--tree T]
        Build a change (a checkout whose history holds the base's commit; committed, uncommitted and untracked files all
        count) into DIR/packs/NAME: a change no build reads (docs, checks, developer tools: built_paths) builds nothing;
        anything else is longshot-builder's build of T in one work folder every pack of the base shares (DIR/pack-work), with
        the stores every base and pack on this PC share, so only the units and objects the change reaches are lifted and
        compiled. The pack keeps a module when an object it is linked from differs from the base's (module_changed), and
        every other file of the game folder that differs.
    dev.py packs rt DIR RUNTIME [PACK...]
        A game folder to run: the base's (a dump's romfs hard-linked; the rest copied, as clones where the file system has
        them: the game writes its caches), the packs' files over it, the shaders prepared again when a pack changed the
        translator (the translator_stamp longshot-builder wrote in its longshot_version.txt, not the base's), the program
        cache's format (kProgramCacheFormat) or the shader seed. A runtime this made before is
        refreshed in place: only the files that differ are replaced, the ones no longer wanted removed, and its caches,
        logs, saves and settings kept; its caches are dropped (the base's taken) only when its translator_stamp changed,
        not when the base merely moved.
    dev.py packs run [--tree T] -- ARGS...
        longshot-builder of the checkout T (default this one) with ARGS: its build, update, lift or compile with T as its
        --root and the PC's stores as its --store, in T's own work folder of the development folder's work/, unless ARGS
        name others, with the development folder's builder-cache/ as its cache, as a
        job of the PC's queue (what `dev.py game`, `vk` and `mise run build` run).
    dev.py packs warm DIR --run "CMD with {runtime}" [--run ...]
        Play a runtime with no pack once per command (split as a POSIX shell would), copy the caches play made or grew
        into the base's game folder and delete the runtime: every later runtime starts warm for those scenes.

longshot-builder: LONGSHOT_BUILD names a program to use for every build; else each build compiles the tree's own (cargo
build --release -p longshot-builder in its builder/, with the development folder's cargo target folder) and keeps a copy beside
what it built (DIR/longshot-builder, DIR/packs/NAME/longshot-builder). Its settings go to it as flags (--store, --root,
--upscaler, --accept-licenses; LONGSHOT_NGX_SDK, the environment's NGX SDK, as --ngx-sdk), the tools' variables as the
developer's overrides they are (dev.py env). A pack's modules come from the same base as the rest of
the runtime, so the game and the renderer always match (MKWVK_API_VERSION). Two packs that bring the same file are refused:
build one pack of both branches. base, pack, rt and warm hold DIR/dev_packs.busy while they run, so a second job on a base
is refused. base.json, pack.json and a runtime's dev_packs.json are read by the Test Farm too.

--json (before the subcommand: dev.py packs --json base ...), or LONGSHOT_BUILD_JSON=1: longshot-builder runs with --output
json and its event lines (docs/dev/reference/build-events.md: an error's code among them) are this command's stdout as it wrote them;
this script's own lines, cargo's and the game's go to stderr. Without it, longshot-builder runs with --output text: never a
window or a message box (docs/dev/reference/build.md, "Output").
"""
import argparse
import hashlib
import json
import os
import re
import shlex
import shutil
import tempfile
import subprocess
import sys
import time

from . import common, jobqueue, vcs

ROOT = common.ROOT
# (the modules' names follow the target, not this PC: RX_TARGET=windows on Linux builds the Windows game)
TARGET_WINDOWS = os.name == "nt" or os.environ.get("RX_TARGET", "").strip().lower() == "windows"
CROSS = TARGET_WINDOWS and os.name != "nt"   # (the Windows game built on Linux: it cannot run here, so it prepares on the PC that plays it)
# (the game's own caches, in its game folder's cache/ (gpu/vk/vk_paths.h cache_path), kept between runs and stamped by the
# translator (gpu/vk/vk_progcache.cpp): a stale one is not used, so a base can start from an older base's. All of them:
# the program cache plain or compressed and its index, its modules' ids (vk_programs.cpp), the pipeline notes, predicted
# (with their keys) and built (vk_draw.cpp), the compute pipelines and their variants, the state lists, the prepare's
# record, and the driver's pipeline cache with its stamp (pcache_stamp.h). tests/test_dev_packs.py fails on a cache the
# renderer names that is not here)
CACHE_DIR = "cache"
CACHES = ("longshot_programs.bin", "longshot_programs.idx", "longshot_programs.zst.bin", "longshot_programs.zst.idx",
          "longshot_module_ids.bin", "longshot_pipelines.bin", "longshot_pipelines.bin.predicted", "longshot_pipelines.bin.predicted_keys",
          "longshot_pipelines.bin.built",
          "longshot_computes.bin", "longshot_compute_variants.bin", "longshot_pipeline_states.bin",
          "longshot_pipeline_states_v1.bin", "longshot_prepared.bin", "longshot_vk_pipelines.bin", "longshot_vk_pipelines.stamp",
          # (the size of the cache's one translator's pipelines, which bounds what another translator carries: pcache_stamp.h)
          "longshot_vk_pipelines.base",
          # (the textures a draw has sampled, for RX_VK_TEX_AHEAD: gpu/vk/texture_ahead.h)
          "longshot_textures_drawn.bin",
          # (AMD FSR 4's unrolled layers' own pipeline cache: gpu/vk/vk_fsr4.cpp own_cache)
          "longshot_fsr4_pipelines.bin")
PLAY_FILES = re.compile(r"longshot_vk_pipelines_play_[0-9]+\.bin")   # (what play adds to the pipeline cache: gpu/vk/vk_draw.cpp play_path)
# (what the game writes into its folder at run time, never a build's output: the caches (and the button prompts it makes) in
# cache/, the logs in logs/ (game/game_layout.c)
RUNTIME_DIRS = (CACHE_DIR + "/", "logs/")
BUSY = "dev_packs.busy"   # (in a base's folder while a job builds, warms or copies from it)
SEED = "longshot_shaders.bin"   # (the shader list the prepare goes through: a new one is prepared again)
# (module, its map, its link list in the work folder's build/, the object that holds the version: build-compile's
# generated/version.rs gives host/version.c and gpu/vk/vk_version.cpp a fingerprint of every source, so those two objects
# differ in every build of another tree; STAMPED_SOURCES are their sources, in the same order)
MODULES = ((("longshot.exe", "longshot.map", "objs.rsp", "host_version.o"),
            ("longshot_vk.dll", "longshot_vk.map", "vk_objs.rsp", "vk_vk_version.o")) if TARGET_WINDOWS else
           (("longshot", None, "objs.rsp", "host_version.o"), ("longshot_vk.so", None, "vk_objs.rsp", "vk_vk_version.o")))
GAME, RENDERER = MODULES[0][0], MODULES[1][0]
STAMPED_SOURCES = ("host/version.c", "gpu/vk/vk_version.cpp")
# (what a build reads: longshot-builder's sources and the Rust workspace's (Cargo.toml, Cargo.lock, longshot-rs/, the
# Rust the game links), the game's (host/, game/, gpu/, third_party/), the assets it makes the game
# folder's files from, the files it puts in it, the release's name and the hook sites' addresses. Anything else (docs,
# checks, CI, the website, scripts/: the developer tools) no build reads)
BUILT = ("builder/", "Cargo.toml", "Cargo.lock", "longshot-rs/", "host/", "game/", "gpu/", "third_party/", "assets/profiles/", "assets/pipeline_states/", "assets/prompt_faces/",
         "assets/longshot.png", "assets/longshot.ico", "assets/addresses.ini", "VERSION")
HEADERS = (".h", ".hpp", ".hh", ".inc", ".inl")
SOURCES = (".c", ".cc", ".cpp") + HEADERS
# (what the game writes beside itself while it runs, never a build's output: settings, logs and reports, and the markers of a
# skipped preparation and of a slow load (vk_main.cpp, vk_draw.cpp); not seeded either)
WRITTEN_IN_PLAY = ("longshot_options.ini", "longshot_log.txt", "longshot_log.1.txt", "longshot_driver_faults.txt",
                   "longshot_prepared.bin.warming", "longshot_pipelines.bin.predicting", "longshot_pipelines.bin.running",
                   "longshot_pipelines.bin.built.warming", "longshot_prepare_skipped", "longshot_load_start.txt",
                   "longshot_load_start.txt.waited", "longshot_pipeline_report.txt", "longshot_frames.csv",
                   "longshot_damaged_files.txt")
# (longshot-builder's own notes in a game folder: what `update` builds the folder again from, with what it copied there
# in it (its files; apart in .longshot_installed.json for an earlier builder).
# Never a pack's change: another folder's notes name another folder's files. longshot_builder.json, longshot.json: the record's names
# in a folder an earlier builder made)
BUILDER_NOTES = (".longshot_installed.json", "longshot_build.json", "longshot_builder.json", "longshot.json")
UPSCALERS = ("dlss", "fsr4", "xess")


def translator_stamp(game):
    """The shader translator's stamp of the build in a game folder, as longshot-builder wrote it in longshot_version.txt
    (translator_stamp=: build-compile's generated/shader_stamp.rs, what the renderer's caches are checked against), or None"""
    try:
        with open(os.path.join(game, "longshot_version.txt"), encoding="utf-8", errors="replace") as f:
            for line in f:
                k, _, v = line.partition("=")
                if k.strip() == "translator_stamp":
                    return v.strip() or None
    except OSError:
        pass
    return None


def translator_changed(base_game, game):
    """Whether the build in game has another shader translator than its base's (both stamps as the builder wrote them; a
    folder without one counts as changed)"""
    old, new = translator_stamp(base_game), translator_stamp(game)
    return old is None or new is None or old != new


PROGRAM_CACHE = "gpu/vk/vk_progcache.cpp"
CACHE_FORMAT = re.compile(r"constexpr\s+uint32_t\s+kProgramCacheFormat\s*=\s*(\d+)")


def cache_format_changed(tree, commit, changed):
    """Whether the checkout's program cache format (kProgramCacheFormat, which the game's program_cache_stamp holds beside the
    translator's stamp) differs from the base commit's: the caches a runtime copies from the base are then dropped at its first
    start, so it prepares again as a translator change does"""
    if PROGRAM_CACHE not in changed:
        return False
    try:
        old = vcs.show(tree, commit, PROGRAM_CACHE)
    except (subprocess.CalledProcessError, OSError):
        old = ""
    try:
        with open(os.path.join(tree, *PROGRAM_CACHE.split("/")), encoding="utf-8") as f:
            new = f.read()
    except OSError:
        new = ""
    def fmt(text):
        m = CACHE_FORMAT.search(text)
        return m.group(1) if m else None
    return fmt(old) != fmt(new)


def built_paths(paths):
    """The paths a build reads (BUILT), but documents"""
    return [p for p in paths if p.startswith(BUILT) and not p.endswith(".md")]


def route(paths):
    """How a change is built: None (nothing a module or the game folder is made from) or "game" (longshot-builder)"""
    return "game" if built_paths(paths) else None


def head(tree):
    """The commit a checkout is on (vcs.commit: jj's @, or @- when @ is empty)"""
    return vcs.commit(tree)[0]


def changed_files(tree, commit):
    """The files a checkout changed since a commit: in its commits, not committed, and new ones not ignored"""
    return vcs.changed_files(tree, commit)


def sha(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for b in iter(lambda: f.read(1 << 22), b""):
            h.update(b)
    return h.hexdigest()


def same_file(a, b):
    try:
        return os.path.samefile(a, b) or (os.path.getsize(a) == os.path.getsize(b) and sha(a) == sha(b))
    except OSError:
        return False


def link_list(build, rsp):
    """The objects a module was linked from, by name (build-compile's response file in the work folder's build/)"""
    with open(os.path.join(build, rsp), encoding="utf-8") as f:
        return {os.path.basename(line.strip().strip('"')): line.strip().strip('"') for line in f if line.strip()}


def module_changed(base_build, build, rsp, stamped, stamped_may_differ):
    """Whether a module differs from the base's beyond the version it names: another set of objects, an object that differs, or
    the object that holds the version when it may differ for more than that (stamped_may_differ: its source changed). Its
    bytes cannot say it: the version (a fingerprint of all the sources) moves the rest of that object's code. True when the
    base kept no link list to compare with"""
    try:
        mine, theirs = link_list(build, rsp), link_list(base_build, rsp)
    except OSError:
        return True
    if set(mine) != set(theirs):
        return True
    return any(not same_file(mine[o], theirs[o]) for o in mine if o != stamped or stamped_may_differ)


def link_inputs_changed(paths):
    """A change that reaches the modules besides their objects, which the link lists do not show: longshot-builder's own code
    (builder/: the flags, the link's options and libraries, the resources it makes), the Rust the game links (the
    workspace's Cargo.toml and Cargo.lock, longshot-rs/: liblongshot_rs.a), or a file under host/, game/, gpu/ or
    third_party/ that is not C or C++ (libraries, resources, shader sources). The pack then keeps both modules"""
    return any(p.startswith(("builder/", "longshot-rs/", "Cargo.")) or (p.startswith(("host/", "game/", "gpu/", "third_party/")) and not p.endswith(SOURCES))
               for p in built_paths(paths))


def build_outputs(game):
    """The files a build made in a new game folder, outside romfs: the base's, recorded when it is built, are the only files a
    pack may find gone (a played folder also holds saves, settings and other files no build makes)"""
    skip = {m for mod in MODULES for m in mod[:2] if m} | set(CACHES) | set(WRITTEN_IN_PLAY) | set(BUILDER_NOTES)
    out = []
    for top, dirs, fs in os.walk(game):
        dirs[:] = sorted(x for x in dirs if not (top == game and x == "romfs"))
        for f in sorted(fs):
            rel = os.path.relpath(os.path.join(top, f), game).replace(os.sep, "/")
            if rel not in skip and not rel.endswith(".log") and not runtime_file(rel):   # (play caches too: a seed brings them)
                out.append(rel)
    return out


def generated_changes(base_game, game, base_outputs=None):
    """The files of a built game folder that differ from the base's, besides the modules, their maps, the caches and the
    builder's notes: what the build generates (the version, the shader list, the shipped pipeline list, button prompts, the
    texts), and the base's files outside romfs that this build did not make, of those its own build made (base_outputs)"""
    skip = {m for mod in MODULES for m in mod[:2] if m} | set(CACHES) | set(BUILDER_NOTES)
    changed = []
    for top, dirs, fs in os.walk(game):
        dirs[:] = sorted(x for x in dirs if not (top == game and x == "romfs"))
        for f in sorted(fs):
            p = os.path.join(top, f)
            rel = os.path.relpath(p, game).replace(os.sep, "/")
            if rel in skip or runtime_file(rel):
                continue
            q = os.path.join(base_game, rel)
            if not (os.path.isfile(q) and same_file(p, q)):
                changed.append(rel)
    gone = [rel for rel in base_outputs or ()
            if not is_cache(rel) and not runtime_file(rel) and rel not in WRITTEN_IN_PLAY and rel not in BUILDER_NOTES
            and not os.path.isfile(os.path.join(game, rel))]
    return changed, gone


def clear_output(game, keep=()):
    """A game folder that will hold only the next build's output: everything but romfs (a dump's hard links) and the
    top-level files named in keep goes, so a file an earlier pack's build made and this one does not is not taken for this
    change's. keep: what longshot-builder checks itself, by its install note (the modules, image.bin: not copied again when
    they are the new ones' bytes) and the romfs's index"""
    for name in os.listdir(game) if os.path.isdir(game) else ():
        p = os.path.join(game, name)
        if name == "romfs" or (name in keep and os.path.isfile(p) and not os.path.islink(p)):
            continue
        if os.path.isdir(p) and not os.path.islink(p):
            shutil.rmtree(p)
        else:
            os.remove(p)


# (the record: its files are the install note (an earlier builder's apart, .longshot_installed.json))
PACK_KEEPS = (GAME, RENDERER, "image.bin", "longshot_romfs.idx", "longshot_build.json", ".longshot_installed.json")


def store():
    """The stores every base and pack on this PC shares (longshot-builder's --store: compile/ and lifts/), unless
    LONGSHOT_STORE names another: a new base compiles and lifts only what no earlier one did. In the development folder,
    beside the bases; on another drive than the bases its objects are copied instead of linked, so set LONGSHOT_STORE to a
    folder on theirs"""
    return os.environ.get("LONGSHOT_STORE") or common.dev_path("store")


def cargo():
    """cargo, to build longshot-builder with: LONGSHOT_CARGO, else the PATH's (mise.toml pins Rust)"""
    c = os.environ.get("LONGSHOT_CARGO") or shutil.which("cargo")
    if not c:
        sys.exit("no cargo to build longshot-builder with: `mise install` (mise.toml pins Rust), or set LONGSHOT_CARGO, or "
                 "LONGSHOT_BUILD to a longshot-builder program")
    return c


def builder(tree, keep):
    """longshot-builder for the checkout tree: LONGSHOT_BUILD's program, else the tree's own, compiled (cargo build --release,
    the development folder's cargo target folder: every checkout shares the crates' builds) and copied to keep, where it
    stays while it builds (another checkout's compile replaces the target folder's). The compile is a job of the PC's queue
    (mode build), which takes the target folder's lock itself once it runs (`dev.py packs builder`), or runs directly when
    why_direct says so: the lock is never held while a slot is waited for (a running job that wants the lock, a build
    inside a job, would wait for it while this waited for that job's slot: neither ever went on)"""
    named = os.environ.get("LONGSHOT_BUILD")
    if named:
        if not os.path.isfile(named):
            sys.exit("LONGSHOT_BUILD names %s, which is not there" % named)
        return named
    environ = dict(os.environ, LONGSHOT_HEAVY="cargo")   # (LONGSHOT_HEAVY marks the cargo for the job queue: a build)
    why = why_direct(environ)
    if why is None:
        cmd = [sys.executable, "-I", os.path.join(ROOT, "scripts", "dev.py"), "packs"] + (["--json"] if JSON else []) + [
            "builder", "--tree", tree, "--keep", keep]
        rc = jobqueue.run("longshot-builder", "build", cmd, environ=environ, cwd=tree,
                          desc="longshot-builder of %s" % os.path.basename(tree))
        if rc:
            sys.exit("longshot-builder's compile failed (exit status %d)" % rc)
        return keep
    print("dev.py packs: longshot-builder runs directly (%s)" % why, file=sys.stderr, flush=True)
    return compile_builder(tree, keep)


def compile_builder(tree, keep):
    """builder()'s compile, here and now: the target folder held (one checkout at a time builds in the shared target folder
    and copies its program out: another's build between the two would replace the program before it is copied), cargo run,
    the program copied to keep"""
    target = common.dev_path("cargo")
    with common.Lock(target + ".lock", waiting="dev.py: another checkout builds longshot-builder: waiting for it"):
        cmd = [cargo(), "build", "--release", "--locked", "-p", "longshot-builder"]
        say("+ " + " ".join(cmd), flush=True)
        rc = subprocess.call(cmd, cwd=os.path.join(tree, "builder"),
                             env=dict(os.environ, CARGO_TARGET_DIR=target, LONGSHOT_HEAVY="cargo"),
                             stdout=sys.stderr if JSON else None)   # (cargo's lines: never on longshot-builder's JSON stdout)
        if rc:
            sys.exit("cargo build -p longshot-builder failed (exit status %d)" % rc)
        built = os.path.join(target, "release", "longshot-builder" + (".exe" if os.name == "nt" else ""))
        os.makedirs(os.path.dirname(keep), exist_ok=True)
        shutil.copy2(built, keep + ".new")
        replace_program(keep + ".new", keep)
    return keep


# (`dev.py packs --no-queue`: every build of this command runs directly, as LONGSHOT_QUEUE=0 asks)
NO_QUEUE = False


def why_direct(environ):
    """Why a build here runs directly rather than as a job of the PC's queue, or None (queued): inside a job already (it
    holds what this needs), --no-queue or LONGSHOT_QUEUE=0, the Test Farm's (Beanstalk: LONGSHOT_BUILD, its own program,
    with LONGSHOT_BUILD_JSON=1, in a sandbox of its own), or a queue this process can't write. Inside a job (jobqueue.
    job_here: a marker in this process's environment or environ, or a running job's process above this one, as when the
    build's environment dropped the markers on the way here) environ gains the job's markers, for what runs directly"""
    found = jobqueue.job_here(environ)
    if found:
        environ.update(found[1])
        return "inside a job"
    if NO_QUEUE:
        return "--no-queue"
    if environ.get("LONGSHOT_QUEUE", "").strip() == "0":
        return "LONGSHOT_QUEUE=0"
    if environ.get("LONGSHOT_BUILD") and environ.get("LONGSHOT_BUILD_JSON", "").strip() == "1":
        return "the Test Farm's (LONGSHOT_BUILD with LONGSHOT_BUILD_JSON=1)"
    if not jobqueue.writable(environ):
        return "the job queue can't be written here"
    return None


def build_job(name, cmd, environ, cwd, desc, to_stderr=False):
    """cmd, a build, as a job of the PC's queue (mode build), or directly when why_direct says so; its exit status.
    to_stderr: its stdout goes to stderr when it runs directly with JSON (cargo's, never longshot-builder's events)"""
    why = why_direct(environ)
    if why is None:
        return jobqueue.run(name, "build", cmd, environ=environ, cwd=cwd, desc=desc)
    print("dev.py packs: %s runs directly (%s)" % (name, why), file=sys.stderr, flush=True)
    return subprocess.call(cmd, cwd=cwd, env=environ, stdout=sys.stderr if JSON and to_stderr else None)


def replace_program(new, keep):
    """new renamed to keep. On Windows a running program can't be replaced but can be renamed: keep is moved aside first
    (keep.old-<n>, deleted at a later replace once nothing runs it)"""
    if os.name != "nt" or not os.path.exists(keep):
        os.replace(new, keep)
        return
    folder, name = os.path.split(keep)
    for f in os.listdir(folder or "."):
        if f.startswith(name + ".old-"):
            try:
                os.remove(os.path.join(folder, f))
            except OSError:
                pass   # (still running)
    try:
        os.replace(new, keep)
    except PermissionError:
        os.replace(keep, "%s.old-%d" % (keep, time.time_ns()))
        os.replace(new, keep)


# (`dev.py packs --json`, or LONGSHOT_BUILD_JSON=1: longshot-builder's events as JSON lines on stdout, as it writes them, for
# the Test Farm to read its error codes; this script's own lines go to stderr then)
JSON = False
# (longshot-builder's commands that take --output: every one that builds, plays or says events)
OUTPUT_COMMANDS = ("build", "update", "check", "image", "index", "lift", "compile", "prepare", "share", "sources",
                   "perf-report", "stamps", "play", "verify")


def output_args(json_events=None):
    """longshot-builder's --output for dev tooling: json when its events are read (JSON, or json_events), else text, so
    nothing a developer, a job or the Test Farm runs opens a window"""
    return ["--output", "json" if (JSON if json_events is None else json_events) else "text"]


def say(*args, **kw):
    """A line of this script's own: stdout, or stderr when stdout carries longshot-builder's events (JSON)"""
    print(*args, file=sys.stderr if JSON else sys.stdout, **kw)


def json_wanted(a):
    """Whether longshot-builder's events are asked for as JSON: `--json`, or LONGSHOT_BUILD_JSON=1"""
    return bool(getattr(a, "json", False)) or os.environ.get("LONGSHOT_BUILD_JSON", "").strip() == "1"


def run(cmd, cwd, env=None, events=False):
    """cmd run in cwd; events: its stdout is longshot-builder's events (left on stdout), else, with JSON, sent to stderr"""
    say("+ " + " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=cwd, check=True, env=dict(os.environ, **(env or {})),
                   stdout=sys.stderr if JSON and not events else None)


def folder_compile_level(game):
    """The game folder's Compile shaders choice (longshot_options.ini's compile_shaders=, as gpu/vk/vk_draw.cpp reads it:
    1 all, else most), or None when it has none"""
    try:
        with open(os.path.join(game, "longshot_options.ini"), encoding="utf-8", errors="replace") as f:
            for line in f:
                k, _, v = line.partition("=")
                if k.strip() == "compile_shaders":
                    return "all" if v.strip() == "1" else "most"
    except OSError:
        pass
    return None


def run_events(cmd, cwd):
    """longshot-builder cmd (with --output json) run in cwd, its events said as they come: on stdout as it wrote them with JSON,
    else their text through say. Returns its exit status and the events read"""
    say("+ " + " ".join(cmd), flush=True)
    events = []
    with subprocess.Popen(cmd, cwd=cwd, stdout=subprocess.PIPE, text=True, encoding="utf-8", errors="replace") as p:
        for line in p.stdout:
            if JSON:
                print(line, end="", flush=True)
            try:
                event = json.loads(line)
            except ValueError:
                event = {"type": "log", "text": line.rstrip("\n")}
            events.append(event)
            if not JSON and event.get("type") != "progress" and event.get("text"):
                say(event["text"], flush=True)
    return p.returncode, events


NOT_PREPARED = re.compile(r"The shaders were not prepared now \((.*)\): the game prepares them at its first start")


def prepare(game, program, required=True):
    """The game's own preparation in a game folder, as longshot-builder runs it at a build: `longshot-builder prepare`, which
    starts the game in its own environment and supervises it (build-driver supervisor.rs: a run that dies or hangs inside one
    shader's compile is started again without that shader); nothing on a Windows game built on Linux, which prepares at its
    first start. required: a failure stops (a runtime); otherwise (a base) it leaves the work to the first start, as
    longshot-builder's own preparation does. Returns why the shaders were not prepared now, or None"""
    if CROSS:
        why = "a Windows game built on Linux"
        say("the shaders were not prepared now (%s): the game prepares them at its first start" % why, flush=True)
        return why
    cmd = [program, "prepare", game, "--compile-shaders", folder_compile_level(game) or "most"] + output_args(True)
    try:
        code, events = run_events(cmd, game)
    except OSError as e:   # (longshot-builder could not start)
        if required:
            raise
        code, events = None, [{"type": "error", "text": str(e)}]
    if code == 0:
        return None
    if required:
        raise subprocess.CalledProcessError(code if code is not None else 1, cmd)
    said = [e.get("text", "") for e in events if e.get("code") == "shaders_not_prepared" or e.get("type") == "error"]
    why = next((m.group(1) for m in map(NOT_PREPARED.search, said) if m), None) or (said[-1] if said else None) \
        or "longshot-builder prepare stopped with exit status %s" % code
    say("the shaders were not prepared now (%s): the game prepares them at its first start" % why, flush=True)
    return why


def env_upscalers(environ):
    """The upscalers a base is built with when --upscaler names none, from the environment (the Test Farm builds with them
    when a tester accepted the vendor's license), and the licenses that acceptance stands for:
    - LONGSHOT_NGX_SDK: NVIDIA DLSS (dlss), with the developer's own SDK, which needs no license here;
    - LONGSHOT_FSR=1: AMD FSR 4 (fsr4); with LONGSHOT_FFX_SDK (AMD's runtime fetched under its license by scripts/fetch.py),
      AMD's license accepted, so longshot-builder puts the runtime in the game folder;
    - LONGSHOT_XESS_SDK: Intel's XeSS library (xess), Intel's license accepted the same way.
    Returns (upscalers, licenses), longshot-builder's names"""
    upscalers, licenses = [], []
    if environ.get("LONGSHOT_NGX_SDK"):
        upscalers.append("dlss")
    if environ.get("LONGSHOT_FSR", "0") not in ("", "0"):
        upscalers.append("fsr4")
        if environ.get("LONGSHOT_FFX_SDK"):
            licenses.append("amd")
    if environ.get("LONGSHOT_XESS_SDK"):
        upscalers.append("xess")
        licenses.append("intel")
    return upscalers, licenses


def setting_flags(environ, args=()):
    """longshot-builder's flags for the settings the environment names to dev.py (docs/dev/reference/build.md, Build
    switches): LONGSHOT_NGX_SDK as --ngx-sdk; none that args give already"""
    ngx = (environ.get("LONGSHOT_NGX_SDK") or "").strip()
    return ["--ngx-sdk", ngx] if ngx and "--ngx-sdk" not in args else []


def longshot_builder(program, tree, b, work, game):
    """longshot-builder's build of tree into game, in work, with the base's game and upscalers, the shaders left to prepare"""
    upscalers = b.get("upscalers") or []
    licenses = b.get("licenses") or []
    run([program, "build", "--game", b["game_source"], "--out", game, "--work", work, "--store", store(), "--root", tree,
         "--language", b.get("language", "en-US"), "--upscaler", ",".join(upscalers) or "none", "--no-prepare-shaders"]
        + (["--accept-licenses", ",".join(licenses)] if licenses else []) + setting_flags(os.environ)
        + output_args(), tree, events=True)


def read_base(d):
    with open(os.path.join(d, "base.json"), encoding="utf-8") as f:
        return json.load(f)


def write_base(d, b):
    b["modules"] = {m[0]: sha(os.path.join(b["game"], m[0])) for m in MODULES}
    with open(os.path.join(d, "base.json"), "w", encoding="utf-8") as f:
        json.dump(b, f, indent=1)


def pack_folders(d):
    """Where a base's packs are built: one work folder and one game folder, the same for every pack (one pack builds at a
    time). longshot-builder keeps the lift there by its key, and takes the objects from the stores"""
    return {"game_work": os.path.join(d, "pack-work"), "game_out": os.path.join(d, "pack-out")}


def overlaps(a, b):
    """One folder is the other or holds it, through links and junctions too (folders on two drives never do)"""
    a, b = (os.path.normcase(os.path.realpath(x)) for x in (a, b))
    try:
        return os.path.commonpath([a, b]) in (a, b)
    except ValueError:
        return False


def check_folders(d, b):
    """A base whose work or game folder is, holds or lies in one of its packs' folders would be rebuilt or deleted by its own
    packs, and compared with itself"""
    for what in ("work", "game"):
        for f in list(pack_folders(d).values()) + [os.path.join(d, "packs")]:
            if overlaps(b[what], f):
                sys.exit("the base's %s folder %s overlaps %s, where its packs are built: give the base other folders" %
                         (what, b[what], f))


def link_or_copy(src, dst):
    """A hard link, or a copy where the two folders are on different drives"""
    try:
        os.link(src, dst)
    except OSError:
        shutil.copy2(src, dst)


FICLONE = 0x40049409   # (Linux's ioctl: dst shares src's blocks until either is written)


def clone(src, dst):
    """A copy of src at dst with its times and mode (shutil.copy2), made a copy-on-write clone where the file system has them
    (btrfs, XFS): a runtime's 600 MB game and the base's caches are copied in no time, and the game writing its copy never
    touches the base's. A plain copy elsewhere. Returns dst, as shutil's copy functions do"""
    if os.name != "nt" and not os.path.islink(src):
        import fcntl
        try:
            with open(src, "rb") as s, open(dst, "wb") as d:
                fcntl.ioctl(d.fileno(), FICLONE, s.fileno())
            shutil.copystat(src, dst)
            return dst
        except OSError:
            pass
    return shutil.copy2(src, dst)


def cmd_base(a):
    tree, d = os.path.abspath(a.tree), os.path.abspath(a.dir)
    work, game = os.path.join(d, "work"), os.path.join(d, "game")
    if os.path.isdir(game) and os.listdir(game):   # (its files are recorded as what the build made: a played folder holds saves)
        sys.exit("%s is not empty: a base is built into a new game folder" % game)
    dirty = [p for p in changed_files(tree, head(tree)) if route([p])]
    if dirty:
        sys.exit("%s has uncommitted or untracked sources (%s): a base is a commit" % (tree, ", ".join(sorted(dirty)[:5])))
    licenses = []
    if not (a.upscaler or "").strip():   # (none named: the environment's, the Test Farm's way)
        upscalers, licenses = env_upscalers(os.environ)
        a.upscaler = ",".join(upscalers)
    upscalers = sorted({{"fsr": "fsr4"}.get(u, u) for u in (a.upscaler or "").lower().replace(" ", "").split(",")}
                       - {"", "none"})
    bad = [u for u in upscalers if u not in UPSCALERS]
    if bad:
        sys.exit("--upscaler: %s is not one of %s" % (", ".join(bad), ", ".join(UPSCALERS)))
    os.makedirs(game, exist_ok=True)
    if a.seed:   # (every cache file, the play files too: recent pipelines can live only in them, vk_draw.cpp play_path)
        src_cache, dst_cache = os.path.join(a.seed, CACHE_DIR), os.path.join(game, CACHE_DIR)
        for c in sorted(os.listdir(src_cache)) if os.path.isdir(src_cache) else []:
            if is_cache(c) and os.path.isfile(os.path.join(src_cache, c)) and not os.path.exists(os.path.join(dst_cache, c)):
                os.makedirs(dst_cache, exist_ok=True)
                shutil.copy2(os.path.join(src_cache, c), os.path.join(dst_cache, c))
    b = {"commit": head(tree), "work": work, "game": game, "game_source": os.path.abspath(a.game), "upscalers": upscalers,
         "licenses": licenses,
         "seeded_from": a.seed}
    t = time.time()
    program = builder(tree, os.path.join(d, "longshot-builder" + (".exe" if os.name == "nt" else "")))
    # (built without preparing; the outputs are recorded then, before the preparation, so nothing it writes beside the game
    # (caches, markers, files the renderer adds later) is ever taken for a build's output)
    longshot_builder(program, tree, b, work, game)
    b["outputs"] = build_outputs(game)
    if os.environ.get("LONGSHOT_NO_PREPARE", "0") != "0":   # (as longshot-builder reads it: set and not 0)
        b["not_prepared"] = "LONGSHOT_NO_PREPARE is set"
        say("the shaders were not prepared now (LONGSHOT_NO_PREPARE is set): the game prepares them at its first start", flush=True)
    else:   # (a failure keeps the compiled base, its reason in base.json)
        b["not_prepared"] = prepare(game, program, required=False)
    b["build_s"] = round(time.time() - t)
    for m in MODULES:   # (the maps beside the modules, for profilers)
        if m[1] and os.path.isfile(os.path.join(work, "build", m[1])):
            shutil.copy2(os.path.join(work, "build", m[1]), os.path.join(game, m[1]))
    write_base(d, b)
    say("base %s in %s: %d min" % (b["commit"][:8], d, b["build_s"] // 60))


def cmd_pack(a):
    d, tree = os.path.abspath(a.dir), os.path.abspath(a.tree)
    b = read_base(d)
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", a.name):   # (a plain name: packs/<name> is deleted and made again)
        sys.exit("a pack's name is letters, digits, '.', '_' and '-': %r" % a.name)
    if "game_source" not in b:
        sys.exit("the base %s was built by an older dev.py (no game_source in its base.json): make a new base" % d)
    check_folders(d, b)
    if not vcs.is_ancestor(tree, b["commit"]):
        sys.exit("%s is not built on the base %s: rebase it on the base, or make a new base" % (tree, b["commit"][:8]))
    changed = changed_files(tree, b["commit"])
    # (a build switch set for this pack builds it even when no file changed: RX_VK_PGO, a renderer that counts or another
    # profile; LONGSHOT_TRACY, Tracy's client in the game)
    switched = os.environ.get("RX_VK_PGO") or os.environ.get("LONGSHOT_TRACY", "0") not in ("", "0")
    how = route(changed) or ("game" if switched else None)
    pack = os.path.join(d, "packs", a.name)
    if os.path.isdir(pack):   # (what an earlier build of this pack brought goes: this one says again what differs)
        shutil.rmtree(pack)
    os.makedirs(pack)
    say("%d files changed since the base: %s" % (len(changed), "the game" if how else "nothing to build"), flush=True)
    t, w, files, extra, gone = time.time(), pack_folders(d), {}, [], []
    whole = link_inputs_changed(changed) or bool(switched)
    if how:
        program = builder(tree, os.path.join(pack, "longshot-builder" + (".exe" if os.name == "nt" else "")))
        clear_output(w["game_out"], PACK_KEEPS)
        longshot_builder(program, tree, b, w["game_work"], w["game_out"])
        built = os.path.join(w["game_work"], "build")
        for mod, mp, rsp, stamped in MODULES:
            source = STAMPED_SOURCES[[m[0] for m in MODULES].index(mod)]
            if whole or module_changed(os.path.join(b["work"], "build"), built, rsp, stamped, source in changed):
                for f in (mod, mp):
                    if f:
                        clone(os.path.join(built, f), os.path.join(pack, f))
                files[mod] = sha(os.path.join(pack, mod))
        extra, gone = generated_changes(b["game"], w["game_out"], b.get("outputs"))
        for rel in extra:
            dst = os.path.join(pack, "files", rel)
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            clone(os.path.join(w["game_out"], rel), dst)
    prepare_again = (bool(how) and translator_changed(b["game"], w["game_out"])) \
        or cache_format_changed(tree, b["commit"], changed) or SEED in extra
    with open(os.path.join(pack, "pack.json"), "w", encoding="utf-8") as f:
        json.dump({"base": b["commit"], "tree": tree, "head": head(tree), "changed": sorted(changed),
                   "route": how, "files": files, "generated": extra, "deleted": gone, "prepare": prepare_again,
                   "build_s": round(time.time() - t)}, f, indent=1)
    say("pack %s: %s%s%s%s, in %d s" % (a.name, ", ".join(files) or "no module differs from the base's",
                                          "; %d generated files" % len(extra) if extra else "",
                                          "; %d generated files gone" % len(gone) if gone else "",
                                          "; prepares" if prepare_again else "", time.time() - t))


def in_runtime(rt, rel):
    """A path a pack names (a module, a generated or deleted file) is relative and names a file below the runtime: pack.json is
    a plain file, and one that named an absolute path or climbed out with .. would have rt write or remove a file anywhere. Judged
    by its text alone, before the runtime exists: no .. anywhere (so no name comparison is needed, which a case-sensitive
    folder could fool); the links the runtime itself holds are checked where each file is written (resolves_inside)"""
    # (rooted counts too: "/x" is not isabs on Windows since Python 3.13, and join() would then put it at the drive's root)
    if not rel or os.path.isabs(rel) or os.path.splitdrive(rel)[0] or rel[0] in "/\\":
        return False
    parts = rel.replace("\\", "/").split("/")
    # (no ':' either: on NTFS romfs/foo:s names a stream of foo, and foo is a hard link shared with the base; the files a pack
    # names never hold one)
    return ".." not in parts and ":" not in rel and os.path.normpath(rel) not in (".", "")


def resolves_inside(rt, path):
    """path (which may not exist yet) lies inside the runtime once links are followed: its nearest existing folder, resolved,
    is the runtime or below it, compared by file identity rather than by name. A link in the base's romfs is cloned into the
    runtime, so a path that reads as inside can still lead out of it"""
    p = os.path.realpath(path)
    while not os.path.exists(p):
        up = os.path.dirname(p)
        if up == p:
            return False
        p = up
    root = os.path.realpath(rt)
    while True:
        if os.path.samefile(p, root):
            return True
        up = os.path.dirname(p)
        if up == p:
            return False
        p = up


RT_LAYOUT = 2   # (dev_packs.json's "layout": a runtime of another, or none, is made again, its play state kept: refresh_runtime)
# (what a runtime holds that no build made, kept when it is refreshed or made again: the game's caches and logs, the saves, the
# settings and what else the game writes beside itself; dev_packs.json is written anew)
PLAY_STATE = (CACHE_DIR, "logs", "user", "longshot_env.txt") + WRITTEN_IN_PLAY


def runtime_files(b, over):
    """What a runtime of the base b with the packs' files over (rel -> source, or None: removed) holds outside its play state:
    rel -> (source, linked). A dump's romfs is hard-linked to the base's (linked), everything else is cloned (the game may
    write it; the romfs's index of a .nup names the .nup, read in place)"""
    want, game = {}, b["game"]
    for top, dirs, fs in os.walk(game):
        rel_top = os.path.relpath(top, game).replace(os.sep, "/")
        rel_top = "" if rel_top == "." else rel_top + "/"
        if not rel_top:
            dirs[:] = [x for x in dirs if x not in PLAY_STATE]
        for f in fs:
            rel = rel_top + f
            if rel not in PLAY_STATE and rel != "dev_packs.json":
                want[rel] = (os.path.join(top, f), rel.startswith("romfs/"))
    for rel, src in over.items():
        if src:
            want[rel] = (src, False)
        else:
            want.pop(rel, None)
    return want


def in_place(dst, src, linked):
    """dst is already what the runtime would put there from src: the same file (a hard link), or a clone of it with its size
    and time (clone keeps them; a file the game or a hand rewrote has another time)"""
    try:
        if linked:
            return os.path.samefile(dst, src)
        if os.path.islink(dst):
            return False
        d, s = os.stat(dst), os.stat(src)
        return d.st_size == s.st_size and d.st_mtime_ns == s.st_mtime_ns and not os.path.samefile(dst, src)
    except OSError:
        return False


def read_runtime(rt):
    """A runtime's dev_packs.json, or None (not a runtime this made, or unreadable)"""
    try:
        with open(os.path.join(rt, "dev_packs.json"), encoding="utf-8") as f:
            made = json.load(f)
    except (OSError, ValueError):
        return None
    return made if isinstance(made, dict) else None


def refresh_runtime(rt, want, made, guard):
    """rt made to hold want (runtime_files) in place: a file already there as it would be is kept, one that differs replaced,
    one the last runtime wrote (its dev_packs.json "files") and this one does not want removed; the play state stays. A runtime
    of another layout (or none) has everything but its play state removed first, and is filled anew. guard(rel): stops when a
    path leads out of the runtime. Returns how many files were written"""
    old = made.get("files") if made and made.get("layout") == RT_LAYOUT else None
    if old is None or not isinstance(old, list):
        for name in os.listdir(rt):
            if name not in PLAY_STATE:
                p = os.path.join(rt, name)
                if os.path.isdir(p) and not os.path.islink(p):
                    shutil.rmtree(p)
                else:
                    os.remove(p)
        old = []
    for rel in old:
        if isinstance(rel, str) and rel not in want and in_runtime(rt, rel) and rel.split("/")[0] not in PLAY_STATE:
            dst = os.path.join(rt, rel)
            guard(rel)
            if os.path.lexists(dst) and resolves_inside(rt, os.path.dirname(dst)):
                os.remove(dst)
    written = 0
    for rel, (src, linked) in sorted(want.items()):
        dst = os.path.join(rt, rel)
        guard(rel)
        if os.path.lexists(dst) and in_place(dst, src, linked):
            continue
        if os.path.lexists(dst):
            os.remove(dst)   # (a romfs hard link, or a link: replaced, never written through to the base or the link's target)
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        (link_or_copy if linked else clone)(src, dst)
        written += 1
    return written


def drop_caches(rt):
    """The runtime's game caches removed (cache/'s CACHES and play's pipeline files, the prepared list among them): what
    an earlier base or translator made. Returns how many"""
    d, n = os.path.join(rt, CACHE_DIR), 0
    for f in sorted(os.listdir(d)) if os.path.isdir(d) and not os.path.islink(d) else ():
        p = os.path.join(d, f)
        if is_cache(f) and os.path.isfile(p):
            os.remove(p)
            n += 1
    return n


def seed_play_state(game, rt):
    """The base's play state (its caches, a warm-up's; its settings) copied into the runtime where the runtime has none of its
    own yet: what the runtime grew in play is never replaced"""
    for name in PLAY_STATE:
        src = os.path.join(game, name)
        files = [src] if os.path.isfile(src) else [os.path.join(top, f) for top, _, fs in os.walk(src) for f in fs]
        for s in files:
            t = os.path.join(rt, os.path.relpath(s, game))
            if os.path.isfile(s) and not os.path.lexists(t):
                os.makedirs(os.path.dirname(t), exist_ok=True)
                clone(s, t)


def cmd_rt(a):
    d, rt = os.path.abspath(a.dir), os.path.abspath(a.runtime)
    b = read_base(d)
    made = read_runtime(rt) if os.path.exists(rt) else None
    if os.path.exists(rt) and made is None:
        sys.exit("%s exists and is not a runtime (no dev_packs.json): delete it, or name another folder" % rt)
    for f in [b["game"], b["work"], os.path.join(d, "packs")] + list(pack_folders(d).values()):
        if overlaps(rt, f):
            sys.exit("%s overlaps %s: a runtime goes in a folder of its own" % (rt, f))
    over, needs_prepare = {}, False   # (a file in the runtime -> the pack and the path it comes from, or None: the pack removes it)
    for p in a.packs:
        pdir = os.path.join(d, "packs", p)
        with open(os.path.join(pdir, "pack.json"), encoding="utf-8") as f:
            meta = json.load(f)
        if meta["base"] != b["commit"]:
            sys.exit("pack %s was built on %s, not the base %s: build it again" % (p, meta["base"][:8], b["commit"][:8]))
        mine = {m: os.path.join(pdir, m) for m in meta["files"]}
        mine.update({m[1]: os.path.join(pdir, m[1]) for m in MODULES
                     if m[0] in meta["files"] and m[1] and os.path.isfile(os.path.join(pdir, m[1]))})
        mine.update({rel: os.path.join(pdir, "files", rel) for rel in meta["generated"]})
        mine.update({rel: None for rel in meta.get("deleted", ())})
        if not set(meta["files"]) <= {m[0] for m in MODULES}:
            sys.exit("pack %s names modules that are not %s: %s" % (p, " or ".join(m[0] for m in MODULES), sorted(meta["files"])))
        bad = sorted(rel for rel in mine if not in_runtime(rt, rel))
        if bad:
            sys.exit("pack %s names paths outside the runtime: %s" % (p, ", ".join(map(repr, bad[:5]))))
        for rel, src in mine.items():
            if rel in over:
                sys.exit("packs %s and %s both bring %s: merge their branches and build one pack" % (over[rel][0], p, rel))
            over[rel] = (p, src)
        needs_prepare |= meta["prepare"]
    os.makedirs(rt, exist_ok=True)

    def guard(rel):
        if rel in over and not resolves_inside(rt, os.path.dirname(os.path.join(rt, rel))):
            sys.exit("pack %s: %s leads outside the runtime through a link in it; nothing of the pack was written past this "
                     "file: delete %s" % (over[rel][0], rel, rt))

    want = runtime_files(b, {rel: src for rel, (_, src) in over.items()})
    stamp = translator_stamp(rt) if made is not None else None
    written = refresh_runtime(rt, want, made, guard)
    # (only a new translator makes the caches stale: a base moved with the same translator keeps the runtime's warm
    # caches, which the renderer checks against their own stamps anyway)
    if made is not None and translator_stamp(rt) != stamp:
        dropped = drop_caches(rt)
        say("%s: the shader translator changed: its caches dropped (%d files), the base's taken" % (rt, dropped))
    seed_play_state(b["game"], rt)
    if needs_prepare:
        prepare(rt, os.environ.get("LONGSHOT_BUILD") or os.path.join(d, "longshot-builder" + (".exe" if os.name == "nt" else "")))
    with open(os.path.join(rt, "dev_packs.json"), "w", encoding="utf-8") as f:
        json.dump({"base": b["commit"], "packs": a.packs, "modules": {m[0]: sha(os.path.join(rt, m[0])) for m in MODULES},
                   "layout": RT_LAYOUT, "files": sorted(want)}, f, indent=1)
    say("%s: the base %s%s (%s)" % (rt, b["commit"][:8], "".join(", %s from %s" % (m, over[m][0]) for m in (GAME, RENDERER) if m in over),
                                      "made, %d files" % written if made is None else "refreshed in place, %d files replaced" % written))


def is_cache(name):
    return name in CACHES or bool(PLAY_FILES.fullmatch(name))


def runtime_file(rel):
    """What the game made in its folder, not the build: anything in cache/ or logs/"""
    return rel.startswith(RUNTIME_DIRS)


def cmd_warm(a):
    """A runtime with no pack, played once by each --run command ({runtime} in it is the runtime's folder; split as a POSIX
    shell would), then the caches play made or grew copied into the base's game folder, and the runtime deleted: every
    later runtime starts warm for those scenes. Only cache files are copied, through a temporary file moved into place"""
    d = os.path.abspath(a.dir)
    b = read_base(d)
    rt = os.path.join(d, "warm-runtime")
    if os.path.exists(rt):   # (made beforehand by `rt <dir> <dir>/warm-runtime`, so the copy can run under a lock of its own)
        try:
            with open(os.path.join(rt, "dev_packs.json"), encoding="utf-8") as f:
                made = json.load(f)
        except (OSError, ValueError):
            made = {}
        if made.get("base") != b["commit"] or made.get("packs"):
            sys.exit("%s exists and is not this base's runtime with no pack (an earlier warm-up stopped?): look at it, delete "
                     "it and run again" % rt)
        say("warming the runtime already made in %s" % rt, flush=True)
    else:
        cmd_rt(argparse.Namespace(dir=d, runtime=rt, packs=[]))
    for cmd in a.run:
        argv = [x.replace("{runtime}", rt) for x in shlex.split(cmd)]
        say("+ " + " ".join(argv), flush=True)
        subprocess.run(argv, check=True, stdout=sys.stderr if JSON else None)
    copied = []
    rt_cache, base_cache = os.path.join(rt, CACHE_DIR), os.path.join(b["game"], CACHE_DIR)
    os.makedirs(base_cache, exist_ok=True)
    for name in sorted(os.listdir(rt_cache)) if os.path.isdir(rt_cache) else []:
        src, dst = os.path.join(rt_cache, name), os.path.join(base_cache, name)
        # (a cache the game wrote: a plain file in the runtime, never a link, whose bytes go into a new temporary file of the
        # base's own and then over the old one, so neither a link in the runtime nor one left in the base is followed)
        if os.path.islink(src) or not os.path.isfile(src) or not is_cache(name) or os.path.islink(dst):
            continue
        if os.path.isfile(dst) and same_file(src, dst):
            continue
        fd, tmp = tempfile.mkstemp(prefix=name + ".", suffix=".tmp", dir=base_cache)
        with os.fdopen(fd, "wb") as out, open(src, "rb") as f:
            shutil.copyfileobj(f, out, 1 << 22)
        shutil.copystat(src, tmp)
        os.replace(tmp, dst)
        copied.append(name)
    b["warmed"] = {"at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "runs": a.run, "files": copied}
    write_base(d, b)
    shutil.rmtree(rt)
    say("the base's caches warmed by %d runs: %s" % (len(a.run), ", ".join(copied) or "nothing grew"))


class Busy:
    """dev_packs.busy in a base's folder while a job builds, warms or copies from it: a second job on the same base is refused
    (game packs share one work folder; a warm-up writes the base's caches). A marker left by a stopped job is deleted by hand"""
    def __init__(self, d, what):
        self.path, self.what = os.path.join(d, BUSY), what

    def __enter__(self):
        common.ensure_dir(os.path.dirname(self.path))
        try:
            fd = os.open(self.path, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
        except FileExistsError:
            try:
                with open(self.path, encoding="utf-8") as f:
                    held = f.read().strip()
            except OSError:
                held = "?"
            sys.exit("the base is busy (%s): wait for that job, or delete %s if it is no longer running" % (held, self.path))
        self.token = "%d %s %s" % (os.getpid(), self.what, time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()))
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(self.token)
        return self

    def __exit__(self, *exc):
        try:
            with open(self.path, encoding="utf-8") as f:
                mine = f.read().strip() == self.token
            if mine:
                os.remove(self.path)
        except OSError:
            pass


def add_commands(sub):
    """`dev.py packs`' subcommands"""
    ap = sub.add_parser("packs", help="a base folder you name and its packs (the Test Farm's: its builds run directly there)",
                        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--json", action="store_true",
                    help="longshot-builder's events as JSON lines on stdout (the Test Farm's), this script's own lines on "
                         "stderr; also LONGSHOT_BUILD_JSON=1")
    ap.add_argument("--no-queue", action="store_true",
                    help="its builds (longshot-builder's compile, packs run's) run directly, not as jobs of the PC's queue; "
                         "also LONGSHOT_QUEUE=0. Directly anyway inside a job, for the Test Farm (LONGSHOT_BUILD with "
                         "LONGSHOT_BUILD_JSON=1) and where the queue can't be written")
    ps = ap.add_subparsers(dest="what", required=True)
    s = ps.add_parser("base", help="build a checkout's commit as a base")
    s.add_argument("dir")
    s.add_argument("--game", required=True, help="the game: a .nup, or a dump folder with romfs/ and exefs/")
    s.add_argument("--tree", default=ROOT)
    s.add_argument("--seed", help="an earlier game folder whose caches the base starts from")
    s.add_argument("--upscaler", default="", help="longshot-builder's --upscaler for the base and its packs (dlss,fsr4,xess); "
                   "default: the environment's (LONGSHOT_NGX_SDK, LONGSHOT_FSR, LONGSHOT_XESS_SDK)")
    s = ps.add_parser("pack", help="a change built into a pack over the base")
    s.add_argument("dir")
    s.add_argument("name")
    s.add_argument("--tree", default=ROOT)
    s = ps.add_parser("rt", help="a game folder to run: the base with packs over it")
    s.add_argument("dir")
    s.add_argument("runtime")
    s.add_argument("packs", nargs="*")
    s = ps.add_parser("run", help="longshot-builder of a checkout (built from its builder/), with the arguments after --")
    s.add_argument("--tree", default=ROOT)
    s.set_defaults(takes_command=True)
    s = ps.add_parser("builder", help=argparse.SUPPRESS)   # (builder()'s queued job: the compile under the target's lock)
    s.add_argument("--tree", required=True)
    s.add_argument("--keep", required=True)
    s = ps.add_parser("warm", help="play a runtime with no pack and keep the caches it grew in the base")
    s.add_argument("dir")
    s.add_argument("--run", action="append", required=True, help="a command that plays the runtime once; {runtime} is its folder")
    ap.set_defaults(func=command)


RUNS_IN_TREE = ("build", "update", "lift", "compile")   # (longshot-builder's commands that take --root and --store)
# TODO(cache budget): longshot-builder keeps its cache and the stores it builds with (the PC's shared store among them)
# within one budget, LONGSHOT_CACHE_MB (12288). If the PC's shared store needs more (many trees' objects and lifts),
# set a larger one in the build's environment for every build of the PC's store.


def cmd_run(a):
    """`dev.py packs run [--tree T] -- ARGS`: longshot-builder of the checkout T with ARGS (its --root the checkout, its
    --store the PC's stores, unless ARGS name them), its exit status; the program kept in T's build-<system>/"""
    tree = os.path.abspath(a.tree)
    args = list(a.command)
    if not args:
        sys.exit("dev.py packs run: longshot-builder's arguments after --")
    own = os.path.join(tree, "build-windows" if os.name == "nt" else "build-linux",
                       "longshot-builder" + (".exe" if os.name == "nt" else ""))
    program = builder(tree, own)
    if args[0] in RUNS_IN_TREE:
        args += (["--root", tree] if "--root" not in args else []) + (["--store", store()] if "--store" not in args else [])
    # (a build, an update, a lift or a compile of your own: the tree's own work folder in the development folder
    # (common.work_dir: one per checkout), not a player's work folder in the per-user cache, unless --work names one)
    if args[0] in RUNS_IN_TREE and not any(x == "--work" or x.startswith("--work=") for x in args):
        args += ["--work", common.work_dir(tree)]
    # (a build, an update, a lift or a compile gets the build's environment, as dev.py base/pack/rt give theirs: the
    # tools' variables, the NGX SDK, dev_build.json's "env" and the store; `mise run build` and `build-dev` come here)
    # (every command, a check, stamps, prepare or index too: the development folder's builder-cache/,
    # never the player's, as the build's environment below sets them)
    env = dict(os.environ)
    env.setdefault("LONGSHOT_CACHE", common.dev_path("builder-cache"))
    if args[0] in RUNS_IN_TREE:
        from . import build
        # (the tools of the tree's own tools.lock, which its builder reads with --root tree: not this checkout's when
        # --tree names another)
        env = build.environment(quiet=True, tree=tree)
        args += setting_flags(env, args)
    if args[0] in OUTPUT_COMMANDS and not any(x == "--output" or x.startswith("--output=") for x in args):
        args += output_args()
    say("+ " + " ".join([program] + args), flush=True)
    if args[0] in RUNS_IN_TREE:
        # (a build through the PC's job queue, `mise run build` and `build-dev` too; directly inside a job already)
        return build_job("longshot-builder-" + args[0], [program] + args, dict(env), tree,
                         "longshot-builder %s of %s" % (args[0], os.path.basename(tree)))
    return subprocess.run([program] + args, cwd=tree, env=env).returncode


def command(a):
    global JSON, NO_QUEUE
    JSON = json_wanted(a)
    NO_QUEUE = bool(getattr(a, "no_queue", False))
    if a.what == "run":
        return cmd_run(a)
    if a.what == "builder":
        compile_builder(os.path.abspath(a.tree), os.path.abspath(a.keep))
        return 0
    with Busy(os.path.abspath(a.dir), a.what):
        {"base": cmd_base, "pack": cmd_pack, "rt": cmd_rt, "warm": cmd_warm}[a.what](a)
