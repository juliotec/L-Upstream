/* (longshot) The hook sites' flags and callback slots, and the data rules' values and tables (game/sites.h has what
   they are; docs/dev/reference/hooks.md the whole). */
#include "sites.h"
#include "phases.h"

#include <stdlib.h>
#include <string.h>

int hos_site_none(Ctx *A64_NOESC c) { (void)c; return 0; }

/* (a replaced bl's hook stores the function the lift calls there before the callback: sites.h) */
_Thread_local uint64_t hos_site_callee;

/* ---- several handlers on one site (sites.h hos_site_add) ----
   A site's chain: its added handlers by priority (the earlier added first among equals), then its own callback when it has
   one. Published whole; the dispatcher of each site runs it with the guest context's cursor at its first handler, and
   hos_site_next moves the cursor one on for the call it makes. */
typedef struct {
    int count;
    HosSiteFn fn[HOS_SITE_HANDLERS + 1];
} Chain;

typedef struct {
    HosSiteFn base;   /* (the site's own callback: HOS_SITE_ENGINE's fn or hos_site_set's; NULL: none) */
    int n;            /* (the handlers added, in the order they were) */
    struct { HosSiteFn fn; int priority, handle; } added[HOS_SITE_HANDLERS];
    const Chain *volatile chain;   /* (the dispatcher's, while n > 0; NULL with none: a late dispatch runs base alone) */
    volatile uint32_t readers;     /* (the hooks in the dispatcher now: a removed handler's grace, hos_site_remove_then) */
} State;

typedef struct {
    const Chain *chain;
    int i;
    uint64_t callee;   /* (hos_site_callee as the site's hook stored it: put back before each handler, as a handler's own
                          calls into the game reach other replace sites, which store theirs) */
} Cursor;
/* (each guest context's place in the chain its handler runs in: hos_site_next. The context's, not the OS thread's: a guest
   fiber can resume on another OS thread in the middle of a handler (host/hos_thread.c), and its chain goes on there. A
   context claims a slot at its first chain and keeps it (contexts live as long as their guest threads, and an address used
   again is the same key); only the thread running the context writes its slot's cursor, which is on that run's stack (a
   fiber's stack moves with it). Past PLACES contexts, a chain still runs, but its handlers' hos_site_next answer 0) */
enum { PLACES = 1024 };
static struct {
    Ctx *volatile ctx;
    Cursor *volatile at;
} places[PLACES];

static int place_of(Ctx *c, int claim) {
    unsigned i = (unsigned)(((uint64_t)(uintptr_t)c * 0x9e3779b97f4a7c15ull) >> 54);   /* (10 bits: PLACES) */
    for (unsigned k = 0; k < PLACES; k++, i = (i + 1) % PLACES) {
        Ctx *at = __atomic_load_n(&places[i].ctx, __ATOMIC_ACQUIRE);
        if (at == c) return (int)i;
        if (at) continue;
        if (!claim) return -1;   /* (an empty slot ends the probe: slots are never given back) */
        Ctx *none = NULL;
        if (__atomic_compare_exchange_n(&places[i].ctx, &none, c, 0, __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE)) return (int)i;
        if (none == c) return (int)i;
    }
    return -1;
}

static int run(State *st, Ctx *A64_NOESC c) {
    /* (in, then the chain: seq_cst both, as publish's store and the reclaim's look at the count, so a chain a removal replaced
       is run only by a hook the reclaim sees in) */
    __atomic_add_fetch(&st->readers, 1, __ATOMIC_SEQ_CST);
    const Chain *ch = __atomic_load_n(&st->chain, __ATOMIC_SEQ_CST);
    if (!ch || !ch->count) {   /* (a hook that loaded the dispatcher before the last handler went: the callback alone) */
        const HosSiteFn base = __atomic_load_n(&st->base, __ATOMIC_ACQUIRE);
        int r = base ? base(c) : 0;
        __atomic_sub_fetch(&st->readers, 1, __ATOMIC_RELEASE);
        return r;
    }
    const int p = place_of(c, 1);
    Cursor mine = {ch, 0, hos_site_callee};
    Cursor *const saved = p >= 0 ? __atomic_load_n(&places[p].at, __ATOMIC_RELAXED) : NULL;   /* (a hook inside a
                                                       handler's call runs its own chain, then this one goes on) */
    if (p >= 0) __atomic_store_n(&places[p].at, &mine, __ATOMIC_RELEASE);
    int r = ch->fn[0](c);
    if (p >= 0) __atomic_store_n(&places[p].at, saved, __ATOMIC_RELEASE);
    __atomic_sub_fetch(&st->readers, 1, __ATOMIC_RELEASE);
    return r;
}

