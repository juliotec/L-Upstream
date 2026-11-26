"""Game counters that add the frame step, or the step times a whole number, cut to whole frames keep counting above 60 fps
(host/frame_whole.h, the hook sites frame.whole*: game/sites/sites.toml, game/oot_sites.c).

The pick-up indicator above Link's head (f_80a87e50) moves the count it shows toward the wallet's by (int)(step * per) and fades
when they meet; the time of day (f_803edf70) adds (int)(2 * step) game seconds a frame. Under a Frame rate cap above 60 the step is
the frame's own time, under 1 (0.42 at 144 fps): cut to 0 every frame, the count never moved and the indicator stayed, and the
clock stood still. With the frames' own time
as the step (carry), hos_frame_whole adds the amount to the counter's own remainder and hands out its whole frames: 144 frames of
60/144 are 60; an amount of 1 or more keeps its fraction too; two counters keep their own remainders; NaN and amounts below zero
give 0. Without it, the amount is cut as the game cuts it (60 fps as before). The water renderer (f_800672e0) runs its ripple
simulation (int)step times a frame the same way, its remainder kept per water object: forty surfaces stepped in turn each still
count their 60 frames a second (its substeps are carried rather than its frames: tests/test_water_substeps.py). Each site is a
replace site on its cut (fcvtzs or fcvtzu of s0); the seven are built with their hooks as the lift lowers them (tests/sitehooks.py) and, side by
side, with the text patch's lines they replaced (nx2/oot_patches.py patch_whole_frames on main), over stand-ins that log each
call to the host: the same registers and the same calls; with a site's flag off, the game's own cut.

Transition check: main's text-patch C kept here as the oracle shows the sites do what it did; remove it once a
deliberate behaviour change needs it.

python -I -B tests/test_frame_whole.py [--cc clang] [--cargo cargo]
"""
import argparse
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves this folder off the path)
from sitecheck import site  # noqa: E402
from whole_sites import SITES, cut, hook_whole, old_whole  # noqa: E402
import sitehooks  # noqa: E402

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

CC = "clang"

FIXTURE = r"""
#include "frame_whole.h"
#include <math.h>
#include <stdio.h>
static int fails;
#define CHECK(c) do { if (!(c)) { printf("FAIL line %d: %s\n", __LINE__, #c); fails++; } } while (0)
static uint32_t bits(float f) { uint32_t b; memcpy(&b, &f, 4); return b; }
int main(void) {
    static FrameWhole w;
    const uint64_t a = 0x1000, b = 0x2000;
    uint32_t n = 0;
    for (int i = 0; i < 144; i++) n += frame_whole(&w, a, bits(60.0f / 144.0f), 1);
    CHECK(n == 60 || n == 59);   /* (a second at 144 fps: 60 frames, give or take the float's last one) */
    n = 0;
    for (int i = 0; i < 1440; i++) n += frame_whole(&w, a, bits(60.0f / 144.0f), 1);
    CHECK(n >= 599 && n <= 601);
    CHECK(frame_whole(&w, b, bits(0.42f), 1) == 0 && frame_whole(&w, b, bits(0.42f), 1) == 0 && frame_whole(&w, b, bits(0.42f), 1) == 1);
    n = 0;
    for (int i = 0; i < 4; i++) n += frame_whole(&w, b + 8, bits(1.25f), 1);   /* (the step times 3: its fractions kept) */
    CHECK(n == 5);
    n = 0;
    for (int i = 0; i < 144; i++) n += frame_whole(&w, b + 16, bits(2.0f * 60.0f / 144.0f), 1);   /* (the time of day) */
    CHECK(n == 120 || n == 119);   /* (a second at 144 fps: the 120 game seconds of one at 60) */
    CHECK(frame_whole(&w, a, bits(0.42f), 0) == 0 && frame_whole(&w, a, bits(1.5f), 0) == 1 && frame_whole(&w, a, bits(2.0f), 0) == 2);
    CHECK(frame_whole(&w, a, bits(-1.0f), 1) == 0 && frame_whole(&w, a, bits(NAN), 1) == 0 && frame_whole(&w, a, bits(0.0f), 1) == 0);
    for (uint64_t k = 0; k < 100; k++) frame_whole(&w, 0x9000 + k, bits(0.5f), 1);   /* (more counters than slots: the oldest go) */
    CHECK(frame_whole(&w, 0x9000 + 99, bits(0.5f), 1) == 1);   /* (a slot still held: 0.5 + 0.5) */
    CHECK(frame_whole(&w, 0x9000, bits(0.5f), 1) == 0);   /* (the oldest's slot was reused: it starts again from 0) */
    /* (a scene's water surfaces, each stepped once a frame at 144 fps: 40 of them, more than the old 16 slots, each still counts
       its 60 frames a second) */
    {
        static FrameWhole water;
        uint32_t passes[40] = {0};
        for (int f = 0; f < 144; f++)
            for (int k = 0; k < 40; k++) passes[k] += frame_whole(&water, 0x7000000 + (uint64_t)k * 0x400, bits(60.0f / 144.0f), 1);
        for (int k = 0; k < 40; k++) CHECK(passes[k] == 60 || passes[k] == 59);
    }
    if (!fails) printf("PASS\n");
    return fails != 0;
}
"""

