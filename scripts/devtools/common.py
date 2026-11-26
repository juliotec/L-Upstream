"""What the development tools share: the checkout, this PC's development folder (dev_home: everything the tools keep, apart
from the player's data), the RAM file system check and a file lock. Python's standard library only."""
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time

try:
    import fcntl
except ImportError:   # (Windows: msvcrt's lock instead)
    fcntl = None
    import msvcrt

# (this PC's system, for paths and process reads: the checks set it to try the other system's. The lock below goes by the
# system Python runs on, whatever this says)
WINDOWS = os.name == "nt"
SCRIPTS = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ROOT = os.path.dirname(SCRIPTS)
GB = float(1 << 30)


def say(who, text, err=False):
    print("%s: %s" % (who, text), file=sys.stderr if err else sys.stdout, flush=True)


def die(who, text):
    sys.exit("%s: %s" % (who, text))


def dev_home():
    """This PC's development data, apart from the player's (LAYOUT lists it): LONGSHOT_DEV_HOME, else
    $XDG_DATA_HOME/longshot-dev (~/.local/share/longshot-dev; Windows %LOCALAPPDATA%\\longshot-dev). Data, not a cache: its
    bases and stores take hours to make again"""
    h = os.environ.get("LONGSHOT_DEV_HOME")
    if not h and WINDOWS:
        h = os.path.join(os.environ.get("LOCALAPPDATA") or os.path.join(os.path.expanduser("~"), "AppData", "Local"),
                         "longshot-dev")
    elif not h:
        h = os.path.join(os.environ.get("XDG_DATA_HOME") or os.path.join(os.path.expanduser("~"), ".local", "share"),
                         "longshot-dev")
    return os.path.normpath(h) if WINDOWS else h   # (D:/x as D:\x: the paths it gives a build read as one)


# (what dev_home() holds, as `dev.py paths` prints it; docs/dev/reference/build.md, Where dev data lives)
LAYOUT = (
    ("bases", "the shared bases, one per commit of main (`dev.py base`)"),
    ("tools-store", "tools.lock's build tools, one folder per tools.lock, and their downloads (`longshot-builder dev tools install`)"),
    ("store", "longshot-builder's stores every base and pack shares: compile/ (the objects), lifts/ (LONGSHOT_STORE)"),
    ("cargo", "the cargo target folder longshot-builder is compiled in, for every checkout (`dev.py packs run`)"),
    ("queue", "the job queue's history.jsonl, jobs' tmp/ and NVIDIA shader cache nvcache/ (LONGSHOT_QUEUE_STATE)"),
    ("config", "dev_build.json and job_queue.json: your settings for dev.py's builds and the queue"),
    ("builder-cache", "longshot-builder's per-user cache for every dev build (LONGSHOT_CACHE: runtimes, sources, tools), not a player's"),
    ("work", "`mise run build`'s work folders, one per checkout (work-windows: the Windows game built on Linux)"),
    ("sdk", "the vendors' SDKs scripts/fetch.py downloads: ngx-linux, ngx-windows, ffx, xess"),
    ("runs", "game runs' output: longshot_ctl.py's control runs, memtour.py's, cover.py's"),
    ("update", "`dev.py update`'s worktree, work folder, backups of the game folder and history"),
    ("tools", "tools the checks and profiling use: glslang, Tracy's capture tools"),
)


def old_places():
    """{folder or file in dev_home(): where the tools before dev_home() kept it}. `dev.py migrate` moves each into
    dev_home(), and until then dev_path reads it there. The old development folder (bases/, builder/) only while
    LONGSHOT_DEV_HOME is unset: when set, it was and is that folder"""
    if WINDOWS:
        local = os.path.join(os.environ.get("LOCALAPPDATA") or os.path.expanduser("~"), "Longshot")
        conf = os.path.join(os.environ.get("APPDATA") or os.path.expanduser("~"), "Longshot")
        old = {"": os.path.join(local, "dev"), "queue": os.path.join(local, "queue")}
    else:
        cache = os.environ.get("XDG_CACHE_HOME") or os.path.join(os.path.expanduser("~"), ".cache")
        conf = os.path.join(os.environ.get("XDG_CONFIG_HOME") or os.path.join(os.path.expanduser("~"), ".config"),
                            "longshot")
        old = {"": os.path.join(cache, "longshot-dev"), "queue": os.path.join(cache, "longshot-queue")}
    # (these two files only: Tooling's sign-in and chat files share that folder and stay in it)
    for name in ("job_queue.json", "dev_build.json"):
        old["config/" + name] = os.path.join(conf, name)
    if os.environ.get("LONGSHOT_DEV_HOME"):
        del old[""]
    return old


def dev_path(rel):
    """dev_home()/rel (rel with /); while that is not there and its old place is (old_places: not migrated yet), that"""
    new = os.path.join(dev_home(), *rel.split("/"))
    if os.path.lexists(new):
        return new
    for part, old in sorted(old_places().items(), key=lambda x: -len(x[0])):
        if part and rel != part and not rel.startswith(part + "/"):
            continue
        rest = rel[len(part):].strip("/")
        p = os.path.join(old, *rest.split("/")) if rest else old
        return p if os.path.exists(p) else new
    return new


def work_dir(tree, windows=None):
    """A checkout's own work folder for `mise run build` and `dev.py packs run` (longshot-builder's --work): work/<checkout>
    (work-windows/ for the Windows game built on Linux, RX_TARGET=windows), keyed by the checkout's folder, so two checkouts
    (two agents) never build in one"""
    if windows is None:
        windows = not WINDOWS and os.environ.get("RX_TARGET", "").strip().lower() == "windows"
    tree = os.path.abspath(tree)
    name = re.sub(r"[^A-Za-z0-9._-]+", "-", os.path.basename(tree)) or "tree"
    key = "%s-%s" % (name, hashlib.sha1(os.path.normcase(tree).encode("utf-8")).hexdigest()[:8])
    return os.path.join(dev_home(), "work-windows" if windows else "work", key)


