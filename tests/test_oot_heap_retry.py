"""The OptimizeChangeBank thread's allocations are tried again when their heap is full (game/oot_heap_retry.c, the hook sites heap.*).

Issue 1697 (thread OptimizeChangeBanc writes guest 0x8 a few frames after loading a save), TomShafford/Hookshot's crash at
backflip + attack, issue 2437 (the same thread writes guest 0x20, lr 0x8044f684, crossing Gerudo Valley's bridge) and the
crash reports that end with a buffer made in the parent and a crash at the build's next allocation: a model's CPU
buffer, one of its 0x238-byte objects, that object's buffers or f_80443000's blocks (a heap's tryAlloc) or its GPU memory
(f_80522050) came back null from a full heap and the game wrote through it. Builds game/oot_heap_retry.c, as it is,
over a guest memory of its own with made-up sead heaps (name, size, parent, a vtable with tryAlloc and getFreeSize) and
a stand-in for the game's f_80522050, then checks:

- an allocation that succeeds is the game's alone: one call, nothing logged, nothing changed;
- a GPU allocation that fails is tried again in each parent with room, then in the game's default heap (no waits:
  it never sleeps); the request names its own heap again afterwards; nothing with room: it fails as before;
- a CPU allocation is tried in each parent with room, returned in x0, the other registers and the flags as they were,
  each of the seven places counted under its own name; every heap method called through game_call_more;
- one that came from a parent hands the rest of its build to that parent: the heaps' holder names the parent, the
  build's own heap is what f_804566e0's two reads of its heap get, and the second puts it back in the holder
  (HOS_GPU_ALLOC_REST=0 or a holder naming another heap: that allocation alone); a parent too small for the whole build
  is skipped; a GPU heap that is the same heap goes and comes back with it; a holder on a stack below the image
  (sead::MainThread's) counts;
- HOS_GPU_ALLOC_RETRY=0: the game's call alone, as shipped; HOS_GPU_ALLOC_TEST_FAIL=<n>: the first n refused by the named
  heap without calling it (nothing leaks), and the retry makes them; <gpu>,<cpu>,<objects>: each kind its own count;
  HOS_GPU_ALLOC_TEST_FILL=<n>[,<place>]: the first n heaps at the place (the object's when none is named) are emptied
  through their own tryAlloc, once each, and never the parent a build was handed;
- the eleven places are hook sites (game/sites/sites.toml, game/oot_sites.c's callbacks: nine replace sites, the two reads
  before sites): built as the lift lowers them (tests/sitehooks.py) and, side by side, as the text patch they replaced had
  them (OLD_*, nx2/oot_patches.py on main), over stand-ins that log each call to the host and the game: the same calls
  with the same arguments (the heaps' holder read where each function keeps it) and the same registers, with the test
  switches off, on and refusing, for a null or not (x30 aside where the patch made no call under a refusal: the hook's
  x30 is the return address, as a call leaves it).

Transition check: main's text-patch C kept here as the oracle shows the sites do what it did; remove it once a
deliberate behaviour change needs it.

python -I -B tests/test_oot_heap_retry.py [--cc clang] [--cargo cargo]
"""
import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

ROOT = Path(__file__).resolve().parents[1]

# (env_on, the switches' rule, before the code cut out of the sources: gpu/vk/compat/longshot_env.h)
ENV_ON_H = str(Path(__file__).resolve().parents[1] / "gpu" / "vk" / "compat" / "longshot_env.h")
sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves this folder off the path)
from sitecheck import site, callback, slot  # noqa: E402
import sitehooks  # noqa: E402

# (the calls as the lift writes them, and as the text patch had them on main)
GPU_CALL = "c->x[30] = %#xULL; f_80522050(c);"
OLD_GPU = "c->x[30] = %#xULL; hos_oot_gpu_alloc(c, f_80522050);   /* (nx2/oot_patches.py: game/oot_heap_retry.c) */"
CPU_CALL = "{ uint64_t _t = c->x[8]; c->x[30] = %#xULL; A64_ICALL(c, _t); }\n"
OLD_CPU = ("{ const uint64_t _hh = c->x[0], _hn = c->x[1], _ha = c->x[2], _ho = %s;   /* (nx2/oot_patches.py: game/oot_heap_retry.c) */\n"
           "  if (hos_oot_heap_test_enabled && hos_oot_heap_refuse(c, _hh, _ho, %#xULL)) c->x[0] = 0;\n"
           "  else { uint64_t _t = c->x[8]; c->x[30] = %#xULL; A64_ICALL(c, _t); }\n"
           "  if (!c->x[0]) hos_oot_heap_retry(c, _hh, _hn, _ha, %#xULL, _ho); }\n")
OLD_REST = ("    hos_oot_heap_rest_%s(c, recomp_load64(c, c->x[31] + 56));   /* (nx2/oot_patches.py: game/oot_heap_retry.c) */\n"
            + CPU_CALL)
SP56, SP0 = "recomp_load64(c, c->x[31] + 56)", "recomp_load64(c, c->x[31] + 0)"
# (name, function, its address, the site's offset, its kind, the holder as main's patch read it)
SITES = (("heap.cpuAlloc", "f_804566e0", 0x804566e0, 0x22c, "cpu", SP56),
         ("heap.cpuAllocOther", "f_804566e0", 0x804566e0, 0x6c0, "cpu", SP56),
         ("heap.objectAlloc", "f_8044f420", 0x8044f420, 0x24c, "cpu", "c->x[21]"),
         ("heap.objectBuffer", "f_8044ed60", 0x8044ed60, 0x264, "cpu", "c->x[22]"),
         ("heap.objectBufferOther", "f_8044ed60", 0x8044ed60, 0x310, "cpu", "c->x[22]"),
         ("heap.blockBuffer", "f_80443000", 0x80443000, 0x1fc, "cpu", SP0),
         ("heap.blockObject", "f_80443000", 0x80443000, 0xe2c, "cpu", SP0),
         ("heap.restOwn", "f_804566e0", 0x804566e0, 0x248, "own", None),
         ("heap.restEnd", "f_804566e0", 0x804566e0, 0xaa8, "end", None),
         ("heap.gpuAlloc", "f_804566e0", 0x804566e0, 0x838, "gpu", None),
         ("heap.gpuAllocOther", "f_804566e0", 0x804566e0, 0x9b4, "gpu", None))
