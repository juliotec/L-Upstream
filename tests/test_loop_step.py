"""The frozen Zoras stand still above a 60 frame-rate cap (#988).

An animation that loops over one frame or less is one pose (the Zoras under the ice in adult Zora's Domain: start 0, end 1). At 60
fps the clock's step is 1 and every update lands on the same phase. Above a 60 cap the step is the frame's own time (0.36 at 165
fps), the phase moved by it each frame and the pose twitched. The game makes the time a skeleton is drawn at in two places:
f_802cfca0 advances the clock of each animation entry (time +4, start +16, end +20, in two places that multiply the step), and
f_80944aa0 writes the time into the list the skeletons read, wrapped into the range kept beside it (+4, +8).

This compiles the production hook sites (anim.loop*: replace sites on the two multiplies and the store, whose callbacks do the
instruction's work with the host's step or time, as the lift lowers them (tests/sitehooks.py) with game/oot_sites.c's callbacks) in authored
updates cut the way the lifted ones are, with host/loop_step.h's functions the callbacks reach; and, side by side, the text
patch's lines they replaced (OLD_*, as main had them). The same checks run on the updates without the hooks (and with them
off): they must fail there, or the test checks nothing. And each of the three places is run alone, hooked and as the patch
had it, from contexts of random registers and memory: the same context (every register, s8 and s0 among them) and the same
memory after. No game files or lifted function body are used.

Transition check: main's text-patch C kept here as the oracle shows the sites do what it did; remove it once a
deliberate behaviour change needs it.

python -I -B tests/test_loop_step.py [--cc clang] [--cargo cargo]
"""
import argparse
import subprocess
import sys
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves this folder off the path)
from sitecheck import site, callback, slot  # noqa: E402
import sitehooks  # noqa: E402

HEADER = ROOT / "host" / "loop_step.h"

SITE_1 = ("if (((c->x[9] >> 3) & 1)) { goto L_80001000; }\n"
          "    {uint64_t _a=0,_b=0,_z,_v;memcpy(&_a,c->vreg[8],4);memcpy(&_b,c->vreg[1],4);_z=(_a^_b)&0x80000000ULL;\n"
          "    _v=a64_fpx_mul32_fma32(c,_a,_b,_z);\n"
          "    a64_vset64(c->vreg[2],_v);}\n")
# (the second is fmul s1, s8, s0, as anim.loopStepOther's signature says: the authored update then hands s1 to the advance)
SITE_2 = ("if (((c->x[9] >> 3) & 1)) { goto L_80001100; }\n"
          "    {uint64_t _a=0,_b=0,_z,_v;memcpy(&_a,c->vreg[8],4);memcpy(&_b,c->vreg[0],4);_z=(_a^_b)&0x80000000ULL;\n"
          "    _v=a64_fpx_mul32_fma32(c,_a,_b,_z);\n"
          "    a64_vset64(c->vreg[1],_v);}\n")
CLOCK = ('#include "a64.h"\n'
         "void f_802cfca0(Ctx *__restrict A64_NOESC c) {\n"
         "    c->x[9]=recomp_load32(c,c->x[20] + 0);\n"
         "    if (c->x[10] == 1) { goto L_80001000; }\n"
         + SITE_1 +
         "goto L_advance;\n"
         "L_80001000:;\n"
         "    c->x[9] = 0;\n"
         + SITE_2 +
         "    a64_vset64(c->vreg[2],c->vreg[1][0]);\n"
         "goto L_advance;\n"
         "L_80001100:;\n"
         "    a64_vset64(c->vreg[2],0);\n"
         "L_advance:;\n"
         "    clock_advance(c);\n"
         "}\n"
         "void f_90000000(Ctx *__restrict A64_NOESC c) {\n"
         "    c->x[9]=recomp_load32(c,c->x[20] + 0);\n"
         + SITE_1 + "L_80001000:;\n}\n")
