"""The setjmp slots of lifted code (host/hos_setjmp.c) are a guest context's, not a host thread's, without a game.

A guest fiber that pushed a setjmp owner on one host thread may be resumed on another (host/hos_thread.c fiber_switch)
and pop it, or longjmp to it, there: the slots must be found from the context on any thread. And a push past the
slots' depth is refused (its buffer matches no return address, and is the process's one, written and never read, so
its pop is safe on any thread after the one that pushed it has ended), never made room for by moving the live slots that
the frames below still hold.

The slots' code (from `#define DEPTH` to the assembly, and recomp_sj_push to recomp_sj_pop) is production code; only
the host's lock and log are made up.
"""
import argparse
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

RUN_ALL_ARGS = ["--cc", "{cxx}"]

ROOT = Path(__file__).resolve().parents[1]
COMPILER = None


def block(text, marker):
    start = text.index(marker)
    end = text.index("{", start) + 1
    depth = 1
    while depth:
        depth += (text[end] == "{") - (text[end] == "}")
        end += 1
    return text[start:end]


FIXTURE = r'''
#define _GNU_SOURCE
#include <pthread.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CHECK(test, message) do { if (!(test)) { fprintf(stderr, "%s\n", message); exit(1); } } while (0)
typedef struct { uint64_t x[32]; } Ctx;
typedef pthread_rwlock_t SRWLOCK;
#define SRWLOCK_INIT PTHREAD_RWLOCK_INITIALIZER
static __attribute__((unused)) void AcquireSRWLockShared(SRWLOCK *l) { pthread_rwlock_rdlock(l); }
static __attribute__((unused)) void ReleaseSRWLockShared(SRWLOCK *l) { pthread_rwlock_unlock(l); }
static __attribute__((unused)) void AcquireSRWLockExclusive(SRWLOCK *l) { pthread_rwlock_wrlock(l); }
static __attribute__((unused)) void ReleaseSRWLockExclusive(SRWLOCK *l) { pthread_rwlock_unlock(l); }
static int logs;
static void hos_log(const char *fmt, ...) { (void)fmt; logs++; }

@SLOTS@

@PUSH_POP@

static Ctx fiber;
static void **pushed;
static void *push_on_a(void *arg) {
    (void)arg;
    pushed = recomp_sj_push(&fiber, 0x81681690, 0, 0, 0);
    return NULL;
}
static void *pop_on_b(void *arg) {
    (void)arg;
    CHECK(hos_setjmp_depth(&fiber) == 1, "the slot pushed on thread A is not the context's on thread B");
    recomp_sj_pop(pushed);
    CHECK(hos_setjmp_depth(&fiber) == 0, "the pop on thread B left the context's slot");
    return NULL;
}

typedef struct { Ctx c; void **over; } Deep;
static Deep deep_a, deep_b;
static void *refuse(void *arg) {
    Deep *d = (Deep *)arg;
    for (int i = 0; i <= DEPTH; i++) d->over = recomp_sj_push(&d->c, 0x1000 + (uint64_t)i, 0, 0, 0);
    return NULL;
}

static SjSlot *slot_of(void **buf) { return (SjSlot *)((char *)buf - offsetof(SjSlot, buf)); }

int main(int argc, char **argv) {
    CHECK(argc == 2, "usage: check <case>");
    if (!strcmp(argv[1], "threads")) {
        pthread_t a, b;
        CHECK(!pthread_create(&a, NULL, push_on_a, NULL) && !pthread_join(a, NULL), "thread A");
        CHECK(!pthread_create(&b, NULL, pop_on_b, NULL) && !pthread_join(b, NULL), "thread B");
        Ctx other;
        recomp_sj_push(&other, 1, 2, 3, 4);
        CHECK(hos_setjmp_depth(&other) == 1 && hos_setjmp_depth(&fiber) == 0, "two contexts share their slots");
        hos_setjmp_forget(&other);
        CHECK(hos_setjmp_depth(&other) == 0, "a forgotten context kept its slots");
        return 0;
    }
    if (!strcmp(argv[1], "deep")) {
        static Ctx c;
        void **bufs[DEPTH];
        for (int i = 0; i < DEPTH; i++) bufs[i] = recomp_sj_push(&c, 0x1000 + (uint64_t)i, 0, 0, 0);
        void **over = recomp_sj_push(&c, 0x9000, 0, 0, 0);
        void **again = recomp_sj_push(&c, 0x9004, 0, 0, 0);
        CHECK(hos_setjmp_depth(&c) == DEPTH, "a push past the depth was taken");
        for (int i = 0; i < DEPTH; i++)
            CHECK(slot_of(bufs[i])->rets[0] == 0x1000 + (uint64_t)i, "a push past the depth moved a live slot");
        CHECK(slot_of(over)->rets[0] != 0x9000 && slot_of(again)->rets[0] != 0x9004,
              "a refused push's slot matches its return address");
        CHECK(logs == 1, "a refused push is not logged once");
        recomp_sj_pop(again);
        recomp_sj_pop(over);
        CHECK(hos_setjmp_depth(&c) == DEPTH, "a refused push's pop dropped slots");
        recomp_sj_pop(bufs[DEPTH - 1]);
        CHECK(hos_setjmp_depth(&c) == DEPTH - 1, "the newest slot's pop");
        return 0;
    }
    if (!strcmp(argv[1], "refused")) {   /* (two threads refused at once, both ended; their pushes popped on this thread) */
        pthread_t a, b;
        CHECK(!pthread_create(&a, NULL, refuse, &deep_a) && !pthread_create(&b, NULL, refuse, &deep_b) &&
              !pthread_join(a, NULL) && !pthread_join(b, NULL), "the refusing threads");
        /* (the threads have ended: a buffer of their own would be freed memory now, and its pop a use after free) */
        CHECK(deep_a.over && deep_a.over == deep_b.over, "a refused push's buffer is not the process's one");
        CHECK(recomp_sj_target(deep_a.over) == 0, "a refused push's buffer has a target");
        recomp_sj_pop(deep_a.over);   /* (a fiber's refused push, popped where it was resumed) */
        recomp_sj_pop(deep_b.over);
        CHECK(hos_setjmp_depth(&deep_a.c) == DEPTH && hos_setjmp_depth(&deep_b.c) == DEPTH,
              "a refused push's pop on another thread dropped slots");
        return 0;
    }
    return 2;
}
'''


class SetjmpContextTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        compiler = COMPILER or shutil.which("gcc") or shutil.which("clang")
        if not compiler:
            raise unittest.SkipTest("no C compiler")
        cls.temporary = tempfile.TemporaryDirectory(prefix="longshot-setjmp-context-")
        cls.addClassCleanup(cls.temporary.cleanup)
        directory = Path(cls.temporary.name)
        production = (ROOT / "host/hos_setjmp.c").read_text(encoding="utf-8")
        slots = production[production.index("#define DEPTH"):production.index("#ifdef _WIN32")]
        push_pop = production[production.index("void **recomp_sj_push("):]
        push_pop = push_pop[:push_pop.index("void recomp_sj_pop(")] + block(push_pop, "void recomp_sj_pop(")
        source = directory / "setjmp_context.c"
        source.write_text(FIXTURE.replace("@SLOTS@", slots).replace("@PUSH_POP@", push_pop), encoding="utf-8")
        cls.executable = directory / "check.exe"
        result = subprocess.run([compiler, "-x", "c", "-std=c11", "-Wall", "-Wextra", "-Werror", "-pthread",
                                 str(source), "-o", str(cls.executable)], capture_output=True, text=True)
        if result.returncode:
            raise AssertionError(result.stdout + result.stderr)

    def run_case(self, name):
        result = subprocess.run([str(self.executable), name], capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_a_context_s_slots_are_found_on_another_host_thread(self):
        self.run_case("threads")

    def test_a_push_past_the_depth_is_refused_and_moves_no_live_slot(self):
        self.run_case("deep")

    def test_a_refused_push_is_popped_safely_after_its_thread_ended(self):
        self.run_case("refused")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cc")
    args, rest = parser.parse_known_args()
    COMPILER = args.cc
    unittest.main(argv=[__file__] + rest)
