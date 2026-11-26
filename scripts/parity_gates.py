"""The parity gates 3, 4 and 5 (docs/dev/reference/parity-gates.md): a refactor of how Longshot changes the game's code is
proven to change nothing a player could see by playing one fixed scripted run on two builds and comparing.

    python -I scripts/parity_gates.py ab --a main=GAME_A --b branch=GAME_B [--out DIR] [--no-aa] [--save NAME] [--boot-save]
                                         [--env NAME=VALUE ...] [--b-env NAME=VALUE ...] [--dry-run]
    python -I scripts/parity_gates.py run --game GAME --out DIR --name NAME [--save NAME] [--watch-from RESULT ...]
                                          [--boot-save] [--env NAME=VALUE ...] [--dry-run]
    python -I scripts/parity_gates.py compare RUN_A RUN_B [--a-again RUN_A2] [--noise FILE] [--json]
    python -I scripts/parity_gates.py noise RUN RUN [RUN ...] --out FILE
    python -I scripts/parity_gates.py nothing-started RUN [--json]
    python -I scripts/parity_gates.py plan

ab: the whole check, main against a branch: A, then B, then A again (--no-aa: not again), each one game through the PC's job
queue in --mode shared (scripts/dev.py queue; docs/dev/developing.md, *Share a PC*), one after the other, in folders
a1-<label>, b-<label> and a2-<label>. The two runs of A measure what differs between two runs of one build (noise), and the
second reads the flags B saw change, so a flag only B changed has a value from A too; B is compared with the first A within
that noise, capped; gate 5 is checked on every run. It prints each gate's verdict with its differences, and writes them to
<out>/verdict.txt and verdict.json. Exit status: 0 every gate passed, 1 a gate failed, 2 bad usage or a run that could not be
made, 3 inconclusive (A's run did not finish, the noise could not be measured, a value one side has and the other lacks,
changed flags the bridge could not all report, no save written). SIGTERM (dev.py queue cancel) or Ctrl+C stops the run in
progress and its game.

run: one game, the scenario below, from a save in the repository (assets/saves; kokiri-start by default) in a test profile of
its own (scripts/longshot_bridge.py start_session: a profile and --user in the run folder, so the player's saves and options
are never read or written), with no mods (HOS_MODS names an empty folder of the run's) and none of the caller's own HOS_, RX_, A64_ or OOT_
switches. It refuses to start a game outside a --mode shared job of the queue, on Windows, and with a run or game folder on a
RAM file system. It writes <out>/<name>/: result.json (each checkpoint, the save files as they were when the game had stopped
writing them, the process's threads and children for gate 5), saves/ (a copy of those files, made before the game was told
to quit), steps.txt (what was sent, at which frame) and session/ (longshot_bridge's run folder: the game's log.txt and the run's
user folder). --watch-from: results of runs of the other build, whose changed flags this run reads at each checkpoint too.
SIGTERM or Ctrl+C ends the game (the bridge's quit, then SIGTERM through its supervisor) and records why.

The scenario (SCENARIO): through the title to the save's spawn (A pressed until Link stands in play, or with --boot-save the
game started straight in the save, HOS_OOT_BOOT_SAVE: pull request #1577), standing, a scripted walk in the house, the debug
teleport to Hyrule Field, a walk and a camera turn there, the teleport back to Kokiri Forest, and the save folder once the
game's autosaves have settled. Every press is timed in the game's frames (the bridge's sequence), and every frame is a 60th
of a second of game time (the 60 cap, Game speed Per frame), so the same presses move Link the same way on any PC at any
frame rate. A checkpoint is taken once the game has stayed calm (Link in a map, no event playing, nothing loading, a fresh
sample) for SETTLED_SAMPLES samples in a row, never at a frame number: a load takes as many frames as the disk and the CPU
make it.

compare, gate 3 (behaviour): every checkpoint on both sides, in order: the map, Link's age, life, maximum life and rupees
exactly; his position, facing and the camera's yaw within a tolerance (TOLERANCE, widened to NOISE_FACTOR times what two runs
of A differed by, never past CEILING; two runs of A further apart than CEILING fail); the flags either side saw change, each
with a value on both sides, exactly, but those that differed between the two runs of A (at most MAX_NOISY_FLAGS). Gate 4
(saves): a save the run wrote on each side, then the save files file by file, byte for byte, but caption.sav's six date
fields and its picture (the wall clock's and a rendering: longshot_saves.py scrub's fields) and the bytes that differed
between the two runs of A (at most MAX_NOISY_SAVE_BYTES a file). nothing-started, gate 5: the run's log, its folders and its
process (the rules above REQUIRED_LOG).

noise: what differs between runs of one build (positions, the exact values, flags, save bytes), for compare --noise. ab
writes it as <out>/noise.json from the two runs of A.

plan: the scenario, the switches and the checks, printed; no game.

Linux (the bridge's Unix socket, /proc for gate 5's threads and children). Standard library only. The client is
scripts/longshot_bridge.py; the driving (the title, the teleport and its refusals, a calm game) is scripts/tours/drive.py's.
"""
import argparse
import hashlib
import importlib.util
import json
import math
import os
import re
import signal
import subprocess
import sys
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCHEMA = 2
WINDOWS = os.name == "nt"

# ---- the run ----

# The game's settings for a run, over a fresh options file (the test profile's user folder holds none): no window, sound or
# controller (the test profile and HOS_NO_SDL); the bridge's input alone; no pacing; the 60 cap with Game speed Per frame,
# every frame a 60th of a second of game time, so a press of N frames is the same game time on every PC; the smallest
# output; text at once; no frame generation or Discord; each save request logged (gate 4 needs a save the run wrote). Nothing
# here changes what the game does in a frame, only how fast frames come and how big they are.
BASE_ENV = {
    "HOS_NO_SDL": "1",
    "HOS_INPUT_EXCLUSIVE": "1",
    "HOS_INPUT_SCRIPT_BACKGROUND": "1",
    "HOS_NO_PACING": "1",
    "HOS_REALTIME_STEP": "0",         # (Game speed: Per frame)
    # (the 60 cap: host/nn_vi.c realtime_step steps a frame a fixed 60th only there. At any other cap the game's rate follows
    # the frames' own: a frame under 60 fps stepped more than a 60th even with HOS_FPS_STEP=0, so the runs of one build on a
    # busy PC walked 0.3 m and 4.7 m on the same presses. host/input_movie.c: a replay is exact only at the 60 cap, Per frame)
    "HOS_FPS_MAX": "60",
    "HOS_FPS_STEP": "0",              # (no frame-time step: it is off at the 60 cap anyway, and kept off for --env HOS_FPS_MAX)
    "HOS_OUTPUT_RES": "640x360",
    "HOS_RT_POOL_MB": "320",
    "HOS_TEXT_SPEED": "3",
    "HOS_DISCORD": "0",
    "HOS_OOT_TRACE_SAVES": "1",       # (save trace: a line each save request)
    "RX_VK_FRAMEGEN": "0",
}
# (the caller's own switches of these families never reach the game: only BASE_ENV, --env and what the run sets)
SWITCH_PREFIXES = ("HOS_", "RX_", "A64_", "OOT_")
SAVE = "kokiri-start"
FIRST_FRAME_S = 1800      # (a first start prepares the shaders before its first frame)
CALM_FRAMES = 60 * 60 * 3
SETTLE_FRAMES = 120       # (after control, before the first checkpoint: the arrival's camera and fade)
# (a checkpoint's game calm this many samples in a row, SETTLED_GAP frames apart: a teleport's arrival scene starts a few frames
# after the map has changed, so one calm sample right after the change was taken mid-arrival, event playing and Link's facing
# still the last map's; and the checkpoint's own sample calm too, else it is taken again, CHECKPOINT_TRIES times at most)
SETTLED_SAMPLES = 4
SETTLED_GAP = 30
CHECKPOINT_TRIES = 5
SCAN_LINE = re.compile(r"control: oot changes scan: .*presents=(\d+)")
SAVE_TRACE = re.compile(r"save trace: reason=")
FLAG_BATCH = 64           # (a subscription's watch list: docs/dev/reference/control-bridge.md, subscribe flags)
NAME = re.compile(r"[a-z0-9][a-z0-9-]{0,63}")
LABEL = re.compile(r"[a-z0-9][a-z0-9-]{0,31}")
INTERRUPTED = 130

