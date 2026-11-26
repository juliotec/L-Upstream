"""Above a 60 fps cap the sound job runs at 60 Hz and the physics solver keeps whole-frame steps (game/oot_high_fps.h, the hook
sites sound.*, music.* and physics.solverSteps*: game/sites/sites.toml, game/oot_sites.c).

Sound: under a Frame rate cap above 60 the frame step is the frame's own time (0.5 at 120 fps). The sound manager's job (f_80435300)
fed steps under 1 left sounds unreleased at area changes until the game's voice budget was full and sounds, then the music,
stopped. The gate runs phase 0 once a whole step has built up (120 frames of 0.5: 60 runs; 144 of 60/144: 60), the other phases
follow phase 0's choice, the engine is handed a whole step then, and a step of 1 or more, 0 or below, NaN, or no frame time at
all (a cap of 60) runs every frame with the game's own step. A step of 1 or more keeps the remainder, so around 60 fps (0.98 and
1.02 alternating, 0.9 and 1.1, or mean 1 with jitter) the sound clock keeps the real one; a cap of 60 runs every phase, before
the first phase 0 too and after a phase 0 that held the job.

Solver: the game cuts frame step * n + 0.001 to its solver steps (n a 60 fps frame: 8 at 60 fps, 3 or 4 at 120 as the step
wanders; with n = 4, 0 above 240). With the frames' own time and a step under 1 it is rounded up (10% leeway for the frame time's
jitter) and at least 1, never under the game's; a step of 1 or more, or no frame time (a cap of 60), keeps the game's count. The
two places are after sites on the cut (fcvtzs w1, s0); each is built with its hook as the lift lowers it (tests/sitehooks.py) and, side by side, with the text
patch's line it replaced (OLD_SOLVER): the same registers for every input.

Music manager: its update (f_83373400, game::Bgm) runs inside the gated job but reads the step (+16) and the seconds (+24) from the
game's time object itself. Lifted as the game has it, with the site music.time's hook after its two loads as the lifted code runs
it, and called on the frames the gate runs it, its clock
keeps the real one: at 120 and 240 fps a whole step and that step's seconds each run (handed the frame's own, it ran at 0.5 and
0.25 of real time); at 60 fps, on a slow frame, and at a cap of 60 it reads the game's own values.

Music manager's change flags: the battle manager sets a byte of them ([[0x84c9dda0]]+0x4898: the battle music's name changed,
its distance volume, ...) for one game frame, and the music manager syncs a property for each bit it finds set. Held to 60 Hz it
saw a frame's flags only on the frames it ran: at 120 fps half the changes were lost (the battle music kept playing after the
fight, or stayed at a lower volume). The whole host file is compiled with stand-ins for guest memory and its lock, the update's
four flag reads (lifted as the game has them) with the hooks of music.update (its entry) and music.changeFlags* (after each read),
and frames run through the gate: a bit set for one frame is handled once, whether the gate ran the job on that frame or held it,
at 120 and 240 fps and at a cap of 60.

The sites replaced text patches (nx2/oot_patches.py patch_sound_job, patch_bgm_time, patch_bgm_flags and patch_solver_steps,
deleted): the job and the update are each built with the lowered hooks and, side by side, with the patches' lines as main had
them (OLD_*), over stand-ins that log every call to the host: the same registers, the same calls with the same arguments.

Transition check: main's text-patch C kept here as the oracle shows the sites do what it did; remove it once a
deliberate behaviour change needs it.

python -I -B tests/test_high_fps_fixes.py [--cc clang] [--cargo cargo]
"""
import argparse
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))   # (python -I leaves the check's own folder off the path)
from srccut import block  # noqa: E402
from sitecheck import site, callback, engine_fn, slot  # noqa: E402
import sitehooks  # noqa: E402

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

CC = "clang"

