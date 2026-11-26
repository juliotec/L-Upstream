"""The game's job barriers waiting for their work (game/oot_barrier.h, game/oot_barrier.c; HOS_OOT_BARRIER_WAIT), game-free.

Three of OoT's parallel-for jobs sleep 20 us and look again while the masks they poll are not full; with the switch the sleep
is a wait that ends when a polled mask word moves, and the thread that ORs into such a word wakes the waiters after the OR.
The fixture is the real glue (oot_barrier.c, which includes the real header) over a stub hos.h with a 64 KB guest arena,
real threads and the OS's own wait (WaitOnAddress on Windows, a futex elsewhere), run in two ways:
  - on (unset: the default; 1, yes): a word that moved since the pass looked, or masks already full, end the call at once with
    the virtual core kept; a wait with no move lasts its timeout (the glue's 1 ms, the primitive's 50 ms) and gives the core
    up once and takes it back once; a waiter asleep in the OS's wait is woken by the wake after an OR (one word, and the
    second of two words), not by a wake for another job of its slot or by a wake with no move; a move and its wake put
    just before the arming, or just after the look and before the wait, end the wait at once (no lost wake-up); four
    waiters against a thread that moves the word and wakes, 6,000 waits with random pauses in both windows, none timed out;
    the FPS counter's sleep time counts the wait;
  - off (0, off): the call answers 0 (the game's sleep runs), gives no core up, and the wake moves nothing.
Then broken variants (the header or the glue edited in the temporary copy), each of which must fail the run it breaks: no
look at the words after arming, the waiter not counted, a wake that wakes nobody, a wake that does not move the slot's word,
no timeout, a wake for another job ending the wait, the core not given up, the switch not read, the wrong slot woken.

And the hook sites that put the calls in (game/sites/sites.toml barrier.*, game/oot_sites.c; the text patch they
replaced, nx2/oot_patches.py patch_barrier_wait, is deleted): five replace sites on the sleeps and seven after sites on the
ORs into polled words, each where the patch put its text. Each place is built with the site's hook as the lift lowers it (tests/sitehooks.py)
and, side by side, with the patch's text there (OLD_PLACES, made-up places of the lift's form), and run over stand-ins that log every call
(the switch on or off, the wait answering yes or no): the same calls with the same arguments in the same order, and the
same registers and memory. A wait told the wrong word, or a wake told the wrong block, is caught (the side-by-side run of
a broken callback must fail). Each sleep site has guards elsewhere in its function (the loop head's loads and the masks'
slot its wait is told), so a build that moved them does not resolve.

Transition check: main's text-patch C kept here as the oracle shows the sites do what it did; remove it once a
deliberate behaviour change needs it.

python -I -B tests/test_oot_barrier.py [--cc clang] [--cargo cargo]
"""
import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "{clang}", "--cargo", "{cargo}"]

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves this folder off the path)
from sitecheck import check, site, callback, slot, engine_fn  # noqa: E402
import sitehooks  # noqa: E402


# (the glue's hos.h: the guest memory as a 64 KB arena at 0x80000000; the rest it calls, defined by the fixture)
STUB_HOS = r"""#ifndef HOS_H
#define HOS_H
#include <stdint.h>
#define ARENA_BASE 0x80000000ull
extern uint8_t test_arena[1 << 16];
#define A64_PTR(a) ((void *)(test_arena + ((uint64_t)(a) - ARENA_BASE)))
#define GP(a) ((void *)A64_PTR(a))
int64_t hos_now_ns(void);
void hos_log(const char *fmt, ...);
int hos_vcore_away(void);
void hos_vcore_back(void);
enum { HOS_CHAIN_WAIT = 1, HOS_CHAIN_WOKE, HOS_CHAIN_SIGNAL, HOS_CHAIN_SLEEP, HOS_CHAIN_AWAKE, HOS_CHAIN_PRESENT };
#define HOS_CHAIN(kind, key) ((void)(kind), (void)(key))
static inline int hos_ab_value(const char *name, int normal) { (void)name; return normal; }   /* (host/hos_ab.c: no A/B) */
#endif
"""