STORE = "    { { uint64_t _v=0; memcpy(&_v,&c->vreg[0][0],4); recomp_store32(c,(c->x[22]+0ULL),_v); } }\n"
POSE = ('#include "a64.h"\n'
        "void f_80944aa0(Ctx *__restrict A64_NOESC c) {\n" + STORE + "}\n"
        "void f_90000001(Ctx *__restrict A64_NOESC c) {\n" + STORE + "}\n")
LOAD8 = "memcpy(&_a,c->vreg[8],4);"
# (the text patch's lines on main: the step through the host in the multiply's own copy of it, and the time in the store's)
OLD_STEP = (LOAD8 + " _a=hos_loop_step(c->x[20], (uint32_t)_a, (uint32_t)recomp_load32(c,c->x[20] + 16), "
            "(uint32_t)recomp_load32(c,c->x[20] + 20));   /* (nx2/oot_patches.py: a loop of one frame by whole frames) */\n    ")
OLD_STORE = ("    { { uint64_t _v=0; memcpy(&_v,&c->vreg[0][0],4); _v=hos_loop_pose((uint32_t)_v, (uint32_t)recomp_load32(c,c->x[22] + 4), "
             "(uint32_t)recomp_load32(c,c->x[22] + 8));   /* (nx2/oot_patches.py: a loop of one frame by its start) */ "
             "recomp_store32(c,(c->x[22]+0ULL),_v); } }\n")
