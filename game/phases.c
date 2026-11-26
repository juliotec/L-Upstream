/* Frame phases (phases.h): the callbacks host code adds to a point of the game's frame, run in order on that point's thread.

   The table is fixed (PHASE_SLOTS callbacks a phase) and lives under one small lock; nothing is allocated, here or in a
   run. A run with nothing added reads one counter (`live`, relaxed) and returns. Otherwise it copies the phase's handles
   in their order onto its own stack under the lock, then for each one takes the lock, checks the handle is still added,
   marks it running, lets the lock go and calls it: the lock is never held over a callback, so a callback may add or
   remove callbacks (its own too), and another thread's add or remove waits at most for the lock's short holds. A remove
   waits on the condition variable until the callback is no longer running, except on the thread that runs it (a
   callback removing itself, or one of its callees doing so): there is nothing to wait for there. An add during a run
   takes effect at the next run (the run's copy was made before it); a remove during a run takes effect at once (each
   callback is checked again before it is called).

   The phase's thread is the one that last ran it with something added: game_phase_on_thread answers 0 for every thread
   until then (the empty run records nothing, to stay one load). One thread runs a phase; a run is not re-entered from
   its own callbacks.

   Self-contained (no hos.h): tests/test_game_phases.py builds it alone. */
#include "phases.h"
#include <stddef.h>
#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
typedef SRWLOCK phase_mutex;
typedef CONDITION_VARIABLE phase_cond;
#define PHASE_MUTEX_INIT SRWLOCK_INIT
#define PHASE_COND_INIT CONDITION_VARIABLE_INIT
static void phase_lock(phase_mutex *m) { AcquireSRWLockExclusive(m); }
static void phase_unlock(phase_mutex *m) { ReleaseSRWLockExclusive(m); }
static void phase_wait(phase_cond *c, phase_mutex *m) { SleepConditionVariableSRW(c, m, INFINITE, 0); }
static void phase_wake(phase_cond *c) { WakeAllConditionVariable(c); }
#else
#include <pthread.h>
typedef pthread_mutex_t phase_mutex;
typedef pthread_cond_t phase_cond;
#define PHASE_MUTEX_INIT PTHREAD_MUTEX_INITIALIZER
#define PHASE_COND_INIT PTHREAD_COND_INITIALIZER
static void phase_lock(phase_mutex *m) { pthread_mutex_lock(m); }
static void phase_unlock(phase_mutex *m) { pthread_mutex_unlock(m); }
static void phase_wait(phase_cond *c, phase_mutex *m) { pthread_cond_wait(c, m); }
static void phase_wake(phase_cond *c) { pthread_cond_broadcast(c); }
#endif

#define PHASE_SLOTS 64                      /* callbacks a phase can hold at once */
#define PHASE_GEN_MAX 0x7fffff              /* a handle: generation << 8 | (phase * PHASE_SLOTS + slot + 1) */
_Static_assert(GAME_PHASE_COUNT * PHASE_SLOTS < 256, "a handle's low 8 bits name the phase and the slot");

typedef struct {
    GamePhaseFn fn;
    void *user;
    int order;
    int handle;   /* 0: free */
} PhaseSlot;

typedef struct {
    PhaseSlot slot[PHASE_SLOTS];
    int list[PHASE_SLOTS];   /* the added slots in run order: by order, then by add order */
    int n;
    int running;             /* the handle being called now, 0 none */
    uintptr_t running_on;    /* the thread calling it */
} PhaseTable;
static PhaseTable phases[GAME_PHASE_COUNT];
static int live[GAME_PHASE_COUNT];            /* phases[p].n, for the run's one load (atomic) */
static uintptr_t phase_thread[GAME_PHASE_COUNT];   /* the thread that ran it last with something added (atomic) */
static int next_gen;
static phase_mutex mu = PHASE_MUTEX_INIT;
static phase_cond done = PHASE_COND_INIT;

