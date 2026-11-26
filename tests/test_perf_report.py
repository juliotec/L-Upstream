"""The performance statistics (docs/dev/reference/profiling.md, Performance statistics): the game's summary (gpu/vk/perf_report.h, compiled with
tests/test_perf_report.cpp: buckets, hitches, pacing, the JSON, a frame's cost), the wiring (on only with the tick, a
frame added at each present, the events where they happen), and what the relay does with what the game sends. The
launcher's side, the tick and the sending, is longshot-builder's (`longshot-builder perf-report`: builder/crates/build-driver/
src/perf.rs, its tests there):

  - the sizes: each biggest-case summary the C++ test writes is re-encoded the way the sender does and is no bigger
    than its MAX_BODY (perf.rs);
  - the relay's rules (a short statement of Longshot-api's telemetry/perf, its public source linked at relay_cell): for
    each exact place and view the C++ test makes, the sent sample is one the relay takes (|yaw| <= pi, |pitch| <= pi/2:
    one sample past that and it refuses the whole summary) and it files it under the cell and view it files the exact
    place under, so the game's rounding costs the relay's totals nothing;
  - the host's pose code is run by tests/test_perf_pose.py, not read here;
  - the summary's map before the first one: "Preparing shaders", then "Title and intro".

python -I -B tests/test_perf_report.py [--cxx g++]
"""
import argparse
import json
import math
import re
import subprocess
import sys
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cxx", "{cxx}"]

ROOT = Path(__file__).resolve().parents[1]
# (the sender is longshot-builder's: builder/crates/build-driver/src/perf.rs, whose tests cover it; its limit, read here)
SENDER = (ROOT / "builder" / "crates" / "build-driver" / "src" / "perf.rs").read_text(encoding="utf-8")
_m = re.search(r"pub const MAX_BODY: usize = (\d+) \* (\d+);", SENDER)
MAX_BODY = int(_m.group(1)) * int(_m.group(2)) if _m else 0

failures = []


def check(ok, what):
    print(("ok   " if ok else "FAIL ") + what)
    if not ok:
        failures.append(what)