SIDE = r"""
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#define A64_NOESC
typedef struct { uint64_t x[32]; uint8_t n, z, c, v; } Ctx;
typedef void (*A64Fn)(Ctx *);
#define SP 0x80070000ull   /* (the stack: the heaps' holder at +0 and +56, as f_80443000 and f_804566e0 keep it) */
static unsigned char stack_[128];
static inline uint64_t recomp_load64(Ctx *c, uint64_t a) {
    uint64_t v = 0;
    (void)c;
    if (a >= SP && a + 8 <= SP + sizeof stack_) memcpy(&v, stack_ + (a - SP), 8);
    return v;
}
static uint64_t log_[96];
static int logged, refuse, null_result;
int hos_oot_heap_test_enabled;
static void note(uint64_t a, uint64_t b, uint64_t c, uint64_t d) {
    if (logged < 92) log_[logged++] = a, log_[logged++] = b, log_[logged++] = c, log_[logged++] = d;
}
void f_80522050(Ctx *c) { note(1, c->x[1], c->x[30], 0); c->x[0] = !null_result; c->x[9] = 0xdead; }
static void try_alloc(Ctx *c) { note(2, c->x[0], c->x[1], c->x[30]); c->x[0] = null_result ? 0 : 0x80080000ull; c->x[3] = 0xdead; }
/* (the method a rest callback points x8 at: the call after it must be this one, as main's patch read x8 after the call) */
static void own_free(Ctx *c) { note(10, c->x[0], c->x[30], 0); c->x[0] = 0x4000; }
A64Fn a64_lookup(uint64_t a) {
    return a == 0x80522050ull ? f_80522050 : a == 0x80002000ull ? try_alloc : a == 0x80003000ull ? own_free : NULL;
}
_Thread_local uint64_t hos_site_callee;   /* (game/sites.c's: a replaced bl's callee, which its hook stores) */
void a64_icall(Ctx *c, uint64_t a) { a64_lookup(a)(c); }
#define A64_ICALL(c, t) a64_icall(c, t)
void hos_oot_gpu_alloc(Ctx *c, A64Fn game) { note(3, c->x[1], c->x[30], 0); if (!refuse) game(c); else c->x[0] = 0; }
int hos_oot_heap_refuse(Ctx *c, uint64_t heap, uint64_t holder, uint64_t lr) {
    note(4, heap, holder, lr), note(7, c->x[1], c->x[2], 0);   /* (x30 aside: the patch set it after this call) */
    return refuse;
}
void hos_oot_heap_retry(Ctx *c, uint64_t heap, uint64_t size, uint64_t align, uint64_t lr, uint64_t holder) {
    note(5, heap, size, align), note(6, lr, holder, 0);
    c->x[0] = 0x80090000ull;
}
void hos_oot_heap_rest_own(Ctx *c, uint64_t holder) { note(8, holder, c->x[0], c->x[8]); c->x[0] = 0x80055500ull; c->x[8] = 0x80003000ull; }
void hos_oot_heap_rest_end(Ctx *c, uint64_t holder) { note(9, holder, c->x[0], c->x[8]); c->x[0] = 0x80055600ull; c->x[8] = 0x80003000ull; }
@CALLBACKS@
@FUNCTIONS@
typedef void (*Fn)(Ctx *);
int main(void) {
    static const Fn hooked[] = {@HOOKED@}, patched[] = {@PATCHED@};
    static const int cpu[] = {@CPU@};
    int fails = 0;
    const uint64_t at0 = 0x80050000ull, at56 = 0x80050100ull;
    memcpy(stack_, &at0, 8), memcpy(stack_ + 56, &at56, 8);
    for (unsigned k = 0; k < sizeof hooked / sizeof hooked[0]; k++)
        for (int mode = 0; mode < 3; mode++)   /* (test switches off; on, not refusing; refusing) */
            for (null_result = 0; null_result < 2; null_result++) {
                hos_oot_heap_test_enabled = mode > 0, refuse = mode == 2;
                Ctx a, b;
                memset(&a, 0, sizeof a);
                for (int r = 0; r < 31; r++) a.x[r] = 0x9e3779b97f4a7c15ull * (r + 1);
                a.x[8] = 0x80002000ull, a.x[31] = SP, a.n = 1, a.v = 1;
                b = a;
                uint64_t la[96];
                int na;
                logged = 0, memset(log_, 0, sizeof log_);
                hooked[k](&a);
                memcpy(la, log_, sizeof la), na = logged;
                logged = 0, memset(log_, 0, sizeof log_);
                patched[k](&b);
                if (refuse && cpu[k]) b.x[30] = a.x[30];   /* (refused: the patch made no call, the hook's x30 is a call's) */
                if (na != logged || memcmp(la, log_, sizeof la) || memcmp(&a, &b, sizeof a)) {
                    printf("FAIL: call %u, mode %d, null %d\n", k, mode, null_result);
                    fails++;
                }
            }
    if (!fails) printf("same\n");
    return fails != 0;
}
"""

# (host/game_call.h for the stand-in Ctx: the fixture's game_call_more calls through a64_lookup as host/game_call.c does,
# x0.. the arguments, LR as given (else 0), the whole context put back)
GAME_H = r'''#pragma once
#include <stdint.h>
typedef struct { int set_x8; uint64_t x8; int set_lr; uint64_t lr; float *s0, *s1; } GameCallMore;
int game_has(uint64_t fn);
int game_call_more(Ctx *c, uint64_t fn, uint64_t sp, int nargs, const uint64_t *args, uint64_t *ret, const GameCallMore *more);
'''

HOS_H = r'''#pragma once
#include <stdint.h>
#include <string.h>
typedef struct { uint64_t x[32]; uint8_t n, z, c, v; } Ctx;
typedef void (*A64Fn)(Ctx *);
#define GUEST 0x80000000ull
#define LOW 0x10ff000ull   /* (a page below the image: sead::MainThread's stack is there) */
extern unsigned char mem[0x100000], low[0x1000];
static inline unsigned char *guest(uint64_t a) { return a < GUEST ? low + (a - LOW) : mem + (a - GUEST); }
static inline uint64_t recomp_load8(Ctx *c, uint64_t a) { (void)c; return *guest(a); }
static inline uint64_t recomp_load64(Ctx *c, uint64_t a) { uint64_t v; (void)c; memcpy(&v, guest(a), 8); return v; }
static inline void recomp_store64(Ctx *c, uint64_t a, uint64_t v) { (void)c; memcpy(guest(a), &v, 8); }
A64Fn a64_lookup(uint64_t a);
void a64_icall(Ctx *c, uint64_t a);
void hos_log(const char *fmt, ...);
void hos_sleep_ns(int64_t ns);
'''