int hos_site_next(Ctx *A64_NOESC c) {
    const int p = place_of(c, 0);
    Cursor *const cur = p >= 0 ? __atomic_load_n(&places[p].at, __ATOMIC_ACQUIRE) : NULL;
    if (!cur || cur->i + 1 >= cur->chain->count) return 0;
    const int i = ++cur->i;
    hos_site_callee = cur->callee;
    int r = cur->chain->fn[i](c);
    cur->i = i - 1;
    return r;
}

/* (each site's pair, named as the lifted hook names them; the engine's own sites on from the start: no start-up order. And
   its chain's state and dispatcher. A value's or a table's storage is where the feature that owns it defines it: the
   symbol sites.def names) */
#define HOS_SITE(id, name) volatile uint8_t hos_site_on_##id; HosSiteFn volatile hos_site_fn_##id = hos_site_none; \
    static State state_##id; static int chain_##id(Ctx *A64_NOESC c) { return run(&state_##id, c); }
#define HOS_SITE_ENGINE(id, name, fn) volatile uint8_t hos_site_on_##id = 1; HosSiteFn volatile hos_site_fn_##id = fn; \
    static State state_##id = {.base = fn}; static int chain_##id(Ctx *A64_NOESC c) { return run(&state_##id, c); }
#define HOS_VALUE(id, name, type, symbol)
#define HOS_TABLE(id, name, symbol)
#define HOS_SITE_OUTCOME(id, name)
#include "sites.def"
#undef HOS_SITE
#undef HOS_SITE_ENGINE
#undef HOS_VALUE
#undef HOS_TABLE
#undef HOS_SITE_OUTCOME

typedef struct {
    const char *name;
    volatile uint8_t *on;
    HosSiteFn volatile *fn;
    State *state;
    HosSiteFn dispatch;
} Site;

static const Site sites[] = {
#define HOS_SITE(id, name) {name, &hos_site_on_##id, &hos_site_fn_##id, &state_##id, chain_##id},
#define HOS_SITE_ENGINE(id, name, fn) HOS_SITE(id, name)
#define HOS_VALUE(id, name, type, symbol)
#define HOS_TABLE(id, name, symbol)
#define HOS_SITE_OUTCOME(id, name)
#include "sites.def"
#undef HOS_SITE
#undef HOS_SITE_ENGINE
#undef HOS_VALUE
#undef HOS_TABLE
#undef HOS_SITE_OUTCOME
    {NULL, NULL, NULL, NULL, NULL}
};

static const Site *find(const char *name) {
    for (const Site *s = sites; name && s->name; s++)
        if (!strcmp(s->name, name)) return s;
    return NULL;
}

/* (the sites that change the game's outcome on a worker thread: no callback set at run time, so hos_site_add and
   hos_site_set refuse them; nothing would order a run-time answer with the game's other threads) */
static const char *const outcome_sites[] = {
#define HOS_SITE(id, name)
#define HOS_SITE_ENGINE(id, name, fn)
#define HOS_VALUE(id, name, type, symbol)
#define HOS_TABLE(id, name, symbol)
#define HOS_SITE_OUTCOME(id, name) name,
#include "sites.def"
#undef HOS_SITE
#undef HOS_SITE_ENGINE
#undef HOS_VALUE
#undef HOS_TABLE
#undef HOS_SITE_OUTCOME
    NULL
};

static int outcome(const char *name) {
    for (int i = 0; outcome_sites[i]; i++)
        if (!strcmp(outcome_sites[i], name)) return 1;
    return 0;
}

static volatile long chain_lock;   /* (hos_site_set's, hos_site_add's and hos_site_remove's: the hooks never take it) */
static int last_handle;

/* (under chain_lock) The site's slot and flag as its handlers and own callback now say: its own callback alone, called
   straight from the hook as before; or, with handlers, a new chain behind the dispatcher. The slot first, then the flag,
   so a hook that sees the flag sees this callback or the one before; turned off, the flag first, then the slot calls
   nothing, for a hook that read the flag just before */