FIXTURE = r"""
#include "oot_high_fps.h"
#include <stdio.h>
static int fails;
#define CHECK(c) do { if (!(c)) { printf("FAIL line %d: %s\n", __LINE__, #c); fails++; } } while (0)
static uint32_t bits(float f) { return oot_float_bits(f); }
static int runs(float step, int frames, int timed) {
    OotSoundGate g = OOT_SOUND_GATE_INIT;
    int n = 0;
    for (int i = 0; i < frames; i++) {
        const int run = oot_sound_gate(&g, 0, step, timed);
        CHECK(oot_sound_gate(&g, 1, step, timed) == run);   /* (phase 1 follows phase 0) */
        n += run;
    }
    return n;
}
/* (the sound engine's time over these frames' steps against the real time: what it is handed each frame the job runs) */
static double sound_clock(const float *steps, int n) {
    OotSoundGate g = OOT_SOUND_GATE_INIT;
    double real = 0, sound = 0;
    for (int i = 0; i < n; i++) {
        real += steps[i];
        if (oot_sound_gate(&g, 0, steps[i], 1)) sound += oot_bits_float(oot_sound_step(oot_float_bits(steps[i]), 1));
    }
    return sound / real;
}
int main(void) {
    CHECK(runs(0.5f, 120, 1) == 60);
    const int n144 = runs(60.0f / 144.0f, 144, 1);
    CHECK(n144 == 60 || n144 == 59);
    CHECK(runs(60.0f / 1000.0f, 1000, 1) >= 59 && runs(60.0f / 1000.0f, 1000, 1) <= 60);
    CHECK(runs(0.5f, 120, 0) == 120);   /* (a cap of 60: every frame) */
    CHECK(runs(1.0f, 60, 1) == 60 && runs(1.5f, 60, 1) == 60);
    CHECK(runs(0.0f, 60, 1) == 60 && runs(-1.0f, 60, 1) == 60 && runs(NAN, 60, 1) == 60);
    {   /* (a whole step after partial ones runs with the game's step and keeps the remainder) */
        OotSoundGate g = OOT_SOUND_GATE_INIT;
        CHECK(oot_sound_gate(&g, 0, 0.75f, 1) == 0 && oot_sound_gate(&g, 2, 0.75f, 1) == 0);
        CHECK(oot_sound_gate(&g, 0, 1.0f, 1) == 1 && g.part == 0.75f && oot_sound_gate(&g, 1, 1.0f, 1) == 1);
        CHECK(oot_sound_gate(&g, 0, 0.75f, 1) == 1 && g.part == 0.5f);
        CHECK(oot_sound_gate(&g, 0, 0.0f, 1) == 1 && g.part == 0.0f);   /* (a step of 0: as the game has it, the remainder cleared) */
    }
    {   /* (around 60 fps the sound clock keeps the real one: dropping the remainder at each whole step made it 0.51 of it) */
        static float steps[20000];
        for (int i = 0; i < 20000; i++) steps[i] = i & 1 ? 1.02f : 0.98f;
        double r = sound_clock(steps, 20000);
        CHECK(r > 0.999 && r < 1.001);
        for (int i = 0; i < 20000; i++) steps[i] = i & 1 ? 1.1f : 0.9f;
        r = sound_clock(steps, 20000);
        CHECK(r > 0.999 && r < 1.001);
        uint32_t x = 12345;   /* (mean 1, about 0.03 apart: a sum of uniforms) */
        for (int i = 0; i < 20000; i++) {
            float u = 0;
            for (int k = 0; k < 12; k++) x = x * 1664525u + 1013904223u, u += (float)(x >> 8) / 16777216.0f;
            steps[i] = 1.0f + (u - 6.0f) * 0.03f;
        }
        r = sound_clock(steps, 20000);
        CHECK(r > 0.999 && r < 1.001);
        for (int i = 0; i < 20000; i++) steps[i] = 0.5f;
        r = sound_clock(steps, 20000);
        CHECK(r > 0.999 && r < 1.001);
    }
    {   /* (a cap of 60 runs every phase: before any phase 0, and after a phase 0 that held the job at a cap above 60) */
        OotSoundGate g = OOT_SOUND_GATE_INIT;
        CHECK(oot_sound_gate(&g, 1, 1.0f, 0) == 1 && oot_sound_gate(&g, 2, 1.0f, 0) == 1);
        OotSoundGate h = OOT_SOUND_GATE_INIT;
        CHECK(oot_sound_gate(&h, 1, 1.0f, 1) == 1);   /* (above 60, before the first phase 0: it runs) */
        CHECK(oot_sound_gate(&h, 0, 0.5f, 1) == 0 && oot_sound_gate(&h, 1, 0.5f, 0) == 1 && h.part == 0.0f);   /* (the cap changed) */
        CHECK(oot_sound_gate(&h, 2, 0.5f, 0) == 1 && oot_sound_gate(&h, 0, 0.5f, 0) == 1);
    }
    CHECK(oot_sound_step(bits(0.5f), 1) == bits(1.0f));
    CHECK(oot_sound_step(bits(0.5f), 0) == bits(0.5f));
    CHECK(oot_sound_step(bits(1.25f), 1) == bits(1.25f) && oot_sound_step(bits(0.0f), 1) == bits(0.0f));
    /* (the game's: (int)(step * n + 0.001), n the steps of a 60 fps frame) */
    #define GAME(step, n) ((int32_t)((step) * (n) + 0.001f))
    #define OURS(step, n, timed) oot_solver_steps(GAME(step, n), bits((step) * (n) + 0.001f), bits(n), timed)
    CHECK(OURS(1.0f, 4.0f, 1) == 4 && OURS(1.0f, 8.0f, 1) == 8 && OURS(1.0f, 8.0f, 0) == 8);
    CHECK(OURS(0.5f, 4.0f, 1) == 2 && OURS(0.49f, 4.0f, 1) == 2 && OURS(0.52f, 4.0f, 1) == 2 && GAME(0.49f, 4.0f) == 1);
    CHECK(OURS(0.49f, 8.0f, 1) == 4 && OURS(0.5f, 8.0f, 1) == 4 && GAME(0.49f, 8.0f) == 3);
    CHECK(OURS(0.25f, 4.0f, 1) == 1 && OURS(0.24f, 4.0f, 1) == 1 && OURS(0.06f, 4.0f, 1) == 1 && GAME(0.24f, 4.0f) == 0);
    CHECK(OURS(60.0f / 144.0f, 4.0f, 1) == 2 && OURS(60.0f / 90.0f, 4.0f, 1) == 3);
    CHECK(OURS(0.49f, 4.0f, 0) == 1 && OURS(0.06f, 4.0f, 0) == 0);   /* (a cap of 60: the game's count) */
    CHECK(OURS(1.6f, 4.0f, 1) == 6 && OURS(1.708f, 8.0f, 1) == 13);   /* (a slow frame: the game's count) */
    CHECK(oot_solver_steps(3, bits(NAN), bits(4.0f), 1) == 3 && oot_solver_steps(0, bits(-1.0f), bits(4.0f), 1) == 0);
    CHECK(oot_solver_steps(1, bits(2.0f), bits(0.0f), 1) == 1);   /* (no steps a frame: the game's) */
    if (!fails) printf("PASS\n");
    return fails != 0;
}
"""