# (each site's function as authored lifted C: a load, the lines its cut is told by around the cut, a return; "void f_" made
# "void old_" for the text patch's copy, "void off_" for the game's own with the flag off)
FUNCTIONS = "".join("void %s(Ctx *__restrict A64_NOESC c) {\n    c->x[9] = recomp_load32(c,(c->x[19] + 864));\n%s%s%sreturn;\n}\n"
                    % (function, before, cut(reg, signed), after) for _, function, reg, signed, before, after, _, _ in SITES)
SIDE = r"""
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#define A64_NOESC
typedef struct { uint64_t x[32]; uint64_t vreg[32][2]; uint32_t fpcr, fpsr; } Ctx;
static uint64_t log_[16];
static int logged;
static uint64_t recomp_load32(Ctx *c, uint64_t a) { (void)c; return a & 0xff; }
static void recomp_store32(Ctx *c, uint64_t a, uint64_t v) { (void)c; if (logged < 14) log_[logged++] = a, log_[logged++] = v; }
static uint64_t recomp_fp_to_int(uint64_t v, int a, int b, int s, int r, uint32_t fpcr, uint32_t *fpsr) {
    (void)a, (void)b, (void)r, (void)fpcr, (void)fpsr;
    return (uint32_t)(v >> 3) ^ (uint32_t)s;
}
uint32_t hos_frame_whole(uint64_t at, uint32_t bits) {
    if (logged < 14) log_[logged++] = at, log_[logged++] = bits;
    return bits * 5u + (uint32_t)at;
}
uint32_t hos_frame_whole_times(uint64_t at, uint32_t bits, uint32_t times) {
    if (logged < 13) log_[logged++] = at, log_[logged++] = bits, log_[logged++] = times;
    return bits * 7u + times + (uint32_t)at;
}
@NEW@
@OLD@
@OFF@
typedef void (*Fn)(Ctx *);
static int same(Fn a, Fn b, const Ctx *start) {
    Ctx x = *start, y = *start;
    uint64_t la[16];
    int na;
    logged = 0, memset(log_, 0, sizeof log_);
    a(&x);
    memcpy(la, log_, sizeof la), na = logged;
    logged = 0, memset(log_, 0, sizeof log_);
    b(&y);
    return na == logged && !memcmp(la, log_, sizeof la) && !memcmp(&x, &y, sizeof x);
}
int main(void) {
    static const Fn hooked[] = {@HOOKED@}, patched[] = {@PATCHED@}, plain[] = {@PLAIN@};
    int fails = 0;
    for (unsigned k = 0; k < sizeof hooked / sizeof hooked[0]; k++)
        for (uint32_t step = 0x3e000000u; step < 0x40400000u; step += 0x00b00000u) {
            Ctx c;
            memset(&c, 0, sizeof c);
            for (int r = 0; r < 31; r++) c.x[r] = 0x9e3779b97f4a7c15ull * (r + 1);
            c.vreg[0][0] = step, c.vreg[0][1] = 0x1234;
            flags(1);
            if (!same(hooked[k], patched[k], &c)) { printf("FAIL: site %u, step %#x: not as the text patch\n", k, step); fails++; }
            flags(0);
            if (!same(hooked[k], plain[k], &c)) { printf("FAIL: site %u, step %#x: off, not the game's cut\n", k, step); fails++; }
        }
    if (!fails) printf("same\n");
    return fails != 0;
}
"""