SITES = (("anim.loopStep", "f_802cfca0", 0x164), ("anim.loopStepOther", "f_802cfca0", 0x530), ("anim.loopPose", "f_80944aa0", 0x3b0))
CALLBACKS = ("static void loop_step_mul(", "int hos_site_loop_step(", "int hos_site_loop_step_other(", "int hos_site_loop_pose(")
CONTEXT = """#pragma once
#include <stdint.h>
#include <string.h>
#define A64_NOESC
typedef struct {
    uint64_t x[32], vreg[32][2];
    uint32_t fpcr, fpsr;
} Ctx;
"""
FIXTURE = r'''#define _CRT_SECURE_NO_WARNINGS
#include <math.h>
#include <stdio.h>
#include "a64.h"
@HOOKS@
static uint8_t heap[4096];
static int failures;
static const char *failed[64];
static int failed_count[64], failed_kinds;
static void fail(const char *what) {   /* (each failing check once, with how often) */
    failures++;
    for (int i = 0; i < failed_kinds; i++) if (failed[i] == what) { failed_count[i]++; return; }
    if (failed_kinds < 64) failed[failed_kinds] = what, failed_count[failed_kinds++] = 1;
}
#define CHECK(test, what) do { if (!(test)) fail(what); } while (0)
static uint32_t bits(float f) { uint32_t b; memcpy(&b, &f, 4); return b; }
static float real(uint32_t b) { float f; memcpy(&f, &b, 4); return f; }
uint64_t recomp_load32(Ctx *c, uint64_t at) { uint32_t v; (void)c; memcpy(&v, heap + at, 4); return v; }
void recomp_store32(Ctx *c, uint64_t at, uint64_t v) { uint32_t w = (uint32_t)v; (void)c; memcpy(heap + at, &w, 4); }
void a64_vset64(uint64_t *v, uint64_t value) { v[0] = value; v[1] = 0; }
uint64_t a64_fpx_mul32_fma32(Ctx *c, uint64_t a, uint64_t b, uint64_t z) {
    (void)c; (void)z;
    return bits(real((uint32_t)a) * real((uint32_t)b));
}
uint64_t recomp_fpe_fma32(Ctx *c, uint64_t a, uint64_t b, uint64_t z) { return a64_fpx_mul32_fma32(c, a, b, z); }
/* (the game's own wrap of a clock that passes its end: back into [start, end)) */
static void clock_advance(Ctx *c) {
    float time = real((uint32_t)recomp_load32(c, c->x[20] + 4)) + real((uint32_t)c->vreg[2][0]);
    const float start = real((uint32_t)recomp_load32(c, c->x[20] + 16)), end = real((uint32_t)recomp_load32(c, c->x[20] + 20));
    if (time >= end) time = start + fmodf(time - start, end - start);
    recomp_store32(c, c->x[20] + 4, bits(time));
}
@UPDATES@

enum { A = 0x100, B = 0x200, C = 0x300, STATE = 0x400 };
static void entry(uint64_t at, float time, float start, float end) {
    Ctx c = {0};
    (void)c;
    recomp_store32(&c, at + 0, 0x403);
    recomp_store32(&c, at + 4, bits(time));
    recomp_store32(&c, at + 16, bits(start));
    recomp_store32(&c, at + 20, bits(end));
}
/* (one update of the entry's clock by the frame step: the time it then holds) */
static float update(uint64_t at, float step, int mode) {
    Ctx c = {0};
    c.x[20] = at;
    c.x[10] = (uint64_t)mode;
    c.vreg[8][0] = bits(step);
    c.vreg[1][0] = c.vreg[0][0] = bits(1.0f);
    f_802cfca0(&c);
    return real((uint32_t)recomp_load32(&c, at + 4));
}
static float phase(float t) { return t - floorf(t); }
/* (the time written for the skeleton: the incoming one, the range [min, max] kept beside it) */
static float pose(float incoming, float min, float max) {
    Ctx c = {0};
    c.x[22] = STATE;
    recomp_store32(&c, STATE + 0, bits(-1.0f));
    recomp_store32(&c, STATE + 4, bits(min));
    recomp_store32(&c, STATE + 8, bits(max));
    c.vreg[0][0] = bits(incoming);
    f_80944aa0(&c);
    return real((uint32_t)recomp_load32(&c, STATE));
}

int main(void) {
    const float frame = 60.0f / 165.0f;   /* (the step at 165 fps) */
    for (int mode = 0; mode < 2; mode++) {
        /* the Zoras: a loop of one frame, a clock a different phase of it each frame at 165 fps */
        timed = 1;
        entry(A, 0.0057f, 0.0f, 1.0f);
        float lo = 2.0f, hi = -1.0f;
        for (int i = 0; i < 165; i++) {
            const float p = phase(update(A, frame, mode));
            if (p < lo) lo = p;
            if (p > hi) hi = p;
        }
        CHECK(hi - lo < 0.01f, "a loop of one frame must stay on one phase at 165 fps");
        /* two entries in turn each keep their own fractions: still one phase each */
        entry(B, 0.25f, 0.0f, 1.0f); entry(C, 0.75f, 0.0f, 1.0f);
        for (int i = 0; i < 165; i++) {
            const float pb = phase(update(B, frame, mode)), pc = phase(update(C, frame, mode));
            CHECK(fabsf(pb - 0.25f) < 0.01f && fabsf(pc - 0.75f) < 0.01f, "each loop of one frame keeps its own phase");
        }
        /* a loop of exactly one frame is a pose, a loop of a frame and a half is not */
        entry(A, 2.5f, 2.0f, 3.0f);
        update(A, frame, mode); update(A, frame, mode); update(A, frame, mode);
        CHECK(fabsf(phase(real((uint32_t)recomp_load32(&(Ctx){0}, A + 4))) - 0.5f) < 0.01f, "the loop from 2 to 3 is a pose too");
        entry(A, 0.0f, 0.0f, 1.5f);
        update(A, frame, mode);
        CHECK(fabsf(real((uint32_t)recomp_load32(&(Ctx){0}, A + 4)) - frame) < 1e-4f, "a loop longer than a frame keeps the fractional step");
        /* a long loop keeps the game's step: a second at 165 fps is 60 frames of time */
        entry(A, 0.0f, 0.0f, 1000.0f);
        float t = 0;
        for (int i = 0; i < 165; i++) t = update(A, frame, mode);
        CHECK(fabsf(t - 60.0f) < 0.05f, "a long loop must keep moving by the frames' own time");
        /* at a cap of 60 nothing changes: the step the game has */
        timed = 0;
        entry(A, 0.0057f, 0.0f, 1.0f);
        CHECK(fabsf(update(A, 0.4f, mode) - 0.4057f) < 1e-4f, "at a cap of 60 a loop of one frame moves by the game's step");
        entry(A, 0.0057f, 0.0f, 1.0f);
        CHECK(fabsf(phase(update(A, 1.0f, mode)) - 0.0057f) < 1e-4f, "at a cap of 60 the step of 1 stays on its phase");
        timed = 1;
        /* a step of 0 (paused) and one under 0 are the game's */
        entry(A, 0.3f, 0.0f, 1.0f);
        CHECK(fabsf(update(A, 0.0f, mode) - 0.3f) < 1e-6f, "a paused clock does not move");
    }
    /* the whole frames handed out add up to the frames' own time, whatever its steps */
    {
        float sum = 0;
        for (int i = 0; i < 1650; i++) sum += real(hos_loop_step(0x778, bits(frame), bits(0.0f), bits(1.0f)));
        CHECK(fabsf(sum - 600.0f) < 1.0f, "ten seconds at 165 fps are 600 whole frames");
    }
    /* the time a skeleton is drawn at */
    timed = 1;
    CHECK(pose(0.7f, 0.0f, 1.0f) == 0.0f, "a loop of one frame is drawn at its start");
    CHECK(pose(0.7f, 0.25f, 1.0f) == 0.25f, "a loop of one frame is drawn at its own start");
    CHECK(pose(0.7f, 0.0f, 100.0f) == 0.7f, "a long loop is drawn at its time");
    CHECK(pose(0.7f, 0.0f, 1.5f) == 0.7f, "a loop of a frame and a half is drawn at its time");
    CHECK(pose(0.7f, 1.0f, 0.0f) == 0.7f, "a range that is the wrong way round is left alone");
    timed = 0;
    CHECK(pose(0.7f, 0.0f, 1.0f) == 0.7f, "at a cap of 60 the time is the game's");
    for (int i = 0; i < failed_kinds; i++) printf("FAIL: %s (%d times)\n", failed[i], failed_count[i]);
    if (!failures) puts("PASS: loop of one frame");
    return failures != 0;
}
'''
WITH_HOOKS = r'''#include "loop_step.h"
static LoopStep loops;
static int timed = 1;
uint32_t hos_loop_step(uint64_t at, uint32_t step, uint32_t start, uint32_t end) { return loop_step(&loops, at, step, start, end, timed); }
uint32_t hos_loop_pose(uint32_t time, uint32_t start, uint32_t end) { return loop_pose(time, start, end, timed); }
'''
NO_HOOKS = r'''static int timed = 1;
uint32_t hos_loop_step(uint64_t at, uint32_t step, uint32_t start, uint32_t end) { (void)at; (void)start; (void)end; return step; }
uint32_t hos_loop_pose(uint32_t time, uint32_t start, uint32_t end) { (void)start; (void)end; return time; }
'''