# (the sound job's head and phase 0's load of the step, made up in the lift's form: the phase in w2, the time object's pointer
# followed to its step at +0x10 in s0, the registers the callbacks read; the text patch's lines on main: OLD_GATE first in the
# job, OLD_STEP_LOAD in place of the step's load)
SLOT = "g_module_base + 0x80435000ULL + (int64_t)75927552"
SOUND_HEAD = "void f_80435300(Ctx *__restrict A64_NOESC c) {\n"
STEP_LOAD = "    { { uint64_t _v=recomp_load32(c,(c->x[10]+16ULL)); a64_vset64(c->vreg[0],_v); } }\n"
SOUND_BODY = ("    /* nop/hint */\nif ((uint32_t)c->x[2] != 0) { goto L_1; }\n"
              "    c->x[10]=" + SLOT + " + 2192;\n"
              "    c->x[10]=recomp_load64(c,c->x[10] + 0);\n"
              "    c->x[10]=recomp_load64(c,c->x[10] + 0);\n" + STEP_LOAD + "    sound_engine(c);\nL_1:;\n    return;\n}\n")
OLD_GATE = ("    if (!hos_oot_sound_gate(c, (uint32_t)c->x[2], " + SLOT + " + 2192)) return;"
            "   /* (nx2/oot_patches.py: the sound job at 60 Hz above 60 fps) */\n")
OLD_STEP_LOAD = STEP_LOAD.replace("uint64_t _v=recomp_load32(c,(c->x[10]+16ULL));",
                                  "uint64_t _v=hos_oot_sound_step((uint32_t)recomp_load32(c,(c->x[10]+16ULL)));")
# (the music manager's step and seconds loads, its four flag reads and its head: OLD_* the text patches' on main)
BGM_LOADS = ("    { { uint64_t _v=recomp_load32(c,(c->x[8]+16ULL)); a64_vset64(c->vreg[8],_v); } }\n"
             "    { { uint64_t _v=recomp_load32(c,(c->x[8]+24ULL)); a64_vset64(c->vreg[9],_v); } }\n")
OLD_BGM_LOADS = ("    { { uint64_t _v=hos_oot_sound_step((uint32_t)recomp_load32(c,(c->x[8]+16ULL))); a64_vset64(c->vreg[8],_v); } }"
                 "   /* (nx2/oot_patches.py: the music manager's whole step above 60 fps) */\n"
                 "    { { uint64_t _v=hos_oot_sound_seconds((uint32_t)recomp_load32(c,(c->x[8]+24ULL)), "
                 "(uint32_t)recomp_load32(c,(c->x[8]+16ULL))); a64_vset64(c->vreg[9],_v); } }\n")
FLAGS_READ = "    c->x[8]=recomp_load8(c,c->x[26] + 2200);\n"
OLD_FLAGS_READ = ("    c->x[8]=hos_oot_bgm_flags(c->x[26] + 2200, recomp_load8(c,c->x[26] + 2200));"
                  "   /* (nx2/oot_patches.py: with the flags of frames the gate held) */\n")
OLD_BGM_HEAD = "    hos_oot_bgm_begin();   /* (nx2/oot_patches.py: the change flags held back since the last run) */\n"
FLAG_SITES = (("music.changeFlags", 0x280), ("music.changeFlags1", 0x3c0), ("music.changeFlags2", 0x550),
              ("music.changeFlags6", 0x64c))


def after(text, line, *sites):
    """text with each site's hook, as the lift lowers it (tests/sitehooks.py), after the next place of line (one site a place, in order)"""
    at = 0
    for x in sites:
        at = text.index(line, at) + len(line)
        hook = sitehooks.line(x) + "\n"
        text, at = text[:at] + hook + text[at:], at + len(hook)
    return text


def sound_sites():
    return site("sound.job", "f_80435300", can_return=True), site("sound.jobStep", "f_80435300", "after", 0x34)


def music_sites():
    return (site("music.update", "f_83373400"), site("music.time", "f_83373400", "after", 0x48),
            [site(name, "f_83373400", "after", offset) for name, offset in FLAG_SITES])


