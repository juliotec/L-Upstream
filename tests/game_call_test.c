/* game_call (host/game_call.c) with the host around it made up here: a hos.h of tests/test_game_call.py's (a context
   with the generated runtime's poll, a guest thread or none, the virtual-core calls logging their order) and a callee
   that records what it saw. Then hos_vcore_held, hos_vcore_away and hos_vcore_back as host/hos_thread.c has them, cut
   from it, over a made-up thread (game_call_vcore.h, written by the check). */
#include "hos.h"
#include "game_call.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL:%d %s\n", __LINE__, #x); exit(1); } } while (0)

/* ---- the host game.c sees */
static char order[64];   /* B back, A away, F the callee */
static void step(char c) { size_t n = strlen(order); CHECK(n + 1 < sizeof order); order[n] = c, order[n + 1] = 0; }
int held = 1, kicked, logs, guest = 1;
Ctx ctx;
static Ctx seen;   /* the context as the callee was given it */
static int callee_runs;
static void callee(Ctx *c) {
    step('F');
    callee_runs++;
    memcpy(&seen, c, sizeof seen);
    c->x[0] = 0x1234, c->x[1] = 99, c->x[19] = 7, c->x[30] = 0xdead, c->x[31] = 0x10;   /* (clobbers, as code does) */
    c->vreg[3][0] = 5, c->n = 1;
}
static void poll_callee(Ctx *c) { c->x[0] = 1, c->poll = 0; }   /* (its loop's poll took the request) */
static void kicking_callee(Ctx *c) { c->x[0] = 2, __atomic_store_n(&c->poll, 1, __ATOMIC_RELAXED), kicked = 1; }
static void float_callee(Ctx *c);
A64Fn a64_lookup(uint64_t fn) {
    return fn == 0x80001000 ? callee : fn == 0x80002000 ? poll_callee : fn == 0x80003000 ? kicking_callee :
           fn == 0x80004000 ? float_callee : NULL;
}
void *hos_self(void) { return guest ? (void *)&ctx : NULL; }
Ctx *hos_ctx(void) { return guest ? &ctx : NULL; }
int hos_vcore_held(void) { return held; }
void hos_vcore_back(void) { step('B'), held = 1; }
int hos_vcore_away(void) { step('A'), held = 0; return 1; }
int hos_vcore_kicked(void) { return kicked; }
void hos_log(const char *fmt, ...) { (void)fmt, logs++; }

static void fill(Ctx *c) {
    for (int i = 0; i < 32; i++) c->x[i] = 0x1000 + (uint64_t)i;
    c->x[31] = 0x7000;
    for (int i = 0; i < 32; i++) c->vreg[i][0] = (uint64_t)i, c->vreg[i][1] = ~(uint64_t)i;
    c->n = 0, c->z = 1, c->tpidr_el0 = 0xabc, c->poll = 0, c->tail = 0x55;
}

static void call_case(void) {
    const uint64_t args[8] = {11, 12, 13, 14, 15, 16, 17, 18};
    uint64_t ret = 0;
    fill(&ctx);
    Ctx before;
    memcpy(&before, &ctx, sizeof before);
    held = 0, order[0] = 0;
    CHECK(game_call(&ctx, 0x80001000, 0x6fff7, 8, args, &ret) == 1);
    CHECK(!strcmp(order, "BFA") && !held);   /* (a thread that gave its core up: back, the call, away again) */
    CHECK(ret == 0x1234);
    CHECK(seen.x[31] == 0x6fff0);            /* (the stack pointer aligned down to 16) */
    CHECK(seen.x[30] == 0);                  /* (no return address: called from the host) */
    for (int i = 0; i < 8; i++) CHECK(seen.x[i] == args[i]);
    CHECK(seen.x[8] == before.x[8] && seen.x[19] == before.x[19] && seen.tpidr_el0 == 0xabc);
    CHECK(!memcmp(&ctx, &before, sizeof ctx));   /* (every register put back) */

    held = 1, order[0] = 0;
    CHECK(game_call(&ctx, 0x80001000, 0, 0, NULL, NULL) == 1);
    CHECK(!strcmp(order, "F"));   /* (a thread holding its core: neither back nor away) */
    CHECK(seen.x[31] == 0x7000 && seen.x[30] == 0 && seen.x[0] == 0x1000);   /* (sp 0: the context's own) */
    CHECK(!memcmp(&ctx, &before, sizeof ctx));
    puts("PASS: game_call: back, the call, away for a thread without its core, neither for one with it; the callee sees "
         "the stack aligned down to 16, LR 0 and x0..x7; *ret is its x0; the context put back whole");
}

static void poll_case(void) {
    uint64_t ret = 0;
    fill(&ctx);
    ctx.poll = 1;   /* (a request pending before the call, which the callee's poll took) */
    CHECK(game_call(&ctx, 0x80002000, 0, 0, NULL, &ret) && ret == 1 && ctx.poll == 1);
    ctx.poll = 0, kicked = 0;
    CHECK(game_call(&ctx, 0x80002000, 0, 0, NULL, &ret) && ctx.poll == 0);   /* (none: none made up) */
    CHECK(game_call(&ctx, 0x80003000, 0, 0, NULL, &ret) && ret == 2 && ctx.poll == 1);   /* (one asked during the call) */
    kicked = 0, ctx.poll = 0;
    puts("PASS: game_call keeps the context's poll: a request pending before or asked during the call stays set");
}

