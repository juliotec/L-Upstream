"""Game-free Shadow resolution hook: the site, the sun's object only, raise-only, live changes, menu wiring.

Compile the actual host helper (game/oot_shadow.c) with the hook site shadow.cascadeSize's callback (game/oot_sites.c) and
its hook as the lift lowers it (tests/sitehooks.py), against an authored shadow object.
No dump, lifted code or game assets are needed.

    python -I -B tests/test_oot_shadow.py
"""
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves the check's own folder off the path)
from sitecheck import hooked, site  # noqa: E402
import sitehooks  # noqa: E402

# (the rounding to a multiple of 8, the size packed into the width and height pair, and its store at the object's +0xe0, as
# main's nx2/oot_shadow.py checked them)
WORDS = (0x121d7117, 0xd343fee8, 0xaa1703e9, 0xb35d7109, 0xf9007269)


def cascade_size():
    return site("shadow.cascadeSize", "f_80492370", "after", 0x680)


class InstallTests(unittest.TestCase):
    def test_site(self):
        x = cascade_size()
        self.assertEqual(x.signature_offset, x.offset, "shadow.cascadeSize's signature does not start at its instruction")
        self.assertEqual([w for w, _ in x.signature], list(WORDS))
        self.assertTrue(all(m == 0xffffffff for _, m in x.signature))
        self.assertIn("hos_oot_shadow(c, c->x[19]);\n    return 0;", hooked(x, "int hos_site_shadow_cascade_size(")[0])

    def test_site_is_the_rounding_into_w23(self):
        # and w23, w8, #0xfffffff8: the rounded size the hook raises, read by every later use of it
        self.assertEqual(WORDS[0], 0x121d7117)
        self.assertEqual(WORDS[0] & 0x1f, 23)

    def test_lift_and_menu_wiring(self):
        menu = (ROOT / "gpu/vk/vk_menu.cpp").read_text(encoding="utf-8")
        api = (ROOT / "game/options.h").read_text(encoding="utf-8")   # (the game's option ids)
        options = (ROOT / "gpu/vk/vk_menu.h").read_text(encoding="utf-8")
        replay = (ROOT / "gpu/vk/vk_replay.cpp").read_text(encoding="utf-8")
        self.assertIn("HOS_OPT_SHADOW_RES = 56", api)
        self.assertIn("int shadow_res = 0;", options)
        detail = (ROOT / "gpu/vk/menu/detail.cpp").read_text(encoding="utf-8")   # (the row's module, #1633)
        self.assertRegex(detail, r'(?s)\.label = "Shadow resolution",.*?\.value = &opts\.shadow_res,\s*\.lo = 0, \.hi = 3,.*?'
                                 r'tx_shadow_res\(v\)')
        self.assertIn("if (id == HOS_OPT_SHADOW_RES) return menu_shadow_res();", replay)

    def test_auto_follows_the_cards_memory(self):
        # Auto, the default, is the size for the memory tier: the game's 1600 on Minimum and Low (4 and 6 GB), 2200 on
        # Balanced (8 GB), 3072 from High (12 GB) up; a chosen size is the row's less one (the host's 0 to 2); a preset
        # puts the row back on Auto
        menu = (ROOT / "gpu/vk/vk_menu.cpp").read_text(encoding="utf-8")
        start = menu.index("int menu_shadow_res() {")
        fn = menu[start:menu.index("\n}\n", start)]
        self.assertIn("if (opts.shadow_res > 0) return std::min(opts.shadow_res, 3) - 1;", fn)
        self.assertIn("const int tier = opts.budget > 0 ? opts.budget : auto_choice.tier;", fn)
        self.assertIn("return tier == 1 || tier == 2 ? 0 : tier >= 4 ? 2 : 1;", fn)
        self.assertLess(menu.index("} // namespace\n"), start, "menu_shadow_res is the header's, outside the file's own namespace")
        engine = (ROOT / "gpu/vk/menu/preset.cpp").read_text(encoding="utf-8")   # (the Quality preset module's, #1633)
        start = engine.index("void apply_preset(int p) {")
        preset = engine[start:engine.index("\n}\n", start)]
        self.assertIn("if (p > 0) opts.shadow_res = 0;", preset)
        self.assertNotIn("opts.shadow_res = 2;", preset)