# The scenario. Each step ends in a checkpoint of its name. kind: what Link's position at the checkpoint comes from, which
# sets its tolerance: "spawn" (a save's or a teleport's arrival point), "still" (no input since one), "moved" (after a walk).
# Changing a step changes what is compared: SCENARIO_VERSION goes up, and runs of different versions are not compared.
SCENARIO_VERSION = 2   # (2: the 60 cap, and checkpoints at a game calm SETTLED_SAMPLES times in a row)
SCENARIO = (
    {"name": "start", "do": "control", "kind": "spawn"},
    {"name": "still", "do": "wait", "frames": 180, "kind": "still"},
    {"name": "walk", "do": "inputs", "kind": "moved", "after": 90, "steps": [
        {"at": 0, "stick": "L", "x": 0, "y": 1, "frames": 40},
        {"at": 40, "stick": "L", "x": 1, "y": 0, "frames": 20},
        {"at": 60, "stick": "L", "x": 0, "y": -1, "frames": 30}]},
    {"name": "field", "do": "teleport", "to": "HyruleField_Child", "kind": "spawn"},
    {"name": "field-walk", "do": "inputs", "kind": "moved", "after": 90, "steps": [
        {"at": 0, "stick": "L", "x": 0, "y": 1, "frames": 120},
        {"at": 120, "stick": "R", "x": 1, "y": 0, "frames": 60}]},
    {"name": "forest", "do": "teleport", "to": "KokiriForest_Child", "kind": "spawn"},
    {"name": "end", "do": "wait", "frames": 120, "kind": "still"},
)

# ---- what is compared, and how closely ----

# Exactly: the game's own values, which the same presses on the same save give on any build that plays the same.
EXACT = ("map", "age", "life", "max_life", "rupees")
SPREAD = ("pos", "facing", "camera")
# Within a tolerance, by the checkpoint's kind: metres for the position, degrees for Link's facing and the camera's yaw. A
# spawn is a fixed point; a walk can start a frame apart (the bridge starts a press at the first controller read after the
# request, and the game reads the controller from a job worker at times), about 0.1 m at a run. The camera
# follows Link on its own and turns with the right stick. Widened to NOISE_FACTOR times what two runs of one build differed
# by (compare --noise), never narrowed, and never past CEILING: two runs of one build further apart than that are a gate that
# cannot see a change of that size, and fail.
TOLERANCE = {
    "spawn": {"pos": 0.05, "facing": 1.0, "camera": 10.0},
    "still": {"pos": 0.05, "facing": 1.0, "camera": 10.0},
    "moved": {"pos": 0.30, "facing": 5.0, "camera": 15.0},
}
CEILING = {
    "spawn": {"pos": 0.15, "facing": 3.0, "camera": 20.0},
    "still": {"pos": 0.15, "facing": 3.0, "camera": 20.0},
    "moved": {"pos": 0.60, "facing": 10.0, "camera": 30.0},
}
NOISE_FACTOR = 3.0
MAX_NOISY_FLAGS = 32          # (flags that differed between two runs of A: past it, the flags say nothing)
MAX_NOISY_SAVE_BYTES = 64     # (a save file's bytes that differed between two runs of A: past it, gate 4 says nothing)

# ---- gate 5: nothing started (docs/dev/reference/parity-gates.md#gate-5-nothing-started) ----

# With no mods the game loads none and makes nothing for them. What a run shows of it:
#   - the log says the mods loader looked and found none: game/mods.c hos_mods_init's "mods: none in <HOS_MODS>" (the run's
#     HOS_MODS is an empty folder of its own) is REQUIRED, and its lines for mods found are MODS_LOADED;
#   - no log line begins with one of RESERVED_LOG's names: "modcache: ", and names nothing in the game uses yet, held for
#     parts to come, whose pull requests say when they start in these words;
#   - the mods' folder is modcache in the game folder (CACHE_NAMES: game/mods.c's size table and list note, with mods only):
#     none in the game folder, the run's user folder or its working folder;
#   - no thread named lsmod-<what> (15 characters at most, Linux's limit) and no child process of RESERVED_PROCESSES' names
#     (both held as above) among the game's threads or children at the run's end, which must both be read.
REQUIRED_LOG = (("the mods loader found no mods", re.compile(r"\bmods: none in ")),)
MODS_LOADED = re.compile(r"\bmods: (\d+ mods from |.+ <- |.+: \d+ RomFS files)")
RESERVED_LOG = re.compile(r"(?:^|\] |: )(script runtime|mod runtime|modcache|merged files|broker|texture packs): ", re.I)
CACHE_NAMES = ("modcache",)
RESERVED_THREADS = ("lsmod",)
RESERVED_PROCESSES = ("longshot-broker", "lsmod")
CACHE_WALK_SKIP = ("romfs", "exefs")   # (the game's own files: large, read-only, never a cache)


class Failed(Exception):
    pass


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


_MODS = {}


def module(name):
    """scripts/<name>.py, loaded once (the tour's Game catches the client's Failed: one load of longshot_bridge for both)"""
    if name not in _MODS:
        _MODS[name] = load(name, os.path.join(ROOT, "scripts", name + ".py"))
    return _MODS[name]


def tour():
    """the driving (the title, the teleport and its refusals, a calm game): scripts/tours/drive.py"""
    if os.path.join(ROOT, "scripts") not in sys.path:
        sys.path.insert(0, os.path.join(ROOT, "scripts"))
    from tours import drive
    return drive


def ctl():
    """the bridge's client, the one the tour's Game uses (so the gate catches the same Failed)"""
    tour()
    import longshot_bridge
    return longshot_bridge


def scenario_hash():
    text = json.dumps({"version": SCENARIO_VERSION, "steps": SCENARIO, "env": BASE_ENV}, sort_keys=True)
    return hashlib.sha256(text.encode()).hexdigest()[:16]


def on_ram(path):
    """(scripts/devtools/common.py's check, which the PC's queue makes too)"""
    if "devtools_common" not in _MODS:
        _MODS["devtools_common"] = load("devtools_common", os.path.join(ROOT, "scripts", "devtools", "common.py"))
    return _MODS["devtools_common"].on_ram(path)


def write_json(path, value):
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(value, f, indent=1, sort_keys=True)
        f.write("\n")
    os.replace(tmp, path)