/* the calling thread, as a number no other live thread has */
static uintptr_t phase_me(void) {
    static _Thread_local char me;
    return (uintptr_t)&me;
}

/* the slot a handle names, NULL when it names none (it may name one that was freed since: the caller compares) */
static PhaseSlot *phase_slot(int handle, int *phase) {
    const int at = (handle & 0xff) - 1;
    if (handle <= 0 || at < 0 || at >= GAME_PHASE_COUNT * PHASE_SLOTS) return NULL;
    *phase = at / PHASE_SLOTS;
    return &phases[*phase].slot[at % PHASE_SLOTS];
}

int game_phase_add(GamePhase phase, int order, GamePhaseFn fn, void *user) {
    if ((unsigned)phase >= GAME_PHASE_COUNT || !fn) return 0;
    phase_lock(&mu);
    PhaseTable *p = &phases[phase];
    int s = 0;
    while (s < PHASE_SLOTS && p->slot[s].handle) s++;
    if (s == PHASE_SLOTS) {
        phase_unlock(&mu);
        return 0;
    }
    next_gen = next_gen >= PHASE_GEN_MAX ? 1 : next_gen + 1;
    const int handle = next_gen << 8 | (phase * PHASE_SLOTS + s + 1);
    p->slot[s] = (PhaseSlot){fn, user, order, handle};
    int at = p->n;   /* (after every callback of the same order or lower: ties by add order) */
    while (at > 0 && p->slot[p->list[at - 1]].order > order) {
        p->list[at] = p->list[at - 1];
        at--;
    }
    p->list[at] = s;
    p->n++;
    __atomic_store_n(&live[phase], p->n, __ATOMIC_RELEASE);
    phase_unlock(&mu);
    return handle;
}

void game_phase_remove(int handle) {
    int phase;
    PhaseSlot *slot = phase_slot(handle, &phase);
    if (!slot) return;
    PhaseTable *p = &phases[phase];
    const uintptr_t me = phase_me();
    phase_lock(&mu);
    if (slot->handle == handle) {
        const int s = (int)(slot - p->slot);
        int at = 0;
        while (at < p->n && p->list[at] != s) at++;
        for (; at + 1 < p->n; at++) p->list[at] = p->list[at + 1];
        p->n--;
        *slot = (PhaseSlot){0};
        __atomic_store_n(&live[phase], p->n, __ATOMIC_RELEASE);
    }
    /* (also when another remove of the same handle came first: it is not running after this returns either) */
    while (p->running == handle && p->running_on != me) phase_wait(&done, &mu);
    phase_unlock(&mu);
}

void game_phase_run(GamePhase phase, uint64_t frame) {
    if ((unsigned)phase >= GAME_PHASE_COUNT || !__atomic_load_n(&live[phase], __ATOMIC_RELAXED)) return;
    PhaseTable *p = &phases[phase];
    const uintptr_t me = phase_me();
    __atomic_store_n(&phase_thread[phase], me, __ATOMIC_RELAXED);
    int handles[PHASE_SLOTS];
    phase_lock(&mu);
    const int n = p->n;
    for (int i = 0; i < n; i++) handles[i] = p->slot[p->list[i]].handle;
    for (int i = 0; i < n; i++) {
        int at;
        PhaseSlot *slot = phase_slot(handles[i], &at);
        if (slot->handle != handles[i]) continue;   /* (removed since the copy) */
        const GamePhaseFn fn = slot->fn;
        void *const user = slot->user;
        p->running = handles[i], p->running_on = me;
        phase_unlock(&mu);
        fn(user, frame);
        phase_lock(&mu);
        p->running = 0, p->running_on = 0;
        phase_wake(&done);
    }
    phase_unlock(&mu);
}

int game_phase_on_thread(GamePhase phase) {
    return (unsigned)phase < GAME_PHASE_COUNT && __atomic_load_n(&phase_thread[phase], __ATOMIC_RELAXED) == phase_me();
}