FIXTURE = r'''#include "hos.h"
#include "game_call.h"
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL:%d %s\n", __LINE__, #x); exit(1); } } while (0)
unsigned char mem[0x100000], low[0x1000];
static int logs, sleeps, game_calls;
static char said[8192];   /* (what was logged since it was last emptied) */
void hos_log(const char *fmt, ...) {
    char line[512];
    va_list a;
    va_start(a, fmt), vsnprintf(line, sizeof line, fmt, a), va_end(a);
    puts(line);
    logs++;
    if (strlen(said) + strlen(line) + 2 < sizeof said) strcat(said, line), strcat(said, "\n");
}
void hos_sleep_ns(int64_t ns) { (void)ns; sleeps++; }

/* heaps: at H(i), +0 the vtable, +0x20 its name, +0x30 its size, +0x38 its parent; the room left in room[i] */
#define VT 0x80001000ull
#define FN_TRY 0x80002000ull
#define FN_FREE 0x80002100ull
#define H(i) (0x80010000ull + 0x100ull * (i))
#define NAME(i) (0x80020000ull + 0x40ull * (i))
static uint64_t room[8], next_block = 0x80080000ull, free_after_calls[8];
static uint64_t heap_index(uint64_t h) { return (h - H(0)) / 0x100; }
/* (host/game_call.c's call over this context; in_call while the callee runs, and each method reached any other way, or
   with another LR than the failed call's, counted in outside; game_try_alloc below stands for the game's own call) */
static int in_call, outside, direct;
static uint64_t want_lr, free_lr;
int game_has(uint64_t fn) { return fn && a64_lookup(fn) != NULL; }
int game_call_more(Ctx *c, uint64_t fn, uint64_t sp, int nargs, const uint64_t *args, uint64_t *ret, const GameCallMore *more) {
    const A64Fn f = fn ? a64_lookup(fn) : NULL;
    if (!f || nargs < 0 || nargs > 8 || (nargs && !args)) return 0;
    const Ctx saved = *c;
    if (sp) c->x[31] = sp & ~0xfULL;
    for (int i = 0; i < nargs; i++) c->x[i] = args[i];
    if (more && more->set_x8) c->x[8] = more->x8;
    c->x[30] = more && more->set_lr ? more->lr : 0;
    in_call++, f(c), in_call--;
    if (ret) *ret = c->x[0];
    *c = saved;
    return 1;
}
static void try_alloc(Ctx *c) {   /* tryAlloc(heap, size, align): the block or 0 */
    if (!direct && (!in_call || (want_lr && c->x[30] != want_lr))) outside++;
    const uint64_t i = heap_index(c->x[0]), size = c->x[1];
    c->x[1] = c->x[2] = c->x[3] = 0xdead;   /* (a callee's scratch registers) */
    c->n = c->z = c->c = c->v = 1;
    if (room[i] < size) { c->x[0] = 0; return; }
    room[i] -= size;
    c->x[0] = next_block, next_block += (size + 15) & ~15ull;
}
static void game_try_alloc(Ctx *c) { direct = 1, try_alloc(c), direct = 0; }   /* (the game's own call of it) */
static void get_free(Ctx *c) {   /* (its LR: the context's own, as getFreeSize is asked; in free_lr) */
    if (!in_call) outside++;
    free_lr = c->x[30];
    c->x[0] = room[heap_index(c->x[0])]; c->x[5] = 0xdead;
}
A64Fn a64_lookup(uint64_t a) { return a == FN_TRY ? try_alloc : a == FN_FREE ? get_free : NULL; }
void a64_icall(Ctx *c, uint64_t a) { a64_lookup(a)(c); }
static void make_heap(int i, int parent, uint64_t free_room, const char *name) {
    recomp_store64(NULL, H(i), VT);
    strcpy((char *)mem + (NAME(i) - GUEST), name);
    recomp_store64(NULL, H(i) + 0x20, NAME(i));
    recomp_store64(NULL, H(i) + 0x30, 0x100000);
    recomp_store64(NULL, H(i) + 0x38, parent < 0 ? 0 : H(parent));
    room[i] = free_room;
}

/* the game's f_80522050: x1 the request (+0 count, +8 the heap, 0: the default, heap 3), x2 the size of one; x0 bit 0 */
static uint64_t default_heap = H(3);
static void game_gpu_alloc(Ctx *c) {
    game_calls++;
    uint64_t heap = recomp_load64(c, c->x[1] + 8);
    if (!heap) heap = default_heap;
    const uint64_t i = heap_index(heap), size = recomp_load64(c, c->x[1]) * c->x[2];
    if (free_after_calls[i] && game_calls >= (int)free_after_calls[i]) room[i] += size;   /* (a free landed) */
    c->x[0] = room[i] >= size ? (room[i] -= size, 1) : 0;
    c->x[9] = 0xdead;
}
void hos_oot_gpu_alloc(Ctx *c, void (*game)(Ctx *));
int hos_oot_heap_refuse(Ctx *c, uint64_t heap, uint64_t holder, uint64_t lr);
extern int hos_oot_heap_test_enabled;
void hos_oot_heap_test_init(void);
void hos_oot_heap_retry(Ctx *c, uint64_t heap, uint64_t size, uint64_t align, uint64_t lr, uint64_t holder);
void hos_oot_heap_rest_own(Ctx *c, uint64_t holder);
void hos_oot_heap_rest_end(Ctx *c, uint64_t holder);
#define BUFFER 0x80456910ull    /* (the places' return addresses: the +200 buffer, the 0x238-byte object, its second */
#define OBJECT 0x8044f670ull    /*  buffer, f_80443000's 0x310-byte object) */
#define OBJECT_BUFFER 0x8044f074ull
#define BLOCK 0x80443e30ull
/* the heaps' holder of a build: +8 the heap its allocations name (f_804558a0's four words on its stack) */
#define HOLDER 0x80050000ull
static void hold(uint64_t heap) { recomp_store64(NULL, HOLDER + 8, heap), recomp_store64(NULL, HOLDER + 16, H(3)); }
static uint64_t held(void) { return recomp_load64(NULL, HOLDER + 8); }
static uint64_t held_gpu(void) { return recomp_load64(NULL, HOLDER + 16); }   /* (the GPU heap: another one unless set) */

#define REQ 0x80030000ull
static void request(Ctx *c, uint64_t heap, uint64_t count) {
    memset(c, 0, sizeof *c);
    recomp_store64(c, REQ, count);
    recomp_store64(c, REQ + 8, heap);
    recomp_store64(c, REQ + 0x20, NAME(7));
    c->x[0] = 0x80040000ull, c->x[1] = REQ, c->x[2] = 0x100, c->x[30] = 0x80456f1cull;
    game_calls = logs = sleeps = 0;
}
static void reset(void) {
    memset(free_after_calls, 0, sizeof free_after_calls);
    make_heap(0, 1, 0, "OptimizeChangeBank");   /* (the named heap; its parent 1, whose parent is the root 2) */
    make_heap(1, 2, 0, "Model");
    make_heap(2, -1, 0, "Root");
    make_heap(3, -1, 0, "Default");
    make_heap(4, 1, 0, "Model/Other");
}
int main(int argc, char **argv) {
    Ctx c;
    recomp_store64(NULL, VT + 0x30, FN_TRY);
    recomp_store64(NULL, VT + 0x80, FN_FREE);
    strcpy((char *)mem + (NAME(7) - GUEST), "gsys::ModelResource(MatBlock)");
    const char *mode = argc > 1 ? argv[1] : "";
    hos_oot_heap_test_init();
    CHECK(hos_oot_heap_test_enabled == (!strcmp(mode, "refuse") || !strcmp(mode, "off-refuse") ||
          !strcmp(mode, "split") || !strcmp(mode, "object") || !strcmp(mode, "fill") ||
          !strcmp(mode, "fill-at") || !strcmp(mode, "blocks")));

    if (!strcmp(mode, "off")) {   /* HOS_GPU_ALLOC_RETRY=0: the game's call alone */
        reset(), room[1] = 1 << 20;
        request(&c, H(0), 4);
        hos_oot_gpu_alloc(&c, game_gpu_alloc);
        CHECK(!(c.x[0] & 1) && game_calls == 1 && !logs);
        c.x[0] = 0;
        hos_oot_heap_retry(&c, H(0), 0x4198, 8, 0x80456910ull, 0);
        CHECK(c.x[0] == 0 && room[1] == 1 << 20);
        hold(H(0));
        hos_oot_heap_retry(&c, H(0), 0x238, 8, 0x8044f670ull, HOLDER);
        CHECK(c.x[0] == 0 && room[1] == 1 << 20 && held() == H(0));
        CHECK(!hos_oot_heap_refuse(&c, H(0), 0, BUFFER) && !hos_oot_heap_refuse(&c, H(0), HOLDER, OBJECT) && room[0] == 0);
        puts("PASS: HOS_GPU_ALLOC_RETRY=0: the game's own call alone");
        return 0;
    }
    if (!strcmp(mode, "off-refuse")) {   /* HOS_GPU_ALLOC_RETRY=0 HOS_GPU_ALLOC_TEST_FAIL=1: the full heap's failure, as shipped */
        reset(), room[0] = room[1] = 1 << 20;
        request(&c, H(0), 4);
        hos_oot_gpu_alloc(&c, game_gpu_alloc);
        CHECK(!(c.x[0] & 1) && game_calls == 0 && room[0] == 1 << 20 && room[1] == 1 << 20);
        CHECK(hos_oot_heap_refuse(&c, H(0), 0, BUFFER) && !hos_oot_heap_refuse(&c, H(0), 0, BUFFER));
        c.x[0] = 0;
        hos_oot_heap_retry(&c, H(0), 0x4198, 8, 0x80456910ull, 0);
        CHECK(c.x[0] == 0 && room[1] == 1 << 20);
        puts("PASS: HOS_GPU_ALLOC_RETRY=0 with HOS_GPU_ALLOC_TEST_FAIL=1: the failure the game crashes on, not tried again");
        return 0;
    }
    if (!strcmp(mode, "refuse")) {   /* HOS_GPU_ALLOC_TEST_FAIL=2 */
        reset(), room[0] = room[1] = 1 << 20;
        for (int k = 0; k < 3; k++) {
            request(&c, H(0), 4);
            const uint64_t named = room[0], parent = room[1];
            hos_oot_gpu_alloc(&c, game_gpu_alloc);
            CHECK(c.x[0] & 1);
            if (k < 2) CHECK(room[0] == named && room[1] == parent - 0x400 && !sleeps);   /* (refused: made in the parent) */
            else CHECK(room[0] == named - 0x400 && game_calls == 1);                 /* (then the game's own again) */
        }
        CHECK(hos_oot_heap_refuse(&c, H(0), 0, BUFFER) && hos_oot_heap_refuse(&c, H(0), 0, BUFFER) && !hos_oot_heap_refuse(&c, H(0), 0, BUFFER));
        hold(H(0));
        CHECK(hos_oot_heap_refuse(&c, H(0), HOLDER, OBJECT) && hos_oot_heap_refuse(&c, H(0), HOLDER, OBJECT) &&
              !hos_oot_heap_refuse(&c, H(0), HOLDER, OBJECT));
        CHECK(room[0] == (1 << 20) - 0x400);   /* (refused without a call: nothing taken from it) */
        puts("PASS: HOS_GPU_ALLOC_TEST_FAIL=2: two of each kind refused by their heap without a call, made by the retry, then as shipped");
        return 0;
    }
    if (!strcmp(mode, "split")) {   /* HOS_GPU_ALLOC_TEST_FAIL=0,1: the model buffer alone */
        reset(), room[0] = 1 << 20;
        request(&c, H(0), 4);
        hos_oot_gpu_alloc(&c, game_gpu_alloc);
        CHECK((c.x[0] & 1) && game_calls == 1 && !logs);
        CHECK(hos_oot_heap_refuse(&c, H(0), 0, BUFFER) && !hos_oot_heap_refuse(&c, H(0), 0, BUFFER));
        CHECK(!hos_oot_heap_refuse(&c, H(0), HOLDER, OBJECT));
        puts("PASS: HOS_GPU_ALLOC_TEST_FAIL=0,1: the GPU memory as shipped, one model buffer refused, no object");
        return 0;
    }
    if (!strcmp(mode, "object")) {   /* HOS_GPU_ALLOC_TEST_FAIL=0,0,1: the 0x238-byte object alone, made by the retry */
        reset(), room[0] = 1 << 20, room[1] = 4 << 20;
        request(&c, H(0), 4);
        hos_oot_gpu_alloc(&c, game_gpu_alloc);
        CHECK((c.x[0] & 1) && game_calls == 1 && !logs);
        CHECK(!hos_oot_heap_refuse(&c, H(0), 0, BUFFER));
        hold(H(0));
        CHECK(hos_oot_heap_refuse(&c, H(0), HOLDER, OBJECT) && !hos_oot_heap_refuse(&c, H(0), HOLDER, OBJECT));
        const uint64_t named = room[0];
        c.x[0] = 0;   /* (refused: the site made no call, the heap it names untouched) */
        hos_oot_heap_retry(&c, H(0), 0x238, 8, 0x8044f670ull, HOLDER);
        CHECK(c.x[0] >= 0x80080000ull && room[0] == named && room[1] == (4 << 20) - 0x238);
        CHECK(strstr(said, "oot: model object, 0x238 bytes from heap") && !strstr(said, "model buffer"));
        CHECK(held() == H(1) && strstr(said, "the rest of the build takes its memory"));
        puts("PASS: HOS_GPU_ALLOC_TEST_FAIL=0,0,1: the GPU memory and the buffers as shipped, one object refused and made in the parent");
        return 0;
    }
    if (!strcmp(mode, "rest-off")) {   /* HOS_GPU_ALLOC_REST=0: the object from the parent, the build left with its heap */
        reset(), room[1] = 4 << 20;
        memset(&c, 0, sizeof c), hold(H(0));
        hos_oot_heap_retry(&c, H(0), 0x238, 8, 0x8044f670ull, HOLDER);
        CHECK(c.x[0] >= 0x80080000ull && room[1] == (4 << 20) - 0x238 && held() == H(0) && !strstr(said, "the rest of"));
        c.x[0] = 0x1234, c.x[8] = 0x5678;
        hos_oot_heap_rest_end(&c, HOLDER);
        CHECK(c.x[0] == 0x1234 && c.x[8] == 0x5678 && held() == H(0));
        puts("PASS: HOS_GPU_ALLOC_REST=0: the object made in the parent, the holder left naming the full heap");
        return 0;
    }
    if (!strcmp(mode, "fill-at")) {   /* HOS_GPU_ALLOC_TEST_FILL=1,0x80443e2c: at f_80443000's object, not at f_8044f420's */
        reset(), room[0] = 0x5123, room[1] = 4 << 20;
        memset(&c, 0, sizeof c), hold(H(0));
        CHECK(!hos_oot_heap_refuse(&c, H(0), HOLDER, OBJECT) && room[0] == 0x5123 && !said[0]);
        CHECK(!hos_oot_heap_refuse(&c, H(0), HOLDER, BLOCK) && room[0] < 16 && strstr(said, "is full"));
        hos_oot_heap_retry(&c, H(0), 0x310, 8, BLOCK, HOLDER);
        CHECK(c.x[0] >= 0x80080000ull && held() == H(1));
        puts("PASS: HOS_GPU_ALLOC_TEST_FILL=1,0x80443e2c: the heap emptied at that place alone, and the build handed over there");
        return 0;
    }
    if (!strcmp(mode, "blocks")) {   /* HOS_GPU_ALLOC_TEST_FAIL=0,0,2: the third number is all the other CPU places' */
        reset(), room[0] = 1 << 20;
        memset(&c, 0, sizeof c), hold(H(0));
        CHECK(!hos_oot_heap_refuse(&c, H(0), HOLDER, BUFFER));
        CHECK(hos_oot_heap_refuse(&c, H(0), HOLDER, OBJECT_BUFFER) && hos_oot_heap_refuse(&c, H(0), HOLDER, BLOCK) &&
              !hos_oot_heap_refuse(&c, H(0), HOLDER, OBJECT) && room[0] == 1 << 20);
        puts("PASS: HOS_GPU_ALLOC_TEST_FAIL=0,0,2: two of the other CPU allocations refused, whichever places they are");
        return 0;
    }
    if (!strcmp(mode, "fill")) {   /* HOS_GPU_ALLOC_TEST_FILL=2: the first two heaps at the object's place emptied, once each */
        reset(), room[0] = 0x5123, room[1] = 4 << 20, room[4] = 0x900;
        memset(&c, 0, sizeof c), hold(H(0));
        for (int r = 0; r < 31; r++) c.x[r] = 0x1000 + r;
        c.n = 1, c.z = 0, c.c = 1, c.v = 0;
        const Ctx before = c;
        CHECK(!hos_oot_heap_refuse(&c, H(0), HOLDER, OBJECT));   /* (not refused: the heap's own tryAlloc says no next) */
        CHECK(room[0] < 16 && room[1] == 4 << 20 && strstr(said, "HOS_GPU_ALLOC_TEST_FILL: ") && strstr(said, "is full"));
        for (int r = 0; r < 31; r++) CHECK(c.x[r] == before.x[r]);
        CHECK(c.n == 1 && c.z == 0 && c.c == 1 && c.v == 0);
        c.x[0] = H(0), c.x[1] = 0x238, c.x[2] = 8, game_try_alloc(&c);
        CHECK(c.x[0] == 0);
        hos_oot_heap_retry(&c, H(0), 0x238, 8, 0x8044f670ull, HOLDER);   /* (the object, and the build's rest) */
        CHECK(c.x[0] >= 0x80080000ull && held() == H(1) && room[1] == (4 << 20) - 0x238);
        CHECK(!hos_oot_heap_refuse(&c, H(1), HOLDER, OBJECT) && room[1] == (4 << 20) - 0x238);   /* (the parent is not filled) */
        hos_oot_heap_rest_end(&c, HOLDER);
        CHECK(held() == H(0));
        room[0] = 0x900, said[0] = 0;   /* (a second time at the same heap: filled once only) */
        CHECK(!hos_oot_heap_refuse(&c, H(0), HOLDER, OBJECT) && room[0] == 0x900 && !said[0]);
        hold(H(4));                     /* (the second heap) */
        CHECK(!hos_oot_heap_refuse(&c, H(4), HOLDER, OBJECT) && room[4] < 16 && strstr(said, "Model/Other"));
        room[3] = 0x900, said[0] = 0, hold(H(3));   /* (n = 2: no third) */
        CHECK(!hos_oot_heap_refuse(&c, H(3), HOLDER, OBJECT) && room[3] == 0x900 && !said[0]);
        CHECK(!outside);   /* (the heaps emptied through game_call_more) */
        puts("PASS: HOS_GPU_ALLOC_TEST_FILL=2: two heaps emptied through their own tryAlloc, once each, the registers as "
             "they were; the parent a build was handed is left alone");
        return 0;
    }

    /* made at once: the game's alone */
    reset(), room[0] = 1 << 20;
    request(&c, H(0), 4);
    hos_oot_gpu_alloc(&c, game_gpu_alloc);
    CHECK((c.x[0] & 1) && game_calls == 1 && !logs && !sleeps && recomp_load64(&c, REQ + 8) == H(0));

    /* full, its parent with room: made there at once, no sleep */
    reset(), room[1] = 1 << 20;
    request(&c, H(0), 4);
    hos_oot_gpu_alloc(&c, game_gpu_alloc);
    CHECK((c.x[0] & 1) && game_calls == 2 && !sleeps && room[1] == (1 << 20) - 0x400 && recomp_load64(&c, REQ + 8) == H(0));

    /* full, its parent too, the root has room: made there; the request names its heap again */
    reset(), room[2] = 1 << 20;
    request(&c, H(0), 4);
    hos_oot_gpu_alloc(&c, game_gpu_alloc);
    CHECK((c.x[0] & 1) && room[2] == (1 << 20) - 0x400 && recomp_load64(&c, REQ + 8) == H(0));
    CHECK(c.x[30] == 0x80456f1cull);

    /* no parent with room: the game's default heap */
    reset(), room[3] = 1 << 20;
    request(&c, H(0), 4);
    hos_oot_gpu_alloc(&c, game_gpu_alloc);
    CHECK((c.x[0] & 1) && room[3] == (1 << 20) - 0x400 && recomp_load64(&c, REQ + 8) == H(0));

    /* nothing has room: it fails, as before, and says so */
    reset();
    request(&c, H(0), 4);
    hos_oot_gpu_alloc(&c, game_gpu_alloc);
    CHECK(!(c.x[0] & 1) && logs >= 3 && recomp_load64(&c, REQ + 8) == H(0));
    CHECK(!sleeps);
    puts("PASS: GPU memory from a full heap: in a parent with room, in the default heap, or not at all; never a sleep");

    /* the CPU buffer: in the nearest parent with room; x0 the block, everything else as it was */
    reset(), room[2] = 1 << 20;
    memset(&c, 0, sizeof c);
    for (int r = 1; r < 31; r++) c.x[r] = 0x1000 + r;
    c.n = 1, c.z = 0, c.c = 1, c.v = 0;
    Ctx before = c;
    want_lr = BUFFER;   /* (tryAlloc asked again with the failed call's return address) */
    hos_oot_heap_retry(&c, H(0), 0x4198, 8, 0x80456910ull, 0);
    CHECK(c.x[0] >= 0x80080000ull && room[2] == (1 << 20) - 0x4198);
    CHECK(free_lr == before.x[30]);   /* (getFreeSize with the context's own LR) */
    for (int r = 1; r < 31; r++) CHECK(c.x[r] == before.x[r]);
    CHECK(c.n == 1 && c.z == 0 && c.c == 1 && c.v == 0);
    reset(), room[1] = 0x4198 + 0x1000;   /* (a parent without the margin is passed over) */
    c = before;
    hos_oot_heap_retry(&c, H(0), 0x4198, 8, 0x80456910ull, 0);
    CHECK(c.x[0] == 0 && room[1] == 0x4198 + 0x1000);
    reset(), room[1] = 1 << 20;
    c = before;
    hos_oot_heap_retry(&c, H(0), 0x4198, 8, 0x80456910ull, 0);
    CHECK(c.x[0] && room[1] == (1 << 20) - 0x4198);
    reset(), room[1] = 1 << 20;   /* (the second site, the +168 buffer, the same way) */
    c = before;
    want_lr = 0x80456da4ull;
    hos_oot_heap_retry(&c, H(0), 0x800, 8, 0x80456da4ull, 0);
    CHECK(c.x[0] && room[1] == (1 << 20) - 0x800);
    CHECK(!hos_oot_heap_refuse(&c, H(0), 0, BUFFER));
    CHECK(strstr(said, "model buffer (0x8045690c)") && strstr(said, "model buffer (0x80456da0) 1 blocks"));
    CHECK(!strstr(said, "model object"));
    CHECK(!outside);   /* (every heap method called through game_call_more, with the failed call's LR where it is told) */
    want_lr = 0;
    puts("PASS: the model buffer from a full heap: made in its nearest parent with room, x0 alone changed");

    /* f_8044f420's object: counted at its own site; a holder naming another heap is left alone. */
    reset(), room[2] = 1 << 20;
    c = before, said[0] = 0, hold(0);
    hos_oot_heap_retry(&c, H(0), 0x238, 8, 0x8044f670ull, HOLDER);
    CHECK(c.x[0] >= 0x80080000ull && room[2] == (1 << 20) - 0x238);
    for (int r = 1; r < 31; r++) CHECK(c.x[r] == before.x[r]);
    CHECK(c.n == 1 && c.z == 0 && c.c == 1 && c.v == 0);
    CHECK(strstr(said, "oot: model object, 0x238 bytes from heap") && strstr(said, "model object (0x8044f66c) 1 blocks"));
    CHECK(!strstr(said, "model buffer") && held() == 0);
    reset(), room[1] = 0x238 + 0x1000;   /* (a parent without the margin is passed over; nothing else has room) */
    c = before, said[0] = 0;
    hos_oot_heap_retry(&c, H(0), 0x238, 8, 0x8044f670ull, HOLDER);
    CHECK(c.x[0] == 0 && room[1] == 0x238 + 0x1000 && strstr(said, "oot: model object: no heap had room") && held() == 0);
    CHECK(!hos_oot_heap_refuse(&c, H(0), HOLDER, OBJECT));
    puts("PASS: f_8044f420's object from a full heap: made in its nearest parent with room, counted as its own site");

    /* the rest of the build: a parent with room for the whole heap is named in the holder, so the build's next
       allocations, which read the heap there, are that parent's; x0 alone changed */
    reset(), room[1] = 0x4000, room[2] = 4 << 20;
    c = before, said[0] = 0, hold(H(0));
    hos_oot_heap_retry(&c, H(0), 0x238, 8, 0x8044f670ull, HOLDER);
    CHECK(c.x[0] >= 0x80080000ull && room[1] == 0x4000 && room[2] == (4 << 20) - 0x238 && held() == H(2) && held_gpu() == H(3));
    for (int r = 1; r < 31; r++) CHECK(c.x[r] == before.x[r]);
    CHECK(!strstr(said, "the GPU's too"));
    CHECK(strstr(said, "the rest of the build takes its memory from heap") && strstr(said, "\"Root\"") &&
          strstr(said, "(1 builds so far)"));
    c.x[0] = held(), c.x[1] = 0x60, c.x[2] = 8, game_try_alloc(&c);   /* (the game's next allocation, unchanged code) */
    CHECK(c.x[0] && room[2] == (4 << 20) - 0x238 - 0x60 && room[0] == 0);
    /* f_804566e0's last read: x0 the heap and x8 its getFreeSize, loaded through the holder, are its own heap's again */
    c = before, c.x[0] = H(2), c.x[8] = 0xbad;
    hos_oot_heap_rest_end(&c, HOLDER);
    CHECK(held() == H(0) && held_gpu() == H(3) && c.x[0] == H(0) && c.x[8] == FN_FREE);
    for (int r = 1; r < 31; r++) CHECK(r == 8 || c.x[r] == before.x[r]);
    /* No parent can support the hand-over: do not consume a block in the smaller one. */
    reset(), room[1] = 0x4000;
    c = before, hold(H(0));
    hos_oot_heap_retry(&c, H(0), 0x238, 8, OBJECT, HOLDER);
    CHECK(!c.x[0] && room[1] == 0x4000 && held() == H(0));
    c.x[0] = 0x1234, c.x[8] = 0x5678;   /* (once: a build that was not handed over is not touched) */
    hos_oot_heap_rest_end(&c, HOLDER);
    CHECK(held() == H(0) && c.x[0] == 0x1234 && c.x[8] == 0x5678);
    /* the parent runs out too: handed on to its parent, and the build's own heap is still the one put back */
    reset(), room[1] = 4 << 20, room[2] = 8 << 20;
    c = before, hold(H(0));
    hos_oot_heap_retry(&c, H(0), 0x238, 8, 0x8044f670ull, HOLDER);
    CHECK(held() == H(1));
    room[1] = 0;
    hos_oot_heap_retry(&c, H(1), 0x238, 8, 0x8044f670ull, HOLDER);
    CHECK(c.x[0] && held() == H(2));
    hos_oot_heap_rest_end(&c, HOLDER);
    CHECK(held() == H(0) && c.x[0] == H(0));
    /* a model whose GPU heap is its CPU heap (f_80522050 takes a small object from it first, unchecked): both handed
       over, both put back; handed on to the next parent together too */
    reset(), room[1] = 4 << 20, room[2] = 8 << 20;
    c = before, said[0] = 0, hold(H(0)), recomp_store64(NULL, HOLDER + 16, H(0));
    hos_oot_heap_retry(&c, H(0), 0x238, 8, 0x8044f670ull, HOLDER);
    CHECK(held() == H(1) && held_gpu() == H(1) && strstr(said, "takes its memory, the GPU's too, from heap"));
    room[1] = 0;
    hos_oot_heap_retry(&c, H(1), 0x238, 8, 0x8044f670ull, HOLDER);
    CHECK(held() == H(2) && held_gpu() == H(2));
    hos_oot_heap_rest_end(&c, HOLDER);
    CHECK(held() == H(0) && held_gpu() == H(0) && c.x[0] == H(0));
    /* a holder that names another heap (the game took the thread's heap: its +8 is 0) is left alone */
    reset(), room[1] = 4 << 20;
    c = before, hold(0);
    hos_oot_heap_retry(&c, H(0), 0x238, 8, 0x8044f670ull, HOLDER);
    CHECK(c.x[0] && held() == 0);
    c.x[0] = 0x1234, c.x[8] = 0x5678;
    hos_oot_heap_rest_end(&c, HOLDER);
    CHECK(held() == 0 && c.x[0] == 0x1234 && c.x[8] == 0x5678);
    /* a build that left its holder otherwise than it was handed (not seen in the game: one return): nothing put back */
    reset(), room[1] = 4 << 20;
    c = before, hold(H(0));
    hos_oot_heap_retry(&c, H(0), 0x238, 8, 0x8044f670ull, HOLDER);
    CHECK(held() == H(1));
    hold(H(3)), c.x[0] = 0x1234, c.x[8] = 0x5678;
    hos_oot_heap_rest_end(&c, HOLDER);
    CHECK(held() == H(3) && c.x[0] == 0x1234 && c.x[8] == 0x5678);
    /* the +200 buffer made in a parent hands the build over as well (the crash reports' case: the buffer made in the
       parent, the crash at the build's next allocation): f_804566e0's first read of its heap, just after, gets the
       build's own with the holder left naming the parent, its last read puts it back */
    reset(), room[1] = 4 << 20;
    c = before, said[0] = 0, hold(H(0));
    hos_oot_heap_retry(&c, H(0), 0x4198, 8, BUFFER, HOLDER);
    CHECK(c.x[0] && held() == H(1) && strstr(said, "oot: model buffer: the rest of the build takes its memory from heap"));
    c = before, c.x[0] = H(1), c.x[8] = 0xbad;
    hos_oot_heap_rest_own(&c, HOLDER);
    CHECK(c.x[0] == H(0) && c.x[8] == FN_FREE && held() == H(1));
    for (int r = 1; r < 31; r++) CHECK(r == 8 || c.x[r] == before.x[r]);
    c.x[0] = H(1), c.x[1] = 0x238, c.x[2] = 8, game_try_alloc(&c);   /* (the build's next allocation, the game's own code) */
    CHECK(c.x[0] && room[0] == 0);
    hos_oot_heap_rest_end(&c, HOLDER);
    CHECK(held() == H(0) && c.x[0] == H(0) && c.x[8] == FN_FREE);
    c.x[0] = 0x1234, c.x[8] = 0x5678;   /* (a build that was not handed over: both reads as the game made them) */
    hos_oot_heap_rest_own(&c, HOLDER), hos_oot_heap_rest_end(&c, HOLDER);
    CHECK(c.x[0] == 0x1234 && c.x[8] == 0x5678 && held() == H(0));
    /* the object's buffers and f_80443000's blocks: the same retry and hand-over, each under its own name */
    reset(), room[1] = 4 << 20;
    c = before, said[0] = 0, hold(H(0));
    hos_oot_heap_retry(&c, H(0), 0x40, 8, OBJECT_BUFFER, HOLDER);
    CHECK(c.x[0] && held() == H(1) && strstr(said, "oot: object buffer, 0x40 bytes from heap") &&
          strstr(said, "object buffer (0x8044f070) 1 blocks"));
    hos_oot_heap_rest_end(&c, HOLDER);
    c = before, said[0] = 0;
    hos_oot_heap_retry(&c, H(0), 0x310, 8, BLOCK, HOLDER);
    CHECK(c.x[0] && held() == H(1) && strstr(said, "oot: model block, 0x310 bytes from heap") &&
          strstr(said, "model block (0x80443e2c) 1 blocks"));
    hos_oot_heap_rest_end(&c, HOLDER);
    CHECK(held() == H(0));
    /* a place that is none of them is not refused and not counted */
    CHECK(!hos_oot_heap_refuse(&c, H(0), HOLDER, 0x80456c20ull));
    /* sead::MainThread's holder: its stack is below the image (guest 0x10ff130 in a run), handed over all the same */
    reset(), room[1] = 4 << 20;
    c = before, recomp_store64(NULL, 0x10ff130ull + 8, H(0));
    hos_oot_heap_retry(&c, H(0), 0x238, 8, 0x8044f670ull, 0x10ff130ull);
    CHECK(c.x[0] && recomp_load64(NULL, 0x10ff130ull + 8) == H(1));
    hos_oot_heap_rest_end(&c, HOLDER);   /* (another build's end is not this one's) */
    CHECK(recomp_load64(NULL, 0x10ff130ull + 8) == H(1));
    hos_oot_heap_rest_end(&c, 0x10ff130ull);
    CHECK(recomp_load64(NULL, 0x10ff130ull + 8) == H(0) && c.x[0] == H(0) && c.x[8] == FN_FREE);
    CHECK(!outside);   /* (the retries' heap methods all through game_call_more) */
    puts("PASS: an object from a parent hands the rest of its build to that parent, and the build's own heap is back "
         "at f_804566e0's last read of it; a holder on a stack below the image too");
    return 0;
}
'''


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default="clang")
    p.add_argument("--cargo")
    a = p.parse_args()
    sitehooks.use(a.cc, a.cargo)

    sites = [site(name, function, "before" if kind in ("own", "end") else "replace", offset)
             for name, function, _, offset, kind, _ in SITES]
    functions, hooked_names, patched_names, cpu = [], [], [], []
    for x, (name, function, base, offset, kind, holder) in zip(sites, SITES):
        lr = base + offset + 4
        if kind in ("own", "end"):   # (before the blr: the hook, then the call as the lift writes it)
            hook = sitehooks.line(x) + "\n" + CPU_CALL % lr
            old = OLD_REST % (kind, lr)
        else:
            call = GPU_CALL if kind == "gpu" else CPU_CALL
            hook = (sitehooks.line(x, base + offset, 0x80522050 if kind == "gpu" else None, call=True) + "\n"
                    + (call % lr).rstrip("\n") + "\n" + sitehooks.REPLACE_END)
            old = OLD_GPU % lr if kind == "gpu" else OLD_CPU % (holder, lr, lr, lr)
        ident = x.ident
        functions.append("static void new_%s(Ctx *c) {\n%s\n}\nstatic void old_%s(Ctx *c) {\n%s\n}\n" % (ident, hook, ident, old))
        hooked_names.append("new_" + ident)
        patched_names.append("old_" + ident)
        cpu.append("1" if kind == "cpu" else "0")
    callbacks = "\n".join(callback(m) for m in ("int hos_site_gpu_alloc(", "static void heap_alloc(", "int hos_site_heap_alloc(",
                                                "int hos_site_heap_alloc_x21(", "int hos_site_heap_alloc_x22(",
                                                "int hos_site_heap_alloc_sp0(", "int hos_site_heap_rest_own(",
                                                "int hos_site_heap_rest_end("))
    side = (SIDE.replace("@CALLBACKS@", callbacks + "\n" + "".join(slot(x) for x in sites))
            .replace("@FUNCTIONS@", "".join(functions)).replace("@HOOKED@", ", ".join(hooked_names))
            .replace("@PATCHED@", ", ".join(patched_names)).replace("@CPU@", ", ".join(cpu)))
    cc = shutil.which(a.cc)
    if cc:
        with tempfile.TemporaryDirectory(prefix="heap-retry-sites-") as tmp:
            c, exe = Path(tmp) / "side.c", Path(tmp) / "side.exe"
            c.write_text(side, encoding="utf-8")
            subprocess.run([cc, "-std=gnu11", "-Wall", "-Wextra", "-Werror", str(c), *sitehooks.objects(tmp, side), "-o", str(exe)],
                           check=True)
            r = subprocess.run([str(exe)], capture_output=True, text=True)
            if r.returncode:
                raise SystemExit("FAIL: the sites and the text patch differ:\n" + r.stdout + r.stderr)
        print("PASS: the eleven places through their sites, as through the text patch: the same calls, arguments, holders and registers")

    cc = shutil.which(a.cc)
    if not cc:
        print("SKIP: no C compiler")
        return
    with tempfile.TemporaryDirectory(prefix="heap-retry-") as tmp:
        tmp = Path(tmp)
        (tmp / "hos.h").write_text(HOS_H, encoding="utf-8")
        (tmp / "game_call.h").write_text(GAME_H, encoding="utf-8")
        (tmp / "game_host.h").write_text('#include "hos.h"\n#include "game_call.h"\n', encoding="utf-8")   # (host/game_host.h: the two above)
        (tmp / "fixture.c").write_text(FIXTURE, encoding="utf-8")
        (tmp / "oot_heap_retry.c").write_text((ROOT / "game" / "oot_heap_retry.c").read_text(encoding="utf-8"),
                                              encoding="utf-8")
        exe = tmp / ("fixture.exe" if os.name == "nt" else "fixture")
        subprocess.run([cc, "-std=gnu11", "-O1", "-include", ENV_ON_H, "-Wall", "-Wextra", "-Werror", "-I", str(tmp), str(tmp / "fixture.c"),
                        str(tmp / "oot_heap_retry.c"), "-o", str(exe)], check=True)
        env = {k: v for k, v in os.environ.items() if not k.startswith("HOS_GPU_ALLOC")}
        for mode, extra in (("", {}), ("off", {"HOS_GPU_ALLOC_RETRY": "0"}), ("refuse", {"HOS_GPU_ALLOC_TEST_FAIL": "2"}),
                            ("off-refuse", {"HOS_GPU_ALLOC_RETRY": "0", "HOS_GPU_ALLOC_TEST_FAIL": "1"}),
                            ("split", {"HOS_GPU_ALLOC_TEST_FAIL": "0,1"}),
                            ("object", {"HOS_GPU_ALLOC_TEST_FAIL": "0,0,1"}),
                            ("rest-off", {"HOS_GPU_ALLOC_REST": "0"}),
                            ("fill", {"HOS_GPU_ALLOC_TEST_FILL": "2"}),
                            ("fill-at", {"HOS_GPU_ALLOC_TEST_FILL": "1,0x80443e2c"}),
                            ("blocks", {"HOS_GPU_ALLOC_TEST_FAIL": "0,0,2"})):
            r = subprocess.run([str(exe), mode], env=dict(env, **extra), capture_output=True, text=True)
            print("\n".join(line for line in r.stdout.splitlines() if line.startswith("PASS")))
            if r.returncode:
                raise SystemExit("FAIL (%s):\n%s%s" % (mode or "retry", r.stdout, r.stderr))


if __name__ == "__main__":
    main()