# (the music manager's update as the lift writes its head: the time object's pointer, its step into s8 and its seconds into s9)
LIFTED_BGM = """#include "a64.h"
void f_83373400(Ctx *__restrict A64_NOESC c) {
    c->x[8]=g_module_base + 0x83373000ULL + (int64_t)26390528;
    { uint64_t _r = ((uint64_t)0 | ((c->x[0]) << 0)); c->x[19] = _r; }
    c->x[8]=recomp_load64(c,c->x[8] + 2192);
    c->x[8]=recomp_load64(c,c->x[8] + 0);
    { { uint64_t _v=recomp_load32(c,(c->x[8]+16ULL)); a64_vset64(c->vreg[8],_v); } }
    { { uint64_t _v=recomp_load32(c,(c->x[8]+24ULL)); a64_vset64(c->vreg[9],_v); } }
    return;
}
"""

# (stand-ins for the lift's runtime: the time object at 0x2000 behind the pointer at 0x84c9e890; then the host's hos_oot_sound_*
# functions cut from game/oot_high_fps.c, the patched update, and the frames: the clock the update counts against the real one)
BGM_RUNTIME = r"""
#include <stdint.h>
#include <stdio.h>
#include "oot_high_fps.h"
typedef struct { uint64_t x[32]; uint64_t vreg[32][2]; } Ctx;
#define A64_NOESC
static const uint64_t g_module_base = 0;
static uint32_t step_bits, seconds_bits;
static int is_timed;
static int timed(void) { return is_timed; }
static uint64_t recomp_load64(Ctx *c, uint64_t a) { (void)c; return a == 0x84c9e890ULL ? 0x1000 : a == 0x1000 ? 0x2000 : 0; }
static uint64_t recomp_load32(Ctx *c, uint64_t a) { (void)c; return a == 0x2010 ? step_bits : a == 0x2018 ? seconds_bits : 0; }
static void a64_vset64(uint64_t *v, uint64_t value) { v[0] = value; v[1] = 0; }
"""

BGM_FRAMES = r"""
/* fps frames of the game's step (60 / fps) and seconds (1 / fps): the update's steps and seconds over the real ones */
static void clock(float step, int frames, int timed_now, double *steps, double *seconds) {
    OotSoundGate g = OOT_SOUND_GATE_INIT;
    Ctx c = {0};
    double s = 0, t = 0;
    is_timed = timed_now;
    step_bits = oot_float_bits(step), seconds_bits = oot_float_bits(step / 60.0f);
    for (int i = 0; i < frames; i++)
        if (oot_sound_gate(&g, 0, step, timed_now)) {
            f_83373400(&c);
            s += oot_bits_float((uint32_t)c.vreg[8][0]), t += oot_bits_float((uint32_t)c.vreg[9][0]);
        }
    *steps = s / (step * frames), *seconds = t / (step / 60.0f * frames);
}
int main(void) {
    const float steps[] = {0.5f, 0.25f, 1.0f, 1.5f, 0.5f};
    const int timed_now[] = {1, 1, 1, 1, 0};
    for (int i = 0; i < 5; i++) {
        double s, t;
        clock(steps[i], 2400, timed_now[i], &s, &t);
        printf("%g %d %.4f %.4f\n", steps[i], timed_now[i], s, t);
    }
    return 0;
}
"""


# (the update's four reads of the change flags, as the lift writes them, each followed by a call where the bit's handler starts:
# the stand-in f_837ab1d0 counts the bit by the call's return address)
LIFTED_BGM_FLAGS = """#include "a64.h"
void f_83373400(Ctx *__restrict A64_NOESC c) {
    c->x[25]=g_module_base + 0x83373000ULL + (int64_t)26386432;
    c->x[25]=recomp_load64(c,c->x[25] + 3488);
    c->x[22]=recomp_load64(c,c->x[25] + 0);
    { uint64_t _a=c->x[22], _b=16384ULL; uint64_t _r=_a+_b; c->x[26]=_r; }
    c->x[8]=recomp_load8(c,c->x[26] + 2200);
if (!((c->x[8] >> 5) & 1)) { goto L_83373770; }
c->x[30] = 0x833736b8ULL; f_837ab1d0(c);
L_83373770:;
    c->x[8]=recomp_load8(c,c->x[26] + 2200);
if (!((c->x[8] >> 1) & 1)) { goto L_8337388c; }
c->x[30] = 0x833737f0ULL; f_837ab1d0(c);
L_8337388c:;
    c->x[8]=recomp_load8(c,c->x[26] + 2200);
if (!((c->x[8] >> 2) & 1)) { goto L_83373a4c; }
c->x[30] = 0x83373978ULL; f_837ab1d0(c);
L_83373a4c:;
    c->x[8]=recomp_load8(c,c->x[26] + 2200);
if (!((c->x[8] >> 6) & 1)) { goto L_83373ad4; }
c->x[30] = 0x83373a80ULL; f_837ab1d0(c);
L_83373ad4:;
    return;
}
"""

