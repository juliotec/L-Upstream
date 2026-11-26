"""Game-free checks of the steady sun shadows (game/oot_shadow_fit.c, the hook sites shadow.fitBasis and shadow.fitBox): the sites, the
sun's cascade 0 only, a world-fixed basis, a steady square on whole texels that holds the game's box, the switch, and by
default the steady fit on Shadow resolution Highest's 3072 texels only, the game's below.

The host helper is compiled with the sites' callbacks (game/oot_sites.c) and their hooks as the lift lowers them (tests/sitehooks.py) against
authored game objects: a camera turning and walking
over a slice like Hyrule Field's (0.1 to about 10 m, an ultrawide view). No dump, lifted code or game assets are needed.

    python -I -B tests/test_oot_shadow_fit.py
"""
import argparse
import os
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

MAIN_BASE = 0x80020000


def basis():
    x = site("shadow.fitBasis", "f_80733120", "after")
    # (its point is by meaning: the instruction after the call to the basis builder, f_80733620, which the lift finds)
    assert (x.point.kind, x.point.target, x.point.shift, x.point.ordinal) == ("call", "f_80733620", 1, None), x.point.describe()
    return x


def box():
    return site("shadow.fitBox", "f_80733be0", "after", 0x31c)


def words_of(x):
    """{address: word} of a site's signature and guards, each masked word the guard's whole one where a guard has it"""
    entry = int(x.function[2:], 16)
    out = {}
    for i, (w, m) in enumerate(x.signature):
        if m == 0xffffffff:
            out[entry + x.signature_offset + 4 * i] = w
    for off, ws in x.guards:
        for i, (w, m) in enumerate(ws):
            assert m == 0xffffffff
            out[entry + off + 4 * i] = w
    if x.point is not None and x.point.kind == "call":   # (the call the point names, its bl the signature's first word)
        at = entry + x.signature_offset
        assert x.signature[0] == (0x94000000, 0xfc000000), "the signature's first word is not the call"
        out[at] = 0x94000000 | (((int(x.point.target[2:], 16) - at) // 4) & 0x3ffffff)
    return out


class InstallTests(unittest.TestCase):
    def test_sites(self):
        bx, bs = basis(), box()
        words = words_of(bx)
        basis_at = int(bx.function[2:], 16) + bx.signature_offset + 4 * bx.point.shift   # (after the call)
        box_at = int(bs.function[2:], 16) + bs.offset
        # (the words main's nx2/oot_shadow_fit.py checked, each whole: the basis call and its three arguments, the box fit's
        # mode test, its branch, the meeting ldp and the three subtractions, and the other mode's way back)
        self.assertEqual(words, {MAIN_BASE + 0x713188 + 4 * i: w for i, w in enumerate((0x94000126, 0x1e204120, 0x1e204101,
                                                                                         0x91020260))})
        self.assertEqual(words_of(bs), {**{MAIN_BASE + 0x713ef0 + 4 * i: w for i, w in enumerate(
            (0x71000b1f, 0x54007840, 0x2a1f03e9, 0x2d401aa7, 0x1e253892, 0x1e223871, 0x1e203830))},
            MAIN_BASE + 0x714dfc: 0x39585289, MAIN_BASE + 0x714e00: 0x17fffc3f})
        # the basis site is the instruction right after the call (bl, imm26 forward to main+0x713620)
        call = basis_at - 4
        self.assertEqual(call + 4 * (words[call] & 0x3ffffff), MAIN_BASE + 0x713620)
        # the box site: fsub s18, s4, s5 / s17, s3, s2 / s16, s1, s0 read the bounds the hook rewrites
        ws = [words_of(bs)[box_at - 12 + 4 * i] for i in range(7)]
        for word, (d, n, m) in zip(ws[4:], ((18, 4, 5), (17, 3, 2), (16, 1, 0))):
            self.assertEqual((word & 0x1f, (word >> 5) & 0x1f, (word >> 16) & 0x1f), (d, n, m))
        self.assertEqual((ws[3] & 0x1f, (ws[3] >> 10) & 0x1f), (7, 6))   # the site's ldp loads s7, s6: not the bounds
        self.assertEqual(box_at, MAIN_BASE + 0x713ef0 + 12, "shadow.fitBox is not after the ldp where the modes meet")
        # both modes reach the site: b.eq (imm19) goes to the other mode, whose b (imm26) comes back to the ldp
        rejoin = MAIN_BASE + 0x714dfc
        beq = box_at - 8
        self.assertEqual(beq + 4 * ((ws[1] >> 5) & 0x7ffff), rejoin)
        ldrb, b = words_of(bs)[rejoin], words_of(bs)[rejoin + 4]
        self.assertEqual(ldrb, 0x39400000 | 0x614 << 10 | 20 << 5 | 9)   # ldrb w9, [x20, #0x614]
        self.assertEqual(b >> 26, 0b000101)
        self.assertEqual(rejoin + 4 + 4 * ((b & 0x3ffffff) - (1 << 26)), box_at)

    def test_callbacks(self):
        self.assertIn("hos_shadow_fit_basis(c, c->x[19]);\n    return 0;", hooked(basis(), "int hos_site_shadow_fit_basis(")[0])
        self.assertIn("hos_shadow_fit_box(c, c->x[20], b)", hooked(box(), "int hos_site_shadow_fit_box(")[0])


HEAD = r'''
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL:%d %s\n", __LINE__, #x); exit(1); } } while (0)
typedef struct { uint64_t x[32]; uint64_t vreg[32][2]; } Ctx;
static unsigned char slot[0x1000], heap[0x8000];
static int writes;
static unsigned char *at(uint64_t a, int n) {
    if (a >= 0x84e5e000ULL && a + n <= 0x84e5f000ULL) return slot + (a - 0x84e5e000ULL);
    CHECK(a >= 0x10000000ULL && a + n <= 0x10008000ULL);
    return heap + (a - 0x10000000ULL);
}
static uint64_t recomp_load64(Ctx *c, uint64_t a) { uint64_t v; (void)c; memcpy(&v, at(a, 8), 8); return v; }
static uint64_t recomp_load32(Ctx *c, uint64_t a) { uint32_t v; (void)c; memcpy(&v, at(a, 4), 4); return v; }
static void recomp_store32(Ctx *c, uint64_t a, uint64_t v) { uint32_t t = (uint32_t)v; (void)c; writes++; memcpy(at(a, 4), &t, 4); }
static void a64_vset64(uint64_t *d, uint64_t v) { d[0] = v; d[1] = 0; }
static void hos_log(const char *fmt, ...) { (void)fmt; }
/* windows.h's lock (host/posix's on Linux), one thread here: each hook takes it and gives it back, never twice */
typedef struct { int locked; } SRWLOCK;
#define SRWLOCK_INIT {0}
static void AcquireSRWLockExclusive(SRWLOCK *l) { CHECK(!l->locked); l->locked = 1; }
static void ReleaseSRWLockExclusive(SRWLOCK *l) { CHECK(l->locked); l->locked = 0; }
'''

MAIN = r'''
static void basis_site(Ctx *c) {
BASIS_C
}
static void box_site(Ctx *c) {
BOX_C
}
enum { SCENE = 0x10000000, RENDERER = 0x10000400, SUN = 0x10001000, STATE = 0x10002000, VIEW = 0x10003000,
       CASCADES = 0x10004000, OTHER = 0x10007000 };
static void put64(uint64_t a, uint64_t v) { memcpy(at(a, 8), &v, 8); }
static void put32(uint64_t a, uint32_t v) { memcpy(at(a, 4), &v, 4); }
static void putf(uint64_t a, float f) { memcpy(at(a, 4), &f, 4); }
static float getf(uint64_t a) { float f; memcpy(&f, at(a, 4), 4); return f; }
static const double sun[3] = {-0.7008, 0.4804, 0.5274};   /* row 2: towards the sun, Hyrule Field's morning */
static double z3[3];
static void norm(double v[3]) { double l = sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]); for (int i = 0; i < 3; i++) v[i] /= l; }
static void cross(const double a[3], const double b[3], double o[3]) {
    o[0] = a[1] * b[2] - a[2] * b[1]; o[1] = a[2] * b[0] - a[0] * b[2]; o[2] = a[0] * b[1] - a[1] * b[0];
}
/* A camera at pos looking along yaw (and a little down), as the game sets a cascade before the hooks: its view matrix
   (cascade+0x2e8, rows), projection (+0x318), slice (+0x144, +0x154), the game's basis (+0x158: sideways from the
   camera's direction) with its origin at eye, and the plain warp (+0x188). */
static double cam[3][4];
static void setup(uint64_t C, double yaw, const double pos[3], double far, const double eye[3]) {
    double f[3] = {sin(yaw), -0.08, cos(yaw)}; norm(f);
    double up[3] = {0, 1, 0}, r[3], u[3]; cross(f, up, r); norm(r); cross(r, f, u);
    for (int j = 0; j < 3; j++) cam[0][j] = r[j], cam[1][j] = u[j], cam[2][j] = -f[j];
    for (int i = 0; i < 3; i++) cam[i][3] = -(cam[i][0] * pos[0] + cam[i][1] * pos[1] + cam[i][2] * pos[2]);
    for (int i = 0; i < 3; i++) for (int j = 0; j < 4; j++) putf(C + 0x2e8 + 16 * i + 4 * j, (float)cam[i][j]);
    const float proj[16] = {0.60314f, 0, 0, 0, 0, 2.1445f, 0, 0, 0, 0, -1, -0.2f, 0, 0, -1, 0};
    for (int i = 0; i < 16; i++) putf(C + 0x318 + 4 * i, proj[i]);
    putf(C + 0x144, 0.1f); putf(C + 0x154, (float)far);
    double back[3] = {-cam[2][0], -cam[2][1], -cam[2][2]}, x[3], y[3];
    cross(back, z3, x); norm(x); cross(z3, x, y);
    const double *rows[3] = {x, y, z3};
    for (int i = 0; i < 3; i++) {
        for (int j = 0; j < 3; j++) putf(C + 0x158 + 16 * i + 4 * j, (float)rows[i][j]);
        putf(C + 0x158 + 16 * i + 12, (float)-(rows[i][0] * eye[0] + rows[i][1] * eye[1] + rows[i][2] * eye[2]));
    }
    const float plain[16] = {1, 0, 0, 0, 0, 1, 0, 0, 0, 0, -1, 0, 0, 0, 0, 1};
    for (int i = 0; i < 16; i++) putf(C + 0x188 + 4 * i, plain[i]);
}
/* The game's box: the slice's eight corners in the cascade's basis as it now is (the hook's or the game's). */
static void bounds(uint64_t C, float b[4]) {
    const double nf[2] = {getf(C + 0x144), getf(C + 0x154)}, tx = 1 / 0.60314, ty = 1 / 2.1445;
    b[0] = b[2] = 1e30f; b[1] = b[3] = -1e30f;
    for (int k = 0; k < 8; k++) {
        const double d = nf[k >> 2], v[3] = {(k & 1 ? 1 : -1) * d * tx - cam[0][3], (k & 2 ? 1 : -1) * d * ty - cam[1][3], -d - cam[2][3]};
        double w[3];
        for (int j = 0; j < 3; j++) w[j] = cam[0][j] * v[0] + cam[1][j] * v[1] + cam[2][j] * v[2];
        for (int r = 0; r < 2; r++) {
            double p = getf(C + 0x158 + 16 * r + 12);
            for (int j = 0; j < 3; j++) p += getf(C + 0x158 + 16 * r + 4 * j) * w[j];
            if (p < b[2 * r]) b[2 * r] = (float)p;
            if (p > b[2 * r + 1]) b[2 * r + 1] = (float)p;
        }
    }
}
/* One fit of cascade C: the basis site, then the box site with the bounds in s5/s4/s2/s3. Returns them. */
static void run(uint64_t C, float out[4], float game[4]) {
    Ctx c; memset(&c, 0, sizeof c);
    c.x[19] = C; basis_site(&c);
    bounds(C, game);
    c.x[20] = C;
    const int regs[4] = {5, 4, 2, 3};
    for (int i = 0; i < 4; i++) { uint32_t u; memcpy(&u, &game[i], 4); a64_vset64(c.vreg[regs[i]], u); c.vreg[regs[i]][1] = 7; }
    box_site(&c);
    for (int i = 0; i < 4; i++) {
        memcpy(&out[i], c.vreg[regs[i]], 4);
        if (memcmp(&out[i], &game[i], 4)) CHECK(c.vreg[regs[i]][0] >> 32 == 0 && c.vreg[regs[i]][1] == 0);
    }
}
int main(void) {
    for (int i = 0; i < 3; i++) z3[i] = sun[i];
    norm(z3);
    put64(0x84e5e258ULL, SCENE); put64(SCENE, 0x84ba95b0ULL);
    put64(SCENE + 0x1e8, RENDERER); put64(RENDERER, 0x84babc00ULL);
    put64(RENDERER + 0xb30, SUN); put64(SUN, 0x84baa0a8ULL);
    put64(SUN + 8, STATE); put64(STATE + 0xd60, VIEW); put64(VIEW + 0x390, CASCADES);
    put32(SUN + 0xe0, 1600);
    const double pos[3] = {113.0, 160.0, 185.0};
    float b[4], game[4];

    if (getenv("FIXTURE_OFF")) {   /* HOS_SHADOW_STABLE=0: nothing written, the game's bounds */
        setup(CASCADES, 0.3, pos, 10.8, pos); writes = 0; run(CASCADES, b, game);
        CHECK(writes == 0 && !memcmp(b, game, sizeof b));
        puts("PASS: steady sun shadows off"); return 0;
    }
    if (getenv("FIXTURE_GATE")) {   /* HOS_SHADOW_STABLE unset: the game's fit below Shadow resolution Highest (3072) */
        for (int round = 0; round < 2; round++) {
            put32(SUN + 0xe0, 1600);
            for (int k = 0; k < 3; k++) {
                setup(CASCADES, 0.3 + 0.2 * k, pos, 10.8, pos); writes = 0; run(CASCADES, b, game);
                CHECK(writes == 0 && !memcmp(b, game, sizeof b));
            }
            put32(SUN + 0xe0, 3072);   /* Highest, at once: the steady square, on whole texels, the game's box inside */
            double first[8];
            for (int k = 0; k < 3; k++) {
                setup(CASCADES, 0.3 + 0.2 * k, pos, 10.8, pos); writes = 0; run(CASCADES, b, game);
                CHECK(writes > 0 && memcmp(b, game, sizeof b));
                CHECK(b[0] <= game[0] && b[1] >= game[1] && b[2] <= game[2] && b[3] >= game[3]);
                CHECK(fabs((b[3] - b[2]) - (b[1] - b[0])) < 1e-4);
                const double texel = (b[1] - b[0]) / 3072, t = b[0] / texel;
                CHECK(fabs(t - floor(t + 0.5)) < 0.01);
                double m[8];
                for (int i = 0; i < 8; i++) m[i] = getf(CASCADES + 0x158 + 4 * i);
                if (!k) memcpy(first, m, sizeof first);
                for (int i = 0; i < 8; i++) if (i % 4 != 3) CHECK(fabs(m[i] - first[i]) < 1e-6);   /* one basis */
                CHECK(fabs(m[1]) < 1e-7);   /* afresh from the world's up after the size changed */
            }
        }
        puts("PASS: steady sun shadows on Highest only by default"); return 0;
    }

    /* Cascade 1, another light's cascade, a cascade with a warp: the game's basis and box. */
    const uint64_t others[3] = {CASCADES + 0x628, OTHER, CASCADES};
    for (int i = 0; i < 3; i++) {
        setup(others[i], 0.3, pos, 10.8, pos);
        if (i == 2) putf(CASCADES + 0x188 + 4 * 13, 172.3f);
        writes = 0; run(others[i], b, game);
        CHECK(writes == 0 && !memcmp(b, game, sizeof b));
    }
    /* The sun straight overhead: the game's. */
    { double keep[3]; memcpy(keep, z3, sizeof keep); z3[0] = 0; z3[1] = 1; z3[2] = 0;
      setup(CASCADES, 0.3, pos, 10.8, pos); writes = 0; run(CASCADES, b, game); CHECK(writes == 0 && !memcmp(b, game, sizeof b));
      memcpy(z3, keep, sizeof keep); }

    /* The camera turns a full circle and walks, the slice's depth wanders between 9.8 and 10.8 m: one basis, one size,
       origins on whole texels, the game's box always inside. The game's origin follows the camera; the anchor follows
       the middle of the slice's depth, by whole texels: a point fixed in the world keeps its place in its texel
       throughout. */
    double side = 0, first[8], spot[2] = {0, 0}, anchor_t[2] = {0, 0};
    int moves = 0;
    const double fixed[3] = {121.0, 158.5, 180.25};
    setup(CASCADES, 0.0, pos, 10.8, pos); run(CASCADES, b, game);   /* the deepest slice once: the size it keeps */
    for (int k = 0; k < 400; k++) {
        const double yaw = k * 0.0314159, walk[3] = {pos[0] + 0.03 * k, pos[1], pos[2] - 0.02 * k};
        setup(CASCADES, yaw, walk, 9.8 + 0.5 * (1 + sin(k * 0.21)), walk);
        run(CASCADES, b, game);
        double m[12];
        for (int i = 0; i < 12; i++) m[i] = getf(CASCADES + 0x158 + 4 * i);
        if (!k) memcpy(first, m, sizeof first);
        for (int i = 0; i < 8; i++) if (i % 4 != 3) CHECK(fabs(m[i] - first[i]) < 1e-6);   /* one basis */
        const double along = 0.5 * (0.1 + getf(CASCADES + 0x154));
        for (int r = 0; r < 2; r++) {   /* measured from within half a texel of the middle of the slice's depth */
            double p = m[4 * r + 3];
            for (int j = 0; j < 3; j++) p += m[4 * r + j] * (walk[j] - cam[2][j] * along);
            CHECK(fabs(p) <= 0.5 * (b[1] - b[0]) / 1600 + 1e-4);
        }
        const int moved = k && (m[3] != anchor_t[0] || m[7] != anchor_t[1]);
        moves += moved;
        anchor_t[0] = m[3], anchor_t[1] = m[7];
        for (int j = 0; j < 3; j++) CHECK(fabs(m[8 + j] - (float)z3[j]) < 1e-7);   /* the sun's row as the game's */
        CHECK(fabs(m[11] - (float)-(z3[0] * walk[0] + z3[1] * walk[1] + z3[2] * walk[2])) < 1e-4);   /* and its origin */
        CHECK(fabs(m[1]) < 1e-7);                                                   /* row 0 level */
        CHECK(fabs(m[0] * m[8] + m[1] * m[9] + m[2] * m[10]) < 1e-6 && fabs(m[4] * m[8] + m[5] * m[9] + m[6] * m[10]) < 1e-6);
        CHECK(fabs(m[0] * m[4] + m[1] * m[5] + m[2] * m[6]) < 1e-6 && fabs(m[0] * m[0] + m[1] * m[1] + m[2] * m[2] - 1) < 1e-6);
        CHECK(b[0] <= game[0] && b[1] >= game[1] && b[2] <= game[2] && b[3] >= game[3]);
        for (int a = 0; a < 2; a++) {   /* the fixed point's place in its texel, as the map is sampled */
            const double u = (m[4 * a] * fixed[0] + m[4 * a + 1] * fixed[1] + m[4 * a + 2] * fixed[2] + m[4 * a + 3] - b[2 * a])
                             / (b[2 * a + 1] - b[2 * a]) * 1600, f = u - floor(u);
            if (!k) spot[a] = f;
            CHECK(fabs(f - spot[a]) < 0.01 || fabs(fabs(f - spot[a]) - 1) < 0.01);
        }
        const double s = b[1] - b[0];
        CHECK(fabs((b[3] - b[2]) - s) < 1e-4);
        if (!k) side = s;
        CHECK(fabs(s - side) < 1e-4);   /* one size while turning and walking */
        CHECK(side >= 30.0 && side <= 40.0);
        const double texel = side / 1600;
        for (int a = 0; a < 2; a++) { const double t = b[2 * a] / texel; CHECK(fabs(t - floor(t + 0.5)) < 0.01); }
    }
    CHECK(moves >= 300);   /* the turn and the walk move it nearly every fit */
    /* The sun moves (0.06 degrees a fit, its bearing and height both changing), the camera still: rows 0 and 1 turn by
       the smallest turn that follows it, with no spin about the sun's direction (rows rebuilt from the world's up spin
       by about 1e-4 a fit here); a point at the anchor keeps its place in its texel. A jump of 5 degrees starts the
       rows again from the world's up, level. */
    { const double ax[3] = {0.2, 0.6, 0.77}, ang = 0.001;
      double prev[8] = {0}, at[3];
      setup(CASCADES, 0.3, pos, 10.8, pos); run(CASCADES, b, game);
      for (int j = 0; j < 3; j++) at[j] = pos[j] - cam[2][j] * 0.5 * (0.1 + 10.8);
      for (int k = 0; k < 300; k++) {
          double a[3]; memcpy(a, ax, sizeof a); norm(a);
          double kx[3]; cross(a, z3, kx);
          const double kd = a[0] * z3[0] + a[1] * z3[1] + a[2] * z3[2];
          for (int j = 0; j < 3; j++) z3[j] = z3[j] * cos(ang) + kx[j] * sin(ang) + a[j] * kd * (1 - cos(ang));
          setup(CASCADES, 0.3, pos, 10.8, pos); run(CASCADES, b, game);
          double m[8];
          for (int i = 0; i < 8; i++) m[i] = getf(CASCADES + 0x158 + 4 * i);
          CHECK(fabs(m[0] * z3[0] + m[1] * z3[1] + m[2] * z3[2]) < 1e-6 && fabs(m[4] * z3[0] + m[5] * z3[1] + m[6] * z3[2]) < 1e-6);
          if (k) CHECK(fabs(prev[4] * m[0] + prev[5] * m[1] + prev[6] * m[2]) < 2e-6);   /* no spin about the sun */
          for (int r = 0; r < 2; r++) {
              const double p = m[4 * r] * at[0] + m[4 * r + 1] * at[1] + m[4 * r + 2] * at[2] + m[4 * r + 3];
              CHECK(fabs(p) <= 0.5 * (b[1] - b[0]) / 1600 + 1e-4);
          }
          memcpy(prev, m, sizeof prev);
      }
      CHECK(fabs(prev[1]) > 1e-3);   /* carried, row 0 is no longer level */
      double a[3] = {1, 0, 0}, kx[3]; cross(a, z3, kx);
      const double j5 = 5 * 3.14159265 / 180;
      for (int j = 0; j < 3; j++) z3[j] = z3[j] * cos(j5) + kx[j] * sin(j5) + a[j] * z3[0] * (1 - cos(j5));
      setup(CASCADES, 0.3, pos, 10.8, pos); run(CASCADES, b, game);
      CHECK(fabs(getf(CASCADES + 0x158 + 4)) < 1e-7);
      for (int i = 0; i < 3; i++) z3[i] = sun[i];
      norm(z3); }
    /* A wider slice grows it at once; a narrower one shrinks it only after 90 fits in a row. */
    setup(CASCADES, 0.3, pos, 16.0, pos); run(CASCADES, b, game);
    const double wide = b[1] - b[0];
    CHECK(wide > side + 8 && b[0] <= game[0] && b[1] >= game[1] && b[2] <= game[2] && b[3] >= game[3]);
    for (int k = 0; k < 89; k++) { setup(CASCADES, 0.3, pos, 10.0, pos); run(CASCADES, b, game); CHECK(fabs((b[1] - b[0]) - wide) < 1e-4); }
    setup(CASCADES, 0.3, pos, 10.0, pos); run(CASCADES, b, game);
    CHECK(b[1] - b[0] < wide - 8);
    /* A bigger map: finer texels, the same square. */
    put32(SUN + 0xe0, 3072); setup(CASCADES, 1.0, pos, 10.0, pos); run(CASCADES, b, game);
    { const double texel = (b[1] - b[0]) / 3072, t = b[0] / texel; CHECK(fabs(t - floor(t + 0.5)) < 0.01); }
    /* The game's second box fit of the frame (main+0x712b9c), on another thread with no basis just before it: the box
       site alone, on the cascade the basis site fixed, gives the same square. */
    { setup(CASCADES, 0.3, pos, 10.0, pos); run(CASCADES, b, game);
      Ctx c; memset(&c, 0, sizeof c); c.x[20] = CASCADES; const int regs[4] = {5, 4, 2, 3}; float again[4];
      for (int i = 0; i < 4; i++) { uint32_t u; memcpy(&u, &game[i], 4); a64_vset64(c.vreg[regs[i]], u); }
      box_site(&c);
      for (int i = 0; i < 4; i++) memcpy(&again[i], c.vreg[regs[i]], 4);
      CHECK(!memcmp(again, b, sizeof b) && memcmp(b, game, sizeof b)); }
    /* The box site on cascade 0 with the game's basis (the basis site didn't run, or left it) keeps the game's bounds. */
    { setup(CASCADES, 0.3, pos, 10.0, pos);
      Ctx c; memset(&c, 0, sizeof c); c.x[20] = CASCADES; float g[4] = {1, 2, 3, 4};
      for (int i = 0; i < 4; i++) { uint32_t u; memcpy(&u, &g[i], 4); a64_vset64(c.vreg[(int[]){5, 4, 2, 3}[i]], u); }
      Ctx before = c; box_site(&c); CHECK(!memcmp(&c, &before, sizeof c)); }
    puts("PASS: steady sun shadows: cascade 0 of the sun only, one basis and size over a turn, whole texels, the game's box inside");
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
    host = (ROOT / "game/oot_shadow_fit.c").read_text(encoding="utf-8")
    body = "".join(line for line in host.splitlines(keepends=True) if not line.startswith("#include"))
    with tempfile.TemporaryDirectory(prefix="oot-shadow-fit-") as temporary:
        source = Path(temporary) / "fixture.c"
        executable = Path(temporary) / "fixture.exe"
        basis_defs, basis_line = hooked(basis(), "int hos_site_shadow_fit_basis(")
        box_defs, box_line = hooked(box(), "int hos_site_shadow_fit_box(")
        text = (HEAD + body + basis_defs + box_defs.replace("#ifndef A64_NOESC\n#define A64_NOESC\n#endif\n", "")
                + MAIN.replace("BASIS_C", basis_line).replace("BOX_C", box_line))
        source.write_text(text, encoding="utf-8")
        subprocess.run([args.cc, "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", str(source),
                        *sitehooks.objects(temporary, text), "-o", str(executable), "-lm"], check=True)
        subprocess.run([str(executable)], check=True, env=dict(os.environ, HOS_SHADOW_STABLE="1"))
        subprocess.run([str(executable)], check=True,
                       env=dict({k: v for k, v in os.environ.items() if k != "HOS_SHADOW_STABLE"}, FIXTURE_GATE="1"))
        subprocess.run([str(executable)], check=True, env=dict(os.environ, HOS_SHADOW_STABLE="0", FIXTURE_OFF="1"))


if __name__ == "__main__":
    main()
