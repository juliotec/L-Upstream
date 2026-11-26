/* (longshot) Widescreen and ultrawide for Ocarina of Time 1.1.1: the engine's callbacks at the hook sites wide.* and ui.*
   (game/sites/sites.toml, game/sites.def), each after one of the game's instructions.

   The game draws a 2560x1440 picture whatever the screen and renders its scene at 1920x1080; its cameras use a 16:9 ratio.
   With these sites the picture takes the window's shape (the height stays the game's, the width grows on an ultrawide
   screen and shrinks on a 16:10 or 4:3 one), the 16:9 cameras take the picture's ratio, the HUD and menus keep their 16:9
   shape in the middle of a wider picture (a narrower one: its whole width), pre-rendered movies keep their 16:9 shape,
   and the picture a save keeps shows that middle 16:9 (a narrower picture: its middle band, at the texture's shape). Every
   callback reads host globals (host/hos_main.c, game/hos_wide_ui.c) that stay 0 for a 16:9 window, so the game runs as
   shipped then.

   The sites and what each does come from NX2navi's recomp/patches-v111.txt, where they are credited to Ivan0xFF (MIT,
   "The matrix-update, UI and movie sites: Copyright (c) 2026 Ivan0xFF"); the picture and scene size sites are the same
   constants that file names; the save picture's site is Longshot's own. Each is C after the original instruction instead
   of a replacement of it. The HUD's layout sites are from https://github.com/NotRexed/Hookshot/pull/2843. */
#include "game_host.h"   /* (what the host gives the game: host/game_host.h) */
#include "sites.h"
#include <stdlib.h>
#include <string.h>

/* ---- build e0f3ccfa's addresses the callbacks read (docs/dev/reference/hooks.md: a callback naming an address belongs to that build) */
/* (the screens whose UI keeps the whole picture: the item-wheel and pause / system menu backgrounds redraw a capture of
   the whole picture; the event and loading fades cover it. Bit k of host/hos_main.c's hos_wide_keep_mask keeps screen k
   whole; the Insert menu's Ultrawide menus row clears bit 1, the pause / system menus, and bit 3, the loading fade
   (vk_framegen.cpp): their layout then keeps its 16:9 shape in the middle (the loading fade's placed as the HUD's,
   game/hos_wide_ui.c), instead of being stretched over the whole picture. Bit 0, the item wheel's background (the last
   frame of play, blurred, HUD and all), and bit 2, the event fade, keep covering it all: bit 0 cleared, that capture went
   into the middle at twice its width. By the class each vtable's factory makes: UIShortCutBGScreen, UICommonScreen,
   UIEventFadeScreen, UILoadingFadeScreen. Bit 4: UISystemOverlayScreen, which draws the item quick select's background
   and the system menus' header and buttons. Squeezed, that background went into the middle 16:9 from its first opening
   on, so it is kept whole; the Ultrawide menus row at Extended clears the bit on a wider screen (game/hos_wide_ui.c), the
   screen then squeezed and its layout placed as the menus') */
#define WIDE_KEEP_SHORTCUT_BG 0x84c0d098ULL
#define WIDE_KEEP_COMMON 0x84c04fb0ULL
#define WIDE_KEEP_EVENT_FADE 0x84c061c8ULL
#define WIDE_KEEP_LOADING_FADE 0x84c07b30ULL
#define WIDE_KEEP_SYSTEM_OVERLAY 0x84c0f4a0ULL
#define WIDE_PICTURE_DRAW 0x81550720ULL   /* (a picture's draw, f_81550720: ui.titleDraw's function, drawn again per strip) */
/* ---- end of the build's addresses */

#define SIXTEEN_NINE (16.0f / 9.0f)
#define UI_M00 (2.0f / 3840.0f)   /* (the UI's full-screen orthographic projection, 3840x2160 grid) */
#define UI_M11 (2.0f / 2160.0f)

extern volatile uint64_t hos_wide_out, hos_wide_scene;   /* (host/hos_main.c) */
extern volatile float hos_wide_ratio, hos_wide_squeeze, hos_wide_margin;
extern volatile uint32_t hos_wide_keep_mask;
extern volatile float hos_ui_qx, hos_ui_qy;   /* (game/hos_wide_ui.c) */
extern volatile uint32_t hos_wide_ui_force;
void hos_wide_ui_parent(uint64_t parent);
void hos_wide_ui_placed(Ctx *c, uint64_t pane);
void hos_wide_ui_size(Ctx *c, uint64_t pane);
void hos_wide_ui_bg(Ctx *c);
void hos_wide_ui_bg_draw(Ctx *c);
void hos_wide_ui_built(Ctx *c, uint64_t layout);
void hos_wide_title_prepare(Ctx *c, uint64_t picture, uint64_t pane, uint64_t params, uint64_t alloc, uint64_t align,
                            uint64_t out, void (*allocate)(Ctx *));