FIXTURE = r"""#define _CRT_SECURE_NO_WARNINGS
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#else
#include <windows.h>   /* (host/posix's: the types; WaitOnAddress and WakeByAddressAll are defined below) */
#include <errno.h>
#include <limits.h>
#include <linux/futex.h>
#include <pthread.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>
#endif
#include "hos.h"
#include "longshot_env.h"   /* (env_on: the switches' one rule; the glue then leaves out its own include of it) */

#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL:%d %s\n", __LINE__, #x); exit(1); } } while (0)

uint8_t test_arena[1 << 16];
volatile int hos_fps_timing;
_Thread_local int64_t hos_slept_ns[2];
static volatile long aways, backs;

int64_t hos_now_ns(void) {
#ifdef _WIN32
    static LARGE_INTEGER f;
    LARGE_INTEGER t;
    if (!f.QuadPart) QueryPerformanceFrequency(&f);
    QueryPerformanceCounter(&t);
    return (int64_t)((double)t.QuadPart * 1e9 / (double)f.QuadPart);
#else
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000000000 + ts.tv_nsec;
#endif
}
void hos_log(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vprintf(fmt, ap);
    va_end(ap);
    printf("\n");
}
int hos_vcore_away(void) { __atomic_add_fetch(&aways, 1, __ATOMIC_SEQ_CST); return 1; }
void hos_vcore_back(void) { __atomic_add_fetch(&backs, 1, __ATOMIC_SEQ_CST); }

typedef struct Tramp { void (*fn)(void *); void *arg; } Tramp;
#ifdef _WIN32
typedef HANDLE Thread;
static DWORD WINAPI tramp(void *p) { Tramp *t = (Tramp *)p; t->fn(t->arg); return 0; }
static Thread go(Tramp *t) { HANDLE h = CreateThread(NULL, 0, tramp, t, 0, NULL); CHECK(h != NULL); return h; }
static void join(Thread h) { WaitForSingleObject(h, INFINITE); CloseHandle(h); }
static void nap_ms(int ms) { Sleep((DWORD)ms); }
#else
typedef pthread_t Thread;
static void *tramp(void *p) { Tramp *t = (Tramp *)p; t->fn(t->arg); return NULL; }
static Thread go(Tramp *t) { pthread_t h; CHECK(pthread_create(&h, NULL, tramp, t) == 0); return h; }
static void join(Thread h) { pthread_join(h, NULL); }
static void nap_ms(int ms) { usleep((useconds_t)ms * 1000); }
BOOL WaitOnAddress(volatile void *addr, void *compare, SIZE_T size, DWORD ms) {
    (void)size;
    struct timespec ts = {(time_t)(ms / 1000), (long)(ms % 1000) * 1000000};
    syscall(SYS_futex, addr, FUTEX_WAIT_PRIVATE, *(uint32_t *)compare, ms == 0xFFFFFFFFu ? NULL : &ts, NULL, 0);
    return TRUE;
}
void WakeByAddressAll(void *addr) { syscall(SYS_futex, addr, FUTEX_WAKE_PRIVATE, INT_MAX, NULL, NULL, 0); }
#endif

/* (the waits' two windows: 1, a pause just before the arming; 2, after a look, just before the OS's wait; 3, a random short
   pause in both. 1 and 2 pause once, until the main thread has made its move and its wake) */
static volatile int race_mode, race_in, race_go;
static _Thread_local uint32_t rng = 0x9e3779b9u;
static void spin_us(int us) { const int64_t end = hos_now_ns() + (int64_t)us * 1000; while (hos_now_ns() < end) {} }
static void race_pause(int where) {
    const int m = __atomic_load_n(&race_mode, __ATOMIC_SEQ_CST);
    if (m == 3) {
        rng ^= rng << 13, rng ^= rng >> 17, rng ^= rng << 5;
        if (rng & 1) spin_us((int)(rng >> 1 & 15));
        return;
    }
    if (m != where) return;
    __atomic_store_n(&race_mode, 0, __ATOMIC_SEQ_CST);
    __atomic_store_n(&race_in, 1, __ATOMIC_SEQ_CST);
    while (!__atomic_load_n(&race_go, __ATOMIC_SEQ_CST)) spin_us(20);
}
#define OOT_BARRIER_ARMING() race_pause(1)
#define OOT_BARRIER_LOOKED() race_pause(2)
#include "oot_barrier.c"

#define P1 (ARENA_BASE + 0x1000)
#define P2 (ARENA_BASE + 0x3000)
#define OFF 0x200
#define OFF2 0x208
static volatile uint32_t *word(uint64_t p, uint32_t off) { return (volatile uint32_t *)GP(p + off); }

typedef struct WaitArgs {
    uint64_t p;
    int n;
    uint32_t seen[2], full;
    int64_t timeout, t_start, t_end;
    int how;
} WaitArgs;
static void waiter(void *v) {
    WaitArgs *a = (WaitArgs *)v;
    volatile uint32_t *const w[2] = {word(a->p, OFF), word(a->p, OFF2)};
    a->t_start = hos_now_ns();
    a->how = oot_barrier_wait(&barrier, a->p, w, a->seen, a->n, a->full, a->timeout);
    a->t_end = hos_now_ns();
}
static void armed(int n) {   /* (the waiter counted, so the wake after a move finds it) */
    const int64_t end = hos_now_ns() + 2000000000LL;
    while (__atomic_load_n(&barrier.waiters, __ATOMIC_SEQ_CST) != n && hos_now_ns() < end) nap_ms(1);
    CHECK(__atomic_load_n(&barrier.waiters, __ATOMIC_SEQ_CST) == n);
}

/* a word moved since the pass looked, or every word full: back at once, the core kept */
static void at_once(void) {
    *word(P1, OFF) = 5, *word(P1, OFF2) = 0xf;
    const long a0 = aways;
    const long long n0 = barrier_how[OOT_BARRIER_AT_ONCE];
    CHECK(hos_oot_barrier_wait(P1, OFF, 4, OFF, 4, 0xf) == 1);
    CHECK(hos_oot_barrier_wait(P1, OFF, 5, OFF2, 0xe, 0xf) == 1);
    *word(P1, OFF) = 0xf;
    CHECK(hos_oot_barrier_wait(P1, OFF, 0xf, OFF2, 0xf, 0xf) == 1);
    CHECK(aways == a0 && barrier_how[OOT_BARRIER_AT_ONCE] == n0 + 3);
}

/* no move: the glue's wait lasts its 1 ms and gives the core up once; the primitive's own timeout is kept */
static void timeouts(void) {
    *word(P1, OFF) = 1;
    const long a0 = aways, b0 = backs;
    const long long t_out = barrier_how[OOT_BARRIER_TIMED_OUT];
    hos_fps_timing = 1;
    hos_slept_ns[1] = 0;
    int64_t t0 = hos_now_ns();
    CHECK(hos_oot_barrier_wait(P1, OFF, 1, OFF, 1, 0xf) == 1);
    int64_t took = hos_now_ns() - t0;
    hos_fps_timing = 0;
    CHECK(took >= OOT_BARRIER_TIMEOUT_NS - 20000 && took < 500000000);
    CHECK(aways == a0 + 1 && backs == b0 + 1 && barrier_how[OOT_BARRIER_TIMED_OUT] == t_out + 1);
    CHECK(hos_slept_ns[1] >= OOT_BARRIER_TIMEOUT_NS - 20000);   /* (the FPS counter's Breakdown counts it as a sleep) */
    CHECK(barrier.waiters == 0);
    volatile uint32_t *const w[2] = {word(P1, OFF), word(P1, OFF)};
    const uint32_t seen[2] = {1, 1};
    t0 = hos_now_ns();
    CHECK(oot_barrier_wait(&barrier, P1, w, seen, 1, 0xf, 50000000) == OOT_BARRIER_TIMED_OUT);
    took = hos_now_ns() - t0;
    CHECK(took >= 50000000 - 20000 && took < 2000000000);
}

/* a waiter asleep in the OS's wait: the wake after an OR into its word, or into its second word, ends it at once */
static void wake_on_move(int second) {
    *word(P1, OFF) = second ? 0xf : 1, *word(P1, OFF2) = 1;
    WaitArgs a = {P1, second ? 2 : 1, {second ? 0xf : 1, 1}, 0xf, 2000000000LL, 0, 0, -1};
    Tramp t = {waiter, &a};
    const Thread h = go(&t);
    armed(1);
    nap_ms(20);   /* (asleep in the OS's wait by now) */
    __atomic_fetch_or(word(P1, second ? OFF2 : OFF), 2, __ATOMIC_SEQ_CST);
    const int64_t t0 = hos_now_ns();
    hos_oot_barrier_wake(P1);
    join(h);
    CHECK(a.how == OOT_BARRIER_MOVED);
    CHECK(a.t_end - t0 < 1000000000LL);   /* (by the wake, long before the 2 s) */
    CHECK(barrier.waiters == 0);
}

/* wakes with no move of the waiter's words (another job of its slot, its own job's other words) do not end the wait */
static void no_move_no_end(void) {
    const uint64_t other = P1 + (1ull << 24);   /* (the same slot, another job block) */
    CHECK(oot_barrier_slot(&barrier, other) == oot_barrier_slot(&barrier, P1));
    *word(P1, OFF) = 1;
    WaitArgs a = {P1, 1, {1, 1}, 0xf, 300000000LL, 0, 0, -1};
    Tramp t = {waiter, &a};
    const Thread h = go(&t);
    armed(1);
    for (int i = 0; i < 10; i++) {
        nap_ms(10);
        hos_oot_barrier_wake(other);
        hos_oot_barrier_wake(P1);
    }
    join(h);
    CHECK(a.how == OOT_BARRIER_TIMED_OUT);
    CHECK(a.t_end - a.t_start >= 300000000LL - 20000);
}

/* a move and its wake in each window of the wait: before the arming (the waker sees no waiter and wakes nobody), and
   between a look and the OS's wait (the waker moves the slot's word before the waiter sleeps on it) */
static void race(int where) {
    *word(P1, OFF) = 1;
    __atomic_store_n(&race_in, 0, __ATOMIC_SEQ_CST);
    __atomic_store_n(&race_go, 0, __ATOMIC_SEQ_CST);
    __atomic_store_n(&race_mode, where, __ATOMIC_SEQ_CST);
    WaitArgs a = {P1, 1, {1, 1}, 0xf, 2000000000LL, 0, 0, -1};
    Tramp t = {waiter, &a};
    const Thread h = go(&t);
    while (!__atomic_load_n(&race_in, __ATOMIC_SEQ_CST)) spin_us(20);
    CHECK(barrier.waiters == (where == 1 ? 0 : 1));
    __atomic_fetch_or(word(P1, OFF), 2, __ATOMIC_SEQ_CST);
    hos_oot_barrier_wake(P1);
    __atomic_store_n(&race_go, 1, __ATOMIC_SEQ_CST);
    join(h);
    CHECK(a.how == OOT_BARRIER_MOVED);
    CHECK(a.t_end - a.t_start < 1000000000LL);   /* (not the 2 s timeout: no lost wake-up) */
}

/* many waiters against a thread that moves the word and wakes, with random pauses in both windows: no wait times out */
enum { STRESS_WAITERS = 4, STRESS_ROUNDS = 1500 };
static volatile int stress_left, stress_timeouts;
static void stress_waiter(void *v) {
    (void)v;
    volatile uint32_t *const w[2] = {word(P2, OFF), word(P2, OFF)};
    for (int r = 0; r < STRESS_ROUNDS && !stress_timeouts; r++) {
        const uint32_t seen[2] = {__atomic_load_n(w[0], __ATOMIC_SEQ_CST), 0};
        if (oot_barrier_wait(&barrier, P2, w, seen, 1, 0xffffffffu, 2000000000LL) == OOT_BARRIER_TIMED_OUT)
            __atomic_add_fetch(&stress_timeouts, 1, __ATOMIC_SEQ_CST);
    }
    __atomic_sub_fetch(&stress_left, 1, __ATOMIC_SEQ_CST);
}
static void stress_waker(void *v) {
    (void)v;
    uint32_t r = 12345;
    while (__atomic_load_n(&stress_left, __ATOMIC_SEQ_CST)) {
        __atomic_fetch_add(word(P2, OFF), 1, __ATOMIC_SEQ_CST);
        hos_oot_barrier_wake(P2);
        r ^= r << 13, r ^= r >> 17, r ^= r << 5;
        spin_us((int)(r % 30));
    }
}
static void stress(void) {
    *word(P2, OFF) = 0;
    stress_left = STRESS_WAITERS, stress_timeouts = 0;
    __atomic_store_n(&race_mode, 3, __ATOMIC_SEQ_CST);
    Tramp tw = {stress_waiter, NULL}, tk = {stress_waker, NULL};
    Thread h[STRESS_WAITERS + 1];
    for (int i = 0; i < STRESS_WAITERS; i++) h[i] = go(&tw);
    h[STRESS_WAITERS] = go(&tk);
    for (int i = 0; i <= STRESS_WAITERS; i++) join(h[i]);
    __atomic_store_n(&race_mode, 0, __ATOMIC_SEQ_CST);
    CHECK(stress_timeouts == 0);
    CHECK(barrier.waiters == 0);
}

/* off: the game's sleep runs (0), no core given up, a wake moves nothing */
static void off(void) {
    CHECK(hos_oot_barrier_on() == 0);
    CHECK(barrier_bind());   /* (so a wait the switch does not stop waits, and fails the checks below, not crashes) */
    *word(P1, OFF) = 1;
    const long a0 = aways;
    CHECK(hos_oot_barrier_wait(P1, OFF, 1, OFF, 1, 0xf) == 0);
    CHECK(aways == a0);
    barrier.waiters = 1;
    const uint32_t g = oot_barrier_slot(&barrier, P1)->gen;
    hos_oot_barrier_wake(P1);
    CHECK(oot_barrier_slot(&barrier, P1)->gen == g);
    barrier.waiters = 0;
}

/* (a wait without its timeout spins for ever: the run then fails here) */
static volatile int finished;
static void watchdog(void *v) {
    (void)v;
    for (int i = 0; i < 200 && !finished; i++) nap_ms(50);
    if (!finished) {
        fprintf(stderr, "FAIL: the fixture did not end in 10 s\n");
        exit(1);
    }
}

int main(int argc, char **argv) {
    CHECK(argc == 2);
    static Tramp tw = {watchdog, NULL};
    go(&tw);
    if (!strcmp(argv[1], "off")) {
        off();
        finished = 1;
        printf("barrier off: all passed\n");
        return 0;
    }
    CHECK(!strcmp(argv[1], "on"));
    CHECK(hos_oot_barrier_on() == 1);
    at_once();
    timeouts();
    wake_on_move(0);
    wake_on_move(1);
    no_move_no_end();
    race(1);
    race(2);
    stress();
    CHECK(aways == backs);
    finished = 1;
    printf("barrier on: all passed (under stress %d waits; %lld wakes found a waiter)\n", STRESS_WAITERS * STRESS_ROUNDS,
           barrier_wakes);
    return 0;
}
"""

