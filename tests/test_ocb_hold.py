"""The neighbouring map the OptimizeChangeBank thread preloads, kept while the game's choice only flickers
(host/oot_ocb_hold.h, nx2/oot_patches.py patch_ocb_hold).

The preload manager's update (f_832b9880) picks one of the map's neighbours every frame, and a change cancels the one loading
or loaded (the thread lets go of all of it and loads the other from the start). A choice flickering between two neighbours,
or in and out of a radius, reloaded them over and over with Link on one spot. The update now goes on with the held map until
another choice has stayed the choice HOS_OCB_HOLD_MS (0: the game's own at every frame).

The rule (the header, compiled alone): no hold, nothing held or the same choice: the game's choice; another choice: the held
map until that choice has lasted the hold without a break, then the choice. Which map is held, read from a manager laid out
as the game has it: the next one asked for, else the current one while it loads or is loaded (states 1-4), found among the
units in use by its path; none with a direct request in. The switch's value: not set or empty is the default, a whole number
its milliseconds, "off" 0, anything else the default.

The patch: the choice goes through the host once, in the update only, where the update tests it (with the registers as
locals, only where the manager's register has its local: else not patched); a file patched before, or
whose code differs (another game version), is left as it is; the host's function is declared.

python -I -B tests/test_ocb_hold.py
"""
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "nx2"))
import oot_patches  # noqa: E402