int hos_wide_title_draw(Ctx *c, void (*draw)(Ctx *));
void hos_wide_title_reset(uint64_t allocator);
int hos_camera_trace_enabled(void);   /* (host/hos_main.c: camera-clearance.trace is there) */
typedef struct { unsigned long long hits, changed, traced; } ClearanceCounts;   /* (the clearance's diagnostics, a site's) */

/* A size just made in x8 (the movk of its height): the host's, when it gives one */
static void wide_size(Ctx *c, uint64_t size) {
    if (size) c->x[8] = size;
}

/* wide.outSize, wide.outSizeOther (after movk x8, #0x5a0, lsl #32: {2560, 1440}): the picture's size */
int hos_site_wide_out_size(Ctx *A64_NOESC c) {
    wide_size(c, hos_wide_out);
    return 0;
}

/* wide.sceneSize (after movk x8, #0x438, lsl #32: {1920, 1080}): the scene the game's upscaler takes in */
int hos_site_wide_scene_size(Ctx *A64_NOESC c) {
    wide_size(c, hos_wide_scene);
    return 0;
}

/* A ratio just loaded into s<vreg>: when it is 16:9 (within 0.01), the picture's; with base (not negative), also stored
   back at x<base> + 0xb0 (sead's perspective projection keeps it there, and culling reads it) */
static void wide_ratio(Ctx *c, int vreg, int base) {
    const float w = hos_wide_ratio;
    if (w > 0.0f) {
        float r;
        memcpy(&r, c->vreg[vreg], 4);
        const float d = r - SIXTEEN_NINE;
        if (d <= 0.01f && d >= -0.01f) {
            uint32_t u;
            memcpy(&u, &w, 4);
            a64_vset64(c->vreg[vreg], u);
            if (base >= 0) recomp_store32(c, c->x[base] + 0xb0, u);
        }
    }
}

/* wide.cameraRatio (after the camera update's ldr s0, [x19, #0x140]: the camera's ratio, copied into the projection the
   renderer reads) */
int hos_site_wide_camera_ratio(Ctx *A64_NOESC c) {
    wide_ratio(c, 0, -1);
    return 0;
}

/* wide.projectionRatio (after sead's perspective projection's matrix update loads ldp s0, s1, [x0, #0xac]) and its three
   inlined copies (wide.projectionRatioCopy, Copy2, Copy3): the ratio in s1 or s3, the projection in x0, x21, x0 or x2 */
int hos_site_wide_projection_ratio(Ctx *A64_NOESC c) {
    wide_ratio(c, 1, 0);
    return 0;
}
int hos_site_wide_projection_ratio_copy(Ctx *A64_NOESC c) {
    wide_ratio(c, 3, 21);
    return 0;
}
int hos_site_wide_projection_ratio_copy2(Ctx *A64_NOESC c) {
    wide_ratio(c, 3, 0);
    return 0;
}
int hos_site_wide_projection_ratio_copy3(Ctx *A64_NOESC c) {
    wide_ratio(c, 1, 2);
    return 0;
}

/* The camera's near-plane clearance sphere keeps the game's ratio, not the picture's (issue 1703): a ratio just loaded
   into s<vreg> that is the picture's (not 16:9) becomes 16:9 again.

   A64_CAMERA_DIAGNOSTICS=1 (read once, at the first hit): each site also counts its hits and its changes, and while
   camera-clearance.trace is in the game's working directory logs up to 256 hits ("camera clearance main+<offset>: hits=
   changed= input= host="), again after the marker was removed and a hit came without it. Diagnostic runs check for the
   marker on every hit. */

static int clearance_diagnostics(void) {
    static int on = -1;
    int v = __atomic_load_n(&on, __ATOMIC_RELAXED);
    if (v < 0) {
        const char *e = getenv("A64_CAMERA_DIAGNOSTICS");
        v = e && !strcmp(e, "1");
        __atomic_store_n(&on, v, __ATOMIC_RELAXED);
    }
    return v;
}

