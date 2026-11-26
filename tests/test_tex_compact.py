"""The game's texture heap compaction asked for once in a while (game/oot_tex_compact.h, the hook site texture.compactAsk).

The texture data manager's per-frame step (f_80797240) asks its Compaction thread for a round at every frame none runs, and
every texture a round moved is made again; here that is a batch of new textures to the renderer every few frames (the long frames
standing in Castle Town's market). The manager now asks only when its last round is a second ago (HOS_TEX_COMPACT_MS=<ms> for another gap, 0 for the game's own).

The rule (the header, compiled alone): a gap of 0 says yes at every ask, as the game has it; a negative gap never; a gap says
yes at the first ask and then only once the gap has passed since the last yes. The switch's value: not set or empty is the
default, a whole number its milliseconds, "off" or "never" or a negative number never, anything else the default.

The site: a branch site on the request's second test (tbz w8, #1 past the ask) whose callback (game/oot_sites.c) asks the host
only when the flag is set. The step is built with the hook as the lift lowers it (tests/sitehooks.py) and, side by side, with the text patch it replaced (OLD, as
main had it): for every flag and answer the same way out and the same asks of the host.

Transition check: main's text-patch C kept here as the oracle shows the sites do what it did; remove it once a
deliberate behaviour change needs it.

python -I -B tests/test_tex_compact.py [--cc clang] [--cargo cargo]
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
from sitecheck import site, callback  # noqa: E402
import sitehooks  # noqa: E402

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]
CC = "clang"

TEST = "if (!((c->x[8] >> 1) & 1)) { goto L_80001000; }\n"   # (a tbz in the lift's form; the label made up)
OLD = ("if (!((c->x[8] >> 1) & 1) || !hos_oot_tex_compact_due()) { goto L_80001000; }"   # (the text patch's, on main)
       "   /* (nx2/oot_patches.py: the texture heap's compaction once in a while) */\n")
SIDE = r"""
#include <stdint.h>
#include <stdio.h>
#define A64_NOESC
typedef struct { uint64_t x[32]; } Ctx;
static int due, asks;
int hos_oot_tex_compact_due(void) { asks++; return due; }
@CALLBACK@
volatile uint8_t hos_site_on_texture_compactAsk = 1;
int (*volatile hos_site_fn_texture_compactAsk)(Ctx *) = hos_site_tex_compact_ask;
static int hooked(Ctx *c) {
@HOOK@
    return 1;   /* (asks) */
L_80001000:;
    return 0;
}
static int patched(Ctx *c) {
@OLD@
    return 1;
L_80001000:;
    return 0;
}
int main(void) {
    int fails = 0;
    for (uint64_t flags = 0; flags < 8; flags++)
        for (due = 0; due < 2; due++) {
            Ctx a = {{0}}, b = {{0}};
            a.x[8] = b.x[8] = flags;
            asks = 0;
            const int x = hooked(&a), xa = asks;
            asks = 0;
            const int y = patched(&b), ya = asks;
            if (x != y || xa != ya || x != (((flags >> 1) & 1) && due)) { printf("FAIL flags %d due %d\n", (int)flags, due); fails++; }
        }
    if (!fails) printf("same\n");
    return fails != 0;
}
"""

FIXTURE = r"""
#include "oot_tex_compact.h"
#include <stdio.h>
static int fails;
#define CHECK(c) do { if (!(c)) { printf("FAIL line %d: %s\n", __LINE__, #c); fails++; } } while (0)
#define MS 1000000LL
int main(void) {
    {   /* (the game's own: every ask) */
        OotTexCompact s = OOT_TEX_COMPACT_INIT;
        for (int i = 0; i < 100; i++) CHECK(oot_tex_compact_due(&s, 0, i * 16 * MS) == 1);
    }
    {   /* (never) */
        OotTexCompact s = OOT_TEX_COMPACT_INIT;
        for (int i = 0; i < 100; i++) CHECK(oot_tex_compact_due(&s, -1, i * 16 * MS) == 0);
    }
    {   /* (a gap of 1 s, asked every 16 ms for 10 s from a clock that starts anywhere: the first ask, then one a second) */
        OotTexCompact s = OOT_TEX_COMPACT_INIT;
        const long long t0 = 123456789012LL;
        int yes = 0;
        long long last = -1;
        for (long long t = 0; t < 10000 * MS; t += 16 * MS) {
            const int due = oot_tex_compact_due(&s, 1000 * MS, t0 + t);
            if (due) {
                CHECK(last < 0 || t - last >= 1000 * MS);
                CHECK(last < 0 || t - last < 1016 * MS);   /* (at the first ask past the gap) */
                last = t, yes++;
            }
        }
        CHECK(yes == 10);
        CHECK(oot_tex_compact_due(&s, 1000 * MS, t0) == 0);   /* (a clock that stood still or went back: not yet) */
    }
    {   /* (the first ask is a yes even at time 0, and at a negative time) */
        OotTexCompact a = OOT_TEX_COMPACT_INIT, b = OOT_TEX_COMPACT_INIT;
        CHECK(oot_tex_compact_due(&a, 5000 * MS, 0) == 1);
        CHECK(oot_tex_compact_due(&a, 5000 * MS, 4999 * MS) == 0);
        CHECK(oot_tex_compact_due(&a, 5000 * MS, 5000 * MS) == 1);
        CHECK(oot_tex_compact_due(&b, 5000 * MS, -7 * MS) == 1);
    }
    /* (the switch's value) */
    CHECK(oot_tex_compact_gap_ns(0, 0) == 0);
    CHECK(oot_tex_compact_gap_ns("", 2000) == 2000 * MS);
    CHECK(oot_tex_compact_gap_ns(0, -1) == -1);
    CHECK(oot_tex_compact_gap_ns("0", 2000) == 0);
    CHECK(oot_tex_compact_gap_ns("1500", 0) == 1500 * MS);
    CHECK(oot_tex_compact_gap_ns("off", 2000) == -1);
    CHECK(oot_tex_compact_gap_ns("never", 2000) == -1);
    CHECK(oot_tex_compact_gap_ns("-1", 2000) == -1);
    CHECK(oot_tex_compact_gap_ns("-0", 2000) == 0);
    CHECK(oot_tex_compact_gap_ns("offf", 2000) == 2000 * MS);
    CHECK(oot_tex_compact_gap_ns("12x", 2000) == 2000 * MS);
    CHECK(oot_tex_compact_gap_ns("x", 0) == 0);
    printf(fails ? "tex_compact: %d FAILED\n" : "tex_compact: all passed\n", fails);
    return fails != 0;
}
"""


class TexCompactTests(unittest.TestCase):
    def test_rule(self):
        compiler = shutil.which("gcc") or shutil.which("clang")
        if not compiler:
            self.skipTest("no C compiler")
        with tempfile.TemporaryDirectory(prefix="longshot-tex-compact-") as tmp:
            tmp = Path(tmp)
            (tmp / "rule.c").write_text(FIXTURE, encoding="utf-8")
            binary = tmp / "rule.exe"
            build = subprocess.run([compiler, "-std=gnu11", "-Wall", "-Wextra", "-Werror", "-O1", "-I", str(ROOT / "host"), "-I", str(ROOT / "game"),
                                    str(tmp / "rule.c"), "-o", str(binary)], text=True, capture_output=True)
            self.assertEqual(build.returncode, 0, build.stderr)
            run = subprocess.run([str(binary)], text=True, capture_output=True, timeout=30)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
            self.assertIn("all passed", run.stdout)

    def test_site(self):
        x = site("texture.compactAsk", "f_80797240", "branch", 0x38)
        self.assertIn("w8:u32:flags", x.args)

    def test_as_the_text_patch(self):
        compiler = shutil.which(CC)
        if not compiler:
            self.skipTest("no C compiler")
        x = site("texture.compactAsk", "f_80797240", "branch", 0x38)
        code = (SIDE.replace("@CALLBACK@", callback("int hos_site_tex_compact_ask("))
                .replace("@HOOK@", sitehooks.line(x).replace(sitehooks.GOTO, "goto L_80001000;") + "\n" + TEST).replace("@OLD@", OLD))
        with tempfile.TemporaryDirectory(prefix="longshot-tex-compact-") as tmp:
            tmp = Path(tmp)
            (tmp / "side.c").write_text(code, encoding="utf-8")
            binary = tmp / "side.exe"
            build = subprocess.run([compiler, "-std=gnu11", "-Wall", "-Wextra", "-Werror", "-O1", str(tmp / "side.c"),
                                    *sitehooks.objects(tmp, code), "-o", str(binary)], text=True, capture_output=True)
            self.assertEqual(build.returncode, 0, build.stderr)
            run = subprocess.run([str(binary)], text=True, capture_output=True, timeout=30)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)

    def test_host_calls_the_rule(self):
        host = (ROOT / "game" / "oot_tex_compact.c").read_text(encoding="utf-8")
        for needle in ('getenv("HOS_TEX_COMPACT_MS")', "oot_tex_compact_due(&compact, compact_gap, now)",
                       "int hos_oot_tex_compact_due(void) {", "#define OOT_TEX_COMPACT_DEFAULT_MS 1000\n",
                       "oot_tex_compact_gap_ns(getenv(\"HOS_TEX_COMPACT_MS\"), OOT_TEX_COMPACT_DEFAULT_MS)"):
            self.assertIn(needle, host)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--cc", default="clang")
    ap.add_argument("--cargo")
    args, rest = ap.parse_known_args()
    CC = args.cc
    sitehooks.use(args.cc, args.cargo)
    unittest.main(argv=[sys.argv[0]] + rest)