static void float_callee(Ctx *c) {   /* (a position in s0, s1, as the layout's pane position returns it) */
    memcpy(&seen, c, sizeof seen);
    const float x = 1.5f, y = -2.25f;
    memcpy(&c->vreg[0][0], &x, 4), memcpy(&c->vreg[1][0], &y, 4);
    c->x[8] = 0, c->x[0] = 3;
}

static void more_case(void) {
    const uint64_t args[2] = {21, 22};
    uint64_t ret = 0;
    float s0 = 0, s1 = 0;
    fill(&ctx);
    Ctx before;
    memcpy(&before, &ctx, sizeof before);
    held = 1, order[0] = 0;
    GameCallMore more = {.set_x8 = 1, .x8 = 0x5550, .set_lr = 1, .lr = 0x80456910, .s0 = &s0, .s1 = &s1};
    CHECK(game_call_more(&ctx, 0x80004000, 0, 2, args, &ret, &more) == 1);
    CHECK(seen.x[0] == 21 && seen.x[1] == 22 && seen.x[2] == before.x[2]);
    CHECK(seen.x[8] == 0x5550 && seen.x[30] == 0x80456910 && seen.x[31] == 0x7000);   /* (x8 and LR given; sp the context's) */
    CHECK(ret == 3 && s0 == 1.5f && s1 == -2.25f);   /* (x0, s0 and s1 as the callee left them) */
    CHECK(!memcmp(&ctx, &before, sizeof ctx));        /* (every register put back, x8, LR, v0 and v1 too) */
    GameCallMore none = {0};
    CHECK(game_call_more(&ctx, 0x80004000, 0, 0, NULL, NULL, &none) == 1);
    CHECK(seen.x[8] == before.x[8] && seen.x[30] == 0);   /* (none set: the context's x8, LR 0, as game_call) */
    CHECK(game_call_more(&ctx, 0x80004000, 0, 0, NULL, NULL, NULL) == 1 && seen.x[30] == 0);
    callee_runs = 0;
    CHECK(!game_call_more(&ctx, 0x80001000, 0, 9, args, &ret, &more) && !callee_runs);   /* (refused as game_call) */
    CHECK(!memcmp(&ctx, &before, sizeof ctx));
    puts("PASS: game_call_more gives x8 and LR when asked (else the context's x8 and LR 0), hands back s0 and s1, and "
         "puts the context back whole");
}

static void refusal_case(void) {
    const uint64_t args[9] = {0};
    uint64_t ret = 77;
    callee_runs = 0, held = 0, order[0] = 0, logs = 0;
    CHECK(!game_call(&ctx, 0x80009000, 0, 0, NULL, &ret));        /* (no such function: refused quietly) */
    CHECK(!logs);
    CHECK(!game_call(&ctx, 0x80001000, 0, 9, args, &ret));        /* (9 arguments) */
    CHECK(!game_call(&ctx, 0x80001000, 0, -1, args, &ret));       /* (a negative count) */
    CHECK(!game_call(&ctx, 0x80001000, 0, 2, NULL, &ret));        /* (no arguments given for 2) */
    CHECK(!game_call(NULL, 0x80001000, 0, 0, NULL, &ret));        /* (no context: a host call without one has it stale) */
    Ctx other;
    memcpy(&other, &ctx, sizeof other);
    CHECK(!game_call(&other, 0x80001000, 0, 0, NULL, &ret));      /* (not the calling thread's context) */
    guest = 0;
    CHECK(!game_call(&ctx, 0x80001000, 0, 0, NULL, &ret));        /* (not a guest thread) */
    CHECK(!game_call(&ctx, 0x80001000, 0, 0, NULL, &ret));
    guest = 1;
    CHECK(!callee_runs && !order[0] && ret == 77 && logs == 1);   /* (nothing called, no core taken, logged once) */
    puts("PASS: game_call refuses no function, 9 or a negative count of arguments, none given, no context, another context and a "
         "thread that is not a guest thread, calling nothing");
}

/* ---- hos_thread.c's hos_vcore_held, away and back, cut from it (as cut_vcore_*: the stubs above have their names) */
#include "game_call_vcore.h"

static void vcore_case(void) {
    HosThread t = {0};
    vcores = 0, cur = &t;
    CHECK(cut_vcore_held());   /* (virtual cores off) */
    vcores = 1, cur = NULL;
    CHECK(cut_vcore_held());   /* (not a guest thread) */
    cur = &t, t.vc_held = 2;
    CHECK(cut_vcore_held());   /* (it holds core 1) */
    CHECK(cut_vcore_away() == 1 && !t.vc_held && !cut_vcore_held());   /* (it gave it up) */
    cut_vcore_back();
    CHECK(t.vc_held && cut_vcore_held());
    t.vc_held = 0, t.vc_escaped = 1;
    CHECK(cut_vcore_held());   /* (it runs without one since an escape) */
    CHECK(cut_vcore_away() == 1 && !t.vc_escaped && !cut_vcore_held());   /* (escaped, then a block: to take one) */
    cut_vcore_back();
    CHECK(t.vc_held && cut_vcore_held());
    puts("PASS: cut_vcore_held: 1 with virtual cores off, off a guest thread, holding a core or escaped; 0 after an away, "
         "also an escaped thread's, until its back");
}

int main(void) {
    call_case();
    poll_case();
    refusal_case();
    more_case();   /* (after refusal_case: game.c logs its first refusal only) */
    vcore_case();
    return 0;
}