# (stand-ins for the host file's and the lift's world: guest memory as one array with the time object's and the battle manager's
# slots, a lock that is none, no log)
FLAGS_RUNTIME = r"""
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "oot_high_fps.h"
typedef struct { uint64_t x[32]; uint64_t vreg[32][2]; } Ctx;
#define A64_NOESC
static const uint64_t g_module_base = 0;
static uint8_t mem[0x20000];
static uint8_t *guest(uint64_t a) {
    if (a == 0x84c9e890ULL) return mem + 0x100;   /* (the time object's slot) */
    if (a == 0x84c9dda0ULL) return mem + 0x108;   /* (the battle manager's) */
    return a < sizeof mem ? mem + a : mem;
}
#define GP(a) ((void *)guest(a))
#define G64(a) (*(uint64_t *)GP(a))
typedef int SRWLOCK;
#define SRWLOCK_INIT 0
static void AcquireSRWLockExclusive(SRWLOCK *l) { (void)l; }
static void ReleaseSRWLockExclusive(SRWLOCK *l) { (void)l; }
typedef long long LONG64;
static LONG64 InterlockedIncrement64(volatile LONG64 *p) { return ++*p; }
static void hos_log(const char *f, ...) { (void)f; }
static int env_on(const char *name) { (void)name; return 0; }
static int env_value_off(const char *v) { (void)v; return 0; }
volatile uint32_t hos_frame_seconds_bits;
volatile uint64_t hos_step_probe;   /* (host/nn_vi.c's, which game/oot_high_fps.c's cloth solve reads: no time object here) */
/* (host/hos.h's, for hos_oot_note_link and the takeoff and ledge steps beside the music manager's: their guest memory is
   this mem, and no address here is one of the heap) */
#define HOS_HEAP_BASE 0x1000000000ULL
static void *hos_guest_range(uint64_t a, uint64_t n) { return a + n <= sizeof mem ? mem + a : NULL; }
static void hos_guest_copy(uint64_t a, const void *src, uint64_t n) { memcpy(guest(a), src, n); }
static int64_t hos_now_ns(void) { return 0; }
static uint64_t recomp_load64(Ctx *c, uint64_t a) { (void)c; return G64(a); }
static uint64_t recomp_load32(Ctx *c, uint64_t a) { uint32_t v; (void)c; memcpy(&v, guest(a), 4); return v; }
static uint64_t recomp_load8(Ctx *c, uint64_t a) { (void)c; return *guest(a); }
static void a64_vset64(uint64_t *v, uint64_t value) { v[0] = value; v[1] = 0; }
static int handled[8];
static void f_837ab1d0(Ctx *c) {
    switch (c->x[30]) {
    case 0x833736b8ULL: handled[5]++; break;
    case 0x833737f0ULL: handled[1]++; break;
    case 0x83373978ULL: handled[2]++; break;
    case 0x83373a80ULL: handled[6]++; break;
    }
}
"""

FLAGS_FRAMES = r"""
#define FLAGS_AT (0x10000 + 16384 + 2200)
static Ctx ctx;
/* one game frame of step (60 / fps; no frame time given at a cap of 60) with the flag byte at flags: the job's phases as the game
   calls them, the update on phase 1 when the gate runs it */
static void frame(float step, int timed_now, uint8_t flags) {
    const float seconds = step / 60.0f;
    uint32_t b;
    memcpy(&b, &seconds, 4);
    hos_frame_seconds_bits = timed_now ? b : 0;
    memcpy(mem + 0x2010, &step, 4);
    mem[FLAGS_AT] = flags;
    for (uint32_t phase = 0; phase < 3; phase++)
        if (hos_oot_sound_gate(&ctx, phase, 0x84c9e890ULL) && phase == 1) f_83373400(&ctx);
    mem[FLAGS_AT] = 0;   /* (the battle manager clears them by the next frame) */
}
int main(void) {
    const uint64_t time_at = 0x1000, battle = 0x10000;
    memcpy(mem + 0x100, &time_at, 8);
    const uint64_t time_obj = 0x2000;
    memcpy(mem + 0x1000, &time_obj, 8);
    const uint64_t battle_at = 0x3000;
    memcpy(mem + 0x108, &battle_at, 8);
    memcpy(mem + 0x3000, &battle, 8);
    const float steps[] = {0.5f, 0.25f, 1.0f};
    const int timed_now[] = {1, 1, 0};
    const int bits[] = {5, 1, 6};
    for (int i = 0; i < 3; i++)
        for (int w = 0; w < 8; w++)
            for (int k = 0; k < 3; k++) {
                for (int f = 0; f < 4; f++) frame(steps[i], timed_now[i], 0);   /* (settle: what an earlier window left is handled) */
                memset(handled, 0, sizeof handled);
                for (int f = 0; f < w; f++) frame(steps[i], timed_now[i], 0);
                frame(steps[i], timed_now[i], (uint8_t)(1u << bits[k]));
                for (int f = 0; f < 8; f++) frame(steps[i], timed_now[i], 0);
                printf("%g %d %d %d %d\n", steps[i], timed_now[i], w, bits[k], handled[bits[k]]);
            }
    return 0;
}
"""


def host_file_for_test(source):
    """game/oot_high_fps.c without its includes but the logic's header, for FLAGS_RUNTIME's stand-ins"""
    keep = ('#include "oot_high_fps.h"',)
    return "\n".join(line for line in source.split("\n") if not line.startswith("#include") or line in keep)


def hos_sound_functions(source):
    """game/oot_high_fps.c's hos_oot_sound_* functions that return the float's bits (the step, and the seconds when there)"""
    names = sorted(set(m.group(1) for m in re.finditer(r"^uint32_t (hos_oot_sound_\w+)\(", source, re.M)))
    return "\n".join(block(source, "uint32_t %s(" % name) for name in names)


