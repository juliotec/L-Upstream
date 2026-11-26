"""The fixed path's scale reaches the 3D through two hook sites of f_831f3200 (game/sites/sites.toml, game/oot_sites.c),
as through the text patch they replaced (nx2/oot_patches.py fixed_scale, deleted).

f_831f3200 recomputes the 3D's scale (+0xe5c = scale * +0xe58 / +0xe50) only while +0xe2c bit 3 is set; on the fixed path
(hos_fixed_resolution) render.fixedScaleApply, before the update of +0xe2c (on every way into it, as the patch's text after the
label L_831f3efc), also asks for it (w8 |= 8) while the 3D's scale is not the chosen one; render.fixedScaleFlags, after the load
of the override flags +0xe20, gives the host the object (hos_res_module) and on the fixed path clears the flags' bits 3/4.

Checked here, game-free: the sites are where the patch was, and the block between them, built with the emitted hooks and with
the patch's lines side by side, leaves the same registers, the same guest memory and the same hos_res_module for the fixed path
on and off, scales that are and are not the chosen one, a 0 at +0xe50, and every flag.

Transition check: main's text-patch C kept here as the oracle shows the sites do what it did; remove it once a
deliberate behaviour change needs it.

python -I -B tests/test_fixed_scale_sites.py [--cc gcc]
"""
import argparse
import subprocess
import sys
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves this folder off the path)
from sitecheck import site, callback, slot  # noqa: E402
import sitehooks  # noqa: E402

# (a made-up block in the lift's form, from the label of the update of +0xe2c to the test of the flags: w8 ORed into +0xe2c
# through a scratch register, a line of filler, then the flags' load into x8, the registers the callbacks read; APPLY and FLAGS
# the text patch's lines on main, after the label and after the flags' load)
LOAD_E2C = "    c->x[11]=recomp_load32(c,c->x[20] + 3628);\n"
REST_E2C = ("    { uint64_t _r = (c->x[11] | c->x[8]); _r &= 0xFFFFFFFFULL; c->x[8] = _r; }\n"
            "    recomp_store32(c,c->x[20] + 3628,c->x[8]);\n"
            "    c->x[12] = 0x3ULL;\n")
LOAD_E20 = "    c->x[8]=recomp_load64(c,c->x[20] + 3616);\n"
APPLY = ("    if (hos_fixed_resolution) {   /* (only while +0xe5c is not the chosen scale: a recompute every frame remade the targets) */\n"
         "        union { uint32_t u; float f; } now = {(uint32_t)recomp_load32(c,c->x[20] + 3676)}, want = {hos_fixed_scale_bits},\n"
         "            e50 = {(uint32_t)recomp_load32(c,c->x[20] + 3664)}, e58 = {(uint32_t)recomp_load32(c,c->x[20] + 3672)};\n"
         "        const float d = now.f - (e50.f != 0 ? want.f * e58.f / e50.f : want.f);\n"
         "        if (d > 1e-4f || d < -1e-4f) c->x[8] |= 8;\n"
         "    }\n")
FLAGS = "    hos_res_module = c->x[20];\n    if (hos_fixed_resolution) c->x[8] &= ~0x18ULL;\n"

