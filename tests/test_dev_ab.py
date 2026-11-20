"""`dev.py ab` (scripts/devtools/ab.py), its parts that need no game, build or queue: the plan (the sides, in-run or separate
runs by whether a switch's reader takes HOS_AB, the interleaved order, HOS_AB's value), the scene table against the
repository's saves and the Debug tab's teleport list read from the source, the frame trace (a line cut short, the warm-up's
cold frames, the measured window), the numbers, the log's facts, the check that the runs ran the same, the verdict, the
Measurements section, the runtime's freshness against its base and pack, the pinned options file, and `--dry-run`.

    python -I -B tests/test_dev_ab.py
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "scripts"))
from devtools import ab, packs  # noqa: E402
from tours import route  # noqa: E402

HEAD = ("frame,ms,pipe_waits,pipe_wait_ms,left_out,translate_ms,uploads,upload_mb,textures,memsync_ms,pipelines_ms,uploads_ms,"
        "programs_ms,slot_wait_ms,gpu_behind_ms,draw_ms,desc_ms,cmds_ms,tex_lookup_ms,textures_gone,late_wait_ms,"
        "readback_wait_ms,tex_frame_ms,game_ms,render_ms,gpu_wait_ms,pace_ms,wait_render_ms,wait_gpu_ms,wait_main_ms,"
        "main_cpu_ms,main_blocked_ms,main_vcore_ms,shown_ms,shown_at_ns,at_ns,ab_side")


def row(frame, ms, at_ns, side=-1, pipe_waits=0, pipelines_ms=0.0):
    cells = {k: "0" for k in HEAD.split(",")}
    cells.update(frame=str(frame), ms="%.2f" % ms, at_ns=str(int(at_ns)), ab_side=str(side), pipe_waits=str(pipe_waits),
                 pipelines_ms="%.2f" % pipelines_ms)
    return ",".join(cells[k] for k in HEAD.split(","))


def plan(how="separate", what="change"):
    a_env, b_env = ({}, {}) if what == "change" else ({}, {"HOS_COLLECT_WIDE": "0"})
    return ab.make_plan(a_env, b_env, what == "change", ab.scene_of("market"), 60, 2, 10, dict(ab.COMMON_ENV), how, "",
                        {"A": "1a2b3c4d" + "0" * 32, "B": "9f8e7d6c" + "0" * 32}, {"A": "/rt-base", "B": "/rt-x"}, "/runs/x")


class Plan(unittest.TestCase):
    def test_sides(self):
        self.assertEqual(ab.differing({"A_X": "1"}, {"A_X": "1", "B_Y": "2"}), ["B_Y"])
        self.assertEqual(ab.differing({"X": "1"}, {"X": "2"}), ["X"])
        with self.assertRaises(SystemExit):
            ab.assignments(["lower=1"], "--env")
        with self.assertRaises(SystemExit):
            ab.assignments(["NOEQUALS"], "--env")
        self.assertEqual(ab.assignments(["HOS_X=a=b"], "--env"), {"HOS_X": "a=b"})

    def test_in_run_or_separate(self):
        wired = {"HOS_COLLECT_WIDE"}
        self.assertEqual(ab.in_run({}, {"HOS_COLLECT_WIDE": "0"}, wired), (True, "HOS_COLLECT_WIDE"))
        self.assertEqual(ab.in_run({"HOS_COLLECT_WIDE": "1"}, {"HOS_COLLECT_WIDE": "0"}, wired)[0], True)
        self.assertFalse(ab.in_run({}, {"RX_VK_ASYNC": "0"}, wired)[0])                        # (read once)
        self.assertFalse(ab.in_run({}, {"HOS_COLLECT_WIDE": "off"}, wired)[0])                 # (HOS_AB takes numbers)
        self.assertFalse(ab.in_run({"HOS_COLLECT_WIDE": "x"}, {"HOS_COLLECT_WIDE": "0"}, wired)[0])
        self.assertFalse(ab.in_run({}, {"HOS_COLLECT_WIDE": "0", "RX_Y": "1"}, wired)[0])      # (two switches)

    def test_wired_from_source(self):
        found = ab.wired_switches()
        self.assertTrue({"HOS_COLLECT_WIDE", "HOS_OS_YIELD", "HOS_OOT_BARRIER_WAIT"} <= found, found)
        d = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, d, True)
        os.makedirs(os.path.join(d, "host"))
        with open(os.path.join(d, "host", "x.c"), "w") as f:
            f.write('int a = hos_ab_value("HOS_ONE", v); int b = getenv("HOS_TWO");')
        self.assertEqual(ab.wired_switches(d), {"HOS_ONE"})

    def test_order_interleaved(self):
        self.assertEqual(ab.order(3, "separate"), [("A", 1), ("B", 1), ("A", 2), ("B", 2), ("A", 3), ("B", 3)])
        self.assertEqual(ab.order(2, "in-run"), [("AB", 1), ("AB", 2)])

    def test_plan(self):
        p = plan()
        self.assertEqual(p["sides"]["A"]["label"], "main @ 1a2b3c4d")
        self.assertEqual(p["sides"]["B"]["label"], "this PR @ 9f8e7d6c")
        self.assertEqual(ab.short("9f8e7d6c11+uncommitted"), "9f8e7d6c (uncommitted files)")
        p = plan("in-run", "switch")
        self.assertEqual(p["sides"]["AB"]["env"], {"HOS_AB": "HOS_COLLECT_WIDE=0,%d,0" % ab.AB_PERIOD_S})
        self.assertEqual(p["sides"]["A"]["label"], "HOS_COLLECT_WIDE unset")
        self.assertEqual(p["sides"]["B"]["label"], "HOS_COLLECT_WIDE=0")
        self.assertIn("AB1, AB2", ab.plan_text(p))
        json.dumps(p)

    def test_common_env(self):
        self.assertEqual(ab.COMMON_ENV["HOS_FPS_MAX"], "1000")
        self.assertEqual(ab.COMMON_ENV["RX_VK_FRAME_TRACE"], "1")


class Scenes(unittest.TestCase):
    def test_every_scene_resolves(self):
        saves = os.listdir(os.path.join(ROOT, "assets", "saves"))
        actions = route.actions()
        self.assertIn(ab.DEFAULT_SCENE, ab.SCENES)
        for name, (save, to, age, hour, about) in ab.SCENES.items():
            self.assertIn(save, saves, name)
            self.assertIn(age, ("child", "adult"), name)
            self.assertTrue(hour is None or 0 <= hour < 24, name)
            dest = ab.destination(ab.scene_of(name), actions)
            if to:
                self.assertEqual(actions["teleports"][dest[0]][0], to)
                self.assertTrue(dest[1])
            else:
                self.assertIsNone(dest)

    def test_custom_save(self):
        s = ab.scene_of("kokiri-house", "/x/my-save", "03")
        self.assertEqual((s["save"], s["folder"], s["slot"]), (None, os.path.abspath("/x/my-save"), "03"))
        with self.assertRaises(SystemExit):
            ab.scene_of("nowhere")

    def test_options_keys_in_source(self):
        with open(os.path.join(ROOT, "gpu", "vk", "vk_menu.cpp"), encoding="utf-8") as f:
            menu = f.read()
        for key, row in ((ab.SKIP_TITLE, '"Skip title screen", &opts.skip_title'), (ab.FRAMEGEN_OFF, '"Frame generation", &opts.framegen'),
                         (ab.VSYNC_OFF, '"V-Sync", &opts.vsync')):
            self.assertIn(row, menu, key)
        self.assertIn('{"Auto", "On", "Off"}', menu)   # (vsync=2: Off)


class Trace(unittest.TestCase):
    def write(self, lines, cut=False):
        d = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, d, True)
        p = os.path.join(d, "longshot_frames.csv")
        with open(p, "w") as f:
            f.write(HEAD + "\n" + "\n".join(lines) + "\n" + ("12,16.0,0" if cut else ""))
        return p

    def test_read_window_warm(self):
        s = 1e9
        lines = [row(i, 16, i * 0.016 * s, pipe_waits=1 if i < 100 else 0) for i in range(1000)]
        p = self.write(lines, cut=True)
        rows = ab.read_trace(p)
        self.assertEqual(len(rows), 1000)   # (the line cut short left out)
        self.assertTrue(ab.cold(rows[0]) and not ab.cold(rows[500]))
        self.assertTrue(ab.settled(rows))                       # (16 s of trace, the last 5 s warm)
        self.assertFalse(ab.settled(rows[:400]))                # (cold frames in the last 5 s)
        self.assertFalse(ab.settled(rows[150:200]))             # (under 5 s of trace)
        w = ab.window(rows, 2 * s, 4)
        self.assertTrue(all(2 * s < ab.num(r, "at_ns") <= 6 * s for r in w))
        self.assertEqual(len(w), 250)
        self.assertEqual(ab.read_trace(os.path.join(os.path.dirname(p), "none.csv")), [])

    def test_frame_stats(self):
        st = ab.frame_stats([10.0] * 99 + [20.0])
        self.assertEqual(st["frames"], 100)
        self.assertAlmostEqual(st["mean_ms"], 10.1)
        self.assertAlmostEqual(st["fps"], 1000 / 10.1)
        self.assertEqual(st["median_ms"], 10.0)
        self.assertAlmostEqual(st["low1_fps"], 1000 / st["p99_ms"])

    def test_in_run(self):
        rows, t = [], 0
        for period in range(10):   # (side 0 at 16 ms, side 1 at 18 ms, 3 s each)
            ms = 16.0 if period % 2 == 0 else 18.0
            for _ in range(int(3000 / ms)):
                t += ms * 1e6
                rows.append({"ms": str(ms), "ab_side": str(period % 2), "at_ns": str(t)})
        summary, periods, dropped, left_out = ab.in_run_stats(rows, "t")
        self.assertAlmostEqual(summary["sides"]["0"]["mean_ms"], 16.0)
        self.assertAlmostEqual(summary["sides"]["1"]["mean_ms"], 18.0)
        self.assertEqual(ab.verdict_in_run(summary)[0], "loss")


LOG = """longshot v0.9.1-12-gabc
run_id 1234
vk: device 0: AMD Radeon RX 580, Vulkan 1.3, driver 0x80000; mesh shaders: compute
[main] vk: switches set (3): HOS_FPS_MAX=1000 (player), RX_VK_FRAME_TRACE=1 (diag), HOS_COLLECT_WIDE=0 (kill)
ab: HOS_AB: after 0 s of warm-up, HOS_COLLECT_WIDE alternates every 3.0 s between its normal value (side 0, first) and 0
"""


class Facts(unittest.TestCase):
    def test_log(self):
        f = ab.log_facts(LOG)
        self.assertEqual(f["version"], "v0.9.1-12-gabc")
        self.assertIn("RX 580", f["gpu"])
        self.assertEqual(f["switches"], {"HOS_FPS_MAX": "1000", "RX_VK_FRAME_TRACE": "1", "HOS_COLLECT_WIDE": "0"})
        self.assertTrue(f["switches_line"].startswith("[main] vk: switches set (3)"))
        self.assertEqual(len(f["ab_lines"]), 1)
        self.assertEqual(ab.log_facts("longshot v1\nvk: switches set: none\n")["switches"], {})

    def test_same_or_not(self):
        status = {"renderer": "Vulkan: RX 580", "window": {"width": 1280, "height": 720}}
        opts = [{"key": "upscaler", "text": "FSR 3.1"}, {"key": "resolution", "text": "67%"}]
        facts = ab.log_facts(LOG)
        a = dict(ab.what_ran(status, opts, facts, skip={"HOS_COLLECT_WIDE"}), version="v1")
        b = dict(ab.what_ran(status, opts, dict(facts, switches=dict(facts["switches"], HOS_COLLECT_WIDE="1")),
                             skip={"HOS_COLLECT_WIDE"}), version="v2")
        self.assertEqual(ab.differences({("A", 1): a, ("B", 1): b}), [])          # (the compared switch, the version)
        self.assertEqual(len(ab.differences({("A", 1): a, ("B", 1): b}, version_counts=True)), 1)
        c = dict(b, options={"upscaler": "DLSS", "resolution": "67%"})
        diff = ab.differences({("A", 1): a, ("B", 1): c})
        self.assertEqual(len(diff), 1)
        self.assertIn("options upscaler", diff[0])
        allowed = ab.what_ran(status, [{"key": "upscaler", "text": "DLSS"}] + opts[1:], facts, skip={"upscaler"})
        self.assertNotIn("upscaler", allowed["options"])

    def test_run_only_switches_not_compared(self):
        """The build's name and the queue's job switches differ from run to run and say nothing of what ran"""
        status = {"renderer": "r", "window": None}
        facts = ab.log_facts(LOG)
        runs = {}
        for k, extra in ((1, {"LONGSHOT_BUILD_NAME": "rt-base", "LONGSHOT_JOB_ID": "7", "LONGSHOT_JOB_MODE": "timing"}),
                         (2, {"LONGSHOT_BUILD_NAME": "rt-x", "LONGSHOT_JOB_ID": "8", "LONGSHOT_JOBS": "4"})):
            runs[("A", k)] = dict(ab.what_ran(status, [], dict(facts, switches=dict(facts["switches"], **extra))), version="v")
        self.assertEqual(ab.differences(runs), [])
        self.assertNotIn("LONGSHOT_BUILD_NAME", runs[("A", 1)]["switches"])

    def test_menu_rows_of_a_switch(self):
        """A compared switch that overrides a menu row (its switches.def row names the options-file key) has that row
        skipped: the row changing is the compared thing"""
        self.assertIn("frame_rate_cap", ab.menu_rows({"HOS_FPS_MAX"}))
        self.assertIn("fullscreen", ab.menu_rows({"HOS_FULLSCREEN"}))
        self.assertEqual(ab.menu_rows({"HOS_COLLECT_WIDE"}), set())
        p = ab.make_plan({}, {"HOS_FPS_MAX": "60"}, False, ab.scene_of("market"), 60, 2, 10, {}, "separate", "", {},
                         {"A": "/rt", "B": "/rt"}, "/runs/x", ["upscaler"])
        self.assertEqual(p["allow"], ["frame_rate_cap", "upscaler"])


class WriteUp(unittest.TestCase):
    def test_verdict(self):
        self.assertEqual(ab.verdict([60, 61, 62], [65, 66, 67])[0], "gain")
        self.assertEqual(ab.verdict([60, 61, 62], [55, 56, 57])[0], "loss")
        kind, text = ab.verdict([60, 61, 62], [61.5, 62.5, 63])
        self.assertEqual(kind, "within spread")
        self.assertIn("within the runs' spread", text)

    def test_measurements(self):
        p = plan()
        runs = []
        for side, fps in (("A", 60.0), ("B", 66.0), ("A", 61.0), ("B", 67.0)):
            st = ab.frame_stats([1000.0 / fps] * 100)
            r = {"side": side, "k": 1, "seconds": 60, "warm": True, "cold": 0, "stats": st,
                 "ran": {"options": {"upscaler": "FSR 3.1"}, "window": {"width": 1280, "height": 720}},
                 "log_lines": ["vk: switches set (1): HOS_FPS_MAX=1000 (player)"]}
            r["line"] = ab.run_line(r)
            runs.append(r)
        res = {"runs": runs, "ran": runs[0]["ran"], "gpu": "RX 580", "system": "Linux 7"}
        res["verdict"] = ab.verdict([60, 61], [66, 67])
        text = ab.measurements(p, res)
        self.assertTrue(text.startswith("**Verdict:** faster:"))
        self.assertIn("**Setup:** Linux 7, RX 580; Market", text)
        self.assertIn("| | Before: main @ 1a2b3c4d | After: this PR @ 9f8e7d6c | Δ |", text)
        self.assertIn("60.0 / 61.0 (median 60.5)", text)
        self.assertIn("A1: 100 frames in 60 s, 60.0 fps", text)
        self.assertIn("vk: switches set (1)", text)
        self.assertIn("FAILED: boom", ab.run_line({"side": "B", "k": 2, "error": "boom"}))

    def write_up(self, p, version="v0.9.1-12-g7afe6ae1"):
        runs = []
        for side, fps in (("A", 60.0), ("B", 66.0), ("A", 61.0), ("B", 67.0)):
            r = {"side": side, "k": 1, "seconds": 60, "warm": True, "cold": 0, "stats": ab.frame_stats([1000.0 / fps] * 100),
                 "ran": {"options": {}, "window": None, "version": version}, "log_lines": []}
            r["line"] = ab.run_line(r)
            runs.append(r)
        res = {"runs": runs, "ran": runs[0]["ran"], "gpu": "RX 580", "system": "Linux 7",
               "verdict": ab.verdict([60, 61], [66, 67])}
        return ab.measurements(p, res)

    def test_measurements_pass_the_pr_check(self):
        """What dev.py ab writes is what check_pr_description.py asks of a PR's Measurements: a change (committed or
        --dirty) against main, and a switch on one build with its Baseline line"""
        import check_pr_description
        change = self.write_up(plan())
        dirty = plan()
        dirty["sides"]["B"]["label"] = "this PR @ %s" % ab.short("9f8e7d6c" + "0" * 32 + "+uncommitted")
        switch = self.write_up(plan("separate", "switch"))
        self.assertIn("| | Before, HOS_COLLECT_WIDE unset: this PR @ 7afe6ae1 | After, HOS_COLLECT_WIDE=0: this PR @ "
                      "7afe6ae1 | Δ |", switch)
        self.assertIn("**Baseline:**", switch)
        self.assertIn("@ 9f8e7d6c (uncommitted files)", self.write_up(dirty))
        for what, text in (("change", change), ("--dirty", self.write_up(dirty)), ("switch", switch)):
            self.assertEqual(check_pr_description.measurement_problems(text, "perf"), [], what)

    def test_in_run_table_without_a_number(self):
        """The in-run table: a side missing a number leaves its Δ empty instead of failing"""
        p = plan("in-run", "switch")
        res = {"ran": {"version": "v0.9.1-12-g7afe6ae1"}, "pooled": {"diff": {}, "sides": {
            "0": {"fps": 60.0, "mean_ms": 16.6, "median_ms": 16.5, "p99_ms": 20.0},
            "1": {"fps": 61.0, "mean_ms": 16.4, "median_ms": 16.3}}}}
        text = ab.table(p, res)
        self.assertIn("| p99 frame time | 20.00 ms | 0.00 ms |  |", text)
        self.assertIn("| median frame time | 16.50 ms | 16.30 ms | -0.20 ms |", text)

    def test_summarise_refuses(self):
        p = plan()
        ran = {"options": {"upscaler": "FSR"}, "switches": {}, "renderer": "r", "window": None, "version": "v"}
        st = ab.frame_stats([16.0] * 10)
        runs = [{"side": "A", "k": 1, "ran": ran, "stats": st, "line": "a"},
                {"side": "B", "k": 1, "ran": dict(ran, options={"upscaler": "DLSS"}), "stats": st, "line": "b"}]
        res = ab.summarise(p, runs)
        self.assertIsNone(res["verdict"])
        self.assertTrue(res["problems"])
        runs[1]["ran"] = ran
        self.assertEqual(ab.summarise(p, runs)["verdict"][0], "within spread")
        runs[1]["error"] = "the game ended"
        self.assertIsNone(ab.summarise(p, runs)["verdict"])


class Files(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.d, True)

    def test_runtime_current(self):
        base, rt = os.path.join(self.d, "base"), os.path.join(self.d, "rt")
        game = os.path.join(base, "game")
        os.makedirs(game)
        os.makedirs(rt)
        for m in packs.MODULES:
            with open(os.path.join(game, m[0]), "w") as f:
                f.write("base " + m[0])
        with open(os.path.join(base, "base.json"), "w") as f:
            json.dump({"commit": "c0ffee", "game": game, "work": os.path.join(base, "work"),
                       "modules": {m[0]: packs.sha(os.path.join(game, m[0])) for m in packs.MODULES}}, f)
        pack = os.path.join(base, "packs", "me")
        os.makedirs(os.path.join(pack, "files"))
        with open(os.path.join(pack, "pack.json"), "w") as f:
            json.dump({"base": "c0ffee", "files": {packs.RENDERER: "newsha"}, "generated": [], "deleted": []}, f)
        made = {"base": "c0ffee", "packs": ["me"], "modules": {packs.GAME: packs.sha(os.path.join(game, packs.GAME)),
                                                               packs.RENDERER: "newsha"}}
        with open(os.path.join(rt, "dev_packs.json"), "w") as f:
            json.dump(made, f)
        self.assertTrue(ab.runtime_current(base, rt, "me"))
        self.assertFalse(ab.runtime_current(base, rt, None))   # (a runtime with a pack is not the base's)
        with open(os.path.join(pack, "pack.json"), "w") as f:
            json.dump({"base": "c0ffee", "files": {packs.RENDERER: "newer"}, "generated": [], "deleted": []}, f)
        self.assertFalse(ab.runtime_current(base, rt, "me"))   # (the pack built again with another renderer)
        self.assertFalse(ab.runtime_current(base, os.path.join(self.d, "none"), "me"))

    def test_pin_options(self):
        mine = os.path.join(self.d, "mine.ini")
        with open(mine, "w") as f:
            f.write("upscaler=3\nvsync=1\n")
        pinned = os.path.join(self.d, "pinned.ini")
        ab.pin_options(mine, pinned, framegen=False, vsync=False)
        with open(pinned) as f:
            lines = f.read().split()
        self.assertEqual(lines, ["upscaler=3", "vsync=2", "skip_title_screen=1", "frame_generation=0"])
        with open(mine) as f:
            self.assertEqual(f.read(), "upscaler=3\nvsync=1\n")   # (the player's file left alone)
        fresh = os.path.join(self.d, "fresh.ini")
        ab.pin_options(None, fresh, framegen=True, vsync=True)
        with open(fresh) as f:
            self.assertEqual(f.read().split(), ["skip_title_screen=1"])

    def test_dry_run(self):
        rt = os.path.join(self.d, "rt")
        os.makedirs(rt)
        env = {k: v for k, v in os.environ.items() if not k.startswith(("HOS_", "RX_"))}
        env["LONGSHOT_DEV_HOME"] = os.path.join(self.d, "dev")
        out = subprocess.run([sys.executable, "-I", os.path.join(ROOT, "scripts", "dev.py"), "ab", "--env", "HOS_COLLECT_WIDE=0",
                              "--runtime", rt, "--scene", "deku-tree", "--dry-run"], env=env, capture_output=True, text=True)
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertIn("in-run", out.stdout)
        self.assertIn("teleport", out.stdout)
        self.assertFalse(os.path.exists(os.path.join(self.d, "dev", "runs")))   # (nothing written)


if __name__ == "__main__":
    unittest.main()