FIXTURE = r"""
#include "oot_ocb_hold.h"
#include <stdio.h>
#include <string.h>
static int fails;
#define CHECK(c) do { if (!(c)) { printf("FAIL line %d: %s\n", __LINE__, #c); fails++; } } while (0)
#define MS 1000000LL

/* (a guest memory: the manager at MGR, its strings inline as the game has them) */
#define BASE 0x10000000ULL
#define MGR (BASE + 0x100)
static unsigned char mem[0x4000];
static uint64_t ld(void *ctx, uint64_t a, int size) {
    (void)ctx;
    uint64_t v = 0;
    if (a < BASE || a - BASE + size > sizeof mem) return 0;
    memcpy(&v, mem + (a - BASE), size);
    return v;
}
static void put(uint64_t a, uint64_t v, int size) { memcpy(mem + (a - BASE), &v, size); }
static void str(uint64_t at, const char *s) { memcpy(mem + (at - BASE), s, strlen(s) + 1); }
static uint64_t slot(int k) { return MGR + OCB_SLOTS + (uint64_t)OCB_SLOT_SIZE * k; }
static void unit(int k, const char *path) {   /* (in use, its path in its own buffer at +0x1c, as f_80d32c00 sets it up) */
    put(slot(k), 1, 1);
    put(slot(k) + OCB_SLOT_PATH, slot(k) + 0x1c, 8);
    str(slot(k) + 0x1c, path);
}
static void manager(int state, const char *cur, const char *pend, int direct) {
    put(MGR + OCB_STATE, (uint64_t)state, 4);
    put(MGR + OCB_DIRECT, (uint64_t)direct, 1);
    put(MGR + OCB_CURRENT, MGR + 0x80, 8), str(MGR + 0x80, cur);
    put(MGR + OCB_PENDING, MGR + 0x1f4, 8), str(MGR + 0x1f4, pend);
}

int main(void) {
    const char *A = "Work/Banc/Scene/MarketEntrance_Child/MarketEntrance_Child.bcett.json";
    const char *B = "Work/Banc/Scene/TempleOfTime_Child/TempleOfTime_Child.bcett.json";
    const char *C = "Work/Banc/Scene/HyruleCastle_Child/HyruleCastle_Child.bcett.json";
    {   /* (no hold, nothing held, the same choice: the game's) */
        OotOcbHold s = OOT_OCB_HOLD_INIT;
        CHECK(oot_ocb_pick(&s, 3, 1, 0, 0) == 3);
        CHECK(oot_ocb_pick(&s, 3, -1, 1000 * MS, 0) == 3);
        CHECK(oot_ocb_pick(&s, -1, -1, 1000 * MS, 0) == -1);
        CHECK(oot_ocb_pick(&s, 1, 1, 1000 * MS, 0) == 1);
    }
    {   /* (a flicker between the held unit 1 and unit 2, every other frame for 10 s: never taken) */
        OotOcbHold s = OOT_OCB_HOLD_INIT;
        int other = 0;
        for (long long t = 0; t < 10000 * MS; t += 16 * MS) other += oot_ocb_pick(&s, (t / (16 * MS)) % 2 ? 2 : 1, 1, 1000 * MS, t) != 1;
        CHECK(other == 0);
    }
    {   /* (in and out of the held unit's radius, 900 ms out each time: never let go) */
        OotOcbHold s = OOT_OCB_HOLD_INIT;
        int other = 0;
        for (long long t = 0; t < 10000 * MS; t += 16 * MS) other += oot_ocb_pick(&s, (t % (1000 * MS)) < 900 * MS ? -1 : 1, 1, 1000 * MS, t) != 1;
        CHECK(other == 0);
    }
    {   /* (a real change: unit 2 the choice from t = 5 s on, taken at the first update 1 s later) */
        OotOcbHold s = OOT_OCB_HOLD_INIT;
        long long taken = -1;
        for (long long t = 0; t < 10000 * MS && taken < 0; t += 16 * MS)
            if (oot_ocb_pick(&s, t >= 5000 * MS ? 2 : 1, 1, 1000 * MS, t) == 2) taken = t;
        CHECK(taken >= 6008 * MS && taken < 6024 * MS);   /* (the first update at 5,008 ms, 16 ms apart) */
        CHECK(oot_ocb_pick(&s, 2, 2, 1000 * MS, taken + 16 * MS) == 2);   /* (held now) */
    }
    {   /* (leaving every radius: let go once out for the hold) */
        OotOcbHold s = OOT_OCB_HOLD_INIT;
        CHECK(oot_ocb_pick(&s, -1, 4, 500 * MS, 0) == 4);
        CHECK(oot_ocb_pick(&s, -1, 4, 500 * MS, 499 * MS) == 4);
        CHECK(oot_ocb_pick(&s, -1, 4, 500 * MS, 500 * MS) == -1);
    }
    {   /* (a break in the other choice starts its time again) */
        OotOcbHold s = OOT_OCB_HOLD_INIT;
        CHECK(oot_ocb_pick(&s, 2, 1, 1000 * MS, 0) == 1);
        CHECK(oot_ocb_pick(&s, 2, 1, 1000 * MS, 800 * MS) == 1);
        CHECK(oot_ocb_pick(&s, 3, 1, 1000 * MS, 900 * MS) == 1);
        CHECK(oot_ocb_pick(&s, 2, 1, 1000 * MS, 1500 * MS) == 1);
        CHECK(oot_ocb_pick(&s, 2, 1, 1000 * MS, 2499 * MS) == 1);
        CHECK(oot_ocb_pick(&s, 2, 1, 1000 * MS, 2500 * MS) == 2);
    }
    /* (which unit is held) */
    unit(0, A), unit(1, B), unit(5, C);
    manager(4, B, "", 0);
    CHECK(oot_ocb_held(ld, 0, MGR) == 1);   /* (loaded) */
    manager(2, C, "", 0);
    CHECK(oot_ocb_held(ld, 0, MGR) == 5);   /* (loading) */
    manager(5, B, A, 0);
    CHECK(oot_ocb_held(ld, 0, MGR) == 0);   /* (cancelling B for A: A, the next one) */
    manager(5, B, "", 0);
    CHECK(oot_ocb_held(ld, 0, MGR) == -1);  /* (cancelling, nothing next) */
    manager(0, "", "", 0);
    CHECK(oot_ocb_held(ld, 0, MGR) == -1);  /* (idle) */
    manager(4, B, "", 1);
    CHECK(oot_ocb_held(ld, 0, MGR) == -1);  /* (a direct request in) */
    manager(4, "Work/Banc/Scene/Other/Other.bcett.json", "", 0);
    CHECK(oot_ocb_held(ld, 0, MGR) == -1);  /* (a path no unit has) */
    manager(4, "Work/Banc/Scene/TempleOfTime_Child/TempleOfTime_Child.bcett.jso", "", 0);
    CHECK(oot_ocb_held(ld, 0, MGR) == -1);  /* (a prefix is not the path) */
    put(slot(1), 0, 1), manager(4, B, "", 0);
    CHECK(oot_ocb_held(ld, 0, MGR) == -1);  /* (the unit no longer in use: the map changed) */
    /* (the switch's value) */
    CHECK(oot_ocb_hold_ms(0, 1000) == 1000);
    CHECK(oot_ocb_hold_ms("", 1000) == 1000);
    CHECK(oot_ocb_hold_ms("0", 1000) == 0);
    CHECK(oot_ocb_hold_ms("off", 1000) == 0);
    CHECK(oot_ocb_hold_ms("2500", 1000) == 2500);
    CHECK(oot_ocb_hold_ms("-5", 1000) == 1000);
    CHECK(oot_ocb_hold_ms("12x", 1000) == 1000);
    CHECK(oot_ocb_hold_ms("offf", 0) == 0);
    printf(fails ? "ocb_hold: %d FAILED\n" : "ocb_hold: all passed\n", fails);
    return fails != 0;
}
"""

# (the update as the lift has it around its choice, and the next function, which has the same label text)
LIFTED = '''#include "a64.h"
void f_832b9880(Ctx *__restrict A64_NOESC c) {
    uint64_t x0_ = c->x[0], x8_ = c->x[8], x9_ = c->x[9], x19_ = c->x[19], x20_ = c->x[20], x21_ = c->x[21], x31_ = c->x[31];
c->x[30] = 0x832b9dc8ULL; c->x[0] = x0_; c->x[8] = x8_; c->x[19] = x19_; c->x[20] = x20_; c->x[21] = x21_; c->x[31] = x31_; f_803e99b0(c);x0_ = c->x[0]; x8_ = c->x[8]; x9_ = c->x[9];
L_832b9dc8:;
if (((x21_ >> 31) & 1)) { goto L_832b9e0c; }
    x8_ = 0x128ULL;
L_832b9e0c:;
}

void f_832bb980(Ctx *__restrict A64_NOESC c) {
L_832b9dc8:;
if (((x21_ >> 31) & 1)) { goto L_832b9e0c; }
}
'''
CALL = "    x21_ = hos_oot_ocb_pick(c, x19_, x21_);   " + oot_patches.OCBH_MARK + "\n"