static void wide_clearance(Ctx *c, int vreg, uint32_t main_offset, ClearanceCounts *n) {
    const float w = hos_wide_ratio;
    const int diagnostics = clearance_diagnostics();
    uint32_t input = 0, host = 0;
    unsigned long long hit = 0;
    if (diagnostics) {
        memcpy(&input, c->vreg[vreg], 4);
        memcpy(&host, &w, 4);
        hit = __atomic_add_fetch(&n->hits, 1, __ATOMIC_RELAXED);
    }
    if (w > 0.0f && w != SIXTEEN_NINE) {
        float r;
        memcpy(&r, c->vreg[vreg], 4);
        if (r == w) {
            if (diagnostics) __atomic_fetch_add(&n->changed, 1, __ATOMIC_RELAXED);
            const float normal = SIXTEEN_NINE;
            uint32_t u;
            memcpy(&u, &normal, 4);
            a64_vset64(c->vreg[vreg], u);
        }
    }
    if (!diagnostics) return;
    if (!hos_camera_trace_enabled()) __atomic_store_n(&n->traced, 0, __ATOMIC_RELAXED);
    else if (__atomic_fetch_add(&n->traced, 1, __ATOMIC_RELAXED) < 256)
        hos_log("camera clearance main+%#x: hits=%llu changed=%llu input=%08x host=%08x", main_offset, hit,
                __atomic_load_n(&n->changed, __ATOMIC_RELAXED), input, host);
}

/* wide.clearance (after ldr s10, [x19, #0x51c]) and wide.clearanceFollow (after its follow-up branch's ldr s14, [x19, #0x51c]) */
int hos_site_wide_clearance(Ctx *A64_NOESC c) {
    static ClearanceCounts n;
    wide_clearance(c, 10, 0x71a844, &n);
    return 0;
}
int hos_site_wide_clearance_follow(Ctx *A64_NOESC c) {
    static ClearanceCounts n;
    wide_clearance(c, 14, 0x71b624, &n);
    return 0;
}

/* wide.hud (after the 2D canvas setup's ldp q1, q4, [x2]: the layout's projection rows; the canvas x0, its screen 0x1b8
   before it): the UI projection of every 2D screen not kept whole scaled across and up by game/hos_wide_ui.c's hos_ui_qx
   and hos_ui_qy for the picture's shape (before the first present: hos_wide_squeeze across), which also places the HUD's
   panes and fits the menus. At 16:9 hos_wide_squeeze is 0 and the projection is the game's */
int hos_site_wide_hud(Ctx *A64_NOESC c) {
    if (!(hos_wide_squeeze > 0.0f)) return 0;
    float m00, m11;
    memcpy(&m00, c->vreg[1], 4);
    memcpy(&m11, (const uint8_t *)c->vreg[4] + 4, 4);
    if (!(m00 == UI_M00 && m11 == UI_M11)) return 0;
    const uint64_t vt = recomp_load64(c, c->x[0] - 0x1b8);
    const uint32_t keep = vt == WIDE_KEEP_SHORTCUT_BG ? 1u : vt == WIDE_KEEP_COMMON ? 2u : vt == WIDE_KEEP_EVENT_FADE ? 4u
                        : vt == WIDE_KEEP_LOADING_FADE ? 8u : vt == WIDE_KEEP_SYSTEM_OVERLAY ? 16u : 0u;
    if (keep & hos_wide_keep_mask) return 0;
    float qx = hos_ui_qx, qy = hos_ui_qy, row[4];
    if (!(qx > 0.0f)) qx = hos_wide_squeeze, qy = 1.0f;
    if (qx != 1.0f) {
        memcpy(row, c->vreg[1], 16);
        for (int k = 0; k < 4; k++) row[k] *= qx;
        memcpy(c->vreg[1], row, 16);
    }
    if (qy > 0.0f && qy != 1.0f) {
        memcpy(row, c->vreg[4], 16);
        for (int k = 0; k < 4; k++) row[k] *= qy;
        memcpy(c->vreg[4], row, 16);
    }
    return 0;
}

/* wide.bubble (after fmul s1, s2, s1: y * 1080; x * 1920 already in s0): the speech bubble over a talking NPC
   (BalloonMessage_00) is anchored at the speaker's place on the screen times the canvas's half size, but its screen draws
   through the HUD's scaled projection: divided by hos_ui_qx and hos_ui_qy, the bubble lands over its speaker. (The site is
   the ultrawide IPS mod's, there 1920 / squeeze in place of 1920; y too here, as the HUD is drawn smaller on a screen
   narrower than 16:9.) Only the low 32 bits of s0 and s1 are written */