# (where, old, new, the mode whose run must fail, what it breaks); a list of (old, new) pairs edits more than one place
BROKEN = [
    ("header", [("    OOT_BARRIER_ARMING();\n",
                 "    if (oot_barrier_moved(w, seen, n, full)) return OOT_BARRIER_MOVED;\n    OOT_BARRIER_ARMING();\n"),
                ("        if (oot_barrier_moved(w, seen, n, full)) {", "        if (0) {"),
                ("        OOT_BARRIER_WAIT_ON(&s->gen, g, (uint32_t)((left + 999999) / 1000000));\n",
                 "        OOT_BARRIER_WAIT_ON(&s->gen, g, (uint32_t)((left + 999999) / 1000000));\n"
                 "        if (oot_barrier_moved(w, seen, n, full)) { how = OOT_BARRIER_MOVED; break; }\n")],
     "on", "the waiter looks at the words before arming and after each wait, not between the arming and its first wait"),
    ("header", [("    __atomic_add_fetch(&b->waiters, 1, __ATOMIC_SEQ_CST);\n", ""),
                ("    __atomic_sub_fetch(&b->waiters, 1, __ATOMIC_SEQ_CST);\n", "")],
     "on", "the waiter is not counted, so the waker skips the wake"),
    ("header", [("    OOT_BARRIER_WAKE_ALL(&s->gen);\n", "    (void)s;\n")], "on", "the wake wakes nobody"),
    ("header", [("    __atomic_add_fetch(&s->gen, 1, __ATOMIC_SEQ_CST);\n", "")], "on",
     "the wake does not move the slot's word (a waiter between its look and its sleep sleeps on)"),
    ("header", [("        if (left <= 0) break;\n", "")], "on", "no timeout"),
    ("header", [("        OOT_BARRIER_WAIT_ON(&s->gen, g, (uint32_t)((left + 999999) / 1000000));\n",
                 "        OOT_BARRIER_WAIT_ON(&s->gen, g, (uint32_t)((left + 999999) / 1000000));\n"
                 "        if (__atomic_load_n(&s->gen, __ATOMIC_SEQ_CST) != g) break;\n")],
     "on", "a wake with no move of the waiter's words (another job of the slot) ends the wait"),
    ("glue", [("    const int away = hos_vcore_away();", "    const int away = 0;")], "on", "the virtual core not given up"),
    ("glue", [("int hos_oot_barrier_wait(uint64_t p, uint32_t off1, uint32_t seen1, uint32_t off2, uint32_t seen2, uint32_t full) {\n"
               "    if (!hos_oot_barrier_on()) return 0;\n",
               "int hos_oot_barrier_wait(uint64_t p, uint32_t off1, uint32_t seen1, uint32_t off2, uint32_t seen2, uint32_t full) {\n")],
     "off", "the switch not read by the wait"),
    ("glue", [("    if (oot_barrier_wake(&barrier, p)) {", "    if (oot_barrier_wake(&barrier, p + 64)) {")], "on",
     "the wrong slot woken"),
]


