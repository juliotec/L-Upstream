"""The parity gates (scripts/parity_gates.py, docs/dev/reference/parity-gates.md) without the game: what gate 3 compares exactly and what
within a tolerance (a value that is not a number never passes; two runs of one build widen a tolerance up to its ceiling and
no further, and fail it when they are further apart than that; noise of another scenario or of unfinished runs is not used),
flags only one side saw change (a value from a second run of A, else inconclusive), gate 4's save comparison (a save the run
wrote on both sides, the copy taken when the game had stopped writing, caption.sav's date and picture left out and nothing
else), gate 5's rules over a log, folders and a process's threads and children (unreadable ones fail), the noise of runs of
one build, a run's result written whatever ends it (an exception, SIGTERM), ab's exit status from its runs, and that a game
run starts only inside a --mode shared job of the PC's queue, on Linux, never on a RAM file system, without the caller's own
switches. Made-up results and saves only.

python -I -B tests/test_parity_gates.py
"""
import contextlib
import copy
import importlib.util
import io
import json
import os
import shutil
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


pg = load("parity_gates", ROOT / "scripts" / "parity_gates.py")
saves = pg.module("longshot_saves")


def point(name, kind="spawn", **kw):
    p = {"name": name, "kind": kind, "map": "KokiriForest_Child", "pos": [1.0, 0.0, 2.0], "facing": 90.0,
         "camera": 45.0, "age": "child", "life": 12, "max_life": 12, "rupees": 0, "event": False, "loading": False,
         "changed": {}, "values": {}, "more": 0}
    p.update(kw)
    return p


def result(points, name="a", error=None):
    return {"scenario_hash": pg.scenario_hash(), "name": name, "checkpoints": points, "steps": len(points),
            "error": error, "saves": {"path": "saves"}, "log": "log.txt"}


def caption(jpeg, scene=b"KokiriForest", year=2026, second=7):
    """A made-up caption.sav in the layout longshot_saves.py knows (as tests/test_longshot_saves.py makes one)"""
    head = bytearray(saves.CAPTION_JPEG)
    head[:4] = saves.CAPTION_MAGIC
    head[saves.CAPTION_CHECK:saves.CAPTION_CHECK + 4] = saves.CAPTION_JPEG_LEN.to_bytes(4, "little")
    date = {"year": year, "month": 11, "day": 14, "hour": 9, "minute": 41, "second": second}
    for i, (field, key) in enumerate(saves.CAPTION_DATE_KEYS):
        at = 280 + 8 * i
        head[at:at + 8] = key.to_bytes(4, "little") + date[field].to_bytes(4, "little")
    head[saves.CAPTION_SCENE:saves.CAPTION_SCENE + len(scene)] = scene
    head[saves.CAPTION_MAP:saves.CAPTION_MAP + 8] = b"Map_Room"
    head[saves.CAPTION_JPEG_LEN:saves.CAPTION_JPEG] = len(jpeg).to_bytes(4, "little")
    return bytes(head) + jpeg


PROGRESS = bytes(range(256)) * 4
NONE = "[hos main] mods: none in /x/run/no-mods (a mod: <name>\\romfs)\n[hos main] vk: 10 presents\n"
GOOD = {"read": True, "threads": ["Game", "sead::MainThread"], "children": []}


def make_run(top, name, points, files=None, written=None, log=NONE, evidence=GOOD, error=None, changed_after=()):
    """A run folder as run writes it: result.json, saves/ (its hashes as taken at settle time), log.txt"""
    run = os.path.join(top, name)
    os.makedirs(os.path.join(run, "saves"))
    files = {"slot_01/progress.sav": PROGRESS} if files is None else files
    for rel, data in files.items():
        p = os.path.join(run, "saves", *rel.split("/"))
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(data)
    with open(os.path.join(run, "log.txt"), "w", encoding="utf-8") as f:
        f.write(log)
    r = result(points, name, error)
    r["saves"] = {"path": "saves", "files": {rel: pg.sha256(d) for rel, d in files.items()},
                  "written": sorted(files) if written is None else written, "changed_after": list(changed_after)}
    r.update(game=os.path.join(top, "game-" + name), user=os.path.join(run, "user"))
    if evidence is not None:
        r["evidence"] = dict(evidence, cwd=os.path.join(top, "game-" + name))
    with open(os.path.join(run, "result.json"), "w", encoding="utf-8") as f:
        json.dump(r, f)
    return run


