"""Several handlers on one hook site (game/sites.c hos_site_add, hos_site_remove, hos_site_next; game/sites.h,
docs/dev/reference/hooks.md#several-handlers-on-one-site), game-free: game/sites.c compiled with a stand-in site list
and run.

  - a site with no handler added is as before: its own callback in the slot, called straight from the hook (an engine
    site on from the start, a run-time one off), so the dormant cost and a lone callback's are unchanged;
  - handlers run lowest priority first, the earlier added first among equals, the site's own callback last; each runs the
    rest with hos_site_next and returns the answer it wants, so wrapping a call or a return value and an allow site's
    condition compose where one replacement would shut the others out;
  - a handler that does not call hos_site_next stops the chain; hos_site_next outside a handler answers 0;
  - a hook reached inside a handler's call runs its own site's chain, and the first chain then goes on where it was;
  - removing every handler puts the site's own callback back in the slot itself; a removed handle, or one never given,
    removes nothing; a site takes HOS_SITE_HANDLERS handlers, the next is refused;
  - hos_site_set still sets the site's own callback, under its handlers;
  - a site that changes the game's outcome on a worker thread (a HOS_SITE_OUTCOME line) takes no handler and no callback
    set at run time: hos_site_add and hos_site_set refuse it (docs/dev/reference/hooks.md, who may change the outcome);
  - a replace site's own callback calls the function its hook stored, even after a handler before it called into the game
    and reached another replace site (hos_site_callee kept per chain);
  - a chain's place is its guest context's: a handler whose fiber resumes on another OS thread goes on with its chain
    there, and another context's hos_site_next inside a handler answers 0;
  - a removed handler is not run by a hook that loaded the dispatcher before the removal, and hos_site_remove_then's done
    runs at the acquire phase's reclaim once no hook is inside the site's chain (not while the handler that removed itself
    still runs), once;
  - a table (hos_table_publish) replaced is freed once its readers have left (hos_table_enter / hos_table_leave), not
    before, and the table already published, published again, is kept, not freed under its readers;
  - and the real site list (game/sites.def) compiles with it, warnings as errors.

python -I -B tests/test_site_chain.py [--cc gcc]
"""
import argparse
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
GAME = ROOT / "game"
A64_H = """#include <stdint.h>
#define A64_NOESC
typedef struct { uint64_t x[32]; } Ctx;
uint64_t a64_lookup(uint64_t guest);
"""

SITES_DEF = '''HOS_SITE_ENGINE(test_engine, "test.engine", engine_fn)
HOS_SITE(test_plain, "test.plain")
HOS_SITE_ENGINE(test_inner, "test.inner", inner_fn)
HOS_SITE_ENGINE(test_call, "test.call", call_fn)
HOS_SITE_ENGINE(test_decide, "test.decide", engine_fn)
HOS_SITE_OUTCOME(test_decide, "test.decide")
HOS_TABLE(test_table, "test.table", test_table)
'''