def tbz(site_text):
    return site_text.split("\n")[0] + "\n"


def head(on=1):
    """the callbacks (cut from game/oot_sites.c) and the sites' slots, for a fixture that has the host's functions"""
    by = [site(name, function, "replace", offset) for name, function, offset in SITES]
    return ("uint32_t hos_loop_step(uint64_t at, uint32_t step, uint32_t start, uint32_t end);\n"
            "uint32_t hos_loop_pose(uint32_t time, uint32_t start, uint32_t end);\n"
            + "".join(callback(fn) + "\n" for fn in CALLBACKS) + "".join(slot(x, on) for x in by)), by


def hooked_updates(on=1):
    """(the authored updates with the sites' hooks as the emit puts them: each multiply and the pose's store in a replace
    site's hook), the callbacks and slots ahead of them; the unrelated functions as they are"""
    calls, (step, other, pose_site) = head(on)
    clock = CLOCK
    for text, x in ((SITE_1, step), (SITE_2, other)):
        mul = text[len(tbz(text)):]
        clock = clock.replace(text, tbz(text) + sitehooks.line(x) + "\n" + mul + sitehooks.REPLACE_END + "\n", 1)
    pose = POSE.replace("\n" + STORE, "\n" + sitehooks.line(pose_site) + "\n" + STORE + sitehooks.REPLACE_END + "\n", 1)
    return clock.replace('#include "a64.h"\n', '#include "a64.h"\n' + calls, 1), pose