def wiring():
    vk = ROOT / "gpu" / "vk"
    fps = (vk / "vk_fps.cpp").read_text(encoding="utf-8")
    check("perf_report_init()" in fps and "perf_frame(" in fps, "vk_fps.cpp starts the summary and adds each frame")
    check("perf_measure" in (vk / "vk_fps.h").read_text(encoding="utf-8"), "the Breakdown is measured while the summary is on")
    cpp = (vk / "perf_report.cpp").read_text(encoding="utf-8")
    check((vk / "perf_curl.cpp").exists() and (vk / "perf_curl.h").exists(),
          "curl's posting lives in perf_curl.cpp/h")
    check((vk / "perf_os.h").exists() and (vk / "perf_os_posix.cpp").exists() and (vk / "perf_os_win.cpp").exists(),
          "external-tool spawning is a generic OS interface with one file per platform")
    check("CreateProcessW" not in cpp and "posix_spawn" not in cpp and "find_curl" not in cpp,
          "perf_report.cpp keeps the policy, not the platform's curl and spawn code")
    check('"longshot_perf.ini"' in cpp and '"send="' in cpp, "the game reads the tick from longshot_perf.ini")
    check('pr::summary_period(getenv("RX_VK_PERF_INTERVAL"))' in cpp and "since_summary >= period" in cpp,
          "the game's interval is RX_VK_PERF_INTERVAL")
    check('"perf_pending"' in cpp and "rename(" in cpp, "summaries are written whole into perf_pending")
    for f, use in (("vk_replay.cpp", "perf_report_final(perf_report::kDeviceLost)"), ("vk_replay.cpp", "kVramOverBudget"),
                   ("vk_replay.cpp", "kMovieUpscalerDropped"), ("vk_dlss.cpp", "kDlssRecreated")):
        check(use in (vk / f).read_text(encoding="utf-8"), "%s reports %s" % (f, use))
    main_cpp = (vk / "vk_main.cpp").read_text(encoding="utf-8")
    close = main_cpp[main_cpp.index("void mkwvk_close_asked(void) {"):]
    check("perf_report_final(-1)" in close[:close.index("\n}\n")],
          "the window's close writes the last summary (ExitProcess is _exit on Linux: no atexit)")
    api = (vk / "vk_api.h").read_text(encoding="utf-8")
    check("perf_info" in api, "the host gives the renderer the map and the movie totals (perf_info)")
    check("hos_perf_info" in (ROOT / "host" / "rx_backend.c").read_text(encoding="utf-8"), "rx_backend.c fills perf_info")
    check("call_once(once, [] {\n        perf_report_final(perf_report::kDeviceLost);" in
          (vk / "vk_replay.cpp").read_text(encoding="utf-8"), "a lost device's summary is written once")
    check("if (!ticked(folder))" in cpp[cpp.index("void summarize("):], "unticked while the game runs: it stops")
    check("constexpr int kMaxMaps = 16;" in (vk / "perf_report.h").read_text(encoding="utf-8") and
          "maps.size() >= (size_t)pr::kMaxMaps" in cpp and "if (where_now()) summarize(false), since_summary = 0, where_now();" in cpp,
          "a summary holds 16 maps at most, as the relay takes them: a 17th ends it early")
    check("shaders_preparing() ? pr::kPreparingShaders : pr::kTitleAndIntro" in cpp and
          "bool shaders_preparing()" in main_cpp, "before a map: Preparing shaders, then Title and intro (not 'not known')")
    header = (vk / "perf_report.h").read_text(encoding="utf-8")
    m = re.search(r"constexpr size_t kMaxBody = (\d+) \* (\d+);", header)
    check(m is not None and int(m.group(1)) * int(m.group(2)) == MAX_BODY,
          "the game fits its summaries to the sender's MAX_BODY (perf.rs: a bigger one is refused and deleted)")
    frame_fn = cpp[cpp.index("void perf_frame("):]
    frame_fn = frame_fn[:frame_fn.index("\n}\n")]
    check("since_sample" not in cpp and "if (!current || since_map >= kMapEvery) {" in frame_fn and
          "constexpr double kMapEvery = 1;" in cpp, "one clock for the map and the one-second sample")
    summarize = cpp[cpp.index("void summarize("):]
    check("if (now) finish_sample();" in summarize[:summarize.index("\n}\n")],
          "a summary mid-second keeps the second being sampled: the frames after it are sampled too")
    tools = (ROOT / "game" / "oot_tools.c").read_text(encoding="utf-8")
    pose_fn = tools[tools.index("static void oot_perf_pose("):]
    pose_fn = pose_fn[:pose_fn.index("\n}\n")]
    check("if (now - last < PERF_POSE_EVERY_NS) return;" in pose_fn and
          pose_fn.index("PERF_POSE_EVERY_NS") < pose_fn.index("oot_gameplay_view(c)"),
          "Link's pose is read a few times a second, not at every update")
    check("recomp_load64(c, view) != 0x84c18a88ULL" in pose_fn and "oot_event_playing(c)" in pose_fn and
          "isfinite" in pose_fn, "the pose is read only from the gameplay camera's view while no event plays, and a stray value is no pose")
    info_fn = tools[tools.index("const char *hos_perf_info("):]
    check("now - perf_pose_ns < PERF_POSE_STALE_NS" in info_fn[:info_fn.index("\n}\n")],
          "a pose Link's update no longer sets (the title, a load) is no pose")
    title = tools[tools.index("if (!loaded && logged[0]) {"):]
    check("perf_pose_clear();" in title[:title.index("\n    }\n")], "back at the title: no pose from the last map")


def relay_round(v):
    """Go's math.Round: halves away from zero"""
    return math.copysign(math.floor(abs(v) + 0.5), v)


def relay_cell(s):
    """The cell the relay files a sample's place under (Longshot-api telemetry/perf/totals.go addPosition at eadbca4,
    https://git.magicbean.dev/MagicBeanDev/DataServicesAPI/src/commit/eadbca4c0bb47a9e128f4c314cf2a9c9916fec15/telemetry/perf/totals.go#L205):
    each axis's nearest multiple of 100, math.Round(v / 100) * 100"""
    return tuple(relay_round(s[k] / 100) * 100 for k in ("x", "y", "z"))


def relay_view(s):
    """The view bins it files the angles under (the same function): the yaw wrapped round and floored to 45 degrees from
    -180, the pitch floored to 30 degrees from -90, both in degrees"""
    yaw = math.fmod(s["yaw"] * 180 / math.pi + 180, 360)
    return (math.floor(yaw / 45) * 45 - 180, math.floor((s["pitch"] * 180 / math.pi + 90) / 30) * 30 - 90)


