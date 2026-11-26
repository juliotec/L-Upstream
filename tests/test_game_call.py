"""game_call (host/game_call.c), the one way host code calls the game's functions, without a game.

tests/game_call_test.c builds game_call.c as it is with a hos.h of this check's (a context with the generated runtime's poll,
a guest thread or none, the virtual-core calls logging their order, a64_lookup handing out callees that record what they
saw) and checks: a thread that gave its core up takes it back, calls, and gives it up again, one holding its core does
neither; the callee sees the stack pointer aligned down to 16, LR 0 and the arguments in x0..x7; *ret is its x0; the
context is put back whole, except that a core request pending before the call or asked during it leaves poll set; and the
refusals (no function, 9 or a negative count of arguments, none given, no context, another context, not a guest thread) call
nothing.
game_call_more gives x8 and LR when asked, hands back s0 and s1, and puts the context back the same way.
Then host/hos_thread.c's hos_vcore_held, hos_vcore_away and hos_vcore_back, cut from it, over a made-up thread: virtual
cores off, not a guest thread, a core held, given up, escaped, escaped and then given up.

python -I -B tests/test_game_call.py [--cc gcc]
"""
import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "gcc"]

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(Path(__file__).resolve().parent))   # (tests/srccut.py: python -I leaves this folder off the path)
from srccut import block   # noqa: E402

HOS_H = r'''#pragma once
#include <stdint.h>
typedef struct GuestContext {   /* (builder/rt/a64.h's, with its poll; tail: whatever follows it) */
    uint64_t x[32];
    uint64_t pc;
    uint8_t n, z, c, v;
    int halted;
    uint64_t vreg[32][2];
    uint64_t tpidr_el0, tpidrro_el0;
    void *thread;
    uint32_t poll;
    int tail;
} GuestContext;
typedef GuestContext Ctx;
typedef void (*A64Fn)(Ctx *c);
A64Fn a64_lookup(uint64_t fn);
void *hos_self(void);
Ctx *hos_ctx(void);
int hos_vcore_held(void);
void hos_vcore_back(void);
int hos_vcore_away(void);
int hos_vcore_kicked(void);
void hos_log(const char *fmt, ...);
'''

VCORE_HEAD = r'''typedef struct HosThread { int vc_held, vc_escaped; int64_t away_at, blocked_total; volatile int vc_kick; } HosThread;
static _Thread_local HosThread *cur;
static int vcores, hos_fps_timing;
static int hos_vcores_on(void) { return vcores; }
static int64_t hos_now_ns(void) { return 1; }
static void vc_release(HosThread *t) { t->vc_held = 0; }
static void vc_acquire(HosThread *t) { t->vc_held = 1; }
'''


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--cc", default="gcc")
    a = p.parse_args()
    thread = (ROOT / "host" / "hos_thread.c").read_text(encoding="utf-8")
    # (the first of each: the virtual cores' own, not the stand-ins of a build without them, after #else)
    cut = "\n".join(block(thread, head, unique=False) for head in ("int hos_vcore_away(void) {", "void hos_vcore_back(void) {",
                                                      "int hos_vcore_held(void) {"))
    with tempfile.TemporaryDirectory(prefix="game-call-") as tmp:
        tmp = Path(tmp)
        (tmp / "hos.h").write_text(HOS_H, encoding="utf-8")
        (tmp / "game_call_vcore.h").write_text(VCORE_HEAD + cut.replace("hos_vcore_", "cut_vcore_") + "\n", encoding="utf-8")
        for name in ("game_call.c", "game_call.h"):   # (game_call.c's "hos.h" is the one above)
            (tmp / name).write_text((ROOT / "host" / name).read_text(encoding="utf-8"), encoding="utf-8")
        exe = tmp / ("game_call_test.exe" if os.name == "nt" else "game_call_test")
        subprocess.run([a.cc, "-std=c11", "-O1", "-g", "-Wall", "-Wextra", "-Werror", "-I", str(tmp),
                        str(ROOT / "tests" / "game_call_test.c"), str(tmp / "game_call.c"), "-o", str(exe)], check=True)
        r = subprocess.run([str(exe)], capture_output=True, text=True, timeout=60)
    print(r.stdout.strip())
    if r.returncode:
        raise SystemExit("FAIL: tests/game_call_test.c exited %d\n%s" % (r.returncode, r.stderr))


if __name__ == "__main__":
    main()