FIXTURE = r"""#include <stdint.h>
#include <stdio.h>
#include <string.h>
#define A64_NOESC
typedef struct { uint64_t x[32]; uint64_t vreg[32][2]; } Ctx;
volatile int hos_fixed_resolution;
volatile uint32_t hos_fixed_scale_bits;
volatile uint64_t hos_res_module;
static uint8_t mem[0x1000];
static uint64_t recomp_load32(Ctx *c, uint64_t a) { uint32_t v; (void)c; memcpy(&v, mem + (a & 0xfff), 4); return v; }
static uint64_t recomp_load64(Ctx *c, uint64_t a) { uint64_t v; (void)c; memcpy(&v, mem + (a & 0xfff), 8); return v; }
static void recomp_store32(Ctx *c, uint64_t a, uint64_t v) { uint32_t w = (uint32_t)v; (void)c; memcpy(mem + (a & 0xfff), &w, 4); }
@CALLBACKS@
static void hooked(Ctx *c) {
@HOOKED@}
static void patched(Ctx *c) {
@PATCHED@}
static uint32_t bits(float f) { uint32_t b; memcpy(&b, &f, 4); return b; }
int main(void) {
    static const float scales[] = {1.0f, 0.75f, 0.5f};
    static const float e5c[] = {1.0f, 0.75f, 0.5625f, 0.3f};
    static const float e50[] = {1.0f, 0.0f, 2.0f};
    int fails = 0;
    for (int fixed = 0; fixed < 2; fixed++)
        for (unsigned s = 0; s < 3; s++)
            for (unsigned k = 0; k < 4; k++)
                for (unsigned d = 0; d < 3; d++)
                    for (uint64_t flags = 0; flags < 32; flags += 3) {
                        uint8_t start[sizeof mem];
                        memset(mem, 0, sizeof mem);
                        uint32_t v = bits(e5c[k]); memcpy(mem + 0xe5c, &v, 4);
                        v = bits(e50[d]); memcpy(mem + 0xe50, &v, 4);
                        v = bits(0.75f); memcpy(mem + 0xe58, &v, 4);
                        v = (uint32_t)flags * 5u; memcpy(mem + 0xe2c, &v, 4);
                        memcpy(mem + 0xe20, &flags, 8);
                        memcpy(start, mem, sizeof mem);
                        hos_fixed_resolution = fixed, hos_fixed_scale_bits = bits(scales[s]);
                        Ctx a, b;
                        memset(&a, 0, sizeof a);
                        for (int r = 0; r < 31; r++) a.x[r] = 0x9e3779b97f4a7c15ull * (r + 1);
                        a.x[20] = 0, a.x[8] = flags & 8;
                        b = a;
                        hos_res_module = 1;
                        hooked(&a);
                        uint8_t after[sizeof mem];
                        memcpy(after, mem, sizeof mem);
                        const uint64_t module = hos_res_module;
                        memcpy(mem, start, sizeof mem);
                        hos_res_module = 1;
                        patched(&b);
                        if (memcmp(&a, &b, sizeof a) || memcmp(after, mem, sizeof mem) || module != hos_res_module) {
                            printf("FAIL: fixed %d, scale %u, +0xe5c %u, +0xe50 %u, flags %u\n", fixed, s, k, d, (unsigned)flags);
                            fails++;
                        }
                    }
    if (!fails) printf("PASS: the fixed path's scale reaches the 3D through its sites as through the text patch\n");
    return fails != 0;
}
"""


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default="clang")
    p.add_argument("--cargo")
    a = p.parse_args()
    sitehooks.use(a.cc, a.cargo)
    apply = site("render.fixedScaleApply", "f_831f3200", "before", 0xcfc)
    flags = site("render.fixedScaleFlags", "f_831f3200", "after", 0xd08)
    callbacks = (callback("int hos_site_fixed_scale_apply(") + "\n" + callback("int hos_site_fixed_scale_flags(") + "\n"
                 + slot(apply) + slot(flags))
    hooked = sitehooks.line(apply) + "\n" + LOAD_E2C + REST_E2C + LOAD_E20 + sitehooks.line(flags) + "\n"
    patched = APPLY + LOAD_E2C + REST_E2C + LOAD_E20 + FLAGS
    fixture = FIXTURE.replace("@CALLBACKS@", callbacks).replace("@HOOKED@", hooked).replace("@PATCHED@", patched)
    with tempfile.TemporaryDirectory(prefix="fixed-scale-sites-") as tmp:
        c, exe = Path(tmp) / "fixture.c", Path(tmp) / "fixture.exe"
        c.write_text(fixture, encoding="utf-8")
        subprocess.run([a.cc, "-std=c11", "-Wall", "-Wextra", "-Werror", str(c), *sitehooks.objects(tmp, fixture), "-o", str(exe)],
                       check=True)
        subprocess.run([str(exe)], check=True)


if __name__ == "__main__":
    main()
