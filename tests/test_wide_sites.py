"""Widescreen and ultrawide's 23 hook sites (wide.*, ui.*: game/sites/sites.toml, game/oot_wide_sites.c) do
what main's C spliced after the same instructions did (nx2/oot_wide.py, deleted with them), game-free.

- Each site is at the instruction main's C went after (main offset + 0x80020000, an after site, its host slot), and its
  signature holds that instruction's word as main checked it.
- Differential: main's after-texts, kept below verbatim as nx2/oot_wide.py at a2dc5385 wrote them (wide.movie's as
  d2c9f7cc, #2358, left it: movies at 16:9 on narrower pictures too) (MAIN_SITES), and each site's hook as the lift
  lowers it (tests/sitehooks.py) with its callback (game/oot_wide_sites.c, compiled whole) run on the same contexts:
  thousands of each, every register and lane drawn from pools that reach every branch (16:9 and near it, the UI
  projection's 2/3840 and 2/2160, the save picture's 0, 0, 543, 306, the kept screens' vtables, ratios of 0, 16:9, 21:9,
  32:9, 4:3, 16:10 and -1, scales 0, 1 and others, the flags on and off), with guest memory and the host's values drawn the
  same way. The whole context, guest memory, the host's values and the log of the host functions they call (with their
  arguments, in order) must be the same after each.
No game files, image or lifted code are used.

Transition check: main's text-patch C kept here as the oracle shows the sites do what it did; remove it once a
deliberate behaviour change needs it.

python -I -B tests/test_wide_sites.py [--cc clang] [--cargo cargo]
"""
import argparse
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))   # (python -I leaves the check's own folder off the path)
from sitecheck import BUILD, check, load, site, slot  # noqa: E402
import sitehooks  # noqa: E402