class Behaviour(unittest.TestCase):
    """Gate 3"""

    def check(self, a_points, b_points, noise=None, a_error=None, b_error=None, a_again=None):
        return pg.compare_behaviour(result(a_points, "a", a_error), result(b_points, "b", b_error), noise,
                                    result(a_again, "a2") if a_again is not None else None)

    def noise(self, **kw):
        n = {"scenario_hash": pg.scenario_hash(), "runs": ["x", "y"], "checkpoints": {}, "flags": [], "saves": {},
             "exact": [], "unfinished": []}
        n.update(kw)
        return n

    def test_the_same_run_passes(self):
        pts = [point("start"), point("walk", "moved")]
        verdict, lines = self.check(pts, copy.deepcopy(pts))
        self.assertEqual(verdict, "pass", lines)

    def test_the_game_values_are_exact(self):
        for key, value in (("map", "HyruleField_Child"), ("age", "adult"), ("life", 11), ("max_life", 16),
                           ("rupees", 5)):
            verdict, lines = self.check([point("start")], [point("start", **{key: value})])
            self.assertEqual(verdict, "fail", key)
            self.assertIn(key, "\n".join(lines))

    def test_position_within_its_kinds_tolerance(self):
        self.assertEqual(self.check([point("walk", "moved")], [point("walk", "moved", pos=[1.2, 0.0, 2.1])])[0], "pass")
        verdict, lines = self.check([point("start")], [point("start", pos=[1.2, 0.0, 2.1])])
        self.assertEqual(verdict, "fail")
        self.assertIn("pos", lines[-1])

    def test_not_a_number_never_passes(self):
        nan = float("nan")
        for key, value in (("pos", [nan, 0.0, 2.0]), ("facing", nan), ("camera", float("inf"))):
            verdict, lines = self.check([point("start")], [point("start", **{key: value})])
            self.assertEqual(verdict, "fail", (key, lines))
        self.assertEqual(self.check([point("start", facing=nan)], [point("start", facing=nan)])[0], "fail")
        self.assertEqual(self.check([point("start")], [point("start", pos=["x", 0, 0])])[0], "fail")

    def test_angles_wrap(self):
        self.assertEqual(self.check([point("start", facing=359.5)], [point("start", facing=0.2)])[0], "pass")

    def test_noise_widens_up_to_the_ceiling(self):
        far = [point("start", pos=[1.12, 0.0, 2.0])]   # (0.12 m: past 0.05, under the spawn's ceiling of 0.15)
        spread = lambda v: self.noise(checkpoints={"start": {"kind": "spawn", "pos": v}})  # noqa: E731
        self.assertEqual(self.check([point("start")], far, spread(0.05))[0], "pass")
        self.assertEqual(self.check([point("start")], far, spread(0.0))[0], "fail")
        farther = [point("start", pos=[1.2, 0.0, 2.0])]   # (0.2 m: 3 x 0.06 = 0.18 would allow it; the ceiling does not)
        self.assertEqual(self.check([point("start")], farther, spread(0.06))[0], "fail")
        self.assertEqual(pg.tolerance("spawn", "pos", 1.0), pg.CEILING["spawn"]["pos"])
        self.assertEqual(pg.tolerance("spawn", "pos", float("nan")), pg.TOLERANCE["spawn"]["pos"])

    def test_two_runs_of_a_past_the_ceiling_fail(self):
        pts = [point("start")]
        verdict, lines = self.check(pts, pts, self.noise(checkpoints={"start": {"kind": "spawn", "pos": 0.4}}))
        self.assertEqual(verdict, "fail")
        self.assertIn("past the most", "\n".join(lines))
        inf = self.noise(checkpoints={"start": {"kind": "spawn", "pos": float("inf")}})
        self.assertEqual(self.check(pts, pts, inf)[0], "fail")
        self.assertEqual(self.check(pts, pts, self.noise(exact=[["start", "map ('a', 'b')"]]))[0], "fail")

    def test_noise_that_cannot_be_used(self):
        pts = [point("start")]
        far = [point("start", pos=[1.12, 0.0, 2.0])]
        wide = {"checkpoints": {"start": {"kind": "spawn", "pos": 0.05}}}
        for bad in ({"scenario_hash": "0" * 16}, {"unfinished": ["a2-main"]},
                    {"flags": ["0x%08x" % i for i in range(40)]}):
            self.assertEqual(self.check(pts, pts, self.noise(**bad))[0], "inconclusive", bad)
            self.assertEqual(self.check(pts, far, self.noise(**dict(wide, **bad)))[0], "fail", bad)   # (widens nothing)

    def test_flags_exact_and_noise_left_out(self):
        a = [point("start", changed={"0x00000001": 1, "0x00000002": 5})]
        b = [point("start", changed={"0x00000001": 1, "0x00000002": 6})]
        self.assertEqual(self.check(a, b)[0], "fail")
        self.assertEqual(self.check(a, b, self.noise(flags=["0x00000002"]))[0], "pass")

    def test_a_flag_only_b_changed(self):
        b = [point("start", changed={"0x0000abcd": 1})]
        verdict, lines = self.check([point("start")], b)
        self.assertEqual(verdict, "inconclusive")   # (A's first run never read it: not a fail)
        self.assertIn("--watch-from", "\n".join(lines))
        self.assertEqual(self.check([point("start")], b, a_again=[point("start", values={"0x0000abcd": 1})])[0], "pass")
        self.assertEqual(self.check([point("start")], b, a_again=[point("start", values={"0x0000abcd": 0})])[0], "fail")
        self.assertEqual(self.check([point("start")], b, a_again=[point("start", values={"0x0000abcd": None})])[0],
                         "fail")   # (A's game has no such flag)

    def test_a_flag_only_a_changed_is_read_in_b(self):
        a = [point("start", changed={"0x00000005": 2})]
        self.assertEqual(self.check(a, [point("start", values={"0x00000005": 2})])[0], "pass")
        self.assertEqual(self.check(a, [point("start", values={"0x00000005": 0})])[0], "fail")
        self.assertEqual(self.check(a, [point("start")])[0], "inconclusive")   # (B ran without --watch-from)

    def test_values_read_now_win_over_the_last_change(self):
        a = [point("start", changed={"0x00000001": 3}, values={"0x00000001": 0})]
        self.assertEqual(self.check(a, [point("start", values={"0x00000001": 0})])[0], "pass")

    def test_unreported_changes_are_inconclusive(self):
        self.assertEqual(self.check([point("start", more=3)], [point("start")])[0], "inconclusive")

    def test_every_checkpoint_on_both_sides(self):
        pts = [point("start"), point("field")]
        short = result(pts[:1], "b")
        short["steps"] = 2
        self.assertEqual(pg.compare_behaviour(result(pts), short)[0], "fail")   # (no error said: still short)
        self.assertEqual(self.check(pts, pts[:1], b_error="RuntimeError: x")[0], "fail")
        self.assertEqual(self.check(pts[:1], pts, a_error="RuntimeError: x")[0], "inconclusive")

    def test_other_scenarios_are_not_compared(self):
        a, b = result([point("start")]), result([point("start")])
        b["scenario_hash"] = "0" * 16
        self.assertEqual(pg.compare_behaviour(a, b)[0], "inconclusive")


