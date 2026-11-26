"""dev.py's build commands (scripts/devtools/build.py: base, pack, rt, game, vk, env, bases) on both systems, with no build, no network and no game: where its shared and
per-checkout folders go (the development folder, common.dev_home: $XDG_DATA_HOME/longshot-dev, %LOCALAPPDATA%\\longshot-dev,
read from the old places while they are there and the new ones are not; build-linux/ and build-windows/, which git
ignores), the build tools of tools.lock's lines for the system (the variables the builders set), the game dev.py
builds from, the renderer's name and the Windows check of its exports, the development folder's NGX SDK, the shell lines `env` prints, the
prebuilt third_party files a Windows tree gets, and the commands' options.

    python -I -B tests/test_dev_build.py
"""
import contextlib
import hashlib
import importlib.util
import io
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRIPT = os.path.join(ROOT, "scripts", "devtools", "build.py")
sys.path.insert(0, os.path.join(ROOT, "scripts"))
from devtools import build as db, common, jobqueue, packs, procs  # noqa: E402


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


TOOLS_RS = os.path.join(ROOT, "builder", "crates", "build-driver", "src", "tools", "mod.rs")
LOCK_RS = os.path.join(ROOT, "builder", "crates", "build-driver", "src", "tools", "lock.rs")


def installed(test, tree=None):
    """db.toolchain() with its builder and `dev tools install` stood in for: (its env, the install's command line)"""
    ran = []

    def run(cmd, **kw):
        ran.append(cmd)
        return subprocess.CompletedProcess(cmd, 0, "", "")
    with mock.patch.object(packs, "builder", return_value="/b/longshot-builder"), mock.patch.object(db.subprocess, "run", run):
        env = db.toolchain() if tree is None else db.toolchain(tree)
    test.assertEqual(len(ran), 1)
    return env, ran[0]


def lock_key(system):
    with open(os.path.join(ROOT, "tools.lock"), encoding="utf-8") as f:
        lines = [l for l in f if l.startswith("tool ") and " %s " % system in l]
    return hashlib.sha1("".join(lines).encode()).hexdigest()[:12]


class OnSystem(unittest.TestCase):
    windows = False

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="dev_build_test_")
        self.addCleanup(shutil.rmtree, self.tmp, True)
        p = mock.patch.object(common, "WINDOWS", self.windows)
        p.start()
        self.addCleanup(p.stop)
        env = {k: v for k, v in os.environ.items() if not k.startswith(("LONGSHOT_", "XDG_"))}
        env.update(XDG_CACHE_HOME=os.path.join(self.tmp, "xdg"), XDG_CONFIG_HOME=os.path.join(self.tmp, "config"),
                   XDG_DATA_HOME=os.path.join(self.tmp, "data"), LOCALAPPDATA=os.path.join(self.tmp, "local"),
                   APPDATA=os.path.join(self.tmp, "roaming"))
        e = mock.patch.dict(os.environ, env, clear=True)
        e.start()
        self.addCleanup(e.stop)


