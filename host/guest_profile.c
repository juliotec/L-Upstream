/* (longshot) The guest profile's counters (lifter/crates/profile): an instrumented build of the new lifter's units
   (`longshot-builder dev lift --profile-generate`) counts each block's entries and each conditional branch's taken edge
   in one array a unit, and registers it here when the game starts (lsp_register, from the unit's constructor).
   HOS_GUEST_PROFILE=<path>: the counts that ran are written to <path> every 10 s while the game runs and when it closes
   (a run stopped from outside, as dev.py ab stops its games, keeps all but its last seconds), and every 500 ms, if
   <path>.reset exists the counters are cleared, and if <path>.dump exists they are written (each file is deleted when
   done), so a run can leave out the loading. A process whose counters are all 0 (one that runs no lifted code, such as
   the shader preparation's) writes nothing, so it never replaces the game's counts. The text, which `longshot-builder dev profile` reads with the
   units' map: "unit <name>" lines, each followed by "<index> <count>" lines of its counters that are not 0.
   Without HOS_GUEST_PROFILE, or in a build that is not instrumented, this does nothing. */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <windows.h>

#define LSP_UNITS 4096

struct lsp_unit {
    uint64_t *counts;
    uint64_t n;
    const char *name;
};

static struct lsp_unit lsp_units[LSP_UNITS];
static volatile LONG lsp_count;

/* (each unit's constructor, before main: zero-initialised storage, so the order of constructors does not matter) */
void lsp_register(uint64_t *counts, uint64_t n, const char *name) {
    LONG i = InterlockedIncrement(&lsp_count) - 1;
    if (i < LSP_UNITS) {
        lsp_units[i].counts = counts;
        lsp_units[i].n = n;
        lsp_units[i].name = name;
    }
}

static LONG lsp_units_now(void) {
    LONG n = lsp_count;
    return n < LSP_UNITS ? n : LSP_UNITS;
}

/* (one write or reset at a time: the timer thread's and the one at exit wrote the same .tmp at once; the last at exit,
   after which the thread writes nothing) */
static SRWLOCK lsp_lock = SRWLOCK_INIT;
static int lsp_closed;

static void lsp_write_locked(const char *path) {
    char tmp[1024];
    snprintf(tmp, sizeof tmp, "%s.tmp", path);
    FILE *f = fopen(tmp, "wb");
    if (!f) {
        fprintf(stderr, "guest profile: %s cannot be written\n", tmp);
        return;
    }
    uint64_t ran = 0;
    for (LONG u = 0; u < lsp_units_now(); u++)
        for (uint64_t i = 0; i < lsp_units[u].n && !ran; i++)
            ran = lsp_units[u].counts[i] != 0;
    if (!ran) {
        fclose(f);
        remove(tmp);
        return;
    }
    ran = 0;
    for (LONG u = 0; u < lsp_units_now(); u++) {
        const struct lsp_unit *x = &lsp_units[u];
        fprintf(f, "unit %s\n", x->name);
        for (uint64_t i = 0; i < x->n; i++) {
            uint64_t c = x->counts[i];
            if (c) {
                fprintf(f, "%llu %llu\n", (unsigned long long)i, (unsigned long long)c);
                ran++;
            }
        }
    }
    fclose(f);
    remove(path);
    if (rename(tmp, path) != 0)
        fprintf(stderr, "guest profile: %s cannot be written\n", path);
    else
        fprintf(stderr, "guest profile: %llu counters of %ld units written to %s\n", (unsigned long long)ran,
                (long)lsp_units_now(), path);
}

/* the counters to path; last: the write at exit, the last one */
static void lsp_write(const char *path, int last) {
    AcquireSRWLockExclusive(&lsp_lock);
    if (!lsp_closed) lsp_write_locked(path);
    if (last) lsp_closed = 1;
    ReleaseSRWLockExclusive(&lsp_lock);
}

static void lsp_reset(void) {
    AcquireSRWLockExclusive(&lsp_lock);
    for (LONG u = 0; u < lsp_units_now(); u++)
        memset(lsp_units[u].counts, 0, lsp_units[u].n * sizeof(uint64_t));
    ReleaseSRWLockExclusive(&lsp_lock);
    fprintf(stderr, "guest profile: counters cleared\n");
}

static char *lsp_path;

static int lsp_take(const char *suffix) {
    char p[1024];
    snprintf(p, sizeof p, "%s%s", lsp_path, suffix);
    FILE *f = fopen(p, "rb");
    if (!f) return 0;
    fclose(f);
    remove(p);
    return 1;
}

static DWORD WINAPI lsp_thread(void *arg) {
    (void)arg;
    for (unsigned tick = 1;; tick++) {
        Sleep(500);
        if (lsp_take(".reset")) lsp_reset();
        if (lsp_take(".dump") || tick % 20 == 0) lsp_write(lsp_path, 0);
    }
    return 0;
}

static void lsp_at_exit(void) {
    if (lsp_path && lsp_units_now() > 0) lsp_write(lsp_path, 1);
}

__attribute__((constructor)) static void lsp_start(void) {
    const char *e = getenv("HOS_GUEST_PROFILE");
    if (!e || !*e) return;
    lsp_path = _strdup(e);
    atexit(lsp_at_exit);
    CreateThread(NULL, 0, lsp_thread, NULL, 0, NULL);
}