class Saves(unittest.TestCase):
    """Gate 4"""

    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp)
        self.n = 0

    def pair(self, fa, fb, noise=None, **kw_b):
        self.n += 1
        a = make_run(self.tmp, "a%d" % self.n, [], fa)
        b = make_run(self.tmp, "b%d" % self.n, [], fb, **kw_b)
        return a, b

    def check(self, fa, fb, noise=None, **kw_b):
        a, b = self.pair(fa, fb, **kw_b)
        return pg.compare_saves(pg.load_run(a)[0], a, pg.load_run(b)[0], b, noise)

    def test_identical_passes(self):
        files = {"slot_01/progress.sav": PROGRESS, "slot_01/caption.sav": caption(saves.grey_jpeg(64, 32))}
        verdict, lines = self.check(files, dict(files))
        self.assertEqual(verdict, "pass", lines)

    def test_caption_date_and_picture_left_out(self):
        a = {"slot_01/caption.sav": caption(saves.grey_jpeg(64, 32), year=2026, second=7)}
        longer = saves.grey_jpeg(64, 32, level=90)
        longer = longer[:2] + b"\xff\xfe\x00\x05abc" + longer[2:]   # (a comment segment: another picture, another length)
        verdict, lines = self.check(a, {"slot_01/caption.sav": caption(longer, year=2027, second=9)})
        self.assertEqual(verdict, "pass", lines)
        self.assertIn("date and picture", "\n".join(lines))

    def test_caption_place_and_picture_size_compared(self):
        a = {"slot_01/caption.sav": caption(saves.grey_jpeg(64, 32))}
        self.assertEqual(self.check(a, {"slot_01/caption.sav": caption(saves.grey_jpeg(64, 32), scene=b"Market")})[0],
                         "fail")
        self.assertEqual(self.check(a, {"slot_01/caption.sav": caption(saves.grey_jpeg(32, 32))})[0], "fail")

    def test_progress_byte_for_byte(self):
        b = bytearray(PROGRESS)
        b[100] ^= 1
        verdict, lines = self.check({"slot_01/progress.sav": PROGRESS}, {"slot_01/progress.sav": bytes(b)})
        self.assertEqual(verdict, "fail")
        self.assertIn("100-100", "\n".join(lines))
        noise = {"scenario_hash": pg.scenario_hash(), "saves": {"slot_01/progress.sav": [[96, 104]]}}
        self.assertEqual(self.check({"slot_01/progress.sav": PROGRESS}, {"slot_01/progress.sav": bytes(b)}, noise)[0],
                         "pass")

    def test_a_file_on_one_side_fails(self):
        verdict, lines = self.check({"slot_01/progress.sav": PROGRESS},
                                    {"slot_01/progress.sav": PROGRESS, "slot_02/progress.sav": PROGRESS})
        self.assertEqual(verdict, "fail")
        self.assertIn("only in B", "\n".join(lines))

    def test_a_save_the_run_wrote_is_needed(self):
        files = {"slot_01/progress.sav": PROGRESS}
        self.assertEqual(self.check(files, files, written=[])[0], "fail")   # (A saved, B did not)
        self.n += 1
        a = make_run(self.tmp, "a%d" % self.n, [], files, written=[])
        b = make_run(self.tmp, "b%d" % self.n, [], files, written=[])
        self.assertEqual(pg.compare_saves(pg.load_run(a)[0], a, pg.load_run(b)[0], b)[0], "inconclusive")

    def test_empty_folders_are_inconclusive(self):
        self.assertEqual(self.check({}, {})[0], "inconclusive")

    def test_the_copy_is_what_was_hashed_at_settle_time(self):
        a, b = self.pair({"slot_01/progress.sav": PROGRESS}, {"slot_01/progress.sav": PROGRESS})
        with open(os.path.join(b, "saves", "slot_01", "progress.sav"), "ab") as f:
            f.write(b"later")
        self.assertEqual(pg.compare_saves(pg.load_run(a)[0], a, pg.load_run(b)[0], b)[0], "inconclusive")
        self.assertEqual(self.check({"slot_01/progress.sav": PROGRESS}, {"slot_01/progress.sav": PROGRESS},
                                    changed_after=["slot_01/progress.sav"])[0], "inconclusive")

    def test_written_by_run(self):
        source = {"slot_00/progress.sav": PROGRESS, "slot_00/caption.sav": b"c"}
        now = dict(source, **{"account.sav": b"a", "slot_01/progress.sav": PROGRESS})
        self.assertEqual(pg.written_by_run(now, source), ["slot_01/progress.sav"])
        self.assertEqual(pg.written_by_run(dict(now, **{"slot_00/caption.sav": b"d"}), source),
                         ["slot_00/caption.sav", "slot_01/progress.sav"])