class FrameWholeTests(unittest.TestCase):
    def test_counter_keeps_counting_under_one(self):
        compiler = shutil.which(CC)
        if not compiler:
            self.skipTest("no C compiler")
        with tempfile.TemporaryDirectory(prefix="longshot-frame-whole-") as tmp:
            tmp = Path(tmp)
            (tmp / "t.c").write_text(FIXTURE, encoding="utf-8")
            binary = tmp / "t.exe"
            build = subprocess.run([compiler, "-std=gnu11", "-Wall", "-O1", "-I", str(ROOT / "host"), "-I", str(ROOT / "game"), str(tmp / "t.c"),
                                    "-o", str(binary), "-lm"], text=True, capture_output=True)
            self.assertEqual(build.returncode, 0, build.stderr)
            run = subprocess.run([str(binary)], text=True, capture_output=True, timeout=30)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)

    def test_sites(self):
        for name, function, reg, signed, *_ in SITES:
            x = site(name, function, "replace")
            self.assertIn("s0:f32:amount", x.args)
        self.assertEqual(hook_whole('#include "a64.h"\n' + FUNCTIONS)[1], len(SITES))

    def test_as_the_text_patch(self):
        compiler = shutil.which(CC)
        if not compiler:
            self.skipTest("no C compiler")
        lifted = '#include "a64.h"\n' + FUNCTIONS
        new, n = hook_whole(lifted)
        old, m = old_whole(lifted)
        self.assertEqual((n, m), (len(SITES), len(SITES)))
        decl = "uint32_t hos_frame_whole_times(uint64_t at, uint32_t amount_bits, uint32_t times);\n"
        names = [function[2:] for _, function, *_ in SITES]
        idents = [name.replace(".", "_") for name, *_ in SITES]
        code = (SIDE.replace("@NEW@", new.replace('#include "a64.h"\n', "")
                             + "static void flags(int on) { %s }\n" % " ".join("hos_site_on_%s = on;" % i for i in idents))
                .replace("@OLD@", old.split(decl, 1)[1].replace("void f_", "void old_"))
                .replace("@OFF@", FUNCTIONS.replace("void f_", "void plain_"))
                .replace("@HOOKED@", ", ".join("f_" + a for a in names))
                .replace("@PATCHED@", ", ".join("old_" + a for a in names))
                .replace("@PLAIN@", ", ".join("plain_" + a for a in names)))
        with tempfile.TemporaryDirectory(prefix="longshot-frame-whole-") as tmp:
            tmp = Path(tmp)
            (tmp / "side.c").write_text(code, encoding="utf-8")
            build = subprocess.run([compiler, "-std=gnu11", "-Wall", "-Wextra", "-Werror", str(tmp / "side.c"),
                                    *sitehooks.objects(tmp, code), "-o", str(tmp / "side.exe")], text=True,
                                   capture_output=True)
            self.assertEqual(build.returncode, 0, build.stderr)
            run = subprocess.run([str(tmp / "side.exe")], text=True, capture_output=True, timeout=30)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)

    def test_clock_hooks_its_seconds_alone(self):
        # (f_803edf70 cuts into w8 the same way once more, at +0x1f0, in the branch that sets the time from another source:
        #  that cut stays the game's; the site's signature takes the cut with its store to +60)
        row = next(r for r in SITES if r[1] == "f_803edf70")
        other = cut(8, 1) + "    recomp_store32(c,c->x[19] + 12,c->x[8]);\n"
        lifted = ('#include "a64.h"\nvoid f_803edf70(Ctx *__restrict A64_NOESC c) {\n' + cut(8, 1) + row[5] + other
                  + "return;\n}\n")
        out, n = hook_whole(lifted)
        self.assertEqual(n, 1)
        self.assertIn(other, out)
        self.assertEqual(site("frame.wholeTimeOfDay", "f_803edf70", "replace", 0x50).offset, 0x50)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--cc", default="clang")
    ap.add_argument("--cargo")
    args, rest = ap.parse_known_args()
    CC = args.cc
    sitehooks.use(args.cc, args.cargo)
    raise SystemExit(unittest.main(argv=[sys.argv[0]] + rest, verbosity=1))