static void publish(const Site *s) {
    State *st = s->state;
    if (st->n == 0) {
        __atomic_store_n(&st->chain, NULL, __ATOMIC_SEQ_CST);   /* (a hook still at the dispatcher runs base alone) */
        if (st->base) {
            *s->fn = st->base;
            __atomic_store_n(s->on, 1, __ATOMIC_RELEASE);
        } else {
            __atomic_store_n(s->on, 0, __ATOMIC_RELEASE);
            *s->fn = hos_site_none;
        }
        return;
    }
    Chain *ch = malloc(sizeof *ch);   /* (the one before is kept: a hook may still run it) */
    if (!ch) return;
    int order[HOS_SITE_HANDLERS], n = st->n;
    for (int i = 0; i < n; i++) {   /* (by priority, stable: the earlier added first among equals) */
        int j = i;
        while (j > 0 && st->added[order[j - 1]].priority > st->added[i].priority) order[j] = order[j - 1], j--;
        order[j] = i;
    }
    for (int i = 0; i < n; i++) ch->fn[i] = st->added[order[i]].fn;
    ch->count = n;
    if (st->base) ch->fn[ch->count++] = st->base;
    __atomic_store_n(&st->chain, ch, __ATOMIC_SEQ_CST);
    *s->fn = s->dispatch;
    __atomic_store_n(s->on, 1, __ATOMIC_RELEASE);
}

int hos_site_set(const char *name, HosSiteFn fn) {
    const Site *s = find(name);
    if (!s || outcome(name)) return 0;
    while (__sync_lock_test_and_set(&chain_lock, 1)) {}
    s->state->base = fn;
    publish(s);
    __sync_lock_release(&chain_lock);
    return 1;
}

HosSiteFn hos_site_get(const char *name) {
    const Site *s = find(name);
    return s ? (*s->on ? *s->fn : hos_site_none) : NULL;
}

int hos_site_add(const char *name, HosSiteFn fn, int priority) {
    const Site *s = find(name);
    if (!s || !fn || outcome(name)) return 0;
    int handle = 0;
    while (__sync_lock_test_and_set(&chain_lock, 1)) {}
    State *st = s->state;
    if (st->n < HOS_SITE_HANDLERS) {
        handle = ++last_handle;
        st->added[st->n].fn = fn, st->added[st->n].priority = priority, st->added[st->n].handle = handle;
        st->n++;
        publish(s);
    }
    __sync_lock_release(&chain_lock);
    return handle;
}

static void retire(void *table, volatile uint32_t *readers, void (*done)(void *), void *user);

int hos_site_remove_then(int handle, void (*done)(void *user), void *user) {
    volatile uint32_t *readers = NULL;
    while (__sync_lock_test_and_set(&chain_lock, 1)) {}
    for (const Site *s = sites; handle > 0 && s->name && !readers; s++) {
        State *st = s->state;
        for (int i = 0; i < st->n; i++) {
            if (st->added[i].handle != handle) continue;
            for (int j = i + 1; j < st->n; j++) st->added[j - 1] = st->added[j];
            st->n--;
            publish(s);
            readers = &st->readers;
            break;
        }
    }
    __sync_lock_release(&chain_lock);
    if (readers && done) retire(NULL, readers, done, user);   /* (at the acquire phase, once no hook is in the chain) */
    return readers != NULL;
}

int hos_site_remove(int handle) { return hos_site_remove_then(handle, NULL, NULL); }

int hos_site_count(void) { return (int)(sizeof sites / sizeof sites[0]) - 1; }

const char *hos_site_name(int i) { return i >= 0 && i < hos_site_count() ? sites[i].name : NULL; }

/* ---- data rules: values and tables ---- */
typedef struct {
    const char *name;
    volatile void *at;
    unsigned size;
} Value;

static const Value values[] = {
#define HOS_SITE(id, name)
#define HOS_SITE_ENGINE(id, name, fn)
#define HOS_VALUE(id, name, type, symbol) {name, &symbol, sizeof(type)},
#define HOS_TABLE(id, name, symbol)
#define HOS_SITE_OUTCOME(id, name)
#include "sites.def"
#undef HOS_SITE
#undef HOS_SITE_ENGINE
#undef HOS_VALUE
#undef HOS_TABLE
#undef HOS_SITE_OUTCOME
    {NULL, NULL, 0}
};

int hos_value_set(const char *name, uint64_t v) {
    for (const Value *x = values; name && x->name; x++) {
        if (strcmp(x->name, name)) continue;
        switch (x->size) {   /* (one store, which the hook's one load sees whole) */
        case 1: __atomic_store_n((volatile uint8_t *)x->at, (uint8_t)v, __ATOMIC_RELEASE); break;
        case 2: __atomic_store_n((volatile uint16_t *)x->at, (uint16_t)v, __ATOMIC_RELEASE); break;
        case 4: __atomic_store_n((volatile uint32_t *)x->at, (uint32_t)v, __ATOMIC_RELEASE); break;
        default: __atomic_store_n((volatile uint64_t *)x->at, v, __ATOMIC_RELEASE); break;
        }
        return 1;
    }
    return 0;
}