class NothingStarted(unittest.TestCase):
    """Gate 5"""

    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp)
        self.game = os.path.join(self.tmp, "game")
        self.user = os.path.join(self.tmp, "run", "user")
        os.makedirs(os.path.join(self.game, "cache"))
        os.makedirs(self.user)

    def check(self, log, threads=("Game", "sead::MainThread"), children=(), read=True, evidence=True):
        run = os.path.join(self.tmp, "run")
        with open(os.path.join(run, "log.txt"), "w", encoding="utf-8") as f:
            f.write(log)
        r = {"name": "b", "log": "log.txt", "game": self.game, "user": self.user}
        if evidence:
            r["evidence"] = {"read": read, "threads": list(threads), "children": list(children), "cwd": self.game}
        return pg.nothing_started(r, run)

    def test_a_vanilla_run_passes(self):
        verdict, lines = self.check(NONE)
        self.assertEqual(verdict, "pass", lines)

    def test_the_loaders_line_is_required(self):
        self.assertEqual(self.check("[hos main] vk: 10 presents\n")[0], "fail")

    def test_a_run_with_mods_is_not_the_gates(self):
        verdict, _ = self.check(NONE.replace("none in", "2 mods from") + "[hos main] mods: a: 3 RomFS files\n")
        self.assertIn(verdict, ("fail", "inconclusive"))
        # (the loader's none there as well: a mod's line alone makes the run not one with no mods)
        for line in ("[hos main] mods: a: 3 RomFS files", "[hos main] mods: 2 mods from /m", "[hos io] mods: x.bin <- a.bin"):
            self.assertEqual(self.check(NONE + line + "\n")[0], "inconclusive", line)

    def test_reserved_lines(self):
        for line in ("[hos main] script runtime: started for 0 mods", "[hos io] modcache: wrote 3 files",
                     "[hos main] broker: started (pid 7)"):
            self.assertEqual(self.check(NONE + line + "\n")[0], "fail", line)

    def test_a_merged_file_cache_folder(self):
        os.makedirs(os.path.join(self.user, "modcache"))
        verdict, lines = self.check(NONE)
        self.assertEqual(verdict, "fail")
        self.assertIn("modcache", "\n".join(lines))

    def test_threads_and_children(self):
        self.assertEqual(self.check(NONE, threads=("Game", "lsmod-rt"))[0], "fail")
        self.assertEqual(self.check(NONE, children=({"pid": 9, "name": "longshot-broker"},))[0], "fail")
        verdict, lines = self.check(NONE, children=({"pid": 9, "name": "something"},))
        self.assertEqual(verdict, "pass")
        self.assertIn("not the mod system's", "\n".join(lines))

    def test_unread_threads_or_children_fail(self):
        self.assertEqual(self.check(NONE, read=False)[0], "fail")
        self.assertEqual(self.check(NONE, evidence=False)[0], "inconclusive")   # (the run never reached its end)

    def test_process_evidence(self):
        if os.path.isdir("/proc/self/task"):
            self.assertTrue(pg.process_evidence(os.getpid())["threads"])
        self.assertFalse(pg.process_evidence(2 ** 30)["read"])


