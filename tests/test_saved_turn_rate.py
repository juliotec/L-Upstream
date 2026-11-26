"""The saved steering rate in f_8263e8e0 does not decay on short steps (#312).

The two bounds' multiplies are the replace sites steer.rateLower and steer.rateUpper
(game/sites/sites.toml, game/oot_sites.c). An authored update with the two
multiplies is built with the sites' hooks as the lift lowers them (tests/sitehooks.py) and, side by
side, with the text patch's arithmetic they replaced (OLD_ANCHOR, as main had it),
then compiled and its saved rate checked. Both signs, rate-limited and
target-limited turns, and the unchanged step >= 1 path are covered. No game files
or lifted function body are used. The floating-point stand-ins check ordinary
finite arithmetic and helper selection, not guest exception flags (the hooks call
the runtime's exact helpers, recomp_fpe_*, the patch the lift's fast ones, which
fall back to those: here both are the same stand-ins).

Transition check: main's text-patch C kept here as the oracle shows the sites do what it did; remove it once a
deliberate behaviour change needs it.

python -I -B tests/test_saved_turn_rate.py [--cc clang] [--cargo cargo]
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

ANCHOR = """    {uint64_t _a=0,_b=0,_z,_v;memcpy(&_a,c->vreg[0],4);memcpy(&_b,c->vreg[1],4);_z=(_a^_b)&0x80000000ULL;
    _v=a64_fpx_mul32_fma32(c,_a,_b,_z);
    a64_vset64(c->vreg[1],_v);}
    {uint64_t _a=0,_b=0,_z,_v;memcpy(&_a,c->vreg[0],4);memcpy(&_b,c->vreg[8],4);_z=(_a^_b)&0x80000000ULL;
    _v=a64_fpx_mul32_fma32(c,_a,_b,_z);
    a64_vset64(c->vreg[0],_v);}