# ---- the sites, side by side with the text patch they replaced: each place as the lift writes it (LIFTED_PLACES) and with
# the patch's text in its place (OLD_PLACES: function, the text in its place). The places are made-up lifted text of the
# lift's form: a sleep's call with its return address (the site's place plus 4) and its loop's back-edge, an OR's add and
# ldset through the registers the callbacks read; the lines between, the labels and the other callee are written here
SLEEP_CALL = "f_85106220(c);"
MARK = "   /* (nx2/oot_patches.py: HOS_OOT_BARRIER_WAIT) */\n"
FILL = "    c->x[12] = 0x3ULL;\n    c->x[12] &= 0xFFFFFFFFULL;\n"   # (made up: a mov of a register no callback reads)


def _sleep(ret, args, after=""):
    """a sleep's place, lifted and patched: the call, `after` lines, then the loop's back-edge"""
    call = "c->x[30] = %#xULL; " % ret
    tail = "%s{ A64_POLL(c); goto L_80001000; }\n" % after
    return (call + SLEEP_CALL + "\n" + tail,
            call + "if (!hos_oot_barrier_on() || !hos_oot_barrier_wait(%s)) %s%s%s" % (args, SLEEP_CALL, MARK, tail))


def _or(base, off, addr, value, between=""):
    """an OR's place, lifted and patched: the add of the polled word's address, `between` lines, the ldset"""
    head = ("    { uint64_t _a=c->x[%d], _b=%dULL; uint64_t _r=_a+_b; c->x[%d]=_r; }\n%s"
            "{ uint64_t _a = c->x[%d]; uint32_t *_p = (uint32_t *)A64_PTR(_a); uint32_t _o = __atomic_fetch_or(_p, "
            "(uint32_t)c->x[%d], __ATOMIC_SEQ_CST); A64_MARK(_a, 4); c->x[8] = (uint64_t)_o;" % (base, off, addr, between, addr, value))
    return head + " }\n", head + " if (hos_oot_barrier_on()) hos_oot_barrier_wake(_a - %dULL); }%s" % (off, MARK)


