"""The eight-note Scarecrow melody keeps its recorded rhythm across frame-rate caps.

Exercise the real whole-frame hook sites (frame.wholeScarecrowPlay, frame.wholeScarecrowRecord and
frame.wholeScarecrowLast, replace sites on the cuts, as the lift lowers them (tests/sitehooks.py) with game/oot_sites.c's
callbacks) on small authored C functions with the two conversion shapes, then compile them with the
real fraction helper; and the same with the text patch's lines they replaced, side by side. Record note
holds/gaps at 60, 120 and 240 updates per second and play every recording at each
rate. The recording counter resets between events; its fraction belongs to the
counter, not one update or function. The separate x12 recording cut stays unchanged.
No game data, save, audio or GPU is needed.

Transition check: main's text-patch C kept here as the oracle shows the sites do what it did; remove it once a
deliberate behaviour change needs it.

python -I -B tests/test_scarecrow_timing.py [--cc clang] [--cargo cargo]
"""
import argparse
import subprocess
import sys
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves this folder off the path)
from whole_sites import hook_whole, old_whole  # noqa: E402
import sitehooks  # noqa: E402

CUT = "    c->x[8]=recomp_fp_to_int(c->vreg[0][0]&UINT64_C(0xffffffff),32,32,0,3,c->fpcr,&c->fpsr);\n"
ADD = "    { uint64_t _a=c->x[%d],_b=((uint64_t)(uint32_t)(((uint32_t)(c->x[8])) << 0)),_r=_a+_b; _r&=0xFFFFFFFFULL; c->x[8]=_r; }\n"
LIFTED = ('#define _CRT_SECURE_NO_WARNINGS\n#include "a64.h"\n'
          'void f_832b6840(Ctx *__restrict A64_NOESC c) {\n'
          '    if (c->x[2]) {\n' + CUT + ADD % 12 +
          '    } else {\n' + CUT + ADD % 10 +
          '    }\n    c->counter = (uint32_t)c->x[8];\n}\n'
          'void f_832b32b0(Ctx *__restrict A64_NOESC c) {\n' + CUT +
          '    c->counter += (uint32_t)c->x[8];\n}\n'
          'void f_832b2690(Ctx *__restrict A64_NOESC c) {\n' + CUT +
          '    c->counter += (uint32_t)c->x[8];\n}\n')

HEADER = r'''#pragma once
#include <stdint.h>
#define A64_NOESC
/* Only the registers the authored conversion shapes use, not the guest ABI. */
typedef struct {
    uint64_t x[32], vreg[32][2];
    uint32_t fpcr, fpsr, counter;
} Ctx;
uint64_t recomp_fp_to_int(uint64_t bits, int from, int to, int sign, int rounding,
                         uint32_t fpcr, uint32_t *fpsr);
'''

