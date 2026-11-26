"""Dynamic resolution's three hook sites do what its text patch did (nx2/oot_patches.py patch_dynres, deleted with them).

f_80825aa0 tests the engine's config flag (tbz w8, #0 to the fixed path); the patch made it `if (hos_fixed_resolution ||
!(w8 & 1)) goto fixed; hos_dynres_runs++;` and put hos_fixed_scale_bits in place of the fixed path's 1.0 (mov w9,
#0x3f800000). The sites (game/sites/sites.toml): dynres.fixedPath, a branch site before the tbz; dynres.dynamicFrame,
before the instruction after it (reached only from it); dynres.fixedScale, a replace site on the mov. Checked here, game-free:
the sites are in the table where the patch was, the host gives them the callbacks of game/oot_sites.c, and the patch's C
and the hooks as the lift lowers them (tests/sitehooks.py), with those callbacks, run side by side over every flag and setting, leave the same
registers, the same count of dynamic frames and take the same path.

Transition check: main's text-patch C kept here as the oracle shows the sites do what it did; remove it once a
deliberate behaviour change needs it.

python -I -B tests/test_dynres_sites.py [--cc clang] [--cargo cargo]
"""
import argparse
import subprocess
import sys
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves this folder off the path)
from srccut import block   # noqa: E402
from sitecheck import BUILD, host_sites, ident, load  # noqa: E402
import sitehooks  # noqa: E402

# (the patch's C on main, at the test and at the scale; the lines between them stand for the two paths)
OLD_TEST = "if (hos_fixed_resolution || !((c->x[8] >> 0) & 1)) { goto L_fixed; }\n    hos_dynres_runs++;\n"
OLD_SCALE = "    c->x[9] = hos_fixed_scale_bits;\n    c->x[9] &= 0xFFFFFFFFULL;\n"
LIFT_TEST = "if (!((c->x[8] >> 0) & 1)) { goto L_fixed; }\n"   # (the tbz as the lift writes it)
LIFT_SCALE = "    c->x[9] = 0x3f800000ULL;\n    c->x[9] &= 0xFFFFFFFFULL;\n"   # (the mov)


def check(cond, what):
    if not cond:
        raise SystemExit("FAIL: " + what)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default="clang")
    p.add_argument("--cargo")
    a = p.parse_args()
    sitehooks.use(a.cc, a.cargo)
    sites = {x.name: x for x in load(BUILD)}
    for name, at, offset in (("dynres.fixedPath", "branch", 0x130), ("dynres.dynamicFrame", "before", 0x134),
                             ("dynres.fixedScale", "replace", 0x980)):
        x = sites.get(name)
        check(x is not None and (x.function, x.at, x.offset) == ("f_80825aa0", at, offset),
              "%s is not the %s site at f_80825aa0+%#x" % (name, at, offset))
        check(host_sites().get(ident(name)) == name, "game/sites.def has no slot for " + name)

    calls = (ROOT / "game" / "oot_sites.c").read_text(encoding="utf-8")
    branch = sitehooks.line(sites["dynres.fixedPath"]).replace(sitehooks.GOTO, "goto L_fixed;")
    before = sitehooks.line(sites["dynres.dynamicFrame"])
    replace = sitehooks.line(sites["dynres.fixedScale"])
    fixture = r'''#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL:%d %s\n", __LINE__, #x); exit(1); } } while (0)
#define A64_NOESC
typedef struct { uint64_t x[32]; uint64_t vreg[32][2]; uint8_t n, z, c, v; } Ctx;
volatile int hos_fixed_resolution, hos_dynres_runs;
volatile uint32_t hos_fixed_scale_bits = 0x3f800000;
''' + "\n".join(block(calls, m) for m in ("int hos_site_dynres_fixed_path(", "int hos_site_dynres_dynamic_frame(",
                                          "int hos_site_dynres_fixed_scale(")) + r'''
volatile uint8_t hos_site_on_dynres_fixedPath = 1, hos_site_on_dynres_dynamicFrame = 1, hos_site_on_dynres_fixedScale = 1;
int (*volatile hos_site_fn_dynres_fixedPath)(Ctx *) = hos_site_dynres_fixed_path;
int (*volatile hos_site_fn_dynres_dynamicFrame)(Ctx *) = hos_site_dynres_dynamic_frame;
int (*volatile hos_site_fn_dynres_fixedScale)(Ctx *) = hos_site_dynres_fixed_scale;
/* the test and the fixed path's scale with the text patch (main), and with the sites: 1 the fixed path, 0 the dynamic one */
static int old_code(Ctx *c) {
''' + OLD_TEST + r'''    return 0;
L_fixed:;
''' + OLD_SCALE + r'''    return 1;
}
static int new_code(Ctx *c) {
''' + branch + "\n" + LIFT_TEST + before + "\n" + r'''    return 0;
L_fixed:;
''' + replace + "\n" + LIFT_SCALE + sitehooks.REPLACE_END + r'''
    return 1;
}
int main(void) {
    static const uint32_t scales[] = {0x3f800000, 0x3f400000, 0x40000000};
    for (int fixed = 0; fixed < 2; fixed++)
        for (uint64_t w8 = 0; w8 < 4; w8++)
            for (unsigned k = 0; k < 3; k++) {
                Ctx was, now;
                memset(&was, 0, sizeof was);
                for (int i = 0; i < 32; i++) was.x[i] = 0x9e3779b97f4a7c15ull * (i + 1);
                was.x[8] = w8 | 0xabcd00000000ull, was.x[9] = 0x1234567890ull;
                now = was;
                hos_fixed_resolution = fixed, hos_fixed_scale_bits = scales[k];
                hos_dynres_runs = 0;
                const int path_was = old_code(&was), runs_was = hos_dynres_runs;
                hos_dynres_runs = 0;
                const int path_now = new_code(&now), runs_now = hos_dynres_runs;
                CHECK(path_was == path_now && runs_was == runs_now);
                CHECK(memcmp(&was, &now, sizeof was) == 0);
                CHECK(path_now == (fixed || !(w8 & 1)));   /* (the fixed path whenever the host holds it) */
            }
    printf("PASS: dynamic resolution's sites do what its text patch did\n");
    return 0;
}
'''
    with tempfile.TemporaryDirectory(prefix="dynres-sites-") as tmp:
        c = Path(tmp) / "fixture.c"
        exe = Path(tmp) / "fixture"
        c.write_text(fixture, encoding="utf-8")
        subprocess.run([a.cc, "-std=c11", "-Wall", "-Wextra", "-Werror", str(c), *sitehooks.objects(tmp, fixture), "-o", str(exe)],
                       check=True)
        subprocess.run([str(exe)], check=True)


if __name__ == "__main__":
    main()