# (the solver's cut and a store, made up in the lift's form: the cut of s0 into w1, the register the callback reads; OLD_SOLVER
# the text patch's line between them on main)
CUT = "    c->x[1]=recomp_fp_to_int(c->vreg[0][0]&UINT64_C(0xffffffff),32,32,1,3,c->fpcr,&c->fpsr);\n"
STORE = "    recomp_store32(c,c->x[19] + 64,c->x[1]);\n"
OLD_SOLVER = ("    c->x[1]=hos_oot_solver_steps((uint32_t)c->x[1], (uint32_t)c->vreg[0][0], (uint32_t)c->vreg[1][0]);"
              "   /* (nx2/oot_patches.py: solver steps rounded up above 60 fps) */\n")
SOLVER_SIDE = r"""
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#define A64_NOESC
typedef struct { uint64_t x[32]; uint64_t vreg[32][2]; uint64_t fpcr, fpsr; } Ctx;
static uint64_t stored;
static uint64_t recomp_fp_to_int(uint64_t v, int a, int b, int s, int r, uint64_t fpcr, uint64_t *fpsr) {
    (void)a, (void)b, (void)s, (void)r, (void)fpcr, (void)fpsr;
    float f; uint32_t u = (uint32_t)v; memcpy(&f, &u, 4); return (uint64_t)(uint32_t)(int32_t)f;
}
static void recomp_store32(Ctx *c, uint64_t a, uint64_t v) { (void)c, (void)a; stored = (uint32_t)v; }
/* (a stand-in for the host's rule: any function of the three, so a register mixed up shows) */
uint32_t hos_oot_solver_steps(uint32_t game, uint32_t scaled_bits, uint32_t per_frame_bits) {
    return game * 7u + (scaled_bits >> 3) + per_frame_bits * 3u;
}
@CALLBACK@
@SLOT@static void hooked(Ctx *c) {
@HOOKED@}
static void patched(Ctx *c) {
@PATCHED@}
int main(void) {
    static const float scaled[] = {0.0f, 0.5f, 3.99f, 4.001f, 8.001f, -1.0f};
    static const float per_frame[] = {1.0f, 4.0f, 8.0f};
    int fails = 0;
    for (unsigned i = 0; i < 6; i++)
        for (unsigned k = 0; k < 3; k++) {
            Ctx a, b;
            memset(&a, 0, sizeof a);
            for (int r = 0; r < 32; r++) a.x[r] = 0x9e3779b97f4a7c15ull * (r + 1);
            memcpy(&a.vreg[0][0], &scaled[i], 4), memcpy(&a.vreg[1][0], &per_frame[k], 4);
            b = a;
            hooked(&a);
            const uint64_t sa = stored;
            patched(&b);
            if (memcmp(&a, &b, sizeof a) || sa != stored) { printf("FAIL %u %u\n", i, k); fails++; }
        }
    if (!fails) printf("same\n");
    return fails != 0;
}
"""


# (stand-ins for the side-by-side check: guest memory a few words, and the host's functions logging their calls and answering from
# their arguments, so a call left out, made twice or given another register shows)
SIDE_RUNTIME = r"""
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#define A64_NOESC
typedef struct { uint64_t x[32]; uint64_t vreg[32][2]; } Ctx;
static const uint64_t g_module_base = 0;
static uint64_t log_[64];
static int logged;
static void note(uint64_t a, uint64_t b, uint64_t c) { if (logged < 60) log_[logged++] = a, log_[logged++] = b, log_[logged++] = c; }
static uint32_t mem32[16];
static uint8_t mem8;
static uint64_t recomp_load64(Ctx *c, uint64_t a) { (void)c; return a == 0x84c9e890ULL ? 0x1000 : a == 0x1000 ? 0x2000 : 0; }
static uint64_t recomp_load32(Ctx *c, uint64_t a) { (void)c; return a >= 0x2000 && a < 0x2040 ? mem32[(a - 0x2000) / 4] : 0x77; }
static uint64_t recomp_load8(Ctx *c, uint64_t a) { (void)c; return a == 0x5000 + 2200 ? mem8 : 0x55; }
static void a64_vset64(uint64_t *v, uint64_t value) { v[0] = value; v[1] = 0; }
static int gate_answer;
int hos_oot_sound_gate(Ctx *c, uint32_t phase, uint64_t slot) { (void)c; note(1, phase, slot); return gate_answer; }
uint32_t hos_oot_sound_step(uint32_t b) { note(2, b, 0); return b * 3u + 1u; }
uint32_t hos_oot_sound_seconds(uint32_t s, uint32_t b) { note(3, s, b); return s ^ (b << 1); }
void hos_oot_bgm_begin(void) { note(4, 0, 0); }
uint64_t hos_oot_bgm_flags(uint64_t at, uint64_t flags) { note(5, at, flags); return flags | 0x80; }
static void sound_engine(Ctx *c) { note(6, c->vreg[0][0], c->vreg[0][1]); }
static void seen(Ctx *c) { note(7, c->x[8], 0); }
"""
SIDE_MAIN = r"""
static int same(void (*a)(Ctx *), void (*b)(Ctx *), const Ctx *start) {
    Ctx x = *start, y = *start;
    uint64_t la[64];
    int na;
    logged = 0;
    a(&x);
    memcpy(la, log_, sizeof la), na = logged;
    logged = 0;
    b(&y);
    return na == logged && !memcmp(la, log_, sizeof la) && !memcmp(&x, &y, sizeof x);
}
int main(void) {
    int fails = 0;
    for (int k = 0; k < 16; k++) mem32[k] = 0x3f000000u + (uint32_t)k * 0x1111u;
    for (uint32_t phase = 0; phase < 3; phase++)
        for (gate_answer = 0; gate_answer < 2; gate_answer++) {
            Ctx c;
            memset(&c, 0, sizeof c);
            for (int r = 0; r < 31; r++) c.x[r] = 0x9e3779b97f4a7c15ull * (r + 1);
            c.x[2] = phase, c.vreg[0][1] = 0xdead;
            if (!same(f_80435300, old_80435300, &c)) { printf("FAIL: the sound job, phase %u, gate %d\n", phase, gate_answer); fails++; }
        }
    for (mem8 = 0; mem8 < 4; mem8++) {
        Ctx c;
        memset(&c, 0, sizeof c);
        c.x[8] = 0x2000, c.x[26] = 0x5000;
        if (!same(f_83373400, old_83373400, &c)) { printf("FAIL: the music manager, flags %u\n", mem8); fails++; }
    }
    if (!fails) printf("same\n");
    return fails != 0;
}
"""


