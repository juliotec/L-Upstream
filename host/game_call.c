/* Calling the game's functions (game_call.h).

   Before, each tool called the game its own way: hos_main.c's Debug tab looked a function up (a64_lookup), copied the
   context, set the stack and the arguments and called it, and hos_hostcall.c had a hos_call_guest (a64_icall, LR kept)
   that nothing used any more. game_call is that, once, with the virtual-core rule the callers had to know: lifted code
   runs only on a thread that holds one of the game's cores (or ran without one since an escape), so a thread that gave
   its core up for a block takes one back first (hos_vcore_back) and gives it up again after (hos_vcore_away), and the
   block around it finds the thread as it left it. */
#include "hos.h"
#include "game_call.h"
#include <stddef.h>
#include <string.h>

int game_has(uint64_t fn) { return fn && a64_lookup(fn) != NULL; }

int game_call(Ctx *c, uint64_t fn, uint64_t sp, int nargs, const uint64_t *args, uint64_t *ret) {
    return game_call_more(c, fn, sp, nargs, args, ret, NULL);
}

int game_call_more(Ctx *c, uint64_t fn, uint64_t sp, int nargs, const uint64_t *args, uint64_t *ret, const GameCallMore *more) {
    const A64Fn f = fn ? a64_lookup(fn) : NULL;
    Ctx *const own = hos_self() ? hos_ctx() : NULL;   /* (the thread's context, or the fiber's it runs) */
    /* (no context: refused. Host code runs guest code only with the Ctx * the lifted code gave it, which a call given
       the context (mir HostArg::Ctx) synced; a host call without it leaves the guest state in the caller's slots, so
       the context is stale) */
    const char *const why = !c ? "no context given" : !own ? "not on a guest thread" : c != own ? "not the calling thread's context" :
        nargs < 0 ? "a negative argument count" : nargs > 8 ? "more than 8 arguments" : nargs && !args ? "no arguments given" : NULL;
    if (!f || why) {
        static volatile int told;
        if (f && !__atomic_exchange_n(&told, 1, __ATOMIC_RELAXED))
            hos_log("game: a call of %#llx was refused: %s", (unsigned long long)fn, why);
        return 0;
    }
    const int take = !hos_vcore_held();
    if (take) hos_vcore_back();
    /* (every register put back afterwards but poll, which another thread may set at any time to ask this one for its core
       (hos_thread.c vc_acquire): never written whole here, and set again when a request is still pending) */
    const uint32_t poll = __atomic_load_n(&c->poll, __ATOMIC_RELAXED);
    Ctx saved;
    memcpy(&saved, c, sizeof saved);
    if (sp) c->x[31] = sp & ~0xfULL;
    for (int i = 0; i < nargs; i++) c->x[i] = args[i];
    if (more && more->set_x8) c->x[8] = more->x8;
    c->x[30] = more && more->set_lr ? more->lr : 0;
    f(c);
    if (ret) *ret = c->x[0];
    if (more && more->s0) memcpy(more->s0, &c->vreg[0][0], 4);
    if (more && more->s1) memcpy(more->s1, &c->vreg[1][0], 4);
    const size_t at = offsetof(Ctx, poll), past = at + sizeof c->poll;
    memcpy(c, &saved, at);
    memcpy((char *)c + past, (const char *)&saved + past, sizeof saved - past);
    if (poll || hos_vcore_kicked()) __atomic_store_n(&c->poll, 1, __ATOMIC_RELAXED);
    if (take) hos_vcore_away();
    return 1;
}
