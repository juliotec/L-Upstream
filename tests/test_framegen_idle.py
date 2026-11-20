"""When frame generation's images are let go (gpu/vk/framegen_idle.h): after 120 presents in a row with it off, never on a present
that recorded made frames, and the call after the present's queue (gpu/vk/vk_replay.cpp).

Compiles the rule with a small fixture and runs it through present sequences, among them the one #1158's review found: off for
119 presents, then a present that records made frames with it on and reads it Off before the idle check. Before, that present
counted as the 120th off and drained, waiting for its own set, which only the queue call after it would hand to the display thread.

python tests/test_framegen_idle.py [--cxx clang++]
"""
import argparse
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
sys.path.insert(0, str(Path(__file__).resolve().parent))   # (tests/srccut.py: python -I leaves this folder off the path)
from srccut import block   # noqa: E402

RUN_ALL_ARGS = ["--cxx", "clang++"]

ROOT = Path(__file__).resolve().parents[1]

FIXTURE = r'''#include <cstdio>
#include "framegen_idle.h"
using namespace framegen_idle_rule;
static int fails;
static void expect(bool got, bool want, const char* what) {
    if (got != want) { std::printf("FAIL: %s\n", what); fails++; }
}
// A run of presents: on (the row), rec (made frames recorded); true where the images are let go at the last one
static bool run(int n_off, bool last_on, bool last_rec) {
    uint32_t off = 0;
    for (int i = 0; i < n_off; i++) off = next_off(off, false, false);
    off = next_off(off, last_on, last_rec);
    return release_due(off, true);
}
int main() {
    expect(run(119, false, false), true, "the 120th present off lets them go");
    expect(run(118, false, false), false, "the 119th does not");
    expect(run(119, false, true), false, "a present that recorded made frames, the row Off by the check: not let go (the hang)");
    expect(run(119, true, false), false, "a present with the row on: not let go");
    expect(release_due(500, false), false, "nothing made: nothing to let go");
    {   // (and after a recorded present the count starts again: 120 more off are needed)
        uint32_t off = 0;
        for (int i = 0; i < 119; i++) off = next_off(off, false, false);
        off = next_off(off, false, true);
        for (int i = 0; i < 119; i++) off = next_off(off, false, false);
        expect(release_due(off, true), false, "119 off after a recorded present");
        off = next_off(off, false, false);
        expect(release_due(off, true), true, "the 120th off after a recorded present");
    }
    if (!fails) std::puts("PASS: frame generation's images go after 120 presents off, never on a present that recorded");
    return fails ? 1 : 0;
}
'''


def check_order():
    """The idle check after the queue call, with this present's recording, in the present's path"""
    replay = (ROOT / "gpu" / "vk" / "vk_replay.cpp").read_text(encoding="utf-8")
    queue = replay.index("framegen_queue(value);")
    idle = replay.index("framegen_idle(interpolated);")
    if not queue < idle < queue + 900:
        raise SystemExit("test_framegen_idle: framegen_idle(interpolated) is not right after framegen_queue(value)")
    fg = (ROOT / "gpu" / "vk" / "vk_framegen.cpp").read_text(encoding="utf-8")
    idle_fn = block(fg, "void framegen_idle(bool recorded)")
    for part in ("next_off(off, on, recorded)", "release_due(off, fg.w != 0)",
                 "next_off(fsr_off, on && fsr3fi::frames_wanted(), recorded)"):
        if part not in idle_fn:
            raise SystemExit("test_framegen_idle: framegen_idle does not use the rule: %s" % part)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cxx", default="clang++", help="C++ compiler")
    a = p.parse_args()
    check_order()
    cxx = shutil.which(a.cxx)
    if not cxx:
        raise SystemExit("test_framegen_idle: no C++ compiler %r (pass --cxx)" % a.cxx)
    with tempfile.TemporaryDirectory(prefix="framegen-idle-") as temp:
        d = Path(temp)
        src, exe = d / "fixture.cpp", d / "fixture.exe"
        src.write_text(FIXTURE, encoding="utf-8")
        subprocess.run([cxx, "-std=c++17", "-O2", "-I", str(ROOT / "gpu" / "vk"), str(src), "-o", str(exe)], check=True)
        result = subprocess.run([str(exe)], capture_output=True, text=True)
        print(result.stdout, end="")
        if result.returncode:
            raise SystemExit("test_framegen_idle: failed")


if __name__ == "__main__":
    main()