MAIN = 0x80020000
# (main's after-texts, nx2/oot_wide.py at a2dc5385 (wide.movie's at d2c9f7cc, #2358), verbatim: the site that replaced it,
# the main offset of the instruction it went after, the instruction's word there as main checked it, and the C)
MAIN_SITES = (
    ('wide.outSize', 0x1202ee4, 0xf2c0b408,
     '{ extern volatile uint64_t hos_wide_out; const uint64_t _s = hos_wide_out; if (_s) c->x[8] = _s; }   /* (nx2/oot_wide.py: hos_wide_out) */'),
    ('wide.outSizeOther', 0x31f39f8, 0xf2c0b408,
     '{ extern volatile uint64_t hos_wide_out; const uint64_t _s = hos_wide_out; if (_s) c->x[8] = _s; }   /* (nx2/oot_wide.py: hos_wide_out) */'),
    ('wide.sceneSize', 0x31f3a04, 0xf2c08708,
     '{ extern volatile uint64_t hos_wide_scene; const uint64_t _s = hos_wide_scene; if (_s) c->x[8] = _s; }   /* (nx2/oot_wide.py: hos_wide_scene) */'),
    ('wide.cameraRatio', 0x4c5334, 0xbd414260,
     '{ extern volatile float hos_wide_ratio; const float _w = hos_wide_ratio; if (_w > 0.0f) { float _r; memcpy(&_r, c->vreg[0], 4); const float _d = _r - (16.0f / 9.0f); if (_d <= 0.01f && _d >= -0.01f) { uint32_t _u; memcpy(&_u, &_w, 4); a64_vset64(c->vreg[0], _u); } } }   /* (nx2/oot_wide.py: camera ratio) */'),
    ('wide.projectionRatio', 0x3485cc0, 0x2d558400,
     '{ extern volatile float hos_wide_ratio; const float _w = hos_wide_ratio; if (_w > 0.0f) { float _r; memcpy(&_r, c->vreg[1], 4); const float _d = _r - (16.0f / 9.0f); if (_d <= 0.01f && _d >= -0.01f) { uint32_t _u; memcpy(&_u, &_w, 4); a64_vset64(c->vreg[1], _u); recomp_store32(c, c->x[0] + 0xb0, _u); } } }   /* (nx2/oot_wide.py: camera ratio) */'),
    ('wide.projectionRatioCopy', 0x4f1e8, 0x2d558ea0,
     '{ extern volatile float hos_wide_ratio; const float _w = hos_wide_ratio; if (_w > 0.0f) { float _r; memcpy(&_r, c->vreg[3], 4); const float _d = _r - (16.0f / 9.0f); if (_d <= 0.01f && _d >= -0.01f) { uint32_t _u; memcpy(&_u, &_w, 4); a64_vset64(c->vreg[3], _u); recomp_store32(c, c->x[21] + 0xb0, _u); } } }   /* (nx2/oot_wide.py: camera ratio) */'),
    ('wide.projectionRatioCopy2', 0x23b9a8, 0x2d558c00,
     '{ extern volatile float hos_wide_ratio; const float _w = hos_wide_ratio; if (_w > 0.0f) { float _r; memcpy(&_r, c->vreg[3], 4); const float _d = _r - (16.0f / 9.0f); if (_d <= 0.01f && _d >= -0.01f) { uint32_t _u; memcpy(&_u, &_w, 4); a64_vset64(c->vreg[3], _u); recomp_store32(c, c->x[0] + 0xb0, _u); } } }   /* (nx2/oot_wide.py: camera ratio) */'),
    ('wide.projectionRatioCopy3', 0x33b82c, 0x2d558440,
     '{ extern volatile float hos_wide_ratio; const float _w = hos_wide_ratio; if (_w > 0.0f) { float _r; memcpy(&_r, c->vreg[1], 4); const float _d = _r - (16.0f / 9.0f); if (_d <= 0.01f && _d >= -0.01f) { uint32_t _u; memcpy(&_u, &_w, 4); a64_vset64(c->vreg[1], _u); recomp_store32(c, c->x[2] + 0xb0, _u); } } }   /* (nx2/oot_wide.py: camera ratio) */'),
    ('wide.clearance', 0x71a844, 0xbd451e6a,
     '{ extern volatile float hos_wide_ratio; const float _w = hos_wide_ratio; if (_w > 0.0f && _w != (16.0f / 9.0f)) { float _r; memcpy(&_r, c->vreg[10], 4); if (_r == _w) { const float _normal = 16.0f / 9.0f; uint32_t _u; memcpy(&_u, &_normal, 4); a64_vset64(c->vreg[10], _u); } } }   /* (nx2/oot_wide.py: camera clearance) */'),
    ('wide.clearanceFollow', 0x71b624, 0xbd451e6e,
     '{ extern volatile float hos_wide_ratio; const float _w = hos_wide_ratio; if (_w > 0.0f && _w != (16.0f / 9.0f)) { float _r; memcpy(&_r, c->vreg[14], 4); if (_r == _w) { const float _normal = 16.0f / 9.0f; uint32_t _u; memcpy(&_u, &_normal, 4); a64_vset64(c->vreg[14], _u); } } }   /* (nx2/oot_wide.py: camera clearance) */'),
    ('wide.hud', 0x774368, 0xad401041,
     '{ extern volatile float hos_wide_squeeze; if (hos_wide_squeeze > 0.0f) { float _m00, _m11; memcpy(&_m00, c->vreg[1], 4); memcpy(&_m11, (const uint8_t *)c->vreg[4] + 4, 4); if (_m00 == (2.0f / 3840.0f) && _m11 == (2.0f / 2160.0f)) { const uint64_t _vt = recomp_load64(c, c->x[0] - 0x1b8); extern volatile uint32_t hos_wide_keep_mask; const uint32_t _kb = _vt == 0x84c0d098ULL ? 1u : _vt == 0x84c04fb0ULL ? 2u : _vt == 0x84c061c8ULL ? 4u : _vt == 0x84c07b30ULL ? 8u : _vt == 0x84c0f4a0ULL ? 16u : 0u; if (!(_kb & hos_wide_keep_mask)) { extern volatile float hos_ui_qx, hos_ui_qy; float _qx = hos_ui_qx, _qy = hos_ui_qy, _row[4]; if (!(_qx > 0.0f)) _qx = hos_wide_squeeze, _qy = 1.0f; if (_qx != 1.0f) { memcpy(_row, c->vreg[1], 16); for (int _k = 0; _k < 4; _k++) _row[_k] *= _qx; memcpy(c->vreg[1], _row, 16); } if (_qy > 0.0f && _qy != 1.0f) { memcpy(_row, c->vreg[4], 16); for (int _k = 0; _k < 4; _k++) _row[_k] *= _qy; memcpy(c->vreg[4], _row, 16); } } } } }   /* (nx2/oot_wide.py: the HUD and menus at their 16:9 shape) */'),
    ('wide.bubble', 0x20c2c8, 0x1e210841,
     '{ extern volatile float hos_ui_qx, hos_ui_qy; const float _qx = hos_ui_qx, _qy = hos_ui_qy; if (_qx > 0.0f && _qy > 0.0f) { float _x, _y; memcpy(&_x, c->vreg[0], 4); memcpy(&_y, c->vreg[1], 4); _x /= _qx; _y /= _qy; memcpy(c->vreg[0], &_x, 4); memcpy(c->vreg[1], &_y, 4); } }   /* (nx2/oot_wide.py: the speech bubble over its speaker) */'),
    ('wide.movie', 0x3b5c758, 0xa941a269,
     '{ extern volatile float hos_wide_margin; const float _m = hos_wide_margin; if (_m > 0.0f) { const uint64_t _vp = c->x[29] - 40; uint32_t _ua = (uint32_t)recomp_load32(c, _vp + 8), _ub = (uint32_t)recomp_load32(c, _vp + 16); float _a, _b; memcpy(&_a, &_ua, 4); memcpy(&_b, &_ub, 4); const float _in = (_b - _a) * _m; _a += _in; _b -= _in; memcpy(&_ua, &_a, 4); memcpy(&_ub, &_b, 4); recomp_store32(c, _vp + 8, _ua); recomp_store32(c, _vp + 16, _ub); } else { extern volatile float hos_wide_ratio; const float _r = hos_wide_ratio; const float _game = 16.0f / 9.0f; if (_r > 0.0f && _r < _game * 0.99f) { const uint64_t _vp = c->x[29] - 40; uint32_t _ua = (uint32_t)recomp_load32(c, _vp + 12), _ub = (uint32_t)recomp_load32(c, _vp + 20); float _a, _b; memcpy(&_a, &_ua, 4); memcpy(&_b, &_ub, 4); const float _in = (_b - _a) * (1.0f - _r / _game) / 2.0f; _a += _in; _b -= _in; memcpy(&_ua, &_a, 4); memcpy(&_ub, &_b, 4); recomp_store32(c, _vp + 12, _ua); recomp_store32(c, _vp + 20, _ub); } } }   /* (nx2/oot_wide.py: movies at 16:9, with side bars on wider pictures or top and bottom bars on narrower ones) */'),
    ('wide.savePicture', 0x284824, 0xf9400108,
     "{ extern volatile float hos_wide_ratio; const float _r = hos_wide_ratio; if (_r > 0.0f && (uint32_t)c->x[1] == 0 && (uint32_t)c->x[2] == 0 && (uint32_t)c->x[3] == 543 && (uint32_t)c->x[4] == 306) { if (_r > 16.0f / 9.0f) { const int _w = (int)(543.0f * _r / (16.0f / 9.0f) + 0.5f); c->x[1] = (uint32_t)((543 - _w) / 2); c->x[3] = (uint32_t)_w; } else { const int _h = (int)(306.0f * (16.0f / 9.0f) / _r + 0.5f); c->x[2] = (uint32_t)((306 - _h) / 2); c->x[4] = (uint32_t)_h; } } }   /* (nx2/oot_wide.py: the save's picture, the middle 16:9) */"),
    ('ui.calcDirty', 0x152a730, 0xb9400008,
     '{ extern volatile uint32_t hos_wide_ui_force; if (hos_wide_ui_force) { c->x[8] |= 0x40; recomp_store32(c, c->x[0], (uint32_t)c->x[8]); } }   /* (nx2/oot_wide.py: every pane remakes its transform) */'),
    ('ui.paneDirty', 0x152caa0, 0xb9400008,
     '{ extern volatile uint32_t hos_wide_ui_force; if (hos_wide_ui_force) c->x[8] |= 0x40; }   /* (nx2/oot_wide.py: every pane remakes its transform) */'),
    ('ui.paneNoParent', 0x152cd98, 0xad4207e0,
     "{ extern void hos_wide_ui_parent(uint64_t parent); hos_wide_ui_parent(0); }   /* (nx2/oot_wide.py: the pane's parent) */"),
    ('ui.paneParent', 0x152cdd4, 0xb9403008,
     "{ extern void hos_wide_ui_parent(uint64_t parent); hos_wide_ui_parent(c->x[0]); }   /* (nx2/oot_wide.py: the pane's parent) */"),
    ('ui.panePlaced', 0x152ce64, 0xfc04c263,
     "{ extern void hos_wide_ui_placed(Ctx *c, uint64_t pane); hos_wide_ui_placed(c, c->x[19]); }   /* (nx2/oot_wide.py: the HUD's side panes at the screen's edges) */"),
    ('ui.paneSize', 0x152ac40, 0x2d4b0400,
     "{ extern void hos_wide_ui_size(Ctx *c, uint64_t pane); hos_wide_ui_size(c, c->x[0]); }   /* (nx2/oot_wide.py: spanning panes the screen's width) */"),
    ('ui.pictureConstants', 0x15306f4, 0xa940a7e8,
     "{ extern void hos_wide_ui_bg(Ctx *c); hos_wide_ui_bg(c); }   /* (nx2/oot_wide.py: the pause menu's background in pieces) */"),
    ('ui.pictureDraw', 0x1530740, 0xaa0003f6,
     "{ extern void hos_wide_ui_bg_draw(Ctx *c); hos_wide_ui_bg_draw(c); }   /* (nx2/oot_wide.py: the pause menu's background in pieces) */"),
    ('ui.layoutBuilt', 0x1525ca0, 0x91004260,
     '{ extern void hos_wide_ui_built(Ctx *c, uint64_t layout); hos_wide_ui_built(c, c->x[19]); }   /* (nx2/oot_wide.py: a list widened as it is built) */'),
)