"""
# (the text patch's arithmetic on main)
OLD_ANCHOR = ANCHOR.replace(
    "    _v=a64_fpx_mul32_fma32(c,_a,_b,_z);",
    "    if ((uint32_t)c->vreg[8][0] > 0 && (uint32_t)c->vreg[8][0] < 0x3f800000u)\n"
    "        _v=a64_fpx_div32_div32(c,_a,_b);\n"
    "    else _v=a64_fpx_mul32_fma32(c,_a,_b,_z);")
FMULS = ANCHOR.split("    {uint64_t")[1:]   # (each multiply's lift text: hooked one by one)
HEAD = "void f_8263e8e0(Ctx *__restrict A64_NOESC c) {\n"
UPDATE = HEAD + ANCHOR + """    const float rate = as_float(c->vreg[2][0]);
    const float lower = as_float(c->vreg[1][0]);
    const float upper = as_float(c->vreg[0][0]);
    c->saved = rate < lower ? lower : (rate > upper ? upper : rate);
}
"""
UNRELATED = UPDATE.replace("f_8263e8e0", "f_90000000")
LIFTED = UPDATE + UNRELATED
FIXTURE = r'''#include <stdint.h>
#include <string.h>
#include <math.h>
#include <stdio.h>
#define A64_NOESC
typedef struct { uint64_t vreg[32][2]; float saved; int divides, multiplies; } Ctx;
static float as_float(uint64_t bits) {
    float f; uint32_t b = (uint32_t)bits; memcpy(&f, &b, 4); return f;
}
static uint64_t as_bits(float f) { uint32_t b; memcpy(&b, &f, 4); return b; }
static void a64_vset64(uint64_t *v, uint64_t bits) { v[0] = bits; v[1] = 0; }
static uint64_t a64_fpx_mul32_fma32(Ctx *c, uint64_t a, uint64_t b, uint64_t z) {
    (void)z; c->multiplies++; return as_bits(as_float(a) * as_float(b));
}
uint64_t a64_fpx_div32_div32(Ctx *c, uint64_t a, uint64_t b) {
    c->divides++; return as_bits(as_float(a) / as_float(b));
}
/* (the runtime's exact helpers the hooks call: the same stand-ins) */
uint64_t recomp_fpe_div32(Ctx *c, uint64_t a, uint64_t b) { return a64_fpx_div32_div32(c, a, b); }
uint64_t recomp_fpe_fma32(Ctx *c, uint64_t a, uint64_t b, uint64_t z) { return a64_fpx_mul32_fma32(c, a, b, z); }
@UPDATE@
static int failures, checks;
static void check(float step, float rate, float applied, float expected, int corrected) {
    Ctx c = {0};
    c.vreg[0][0] = as_bits(fabsf(applied)); c.vreg[1][0] = as_bits(-step);
    c.vreg[8][0] = as_bits(step); c.vreg[2][0] = as_bits(rate);
    f_8263e8e0(&c);
    int same = fabsf(c.saved - expected) <= 1e-7f;
    if (!corrected) same = as_bits(c.saved) == as_bits(expected);
    int ok = same && c.divides == (corrected ? 2 : 0) && c.multiplies == (corrected ? 0 : 2);
    checks++;
    if (!ok) {
        printf("FAIL: saved rate step=%.9g rate=%.9g applied=%.9g saved=%.9g expected=%.9g div=%d mul=%d\n",
               step, rate, applied, c.saved, expected, c.divides, c.multiplies);
        failures++;
    }
}
static float old_saved(float step, float rate, float applied) {
    const float lower = fabsf(applied) * -step, upper = fabsf(applied) * step;
    return rate < lower ? lower : (rate > upper ? upper : rate);
}
int main(void) {
    const float steps[] = {0.25f, 0.42f, 0.5f, 0.66f, 1.0f, 2.0f};
    for (int k = 0; k < 6; k++) for (int sign = -1; sign <= 1; sign += 2) {
        const float step = steps[k], rate = sign * 0.02f;
        for (int target = 0; target < 2; target++) {
            const float applied = target ? sign * 0.002f : rate * step;
            const float expected = step < 1 ? (target ? sign * 0.002f / step : rate)
                                            : old_saved(step, rate, applied);
            check(step, rate, applied, expected, step < 1);
        }
        check(step, rate, 0.0f, step < 1 ? 0.0f : old_saved(step, rate, 0.0f), step < 1);
    }
    check(0.5f, 0.02f, -0.002f, 0.004f, 1);
    check(0.5f, -0.02f, 0.002f, -0.004f, 1);
    check(0.5f, 0.002f, 0.002f, 0.002f, 1);
    check(0.5f, -0.002f, -0.002f, -0.002f, 1);
    const uint32_t unchanged[] = {0, 0x80000000u, 0xbf000000u, 0xc0000000u,
                                 0x7f800000u, 0xff800000u, 0x7fc00000u, 0xffc00000u,
                                 0x3f800000u, 0x3f800001u, 0x40000000u};
    for (unsigned k = 0; k < sizeof unchanged / sizeof unchanged[0]; k++)
        for (int sign = -1; sign <= 1; sign += 2) {
            float step = as_float(unchanged[k]), rate = sign * 0.02f, applied = sign * 0.002f;
            check(step, rate, applied, old_saved(step, rate, applied), 0);
        }
    /* Carry each update's result into the next; the retained rate must not decay. */
    Ctx repeated = {0};
    repeated.saved = -0.02f;
    for (int k = 0; k < 100; k++) {
        repeated.vreg[0][0] = as_bits(fabsf(repeated.saved * 0.42f));
        repeated.vreg[1][0] = as_bits(-0.42f); repeated.vreg[8][0] = as_bits(0.42f);
        repeated.vreg[2][0] = as_bits(repeated.saved);
        f_8263e8e0(&repeated);
    }
    if (fabsf(repeated.saved + 0.02f) > 1e-7f) {
        puts("FAIL: saved rate decays over repeated short steps"); failures++;
    }
    Ctx other = {0};
    other.vreg[0][0] = as_bits(0.01f); other.vreg[1][0] = as_bits(-0.5f);
    other.vreg[8][0] = as_bits(0.5f); other.vreg[2][0] = as_bits(0.02f);
    f_90000000(&other);
    if (as_bits(other.saved) != as_bits(0.005f) || other.divides || other.multiplies != 2) {
        puts("FAIL: unrelated function changed"); failures++;
    }
    if (!failures) printf("PASS: saved turn rate (%d arithmetic checks and unrelated function)\n", checks);
    return failures != 0;
}
'''


def hooked(on=1):
    """LIFTED with the two multiplies as the replace sites' hooks around them, the callbacks and slots ahead"""
    lower = site("steer.rateLower", "f_8263e8e0", "replace", 0x238)
    upper = site("steer.rateUpper", "f_8263e8e0", "replace", 0x23c)
    new = "".join(sitehooks.line(x) + "\n    {uint64_t" + text + sitehooks.REPLACE_END + "\n" for x, text in zip((lower, upper), FMULS))
    calls = (callback("static void turn_bound(") + "\n" + callback("int hos_site_turn_rate_lower(") + "\n"
             + callback("int hos_site_turn_rate_upper(") + "\n" + slot(lower, on) + slot(upper, on))
    return calls + LIFTED.replace(HEAD + ANCHOR, HEAD + new, 1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cc", default="clang")
    parser.add_argument("--cargo")
    args = parser.parse_args()
    sitehooks.use(args.cc, args.cargo)
    with tempfile.TemporaryDirectory(prefix="saved-turn-rate-") as directory:
        source, binary = Path(directory) / "fixture.c", Path(directory) / "fixture.exe"

        def build(update):
            text = FIXTURE.replace("@UPDATE@", update)
            source.write_text(text, encoding="utf-8")
            subprocess.run([args.cc, "-std=c11", "-Wall", "-Wextra", "-Werror", "-Wno-unused-function", str(source),
                            *sitehooks.objects(directory, text), "-o", str(binary)], check=True)
        for update in (hooked(), LIFTED.replace(HEAD + ANCHOR, HEAD + OLD_ANCHOR, 1)):   # (the hooks; the text patch)
            build(update)
            subprocess.run([str(binary)], check=True, timeout=30)
        for what, update in (("the original arithmetic", LIFTED), ("the hooks off", hooked(0))):
            build(update)
            native = subprocess.run([str(binary)], text=True, capture_output=True, timeout=30)
            if native.returncode == 0 or "FAIL: saved rate step=0.25" not in native.stdout:
                raise SystemExit("FAIL: %s did not fail the short-step check" % what)
        print("PASS: the original arithmetic, and the hooks off, compile and fail the same short-step checks")


if __name__ == "__main__":
    main()