class Linux(OnSystem):
    windows = False

    def test_folders(self):
        home, x = os.path.join(self.tmp, "data", "longshot-dev"), os.path.join(self.tmp, "xdg")
        self.assertEqual(common.dev_home(), home)
        self.assertEqual(db.base_dir("0123456789abcdef"), os.path.join(home, "bases", "0123456789ab"))
        self.assertEqual(packs.store(), os.path.join(home, "store"))
        self.assertEqual(jobqueue.state_dir(dict(jobqueue.DEFAULTS)), os.path.join(home, "queue"))
        self.assertEqual(common.config_file(db.CONFIG), os.path.join(home, "config", "dev_build.json"))
        # (not migrated yet: each old place read while it is there and its new one is not)
        old = {"bases": os.path.join(x, "longshot-dev", "bases"), "queue": os.path.join(x, "longshot-queue")}
        for p in old.values():
            os.makedirs(p)
        os.makedirs(os.path.join(self.tmp, "config", "longshot"))
        with open(os.path.join(self.tmp, "config", "longshot", db.CONFIG), "w") as f:
            f.write('{"fsr": true}')
        self.assertEqual(db.base_dir("0123456789abcdef"), os.path.join(old["bases"], "0123456789ab"))
        self.assertEqual(jobqueue.state_dir(dict(jobqueue.DEFAULTS)), old["queue"])
        self.assertEqual(db.config(), {"fsr": True})
        self.assertEqual(common.dev_path("work"), os.path.join(home, "work"))   # (no old place)
        os.environ["LONGSHOT_STORE"] = "/elsewhere"
        self.assertEqual(packs.store(), "/elsewhere")
        os.environ["LONGSHOT_DEV_HOME"] = "rel/home"   # (taken as it is: the folder of bases/ and builder/, as before)
        self.assertEqual(common.dev_home(), "rel/home")
        self.assertEqual(db.base_dir("0123456789abcdef"), os.path.join("rel/home", "bases", "0123456789ab"))
        self.assertEqual(db.own_dir(), os.path.join(db.ROOT, "build-linux"))
        self.assertEqual(db.renderer_name(), "longshot_vk.so")
        self.assertEqual(db.game_roots(), [os.path.expanduser("~/Games")])

    def test_tools_lock_linux_lines(self):
        self.assertEqual(db.system(), "linux-x64")
        self.assertEqual(db.tools_lock_key(), lock_key("linux-x64"))
        self.assertEqual(db.builder_dir(), os.path.join(common.dev_home(), "tools-store", lock_key("linux-x64")))

    def test_another_tree_s_tools_lock(self):
        """`dev.py packs run --tree OTHER`: the tools of OTHER's tools.lock, in their own folder, not this checkout's"""
        other = os.path.join(self.tmp, "other")
        os.makedirs(other)
        with open(os.path.join(db.ROOT, "tools.lock"), encoding="utf-8") as f:
            lines = [l for l in f if l.startswith("tool ") and " linux-x64 " in l]
        lines[0] = lines[0].replace(lines[0].split()[2], lines[0].split()[2] + "-other", 1)
        with open(os.path.join(other, "tools.lock"), "w", encoding="utf-8") as f:
            f.write("".join(lines))
        self.assertNotEqual(db.tools_lock_key(other), db.tools_lock_key())
        for d in db.tool_dirs(other).values():
            os.makedirs(d)
        env, cmd = installed(self, other)
        self.assertEqual((cmd[4], cmd[6]), (os.path.join(other, "tools.lock"), db.builder_dir(other)))
        self.assertEqual(env["LONGSHOT_LLVM"], db.tool_dirs(other)["llvm"])
        with open(os.path.join(db.ROOT, "scripts", "devtools", "packs.py"), encoding="utf-8") as f:
            self.assertIn("build.environment(quiet=True, tree=tree)", f.read())

    def test_toolchain_env(self):
        """tools.lock's linux-x64 tools as the variables longshot-builder reads with LONGSHOT_OWN_TOOLS=1 (build-driver's
        tools/mod.rs TOOLS)"""
        dirs = db.tool_dirs()
        for d in dirs.values():
            os.makedirs(d)
        env, cmd = installed(self)
        self.assertEqual(cmd[:3] + cmd[5:9], ["/b/longshot-builder", "dev", "tools", "install", db.builder_dir(), "--system",
                                              "linux-x64"])
        self.assertEqual(env, {"LONGSHOT_OWN_TOOLS": "1", "LONGSHOT_LLVM": dirs["llvm"], "LONGSHOT_SYSROOT": dirs["sysroot"],
                               "LONGSHOT_FFMPEG": dirs["ffmpeg"], "LONGSHOT_SDL2": dirs["sdl2"], "RX_CPM": dirs["cpm"]})
        self.assertEqual(cmd[4], os.path.join(db.ROOT, "tools.lock"))
        with open(TOOLS_RS, encoding="utf-8") as f:
            rs = f.read()
        for name, var in db.TOOL_ENV["linux-x64"].items():
            self.assertIn('("linux-x64", "%s", "%s")' % (name, var), rs)

    def test_the_game(self):
        """The game: LONGSHOT_DEV_GAME, else dev_build.json's "game", else the newest .nup in ~/Games, two folders down"""
        games = os.path.join(self.tmp, "Games")
        os.makedirs(os.path.join(games, "OoT [NUP]"))
        nup = os.path.join(games, "OoT [NUP]", "game.nup")
        open(nup, "wb").close()
        with mock.patch.object(db, "game_roots", return_value=[games]):
            self.assertEqual(db.game_source({}), nup)
            dump = os.path.join(self.tmp, "dump")
            for d in ("romfs", "exefs"):
                os.makedirs(os.path.join(dump, d))
            self.assertEqual(db.game_source({"game": dump}), dump)
            os.environ["LONGSHOT_DEV_GAME"] = nup
            self.assertEqual(db.game_source({"game": dump}), nup)
            os.environ["LONGSHOT_DEV_GAME"] = self.tmp
            with self.assertRaises(SystemExit):
                db.game_source({})
            del os.environ["LONGSHOT_DEV_GAME"]
            os.remove(nup)
            with self.assertRaises(SystemExit):
                db.game_source({})

    def test_upscalers(self):
        with mock.patch.object(db, "config", return_value={}):
            self.assertEqual(db.upscalers({}), "none")
            self.assertEqual(db.upscalers({"LONGSHOT_NGX_SDK": "/sdk"}, fsr=True), "dlss,fsr4")
        with mock.patch.object(db, "config", return_value={"fsr": True}):
            self.assertEqual(db.upscalers({}), "fsr4")

    def test_ngx_and_shell(self):
        ngx, lib = db.ngx_sdk()
        self.assertEqual(ngx, os.path.join(common.dev_home(), "sdk", "ngx-linux"))   # (scripts/fetch.py dlss's default)
        self.assertEqual(lib, "libnvsdk_ngx.a")
        self.assertEqual(db.shell_line("A", "b c'd"), "export A='b c'\"'\"'d'")

    def test_vk_keeps_the_exported_jobs(self):
        """An exported LONGSHOT_JOBS goes through environment() to the build's environment (longshot-builder reads it there),
        while the queue's own LONGSHOT_JOB and LONGSHOT_JOB_* are dropped: a job that kept an outer job's LONGSHOT_JOB would
        take itself for that job"""
        ran = []

        class Queued(Exception):
            pass

        def queued(name, mode, cmd, env, cwd=None):
            ran.append((cmd, env))
            raise Queued

        os.environ.update(LONGSHOT_JOB="4242", LONGSHOT_JOB_NAME="outer", LONGSHOT_JOB_MODE="build", LONGSHOT_JOBS="14")
        with mock.patch.object(db, "own_dir", return_value=self.tmp), mock.patch.object(db, "not_on_ram"), \
                mock.patch.object(db, "config", return_value={}), mock.patch.object(db, "toolchain", return_value={}), \
                mock.patch.object(db, "queued", queued), self.assertRaises(Queued):
            db.cmd_vk(mock.Mock())
        cmd, env = ran[-1]
        self.assertEqual(env.get("LONGSHOT_JOBS"), "14")
        for name in ("LONGSHOT_JOB", "LONGSHOT_JOB_NAME", "LONGSHOT_JOB_MODE"):
            self.assertNotIn(name, env)
        # (longshot-builder's per-user cache: the development folder's, never the player's ~/.cache/longshot-builder)
        self.assertEqual(env.get("LONGSHOT_CACHE"), common.dev_path("builder-cache"))
        # (no data folder: the licences are a profile's, never the PC's)
        self.assertNotIn("LONGSHOT_DATA", env)

    def test_the_queue_only_when_usable_and_wanted(self):
        """packs' builds (longshot-builder's compile, packs run's) are queue jobs only outside a job, unasked otherwise,
        and where the queue can be written: the Test Farm's (Beanstalk: LONGSHOT_BUILD, LONGSHOT_STORE, LONGSHOT_CACHE,
        LONGSHOT_BUILD_JSON=1, in a sandbox with no queue) always runs directly"""
        os.environ["LONGSHOT_QUEUE_DIR"] = os.path.join(self.tmp, "queue-tickets")
        self.assertIsNone(packs.why_direct(dict(os.environ)))
        farm = dict(os.environ, LONGSHOT_BUILD="/farm/longshot-builder", LONGSHOT_STORE="/farm/store",
                    LONGSHOT_CACHE="/farm/cache", LONGSHOT_BUILD_JSON="1")
        self.assertIn("Test Farm", packs.why_direct(farm))
        self.assertEqual(packs.why_direct(dict(os.environ, LONGSHOT_QUEUE="0")), "LONGSHOT_QUEUE=0")
        self.assertEqual(packs.why_direct(dict(os.environ, LONGSHOT_JOB="7")), "inside a job")
        with mock.patch.object(packs, "NO_QUEUE", True):
            self.assertEqual(packs.why_direct(dict(os.environ)), "--no-queue")
        with mock.patch.object(jobqueue, "writable", return_value=False):
            self.assertIn("can't be written", packs.why_direct(dict(os.environ)))
        # (the farm's packs run: directly, never a ticket; a developer's: a job)
        for environ, queued in ((farm, False), (dict(os.environ), True)):
            with mock.patch.object(jobqueue, "run", return_value=0) as run, \
                    mock.patch.object(packs.subprocess, "call", return_value=0) as call, \
                    contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(packs.build_job("lb", ["x"], environ, self.tmp, "d"), 0)
            self.assertEqual((run.called, call.called), (queued, not queued))
        # (a queue folder this process can't write: no exit, only False)
        os.environ["LONGSHOT_QUEUE_DIR"] = os.path.join(self.tmp, "a-file", "tickets")
        with open(os.path.join(self.tmp, "a-file"), "w"):
            pass
        self.assertFalse(jobqueue.writable())

    def test_vk_inside_a_job_never_queues_its_builder(self):
        """`dev.py vk` inside a job (grsp-v1, 2026-11-24): its build environment drops the job's markers, yet the `dev.py
        packs run` it runs directly gets them back, so packs' longshot-builder compile and build run directly too. Before,
        that compile queued a ticket of its own behind a timed run waiting for the job around it: a deadlock"""
        os.environ.update(LONGSHOT_JOB="4242", LONGSHOT_JOB_NAME="grsp-v1", LONGSHOT_JOB_MODE="build",
                          LONGSHOT_QUEUE_DIR=os.path.join(self.tmp, "queue-tickets"))
        ran = []

        class Ran(Exception):
            pass

        def call(cmd, cwd=None, env=None, **k):
            ran.append(env)
            raise Ran

        with mock.patch.object(db, "own_dir", return_value=self.tmp), mock.patch.object(db, "not_on_ram"), \
                mock.patch.object(db, "config", return_value={}), mock.patch.object(db, "toolchain", return_value={}), \
                mock.patch.object(jobqueue, "write_ticket", side_effect=AssertionError("a ticket inside a job")), \
                mock.patch.object(jobqueue.subprocess, "call", call), contextlib.redirect_stderr(io.StringIO()), \
                self.assertRaises(Ran):
            db.cmd_vk(mock.Mock())
        self.assertEqual((ran[-1].get("LONGSHOT_JOB"), ran[-1].get("LONGSHOT_JOB_NAME")), ("4242", "grsp-v1"))
        # (that packs run, in the environment it got: its own build environment drops them once more, its builds still
        # run directly, with them)
        with mock.patch.dict(os.environ, ran[-1], clear=True), mock.patch.object(db, "config", return_value={}), \
                mock.patch.object(db, "toolchain", return_value={}), mock.patch.object(db, "not_on_ram"):
            env = db.environment(quiet=True)
            for k in jobqueue.JOB_MARKERS:
                self.assertNotIn(k, env)
            self.assertEqual(packs.why_direct(env), "inside a job")
            self.assertEqual(env.get("LONGSHOT_JOB_NAME"), "grsp-v1")

    def test_a_job_above_with_no_marker_left(self):
        """packs' builds inside a job whose markers are gone (`env -i`, a caller's own environment) still run directly: a
        running job's ticket, any version's (as main's runner writes it), whose runner is above this process says so"""
        qdir = os.path.join(self.tmp, "queue-tickets")
        os.environ["LONGSHOT_QUEUE_DIR"] = qdir
        self.assertIsNone(packs.why_direct(dict(os.environ)))
        parent = os.getppid()
        jobqueue.write_ticket(qdir, {"name": "grsp-v1", "pid": parent, "start": procs.proc_start(parent), "ns": procs.pidns(),
                                     "mode": "build", "gpu": None, "cpu": "heavy", "prio": "normal", "t": 0,
                                     "state": "active", "ready": True, "idle": False, "group": "", "cmd": "sh v.sh", "cwd": "/"})
        env = dict(os.environ)
        self.assertEqual(packs.why_direct(env), "inside a job")
        self.assertEqual((env["LONGSHOT_JOB"], env["LONGSHOT_JOB_NAME"]), (str(parent), "grsp-v1"))
        with mock.patch.object(jobqueue, "run", side_effect=AssertionError("queued inside a job")), \
                mock.patch.object(packs.subprocess, "call", return_value=0) as call, \
                contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(packs.build_job("lb", ["x"], dict(os.environ), self.tmp, "d"), 0)
        self.assertEqual(call.call_args.kwargs["env"]["LONGSHOT_JOB_NAME"], "grsp-v1")

    def test_a_work_folder_per_checkout(self):
        a, b = os.path.join(self.tmp, "one", "Longshot"), os.path.join(self.tmp, "two", "Longshot")
        self.assertNotEqual(common.work_dir(a), common.work_dir(b))
        self.assertEqual(os.path.dirname(common.work_dir(a)), os.path.join(common.dev_home(), "work"))
        self.assertTrue(os.path.basename(common.work_dir(a)).startswith("Longshot-"))
        self.assertEqual(os.path.dirname(common.work_dir(a, windows=True)), os.path.join(common.dev_home(), "work-windows"))
        with open(os.path.join(db.ROOT, "scripts", "devtools", "packs.py"), encoding="utf-8") as f:
            src = f.read()
        self.assertIn('args += ["--work", common.work_dir(tree)]', src)
        self.assertIn("if args[0] in RUNS_IN_TREE and not any(", src, "an update gets the dev work folder too")

    def test_prune_lists_then_removes_what_nothing_reads(self):
        os.environ["LONGSHOT_QUEUE_DIR"] = os.path.join(self.tmp, "queue-tickets")
        home = common.dev_home()
        old_base, new_base = os.path.join(home, "bases", "aaaaaaaaaaaa"), os.path.join(home, "bases", "bbbbbbbbbbbb")
        for d, b in ((old_base, {"commit": "a" * 40}), (new_base, {"commit": "b" * 40, "game_source": "g.nup"})):
            os.makedirs(d)
            with open(os.path.join(d, "base.json"), "w") as f:
                f.write(__import__("json").dumps(b))
        for d in ("builder/k", "stores/objects", "tools-store/k"):
            os.makedirs(os.path.join(home, d))
            with open(os.path.join(home, d, "f"), "w") as f:
                f.write("x" * 10)
        os.makedirs(os.path.join(self.tmp, "xdg", "longshot-lifts"))
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            db.cmd_bases(mock.Mock(prune=True, yes=False))
        listed = out.getvalue()
        for p in (old_base, os.path.join(home, "builder"), os.path.join(home, "stores"),
                  os.path.join(self.tmp, "xdg", "longshot-lifts")):
            self.assertIn(p, listed)
            self.assertTrue(os.path.isdir(p), "a dry run removes nothing")
        self.assertNotIn(new_base, listed)
        self.assertNotIn(os.path.join(home, "tools-store"), listed)
        with contextlib.redirect_stdout(io.StringIO()):
            db.cmd_bases(mock.Mock(prune=True, yes=True))
        self.assertFalse(os.path.exists(old_base) or os.path.exists(os.path.join(home, "builder")))
        self.assertTrue(os.path.isdir(new_base) and os.path.isdir(os.path.join(home, "tools-store")))


