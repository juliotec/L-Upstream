/* The frame phases (game/phases.c, built alone) against callbacks of this test's: the order they run in, adds and
   removes from inside a callback and from another thread while one runs, a full table, stale handles, and the empty run
   that does nothing. tests/test_game_phases.py builds and runs it (and checks in the source that the empty run is one
   relaxed load). */
#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#include "game/phases.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
static void sleep_ms(int ms) { Sleep((DWORD)ms); }
typedef HANDLE thread_t;
typedef struct { void *(*fn)(void *); void *arg; } Start;
static DWORD WINAPI thread_main(void *p) {
    Start s = *(Start *)p;
    free(p);
    s.fn(s.arg);
    return 0;
}
static thread_t start_thread(void *(*fn)(void *), void *arg) {
    Start *s = (Start *)malloc(sizeof *s);
    s->fn = fn, s->arg = arg;
    return CreateThread(NULL, 0, thread_main, s, 0, NULL);
}
static void join_thread(thread_t t) { WaitForSingleObject(t, INFINITE), CloseHandle(t); }
#else
#include <pthread.h>
#include <time.h>
static void sleep_ms(int ms) {
    struct timespec t = {ms / 1000, (long)(ms % 1000) * 1000000L};
    nanosleep(&t, NULL);
}
typedef pthread_t thread_t;
static thread_t start_thread(void *(*fn)(void *), void *arg) {
    pthread_t t;
    if (pthread_create(&t, NULL, fn, arg)) exit(3);
    return t;
}
static void join_thread(thread_t t) { pthread_join(t, NULL); }
#endif

#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL:%d %s\n", __LINE__, #x); exit(1); } } while (0)
#define LOAD(v) __atomic_load_n(&(v), __ATOMIC_ACQUIRE)
#define STORE(v, x) __atomic_store_n(&(v), (x), __ATOMIC_RELEASE)

/* ---- the order: each callback writes its tag into the run's log ---- */
static int ran[128], nran;
static uint64_t ran_frame;
static void tag(void *user, uint64_t frame) {
    ran[nran++] = (int)(intptr_t)user;
    ran_frame = frame;
}
static void expect(const int *want, int n) {
    CHECK(nran == n);
    for (int i = 0; i < n; i++) CHECK(ran[i] == want[i]);
    nran = 0;
}

static void order_case(void) {
    nran = 0;
    game_phase_run(GAME_PHASE_ACQUIRE, 1);   /* (nothing added: nothing runs, and the run records no thread) */
    CHECK(nran == 0 && !game_phase_on_thread(GAME_PHASE_ACQUIRE));
    const int a = game_phase_add(GAME_PHASE_ACQUIRE, 110, tag, (void *)(intptr_t)1);
    const int b = game_phase_add(GAME_PHASE_ACQUIRE, 10, tag, (void *)(intptr_t)2);
    const int c = game_phase_add(GAME_PHASE_ACQUIRE, 100, tag, (void *)(intptr_t)3);
    const int d = game_phase_add(GAME_PHASE_ACQUIRE, 10, tag, (void *)(intptr_t)4);   /* (a tie: after b, added first) */
    const int e = game_phase_add(GAME_PHASE_ACQUIRE, -5, tag, (void *)(intptr_t)5);
    CHECK(a > 0 && b > 0 && c > 0 && d > 0 && e > 0);
    CHECK(a != b && b != c && c != d && d != e && a != e);
    game_phase_run(GAME_PHASE_ACQUIRE, 42);
    expect((const int[]){5, 2, 4, 3, 1}, 5);
    CHECK(ran_frame == 42);
    CHECK(game_phase_on_thread(GAME_PHASE_ACQUIRE));
    game_phase_remove(d);
    game_phase_run(GAME_PHASE_ACQUIRE, 43);
    expect((const int[]){5, 2, 3, 1}, 4);
    const int f = game_phase_add(GAME_PHASE_ACQUIRE, 10, tag, (void *)(intptr_t)6);   /* (after b again: d is gone) */
    game_phase_run(GAME_PHASE_ACQUIRE, 44);
    expect((const int[]){5, 2, 6, 3, 1}, 5);
    game_phase_remove(d);   /* (removed twice: nothing) */
    game_phase_remove(0), game_phase_remove(-3), game_phase_remove(0x7fffffff);
    game_phase_run(GAME_PHASE_ACQUIRE, 45);
    expect((const int[]){5, 2, 6, 3, 1}, 5);
    for (const int *h = (const int[]){a, b, c, e, f, 0}; *h; h++) game_phase_remove(*h);
    game_phase_run(GAME_PHASE_ACQUIRE, 46);
    CHECK(nran == 0);
    /* (refused: no such phase, no function) */
    CHECK(!game_phase_add(GAME_PHASE_COUNT, 0, tag, NULL) && !game_phase_add((GamePhase)-1, 0, tag, NULL));
    CHECK(!game_phase_add(GAME_PHASE_ACQUIRE, 0, NULL, NULL));
    CHECK(!game_phase_on_thread(GAME_PHASE_COUNT));
    game_phase_run(GAME_PHASE_COUNT, 1);
    puts("PASS: callbacks run by order then by add order, with the frame; removed ones do not; bad handles do nothing");
}