def read_json(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def result_path(run):
    """A run's result.json from its folder or the file itself"""
    return run if os.path.isfile(run) else os.path.join(run, "result.json")


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def stop_signal(signum, frame):
    raise KeyboardInterrupt   # (SIGTERM as Ctrl+C: the run's finally ends the game, as pipeline_tour.py's stopped)


def hold_stops():
    for sig in (signal.SIGINT, signal.SIGTERM):
        try:
            signal.signal(sig, signal.SIG_IGN)
        except ValueError:
            pass


def catch_stops():
    try:
        signal.signal(signal.SIGTERM, stop_signal)
    except ValueError:   # (not the main thread: a test's)
        pass


class Flags:
    """The oot subscription's changed flags (docs/dev/reference/control-bridge.md, subscribe changes), on a connection of its own read on a thread:
    each flag's last value seen since the baseline, and how many changes the bridge could not report (more)"""

    def __init__(self, session):
        self.b = session.connect(30)
        self.lock = threading.Lock()
        self.values, self.more, self.frame, self.error = {}, 0, -1, None
        self.stop = threading.Event()
        self.b.send({"id": "parity-flags", "cmd": "subscribe", "events": ["oot"], "every": 1, "changes": True})
        self.thread = threading.Thread(target=self.read, daemon=True)
        self.thread.start()

    def read(self):
        while not self.stop.is_set():
            try:
                msg = self.b.line(1.0)
            except ctl().Failed as e:
                if getattr(e, "code", None) == 3:   # (no line this second)
                    continue
                self.error = str(e)
                return
            except Exception as e:   # (anything else ends the reading, and the next checkpoint says so)
                self.error = "%s: %s" % (type(e).__name__, e)
                return
            if msg is None:
                self.error = "the game closed the flags' connection"
                return
            if msg.get("id") == "parity-flags" and msg.get("ok") is False:
                self.error = "subscribe: %s: %s" % (msg.get("error"), msg.get("message", ""))
                return
            if msg.get("event") != "oot" or msg.get("sub") != "parity-flags":
                continue
            game = msg.get("game") or {}
            with self.lock:
                self.values.update(game.get("changed") or {})
                self.more += int(game.get("more") or msg.get("more") or 0)
                if isinstance(game.get("frame"), int):
                    self.frame = max(self.frame, game["frame"])

    def reset(self):
        with self.lock:
            self.values, self.more = {}, 0

    def snapshot(self):
        with self.lock:
            return dict(self.values), self.more, self.frame

    def close(self):
        self.stop.set()
        self.thread.join(3)
        try:
            self.b.close()
        except OSError:
            pass


def lookup(session, keys, timeout=30):
    """The flags' values now ({"0x<hash>": value}), read through a subscription's watch list in batches of 64 (the first
    sample of each: Link's next update). A flag the game does not have reads null"""
    out = {}
    if not keys:
        return out
    b = session.connect(30)
    try:
        for i in range(0, len(keys), FLAG_BATCH):
            rid = "parity-lookup-%d" % i
            b.send({"id": rid, "cmd": "subscribe", "events": ["oot"], "every": 1, "flags": keys[i:i + FLAG_BATCH]})
            end = time.monotonic() + timeout
            while True:
                msg = b.line(max(0.1, end - time.monotonic()))
                if msg is None:
                    raise Failed("the game closed the lookup's connection")
                if msg.get("id") == rid and msg.get("ok") is False:
                    raise Failed("flags lookup: %s: %s" % (msg.get("error"), msg.get("message", "")))
                if msg.get("event") == "oot" and msg.get("sub") == rid:
                    flags = (msg.get("game") or {}).get("flags") or {}
                    out.update({k: flags.get(k) for k in keys[i:i + FLAG_BATCH]})
                    break
        b.send({"id": "parity-lookup-end", "cmd": "unsubscribe"})
    finally:
        b.close()
    return out


def scan_presents(log_path, cursor, g, limit_frames=60 * 60):
    """The presents of one full pass of the flags' scan, from its log line (written once the four tables have a baseline:
    game/oot_state_tap.c state_log_scan), waited for from the log's byte cursor"""
    start = g.frame
    while True:
        with open(log_path, "rb") as f:
            f.seek(cursor)
            text = f.read().decode("utf-8", "replace")
        m = SCAN_LINE.search(text)
        if m:
            return int(m.group(1)), m.group(0)
        if g.frame - start > limit_frames:
            raise Failed("no 'control: oot changes scan:' line in %d frames: the flags' tables were not read" % limit_frames)
        g.frames(30)


def is_calm(st):
    """A status (drive.py's Game.status) of Link in a map, no event playing, nothing loading, a fresh sample"""
    return bool(st.get("map")) and st.get("event") is False and st.get("loading") is False and bool(st.get("fresh"))


def settled(g, limit_frames=CALM_FRAMES):
    """Until the game has been calm SETTLED_SAMPLES samples in a row, SETTLED_GAP frames apart: that last state"""
    start, run = g.frame, 0
    while True:
        st = g.status()
        run = run + 1 if is_calm(st) else 0
        if run >= SETTLED_SAMPLES:
            return st
        if g.frame - start > limit_frames:
            raise Failed("not settled after %d frames (last state %s)" % (g.frame - start, st))
        g.frames(SETTLED_GAP)


def checkpoint(g, session, flags, step, scan, watch):
    """The game settled, the flags' scan a full pass past it, then: the bridge's sample of Link and the map (calm, else all
    again), the changed flags since the baseline and their values now, with those the other build's runs saw changed (watch)"""
    t = tour()
    for _ in range(CHECKPOINT_TRIES):
        settled(g)
        calm = g.frame
        g.frames(scan + 2)
        for _ in range(40):   # (the samples come every frame; one taken after the pass is enough)
            if flags.snapshot()[2] >= calm + scan or flags.error:
                break
            g.frames(5)
        if flags.error:
            raise Failed("flags: %s" % flags.error)
        changed, more, _ = flags.snapshot()
        r = g.must("status")
        game = r.get("game") or {}
        if is_calm({"map": r.get("map"), "event": game.get("event"), "loading": game.get("loading"), "fresh": t.fresh(r)}):
            break
        g.note("checkpoint %s: not calm at its sample (event %s, loading %s): again" % (step["name"], game.get("event"),
                                                                                         game.get("loading")))
    else:
        raise Failed("checkpoint %s: the game was not calm at its sample %d times" % (step["name"], CHECKPOINT_TRIES))
    keys = sorted(set(changed) | set(watch.get(step["name"], ())))
    values = lookup(session, keys)
    point = {"name": step["name"], "kind": step["kind"], "frame": r.get("frame"), "sample_frame": game.get("frame"),
             "changed": changed, "more": more, "values": values, "fresh": t.fresh(r)}
    for k in ("map", "pos", "facing", "camera", "age", "life", "max_life", "rupees", "event", "loading"):
        point[k] = game.get(k)
    g.note("checkpoint %s: %s %s, %d changed flags%s" % (step["name"], point["map"], point["pos"], len(changed),
                                                         ", %d not reported" % more if more else ""))
    return point


def save_listing(r):
    """The bridge's saves reply as what changes when the game writes: each slot's files with their sizes and times"""
    if not r.get("ok"):
        return None
    return [[s.get("slot"), [[f.get("name"), f.get("size"), f.get("mtime")] for f in s.get("files") or []]]
            for s in r.get("slots") or []] + [["account", r.get("account")], ["option", r.get("option")]]


def settle_saves(g, polls=3, limit_frames=60 * 60 * 2):
    """The save folder once the game has stopped writing it: the same listing `polls` times, a second of wall time and a
    second of frames apart (a file's time is in seconds; the bridge's quit does not wait for a save: docs/dev/reference/control-bridge.md)"""
    start, last, same, r = g.frame, None, 0, None
    while same < polls:
        r = g.ask("saves")
        listing = save_listing(r)
        same = same + 1 if listing is not None and listing == last else 0
        last = listing
        if g.frame - start > limit_frames:
            raise Failed("the save folder did not settle in %d frames" % limit_frames)
        g.frames(60)
        time.sleep(1.0)
    return r


def save_files(folder):
    """{relative path: bytes} of a save folder, or None when there is none"""
    if not folder or not os.path.isdir(folder):
        return None
    out = {}
    for top, dirs, files in os.walk(folder):
        dirs.sort()
        for f in sorted(files):
            p = os.path.join(top, f)
            with open(p, "rb") as fh:
                out[os.path.relpath(p, folder).replace(os.sep, "/")] = fh.read()
    return out


def repo_save_files(name):
    """The repository save's files as the run's save folder holds them after the copy (assets/saves/<name>/slot_00/...)"""
    files = save_files(os.path.join(ROOT, "assets", "saves", name, "slot_00")) or {}
    return {"slot_00/" + rel: data for rel, data in files.items()}


def written_by_run(files, source):
    """The slot files the run wrote: in a slot_ folder, and not the copied save's same bytes (account.sav and option.sav
    are made when the game loads a slot, so they are not a save of the run)"""
    return sorted(rel for rel, data in files.items()
                  if rel.startswith("slot_") and source.get(rel) != data)


def snapshot_saves(folder, run_dir, source):
    """The save files as they are now, before the game is told to quit: copied to <run>/saves, with their hashes and which
    of them the run wrote"""
    files = save_files(folder)
    if files is None:
        return {"path": None, "files": {}, "written": []}
    dest = os.path.join(run_dir, "saves")
    for rel, data in files.items():
        p = os.path.join(dest, *rel.split("/"))
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(data)
    return {"path": "saves", "files": {rel: sha256(data) for rel, data in files.items()},
            "written": written_by_run(files, source)}


def process_evidence(pid):
    """(Linux) the game's threads by name and its child processes, from /proc: gate 5 reads them. read is False unless every
    live thread's name and children were read (a thread that ended meanwhile is skipped: its children go to the others)"""
    out = {"threads": [], "children": [], "read": False}
    task = "/proc/%d/task" % pid
    try:
        tids = os.listdir(task)
    except OSError as e:
        out["why"] = "%s: %s" % (task, e)
        return out
    children, unread = set(), []
    for tid in tids:
        try:
            with open(os.path.join(task, tid, "comm"), encoding="utf-8", errors="replace") as f:
                out["threads"].append(f.read().strip())
            with open(os.path.join(task, tid, "children"), encoding="ascii") as f:
                children.update(int(c) for c in f.read().split())
        except (OSError, ValueError) as e:
            if os.path.isdir(os.path.join(task, tid)):   # (still there: unreadable, e.g. no /proc/.../children here)
                unread.append("%s: %s" % (tid, e))
    for c in sorted(children):
        try:
            with open("/proc/%d/comm" % c, encoding="utf-8", errors="replace") as f:
                out["children"].append({"pid": c, "name": f.read().strip()})
        except OSError:
            out["children"].append({"pid": c, "name": None})
    out["threads"].sort()
    out["read"] = not unread and bool(out["threads"])
    if unread:
        out["why"] = "unreadable: " + "; ".join(unread[:4])
    return out


def run_env(args):
    env = dict(BASE_ENV)
    for kv in args.env:
        k, sep, v = kv.partition("=")
        if not sep or not k:
            raise SystemExit("parity_gates: --env NAME=VALUE, not %r" % kv)
        env[k] = v
    if args.boot_save:
        env["HOS_OOT_BOOT_SAVE"] = "1"
    return env


def drop_callers_switches(keep):
    """The caller's own HOS_, RX_, A64_ and OOT_ variables out of this process's environment, which the game's supervisor
    inherits (longshot_bridge.py runs it with os.environ plus the run's): a player's RX_VK_X=1 in the shell would change one
    side unsaid. Returns the names dropped"""
    gone = sorted(k for k in os.environ if k.startswith(SWITCH_PREFIXES) and k not in keep)
    for k in gone:
        del os.environ[k]
    return gone


def queue_refusal():
    """Why a game may not start here: outside a --mode shared job of the PC's queue (AGENTS.md), or on Windows"""
    if WINDOWS:
        return "the gates run on Linux (the bridge's Unix socket, /proc): not on Windows"
    if not os.environ.get("LONGSHOT_JOB"):
        return ("a game run goes through the PC's job queue: python -I scripts/dev.py queue run parity --mode shared -- "
                "python -I scripts/parity_gates.py run ... (ab does it for you)")
    mode = os.environ.get("LONGSHOT_JOB_MODE")
    if mode != "shared":
        return "a game run is a --mode shared job (this job's mode is %s)" % mode
    return None


def ram_refusal(paths):
    for what, path in paths:
        if path and on_ram(path):
            return "%s %s is on a RAM file system (tmpfs): game runs and their output go on a disk" % (what, path)
    return None


def watched(paths):
    """{checkpoint: [flag keys]} the other build's runs saw change or read"""
    watch = {}
    for path in paths:
        for p in read_json(result_path(path)).get("checkpoints") or []:
            watch.setdefault(p["name"], set()).update(set(p.get("changed") or {}) | set(p.get("values") or {}))
    return {k: sorted(v) for k, v in watch.items()}


def inner_command(game, out, name, save, watch_from, boot_save, env):
    cmd = [sys.executable, "-I", "-B", os.path.abspath(__file__), "run", "--game", game, "--out", out, "--name", name,
           "--save", save]
    for w in watch_from:
        cmd += ["--watch-from", w]
    if boot_save:
        cmd.append("--boot-save")
    for kv in env:
        cmd += ["--env", kv]
    return cmd


def queue_command(name, inner):
    """One run as a job of the PC's queue, in --mode shared, with what it is for the game's version line (AGENTS.md)"""
    return [sys.executable, "-I", "-B", os.path.join(ROOT, "scripts", "dev.py"), "queue", "run", "parity-" + name,
            "--mode", "shared", "--desc", "parity gates run " + name, "--"] + inner


def run(args):
    if not NAME.fullmatch(args.name):
        print("parity_gates: --name is a-z, 0-9 and - (a folder of --out), not %r" % args.name, file=sys.stderr)
        return 2
    if not NAME.fullmatch(args.save) or not os.path.isdir(os.path.join(ROOT, "assets", "saves", args.save, "slot_00")):
        print("parity_gates: no save %r in assets/saves" % args.save, file=sys.stderr)
        return 2
    out = os.path.abspath(args.out)
    run_dir = os.path.join(out, args.name)
    game = os.path.abspath(args.game)
    env = run_env(args)
    watch = watched(args.watch_from)
    why = queue_refusal() or ram_refusal((("the run folder", out), ("the game folder", game)))
    if args.dry_run:
        inner = inner_command(game, out, args.name, args.save, args.watch_from, args.boot_save, args.env)
        print("run %s: %s in %s, save %s, scenario %d (%s)" % (args.name, game, run_dir, args.save, SCENARIO_VERSION,
                                                               scenario_hash()))
        print("environment: %s" % " ".join("%s=%s" % kv for kv in sorted(env.items())))
        print("watched flags from the other build's runs: %d" % sum(len(v) for v in watch.values()))
        print("through the queue: %s" % " ".join(queue_command(args.name, inner)))
        print("would start here: %s" % ("no: " + why if why else "yes"))
        return 0
    if why:
        print("parity_gates: %s" % why, file=sys.stderr)
        return 2
    if os.path.exists(run_dir) and os.listdir(run_dir):
        print("parity_gates: %s is not empty: give another --name or --out" % run_dir, file=sys.stderr)
        return 2
    os.makedirs(run_dir, exist_ok=True)
    no_mods = os.path.join(run_dir, "no-mods")
    os.makedirs(no_mods, exist_ok=True)
    env["HOS_MODS"] = no_mods
    result = {"schema": SCHEMA, "scenario": SCENARIO_VERSION, "scenario_hash": scenario_hash(), "name": args.name,
              "game": game, "save": args.save, "boot_save": bool(args.boot_save), "environment": env,
              "dropped_switches": drop_callers_switches(env), "steps": len(SCENARIO),
              "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "checkpoints": [], "error": None}
    catch_stops()
    code = drive(args, game, run_dir, env, watch, result)
    print("run %s: %s, %d of %d checkpoints: %s" % (args.name, result["error"] or "done", len(result["checkpoints"]),
                                                    len(SCENARIO), os.path.join(run_dir, "result.json")))
    return code


def drive(args, game, run_dir, env, watch, result):
    """The scenario in one game, its result written whatever happens (an exception, SIGTERM, Ctrl+C), the game ended"""
    session = flags = None
    code = 0
    try:
        t, c = tour(), ctl()
        session = c.start_session(game, run=os.path.join(run_dir, "session"), profile="test", repo_save=args.save,
                                  repo=ROOT, env=env, ready="bridge", timeout=FIRST_FRAME_S)
        info = session.info()
        result.update(session=os.path.relpath(info["run"], run_dir), user=info["user"], cwd=info["cwd"],
                      log=os.path.relpath(info["log"], run_dir), pid=info["pid"])
        g = t.Game(session.connect(30), run_dir, os.devnull)
        st = g.status()
        result.update(version=st.get("version"), renderer=st.get("renderer"))
        for step in SCENARIO:
            g.note("step %s: %s" % (step["name"], step["do"]))
            if step["do"] == "control":
                if args.boot_save:   # (the game loads the save itself: no presses; HOS_OOT_BOOT_SAVE, #1577)
                    first = g.ask("wait", timeout=FIRST_FRAME_S, event="frame", frame=1)
                    if not first.get("ok"):
                        raise Failed("no first frame in %d s" % FIRST_FRAME_S)
                else:                # (A at the title, the save list and its question, until Link stands in play)
                    t.new_game(g, first_frame_s=FIRST_FRAME_S)
                t.calm_after(g, lambda s: True, g.frame, CALM_FRAMES)
                g.frames(SETTLE_FRAMES)
                cursor = os.path.getsize(info["log"])
                flags = Flags(session)
                scan, line = scan_presents(info["log"], cursor, g)
                result["scan"] = {"presents": scan, "line": line}
                g.frames(scan + 2)
                flags.reset()   # (changes from here on: the baseline is the save's state, settled)
            elif step["do"] == "wait":
                g.frames(step["frames"])
            elif step["do"] == "inputs":
                r = g.ask("sequence", steps=step["steps"], exclusive=True)
                if not r.get("ok"):
                    raise Failed("sequence: %s: %s" % (r.get("error"), r.get("message", "")))
                g.note("inputs %d-%d" % (r.get("start", -1), r.get("end", -1)))
                g.frames(step["after"])
            elif step["do"] == "teleport":
                t.map_change(g, "teleport", lambda s, to=step["to"]: s["map"] == to, to=step["to"])
            result["checkpoints"].append(checkpoint(g, session, flags, step, result["scan"]["presents"], watch))
        r = settle_saves(g)
        live = r.get("path")
        result["saves"] = snapshot_saves(live, run_dir, repo_save_files(args.save))
        result["saves"]["live"] = live
        result["saves"]["listing"] = save_listing(r)
        result["evidence"] = process_evidence(info["pid"])
        result["evidence"]["cwd"] = info["cwd"]
    except KeyboardInterrupt:
        result["error"] = "stopped (SIGTERM or Ctrl+C)"
        code = INTERRUPTED
    except Exception as e:   # (any of them: recorded, and the game ended below)
        result["error"] = "%s: %s" % (type(e).__name__, e)
        code = 2
    finally:
        hold_stops()   # (a second SIGTERM or Ctrl+C does not cut the game's end and the result short)
        if flags:
            flags.close()
        if session:
            try:
                stopped = session.stop(timeout=30, terminate=True)
                result["exit_code"] = stopped.get("exit_code")
            except Exception as e:
                result["stop_error"] = "%s: %s" % (type(e).__name__, e)
        try:
            saves = result.get("saves") or {}
            if saves.get("live"):   # (written after the copy, before the game ended: the copy is not the last save)
                after = {rel: sha256(data) for rel, data in (save_files(saves["live"]) or {}).items()}
                taken = saves.get("files") or {}
                saves["changed_after"] = sorted(rel for rel in set(after) | set(taken)
                                                if after.get(rel) != taken.get(rel))
            log = os.path.join(run_dir, result["log"]) if result.get("log") else None
            if log and os.path.isfile(log):
                with open(log, encoding="utf-8", errors="replace") as f:
                    result["save_requests"] = sum(1 for line in f if SAVE_TRACE.search(line))
        except Exception as e:   # (the result is written all the same)
            result["finish_error"] = "%s: %s" % (type(e).__name__, e)
            result["error"] = result["error"] or result["finish_error"]
            code = code or 2
        result["ended_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        write_json(os.path.join(run_dir, "result.json"), result)
    if result["error"]:
        print("parity_gates: run %s: %s" % (result["name"], result["error"]), file=sys.stderr)
    return code


# ---- comparing ----

class Verdict:
    """A gate's verdict (pass, inconclusive, fail: the worst said) and its lines"""
    ORDER = {"pass": 0, "inconclusive": 1, "fail": 2}

    def __init__(self):
        self.verdict, self.lines = "pass", []

    def say(self, text):
        self.lines.append(text)

    def bad(self, level, text):
        self.lines.append(text)
        if self.ORDER[level] > self.ORDER[self.verdict]:
            self.verdict = level

    def out(self):
        return self.verdict, self.lines


def angle_diff(a, b):
    d = abs(a - b) % 360.0
    return min(d, 360.0 - d)


def distance(a, b):
    return math.sqrt(sum((x - y) ** 2 for x, y in zip(a, b)))


def number_diff(a, b, kind):
    """The distance between two sampled values of a kind (pos, facing, camera); None when either is unreadable (null).
    A value that is not finite gives NaN, which no tolerance admits"""
    if kind == "pos":
        ok = isinstance(a, list) and isinstance(b, list) and len(a) == len(b) == 3
        return distance(a, b) if ok and all(isinstance(v, (int, float)) for v in a + b) else None
    if isinstance(a, (int, float)) and isinstance(b, (int, float)):
        if not (math.isfinite(a) and math.isfinite(b)):
            return float("nan")
        return angle_diff(a, b)
    return None


MISSING = object()   # (a flag with no value read on a side)


def within(d, tol):
    return d is not None and math.isfinite(d) and d <= tol


def flag_values(point):
    """A checkpoint's flags: the values read at it (null: the game has no such flag), over the last value its changes
    reported"""
    out = dict(point.get("changed") or {})
    out.update(point.get("values") or {})
    return out


def full(result):
    return len(result.get("checkpoints") or []) == result.get("steps", len(SCENARIO)) and not result.get("error")


def noise_problems(noise, a):
    """Why a noise file can't widen anything for this A: (level, text) pairs"""
    if not noise:
        return []
    out = []
    if noise.get("scenario_hash") != a.get("scenario_hash"):
        out.append(("inconclusive", "the noise is of another scenario (%s, A's %s)" % (noise.get("scenario_hash"),
                                                                                      a.get("scenario_hash"))))
    if noise.get("unfinished"):
        out.append(("inconclusive", "the noise's runs did not all finish: %s" % ", ".join(noise["unfinished"])))
    for name, text in noise.get("exact") or ():
        out.append(("fail", "two runs of A differ at %s in %s: the gate cannot tell a change from them" % (name, text)))
    for name, spread in sorted((noise.get("checkpoints") or {}).items()):
        kind = spread.get("kind") or "moved"
        for k in SPREAD:
            v = spread.get(k)
            if v is not None and not within(v, CEILING[kind][k]):
                out.append(("fail", "two runs of A are %s apart in %s at %s, past the most the gate allows (%s)" % (
                    v, k, name, CEILING[kind][k])))
    if len(noise.get("flags") or ()) > MAX_NOISY_FLAGS:
        out.append(("inconclusive", "%d flags differ between two runs of A (at most %d): the flags say nothing" % (
            len(noise["flags"]), MAX_NOISY_FLAGS)))
    for rel, ranges in sorted((noise.get("saves") or {}).items()):
        n = sum(r1 - r0 for r0, r1 in ranges)
        if n > MAX_NOISY_SAVE_BYTES:
            out.append(("inconclusive", "%s: %d bytes differ between two runs of A (at most %d)" % (
                rel, n, MAX_NOISY_SAVE_BYTES)))
    return out


def tolerance(kind, k, spread):
    """A checkpoint's tolerance: its kind's, widened to NOISE_FACTOR times the noise, never past the ceiling"""
    widened = NOISE_FACTOR * spread if spread is not None and math.isfinite(spread) else 0.0
    return min(CEILING[kind][k], max(TOLERANCE[kind][k], widened))


def compare_behaviour(a, b, noise=None, a_again=None):
    """Gate 3: (verdict, lines). a_again: a second run of A (it read the flags B saw change), for flags A's first run has no
    value for"""
    v = Verdict()
    if a.get("scenario_hash") != b.get("scenario_hash"):
        v.bad("inconclusive", "the runs played different scenarios (%s, %s): run both with this script" % (
            a.get("scenario_hash"), b.get("scenario_hash")))
        return v.out()
    for level, text in noise_problems(noise, a):
        v.bad(level, text)
    if not full(a):
        v.bad("inconclusive", "A did not reach every checkpoint (%d of %d): %s" % (
            len(a.get("checkpoints") or []), a.get("steps", len(SCENARIO)), a.get("error") or "no error said"))
    if not full(b):
        v.bad("fail", "B did not reach every checkpoint (%d of %d): %s" % (
            len(b.get("checkpoints") or []), b.get("steps", len(SCENARIO)), b.get("error") or "no error said"))
    usable = noise and not any(level == "inconclusive" for level, _ in noise_problems(noise, a))
    volatile = set(noise.get("flags") or ()) if usable else set()
    spreads = (noise.get("checkpoints") or {}) if usable else {}
    pa = {p["name"]: p for p in a.get("checkpoints") or []}
    pa2 = {p["name"]: p for p in (a_again or {}).get("checkpoints") or []}
    compared = ignored = 0
    for i, pb in enumerate(b.get("checkpoints") or []):
        name = pb["name"]
        p = pa.get(name)
        if p is None:
            continue
        if [q["name"] for q in a.get("checkpoints") or []].index(name) != i:
            v.bad("fail", "%s: the %dth checkpoint in B, not in A's order" % (name, i + 1))
        for k in EXACT:
            if p.get(k) != pb.get(k):
                v.bad("fail", "%s: %s %r in A, %r in B" % (name, k, p.get(k), pb.get(k)))
        kind = p.get("kind") if p.get("kind") in TOLERANCE else "moved"
        seen = spreads.get(name) or {}
        for k, unit in (("pos", "m"), ("facing", "deg"), ("camera", "deg")):
            tol = tolerance(kind, k, seen.get(k))
            d = number_diff(p.get(k), pb.get(k), k)
            if d is None:
                if (p.get(k) is None) != (pb.get(k) is None):
                    v.bad("fail", "%s: %s %r in A, %r in B" % (name, k, p.get(k), pb.get(k)))
                elif p.get(k) is not None:   # (both there, one not a number)
                    v.bad("fail", "%s: %s not a number: %r in A, %r in B" % (name, k, p.get(k), pb.get(k)))
            elif not within(d, tol):
                v.bad("fail", "%s: %s %s %s apart (tolerance %.3f): %r in A, %r in B" % (
                    name, k, "%.3f" % d if math.isfinite(d) else d, unit, tol, p.get(k), pb.get(k)))
        if p.get("more") or pb.get("more"):
            v.bad("inconclusive", "%s: the bridge left out %s changed flags in A and %s in B (more): flags not compared" % (
                name, p.get("more") or 0, pb.get("more") or 0))
            continue
        fa, fb = flag_values(p), flag_values(pb)
        fa2 = flag_values(pa2[name]) if name in pa2 else {}
        for key in sorted(set(fa) | set(fb)):
            if key in volatile:
                ignored += 1
                continue
            va = fa[key] if key in fa else fa2.get(key, MISSING)
            vb = fb.get(key, MISSING)
            if va is MISSING or vb is MISSING:
                side = "A" if va is MISSING else "B"
                v.bad("inconclusive", "%s: flag %s has no value read in %s (%s saw it change): run A again with "
                                      "--watch-from B" % (name, key, side, "B" if side == "A" else "A"))
                continue
            compared += 1
            if va != vb:
                v.bad("fail", "%s: flag %s is %r in A, %r in B" % (name, key, va, vb))
    v.lines.insert(0, "%d checkpoints in A, %d in B, of %d; %d flag values compared, %d left out as noise" % (
        len(pa), len(b.get("checkpoints") or []), len(SCENARIO), compared, ignored))
    return v.out()


def caption_mask(data):
    """caption.sav's bytes that are not the game's state: its six date values (the wall clock) and its picture (a
    rendering), as longshot_saves.py scrub finds them; [] when the layout is not the one known (compared whole)"""
    saves = module("longshot_saves")
    try:
        dates = saves.caption_dates(data)
        saves.caption_layout(data)
    except saves.Fail:
        return []
    return [(off + 4, off + 8) for off, _ in dates.values()] + [(saves.CAPTION_JPEG_LEN, len(data))]


def caption_picture_size(data):
    saves = module("longshot_saves")
    try:
        return saves.jpeg_size(saves.caption_layout(data)["jpeg"])
    except saves.Fail:
        return None


def diff_ranges(a, b, gap=8):
    """The byte ranges [start, end) where a and b differ (over the shorter's length), runs closer than gap joined"""
    out = []
    for i in range(min(len(a), len(b))):
        if a[i] != b[i]:
            if out and i - out[-1][1] <= gap:
                out[-1][1] = i + 1
            else:
                out.append([i, i + 1])
    return out


def inside(r, masks):
    return any(m0 <= r[0] and r[1] <= m1 for m0, m1 in masks)


def run_saves(result, run_dir):
    """A run's save files as copied at settle time, checked against the hashes taken then: (files, None) or (None, (kind,
    why)), kind "missing" (no folder, an empty one, a copy that is not what was hashed, a save written after it) or
    "unwritten" (the run wrote no save)"""
    saves = result.get("saves") or {}
    files = save_files(os.path.join(run_dir, saves["path"])) if saves.get("path") else None
    if files is None:
        return None, ("missing", "no save folder%s" % (": " + result["error"] if result.get("error") else ""))
    if not files:
        return None, ("missing", "an empty save folder")
    if {rel: sha256(d) for rel, d in files.items()} != (saves.get("files") or {}):
        return None, ("missing", "its copied saves are not the ones hashed when the game had stopped writing")
    if saves.get("changed_after"):
        return None, ("missing", "the game wrote %s after the saves were taken" % ", ".join(saves["changed_after"]))
    if not saves.get("written"):
        return None, ("unwritten", "no save written by the run (%s save requests in the log)" % (
            result.get("save_requests"),))
    return files, None


def compare_saves(a, a_dir, b, b_dir, noise=None):
    """Gate 4: (verdict, lines)"""
    v = Verdict()
    usable = noise and not any(level == "inconclusive" for level, _ in noise_problems(noise, a))
    noise_saves = ((noise or {}).get("saves") or {}) if usable else {}
    fa, why_a = run_saves(a, a_dir)
    fb, why_b = run_saves(b, b_dir)
    if why_a:   # (A that wrote no save: the scenario saves nothing on this PC, so the gate cannot say)
        v.bad("inconclusive", "A: " + why_a[1])
    if why_b:   # (B that wrote none where A did: a change in what the game does)
        v.bad("fail" if not why_a and why_b[0] == "unwritten" else "inconclusive", "B: " + why_b[1])
    if why_a or why_b:
        return v.out()
    for rel in sorted(set(fa) | set(fb)):
        if rel not in fa or rel not in fb:
            v.bad("fail", "%s: only in %s" % (rel, "A" if rel in fa else "B"))
            continue
        da, db = fa[rel], fb[rel]
        if da == db:
            v.say("%s: identical (%d bytes)" % (rel, len(da)))
            continue
        masks = [tuple(m) for m in noise_saves.get(rel, ())]
        why = []
        if rel.endswith("caption.sav"):
            ma, mb = caption_mask(da), caption_mask(db)
            if ma and mb and ma[:-1] == mb[:-1]:   # (the same layout: the dates at the same places; the picture's
                cut = module("longshot_saves").CAPTION_JPEG_LEN   # length and bytes are the rendering's, so compared
                da, db = da[:cut], db[:cut]                       # as a size alone)
                masks += ma[:-1]
                why.append("its date and picture")
                if caption_picture_size(fa[rel]) != caption_picture_size(fb[rel]):
                    v.bad("fail", "%s: the picture is %s in A, %s in B" % (rel, caption_picture_size(fa[rel]),
                                                                            caption_picture_size(fb[rel])))
        if len(da) != len(db):
            v.bad("fail", "%s: %d bytes in A, %d in B" % (rel, len(da), len(db)))
            continue
        if noise_saves.get(rel):
            why.append("%d ranges that differ between runs of A" % len(noise_saves[rel]))
        left = [r for r in diff_ranges(da, db) if not inside(r, masks)]
        if left:
            v.bad("fail", "%s: %d ranges differ: %s%s" % (rel, len(left), ", ".join(
                "%d-%d" % (r0, r1 - 1) for r0, r1 in left[:12]), " ..." if len(left) > 12 else ""))
        else:
            v.say("%s: the same but for %s" % (rel, " and ".join(why)))
    v.lines.insert(0, "written by the run: %s in A, %s in B" % (", ".join(a["saves"]["written"]),
                                                               ", ".join(b["saves"]["written"])))
    return v.out()


def nothing_started(result, run_dir):
    """Gate 5: (verdict, lines) for one run: the rules above REQUIRED_LOG"""
    v = Verdict()
    log = os.path.join(run_dir, result["log"]) if result.get("log") else None
    if not log or not os.path.isfile(log):
        v.bad("inconclusive", "no log")
        return v.out()
    with open(log, encoding="utf-8", errors="replace") as f:
        text = f.read().splitlines()
    for what, pattern in REQUIRED_LOG:
        if not any(pattern.search(line) for line in text):
            v.bad("fail", "the log does not say %s (%s)" % (what, pattern.pattern))
    loaded = [line for line in text if MODS_LOADED.search(line)]
    if loaded:
        v.bad("inconclusive", "mods were loaded, so this is not a run with none: %s" % loaded[0].strip())
    for line in text:
        if RESERVED_LOG.search(line):
            v.bad("fail", "started: %s" % line.strip())
    roots = [("the game folder", result.get("game")), ("the user folder", result.get("user")),
             ("the working folder", (result.get("evidence") or {}).get("cwd"))]
    seen = set()
    for what, top in roots:
        if not top or not os.path.isdir(top) or os.path.realpath(top) in seen:
            continue
        seen.add(os.path.realpath(top))
        for here, dirs, files in os.walk(top):
            dirs[:] = [d for d in dirs if d.lower() not in CACHE_WALK_SKIP and not os.path.islink(os.path.join(here, d))]
            for name in dirs + files:
                if name.lower() in CACHE_NAMES:
                    v.bad("fail", "a merged-file cache in %s: %s" % (what, os.path.join(here, name)))
    ev = result.get("evidence") or {}
    if not ev:
        v.bad("inconclusive", "the game's threads and children were not read: the run did not reach its end (%s)" % (
            result.get("error") or "no error said"))
    elif not ev.get("read"):
        v.bad("fail", "the game's threads and children could not be read: %s" % ev.get("why", "unknown"))
    for name in ev.get("threads") or ():
        if name.lower().startswith(RESERVED_THREADS):
            v.bad("fail", "a mod system thread: %s" % name)
    for child in ev.get("children") or ():
        name = (child.get("name") or "").lower()
        if name.startswith(RESERVED_PROCESSES):
            v.bad("fail", "a mod system process: %s (pid %s)" % (child.get("name"), child.get("pid")))
        else:
            v.say("a child process, not the mod system's: %s (pid %s)" % (child.get("name"), child.get("pid")))
    if v.verdict == "pass":
        v.lines.insert(0, "the mods loader found none; no runtime, cache or broker line, folder, thread or process "
                          "(%d threads read)" % len(ev.get("threads") or ()))
    return v.out()


def load_run(run):
    path = result_path(run)
    return read_json(path), os.path.dirname(os.path.abspath(path))


def make_noise(runs):
    """What differs between runs of one build: per checkpoint the largest pos/facing/camera distance (NaN: unbounded), the
    exact values that differ (a gate that must then fail), the flags whose values differ, per save file the byte ranges"""
    results = [load_run(r) for r in runs]
    hashes = {r.get("scenario_hash") for r, _ in results}
    if len(hashes) != 1:
        raise Failed("the runs played different scenarios: %s" % sorted(map(str, hashes)))
    noise = {"scenario_hash": hashes.pop(), "runs": [os.path.abspath(r) for r in runs], "checkpoints": {},
             "exact": [], "flags": [], "saves": {},
             "unfinished": [os.path.basename(d) for r, d in results if not full(r)]}
    flags = set()
    first, first_dir = results[0]
    for other, other_dir in results[1:]:
        pa = {p["name"]: p for p in first.get("checkpoints") or []}
        for pb in other.get("checkpoints") or []:
            p = pa.get(pb["name"])
            if p is None:
                continue
            spread = noise["checkpoints"].setdefault(pb["name"], {"kind": p.get("kind")})
            for k in SPREAD:
                d = number_diff(p.get(k), pb.get(k), k)
                if d is not None:
                    d = round(d, 4) if math.isfinite(d) else float("inf")
                    spread[k] = max(spread.get(k, 0.0), d)
            for k in EXACT:
                if p.get(k) != pb.get(k):
                    noise["exact"].append([pb["name"], "%s (%r, %r)" % (k, p.get(k), pb.get(k))])
            fa, fb = flag_values(p), flag_values(pb)
            flags.update(k for k in set(fa) & set(fb) if fa[k] != fb[k])
        sa, _ = run_saves(first, first_dir)
        sb, _ = run_saves(other, other_dir)
        for rel in sorted(set(sa or {}) & set(sb or {})):
            da, db, skip = sa[rel], sb[rel], []
            if rel.endswith("caption.sav"):   # (its date and picture: left out as compare_saves leaves them out)
                ma, mb = caption_mask(da), caption_mask(db)
                if ma and mb and ma[:-1] == mb[:-1]:
                    cut = module("longshot_saves").CAPTION_JPEG_LEN
                    da, db, skip = da[:cut], db[:cut], ma[:-1]
            if len(da) == len(db) and da != db:
                ranges = [r for r in diff_ranges(da, db) if not inside(r, skip)]
                if ranges:
                    noise["saves"].setdefault(rel, []).extend(ranges)
    noise["flags"] = sorted(flags)
    return noise


def gate_lines(title, verdict, lines):
    return ["%s: %s" % (title, verdict.upper())] + ["  " + line for line in lines]


def verdict_code(verdicts):
    if "fail" in verdicts:
        return 1
    if "inconclusive" in verdicts:
        return 3
    return 0


def compare_runs(a, a_dir, b, b_dir, noise, a_again=None, extra=()):
    gates = []
    for gate, title, (verdict, lines) in (
            (3, "gate 3, behaviour", compare_behaviour(a, b, noise, a_again)),
            (4, "gate 4, saves", compare_saves(a, a_dir, b, b_dir, noise)),
            (5, "gate 5, nothing started (B: %s)" % b.get("name"), nothing_started(b, b_dir))) + tuple(extra):
        gates.append({"gate": gate, "title": title, "verdict": verdict, "lines": lines})
    text = ["A: %s (%s), B: %s (%s); noise from %s" % (a.get("name"), a.get("version"), b.get("name"), b.get("version"),
                                                       "%d runs of A" % len(noise["runs"]) if noise else "none")]
    for g in gates:
        text += gate_lines(g["title"], g["verdict"], g["lines"])
    code = verdict_code([g["verdict"] for g in gates])
    text.append("parity: %s" % ("PASS" if code == 0 else "FAIL" if code == 1 else "INCONCLUSIVE"))
    return {"gates": gates, "text": text, "code": code}


def compare(args):
    a, a_dir = load_run(args.run_a)
    b, b_dir = load_run(args.run_b)
    a_again = load_run(args.a_again)[0] if args.a_again else None
    noise = read_json(args.noise) if args.noise else None
    report = compare_runs(a, a_dir, b, b_dir, noise, a_again)
    print(json.dumps(report, indent=1) if args.json else "\n".join(report["text"]))
    return report["code"]


def noise_cmd(args):
    if len(args.runs) < 2:
        print("parity_gates: noise needs two runs of one build at least", file=sys.stderr)
        return 2
    noise = make_noise(args.runs)
    write_json(args.out, noise)
    print("noise of %d runs: %d checkpoints, %d flags differ, %d save files differ, %d exact values differ: %s" % (
        len(args.runs), len(noise["checkpoints"]), len(noise["flags"]), len(noise["saves"]), len(noise["exact"]),
        args.out))
    return 0


def nothing_started_cmd(args):
    result, run_dir = load_run(args.run)
    verdict, lines = nothing_started(result, run_dir)
    if args.json:
        print(json.dumps({"verdict": verdict, "lines": lines}, indent=1))
    else:
        print("\n".join(gate_lines("gate 5, nothing started (%s)" % result.get("name"), verdict, lines)))
    return verdict_code([verdict])


# ---- the whole check ----

def side(text, flag):
    label, sep, game = text.partition("=")
    if not sep or not LABEL.fullmatch(label) or not game:
        raise SystemExit("parity_gates: %s LABEL=GAME_FOLDER (a label of a-z, 0-9 and -), not %r" % (flag, text))
    return label, os.path.abspath(game)


def launch(cmd):
    """One queued run, waited for: its exit status. SIGTERM or Ctrl+C here is passed on (the queue's runner stops its
    command, whose run ends its game) and waited for, so no game is left behind"""
    p = subprocess.Popen(cmd)
    try:
        return p.wait()
    except KeyboardInterrupt:
        p.send_signal(signal.SIGTERM)
        try:
            p.wait(180)
        except subprocess.TimeoutExpired:
            p.kill()
            p.wait()
        raise


def ab(args):
    (la, ga), (lb, gb) = side(args.a, "--a"), side(args.b, "--b")
    out = os.path.abspath(args.out or os.path.join(ROOT, "work", "parity", "%s-%s-%s" % (
        la, lb, time.strftime("%Y%m%d-%H%M%S"))))
    a1, b1, a2 = "a1-" + la, "b-" + lb, "a2-" + la   # (a side's prefix keeps two labels' folders apart)
    first, second = os.path.join(out, a1, "result.json"), os.path.join(out, b1, "result.json")
    plan = [(a1, ga, args.env, []), (b1, gb, args.env + args.b_env, [first])]
    if not args.no_aa:
        plan.append((a2, ga, args.env, [first, second]))
    commands = [(name, queue_command(name, inner_command(game, out, name, args.save, watch, args.boot_save, env)))
                for name, game, env, watch in plan]
    why = ram_refusal((("the output folder", out), ("A's game folder", ga), ("B's game folder", gb)))
    if args.dry_run:
        for _, cmd in commands:
            print(" ".join(cmd))
        print("then: compare %s %s%s" % (first, second, "" if args.no_aa else " --a-again %s --noise %s" % (
            os.path.join(out, a2), os.path.join(out, "noise.json"))))
        if WINDOWS or why:
            print("would start: no: %s" % ("not on Windows" if WINDOWS else why))
        return 0
    if WINDOWS:
        print("parity_gates: the gates run on Linux (the bridge's Unix socket, /proc): not on Windows", file=sys.stderr)
        return 2
    if why:
        print("parity_gates: %s" % why, file=sys.stderr)
        return 2
    if os.path.exists(out) and os.listdir(out):
        print("parity_gates: %s is not empty: give another --out" % out, file=sys.stderr)
        return 2
    os.makedirs(out, exist_ok=True)
    catch_stops()
    codes = {}
    for name, cmd in commands:
        print("$ " + " ".join(cmd), flush=True)
        try:
            codes[name] = launch(cmd)
        except KeyboardInterrupt:
            print("parity_gates: stopped during %s" % name, file=sys.stderr)
            return INTERRUPTED
        if not os.path.isfile(os.path.join(out, name, "result.json")):
            print("parity_gates: run %s left no result (exit %d)" % (name, codes[name]), file=sys.stderr)
            return 2
    noise = a_again = None
    if not args.no_aa:
        noise = make_noise([os.path.join(out, a1), os.path.join(out, a2)])
        write_json(os.path.join(out, "noise.json"), noise)
        a_again = load_run(os.path.join(out, a2))[0]
    a, a_dir = load_run(os.path.join(out, a1))
    b, b_dir = load_run(os.path.join(out, b1))
    extra = [(5, "gate 5, nothing started (A: %s)" % a.get("name"), nothing_started(a, a_dir))]
    exits = [(name, code) for name, code in codes.items() if code != 0]
    if exits:   # (a run that exited badly though its result says nothing: never a pass)
        extra.append((0, "the runs' exit status", ("fail" if any(n == b1 for n, _ in exits) else "inconclusive",
                                                    ["%s exited %d" % e for e in exits])))
    report = compare_runs(a, a_dir, b, b_dir, noise, a_again, extra)
    report["exits"] = codes
    write_json(os.path.join(out, "verdict.json"), report)
    with open(os.path.join(out, "verdict.txt"), "w", encoding="utf-8") as f:
        f.write("\n".join(report["text"]) + "\n")
    print("\n".join(report["text"]))
    return report["code"]


def plan(_args):
    print("scenario %d (%s), from the repository's save %s" % (SCENARIO_VERSION, scenario_hash(), SAVE))
    for step in SCENARIO:
        tol, cap = TOLERANCE[step["kind"]], CEILING[step["kind"]]
        what = {"control": "the title pressed through (or --boot-save) to the save's spawn, settled",
                "wait": "%d frames with no input" % step.get("frames", 0),
                "inputs": "presses: %s, then %d frames" % (json.dumps(step.get("steps")), step.get("after", 0)),
                "teleport": "the debug teleport to %s" % step.get("to")}[step["do"]]
        print("  %-11s %s; checkpoint: %s exactly, pos within %.2f m (at most %.2f), facing %.0f deg (%.0f), camera %.0f "
              "deg (%.0f), changed flags" % (step["name"], what, "/".join(EXACT), tol["pos"], cap["pos"], tol["facing"],
                                             cap["facing"], tol["camera"], cap["camera"]))
    print("  then the save folder once it has settled, copied and hashed (gate 4), the game's threads and children "
          "(gate 5), quit")
    print("environment: %s, HOS_MODS=<an empty folder of the run's>; the caller's other %s variables dropped" % (
        " ".join("%s=%s" % kv for kv in sorted(BASE_ENV.items())), "/".join(SWITCH_PREFIXES)))
    print("gate 5 log: required %s; forbidden %s and %s" % (
        ", ".join(p.pattern for _, p in REQUIRED_LOG), MODS_LOADED.pattern, RESERVED_LOG.pattern))
    return 0


def main(argv=None):
    p = argparse.ArgumentParser(prog="parity_gates.py", description=__doc__.split("\n\n")[0],
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)

    def run_flags(q):
        q.add_argument("--save", default=SAVE, help="a save in assets/saves (default %s)" % SAVE)
        q.add_argument("--boot-save", action="store_true", help="HOS_OOT_BOOT_SAVE=1: no presses at the title (#1577)")
        q.add_argument("--env", action="append", default=[], metavar="NAME=VALUE", help="a switch for the run(s)")
        q.add_argument("--dry-run", action="store_true", help="say what would run, with the queue's commands; start nothing")

    q = sub.add_parser("ab", help="A, B and A again through the job queue, then every gate")
    q.add_argument("--a", required=True, metavar="LABEL=GAME", help="the build compared against (main)")
    q.add_argument("--b", required=True, metavar="LABEL=GAME", help="the build checked (the branch)")
    q.add_argument("--out", help="a new folder for the runs (default work/parity/<a>-<b>-<time>)")
    q.add_argument("--no-aa", action="store_true", help="no second run of A: no noise measured")
    q.add_argument("--b-env", action="append", default=[], metavar="NAME=VALUE",
                   help="a switch for B only (a side made different on purpose: the gates' own check)")
    run_flags(q)
    q.set_defaults(func=ab)

    q = sub.add_parser("run", help="one game: the scenario, its checkpoints and saves (inside a job of the queue)")
    q.add_argument("--game", required=True)
    q.add_argument("--out", required=True)
    q.add_argument("--name", required=True, help="the run's folder in --out: a-z, 0-9 and -")
    q.add_argument("--watch-from", action="append", default=[],
                   help="the other build's run: its changed flags are read here too (again for several)")
    run_flags(q)
    q.set_defaults(func=run)

    q = sub.add_parser("compare", help="gates 3, 4 and 5 for two runs")
    q.add_argument("run_a")
    q.add_argument("run_b")
    q.add_argument("--a-again", help="a second run of A, which read the flags B saw change")
    q.add_argument("--noise", help="noise.json: what differs between runs of A")
    q.add_argument("--json", action="store_true")
    q.set_defaults(func=compare)

    q = sub.add_parser("noise", help="what differs between runs of one build")
    q.add_argument("runs", nargs="+")
    q.add_argument("--out", required=True)
    q.set_defaults(func=noise_cmd)

    q = sub.add_parser("nothing-started", help="gate 5 for one run")
    q.add_argument("run")
    q.add_argument("--json", action="store_true")
    q.set_defaults(func=nothing_started_cmd)

    q = sub.add_parser("plan", help="the scenario and the checks; no game")
    q.set_defaults(func=plan)

    args = p.parse_args(argv)
    try:
        return args.func(args)
    except (Failed, OSError, ValueError, KeyError) as e:
        print("parity_gates: %s" % e, file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