FIXTURE = r'''
#include "frame_whole.h"
#include <stdio.h>
#include <string.h>
static FrameWhole sc_fractions;
static int sc_carry, sc_failures;
static unsigned sc_calls;
static const uint64_t sc_object = 0x100000;
static const uint64_t sc_counter = 0x1265fc;
#define CHECK(condition) do { if (!(condition)) { \
    printf("FAIL line %d: %s\n", __LINE__, #condition); ++sc_failures; \
} } while (0)

/* Exact for this fixture's finite, nonnegative frame steps. */
uint64_t recomp_fp_to_int(uint64_t bits, int from, int to, int sign, int rounding,
                         uint32_t fpcr, uint32_t *fpsr) {
    (void)from; (void)to; (void)sign; (void)rounding; (void)fpcr; (void)fpsr;
    uint32_t word = (uint32_t)bits;
    float step;
    memcpy(&step, &word, sizeof step);
    return (uint32_t)step;
}
uint32_t hos_frame_whole(uint64_t at, uint32_t bits) {
    CHECK(at == sc_counter);
    ++sc_calls;
    return frame_whole(&sc_fractions, at, bits, sc_carry);
}
static void sc_tick(Ctx *c, int rate, int recording) {
    float step = 60.0f / rate;
    uint32_t bits;
    memcpy(&bits, &step, sizeof bits);
    c->vreg[0][0] = bits;
    c->x[0] = c->x[19] = sc_object;
    c->x[9] = sc_counter;
    c->x[10] = c->counter;
    sc_carry = rate > 60;
    if (recording == 2) f_832b2690(c);
    else if (recording) f_832b6840(c);
    else f_832b32b0(c);
}
int main(void) {
    const int rates[] = {60, 120, 240};
    for (unsigned r = 0; r < sizeof rates / sizeof rates[0]; ++r) {
        for (unsigned p = 0; p < sizeof rates / sizeof rates[0]; ++p) {
            memset(&sc_fractions, 0, sizeof sc_fractions);
            Ctx c = {0};
            uint32_t melody_ticks = 0;
            const int before = sc_failures;
            /* Eight 0.1-second holds and seven 0.4-second gaps; completion times the eighth hold. */
            for (int event = 0; event < 15; ++event) {
                const int expected = event % 2 ? 24 : 6;
                c.counter = 0;
                for (int f = 0; f < expected * rates[r] / 60; ++f)
                    sc_tick(&c, rates[r], event == 14 ? 2 : 1);
                CHECK(c.counter == (uint32_t)expected);
                melody_ticks += c.counter;
            }
            CHECK(melody_ticks == 216);
            c.counter = 0;
            int frames = 0;
            while (c.counter < melody_ticks && frames < rates[p] * 10) {
                sc_tick(&c, rates[p], 0);
                ++frames;
            }
            CHECK(c.counter == 216);
            CHECK(frames == 216 * rates[p] / 60);
            printf("record %d, play %d: %s (stored %u ticks, playback %d updates)\n",
                   rates[r], rates[p], before == sc_failures ? "PASS" : "FAIL", melody_ticks, frames);
        }
    }
    /* A sub-tick remainder survives a counter reset and is shared by all three update sites. */
    memset(&sc_fractions, 0, sizeof sc_fractions);
    Ctx split = {0};
    sc_tick(&split, 240, 1);
    CHECK(split.counter == 0);
    split.counter = 0;
    sc_tick(&split, 240, 2);
    CHECK(split.counter == 0);
    sc_tick(&split, 240, 0);
    CHECK(split.counter == 0);
    sc_tick(&split, 240, 2);
    CHECK(split.counter == 1);
    /* The other recording branch's cut is outside the eight-note change. */
    Ctx other = {0};
    const unsigned calls = sc_calls;
    other.x[2] = 1;
    other.x[12] = 7;
    sc_tick(&other, 240, 1);
    CHECK(other.counter == 7);
    CHECK(sc_calls == calls);
    if (!sc_failures) puts("PASS: short-song recording and playback share a fractional clock");
    return sc_failures != 0;
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cc", default="clang")
    parser.add_argument("--cargo")
    args = parser.parse_args()
    sitehooks.use(args.cc, args.cargo)
    source, changed = hook_whole(LIFTED)
    old, _ = old_whole(LIFTED)
    with tempfile.TemporaryDirectory(prefix="longshot-scarecrow-") as folder:
        tmp = Path(folder)
        (tmp / "a64.h").write_text(HEADER, encoding="utf-8")
        for what, text in (("the hooks", source), ("the text patch", old)):
            fixture = tmp / "fixture.c"
            fixture.write_text(text + FIXTURE, encoding="utf-8")
            binary = tmp / "fixture.exe"
            command = [args.cc, "-std=c11", "-Wall", "-Wextra", "-Werror", "-Wno-unused-function", "-O1",
                       "-I", str(ROOT / "host"), "-I", str(ROOT / "game"), str(fixture), *sitehooks.objects(tmp, text), "-o", str(binary)]
            print("compiler (%s):" % what, " ".join(command), flush=True)
            subprocess.run(command, check=True)
            subprocess.run([str(binary)], check=True, timeout=30)
    assert changed == 3, "all three eight-note conversions must each be hooked once"
    assert source.count(CUT + ADD % 12) == 1, "the other recording cut must be preserved"


if __name__ == "__main__":
    main()
