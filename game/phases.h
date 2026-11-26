/* Frame phases: points in the game's frame where host code runs on one known thread.
   Today there is one: GAME_PHASE_ACQUIRE, on the game's main thread (sead::MainThread), at nvnWindowAcquireTexture
   (host/nn_vi.c hos_vi_native_acquire), once per game frame in every state (play, menus, pause, loading, movies).
   Whether the job workers have joined there is not yet proven; background threads (actor create/delete, loading,
   placement) run through it.

   Order numbers: 0..99 the engine's (the hook tables' reclaim 20: game/sites.c). game/phases.c says how a run
   and a removal meet. */
#ifndef LONGSHOT_GAME_PHASES_H
#define LONGSHOT_GAME_PHASES_H
#include <stdint.h>

typedef enum { GAME_PHASE_ACQUIRE = 0, GAME_PHASE_COUNT } GamePhase;

typedef void (*GamePhaseFn)(void *user, uint64_t frame);   /* frame: the game frame counter at this phase */

/* Add/remove a callback. Callbacks of one phase run in the order of their `order` (lower first; ties by add order).
   Thread-safe; may be called from any thread; takes effect from the next time the phase runs.
   Returns a handle > 0, or 0 when the table is full. With nothing added, game_phase_run costs one relaxed atomic load. */
int  game_phase_add(GamePhase phase, int order, GamePhaseFn fn, void *user);
void game_phase_remove(int handle);   /* after it returns, fn is not running and will not run again for this handle */
/* A remove waits while the callback runs (on another thread). The acquire phase's callbacks run with the main thread's
   virtual core given up and may take one back (game_call): a guest thread holding a core must not remove a callback that
   may be running, or both can wait for each other (the callback for a core, the remove for the callback). Remove from a
   thread holding none (a host thread, or inside a block), or from the callback itself. */

/* Called by the host at the phase's point, on the phase's thread. */
void game_phase_run(GamePhase phase, uint64_t frame);

/* The phase's thread, for asserts: true when the caller is on it. 0 on every thread until the phase has first run with a
   callback added (a run with nothing added records nothing, to stay one load). */
int  game_phase_on_thread(GamePhase phase);

#endif
