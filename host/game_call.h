/* Calling the game's functions (host/game_call.c): game_call, the one way host code and the game's own (game/) call
   guest code. The game's flags and the points of its frame where host code runs are the game's (game/flags.h,
   game/phases.h). */
#ifndef LONGSHOT_GAME_CALL_H
#define LONGSHOT_GAME_CALL_H
#include <stdint.h>

typedef struct GuestContext Ctx;   /* (a64.h's) */

/* 1 when this build has a game function at fn (another game build may not) */
int game_has(uint64_t fn);

/* Calls the game's function at fn, the one way host code does: up to 8 integer arguments (x0..), its x0 into *ret (when
   ret is not NULL). sp: the stack pointer it runs with (aligned down to 16), so that the caller can put what the function
   reads through pointers in guest memory below its own stack first; 0: the context's own. Every register of the context
   is put back afterwards (LR is 0 during the call: called from the host). c: the calling guest thread's context; NULL:
   the calling thread's own (hos_ctx).

   Only a guest thread can call (the function runs on its guest stack, with its thread-local storage); another thread is
   refused. A guest thread that gave its virtual core up (it is inside a block: a frame phase's callback, a wait) takes
   one back for the call and gives it up again after, as Horizon would run it, so the game's per-core state is never
   used from two threads at once (host/hos_thread.c, hos_vcore_away and its checkpoints).

   c must be the calling thread's own context (hos_ctx: the thread's, or the fiber's it runs), as the lifted code gave it
   to the host function making the call; another, or none, is refused. (A host function the lifted code calls without
   the context runs while the guest state is still in the lifted function's registers: the context is stale there, and
   it must not call the game.)
   Every register of it is put back afterwards except poll, which another thread may set during the call to ask this
   one for its core: a request pending before the call or still asked after it leaves poll set.

   Taking a core back can wait for one: a thread that gave its core up must not call while holding a lock that a thread
   holding a core may wait on (a frame phase's callback that took a host lock the game's threads also take), or the two
   wait for each other.

   1 when the function ran; 0 when it did not: no such function in this build, no context, not a guest thread, another
   thread's context, a negative count or more than 8 arguments, or arguments counted but none given. */
int game_call(Ctx *c, uint64_t fn, uint64_t sp, int nargs, const uint64_t *args, uint64_t *ret);

/* What game_call_more adds to a call, for a call made as the game's own code makes it: x8 (AAPCS64's indirect result: the
   address the callee writes a returned record to) and LR as the caller gives them, in place of the context's x8 and LR 0;
   and s0 and s1 as the callee left them (a float result, a position). */
typedef struct {
    int set_x8;
    uint64_t x8;
    int set_lr;
    uint64_t lr;
    float *s0, *s1;   /* (not NULL: the callee's s0, s1, the low 32 bits of v0, v1) */
} GameCallMore;

/* game_call with *more (NULL: game_call itself): the same refusals, core rule and context put back */
int game_call_more(Ctx *c, uint64_t fn, uint64_t sp, int nargs, const uint64_t *args, uint64_t *ret, const GameCallMore *more);

#endif
