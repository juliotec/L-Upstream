"""A horse's jump blend above 60 fps (the hook site horse.jumpStep, game/oot_high_fps.h oot_horse_whole_step).

While a horse gets ready to jump an obstacle, its jump state machine (f_82c59eb0) sets each frame the blend frames into its
JumpReady animation. With a step under 1 (slow motion at 60 fps) it takes them from last frame's plus the step. Above 60 fps
every step is under 1, so the blend frames grew each frame, the blend into the jump hardly started, and Epona galloped through
the fence while her body went over it. The site horse.jumpStep (game/sites/sites.toml), a branch site on the game's
"step < 1" branch (b.pl past the slow-motion branch), takes that branch too when the host's whole-frame test says the step is
the whole frame's (the time object's step).

An authored update with the step test, its branch and a stand-in for the slow-motion branch is built with the branch site's hook
as the lift lowers it (tests/sitehooks.py) (its callback cut from game/oot_sites.c) and, side by side, with the text patch it replaced (OLD_SKIP,
as main had it), each compiled with hos_oot_horse_whole_step cut from game/oot_high_fps.c (guest memory, the time object and
the switches as stand-ins) and run frame by frame, carrying the blend frames from one frame to the next as the game does:
- at 70, 120, 144 and 240 fps, with jitter between the horse's step and the frame's, the blend frames stay the game's own (they
  grew by the step each frame before the fix);
- slow motion above 60 fps (the horse's step under the frame's), slow motion at a cap of 60, a cap of 60, a frame slower than
  60 fps and a time object with no step all take the game's own test;
- the update without the hook (or with its flag off) compiled the same way fails the 120 fps check (the bug, shown without
  the fix).
No game files or lifted function body are used.

Transition check: main's text-patch C kept here as the oracle shows the sites do what it did; remove it once a
deliberate behaviour change needs it.

python -I -B tests/test_horse_jump_step.py [--cc gcc]
"""
import argparse
import subprocess
import sys
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))   # (python -I leaves the check's own folder off the path)
from srccut import block  # noqa: E402
from sitecheck import site, callback  # noqa: E402
import sitehooks  # noqa: E402

HEAD = "void f_82c59eb0(Ctx *__restrict A64_NOESC c) {\n"
# (a step test in the lift's form, the scratch register and the label made up, its flag setting cut short to the stand-in's;
# then its branch past the slow-motion one)
TEST = ("    a64_vset64(c->vreg[11],0x3f800000ULL);\n"
        "    { if (!a64_fcmp32(c,c->vreg[10],c->vreg[11])) { c->n = 0; } }\n")
BRANCH = "if (!c->n) { goto L_80001000; }\n"
# (the text patch's line on main, between the test and its branch)
OLD_SKIP = ("    if (hos_oot_horse_whole_step((uint32_t)c->vreg[10][0], g_module_base + 0x80435000ULL + (int64_t)75927552 + 2192)) "
            "{ goto L_80001000; }   /* (nx2/oot_patches.py: a horse's jump blend above 60 fps) */\n")


def update(between=""):
    return (HEAD + "    c->slow = 0;\n" + TEST + between + BRANCH
            + "    c->slow = 1;   /* (the game's branch for a step under 1: last frame's blend frames plus the step) */\n"
            + "L_80001000:;\n}\n")