SHIFT = "    { c->x[10]=(uint64_t)(uint32_t)((uint32_t)c->x[4]<<(c->x[20]&31)); }\n    recomp_barrier();\n"
PLACES = (
    ("f_800998a0", _sleep(0x8009a478, "c->x[24], 500, (uint32_t)recomp_load32(c,c->x[24] + 500), 508, "
                                      "(uint32_t)recomp_load32(c,c->x[24] + 508), (uint32_t)c->x[22]")),
    ("f_800998a0", _sleep(0x8009b234, "c->x[24], 512, (uint32_t)c->x[25], 512, (uint32_t)c->x[25], (uint32_t)c->x[22]", FILL)),
    ("f_800998a0", _sleep(0x8009aff0, "c->x[24], 520, (uint32_t)c->x[19], 520, (uint32_t)c->x[19], "
                                      "(uint32_t)recomp_load32(c,c->x[29] - 100)")),
    ("f_800998a0", _or(24, 500, 8, 13)),
    ("f_800998a0", _or(24, 508, 8, 13)),
    ("f_800998a0", _or(24, 512, 8, 13)),
    ("f_800998a0", _or(24, 520, 8, 10, FILL + "    c->x[10]=recomp_load32(c,c->x[31] + 48);\n")),
    ("f_800738e0", _sleep(0x800747ac, "c->x[19], 528, (uint32_t)c->x[25], 536, (uint32_t)c->x[28], "
                                      "(uint32_t)recomp_load32(c,c->x[31] + 16)")),
    ("f_800738e0", _or(19, 528, 27, 14, "c->x[30] = 0x80001204ULL; f_80001200(c);\n    recomp_barrier();\n")),
    ("f_800738e0", _or(19, 536, 8, 10, SHIFT)),
    ("f_8007e200", _sleep(0x8007e90c, "c->x[26], 544, (uint32_t)c->x[21], 544, (uint32_t)c->x[21], ~(uint32_t)c->x[28]")),
    ("f_8007e200", _or(26, 544, 8, 10, SHIFT)),
)
LIFTED_PLACES = tuple(lifted for _, (lifted, _old) in PLACES)
OLD_PLACES = tuple((function, old) for function, (_lifted, old) in PLACES)
# (the site at each place, in OLD_PLACES' order: the five sleeps are replace sites on the bl, the ORs after sites on the ldset)
PLACE_SITES = (("barrier.sleepClaim", "f_800998a0", "replace", 0xbd4), ("barrier.sleepGather", "f_800998a0", "replace", 0x1990),
               ("barrier.sleepDone", "f_800998a0", "replace", 0x174c), ("barrier.orClaim", "f_800998a0", "after", 0xe10),
               ("barrier.orClaimOther", "f_800998a0", "after", 0xe24), ("barrier.orGather", "f_800998a0", "after", 0xe48),
               ("barrier.orDone", "f_800998a0", "after", 0x114c), ("barrier.sleepOther", "f_800738e0", "replace", 0xec8),
               ("barrier.orOther", "f_800738e0", "after", 0x7c), ("barrier.orOtherSecond", "f_800738e0", "after", 0x52c),
               ("barrier.sleepThird", "f_8007e200", "replace", 0x708), ("barrier.orThird", "f_8007e200", "after", 0x30c))

