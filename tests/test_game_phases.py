"""The frame phases (game/phases.c) and where they are run, without a game.

tests/game_phases_test.c is built with game/phases.c alone and runs: the order callbacks run in (by order, ties by
add order) and the frame they are given; removes and adds from inside a callback; a remove from another thread while
the callback runs, which returns only after it did; an add during a run, which counts from the next run; a full table
and a stale handle; and three threads adding and removing while a fourth runs the phase, with no callback run after its
remove returned. Then, in the source:

- with nothing added a run is one relaxed atomic load and a return, before anything else (the parity gates' rule: an
  empty registry is a provable no-op);
- the acquire phase is run first in host/nn_vi.c's hos_vi_native_acquire, before the pacing's returns and its sleep.

(That the game compiles game/ is build-compile's own test, src/host_sources_tests.rs.)

python -I -B tests/test_game_phases.py [--cc gcc]
"""
import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "gcc"]
RUN_ALL_ALONE = True   # (it measures time or CPU, or races threads: beside other checks on a loaded runner it failed or timed out)

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))   # (tests/srccut.py: python -I leaves this folder off the path)
from srccut import block   # noqa: E402


def check(cond, what):
    if not cond:
        raise SystemExit("FAIL: " + what)


def source_checks():
    phases = (ROOT / "game" / "phases.c").read_text(encoding="utf-8")
    run = block(phases, "void game_phase_run(GamePhase phase, uint64_t frame) {")
    body = run[run.index("{") + 1:].lstrip()
    first = body[:body.index(";") + 1]
    check(first == "if ((unsigned)phase >= GAME_PHASE_COUNT || !__atomic_load_n(&live[phase], __ATOMIC_RELAXED)) return;",
          "game/phases.c: game_phase_run does not begin with its one relaxed load and return (%r)" % first)

    vi = (ROOT / "host" / "nn_vi.c").read_text(encoding="utf-8")
    acquire = block(vi, "void hos_vi_native_acquire(int active) {")
    at = acquire.find("game_phase_run(GAME_PHASE_ACQUIRE, acquired++);")
    check(at >= 0, "host/nn_vi.c: hos_vi_native_acquire does not run GAME_PHASE_ACQUIRE")
    for later in ("if (no_pacing || !next_frame_at) return;", "hos_sleep_ns(until - now);"):
        check(0 <= at < acquire.index(later), "host/nn_vi.c: the acquire phase runs after %r" % later)
    check('#include "../game/game.h"' in vi, "host/nn_vi.c does not include game/game.h")
    game_h = (ROOT / "game" / "game.h").read_text(encoding="utf-8")
    check('#include "phases.h"' in game_h, "game/game.h does not include game/phases.h (game_phase_run, for host/nn_vi.c)")
    native = (ROOT / "host" / "nvn_native.c").read_text(encoding="utf-8")
    texture = block(native, "NV(nvnWindowAcquireTexture) {")
    check(texture.index("hos_vcore_away()") < texture.index("hos_vi_native_acquire(") < texture.index("hos_vcore_back()"),
          "host/nvn_native.c: the acquire (and its phase) no longer runs with the virtual core given up")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--cc", default="gcc")
    a = p.parse_args()
    source_checks()
    print("PASS: the empty run is one relaxed load; the acquire phase runs first at the window acquire")
    windows = os.name == "nt"
    with tempfile.TemporaryDirectory(prefix="game-phases-") as tmp:
        exe = Path(tmp, "game_phases_test.exe" if windows else "game_phases_test")
        subprocess.run([a.cc, "-std=c11", "-O1", "-g", "-Wall", "-Wextra", "-Werror", "-D_GNU_SOURCE", "-I", str(ROOT / "host"), "-I", str(ROOT / "game"),
                        "-I", str(ROOT), str(ROOT / "tests" / "game_phases_test.c"), str(ROOT / "game" / "phases.c"), "-o", str(exe)] +
                       ([] if windows else ["-lpthread"]), check=True)
        r = subprocess.run([str(exe)], capture_output=True, text=True, timeout=120)
    print(r.stdout.strip())
    if r.returncode:
        raise SystemExit("FAIL: tests/game_phases_test.c exited %d\n%s" % (r.returncode, r.stderr))


if __name__ == "__main__":
    main()