def old_data():
    """[(path, what)]: data the tools of before kept and nothing reads now, there on this PC: the development folder's
    builder/ (the build tools before tools-store/) and stores/ (the objects and lifts before store/), and the stores' older
    places outside it (`dev.py bases --prune` lists and removes them)"""
    home = dev_home()
    found = [(os.path.join(home, "builder"), "the build tools before tools-store/"),
             (os.path.join(home, "stores"), "the compiled objects and lifts before store/")]
    if WINDOWS:
        local = os.path.join(os.environ.get("LOCALAPPDATA") or os.path.expanduser("~"), "Longshot")
        found += [(os.path.join(local, "objects"), "the compiled objects of dev.py before the development folder"),
                  (os.path.join(local, "lifts"), "the lifts of dev.py before the development folder")]
    else:
        cache = os.environ.get("XDG_CACHE_HOME") or os.path.join(os.path.expanduser("~"), ".cache")
        found += [(os.path.join(cache, "longshot-objects"), "the compiled objects of dev.py before the development folder"),
                  (os.path.join(cache, "longshot-lifts"), "the lifts of dev.py before the development folder")]
    return [(p, what) for p, what in found if os.path.isdir(p) and not os.path.islink(p)]


def tree_size(path):
    """Bytes under path, each file once (hard links counted once), links not followed"""
    seen, n = set(), 0
    for top, _, files in os.walk(path):
        for f in files:
            try:
                st = os.lstat(os.path.join(top, f))
            except OSError:
                continue
            if (st.st_dev, st.st_ino) not in seen:
                seen.add((st.st_dev, st.st_ino))
                n += st.st_size
    return n


def config_file(name):
    """A settings file in dev_home()/config: job_queue.json, dev_build.json"""
    return dev_path("config/" + name)


def read_config(name):
    """A settings file's object, {} when it is missing or not one"""
    try:
        with open(config_file(name), encoding="utf-8") as f:
            c = json.load(f)
        return c if isinstance(c, dict) else {}
    except (OSError, ValueError):
        return {}


def mounts():
    try:
        with open("/proc/mounts") as f:
            return [(l.split()[1].replace("\\040", " "), l.split()[2]) for l in f if len(l.split()) >= 3]
    except OSError:
        return []


def on_ram(path, table=None):
    """The path is on a RAM file system (tmpfs, ramfs): never for builds, game folders or test output. table: a made-up
    mount table and a path of it as it is, on any OS (the test)"""
    if WINDOWS and table is None:
        return False
    p = os.path.realpath(path) if table is None else path
    best, kind = "", ""
    for mnt, fs in mounts() if table is None else table:
        if (p == mnt or p.startswith(mnt.rstrip("/") + "/")) and len(mnt) >= len(best):
            best, kind = mnt, fs
    return kind in ("tmpfs", "ramfs")


def disk_free_gb(path):
    try:
        return shutil.disk_usage(path).free / GB
    except OSError:
        return None


def ensure_dir(d):
    """d made when it isn't there yet. os.makedirs(d, exist_ok=True) on a folder that is already there still looks at each
    folder above it, and in the Beanstalk agent's Windows sandbox (an AppContainer that may open the folders it was given,
    not see the ones above them) those look missing: it climbs to C:\\ and fails there with "Access is denied"
    """
    if not os.path.isdir(d):
        os.makedirs(d, exist_ok=True)


def ago(sec):
    sec = int(sec)
    return "%d s" % sec if sec < 120 else "%d min" % (sec // 60) if sec < 7200 else "%.1f h" % (sec / 3600)


def output(who, cmd, cwd=ROOT):
    """A command's output; a failure stops the tool with what it said"""
    r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, encoding="utf-8", errors="replace")
    if r.returncode:
        sys.exit("%s: %s failed: %s" % (who, " ".join(cmd[:3]), (r.stderr or r.stdout).strip()[-400:]))
    return r.stdout.strip()


class Lock:
    """An exclusive lock on a file, waited for: flock, or msvcrt's on the file's first byte on Windows (its LK_LOCK gives up
    after 10 s, so a loop). waiting: said once when another holds it; wait=False: BlockingIOError instead of waiting. The job
    queue's .lock and a shared base's <base>.lock are taken this way by every version of these tools"""

    def __init__(self, path, waiting=None, wait=True):
        self.path, self.waiting, self.wait = path, waiting, wait

    def __enter__(self):
        os.makedirs(os.path.dirname(self.path), exist_ok=True)
        self.f = open(self.path, "a+")
        said = False
        while True:
            try:
                if fcntl:
                    fcntl.flock(self.f, fcntl.LOCK_EX | fcntl.LOCK_NB)
                else:
                    self.f.seek(0)
                    msvcrt.locking(self.f.fileno(), msvcrt.LK_NBLCK, 1)
                return self
            except OSError:
                if not self.wait:
                    self.f.close()
                    raise BlockingIOError("%s is held" % self.path)
                if self.waiting and not said:
                    print(self.waiting, flush=True)
                    said = True
                if fcntl:
                    fcntl.flock(self.f, fcntl.LOCK_EX)
                    return self
                time.sleep(0.05)

    def __exit__(self, *exc):
        if not fcntl:
            try:
                self.f.seek(0)
                msvcrt.locking(self.f.fileno(), msvcrt.LK_UNLCK, 1)
            except OSError:
                pass
        self.f.close()