SIDE = r"""#include <stdint.h>
#include <stdio.h>
#include <string.h>
#define A64_NOESC
typedef struct { uint64_t x[32]; } Ctx;
typedef void (*A64Fn)(Ctx *);
static uint8_t arena[0x10000];
#define A64_PTR(a) ((void *)(arena + ((a) & 0xfffc)))
#define A64_MARK(a, n) ((void)(a), (void)(n))
static uint64_t log_[256];
static int logged, on, answer;
static void note(uint64_t a) { if (logged < 256) log_[logged++] = a; }
static uint64_t recomp_load32(Ctx *c, uint64_t a) { (void)c; note(0x1000 + (a & 0xffff)); uint32_t v; memcpy(&v, A64_PTR(a), 4); return v; }
static void recomp_barrier(void) { note(0x2000); }
void f_85106220(Ctx *c) { note(0x3000), note(c->x[30]); }
static void f_80001200(Ctx *c) { note(0x4000), note(c->x[30]); }
A64Fn a64_lookup(uint64_t a) { return a == 0x85106220ull ? f_85106220 : NULL; }
_Thread_local uint64_t hos_site_callee;   /* (game/sites.c's: a replaced bl's callee, which its hook stores) */
int hos_oot_barrier_on(void) { note(0x5000); return on; }
int hos_oot_barrier_wait(uint64_t p, uint32_t o1, uint32_t s1, uint32_t o2, uint32_t s2, uint32_t full) {
    note(0x6000), note(p), note(o1), note(s1), note(o2), note(s2), note(full);
    return answer;
}
void hos_oot_barrier_wake(uint64_t p) { note(0x7000), note(p); }
@CALLBACKS@
@FUNCTIONS@
typedef void (*Fn)(Ctx *);
int main(void) {
    static const Fn hooked[] = {@HOOKED@}, patched[] = {@PATCHED@};
    int fails = 0;
    for (unsigned k = 0; k < sizeof hooked / sizeof hooked[0]; k++)
        for (on = 0; on < 2; on++)
            for (answer = 0; answer < 2; answer++) {
                Ctx a, b;
                for (int r = 0; r < 32; r++) a.x[r] = 0x9e3779b97f4a7c15ull * (r + 1) + k;
                for (unsigned i = 0; i < sizeof arena; i++) arena[i] = (uint8_t)(i * 7 + k);
                b = a;
                uint64_t la[256];
                uint8_t ma[sizeof arena];
                int na;
                logged = 0, memset(log_, 0, sizeof log_);
                hooked[k](&a);
                memcpy(la, log_, sizeof la), na = logged, memcpy(ma, arena, sizeof arena);
                for (unsigned i = 0; i < sizeof arena; i++) arena[i] = (uint8_t)(i * 7 + k);
                logged = 0, memset(log_, 0, sizeof log_);
                patched[k](&b);
                if (na != logged || memcmp(la, log_, sizeof la) || memcmp(&a, &b, sizeof a) || memcmp(ma, arena, sizeof arena)) {
                    printf("FAIL: place %u, on %d, answer %d\n", k, on, answer);
                    fails++;
                }
            }
    if (!fails) printf("same\n");
    return fails != 0;
}
"""