class HighFpsTests(unittest.TestCase):
    def test_gate_and_steps(self):
        compiler = CC
        with tempfile.TemporaryDirectory(prefix="longshot-high-fps-") as tmp:
            tmp = Path(tmp)
            (tmp / "t.c").write_text(FIXTURE, encoding="utf-8")
            binary = tmp / "t.exe"
            build = subprocess.run([compiler, "-std=gnu11", "-Wall", "-O1", "-I", str(ROOT / "host"), "-I", str(ROOT / "game"), str(tmp / "t.c"),
                                    "-o", str(binary), "-lm"], text=True, capture_output=True)
            self.assertEqual(build.returncode, 0, build.stderr)
            run = subprocess.run([str(binary)], text=True, capture_output=True, timeout=30)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)

    def test_as_the_text_patches(self):
        """the job and the update with the emitted hooks, and with the patches' lines, over stand-ins that log each call to the
        host and answer from what they were given: the same registers and the same calls"""
        compiler = CC
        gate, step = sound_sites()
        update, time, flags = music_sites()
        job_new = sitehooks.splice_entry(after(SOUND_HEAD + SOUND_BODY, STEP_LOAD, step), 0x80435300, sitehooks.line(gate))
        job_old = (SOUND_HEAD + OLD_GATE + SOUND_BODY.replace(STEP_LOAD, OLD_STEP_LOAD)).replace("f_80435300", "old_80435300")
        bgm = "void f_83373400(Ctx *__restrict A64_NOESC c) {\n" + BGM_LOADS + (FLAGS_READ + "    seen(c);\n") * 4 + "}\n"
        bgm_new = sitehooks.splice_entry(after(after(bgm, BGM_LOADS.split("\n")[1] + "\n", time), FLAGS_READ, *flags),
                                         0x83373400, sitehooks.line(update))
        bgm_old = bgm.replace(BGM_LOADS, OLD_BGM_LOADS).replace(FLAGS_READ, OLD_FLAGS_READ).replace(
            "c) {\n", "c) {\n" + OLD_BGM_HEAD, 1).replace("f_83373400", "old_83373400")
        names = ("int hos_site_sound_job(", "int hos_site_sound_job_step(", "int hos_site_music_update(",
                 "int hos_site_music_time(", "int hos_site_music_change_flags(")
        program = (SIDE_RUNTIME + "\n".join(callback(n) for n in names) + "\n"
                   + "".join(slot(x) for x in (gate, step, update, time) + tuple(flags))
                   + job_new + job_old + bgm_new + bgm_old + SIDE_MAIN)
        with tempfile.TemporaryDirectory(prefix="longshot-high-fps-") as tmp:
            tmp = Path(tmp)
            (tmp / "side.c").write_text(program, encoding="utf-8")
            binary = tmp / "side.exe"
            build = subprocess.run([compiler, "-std=gnu11", "-Wall", "-Wextra", "-Werror", "-O1", str(tmp / "side.c"),
                                    *sitehooks.objects(tmp, program), "-o", str(binary)], text=True, capture_output=True)
            self.assertEqual(build.returncode, 0, build.stderr)
            run = subprocess.run([str(binary)], text=True, capture_output=True, timeout=30)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)

    def test_music_manager_clock(self):
        compiler = CC
        time = music_sites()[1]
        hooked = after(LIFTED_BGM, BGM_LOADS.split("\n")[1] + "\n", time)
        host = (ROOT / "game" / "oot_high_fps.c").read_text(encoding="utf-8")
        program = (BGM_RUNTIME + hos_sound_functions(host) + "\n" + callback("int hos_site_music_time(") + "\n" + slot(time)
                   + hooked.replace('#include "a64.h"\n', "") + BGM_FRAMES)
        with tempfile.TemporaryDirectory(prefix="longshot-high-fps-") as tmp:
            tmp = Path(tmp)
            (tmp / "bgm.c").write_text(program, encoding="utf-8")
            binary = tmp / "bgm.exe"
            build = subprocess.run([compiler, "-std=gnu11", "-Wall", "-O1", "-I", str(ROOT / "host"), "-I", str(ROOT / "game"), str(tmp / "bgm.c"),
                                    *sitehooks.objects(tmp, program), "-o", str(binary), "-lm"], text=True, capture_output=True)
            self.assertEqual(build.returncode, 0, build.stderr)
            run = subprocess.run([str(binary)], text=True, capture_output=True, timeout=30)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        lines = [line for line in run.stdout.split("\n") if line]
        self.assertEqual(len(lines), 5, run.stdout)
        for line in lines:
            step, timed, steps, seconds = line.split()
            what = "music manager clock at a step of %s%s" % (step, "" if timed == "1" else ", a cap of 60")
            # (each ratio: what the update counted over the real time; a step of 0.5 at a cap of 60 runs every frame with it)
            self.assertAlmostEqual(float(steps), 1.0, delta=0.01, msg="%s: %s of the real steps" % (what, steps))
            self.assertAlmostEqual(float(seconds), 1.0, delta=0.01, msg="%s: %s of the real seconds" % (what, seconds))

    def test_music_manager_flags(self):
        compiler = CC
        update, _, flags = music_sites()
        hooked = sitehooks.splice_entry(after(LIFTED_BGM_FLAGS, FLAGS_READ, *flags), 0x83373400, sitehooks.line(update))
        host = (ROOT / "game" / "oot_high_fps.c").read_text(encoding="utf-8")
        program = (FLAGS_RUNTIME + host_file_for_test(host) + "\n" + callback("int hos_site_music_update(") + "\n"
                   + callback("int hos_site_music_change_flags(") + "\n" + slot(update) + "".join(slot(x) for x in flags)
                   + hooked.replace('#include "a64.h"\n', "") + FLAGS_FRAMES)
        with tempfile.TemporaryDirectory(prefix="longshot-high-fps-") as tmp:
            tmp = Path(tmp)
            (tmp / "flags.c").write_text(program, encoding="utf-8")
            binary = tmp / "flags.exe"
            build = subprocess.run([compiler, "-std=gnu11", "-Wall", "-Wno-unused-function", "-O1", "-I", str(ROOT / "host"), "-I", str(ROOT / "game"),
                                    str(tmp / "flags.c"), *sitehooks.objects(tmp, program), "-o", str(binary), "-lm"], text=True, capture_output=True)
            self.assertEqual(build.returncode, 0, build.stderr)
            run = subprocess.run([str(binary)], text=True, capture_output=True, timeout=30)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        lines = [line for line in run.stdout.split("\n") if line]
        self.assertEqual(len(lines), 72, run.stdout)
        lost = []
        for line in lines:
            step, timed, frame, bit, handled = line.split()
            if handled != "1":   # (0: the change was lost; 2: handled twice, a count in the game run down twice)
                lost.append("bit %s set on frame %s at a step of %s%s: handled %s times" % (
                    bit, frame, step, "" if timed == "1" else " (a cap of 60)", handled))
        self.assertEqual(lost, [], "%d of %d one-frame flags not handled once:\n%s" % (len(lost), len(lines), "\n".join(lost)))

    def test_solver_sites(self):
        sites = [site("physics.solverSteps", "f_838ceb80", "after", 0x74),
                 site("physics.solverStepsOther", "f_838cfc10", "after", 0x110)]
        compiler = CC
        for x in sites:   # (each site's own flag and slot, holding the one callback)
            self.assertEqual(engine_fn(x), "hos_site_solver_steps", "%s's game/sites.def line calls another callback"
                             % x.name)
            line = sitehooks.line(x)
            code = (SOLVER_SIDE.replace("@CALLBACK@", callback("int hos_site_solver_steps(")).replace("@SLOT@", slot(x))
                    .replace("@HOOKED@", CUT + line + "\n" + STORE).replace("@PATCHED@", CUT + OLD_SOLVER + STORE))
            with tempfile.TemporaryDirectory(prefix="longshot-solver-") as tmp:
                tmp = Path(tmp)
                (tmp / "s.c").write_text(code, encoding="utf-8")
                build = subprocess.run([compiler, "-std=gnu11", "-Wall", "-Wextra", "-Werror", str(tmp / "s.c"),
                                        *sitehooks.objects(tmp, code), "-o", str(tmp / "s.exe")], text=True, capture_output=True)
                self.assertEqual(build.returncode, 0, build.stderr)
                run = subprocess.run([str(tmp / "s.exe")], text=True, capture_output=True, timeout=30)
                self.assertEqual(run.returncode, 0, x.name + ": " + run.stdout + run.stderr)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--cc", default="clang")
    ap.add_argument("--cargo")
    args, rest = ap.parse_known_args()
    CC = args.cc
    sitehooks.use(args.cc, args.cargo)
    raise SystemExit(unittest.main(argv=[sys.argv[0]] + rest, verbosity=1))