class OcbHoldTests(unittest.TestCase):
    def test_rule(self):
        compiler = shutil.which("gcc") or shutil.which("clang")
        if not compiler:
            self.skipTest("no C compiler")
        with tempfile.TemporaryDirectory(prefix="longshot-ocb-hold-") as tmp:
            tmp = Path(tmp)
            (tmp / "rule.c").write_text(FIXTURE, encoding="utf-8")
            binary = tmp / "rule.exe"
            build = subprocess.run([compiler, "-std=gnu11", "-Wall", "-Wextra", "-Werror", "-O1", "-I", str(ROOT / "host"),
                                    str(tmp / "rule.c"), "-o", str(binary)], text=True, capture_output=True)
            self.assertEqual(build.returncode, 0, build.stderr)
            run = subprocess.run([str(binary)], text=True, capture_output=True, timeout=30)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
            self.assertIn("all passed", run.stdout)

    def test_patch(self):
        out, n = oot_patches.patch_ocb_hold(LIFTED)
        self.assertEqual(n, 1)
        update = out[out.index(oot_patches.OCBH_HEAD):out.index("void f_832bb980")]
        self.assertIn("L_832b9dc8:;\n" + CALL + "if (((x21_ >> 31) & 1)) { goto L_832b9e0c; }\n", update)
        self.assertEqual(out.count("hos_oot_ocb_pick(c,"), 1)   # (the one call; the next function's text as it was)
        self.assertIn('#include "a64.h"\n' + oot_patches.OCBH_DECL, out)
        self.assertEqual(oot_patches.patch_ocb_hold(out), (out, 1))   # (patched before: as it is)
        other = LIFTED.replace("{ goto L_832b9e0c; }\n    x8_ = 0x128ULL;", "{ goto L_832b9e10; }\n    x8_ = 0x128ULL;")
        self.assertNotEqual(other, LIFTED)
        self.assertEqual(oot_patches.patch_ocb_hold(other), (other, 0))   # (another game version's code)
        none = LIFTED.replace("void f_832b9880", "void f_932b9880")
        self.assertEqual(oot_patches.patch_ocb_hold(none), (none, 0))

    def test_patch_without_locals(self):
        plain = LIFTED.replace("    uint64_t x0_ = c->x[0], x8_ = c->x[8], x9_ = c->x[9], x19_ = c->x[19], x20_ = c->x[20], "
                               "x21_ = c->x[21], x31_ = c->x[31];\n", "").replace(
            "L_832b9dc8:;\nif (((x21_ >> 31)", "L_832b9dc8:;\nif (((c->x[21] >> 31)", 1)
        out, n = oot_patches.patch_ocb_hold(plain)
        self.assertEqual(n, 1)
        self.assertIn("    c->x[21] = hos_oot_ocb_pick(c, c->x[19], c->x[21]);", out)

    def test_patch_locals_without_the_managers_local(self):
        # (the choice in x21's local but no local for x19, the manager: c->x[19] may be stale there, so not patched)
        no_x19 = LIFTED.replace(" x19_ = c->x[19],", "", 1)
        self.assertNotEqual(no_x19, LIFTED)
        self.assertEqual(oot_patches.patch_ocb_hold(no_x19), (no_x19, 0))
        out, counts, lines = oot_patches.patch_file(no_x19, "code_037.c", False)
        self.assertNotIn("hos_oot_ocb_pick", out)

    def test_patch_file_counts_it(self):
        out, counts, lines = oot_patches.patch_file(LIFTED, "code_037.c", False)
        self.assertEqual(counts["preload choice"], 1)
        self.assertIn(CALL, out)
        self.assertTrue(any("choice of a neighbouring map" in line for line in lines), lines)
        again, counts, lines = oot_patches.patch_file(out, "code_037.c", False)
        self.assertEqual(again, out)
        self.assertEqual(counts["preload choice"], 1)

    def test_host_calls_the_rule(self):
        host = (ROOT / "host" / "oot_ocb_hold.c").read_text(encoding="utf-8")
        for needle in ('getenv("HOS_OCB_HOLD_MS")', "oot_ocb_held(guest_load, c, mgr)",
                       "oot_ocb_pick(&hold, want, held, hold_ns, hos_now_ns())",
                       "uint64_t hos_oot_ocb_pick(Ctx *c, uint64_t mgr, uint64_t chosen) {"):
            self.assertIn(needle, host)


if __name__ == "__main__":
    unittest.main()