class Noise(unittest.TestCase):
    def test_runs_of_one_build(self):
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp)
        runs = []
        for i, (x, flag, byte, rupees) in enumerate(((1.0, 5, 0, 0), (1.04, 6, 1, 1))):
            data = bytearray(PROGRESS)
            data[200] = byte
            runs.append(make_run(tmp, "a%d" % i, [point("walk", "moved", pos=[x, 0.0, 2.0], rupees=rupees,
                                                        changed={"0x00000002": flag, "0x00000003": 1})],
                                 {"slot_01/progress.sav": bytes(data)}))
        noise = pg.make_noise(runs)
        self.assertEqual(noise["flags"], ["0x00000002"])
        self.assertAlmostEqual(noise["checkpoints"]["walk"]["pos"], 0.04, places=3)
        self.assertEqual(noise["saves"]["slot_01/progress.sav"], [[200, 201]])
        self.assertEqual(len(noise["exact"]), 1)
        self.assertEqual(noise["unfinished"], [])

    def test_caption_date_and_picture_are_not_noise(self):
        """two runs of one build write captions of other dates and pictures of one length: gate 4 leaves those out, so the
        noise does too (counted, a picture's bytes would pass the cap and make every gate 4 inconclusive)"""
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp)
        jpeg = saves.grey_jpeg(64, 32)
        other = bytearray(jpeg)
        for i in range(len(other) // 2, len(other) // 2 + 100):
            other[i] ^= 0x55
        runs = [make_run(tmp, "a%d" % i, [point("walk", "moved", pos=[1.0, 0.0, 2.0])],
                         {"slot_01/caption.sav": caption(pic, second=s)})
                for i, (pic, s) in enumerate(((jpeg, 7), (bytes(other), 9)))]
        noise = pg.make_noise(runs)
        self.assertNotIn("slot_01/caption.sav", noise["saves"])
        self.assertEqual([x for x in pg.noise_problems(noise, pg.read_json(os.path.join(runs[0], "result.json")))
                          if x[0] == "inconclusive"], [])


class Runs(unittest.TestCase):
    """A run's result is written whatever ends it, and the game's session is stopped"""

    def drive(self, fake):
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp)
        res = {"name": "x", "checkpoints": [], "error": None}
        args = SimpleNamespace(save="kokiri-start", boot_save=False)
        with mock.patch.object(pg, "ctl", return_value=fake), mock.patch.object(pg, "hold_stops"), \
                contextlib.redirect_stderr(io.StringIO()):
            code = pg.drive(args, tmp, tmp, {}, {}, res)
        return code, pg.read_json(os.path.join(tmp, "result.json"))

    def failing(self, error):
        fake = mock.Mock()
        fake.start_session.side_effect = error
        return fake

    def test_any_exception_is_recorded(self):
        code, r = self.drive(self.failing(RuntimeError("boom")))
        self.assertEqual(code, 2)
        self.assertEqual(r["error"], "RuntimeError: boom")

    def test_sigterm_is_recorded(self):
        code, r = self.drive(self.failing(KeyboardInterrupt()))
        self.assertEqual(code, pg.INTERRUPTED)
        self.assertIn("SIGTERM", r["error"])

    def test_the_session_is_stopped(self):
        session = mock.Mock()
        session.info.return_value = {"run": "/r/session", "user": "/r/u", "cwd": "/r", "log": "/r/session/log.txt",
                                     "pid": 1}
        session.connect.side_effect = ValueError("no bridge")
        session.stop.return_value = {"exit_code": 0}
        fake = mock.Mock()
        fake.start_session.return_value = session
        code, r = self.drive(fake)
        self.assertEqual(code, 2)
        session.stop.assert_called_once()
        self.assertEqual(r["exit_code"], 0)
        self.assertEqual(r["error"], "ValueError: no bridge")

    def test_sigterm_becomes_keyboardinterrupt(self):
        with self.assertRaises(KeyboardInterrupt):
            pg.stop_signal(15, None)


