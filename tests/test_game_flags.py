"""The game's flags (game/flags.c), the one reader and writer of GameDataMgr in host code, without a game.

tests/game_flags_test.c builds a GameDataMgr of its own in two bounded windows of guest memory (the singleton slots and a
heap) and reads it through flags.c as it is, compiled with a hos.h of this check's (hos_guest_range over those windows,
HOS_WRITTEN counted): the hash (MurmurHash3 x86 32, seed 0, against its reference vectors and the names the host's
source gives), every container kind by hash with collisions walked past the table's end, Int signed and UInt and Enum
unsigned, Float finite only, String64 through its pointer and cut to the buffer, a Bool's write marked, Bool64bitKey by
id with the place kept, a container's header and its slots as the changes scan reads them, and every bound (an entry
past the count, a size past the cap, a table or text out of the window, no GameDataMgr) reading as no flag, never a fault.
Then, in the source, that no file of host/ or game/ but the flags service (game/flags.c) keeps GameDataMgr's slot or a
reader of its own.

python -I -B tests/test_game_flags.py [--cc gcc]
"""
import argparse
import os
import re
import subprocess
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "gcc"]

ROOT = Path(__file__).resolve().parents[1]

HOS_H = r'''#pragma once
#include <stdint.h>
#include <string.h>
#define SLOTS 0x84c00000ull
#define HEAP 0x10000000ull
extern unsigned char slots[0x100000], heap[0x100000];
extern int marks;
extern void (*range_hook)(uint64_t a, uint64_t n);   /* (the test's: called at each read, before it) */
static inline void *hos_guest_range(uint64_t a, uint64_t n) {
    if (range_hook) range_hook(a, n);
    if (a >= SLOTS && n <= sizeof slots && a - SLOTS <= sizeof slots - n) return slots + (a - SLOTS);
    if (a >= HEAP && n <= sizeof heap && a - HEAP <= sizeof heap - n) return heap + (a - HEAP);
    return (void *)0;
}
#define HOS_WRITTEN(p, n) ((void)(p), (void)(n), marks++)
'''


def check(cond, what):
    if not cond:
        raise SystemExit("FAIL: " + what)


def source_checks():
    """GameDataMgr's slot and its containers' walk live in game/flags.c alone"""
    paths = [p for d in ("host", "game") for ext in ("*.c", "*.h") for p in sorted((ROOT / d).rglob(ext))]
    for path in paths:
        rel = path.relative_to(ROOT).as_posix()
        if rel in ("game/flags.c", "game/flags.h"):   # (the flags service itself)
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        check("0x84c9e180" not in text, "%s keeps GameDataMgr's slot: read flags through game/flags.h" % rel)
        check(not re.search(r"\b0x8022d9c0|0x80cd0ea0\b", text),
              "%s calls the game's flag getters: read flags through game/flags.h" % rel)
    for name in ("oot_state_tap.c", "oot_voice.c", "oot_heart_containers.c", "oot_tools.c"):
        check(re.search(r'#include "(\.\./game/)?flags\.h"', (ROOT / "game" / name).read_text(encoding="utf-8")),
              "game/%s does not use the flags service" % name)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--cc", default="gcc")
    a = p.parse_args()
    source_checks()
    print("PASS: no host or game file but game/flags.c keeps GameDataMgr's slot or calls the game's flag getters")
    with tempfile.TemporaryDirectory(prefix="game-flags-") as tmp:
        tmp = Path(tmp)
        (tmp / "game").mkdir()
        (tmp / "hos.h").write_text(HOS_H, encoding="utf-8")
        (tmp / "game_host.h").write_text('#include "hos.h"\n', encoding="utf-8")   # (flags.c's host/game_host.h: the hos.h above)
        for name in ("flags.c", "flags.h"):   # (flags.c's "game_host.h" is the one above, found through -I)
            (tmp / "game" / name).write_text((ROOT / "game" / name).read_text(encoding="utf-8"), encoding="utf-8")
        exe = tmp / ("game_flags_test.exe" if os.name == "nt" else "game_flags_test")
        subprocess.run([a.cc, "-std=c11", "-O1", "-g", "-Wall", "-Wextra", "-Werror", "-I", str(tmp),
                        str(ROOT / "tests" / "game_flags_test.c"), str(tmp / "game" / "flags.c"), "-lm", "-o", str(exe)], check=True)
        r = subprocess.run([str(exe)], capture_output=True, text=True, timeout=60)
    print(r.stdout.strip())
    if r.returncode:
        raise SystemExit("FAIL: tests/game_flags_test.c exited %d\n%s" % (r.returncode, r.stderr))


if __name__ == "__main__":
    main()