typedef struct {
    const char *name;
    const void *volatile *at;
    volatile uint32_t *readers;
} Table;

/* (each table's readers: the hooks in it now, hos_table_enter / hos_table_leave) */
#define HOS_SITE(id, name)
#define HOS_SITE_ENGINE(id, name, fn)
#define HOS_VALUE(id, name, type, symbol)
#define HOS_TABLE(id, name, symbol) volatile uint32_t hos_table_readers_##id;
#define HOS_SITE_OUTCOME(id, name)
#include "sites.def"
#undef HOS_SITE
#undef HOS_SITE_ENGINE
#undef HOS_VALUE
#undef HOS_TABLE
#undef HOS_SITE_OUTCOME

static const Table tables[] = {
#define HOS_SITE(id, name)
#define HOS_SITE_ENGINE(id, name, fn)
#define HOS_VALUE(id, name, type, symbol)
#define HOS_TABLE(id, name, symbol) {name, &symbol, &hos_table_readers_##id},
#define HOS_SITE_OUTCOME(id, name)
#include "sites.def"
#undef HOS_SITE
#undef HOS_SITE_ENGINE
#undef HOS_VALUE
#undef HOS_TABLE
#undef HOS_SITE_OUTCOME
    {NULL, NULL, NULL}
};

/* (the tables replaced, each with its table's readers, and the handlers removed with something to let go, each with its
   site's: freed, or let go, once those have been seen at 0 after the replacement or the removal) */
typedef struct {
    void *table;
    volatile uint32_t *readers;
    void (*done)(void *);
    void *user;
} Retired;
static Retired retired[256];
static int nretired;
static volatile long retire_lock;   /* (the publishers' and the reclaim's: the hooks never take it) */
static int reclaim_phase;           /* (the acquire phase's handle, added at the first retirement: under retire_lock) */

static void reclaim_at_phase(void *user, uint64_t frame) {
    (void)user, (void)frame;
    hos_tables_reclaim();
}

void hos_tables_reclaim(void) {
    Retired go[sizeof retired / sizeof retired[0]];
    int n = 0;
    while (__sync_lock_test_and_set(&retire_lock, 1)) {}
    int kept = 0;
    for (int i = 0; i < nretired; i++) {
        /* (seq_cst, as the readers' enter and the publisher's exchange, which this thread's lock orders before it) */
        if (__atomic_load_n(retired[i].readers, __ATOMIC_SEQ_CST)) retired[kept++] = retired[i];
        else go[n++] = retired[i];
    }
    nretired = kept;
    __sync_lock_release(&retire_lock);
    for (int i = 0; i < n; i++) {   /* (outside the lock: a done may remove or publish again) */
        free(go[i].table);
        if (go[i].done) go[i].done(go[i].user);
    }
}

static void retire(void *table, volatile uint32_t *readers, void (*done)(void *), void *user) {
    int add = 0;
    while (__sync_lock_test_and_set(&retire_lock, 1)) {}
    if (nretired < (int)(sizeof retired / sizeof retired[0])) retired[nretired++] = (Retired){table, readers, done, user};
    /* (past 256 waiting for their readers the old table stays allocated, and a done is not called: never let go early) */
    if (!__atomic_load_n(&reclaim_phase, __ATOMIC_RELAXED)) __atomic_store_n(&reclaim_phase, -1, __ATOMIC_RELAXED), add = 1;
    __sync_lock_release(&retire_lock);
    /* (the phase table's own lock is not taken under ours. Order 20: the engine's range, game/phases.h) */
    if (add)   /* (0 when the phase's table is full: the next retirement tries again) */
        __atomic_store_n(&reclaim_phase, game_phase_add(GAME_PHASE_ACQUIRE, 20, reclaim_at_phase, NULL), __ATOMIC_RELAXED);
}

int hos_table_publish(const char *name, const void *table) {
    for (const Table *t = tables; name && t->name; t++) {
        if (strcmp(t->name, name)) continue;
        const void *old = __atomic_exchange_n(t->at, table, __ATOMIC_SEQ_CST);
        if (old && old != table) {   /* (the same table again: still the live one, not retired) */
            retire((void *)old, t->readers, NULL, NULL);
            hos_tables_reclaim();
        }
        return 1;
    }
    return 0;
}