SIDE = r"""#include <stdint.h>
#include <stdio.h>
#include <string.h>
#define A64_NOESC
typedef struct { uint64_t x[32], vreg[32][2]; uint32_t fpcr, fpsr; } Ctx;
static uint8_t heap[4096];
uint64_t recomp_load32(Ctx *c, uint64_t at) { uint32_t v; (void)c; memcpy(&v, heap + (at & 0xffc), 4); return v; }
void recomp_store32(Ctx *c, uint64_t at, uint64_t v) { uint32_t w = (uint32_t)v; (void)c; memcpy(heap + (at & 0xffc), &w, 4); }
void a64_vset64(uint64_t *v, uint64_t value) { v[0] = value; v[1] = 0; }
uint64_t a64_fpx_mul32_fma32(Ctx *c, uint64_t a, uint64_t b, uint64_t z) {
    c->fpsr ^= (uint32_t)(a * 3 + b * 5 + z);   /* (a stand-in whose result and FPSR depend on every operand) */
    return (a * 0x9e3779b1u + b) & 0xffffffffu;
}
uint64_t recomp_fpe_fma32(Ctx *c, uint64_t a, uint64_t b, uint64_t z) { return a64_fpx_mul32_fma32(c, a, b, z); }
uint32_t hos_loop_step(uint64_t at, uint32_t step, uint32_t start, uint32_t end) { return step * 7 + start * 3 + end + (uint32_t)at; }
uint32_t hos_loop_pose(uint32_t time, uint32_t start, uint32_t end) { return time * 5 + start * 11 + end; }
@HEAD@
@PLACES@
typedef void (*Fn)(Ctx *);
int main(void) {
    static const Fn hooked[] = {new_0, new_1, new_2}, patched[] = {old_0, old_1, old_2};
    int fails = 0;
    for (int k = 0; k < 3; k++)
        for (int round = 0; round < 64; round++) {
            Ctx a;
            uint64_t r = 0x9e3779b97f4a7c15ull * (uint64_t)(round + 1) + (uint64_t)k;
            for (int i = 0; i < 32; i++) {
                r = r * 6364136223846793005ull + 1442695040888963407ull, a.x[i] = r;
                r = r * 6364136223846793005ull + 1442695040888963407ull, a.vreg[i][0] = r;
                r = r * 6364136223846793005ull + 1442695040888963407ull, a.vreg[i][1] = r;
            }
            a.fpcr = (uint32_t)r, a.fpsr = (uint32_t)(r >> 32);
            Ctx b = a;
            uint8_t ha[sizeof heap];
            for (unsigned i = 0; i < sizeof heap; i++) heap[i] = (uint8_t)(i * 13 + round);
            hooked[k](&a);
            memcpy(ha, heap, sizeof heap);
            for (unsigned i = 0; i < sizeof heap; i++) heap[i] = (uint8_t)(i * 13 + round);
            patched[k](&b);
            if (memcmp(&a, &b, sizeof a) || memcmp(ha, heap, sizeof heap)) {
                printf("FAIL: place %d, round %d: the context or memory differs from the text patch's\n", k, round);
                fails++;
            }
        }
    if (!fails) printf("same\n");
    return fails != 0;
}
"""