/* ---- a full table, and a stale handle after its slot is used again ---- */
static void table_case(void) {
    int h[64];
    for (int i = 0; i < 64; i++) CHECK((h[i] = game_phase_add(GAME_PHASE_ACQUIRE, i, tag, (void *)(intptr_t)i)) > 0);
    CHECK(game_phase_add(GAME_PHASE_ACQUIRE, 0, tag, NULL) == 0);
    nran = 0;
    game_phase_run(GAME_PHASE_ACQUIRE, 1);
    CHECK(nran == 64);
    for (int i = 0; i < 64; i++) CHECK(ran[i] == i);
    nran = 0;
    game_phase_remove(h[7]);
    const int again = game_phase_add(GAME_PHASE_ACQUIRE, 7, tag, (void *)(intptr_t)700);   /* (h[7]'s slot) */
    CHECK(again > 0 && again != h[7]);
    game_phase_remove(h[7]);   /* (stale: the new one stays) */
    game_phase_run(GAME_PHASE_ACQUIRE, 2);
    CHECK(nran == 64 && ran[7] == 700);
    nran = 0;
    game_phase_remove(again);
    for (int i = 0; i < 64; i++) game_phase_remove(h[i]);
    game_phase_run(GAME_PHASE_ACQUIRE, 3);
    CHECK(nran == 0);
    puts("PASS: a full table refuses an add with 0; a stale handle does not remove the callback that took its slot");
}

/* ---- adds and removes from inside a callback ---- */
static int self_handle, victim_handle, added_handle;
static void removes_itself(void *user, uint64_t frame) {
    tag(user, frame);
    game_phase_remove(self_handle);   /* (must not wait for itself) */
}
static void removes_later(void *user, uint64_t frame) {
    tag(user, frame);
    game_phase_remove(victim_handle);   /* (a later one of this run: it does not run) */
    if (!added_handle) added_handle = game_phase_add(GAME_PHASE_ACQUIRE, 0, tag, (void *)(intptr_t)9);   /* (next run) */
}
static void inside_case(void) {
    nran = 0, added_handle = 0;
    self_handle = game_phase_add(GAME_PHASE_ACQUIRE, 1, removes_itself, (void *)(intptr_t)1);
    const int remover = game_phase_add(GAME_PHASE_ACQUIRE, 2, removes_later, (void *)(intptr_t)2);
    victim_handle = game_phase_add(GAME_PHASE_ACQUIRE, 3, tag, (void *)(intptr_t)3);
    game_phase_run(GAME_PHASE_ACQUIRE, 1);
    expect((const int[]){1, 2}, 2);
    CHECK(added_handle > 0);
    game_phase_run(GAME_PHASE_ACQUIRE, 2);
    expect((const int[]){9, 2}, 2);
    game_phase_remove(remover), game_phase_remove(added_handle);
    game_phase_run(GAME_PHASE_ACQUIRE, 3);
    CHECK(nran == 0);
    puts("PASS: a callback removes itself, removes a later one before it runs, and adds one that runs from the next run");
}