class Windows(OnSystem):
    windows = True

    def test_folders(self):
        local = os.path.join(self.tmp, "local")
        home = os.path.normpath(os.path.join(local, "longshot-dev"))
        self.assertEqual(common.dev_home(), home)
        self.assertEqual(packs.store(), os.path.join(home, "store"))
        self.assertEqual(jobqueue.state_dir(dict(jobqueue.DEFAULTS)), os.path.join(home, "queue"))
        self.assertEqual(common.config_file(db.CONFIG), os.path.join(home, "config", "dev_build.json"))
        # (the old places, in %LOCALAPPDATA%\Longshot beside the player's and %APPDATA%\Longshot, until migrated)
        os.makedirs(os.path.join(local, "Longshot", "dev", "bases"))
        os.makedirs(os.path.join(self.tmp, "roaming", "Longshot"))
        open(os.path.join(self.tmp, "roaming", "Longshot", db.CONFIG), "w").close()
        self.assertEqual(db.base_dir("0123456789abcdef"), os.path.join(local, "Longshot", "dev", "bases", "0123456789ab"))
        self.assertEqual(common.config_file(db.CONFIG), os.path.join(self.tmp, "roaming", "Longshot", "dev_build.json"))
        self.assertEqual(db.own_dir(), os.path.join(db.ROOT, "build-windows"))
        self.assertEqual(db.renderer_name(), "longshot_vk.dll")
        self.assertIn(os.path.expanduser(os.path.join("~", "Games", "Longshot")), db.game_roots())

    def test_build_windows_ignored(self):
        with open(os.path.join(ROOT, ".gitignore"), encoding="utf-8") as f:
            self.assertIn("build-windows/", f.read().split())

    def test_tools_lock_windows_lines(self):
        self.assertEqual(db.system(), "windows-x64")
        self.assertEqual(db.tools_lock_key(), lock_key("windows-x64"))
        self.assertNotEqual(db.tools_lock_key(), lock_key("linux-x64"))
        dirs = db.tool_dirs()
        with open(os.path.join(ROOT, "tools.lock"), encoding="utf-8") as f:
            names = [l.split()[1] for l in f if l.startswith("tool ") and l.split()[3] == "windows-x64"]
        for name in names:   # (each line's dest is tools/<name>: build-driver's tools/lock.rs test)
            self.assertEqual(dirs[name], os.path.join(db.builder_dir(), "tools", name))
        # (every tool the Windows builder carries has its variable, as build-driver's tools/mod.rs names it, and is one
        # tools/lock.rs's REQUIRED names)
        self.assertEqual(sorted(db.TOOL_ENV["windows-x64"]), sorted(names))
        with open(LOCK_RS, encoding="utf-8") as f:
            self.assertIn('("windows-x64", &["llvm-mingw", "cpm", "sdl"])', f.read())
        with open(TOOLS_RS, encoding="utf-8") as f:
            rs = f.read()
        for name, var in db.TOOL_ENV["windows-x64"].items():
            self.assertIn('("windows-x64", "%s", "%s")' % (name, var), rs)

    def test_windows_toolchain_env(self):
        dirs = db.tool_dirs()
        for d in dirs.values():
            os.makedirs(d)
        env, cmd = installed(self)
        self.assertIn("windows-x64", cmd)
        self.assertEqual(env, {"LONGSHOT_OWN_TOOLS": "1", "LONGSHOT_MINGW": dirs["llvm-mingw"], "RX_CPM": dirs["cpm"],
                               "LONGSHOT_SDL_DIR": dirs["sdl"]})

    def test_ngx_and_shell(self):
        ngx, lib = db.ngx_sdk()
        self.assertEqual(ngx, os.path.join(common.dev_home(), "sdk", "ngx-windows"))
        with open(os.path.join(ROOT, "builder", "crates", "build-compile", "src", "packages.rs"), encoding="utf-8") as f:
            self.assertIn('"%s"' % lib, f.read())
        self.assertEqual(db.shell_line("A", r"C:\x y\it's"), r"$env:A = 'C:\x y\it''s'")

    def test_dll_exports(self):
        text = ("\nThe Data Directory\n\nExport Table:\n DLL name: longshot_vk.dll\n Ordinal base: 1\n"
                " Ordinal      RVA  Name\n       1 0x27cf70  mkwvk_close_asked\n       2 0x27cff0  mkwvk_init\n\n"
                "The Import Tables:\n  lookup 0x1 time 0x0 fwd 0x0 name 0x2 addr 0x3\n")
        self.assertEqual(db.dll_exports(text), {"mkwvk_close_asked", "mkwvk_init"})
        self.assertEqual(db.dll_exports(text.replace("mkwvk_init", "other")), {"mkwvk_close_asked", "other"})
        with open(os.path.join(ROOT, "host", "rx_backend.c"), encoding="utf-8") as f:
            host = f.read()
        for name in db.RENDERER_EXPORTS:
            self.assertIn('GetProcAddress(m, "%s")' % name, host)