HEAD = r'''
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL:%d %s\n", __LINE__, #x); exit(1); } } while (0)
typedef struct { uint64_t x[32]; unsigned char vreg[32][16]; } Ctx;
enum { HOS_OPT_SHADOW_RES = 56 };
static unsigned char objects[0x1000];
static int menu, reads, asked;
static uint64_t recomp_load64(Ctx *c, uint64_t a) {
    uint64_t v; (void)c; reads++;
    CHECK(a >= 0x10000000ULL && a + 8 <= 0x10001000ULL);
    memcpy(&v, objects + (a - 0x10000000ULL), 8); return v;
}
int rx_option(int id, int otherwise) { CHECK(id == HOS_OPT_SHADOW_RES && otherwise == 0); asked++; return menu; }
'''

MAIN = r'''
static void site(Ctx *c) {
SITE_C
}
static const uint64_t sun = 0x10000100, other = 0x10000800;
static Ctx input(uint64_t object, uint32_t size) {
    Ctx c; memset(&c, 0xab, sizeof c);
    c.x[19] = object; c.x[23] = size;   /* and w23: the upper half zero */
    return c;
}
static uint32_t after(uint64_t object, uint32_t size) { Ctx c = input(object, size); site(&c); return (uint32_t)c.x[23]; }
static void unchanged(uint64_t object, uint32_t size) {
    Ctx c = input(object, size), before = c; site(&c); CHECK(!memcmp(&c, &before, sizeof c));
}
int main(void) {
    uint64_t vt = 0x84baa0a8ULL; memcpy(objects + 0x100, &vt, 8);
    uint64_t else_vt = 0x84baa0b0ULL; memcpy(objects + 0x800, &else_vt, 8);
    unsigned char before[sizeof objects]; memcpy(before, objects, sizeof objects);

    /* Game's and choices out of range touch no game memory and keep every register. */
    for (int i = 0; i < 4; i++) { menu = (int[]){0, -1, 3, 100}[i]; unchanged(sun, 1600); }
    CHECK(reads == 0 && asked == 4);

    /* High and Highest raise the sun's rounded size, read live each frame. */
    menu = 1; CHECK(after(sun, 1600) == 2200);
    menu = 2; CHECK(after(sun, 1600) == 3072);
    menu = 1; CHECK(after(sun, 1600) == 2200);
    menu = 0; unchanged(sun, 1600);   /* back to the game's at once: nothing cached */

    /* A frame where the game asks for more keeps the game's (raise only); the register's upper half stays zero. */
    menu = 1; CHECK(after(sun, 3072) == 3072); CHECK(after(sun, 2400) == 2400);
    { Ctx c = input(sun, 1000); menu = 2; site(&c); CHECK(c.x[23] == 3072); }

    /* Only x23 changes. */
    { menu = 2; Ctx c = input(sun, 1600), expected = c; expected.x[23] = 3072; site(&c); CHECK(!memcmp(&c, &expected, sizeof c)); }

    /* Another object, or none, keeps its size. */
    menu = 2; unchanged(other, 1600); unchanged(0, 1600);
    CHECK(!memcmp(objects, before, sizeof objects));
    puts("PASS: shadow resolution: the sun's object only, raise only, live changes, register preservation");
    return 0;
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cc", default="clang")
    parser.add_argument("--cargo")
    args = parser.parse_args()
    sitehooks.use(args.cc, args.cargo)
    result = unittest.TextTestRunner().run(unittest.defaultTestLoader.loadTestsFromTestCase(InstallTests))
    if not result.wasSuccessful():
        raise SystemExit(1)
    host = (ROOT / "game/oot_shadow.c").read_text(encoding="utf-8")
    body = "".join(line for line in host.splitlines(keepends=True) if not line.startswith("#include"))
    with tempfile.TemporaryDirectory(prefix="oot-shadow-") as temporary:
        source = Path(temporary) / "fixture.c"
        executable = Path(temporary) / "fixture.exe"
        defs, line = hooked(cascade_size(), "int hos_site_shadow_cascade_size(")
        text = HEAD + body + defs + MAIN.replace("SITE_C", line)
        source.write_text(text, encoding="utf-8")
        subprocess.run([args.cc, "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", str(source),
                        *sitehooks.objects(temporary, text), "-o", str(executable)], check=True)
        subprocess.run([str(executable)], check=True)


if __name__ == "__main__":
    main()