TEST = r'''
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include "sites.h"
#include "phases.h"

/* (sites.c's free, compiled as test_free: the tables it frees, counted and not freed) */
static void *freed[8];
static int nfreed;
void test_free(void *p) { if (nfreed < 8) freed[nfreed] = p; nfreed++; }
const void *volatile test_table;

int game_phase_add(GamePhase phase, int order, GamePhaseFn fn, void *user) { (void)phase, (void)order, (void)fn, (void)user; return 1; }

static char trail[256];
static void mark(const char *s) { strncat(trail, s, sizeof trail - strlen(trail) - 1); }
static int fails;
#define CHECK(c, what) do { if (!(c)) { printf("FAIL: %s\n", what); fails++; } } while (0)

/* (what the lifted hook does at a callback site: builder/crates/edits/src/site.rs; tests/sitehooks.py runs the real one) */
static int hook(volatile uint8_t *on, HosSiteFn volatile *fn, Ctx *c) { return *on ? (*fn)(c) : -1; }
#define HOOK(id, c) hook(&hos_site_on_##id, &hos_site_fn_##id, c)

int engine_fn(Ctx *c) { (void)c; mark("E"); return 7; }
int inner_fn(Ctx *c) { (void)c; mark("I"); return 3; }
static int wrap_a(Ctx *c) { mark("a<"); int r = hos_site_next(c); mark(">a"); return r + 100; }
static int wrap_b(Ctx *c) { mark("b<"); int r = hos_site_next(c); mark(">b"); return r * 2; }
static int wrap_c(Ctx *c) { mark("c<"); int r = hos_site_next(c); mark(">c"); return r; }
static int stop(Ctx *c) { (void)c; mark("S"); return 42; }
static int veto(Ctx *c) { mark("v"); return c->x[0] && hos_site_next(c); }   /* (an allow site's condition) */
static int nested(Ctx *c) {   /* (a hook reached while this handler runs: its own chain, then this one goes on) */
    mark("n<"); int inner = HOOK(test_inner, c); mark("|"); int r = hos_site_next(c); mark(">n"); return r + inner;
}
static int note(Ctx *c) { mark("i"); return hos_site_next(c) + 1000; }
/* (a replace site's own callback calls the function its hook stored; a handler before it that calls into the game reaches
   another replace site, whose hook stores its own) */
static uint64_t called;
int call_fn(Ctx *c) { (void)c; called = hos_site_callee; return 0; }
static int into_game(Ctx *c) { hos_site_callee = 0x2222; HOOK(test_inner, c); return hos_site_next(c); }
/* (a guest fiber resumed on another OS thread in the middle of a handler: the rest of its chain from there) */
struct moved { Ctx *c; int r; };
static void *resumed(void *arg) { struct moved *m = arg; m->r = hos_site_next(m->c); return NULL; }
static int migrate(Ctx *c) {
    mark("m<");
    struct moved m = {c, -1};
    pthread_t t;
    if (pthread_create(&t, NULL, resumed, &m) || pthread_join(t, NULL)) return -1;
    mark(">m");
    return m.r;
}
static Ctx other_ctx;
/* (a removed handler's state, let go once no hook can still run it) */
static int done_calls, done_inside;
static void let_go(void *user) { (void)user; done_calls++; }
static int self_handle;
static int removes_itself(Ctx *c) {
    mark("x");
    CHECK(hos_site_remove_then(self_handle, let_go, NULL) == 1, "a handler removes itself");
    hos_tables_reclaim();   /* (the acquire phase while this handler still runs: not yet) */
    done_inside = done_calls;
    return hos_site_next(c);
}
static int other_context(Ctx *c) { mark("o"); return hos_site_next(&other_ctx) == 0 ? hos_site_next(c) : -100; }

int main(void) {
    Ctx ctx; memset(&ctx, 0, sizeof ctx); Ctx *c = &ctx;
    /* no handler: as before */
    CHECK(hos_site_fn_test_engine == engine_fn && hos_site_on_test_engine == 1, "an engine site's slot is its callback");
    CHECK(hos_site_on_test_plain == 0 && hos_site_fn_test_plain == hos_site_none, "a run-time site starts off");
    CHECK(HOOK(test_plain, c) == -1, "an off site calls nothing");
    CHECK(hos_site_next(c) == 0, "hos_site_next outside a handler answers 0");
    /* order: priority, then the order added; the site's own callback last */
    int ha = hos_site_add("test.engine", wrap_a, 10), hb = hos_site_add("test.engine", wrap_b, 0);
    int hc = hos_site_add("test.engine", wrap_c, 0);
    CHECK(ha > 0 && hb > 0 && hc > 0 && ha != hb && hb != hc, "handles");
    CHECK(hos_site_fn_test_engine != engine_fn && hos_site_on_test_engine == 1, "with handlers the slot is the dispatcher");
    trail[0] = 0;
    int r = HOOK(test_engine, c);
    CHECK(!strcmp(trail, "b<c<a<E>a>c>b"), trail);
    CHECK(r == (7 + 100) * 2, "each handler's answer from the rest's");
    /* a handler that does not call the rest stops the chain */
    int hs = hos_site_add("test.engine", stop, 5);
    trail[0] = 0;
    r = HOOK(test_engine, c);
    CHECK(!strcmp(trail, "b<c<S>c>b") && r == 84, trail);
    CHECK(hos_site_remove(hs) == 1 && hos_site_remove(hs) == 0 && hos_site_remove(9999) == 0 && hos_site_remove(0) == 0,
          "remove: once, and nothing for a handle not given");
    /* nested: a hook inside a handler's call */
    int hn = hos_site_add("test.engine", nested, -5), hi = hos_site_add("test.inner", note, 0);
    trail[0] = 0;
    r = HOOK(test_engine, c);
    CHECK(!strcmp(trail, "n<iI|b<c<a<E>a>c>b>n"), trail);
    CHECK(r == (7 + 100) * 2 + 1003, "the nested chain's answer, then the outer one's");
    CHECK(hos_site_remove(hn) && hos_site_remove(hi) && hos_site_remove(ha) && hos_site_remove(hb) && hos_site_remove(hc),
          "remove each");
    CHECK(hos_site_fn_test_engine == engine_fn && hos_site_on_test_engine == 1, "no handler left: the callback itself again");
    CHECK(hos_site_fn_test_inner == inner_fn, "the inner site's callback itself again");
    /* a run-time site with handlers only: on while they are there */
    int hv = hos_site_add("test.plain", veto, 0);
    CHECK(hos_site_on_test_plain == 1, "a handler turns a run-time site on");
    c->x[0] = 1; trail[0] = 0;
    CHECK(HOOK(test_plain, c) == 0 && !strcmp(trail, "v"), "the end of a chain with no callback of the site's answers 0");
    hos_site_set("test.plain", inner_fn);
    c->x[0] = 1; trail[0] = 0;
    CHECK(HOOK(test_plain, c) == 1 && !strcmp(trail, "vI"), "hos_site_set's callback under the handlers");
    c->x[0] = 0; trail[0] = 0;
    CHECK(HOOK(test_plain, c) == 0 && !strcmp(trail, "v"), "a condition that says no: the rest not run");
    hos_site_set("test.plain", NULL);
    CHECK(hos_site_on_test_plain == 1, "the handler keeps the site on without a callback of its own");
    CHECK(hos_site_remove(hv) && hos_site_on_test_plain == 0 && hos_site_fn_test_plain == hos_site_none,
          "the last handler gone: off, the slot calling nothing");
    /* capacity, unknown sites */
    int h[HOS_SITE_HANDLERS + 1];
    for (int i = 0; i < HOS_SITE_HANDLERS; i++) h[i] = hos_site_add("test.plain", wrap_c, i);
    CHECK(h[HOS_SITE_HANDLERS - 1] > 0 && hos_site_add("test.plain", wrap_c, 0) == 0, "a site takes HOS_SITE_HANDLERS handlers");
    for (int i = 0; i < HOS_SITE_HANDLERS; i++) hos_site_remove(h[i]);
    CHECK(hos_site_add("test.none", wrap_c, 0) == 0 && hos_site_add("test.plain", NULL, 0) == 0, "no such site, no handler");
    CHECK(hos_site_on_test_plain == 0, "off again");
    /* tables: the one replaced freed once its readers have left; the same one published again kept */
    static int ta, tb;
    CHECK(hos_table_publish("test.table", &ta) == 1 && test_table == &ta && nfreed == 0, "a first table: nothing freed");
    CHECK(hos_table_publish("test.table", &ta) == 1 && test_table == &ta && nfreed == 0,
          "the table already published, published again: kept, not freed under its readers");
    const void *in = hos_table_enter(&hos_table_readers_test_table, &test_table);
    CHECK(in == &ta, "a reader enters the table");
    CHECK(hos_table_publish("test.table", &tb) == 1 && test_table == &tb && nfreed == 0, "replaced while read: not freed yet");
    hos_tables_reclaim();
    CHECK(nfreed == 0, "the reclaim keeps it while its reader is in");
    hos_table_leave(&hos_table_readers_test_table);
    hos_tables_reclaim();
    CHECK(nfreed == 1 && freed[0] == &ta, "freed once its reader has left");
    hos_tables_reclaim();
    CHECK(nfreed == 1, "freed once");
    CHECK(hos_table_publish("test.none", &ta) == 0, "no such table");
    /* a replace site's callee, kept across a handler's call into the game */
    int hg = hos_site_add("test.call", into_game, 0);
    hos_site_callee = 0x1111, trail[0] = 0;
    HOOK(test_call, c);
    CHECK(called == 0x1111, "a handler's call into the game changed the function the replace site's callback calls");
    CHECK(hos_site_remove(hg), "remove the handler");
    /* the chain's place is the guest context's: a fiber resumed on another OS thread goes on with its chain there, and
       another context's hos_site_next inside a handler is not this chain's */
    int hm = hos_site_add("test.engine", migrate, 0);
    trail[0] = 0;
    r = HOOK(test_engine, c);
    CHECK(!strcmp(trail, "m<E>m") && r == 7, "a chain resumed on another OS thread does not go on");
    CHECK(hos_site_remove(hm), "remove the handler");
    int ho = hos_site_add("test.engine", other_context, 0);
    trail[0] = 0;
    r = HOOK(test_engine, c);
    CHECK(!strcmp(trail, "oE") && r == 7, "another context's hos_site_next ran this chain");
    CHECK(hos_site_remove(ho), "remove the handler");
    /* removed: a hook that loaded the dispatcher before still runs no removed handler; and what the handler uses is let go
       only once no hook is inside the site's chain */
    int hw = hos_site_add("test.engine", wrap_a, 0);
    HosSiteFn loaded = hos_site_fn_test_engine;   /* (a hook's load of the slot, just before the removal) */
    CHECK(hos_site_remove(hw) && hos_site_fn_test_engine == engine_fn, "the last handler removed: the callback itself");
    trail[0] = 0;
    r = loaded(c);
    CHECK(!strcmp(trail, "E") && r == 7, "a hook that loaded the dispatcher before the removal ran the removed handler");
    self_handle = hos_site_add("test.engine", removes_itself, 0);
    trail[0] = 0, done_calls = 0;
    HOOK(test_engine, c);
    CHECK(!strcmp(trail, "xE") && done_inside == 0, "let go while the handler still ran");
    CHECK(done_calls == 0, "let go before the acquire phase");
    hos_tables_reclaim();
    CHECK(done_calls == 1, "not let go once no hook is in the site's chain");
    hos_tables_reclaim();
    CHECK(done_calls == 1, "let go twice");
    int hq = hos_site_add("test.engine", wrap_b, 0);
    CHECK(hos_site_remove_then(hq, let_go, NULL) == 1 && hos_site_remove_then(hq, let_go, NULL) == 0, "removed once");
    hos_tables_reclaim();
    CHECK(done_calls == 2, "a handler removed with no hook in its chain is let go at the next acquire phase");
    /* a site that changes the game's outcome on a worker thread (HOS_SITE_OUTCOME): no callback set at run time */
    CHECK(hos_site_add("test.decide", wrap_a, 0) == 0, "a run-time handler on a worker thread's outcome site");
    CHECK(hos_site_set("test.decide", inner_fn) == 0 && hos_site_fn_test_decide == engine_fn,
          "a run-time callback set on a worker thread's outcome site");
    CHECK(hos_site_add("test.engine", wrap_a, 0) > 0, "a site of another form still takes handlers");
    if (!fails) printf("all passed\n");
    return fails != 0;
}
'''


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default=shutil.which("gcc") or shutil.which("clang") or "cc")
    a = p.parse_args()
    flags = ["-std=gnu11", "-Wall", "-Wextra", "-Werror", "-O1", "-pthread"]
    with tempfile.TemporaryDirectory(prefix="site-chain-") as d:
        tmp = Path(d)
        stand = tmp / "stand"   # (a64.h's stand-in: the lifted code's own headers exist only after a lift)
        stand.mkdir()
        (stand / "a64.h").write_text(A64_H, encoding="utf-8")
        flags += ["-I", str(stand)]
        # (the real list: every site of game/sites.def, its engine callbacks declared by sites.h)
        real = subprocess.run([a.cc, *flags, "-fsyntax-only", "-I", str(GAME), str(GAME / "sites.c")], text=True,
                              capture_output=True)
        if real.returncode:
            raise SystemExit("FAIL: game/sites.c does not compile with game/sites.def:\n" + real.stderr)
        for f in ("sites.c", "sites.h", "phases.h"):
            shutil.copy(GAME / f, tmp / f)
        (tmp / "sites.def").write_text(SITES_DEF, encoding="utf-8")
        (tmp / "test.c").write_text(TEST, encoding="utf-8")
        binary = tmp / "chain"
        build = subprocess.run([a.cc, *flags, "-I", str(tmp), "-Dfree=test_free", str(tmp / "sites.c"), str(tmp / "test.c"),
                                "-o", str(binary)], text=True, capture_output=True)
        if build.returncode:
            raise SystemExit("FAIL: the stand-in build:\n" + build.stderr)
        run = subprocess.run([str(binary)], text=True, capture_output=True, timeout=30)
        sys.stdout.write(run.stdout)
        if run.returncode or "all passed" not in run.stdout:
            raise SystemExit("FAIL: game/sites.c's handler chains (above)")
    print("test_site_chain: ok")


if __name__ == "__main__":
    main()
