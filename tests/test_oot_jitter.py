"""The temporal upscalers' camera jitter (DLSS, FSR 4, FSR 3.1): its hook sites jitter.viewRows, jitter.viewRowX and
jitter.viewRowY (game/sites/sites.toml) are after the loads of a view's projection where the game fills the
views' uniform buffers, and their callbacks (game/oot_sites.c) add the jitter to the loaded registers only (the third
lane: P[0][2], P[1][2]), never to the game's memory, where the shadow cascades' fit and culling would see it too (#841:
shadows flickered). The callbacks, with the hooks as the lift lowers them (tests/sitehooks.py: longshot-builder dev
site-hooks, compiled by clang), are built against a stand-in context and run. No game or GPU needed.

python -I -B tests/test_oot_jitter.py [--cc clang] [--cargo cargo]
"""
import argparse
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves the check's own folder off the path)
from sitecheck import callback, site, slot  # noqa: E402
import sitehooks  # noqa: E402

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

CC = "clang"
MAIN = 0x80020000
# (each site: its name, where, the load's word as main's nx2/oot_jitter.py checked it, and the registers whose third lane gets
# the jitter's x and y, -1 for a row not loaded there)
SITES = (
    ("jitter.viewRows", 0x1e8, 0xad415915, 19, 20),   # (ldp q21, q22, [x8, #0x20], after ldp q19, q20: rows 0 and 1)
    ("jitter.viewRowX", 0xa6c, 0x3dc0013a, 26, -1),   # (ldr q26, [x9]: row 0)
    ("jitter.viewRowY", 0xa7c, 0x3dc00126, -1, 6),    # (ldr q6, [x9]: row 1)
)
CALLBACKS = ("int hos_site_jitter_view_rows(", "int hos_site_jitter_view_row_x(", "int hos_site_jitter_view_row_y(")


def sites():
    return [site(name, "f_806dc520", "after", off) for name, off, _, _, _ in SITES]


class SiteTests(unittest.TestCase):
    def test_the_three_loads(self):
        for x, (_, off, word, _, _) in zip(sites(), SITES):
            self.assertEqual(x.signature_offset, off, "%s's signature does not start at its load" % x.name)
            self.assertEqual(x.signature[0], (word, 0xffffffff), "%s is not after the load main hooked" % x.name)
        text = (ROOT / "game" / "oot_sites.c").read_text(encoding="utf-8")
        self.assertIn("hos_dlss_jitter_clip(c->x[28] + 0x1a8, j)", text)
        self.assertNotIn("recomp_store", callback("static void jitter_rows("))   # (the game's memory is left alone)

    def test_registers_get_the_jitter_in_their_third_lane(self):
        """Each snippet, built and run: the named registers' lane 2 (bytes 8..11) moves by the jitter's x or y, the rest of
        every register stays, and with no jitter (hos_dlss_jitter_clip 0) nothing moves"""
        xs = sites()
        cases = ["#define A64_NOESC", callback("static void jitter_rows(")]
        for i, (x, marker) in enumerate(zip(xs, CALLBACKS)):
            cases += [callback(marker), slot(x), "static void site%d(Ctx *c) {\n%s\n}" % (i, sitehooks.line(x))]
        runs = []
        for i, (_, _, _, xreg, yreg) in enumerate(SITES):
            for on in (1, 0):
                runs.append("check(site%d, %d, %d, %d);" % (i, xreg, yreg, on))
        src = r"""
#include <stdint.h>
#include <stdio.h>
#include <string.h>
typedef struct { uint64_t x[32], pc; uint8_t n, z, c, v; int halted; uint64_t vreg[32][2]; } Ctx;

static int jitter_on;
static uint64_t asked;
int hos_dlss_jitter_clip(uint64_t projection, float out[2]) {
    asked = projection;
    if (!jitter_on) return 0;
    out[0] = 0.25f, out[1] = -0.5f;
    return 1;
}
%s
static int check(void (*site)(Ctx *), int xreg, int yreg, int on) {
    Ctx c, before;
    for (int r = 0; r < 32; r++) {
        c.x[r] = 0x1000u * r;
        const float lanes[4] = {r + 0.5f, r + 1.5f, 1.0f, r + 3.5f};
        memcpy(c.vreg[r], lanes, 16);
    }
    before = c;
    jitter_on = on, asked = 0;
    site(&c);
    if (asked != 0x1000u * 28 + 0x1a8) { printf("projection asked at %%llx\n", (unsigned long long)asked); return 1; }
    if (memcmp(c.x, before.x, sizeof c.x)) { puts("x registers changed"); return 1; }
    for (int r = 0; r < 32; r++) {
        float now[4], was[4];
        memcpy(now, c.vreg[r], 16), memcpy(was, before.vreg[r], 16);
        const float want = !on ? 1.0f : r == xreg ? 1.25f : r == yreg ? 0.5f : 1.0f;
        if (now[0] != was[0] || now[1] != was[1] || now[3] != was[3] || now[2] != want) {
            printf("v%%d: %%g %%g %%g %%g (lane 2 wanted %%g)\n", r, now[0], now[1], now[2], now[3], want);
            return 1;
        }
    }
    return 0;
}
int main(void) {
    int bad = 0;
#define check(...) bad |= check(__VA_ARGS__)
    %s
    return bad;
}
""" % ("\n".join(cases), "\n    ".join(runs))
        with tempfile.TemporaryDirectory(prefix="oot-jitter-") as tmp:
            c = os.path.join(tmp, "sites.c")
            exe = os.path.join(tmp, "sites")
            Path(c).write_text(src)
            subprocess.run([CC, "-std=c11", "-Wall", "-Werror", "-Wno-unused-function", c, *sitehooks.objects(tmp, src),
                            "-o", exe], check=True)
            r = subprocess.run([exe], capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--cc", default="clang")
    ap.add_argument("--cargo")
    args, rest = ap.parse_known_args()
    CC = args.cc
    sitehooks.use(args.cc, args.cargo)
    unittest.main(argv=[sys.argv[0]] + rest)