def relay_takes(s):
    """Whether the relay takes the sample (telemetry/perf/summary.go, the sample's coordinates at L471): a coordinate out of
    its range, or not a number, and it refuses the whole summary with a 400, which the sender then deletes"""
    ranges = {"x": 1e9, "y": 1e9, "z": 1e9, "yaw": math.pi, "pitch": math.pi / 2}
    return all(isinstance(s.get(k), (int, float)) and not isinstance(s.get(k), bool) and math.isfinite(s[k]) and abs(s[k]) <= top
               for k, top in ranges.items())


def relay_rules(written):
    """The exact places and views the C++ test made, and the sample the game makes of each, against the relay's rules"""
    rows = [line.rstrip("\n").split("\t") for line in open(written / "pairs.tsv", encoding="utf-8")]
    check(len(rows) > 20000, "the C++ test wrote its places and views (%d)" % len(rows))
    refused, other_cell, other_view, cells, straight_up = [], [], [], set(), 0
    for x, y, z, yaw, pitch, sample_text in rows:
        exact = dict(zip("xyz", map(float, (x, y, z))), yaw=float(yaw), pitch=float(pitch))
        sample = json.loads(sample_text)
        if not relay_takes(exact):
            check(False, "the C++ test's own exact place is out of the relay's range: %s" % exact)
        if not relay_takes(sample):
            refused.append((exact, sample))
            continue
        cells.add(relay_cell(sample))
        if relay_cell(sample) != relay_cell(exact):   # (-0.0 == 0.0: the relay's own label for an exact -40 is "x=-0")
            other_cell.append((exact, sample, relay_cell(exact), relay_cell(sample)))
        want = relay_view(exact)
        if exact["pitch"] >= math.pi / 2:   # (the relay's 90-degree pitch bin holds only exactly straight up: the game puts it in the top bin)
            want, straight_up = (want[0], 60), straight_up + 1
        if relay_view(sample) != want:
            other_view.append((exact, sample, want, relay_view(sample)))
    check(not refused, "the relay takes every sample the game sends (it refuses the whole summary for one out of range; %d of %d "
                       "are, the first: %s)" % (len(refused), len(rows), refused[:1]))
    check(not other_cell, "the relay files each sample under the cell it files the exact place under (%d of %d differ, the first: %s)"
          % (len(other_cell), len(rows), other_cell[:1]))
    check(not other_view, "the relay files each sample under the view bins it files the exact angles under (%d of %d differ, "
                          "the first: %s)" % (len(other_view), len(rows), other_view[:1]))
    check(straight_up >= 1 and (0.0, 0.0, 0.0) in cells, "the cases include straight up, and the cell at the origin, x=0, y=0, z=0, is sent "
                                                          "(the relay's own rounding of a half-way sample sent it away)")


def sizes(tmp, written):
    """Each size case re-encoded the way the sender does, no bigger than its MAX_BODY"""
    cases = sorted(written.glob("size-*.json"))
    check(len(cases) == 8, "the C++ test wrote its size cases (%d)" % len(cases))
    for f in cases:
        body = json.dumps(json.loads(f.read_bytes()), separators=(",", ":")).encode()
        check(len(body) <= MAX_BODY, "%s re-encoded as the sender does: %d B, at most %d" % (f.name, len(body), MAX_BODY))
    # (the sender re-encodes compactly, as here (serde_json); that it sends each one is perf.rs's tests')
    check(MAX_BODY > 0, "perf.rs names its MAX_BODY")


def unit(cxx, tmp, written):
    exe = Path(tmp, "perf_report_test")
    subprocess.run([cxx, "-std=c++20", "-O2", "-D_CRT_SECURE_NO_WARNINGS", "-include", str(ROOT / "gpu" / "vk" / "compat" / "longshot_env.h"),
                    "-I", str(ROOT / "gpu" / "vk"), str(ROOT / "tests" / "test_perf_report.cpp"), "-o", str(exe)],
                   check=True)
    out = subprocess.run([str(exe), str(written)], capture_output=True, text=True)
    print(out.stdout.strip())
    check(out.returncode == 0, "tests/test_perf_report.cpp" + (": " + out.stderr.strip() if out.stderr else ""))
    m = re.search(r"add_frame: ([\d.]+) ns", out.stdout)
    check(m is not None and float(m.group(1)) < 200, "a frame costs well under a microsecond to add")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cxx", default="g++")
    a = p.parse_args()
    with tempfile.TemporaryDirectory() as tmp:
        written = Path(tmp, "written")
        written.mkdir()
        unit(a.cxx, tmp, written)
        relay_rules(written)
        sizes(tmp, written)
        wiring()
    print("%d failed" % len(failures) if failures else "all passed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