/* ---- another thread's remove and add while a callback runs ---- */
static volatile int entered, remover_waiting, slow_done, other_ran, late_handle;
static int slow_handle;
static void slow(void *user, uint64_t frame) {
    (void)user, (void)frame;
    STORE(entered, 1);
    while (!LOAD(remover_waiting)) sleep_ms(1);
    sleep_ms(80);   /* (the remove has started meanwhile, and must still be waiting) */
    STORE(slow_done, 1);
}
static void late(void *user, uint64_t frame) { (void)user, (void)frame; STORE(other_ran, LOAD(other_ran) + 1); }
static void *remover(void *unused) {
    (void)unused;
    while (!LOAD(entered)) sleep_ms(1);
    STORE(late_handle, game_phase_add(GAME_PHASE_ACQUIRE, 50, late, NULL));   /* (during the run: from the next one) */
    CHECK(!game_phase_on_thread(GAME_PHASE_ACQUIRE));
    STORE(remover_waiting, 1);
    game_phase_remove(slow_handle);
    CHECK(LOAD(slow_done));   /* (it returned before the remove did) */
    return NULL;
}
static void *runner(void *frames) {
    for (int i = 0; i < (int)(intptr_t)frames; i++) game_phase_run(GAME_PHASE_ACQUIRE, (uint64_t)i);
    return NULL;
}
static void thread_case(void) {
    slow_handle = game_phase_add(GAME_PHASE_ACQUIRE, 10, slow, NULL);
    thread_t r = start_thread(remover, NULL);
    thread_t g = start_thread(runner, (void *)(intptr_t)1);
    join_thread(g);
    join_thread(r);
    CHECK(LOAD(slow_done) && LOAD(other_ran) == 0 && LOAD(late_handle) > 0);
    STORE(slow_done, 0);
    runner((void *)(intptr_t)3);
    CHECK(!LOAD(slow_done) && LOAD(other_ran) == 3);
    game_phase_remove(LOAD(late_handle));
    puts("PASS: a remove from another thread returns only after the running callback did; an add during a run counts from the next");
}

/* ---- many adds and removes from other threads while the phase runs: nothing runs after its remove returned ---- */
enum { CHURNERS = 3, ROUNDS = 3000 };
typedef struct { volatile int removed; volatile int calls; } Churn;
static volatile int churn_bad, running_frames;
static void churned(void *user, uint64_t frame) {
    Churn *c = (Churn *)user;
    (void)frame;
    if (LOAD(c->removed)) STORE(churn_bad, 1);
    STORE(c->calls, LOAD(c->calls) + 1);
}
static void *churner(void *seed) {
    unsigned x = (unsigned)(intptr_t)seed;
    static Churn pool[CHURNERS][8];
    Churn *mine = pool[(intptr_t)seed % CHURNERS];
    int handles[8] = {0};
    for (int r = 0; r < ROUNDS; r++) {
        x = x * 1103515245u + 12345u;
        const int i = (int)(x >> 16) % 8;
        if (handles[i]) {
            game_phase_remove(handles[i]);
            STORE(mine[i].removed, 1);   /* (from here on it must not run) */
            handles[i] = 0;
        } else {
            STORE(mine[i].removed, 0);
            handles[i] = game_phase_add(GAME_PHASE_ACQUIRE, (int)(x >> 8) % 200, churned, &mine[i]);
        }
    }
    for (int i = 0; i < 8; i++)
        if (handles[i]) game_phase_remove(handles[i]), STORE(mine[i].removed, 1);
    return NULL;
}
static void *frames_until(void *stop) {
    while (!LOAD(*(volatile int *)stop)) game_phase_run(GAME_PHASE_ACQUIRE, (uint64_t)LOAD(running_frames)), STORE(running_frames, LOAD(running_frames) + 1);
    return NULL;
}
static void churn_case(void) {
    static volatile int stop;
    thread_t g = start_thread(frames_until, (void *)&stop);
    thread_t t[CHURNERS];
    for (int i = 0; i < CHURNERS; i++) t[i] = start_thread(churner, (void *)(intptr_t)i);
    for (int i = 0; i < CHURNERS; i++) join_thread(t[i]);
    STORE(stop, 1);
    join_thread(g);
    CHECK(!LOAD(churn_bad));
    CHECK(LOAD(running_frames) > 0);
    nran = 0;
    game_phase_run(GAME_PHASE_ACQUIRE, 0);
    CHECK(nran == 0);
    printf("PASS: %d adds and removes from %d threads over %d runs: no callback ran after its remove returned\n",
           CHURNERS * ROUNDS, CHURNERS, (int)LOAD(running_frames));
}

int main(void) {
    order_case();
    table_case();
    inside_case();
    thread_case();
    churn_case();
    return 0;
}