FIXTURE = r"""
#define _CRT_SECURE_NO_WARNINGS
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define A64_NOESC
typedef struct { uint64_t x[32]; uint64_t vreg[32][2]; uint8_t n, z, c, v; } Ctx;
static uint8_t mem[0x4000];
static uint8_t *G(uint64_t a) { return mem + (uint32_t)a % (sizeof mem - 16); }
static uint64_t recomp_load32(Ctx *c, uint64_t a) { uint32_t v; (void)c; memcpy(&v, G(a), 4); return v; }
static uint64_t recomp_load64(Ctx *c, uint64_t a) { uint64_t v; (void)c; memcpy(&v, G(a), 8); return v; }
static void recomp_store32(Ctx *c, uint64_t a, uint64_t v) { uint32_t t = (uint32_t)v; (void)c; memcpy(G(a), &t, 4); }
static void a64_vset64(uint64_t *v, uint64_t bits) { v[0] = bits; v[1] = 0; }
volatile uint64_t hos_wide_out, hos_wide_scene;
volatile float hos_wide_ratio, hos_wide_squeeze, hos_wide_margin;
volatile uint32_t hos_wide_keep_mask;
volatile float hos_ui_qx, hos_ui_qy;
volatile uint32_t hos_wide_ui_force;
/* (the host functions the sites call: each logged with its arguments and leaving a mark in the context, so a call missed,
   doubled, moved or with another argument shows) */
static uint64_t calls[64];
static unsigned ncalls;
static void note(uint64_t what, uint64_t a, Ctx *c) {
    if (ncalls + 2 <= 64) calls[ncalls++] = what, calls[ncalls++] = a;
    if (c) c->x[5] = c->x[5] * 31 + what + a, c->vreg[7][1] ^= a;
}
void hos_wide_ui_parent(uint64_t parent) { note(1, parent, NULL); }
void hos_wide_ui_placed(Ctx *c, uint64_t pane) { note(2, pane, c); }
void hos_wide_ui_size(Ctx *c, uint64_t pane) { note(3, pane, c); }
void hos_wide_ui_bg(Ctx *c) { note(4, c->x[8], c); }
void hos_wide_ui_bg_draw(Ctx *c) { note(5, c->x[1], c); }
void hos_wide_ui_built(Ctx *c, uint64_t layout) { note(6, layout, c); }
int hos_camera_trace_enabled(void) { return 0; }
void hos_log(const char *fmt, ...) { (void)fmt; note(7, 0, NULL); }
/* (the title card's sites, game/oot_wide_sites.c's too, are tests/test_title_card_sites.py's: here only what they link to) */
typedef void (*A64Fn)(Ctx *);
_Thread_local uint64_t hos_site_callee;
static A64Fn a64_lookup(uint64_t addr) { (void)addr; return NULL; }
void hos_wide_title_prepare(Ctx *c, uint64_t picture, uint64_t pane, uint64_t params, uint64_t alloc, uint64_t align,
                            uint64_t out, void (*allocate)(Ctx *)) {
    (void)picture, (void)pane, (void)params, (void)alloc, (void)align, (void)out;
    allocate(c);
}
int hos_wide_title_draw(Ctx *c, void (*draw)(Ctx *)) { (void)c, (void)draw; return 0; }
void hos_wide_title_reset(uint64_t allocator) { (void)allocator; }
@CALLBACKS@
@SLOTS@
@OLD@
@NEW@
typedef void (*Fn)(Ctx *);
static const Fn old_fns[] = { @OLD_LIST@ };
static const Fn new_fns[] = { @NEW_LIST@ };
static const char *names[] = { @NAMES@ };

static uint64_t rng = 0x9e3779b97f4a7c15ULL;
static uint64_t next(void) { rng ^= rng << 13, rng ^= rng >> 7, rng ^= rng << 17; return rng; }
static uint32_t fbits(float f) { uint32_t u; memcpy(&u, &f, 4); return u; }
static float pick_float(const float *pool, unsigned n) { return pool[next() % n]; }
/* a 32-bit lane: random, or a value a site tests for */
static uint32_t lane(void) {
    const float pool[] = { 16.0f / 9.0f, 16.0f / 9.0f + 0.005f, 16.0f / 9.0f - 0.009f, 16.0f / 9.0f + 0.02f, 2.0f / 3840.0f,
                           2.0f / 2160.0f, 21.0f / 9.0f, 32.0f / 9.0f, 4.0f / 3.0f, 1.6f, 0.0f, 1.0f, -1.0f, 1920.5f, 300.25f };
    const unsigned k = next() % 24;
    return k < sizeof pool / sizeof pool[0] ? fbits(pool[k]) : (uint32_t)next();
}
static uint64_t xreg(void) {
    const uint64_t pool[] = { 0, 543, 306, 1, 0x1000, 0x1800, 0x2400, 0x3000 };
    const unsigned k = next() % 12;
    return k < 8 ? pool[k] + (k >= 4 ? (next() % 64) * 16 : 0) : next();
}
static void setup(Ctx *c) {
    for (unsigned i = 0; i < sizeof mem; i += 4) { const uint32_t w = (next() & 3) ? (uint32_t)next() : lane(); memcpy(mem + i, &w, 4); }
    memset(c, 0, sizeof *c);
    for (int i = 0; i < 32; i++) c->x[i] = xreg();
    for (int i = 0; i < 32; i++) {
        const uint64_t lo = lane() | (next() & 1 ? (uint64_t)lane() << 32 : 0), hi = next() & 1 ? next() : 0;
        c->vreg[i][0] = lo, c->vreg[i][1] = hi;
    }
    c->n = next() & 1, c->z = next() & 1, c->c = next() & 1, c->v = next() & 1;
    /* (the save picture's arguments together, now and then) */
    if (next() % 3 == 0) c->x[1] = 0, c->x[2] = 0, c->x[3] = 543 | (next() & 1 ? 0 : 0x100000000ULL), c->x[4] = 306;
    /* (the screen behind the canvas: a kept one now and then) */
    const uint64_t vt[] = { KEEP_VTABLES };
    if (next() & 1) { const uint64_t v = vt[next() % (sizeof vt / sizeof vt[0])]; memcpy(G(c->x[0] - 0x1b8), &v, 8); }
    /* (the UI projection's rows: q1's first lane and q4's second, now and then) */
    if (next() % 3 == 0) {
        c->vreg[1][0] = (c->vreg[1][0] & ~0xffffffffULL) | fbits(2.0f / 3840.0f);
        c->vreg[4][0] = (c->vreg[4][0] & 0xffffffffULL) | (uint64_t)fbits(2.0f / 2160.0f) << 32;
    }
    const float ratios[] = { 0.0f, 16.0f / 9.0f, 21.0f / 9.0f, 32.0f / 9.0f, 4.0f / 3.0f, 1.6f, -1.0f };
    const float scales[] = { 0.0f, 1.0f, 0.75f, 1.3333f, 0.5f, -1.0f };
    hos_wide_ratio = pick_float(ratios, 7);
    if (next() % 4 == 0) {   /* (a ratio just loaded that is the picture's, for the clearance) */
        const int r = next() & 1 ? 10 : 14;
        c->vreg[r][0] = (c->vreg[r][0] & ~0xffffffffULL) | fbits(hos_wide_ratio);
    }
    hos_wide_squeeze = pick_float(scales, 6), hos_wide_margin = next() & 1 ? 0.0f : 0.125f;
    hos_ui_qx = pick_float(scales, 6), hos_ui_qy = pick_float(scales, 6);
    const uint32_t masks[] = { 0, 0x1f, 0x1d, 0x15, 0x0f, 0x10 };
    hos_wide_keep_mask = masks[next() % 6];
    hos_wide_ui_force = next() % 3;
    hos_wide_out = next() & 1 ? 0 : next(), hos_wide_scene = next() & 1 ? 0 : next();
}
typedef struct { Ctx c; uint8_t mem[sizeof mem]; uint64_t calls[64]; unsigned ncalls; uint64_t out, scene; float ratio, squeeze,
                 margin, qx, qy; uint32_t keep, force; } State;
static void save(State *s, const Ctx *c) {
    memcpy(&s->c, c, sizeof *c);
    memcpy(s->mem, mem, sizeof mem);
    memcpy(s->calls, calls, sizeof calls);
    s->ncalls = ncalls, s->out = hos_wide_out, s->scene = hos_wide_scene, s->ratio = hos_wide_ratio, s->squeeze = hos_wide_squeeze;
    s->margin = hos_wide_margin, s->qx = hos_ui_qx, s->qy = hos_ui_qy, s->keep = hos_wide_keep_mask, s->force = hos_wide_ui_force;
}
static void load(const State *s, Ctx *c) {
    memcpy(c, &s->c, sizeof *c);
    memcpy(mem, s->mem, sizeof mem);
    memcpy(calls, s->calls, sizeof calls);
    ncalls = s->ncalls, hos_wide_out = s->out, hos_wide_scene = s->scene, hos_wide_ratio = s->ratio, hos_wide_squeeze = s->squeeze;
    hos_wide_margin = s->margin, hos_ui_qx = s->qx, hos_ui_qy = s->qy, hos_wide_keep_mask = s->keep, hos_wide_ui_force = s->force;
}
static State start, a, b;
int main(void) {
    int failures = 0;
    for (unsigned k = 0; k < sizeof old_fns / sizeof old_fns[0]; k++) {
        unsigned changed = 0, cases = 6000;
        for (unsigned i = 0; i < cases; i++) {
            Ctx c;
            setup(&c);
            ncalls = 0;
            memset(calls, 0, sizeof calls);
            save(&start, &c);
            old_fns[k](&c);
            save(&a, &c);
            load(&start, &c);
            new_fns[k](&c);
            save(&b, &c);
            if (memcmp(&a, &b, sizeof a)) {
                printf("FAIL: %s: case %u differs from main's C\n", names[k], i);
                failures++;
                break;
            }
            changed += memcmp(&a, &start, sizeof a) != 0;
        }
        if (!changed) { printf("FAIL: %s: no case changed anything (the pools miss its branch)\n", names[k]); failures++; }
        else printf("%s: %u cases as main's C, %u of them changing something\n", names[k], cases, changed);
    }
    if (!failures) puts("PASS: every widescreen site does what main's C after the instruction did");
    return failures != 0;
}
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cc", default="clang")
    parser.add_argument("--cargo")
    args = parser.parse_args()
    sitehooks.use(args.cc, args.cargo)
    sites = []
    table = {s.name: s for s in load(BUILD)}
    for name, off, word, _ in MAIN_SITES:
        pc = MAIN + off
        check(name in table, "the table has no site %s" % name)
        x = site(name, table[name].function, "after", pc - int(table[name].function[2:], 16))
        at = (x.offset - x.signature_offset) // 4
        check(0 <= at < len(x.signature) and x.signature[at] == (word, 0xffffffff),
              "%s's signature does not hold main's word %08x at its instruction" % (name, word))
        sites.append(x)
    check(len({MAIN + off for _, off, _, _ in MAIN_SITES}) == len(MAIN_SITES), "two sites at one instruction")
    host = (ROOT / "game" / "oot_wide_sites.c").read_text(encoding="utf-8")
    host = re.sub(r"^#include.*$", "", host, flags=re.M)
    keep = re.findall(r"#define WIDE_KEEP_\w+ (0x[0-9a-f]+ULL)", host)
    check(len(keep) == 5, "game/oot_wide_sites.c does not name the five kept screens")
    old = "".join("static void old_%d(Ctx *c) {\n    (void)c;\n%s\n}\n" % (i, text) for i, (_, _, _, text) in enumerate(MAIN_SITES))
    new = "".join("static void new_%d(Ctx *c) {\n%s\n}\n" % (i, sitehooks.line(x)) for i, x in enumerate(sites))
    source = (FIXTURE.replace("@CALLBACKS@", host).replace("@SLOTS@", "".join(slot(x) for x in sites))
              .replace("@OLD@", old).replace("@NEW@", new).replace("KEEP_VTABLES", ", ".join(keep))
              .replace("@OLD_LIST@", ", ".join("old_%d" % i for i in range(len(sites))))
              .replace("@NEW_LIST@", ", ".join("new_%d" % i for i in range(len(sites))))
              .replace("@NAMES@", ", ".join('"%s"' % x.name for x in sites)))
    with tempfile.TemporaryDirectory(prefix="wide-sites-") as directory:
        src, exe = Path(directory) / "wide.c", Path(directory) / "wide.exe"
        src.write_text(source, encoding="utf-8")
        subprocess.run([args.cc, "-std=gnu11", "-O1", "-ffp-contract=off", "-Wall", "-Wextra", "-Werror", "-Wno-unused-function",
                        str(src), *sitehooks.objects(directory, source), "-o", str(exe)], check=True)
        env = {k: v for k, v in os.environ.items() if k != "A64_CAMERA_DIAGNOSTICS"}
        run = subprocess.run([str(exe)], text=True, capture_output=True, timeout=120, env=env)
        print(run.stdout, end="")
        if run.returncode:
            raise SystemExit("FAIL: a widescreen site differs from main's C" + run.stderr)
    print("PASS: the 23 widescreen sites are at main's instructions, and the output size's edit finds wide.outSize's hook")


if __name__ == "__main__":
    main()