FIXTURE = r'''#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <math.h>
#include "oot_high_fps.h"
#define A64_NOESC
typedef struct { uint64_t x[32]; uint64_t vreg[32][2]; uint8_t n; int slow; } Ctx;
static const uint64_t g_module_base = 0;
static void a64_vset64(uint64_t *v, uint64_t bits) { v[0] = bits; v[1] = 0; }
/* (1 when ordered, with n set for less than; 0 for NaN, as the lift's test then leaves n clear) */
static int a64_fcmp32(Ctx *c, const uint64_t *a, const uint64_t *b) {
    const float x = oot_bits_float((uint32_t)a[0]), y = oot_bits_float((uint32_t)b[0]);
    if (x != x || y != y) return 0;
    c->n = x < y;
    return 1;
}
/* (the host file's world: the time object's step at [[0x84c9e890]]+0x10, the cap, no log) */
static uint8_t mem[0x3000];
static uint8_t *guest(uint64_t a) {
    if (a == 0x84c9e890ULL) return mem + 0x100;
    return a < sizeof mem ? mem + a : mem;
}
#define GP(a) ((void *)guest(a))
#define G64(a) (*(uint64_t *)GP(a))
typedef long long LONG64;
#define InterlockedIncrement64(p) (++*(p))
#define InterlockedExchange64(p, v) (*(p) = (v))
static void hos_log(const char *f, ...) { (void)f; }
static int is_timed;
static int timed(void) { return is_timed; }
static int fps_log(void) { return 0; }
@CODE@
static int failures, checks;
static float blend_frames;   /* (the animation variable the state machine sets) */
/* one frame: the horse's step, the frame's (the time object's; 0: none), a cap above 60 or not. The blend frames as the game
   carries them: the slow-motion branch adds the step to last frame's (20 when there were none), the other works them out for
   the blend to end at takeoff (25 here). 1 when the slow-motion branch ran */
static int frame(float step, float frame_step, int timed_now) {
    Ctx c;
    memset(&c, 0, sizeof c);
    is_timed = timed_now;
    G64(0x84c9e890ULL) = frame_step != 0.0f ? 0x1000 : 0;
    G64(0x1000) = 0x2000;
    const uint32_t f = oot_float_bits(frame_step);
    memcpy(mem + 0x2010, &f, 4);
    c.vreg[10][0] = oot_float_bits(step);
    f_82c59eb0(&c);
    blend_frames = c.slow ? (blend_frames > 1.0f ? blend_frames : 20.0f) + step : 25.0f;
    return c.slow;
}
static void run(const char *what, float step, float frame_step, int timed_now, int frames, int slow, float expected) {
    blend_frames = 25.0f;
    int slows = 0;
    for (int i = 0; i < frames; i++) slows += frame(step, frame_step, timed_now);
    checks++;
    if (slows != (slow ? frames : 0) || fabsf(blend_frames - expected) > 1e-3f) {
        printf("FAIL: %s: the slow-motion branch ran %d of %d frames, blend frames %.3f (expected %s, %.3f)\n", what, slows,
               frames, blend_frames, slow ? "every frame" : "none", expected);
        failures++;
    }
}
int main(void) {
    /* (above 60 fps: the whole frame's step) */
    run("120 fps", 0.5f, 0.5f, 1, 40, 0, 25.0f);
    run("70 fps", 60.0f / 70.0f, 60.0f / 70.0f, 1, 40, 0, 25.0f);
    run("144 fps", 60.0f / 144.0f, 60.0f / 144.0f, 1, 40, 0, 25.0f);
    run("240 fps", 0.25f, 0.25f, 1, 40, 0, 25.0f);
    run("120 fps, the horse's step a little under the frame's", 0.5f * 0.9995f, 0.5f, 1, 40, 0, 25.0f);
    run("120 fps, the horse's step a little over the frame's", 0.5f * 1.0005f, 0.5f, 1, 40, 0, 25.0f);
    /* (the game's own test) */
    run("slow motion at 120 fps", 0.25f, 0.5f, 1, 40, 1, 25.0f + 40 * 0.25f);
    run("slow motion at a cap of 60", 0.5f, 1.0f, 0, 40, 1, 25.0f + 40 * 0.5f);
    run("a cap of 60", 1.0f, 1.0f, 0, 40, 0, 25.0f);
    run("a cap of 60, its time object at 0.5", 0.5f, 0.5f, 0, 40, 1, 25.0f + 40 * 0.5f);
    run("a frame slower than 60 fps", 1.25f, 1.25f, 1, 40, 0, 25.0f);
    run("no time object", 0.5f, 0.0f, 1, 40, 1, 25.0f + 40 * 0.5f);
    run("a NaN step", NAN, 0.5f, 1, 1, 0, 25.0f);
    if (!failures) printf("PASS: a horse's jump blend frames (%d checks)\n", checks);
    return failures != 0;
}
'''


def build_and_run(cc, code, directory):
    """code (the host's function and the update) built into the fixture and run"""
    source, binary = Path(directory) / "fixture.c", Path(directory) / "fixture.exe"
    text = FIXTURE.replace("@CODE@", code)
    source.write_text(text, encoding="utf-8")
    subprocess.run([cc, "-std=gnu11", "-Wall", "-Wextra", "-Werror", "-Wno-unused-function", "-Wno-unused-const-variable",
                    "-I", str(ROOT / "host"), "-I", str(ROOT / "game"),
                    str(source), *sitehooks.objects(directory, text), "-o", str(binary), "-lm"], check=True)
    return subprocess.run([str(binary)], text=True, capture_output=True, timeout=30)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cc", default="clang")
    parser.add_argument("--cargo")
    args = parser.parse_args()
    sitehooks.use(args.cc, args.cargo)
    x = site("horse.jumpStep", "f_82c59eb0", "branch", 0xf70)
    text = (ROOT / "game" / "oot_high_fps.c").read_text(encoding="utf-8")
    host = block(text, "int hos_oot_horse_whole_step(uint32_t step_bits, uint64_t slot) {")
    slot = ("\n" + callback("int hos_site_horse_jump_step(") + "\nvolatile uint8_t hos_site_on_horse_jumpStep = %d;\n"
            "int (*volatile hos_site_fn_horse_jumpStep)(Ctx *) = hos_site_horse_jump_step;\n")
    hooked = update(sitehooks.line(x).replace(sitehooks.GOTO, "goto L_80001000;") + "\n")
    with tempfile.TemporaryDirectory(prefix="horse-jump-step-") as directory:
        for what, code in (("the hook", host + slot % 1 + hooked), ("the text patch", host + update(OLD_SKIP))):
            result = build_and_run(args.cc, code, directory)
            print(result.stdout, end="")
            if result.returncode != 0:
                raise SystemExit("FAIL: a horse's jump blend frames above 60 fps (%s)" % what)
        for what, code in (("the update without the hook", update()), ("the hook off", host + slot % 0 + hooked)):
            native = build_and_run(args.cc, code, directory)
            if native.returncode == 0 or "FAIL: 120 fps:" not in native.stdout:
                raise SystemExit("FAIL: %s did not fail the 120 fps check" % what)
        print("PASS: the update without the hook, or with it off, compiles and fails the 120 fps check (its blend frames grow by the step)")


if __name__ == "__main__":
    main()