int hos_site_wide_bubble(Ctx *A64_NOESC c) {
    const float qx = hos_ui_qx, qy = hos_ui_qy;
    if (qx > 0.0f && qy > 0.0f) {
        float x, y;
        memcpy(&x, c->vreg[0], 4);
        memcpy(&y, c->vreg[1], 4);
        x /= qx;
        y /= qy;
        memcpy(c->vreg[0], &x, 4);
        memcpy(c->vreg[1], &y, 4);
    }
    return 0;
}

/* wide.movie (after the movie layer's draw loads ldp x9, x8, [x19, #0x18]): the movie kept at 16:9. Its viewport is at
   x29 - 40, its float bounds left, top, right, bottom at +8, +12, +16, +20. A wider picture: the left and right bounds
   narrowed to the middle 16:9; a narrower one (by more than 1%): the top and bottom moved in by as much, bars above and
   below */
int hos_site_wide_movie(Ctx *A64_NOESC c) {
    const float m = hos_wide_margin;
    if (m > 0.0f) {
        const uint64_t vp = c->x[29] - 40;
        uint32_t ua = (uint32_t)recomp_load32(c, vp + 8), ub = (uint32_t)recomp_load32(c, vp + 16);
        float a, b;
        memcpy(&a, &ua, 4);
        memcpy(&b, &ub, 4);
        const float in = (b - a) * m;
        a += in;
        b -= in;
        memcpy(&ua, &a, 4);
        memcpy(&ub, &b, 4);
        recomp_store32(c, vp + 8, ua);
        recomp_store32(c, vp + 16, ub);
        return 0;
    }
    const float r = hos_wide_ratio, game = 16.0f / 9.0f;
    if (r > 0.0f && r < game * 0.99f) {
        const uint64_t vp = c->x[29] - 40;
        uint32_t ua = (uint32_t)recomp_load32(c, vp + 12), ub = (uint32_t)recomp_load32(c, vp + 20);
        float a, b;
        memcpy(&a, &ua, 4);
        memcpy(&b, &ub, 4);
        const float in = (b - a) * (1.0f - r / game) / 2.0f;
        a += in;
        b -= in;
        memcpy(&ua, &a, 4);
        memcpy(&ub, &b, 4);
        recomp_store32(c, vp + 12, ua);
        recomp_store32(c, vp + 20, ub);
    }
    return 0;
}

/* wide.savePicture (after the viewport helper's ldr x8, [x8], before its blr x8: SetViewport(x, y, w, h) in w1-w4): the
   pause menu draws the whole picture into a 543 x 306 texture, the save's JPEG, through the game's viewport-and-scissor
   helper (f_802a46e0). Wider than 16:9, that one viewport is made wider than the texture by the picture's ratio over 16:9,
   centred: the middle 16:9 fills the texture (the scissor keeps the texture's), as a 16:9 window would show. Narrower than
   16:9 (4:3, 16:10) it is made taller instead, by 16:9 over the picture's ratio: the picture's middle band fills the
   texture at its own shape, neither squeezed nor with bars */
int hos_site_wide_save_picture(Ctx *A64_NOESC c) {
    const float r = hos_wide_ratio;
    if (r > 0.0f && (uint32_t)c->x[1] == 0 && (uint32_t)c->x[2] == 0 && (uint32_t)c->x[3] == 543 && (uint32_t)c->x[4] == 306) {
        if (r > SIXTEEN_NINE) {
            const int w = (int)(543.0f * r / SIXTEEN_NINE + 0.5f);
            c->x[1] = (uint32_t)((543 - w) / 2);
            c->x[3] = (uint32_t)w;
        } else {
            const int h = (int)(306.0f * SIXTEEN_NINE / r + 0.5f);
            c->x[2] = (uint32_t)((306 - h) / 2);
            c->x[4] = (uint32_t)h;
        }
    }
    return 0;
}

/* The HUD's layout for the screen's shape (game/hos_wide_ui.c). A 2D layout pane's final transform is made by f_8154cab0
   when its dirty bit 6 is set (f_8154caa0 checks it and falls into it); the pane is x0 there and x19 in it. The parent it
   multiplies by is x0 after +0x334; +0x2f8 is where every path without one joins. The translation is stored at +0x3c4,
   before the stored transform is read back to tell whether the children need theirs remade. f_8154ac30 reads a pane's
   size as drawn (ldp s0, s1, [x0, #0x58]). f_8154a730, the calculate pass, skips a pane with no dirty bits at all. A
   picture's draw: f_815505b0 packs its record into the frame's constants (read into x8, x9 at +0x144), the drawer keeps
   them at +0x48; f_81550720 draws with them. The pause menu's background is drawn in pieces there (Ultrawide menus:
   Extended). f_81545b60 builds a layout's panes (the layout x19 after its call of f_81545d60) */