def side_by_side(callbacks_text=None):
    """the C of the side-by-side run: each place hooked (the sites' hooks around or after the lifted text) and as the patch
    made it, the callbacks (game/oot_sites.c's, or callbacks_text) and the sites' slots ahead"""
    sites = [site(name, function, at, offset) for name, function, at, offset in PLACE_SITES]
    functions, hooked_names, patched_names, fns = [], [], [], []
    for k, (x, lifted, (function, old)) in enumerate(zip(sites, LIFTED_PLACES, OLD_PLACES)):
        check(function == x.function, "%s is not in %s, where the patch put its text" % (x.name, function))
        if x.at == "replace":
            call = lifted.split("\n")[0]
            check(call.endswith(SLEEP_CALL), "%s's place is not the sleep's call" % x.name)
            # (the words and registers the wait is told come from the loop's head and the stack, elsewhere in the function:
            # its guards are what keep a build that moved them from resolving; the side-by-side runs cannot see them)
            check(len(x.guards) >= 3 and all(off != x.signature_offset and any(m == 0xffffffff for _, m in words)
                                             for off, words in x.guards),
                  "%s's guards do not hold the loop head's loads and the masks' slot (at least three, apart from its "
                  "signature, each with a whole word)" % x.name)
            pc = int(call.split("0x")[1].split("ULL")[0], 16) - 4
            check(pc == int(x.function[2:], 16) + x.offset, "%s is not at the call whose return address the patch named" % x.name)
            hook = (sitehooks.line(x, pc, 0x85106220) + "\n" + call + "\n" + sitehooks.REPLACE_END + "\n"
                    + lifted[len(call) + 1:])
            old = old.replace("\n{ A64_POLL(c); goto", "\n{ goto")
            hook = hook.replace("\n{ A64_POLL(c); goto", "\n{ goto")
        else:
            hook = lifted + sitehooks.line(x) + "\n"
        # (the loops' back-edges go nowhere here: each place is one run of its text)
        hook = "".join(line for line in hook.splitlines(True) if "goto L_" not in line)
        old = "".join(line for line in old.splitlines(True) if "goto L_" not in line)
        functions.append("static void new_%d(Ctx *c) {\n%s}\nstatic void old_%d(Ctx *c) {\n%s}\n" % (k, hook, k, old))
        hooked_names.append("new_%d" % k)
        patched_names.append("old_%d" % k)
        fn = engine_fn(x)
        if fn not in fns:
            fns.append(fn)
    calls = callbacks_text if callbacks_text is not None else (
        "".join(callback("int %s(" % fn) + "\n" for fn in fns))
    return (SIDE.replace("@CALLBACKS@", calls + "".join(slot(x) for x in sites))
            .replace("@FUNCTIONS@", "".join(functions)).replace("@HOOKED@", ", ".join(hooked_names))
            .replace("@PATCHED@", ", ".join(patched_names)))


def run_side(cc, tmp, code, name):
    d = Path(tmp) / name
    d.mkdir()
    (d / "side.c").write_text(code, encoding="utf-8", newline="\n")
    r = subprocess.run([cc, "-std=gnu11", "-Wall", "-Wextra", "-Werror", "-Wno-unused-function", str(d / "side.c"),
                        *sitehooks.objects(d, code), "-o", str(d / "side.exe")], capture_output=True, text=True)
    check(r.returncode == 0, "the side-by-side run (%s) does not compile:\n%s%s" % (name, r.stdout, r.stderr))
    return subprocess.run([str(d / "side.exe")], capture_output=True, text=True, timeout=60)


# (callbacks broken in the side-by-side run's copy: each must make it fail)
BROKEN_CALLBACKS = [
    ("c->x[24], 520, (uint32_t)c->x[19], 520, (uint32_t)c->x[19]", "c->x[24], 512, (uint32_t)c->x[19], 512, (uint32_t)c->x[19]",
     "a wait told the wrong word"),
    ("hos_oot_barrier_wake(c->x[19]);", "hos_oot_barrier_wake(c->x[27]);", "a wake told the OR's address, not its block"),
    ("    if (!hos_oot_barrier_on() || !hos_oot_barrier_wait(c->x[26]", "    if (!hos_oot_barrier_wait(c->x[26]",
     "a wait that does not ask the switch first"),
]