def side_by_side(cc, directory):
    """each place alone, hooked and as the patch had it, from the same random context and memory: the same after"""
    calls, sites = head()
    places = []
    for k, (x, text, old) in enumerate(((sites[0], SITE_1[len(tbz(SITE_1)):], SITE_1[len(tbz(SITE_1)):].replace(LOAD8, OLD_STEP, 1)),
                                        (sites[1], SITE_2[len(tbz(SITE_2)):], SITE_2[len(tbz(SITE_2)):].replace(LOAD8, OLD_STEP, 1)),
                                        (sites[2], STORE, OLD_STORE))):
        places.append("static void new_%d(Ctx *c) {\n%s\n%s%s\n}\nstatic void old_%d(Ctx *c) {\n%s}\n"
                      % (k, sitehooks.line(x), text, sitehooks.REPLACE_END, k, old))
    source, binary = directory / "side.c", directory / "side.exe"
    text = SIDE.replace("@HEAD@", calls).replace("@PLACES@", "".join(places))
    source.write_text(text, encoding="utf-8")
    subprocess.run([cc, "-std=gnu11", "-Wall", "-Wextra", "-Werror", "-Wno-unused-function", str(source),
                    *sitehooks.objects(directory, text), "-o", str(binary)], check=True)
    r = subprocess.run([str(binary)], text=True, capture_output=True, timeout=30)
    if r.returncode != 0 or "same" not in r.stdout:
        raise SystemExit("FAIL: the sites and the text patch leave different contexts or memory:\n" + r.stdout + r.stderr)
    print("PASS: each place through its site leaves the context and memory the text patch left")


def old_updates():
    """the authored updates with the text patch's lines in place, as main made them"""
    clock = CLOCK.replace(SITE_1, SITE_1.replace(LOAD8, OLD_STEP, 1), 1).replace(SITE_2, SITE_2.replace(LOAD8, OLD_STEP, 1), 1)
    return clock, POSE.replace(STORE, OLD_STORE, 1)


def run(cc, directory, clock, pose):
    (directory / "a64.h").write_text(CONTEXT, encoding="utf-8")
    hooks = WITH_HOOKS if HEADER.exists() else NO_HOOKS
    source, binary = directory / "fixture.c", directory / "fixture.exe"
    # (the patched text starts with the include the fixture has already: its second copy is left out)
    updates = clock.replace('#include "a64.h"\n', "", 1) + pose.replace('#include "a64.h"\n', "", 1)
    text = FIXTURE.replace("@HOOKS@", hooks).replace("@UPDATES@", updates)
    source.write_text(text, encoding="utf-8")
    command = [cc, "-std=c11", "-Wall", "-Wextra", "-Werror", "-Wno-unused-function", "-I", str(ROOT / "host"), "-I", str(ROOT / "game"), str(source),
               *sitehooks.objects(directory, text), "-o", str(binary), "-lm"]
    subprocess.run(command, check=True)
    return subprocess.run([str(binary)], text=True, capture_output=True, timeout=30)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cc", default="clang")
    parser.add_argument("--cargo")
    args = parser.parse_args()
    sitehooks.use(args.cc, args.cargo)
    with tempfile.TemporaryDirectory(prefix="loop-step-") as directory:
        directory = Path(directory)
        for what, (clock, pose) in (("the hooks", hooked_updates()), ("the text patch", old_updates())):
            fixed = run(args.cc, directory, clock, pose)
            sys.stdout.write(fixed.stdout)
            if fixed.returncode != 0:
                raise SystemExit("FAIL: the updates with %s do not keep a loop of one frame on one phase above a cap of 60" % what)
        # The negative controls must compile, then fail the same behavioral checks.
        for what, (clock, pose) in (("the updates without the hooks", (CLOCK, POSE)), ("the hooks off", hooked_updates(0))):
            native = run(args.cc, directory, clock, pose)
            if native.returncode == 0 or "must stay on one phase" not in native.stdout:
                raise SystemExit("FAIL: %s did not fail the one-phase check" % what)
        print("PASS: the updates without the hooks, or with them off, are rejected by the same checks")
        side_by_side(args.cc, directory)
    clock, pose = hooked_updates()
    assert clock.split("void f_90000000")[1] == CLOCK.split("void f_90000000")[1]  # the authored unrelated function is left alone
    assert pose.split("void f_90000001")[1] == POSE.split("void f_90000001")[1]


if __name__ == "__main__":
    main()