/* ui.calcDirty (after the calculate pass's first look at the dirty bits, ldr w8, [x0]): the calculate pass returns at once
   for a pane with no dirty bits, so a pane that never changes would never remake its transform. While forced its bit 6
   is set where the pass reads the bits, so it reaches f_8154caa0 */
int hos_site_ui_calc_dirty(Ctx *A64_NOESC c) {
    if (hos_wide_ui_force) {
        c->x[8] |= 0x40;
        recomp_store32(c, c->x[0], (uint32_t)c->x[8]);
    }
    return 0;
}

/* ui.paneDirty (after ldr w8, [x0]: the pane's dirty bits): every pane remakes its transform while forced */
int hos_site_ui_pane_dirty(Ctx *A64_NOESC c) {
    if (hos_wide_ui_force) c->x[8] |= 0x40;
    return 0;
}

/* ui.paneNoParent (after ldp q0, q1, [sp, #0x40]: every path without a parent joins) and ui.paneParent (after
   ldr w8, [x0, #0x30]: the parent's transform, the parent in x0) */
int hos_site_ui_pane_no_parent(Ctx *A64_NOESC c) {
    (void)c;
    hos_wide_ui_parent(0);
    return 0;
}
int hos_site_ui_pane_parent(Ctx *A64_NOESC c) {
    hos_wide_ui_parent(c->x[0]);
    return 0;
}

/* ui.panePlaced (after stur d3, [x19, #0x4c]: the translation stored): the HUD's side panes at the screen's edges */
int hos_site_ui_pane_placed(Ctx *A64_NOESC c) {
    hos_wide_ui_placed(c, c->x[19]);
    return 0;
}

/* ui.paneSize (after ldp s0, s1, [x0, #0x58]: the size as drawn): spanning panes the screen's width */
int hos_site_ui_pane_size(Ctx *A64_NOESC c) {
    hos_wide_ui_size(c, c->x[0]);
    return 0;
}

/* ui.pictureConstants (after ldp x8, x9, [sp, #8]: a picture's constants packed, before the drawer keeps them) and
   ui.pictureDraw (after mov x22, x0: a picture drawn, the drawer x0, x1 what it draws with): the pause menu's background
   in pieces */
int hos_site_ui_picture_constants(Ctx *A64_NOESC c) {
    hos_wide_ui_bg(c);
    return 0;
}
int hos_site_ui_picture_draw(Ctx *A64_NOESC c) {
    hos_wide_ui_bg_draw(c);
    return 0;
}

/* ui.titlePrepare (replace bl f_8154f1c0, x30 set: a picture's record packed; x0 the record, x1 the arena, x2 the
   alignment, x8 where the constants go, x19 the drawer, x20 its pane), ui.titleDraw (the entry of f_81550720: x0 the
   drawer, x1 what it draws with; non-zero: drawn) and ui.titleReset (the entry of f_8156abc0: x0 the arena reset): the
   area title card's frame in three strips (game/hos_wide_ui.c). The draw draws the picture once per strip through the
   function itself, whose own entry hook then lets the game draw it (title_drawing: that picture, on this thread) */
int hos_site_ui_title_prepare(Ctx *A64_NOESC c) {
    hos_wide_title_prepare(c, c->x[19], c->x[20], c->x[0], c->x[1], c->x[2], c->x[8], a64_lookup(hos_site_callee));
    return 0;
}
static _Thread_local uint64_t title_drawing;
static void title_draw_game(Ctx *A64_NOESC c) {
    const uint64_t outer = title_drawing;
    title_drawing = c->x[0];
    a64_lookup(WIDE_PICTURE_DRAW)(c);
    title_drawing = outer;
}
int hos_site_ui_title_draw(Ctx *A64_NOESC c) {
    if (title_drawing && title_drawing == c->x[0]) return 0;
    return hos_wide_title_draw(c, title_draw_game);
}
int hos_site_ui_title_reset(Ctx *A64_NOESC c) {
    hos_wide_title_reset(c->x[0]);
    return 0;
}

/* ui.layoutBuilt (after add x0, x19, #0x10: a layout's panes built, f_81545d60 returned): a list widened as it is built */
int hos_site_ui_layout_built(Ctx *A64_NOESC c) {
    hos_wide_ui_built(c, c->x[19]);
    return 0;
}