class Ab(unittest.TestCase):
    """ab's exit status: from the gates and from its runs' own"""

    def ab(self, b_points, b_code=0, a_missing=False, **b_kw):
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp)
        out = os.path.join(tmp, "o")
        pts = [point(s["name"], s["kind"]) for s in pg.SCENARIO]

        def launch(cmd):
            name = cmd[cmd.index("--name") + 1]
            if name.startswith("b-"):
                make_run(out, name, b_points(pts), **b_kw)
                return b_code
            if not a_missing:
                make_run(out, name, pts)
            return 0
        with mock.patch.object(pg, "launch", side_effect=launch), mock.patch.object(pg, "on_ram", return_value=False), \
                mock.patch.object(pg, "catch_stops"), contextlib.redirect_stdout(io.StringIO()), \
                contextlib.redirect_stderr(io.StringIO()):
            return pg.main(["ab", "--a", "main=" + tmp, "--b", "main=" + tmp, "--out", out])

    def test_the_same_passes(self):
        self.assertEqual(self.ab(lambda pts: pts), 0)

    def test_a_difference_fails(self):
        self.assertEqual(self.ab(lambda pts: [dict(p, rupees=5) for p in pts]), 1)

    def test_a_run_that_exited_badly_never_passes(self):
        self.assertEqual(self.ab(lambda pts: pts, b_code=2), 1)

    def test_a_run_with_no_result(self):
        self.assertEqual(self.ab(lambda pts: pts, a_missing=True), 2)

    def test_no_save_written_by_b_fails(self):
        self.assertEqual(self.ab(lambda pts: pts, written=[]), 1)   # (A wrote one: B's none is a change)

    def test_unread_threads_in_b_fail(self):
        self.assertEqual(self.ab(lambda pts: pts, evidence={"read": False, "why": "x"}), 1)