class Both(unittest.TestCase):
    def test_packs_of_a_checkout(self):
        """A base's checkout runs its own `dev.py packs`"""
        d = os.path.join("some", "checkout")
        self.assertEqual(db.packs_cmd(d, "rt", "b", "r"),
                         [sys.executable, "-I", os.path.join(d, "scripts", "dev.py"), "packs", "rt", "b", "r"])

    def test_commands(self):
        """dev.py's build commands and the few options each takes, and the `dev.py packs` command lines the Test Farm runs
        (Beanstalk: `packs base <dir> --game G --tree T [--seed S] [--upscaler U]`, `packs pack <dir> pr --tree T`,
        `packs rt <dir> <game> [pr]`), and `packs run [--tree T] -- ARGS`"""
        dev = load("dev_for_build_test", os.path.join(ROOT, "scripts", "dev.py"))
        p = dev.parser()
        for argv, func in ((["base"], db.cmd_base), (["base", "main"], db.cmd_base), (["pack"], db.cmd_pack),
                           (["rt", "--empty"], db.cmd_rt), (["game", "--quick", "--fsr"], db.cmd_game),
                           (["vk"], db.cmd_vk), (["env"], db.cmd_env), (["bases"], db.cmd_bases)):
            self.assertIs(p.parse_args(argv).func, func, argv)
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            p.parse_args(["game", "--quick", "--timing"])   # (a -O1 build is never measured)
        a = p.parse_args(["packs", "base", "b", "--game", "g.nup", "--tree", "t", "--seed", "s", "--upscaler", "fsr4"])
        self.assertEqual((a.what, a.dir, a.game, a.tree, a.seed, a.upscaler), ("base", "b", "g.nup", "t", "s", "fsr4"))
        a = p.parse_args(["packs", "run", "--tree", "t"])
        self.assertEqual((a.what, a.tree, a.takes_command), ("run", "t", True))
        self.assertTrue(p.parse_args(["packs", "--no-queue", "run"]).no_queue)
        self.assertFalse(p.parse_args(["packs", "run"]).no_queue)
        a = p.parse_args(["packs", "pack", "b", "pr", "--tree", "t"])
        self.assertEqual((a.what, a.dir, a.name, a.tree), ("pack", "b", "pr", "t"))
        for packs_given in ([], ["pr"]):
            a = p.parse_args(["packs", "rt", "b", "g"] + packs_given)
            self.assertEqual((a.what, a.dir, a.runtime, a.packs), ("rt", "b", "g", packs_given))
        self.assertEqual(p.parse_args(["packs", "warm", "b", "--run", "x {runtime}"]).run, ["x {runtime}"])
        for argv in (["base", "b", "--game", "g"], ["pack", "b", "pr"], ["rt", "b", "g"], ["warm", "b", "--run", "x"], ["run"]):
            self.assertIs(p.parse_args(["packs"] + argv).func, packs.command, argv)

    def test_no_refusal(self):
        with open(SCRIPT, encoding="utf-8") as f:
            self.assertNotIn("Linux only", f.read())

    def test_base_lock(self):
        d = tempfile.mkdtemp(prefix="dev_build_lock_")
        self.addCleanup(shutil.rmtree, d, True)
        with db.base_lock(os.path.join(d, "bases", "abc")) as l:
            self.assertTrue(l.path.endswith(os.path.join("bases", "abc.lock")))
        with db.base_lock(os.path.join(d, "bases", "abc") + os.sep):   # (again, after it was let go)
            pass

    def test_vk_is_the_renderer_alone(self):
        """`dev.py vk`: longshot-builder's compile of the renderer alone (--only renderer), through `dev.py packs run`, in the
        queue's light mode"""
        ran = []

        class Queued(Exception):
            pass

        def queued(name, mode, cmd, env, cwd=None):
            ran.append((mode, cmd, env))
            raise Queued

        d = tempfile.mkdtemp(prefix="dev_build_vk_")
        self.addCleanup(shutil.rmtree, d, True)
        with mock.patch.object(db, "own_dir", return_value=d), mock.patch.object(db, "not_on_ram"), \
                mock.patch.object(db, "queued", queued), mock.patch.object(db, "config", return_value={}), \
                mock.patch.object(db, "environment", return_value={}), self.assertRaises(Queued):
            db.cmd_vk(mock.Mock())
        mode, cmd, env = ran[-1]
        self.assertEqual(mode, "light")
        self.assertEqual(cmd[:7], [sys.executable, "-I", os.path.join(db.ROOT, "scripts", "dev.py"), "packs", "run", "--",
                                   "compile"])
        self.assertEqual(cmd[cmd.index("--only") + 1], "renderer")
        self.assertEqual(env.get("RX_RELEASE"), "1")


if __name__ == "__main__":
    unittest.main()
