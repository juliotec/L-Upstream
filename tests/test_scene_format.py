"""With HDR set to Native and the PsychoV35 tone mapper the game's scene buffer is RGBA16F, not R11G11B10F; otherwise the game picks it as shipped.

The hook site render.sceneFormat (game/sites/sites.toml, a replace site on f_80672260's mov w9, #0x1a; game/oot_sites.c)
makes the R11G11B10F choice (26) RGBA16F (44) while hos_scene_rgba16f is set, which host/nn_vi.c sets from the tone mapper row
(HOS_OPT_HDR_TONEMAP: Native HDR with PsychoV35) at each present. Only the supported build has a table. The pick is built with
the hook as the lift lowers it (tests/sitehooks.py) and, side by side, with the text patch it replaced (as main had it): the same formats for
every flag and setting. The lifted text here is made up in the lift's shape, not the game's.

Transition check: main's text-patch C kept here as the oracle shows the sites do what it did; remove it once a
deliberate behaviour change needs it.

python -I -B tests/test_scene_format.py [--cc gcc]
"""
import argparse
import subprocess
import sys
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))   # (tests/srccut.py: python -I leaves this folder off the path)
from srccut import block   # noqa: E402
from sitecheck import site, callback   # noqa: E402
import sitehooks  # noqa: E402

MOV = "    c->x[9] = 0x1aULL;\n    c->x[9] &= 0xFFFFFFFFULL;\n"   # (mov w9, #0x1a as the lift writes it)
PICK = '''    { uint64_t _r = c->x[9] & 0x8ULL; _r &= 0xFFFFFFFFULL; c->x[9] = _r; }
    c->x[10] = 0x2cULL;
    c->x[10] &= 0xFFFFFFFFULL;
    { uint64_t _r = (c->x[23] | ((uint64_t)(((uint32_t)(c->x[9])) >> 3))); _r &= 0xFFFFFFFFULL; c->x[9] = _r; }
    { uint64_t _a=c->x[9]; c->z=((uint32_t)_a==0); }
''' + MOV + '''    { uint64_t _r = (!c->z) ? c->x[10] : c->x[9]; _r &= 0xFFFFFFFFULL; c->x[9] = _r; }
'''
# (the text patch's pick on main: the mov and the csel's opening made one line)
OLD = "    c->x[9] = hos_scene_rgba16f ? 0x2cULL : 0x1aULL;\n    c->x[9] &= 0xFFFFFFFFULL;\n"


def check(cond, what):
    if not cond:
        raise SystemExit("FAIL: " + what)


def site_checks():
    x = site("render.sceneFormat", "f_80672260", "replace", 0xec)
    check("w9:u32:format" in x.args, "the site does not name w9")
    return x


def source_checks():
    vi = (ROOT / "host" / "nn_vi.c").read_text(encoding="utf-8")
    check("hos_scene_rgba16f = rx_option(HOS_OPT_HDR_TONEMAP, 0) != 0;" in block(vi, "static void realtime_step("),
          "the switch is not set from the tone mapper row at each present (off without the renderer's menu)")
    replay = (ROOT / "gpu" / "vk" / "vk_replay.cpp").read_text(encoding="utf-8")
    check("if (id == HOS_OPT_HDR_TONEMAP) return scene_tonemap_on();" in replay,
          "the renderer does not answer the tone mapper's option with scene_tonemap_on (Native HDR on Vanilla must keep R11G11B10F)")
    api = (ROOT / "gpu" / "vk" / "vk_api.h").read_text(encoding="utf-8")
    check("enum { HOS_OPT_HDR_TONEMAP = 47 };" in api, "HOS_OPT_HDR_TONEMAP is not option 47")
    check("volatile int hos_scene_rgba16f = 0;" in vi, "host/nn_vi.c does not define hos_scene_rgba16f, off at start")


def fixture_checks(cc, x):
    hooked = PICK.replace(MOV, sitehooks.line(x) + "\n" + MOV + sitehooks.REPLACE_END + "\n")
    fixture = r'''#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL:%d %s\n", __LINE__, #x); exit(1); } } while (0)
#define A64_NOESC
typedef struct { uint64_t x[32]; int z; } Ctx;
volatile int hos_scene_rgba16f;
''' + callback("int hos_site_scene_format(") + r'''
volatile uint8_t hos_site_on_render_sceneFormat = 1;
int (*volatile hos_site_fn_render_sceneFormat)(Ctx *) = hos_site_scene_format;
static void pick(Ctx *c) {
''' + hooked + r'''}
static void pick_old(Ctx *c) {
''' + PICK.replace(MOV, OLD) + r'''}
static uint64_t format(int rgba16f, uint64_t flags, uint64_t x23) {
    Ctx c, d;
    memset(&c, 0, sizeof c);
    c.x[9] = flags, c.x[23] = x23;
    d = c;
    hos_scene_rgba16f = rgba16f;
    pick(&c);
    pick_old(&d);
    CHECK(memcmp(&c, &d, sizeof c) == 0);   /* (the hook and the text patch: the same registers and flags) */
    return c.x[9];
}
int main(void) {
    CHECK(format(0, 0, 0) == 26 && format(0, 8, 0) == 44 && format(0, 0, 1) == 44);   /* (as shipped) */
    CHECK(format(1, 0, 0) == 44 && format(1, 8, 0) == 44 && format(1, 0, 1) == 44);
    printf("PASS: the scene buffer is RGBA16F with the PsychoV35 tone mapper, the game's pick otherwise\n");
    return 0;
}
'''
    with tempfile.TemporaryDirectory(prefix="scene-format-") as tmp:
        c = Path(tmp) / "fixture.c"
        exe = Path(tmp) / "fixture"
        c.write_text(fixture, encoding="utf-8")
        subprocess.run([cc, "-std=c11", "-Wall", "-Wextra", "-Werror", str(c), *sitehooks.objects(tmp, fixture), "-o", str(exe)],
                       check=True)
        subprocess.run([str(exe)], check=True)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default="clang")
    p.add_argument("--cargo")
    a = p.parse_args()
    sitehooks.use(a.cc, a.cargo)
    x = site_checks()
    source_checks()
    fixture_checks(a.cc, x)


if __name__ == "__main__":
    main()