class Queue(unittest.TestCase):
    """A game run starts only inside a --mode shared job of the PC's queue, on Linux, never on a RAM file system"""

    def call(self, argv, env):
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.dict(os.environ, env, clear=False), contextlib.redirect_stdout(out), \
                contextlib.redirect_stderr(err), mock.patch.object(pg, "ctl", side_effect=AssertionError("started")):
            for k in ("LONGSHOT_JOB", "LONGSHOT_JOB_MODE"):
                if k not in env:
                    os.environ.pop(k, None)
            code = pg.main(argv)
        return code, out.getvalue(), err.getvalue()

    def test_run_refuses_outside_the_queue(self):
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp)
        argv = ["run", "--game", tmp, "--out", tmp, "--name", "a"]
        code, _, err = self.call(argv, {})
        self.assertEqual(code, 2)
        self.assertIn("dev.py queue run", err)
        for mode in ("timing", "gpu"):
            code, _, err = self.call(argv, {"LONGSHOT_JOB": "1", "LONGSHOT_JOB_MODE": mode})
            self.assertEqual(code, 2)
            self.assertIn("--mode shared", err)
        with mock.patch.object(pg, "on_ram", return_value=True):
            code, _, err = self.call(argv, {"LONGSHOT_JOB": "1", "LONGSHOT_JOB_MODE": "shared"})
        self.assertEqual(code, 2)
        self.assertIn("RAM file system", err)
        with mock.patch.object(pg, "WINDOWS", True):
            code, _, err = self.call(argv, {"LONGSHOT_JOB": "1", "LONGSHOT_JOB_MODE": "shared"})
        self.assertEqual(code, 2)
        self.assertIn("Windows", err)

    def test_names_stay_in_their_folder(self):
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp)
        for name in ("../up", "a/b", "", "A"):
            self.assertEqual(self.call(["run", "--game", tmp, "--out", tmp, "--name", name], {})[0], 2, name)
        self.assertEqual(self.call(["run", "--game", tmp, "--out", tmp, "--name", "a", "--save", "../x"], {})[0], 2)

    def test_run_dry_run_prints_the_queue_command(self):
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp)
        code, out, _ = self.call(["run", "--game", tmp, "--out", tmp, "--name", "a", "--dry-run"], {})
        self.assertEqual(code, 0)
        self.assertIn("dev.py queue run parity-a --mode shared --desc parity gates run a --", out)

    def test_ab_puts_every_run_through_the_queue(self):
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp)
        with mock.patch.object(pg, "on_ram", return_value=False):   # (the test's own folder may be in a tmpfs /tmp)
            code, out, _ = self.call(["ab", "--a", "main=" + tmp, "--b", "main=" + tmp, "--out", os.path.join(tmp, "o"),
                                      "--b-env", "HOS_FPS_STEP=1", "--dry-run"], {})
        self.assertEqual(code, 0)
        runs = [line for line in out.splitlines() if "dev.py queue run" in line]
        self.assertEqual(len(runs), 3)   # (A, B, A again)
        names = [line.split("--name ")[1].split()[0] for line in runs]
        self.assertEqual(len(set(names)), 3)   # (one label on both sides: still three folders)
        for line in runs:
            self.assertIn("--mode shared --", line)
        self.assertIn("--watch-from", runs[1])
        self.assertEqual(runs[2].count("--watch-from"), 2)   # (A again reads what A and B saw change)
        self.assertIn("HOS_FPS_STEP=1", runs[1])
        self.assertNotIn("HOS_FPS_STEP=1", runs[0] + runs[2])

    def test_the_callers_switches_are_dropped(self):
        with mock.patch.dict(os.environ, {"RX_VK_SOMETHING": "1", "HOS_FPS_MAX": "60", "OOT_X": "1", "PATH_X": "1"}):
            gone = pg.drop_callers_switches({"HOS_FPS_MAX": "1000"})
            self.assertIn("RX_VK_SOMETHING", gone)
            self.assertIn("OOT_X", gone)
            self.assertNotIn("RX_VK_SOMETHING", os.environ)
            self.assertIn("HOS_FPS_MAX", os.environ)
            self.assertIn("PATH_X", os.environ)