def sites_checks(cc, tmp):
    r = run_side(cc, tmp, side_by_side(), "side")
    check(r.returncode == 0 and "same" in r.stdout, "the sites and the text patch differ:\n" + r.stdout + r.stderr)
    fns = []
    for name, function, at, offset in PLACE_SITES:
        fn = engine_fn(site(name, function, at, offset))
        if fn not in fns:
            fns.append(fn)
    good = "".join(callback("int %s(" % fn) + "\n" for fn in fns)
    for i, (old, new, what) in enumerate(BROKEN_CALLBACKS):
        check(good.count(old) == 1, "game/oot_sites.c no longer has the text broken callback %d changes: %r" % (i, old))
        r = run_side(cc, tmp, side_by_side(good.replace(old, new, 1)), "broken-cb-%d" % i)
        check(r.returncode != 0 and "FAIL" in r.stdout, "broken callback %d (%s) passed the side-by-side run" % (i, what))


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--cc", default="clang")
    p.add_argument("--cargo")
    a = p.parse_args()
    sitehooks.use(a.cc, a.cargo)
    header = (ROOT / "game" / "oot_barrier.h").read_text(encoding="utf-8")
    glue = (ROOT / "game" / "oot_barrier.c").read_text(encoding="utf-8")
    env = {k: v for k, v in os.environ.items() if not k.startswith("HOS_")}

    def build(tmp, name, h=header, g=glue):
        d = Path(tmp) / name
        d.mkdir()
        (d / "oot_barrier.h").write_text(h, encoding="utf-8", newline="\n")
        (d / "oot_barrier.c").write_text(g, encoding="utf-8", newline="\n")
        (d / "hos.h").write_text(STUB_HOS, encoding="utf-8", newline="\n")
        (d / "game_host.h").write_text('#include "hos.h"\n', encoding="utf-8", newline="\n")   # (host/game_host.h: the stub hos.h)
        (d / "fixture.c").write_text(FIXTURE, encoding="utf-8", newline="\n")
        exe = d / "fixture.exe"
        extra = [] if os.name == "nt" else ["-I", str(ROOT / "host" / "posix"), "-pthread"]
        r = subprocess.run([a.cc, "-std=gnu11", "-O2", "-Wall", "-Wextra", "-Werror", "-I", str(d),
                            "-I", str(ROOT / "gpu" / "vk" / "compat")] + extra + [str(d / "fixture.c"), "-o", str(exe)],
                           capture_output=True, text=True)
        check(r.returncode == 0, "the fixture (%s) does not compile:\n%s%s" % (name, r.stdout, r.stderr))
        return exe

    def run(exe, mode, value):
        e = dict(env) if value is None else dict(env, HOS_OOT_BARRIER_WAIT=value)
        try:
            return subprocess.run([str(exe), mode], env=e, capture_output=True, text=True, timeout=120)
        except subprocess.TimeoutExpired:
            return subprocess.CompletedProcess([str(exe), mode], 1, "", "FAIL: no end in 120 s")

    with tempfile.TemporaryDirectory(prefix="oot-barrier-") as tmp:
        sites_checks(a.cc, tmp)
        exe = build(tmp, "fixture")
        # (on by default since 18 Nov: unset is on; 0 / off the sleeps as before)
        for mode, value in (("on", "1"), ("on", " yes "), ("on", None), ("off", "0"), ("off", "off")):
            r = run(exe, mode, value)
            check(r.returncode == 0 and ("barrier %s: all passed" % mode) in r.stdout,
                  "%s, HOS_OOT_BARRIER_WAIT=%r:\n%s%s" % (mode, value, r.stdout, r.stderr))
            if mode == "on" and value == "1":
                print(r.stdout.strip().splitlines()[-1])
        for i, (where, edits, mode, what) in enumerate(BROKEN):
            text = header if where == "header" else glue
            for old, _ in edits:
                check(text.count(old) == 1, "game/oot_barrier.%s no longer has the text broken variant %d changes: %r"
                      % ("h" if where == "header" else "c", i, old))
            for old, new in edits:
                text = text.replace(old, new, 1)
            exe = build(tmp, "broken-%d" % i, h=text if where == "header" else header, g=text if where == "glue" else glue)
            r = run(exe, mode, "1" if mode == "on" else "0")
            check(r.returncode != 0 and "FAIL" in r.stderr, "broken variant %d: %s\n%s" % (i, what, r.stdout + r.stderr))
    print("PASS: the job barriers wait until their masks move, are woken by the ORs, lose no wake-up and time out; the sites "
          "call them as the text patch did (%d broken variants and %d broken callbacks caught)"
          % (len(BROKEN), len(BROKEN_CALLBACKS)))


if __name__ == "__main__":
    main()
