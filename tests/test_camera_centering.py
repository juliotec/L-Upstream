"""Disable camera centering (the Controls row, host/mouse_native.c hos_disable_camera_centering) skips the follow camera's
movement yaw through the hook site camera.followYaw (game/sites/sites.toml), a jump site with game/oot_sites.c's callback,
where the text patch it replaced (nx2/oot_patches.py patch_camera_centering, deleted) put
`if (hos_disable_camera_centering()) { c->x[25] = c->x[26]; goto L_80645ad8; }` on the fall-through of the authored heading
override's b.ne (L_8064720c).

Checked here, game-free: the site is the jump at f_80644c80+0x2590 (the b.ne's fall-through, which no other way reaches: no
label) to +0xe58 (L_80645ad8), and a block with the hook as the lift lowers it (tests/sitehooks.py) and one with the patch's line, run side by side with the row
off and on, take the same way and leave the same registers.

Transition check: main's text-patch C kept here as the oracle shows the sites do what it did; remove it once a
deliberate behaviour change needs it.

python -I -B tests/test_camera_centering.py [--cc clang] [--cargo cargo]
"""
import argparse
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]
CC = "clang"

sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves this folder off the path)
from sitecheck import site, callback, slot  # noqa: E402
import sitehooks  # noqa: E402

OLD = ("    if (hos_disable_camera_centering()) { c->x[25] = c->x[26]; goto L_80001100; }"
       "   /* (nx2/oot_patches.py: disable camera centering) */\n")
SIDE = r"""
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#define A64_NOESC
typedef struct { uint64_t x[32]; uint8_t z; } Ctx;
static int row;
int hos_disable_camera_centering(void) { return row; }
@CALLBACK@
/* (made up, in the lift's form: the authored override's b.ne, its fall-through: the movement path, then the yaw
   paths' join; the labels are written here) */
static int hooked(Ctx *c) {
L_80001000:;
if (!c->z) { goto L_80001200; }
@HOOK@
    c->x[1] = c->x[14];
    c->x[25] = c->x[26] + 24;
    return 1;
L_80001100:;
    return 2;
L_80001200:;
    return 3;
}
static int patched(Ctx *c) {
L_80001000:;
if (!c->z) { goto L_80001200; }
@OLD@
    c->x[1] = c->x[14];
    c->x[25] = c->x[26] + 24;
    return 1;
L_80001100:;
    return 2;
L_80001200:;
    return 3;
}
int main(void) {
    int fails = 0;
    for (row = 0; row < 2; row++)
        for (int z = 0; z < 2; z++) {
            Ctx a, b;
            memset(&a, 0, sizeof a);
            for (int r = 0; r < 32; r++) a.x[r] = 0x9e3779b97f4a7c15ull * (r + 1);
            a.z = (uint8_t)z;
            b = a;
            const int x = hooked(&a), y = patched(&b);
            if (x != y || memcmp(&a, &b, sizeof a) || x != (!z ? 3 : row ? 2 : 1)) { printf("FAIL row %d z %d\n", row, z); fails++; }
        }
    if (!fails) printf("same\n");
    return fails != 0;
}
"""


class CameraCenteringSiteTests(unittest.TestCase):
    def test_the_site(self):
        x = site("camera.followYaw", "f_80644c80", "jump", 0x2590)
        self.assertEqual(x.to, 0xe58)
        self.assertIn("x26:ptr:base", x.args)

    def test_as_the_text_patch(self):
        compiler = shutil.which(CC)
        if not compiler:
            self.skipTest("no C compiler")
        x = site("camera.followYaw", "f_80644c80", "jump", 0x2590)
        code = (SIDE.replace("@CALLBACK@", callback("int hos_site_camera_follow_yaw(") + "\n" + slot(x))
                .replace("@HOOK@", sitehooks.line(x).replace(sitehooks.GOTO, "goto L_80001100;")).replace("@OLD@", OLD))
        with tempfile.TemporaryDirectory(prefix="longshot-centering-") as tmp:
            tmp = Path(tmp)
            (tmp / "side.c").write_text(code, encoding="utf-8")
            build = subprocess.run([compiler, "-std=gnu11", "-Wall", "-Wextra", "-Werror", "-Wno-unused-label",
                                    str(tmp / "side.c"), *sitehooks.objects(tmp, code), "-o", str(tmp / "side.exe")],
                                   text=True, capture_output=True)
            self.assertEqual(build.returncode, 0, build.stderr)
            run = subprocess.run([str(tmp / "side.exe")], text=True, capture_output=True, timeout=30)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--cc", default="clang")
    ap.add_argument("--cargo")
    args, rest = ap.parse_known_args()
    CC = args.cc
    sitehooks.use(args.cc, args.cargo)
    unittest.main(argv=[sys.argv[0]] + rest)