class Scenario(unittest.TestCase):
    def test_steps(self):
        names = [s["name"] for s in pg.SCENARIO]
        self.assertEqual(len(names), len(set(names)))
        self.assertEqual(pg.SCENARIO[0]["do"], "control")
        for s in pg.SCENARIO:
            self.assertIn(s["kind"], pg.TOLERANCE)
            for k in pg.SPREAD:
                self.assertLessEqual(pg.TOLERANCE[s["kind"]][k], pg.CEILING[s["kind"]][k])
            for step in s.get("steps", ()):
                self.assertTrue(1 <= step["frames"] <= 216000 and 0 <= step["at"] <= 216000, step)

    def test_the_runs_settings_are_per_frame_game_time(self):
        self.assertEqual(pg.BASE_ENV["HOS_FPS_STEP"], "0")
        self.assertEqual(pg.BASE_ENV["HOS_REALTIME_STEP"], "0")
        # the 60 cap: at any other the game's rate follows the frames' own, and a frame under 60 fps steps more than a
        # 60th (two runs of main walked 0.3 m and 4.7 m on the same presses, Uncapped on a busy PC)
        self.assertEqual(pg.BASE_ENV["HOS_FPS_MAX"], "60")
        vi = (ROOT / "host" / "nn_vi.c").read_text(encoding="utf-8")
        self.assertIn("if (!rx_option(0, 1) && fps_max() == 60) {", vi)   # (Per frame at the 60 cap: a 60th, then return)
        self.assertIn("const int timed = fps_max() != 60 && timed_step && hos_step_probe;", vi)   # (the self-check's B)

    def test_the_lines_relied_on_are_the_sources(self):
        mods = (ROOT / "game" / "mods.c").read_text(encoding="utf-8")
        self.assertIn('hos_log("mods: none in %s', mods)
        self.assertIn('hos_log("mods: %d mods from %s', mods)
        self.assertIn('"save trace: reason=', (ROOT / "game" / "oot_tools.c").read_text(encoding="utf-8"))


class FakeGame:
    """drive.py's Game as far as settled() uses it: status() from a list of states, frames() counting"""

    def __init__(self, states):
        self.states, self.frame, self.asked = list(states), 0, 0

    def status(self):
        self.asked += 1
        return self.states.pop(0) if self.states else CALM

    def frames(self, n):
        self.frame += n


CALM = {"map": "HyruleField_Child", "event": False, "loading": False, "fresh": True}
ARRIVING = dict(CALM, event=True)


class Settled(unittest.TestCase):
    """A checkpoint waits for the game calm SETTLED_SAMPLES samples in a row: castle-gate's first runs took the field
    checkpoint on one calm sample right after the map changed, before the arrival scene started (event playing, Link's facing
    still the last map's: 114 degrees apart between two runs of main)"""

    def test_one_calm_sample_before_the_arrival_scene_is_not_settled(self):
        g = FakeGame([CALM, ARRIVING, ARRIVING] + [CALM] * pg.SETTLED_SAMPLES)
        self.assertEqual(pg.settled(g), CALM)
        self.assertEqual(g.asked, 3 + pg.SETTLED_SAMPLES)
        self.assertGreaterEqual(pg.SETTLED_SAMPLES * pg.SETTLED_GAP, 60)

    def test_a_stale_or_loading_sample_breaks_the_run(self):
        g = FakeGame([CALM, CALM, dict(CALM, fresh=False), CALM, dict(CALM, loading=True)] + [CALM] * pg.SETTLED_SAMPLES)
        pg.settled(g)
        self.assertEqual(g.asked, 5 + pg.SETTLED_SAMPLES)

    def test_never_settled_fails(self):
        g = FakeGame([ARRIVING] * 100000)
        with self.assertRaises(pg.Failed):
            pg.settled(g, limit_frames=600)


class Driving(unittest.TestCase):
    """The game is driven by the modules the gate loads: every name it calls on them is there (gate 3 died before any run
    with pipeline_tour's driving moved to scripts/tours/drive.py and the client to longshot_bridge.py)"""

    def test_the_driving_names_are_there(self):
        t, c = pg.tour(), pg.ctl()
        for name in ("Game", "fresh", "calm_after", "map_change", "new_game"):
            self.assertTrue(callable(getattr(t, name, None)), name)
        for name in ("start_session", "Failed"):
            self.assertTrue(hasattr(c, name), name)
        self.assertIs(getattr(t, "longshot_bridge", c), c)   # the tour catches the client's own Failed


if __name__ == "__main__":
    unittest.main()
