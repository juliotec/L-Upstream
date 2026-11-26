"""A headless run ends on SIGTERM: a windowless game (HOS_HEADLESS=1) leaves SIGINT and SIGTERM to the system, SDL's own
handlers off before SDL starts anything. With no window nothing reads SDL's quit event, and SDL's handlers turn those
signals into that event: a smoke run under `timeout 150` went on rendering for 83 minutes, until SIGKILL. SDL installs its
handlers when its first subsystem starts (sound, video and the controllers each start its events) and reads
SDL_NO_SIGNAL_HANDLERS only then. In a headless run the first is the sound (the Insert menu's list of outputs at the
renderer's start, gpu/vk/vk_menu.cpp menu_init), before the controllers (host/pad_sdl.c pads_start), which set the hint
too late.

host/hos_sdl.c's hos_sdl_load and hos_sdl_init are cut out and run against a stand-in SDL (SDL3 and SDL2) that installs
its handlers when its events first start, unless the hint says no. Each case is a run of its own (hos_sdl_load loads
once): headless, the first subsystem the sound or the controllers, no handlers; with a window, SDL's handlers stay (the
window's loop reads the quit event and closes the game). No game, no SDL.

    python -I -B tests/test_headless_sigterm.py [--cc gcc]
"""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves the check's own folder off the path)
from srccut import block  # noqa: E402

RUN_ALL_ARGS = ["--cc", "{cc}"]

ROOT = Path(__file__).resolve().parents[1]

FIXTURE = r'''
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "longshot_env.h"

typedef void *HMODULE;
typedef long LONG;
static LONG InterlockedCompareExchange(volatile LONG *p, LONG value, LONG compare) {
    const LONG old = *p;
    if (old == compare) *p = value;
    return old;
}
static LONG InterlockedExchange(volatile LONG *p, LONG value) { const LONG old = *p; *p = value; return old; }
static void Sleep(unsigned ms) { (void)ms; }
static void hos_log(const char *fmt, ...) { (void)fmt; }

static int version;   /* 3, 2, or 0 none (as in hos_sdl.c) */
static struct { void *InitSubSystem, *SetHint, *GetError; } f;
#define CALL(type, fn) ((type)f.fn)

/* ---- the stand-in SDL: its hints, and its signal handlers installed when its events first start ---- */
enum { AUDIO = 0x10, VIDEO = 0x20, GAMEPAD = 0x2000, EVENTS = 0x4000 };
static int sdl_have;   /* the SDL "on the system": 3 or 2 */
static char no_signal_hint[8];
static int events_started, handlers_installed;
static void sdl_set_hint(const char *name, const char *value) {
    if (!strcmp(name, "SDL_NO_SIGNAL_HANDLERS")) snprintf(no_signal_hint, sizeof no_signal_hint, "%s", value ? value : "");
}
static void sdl_init(uint32_t flags) {
    /* (both: sound, video and the controllers imply the events; the events' start reads the hint, once) */
    if ((flags & (AUDIO | VIDEO | GAMEPAD | EVENTS)) && !events_started) {
        events_started = 1;
        handlers_installed = strcmp(no_signal_hint, "1") != 0;
    }
}
static _Bool set_hint3(const char *name, const char *value) { sdl_set_hint(name, value); return 1; }
static int set_hint2(const char *name, const char *value) { sdl_set_hint(name, value); return 1; }
static _Bool init3(uint32_t flags) { sdl_init(flags); return 1; }
static int init2(uint32_t flags) { sdl_init(flags); return 0; }

static HMODULE load_one(const char *dll) {
    static int handle;
    if (sdl_have == 3 && !strcmp(dll, "SDL3.dll")) return &handle;
    if (sdl_have == 2 && !strcmp(dll, "SDL2.dll")) return &handle;
    return NULL;
}
static int get_all(HMODULE m, int three) {
    (void)m;
    f.InitSubSystem = three ? (void *)init3 : (void *)init2;
    f.SetHint = three ? (void *)set_hint3 : (void *)set_hint2;
    return 1;
}

LOAD

INIT

int main(int argc, char **argv) {
    if (argc != 3) return 2;
    sdl_have = atoi(argv[1]);
    const uint32_t first = !strcmp(argv[2], "audio") ? AUDIO : !strcmp(argv[2], "gamepad") ? GAMEPAD : VIDEO | EVENTS;
    if (!hos_sdl_init(first)) { puts("not started"); return 1; }
    if (version != sdl_have) { printf("SDL%d loaded, not SDL%d\n", version, sdl_have); return 1; }
    puts(handlers_installed ? "handlers" : "no handlers");
    return 0;
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cc", default="gcc")
    args = parser.parse_args()
    src = (ROOT / "host" / "hos_sdl.c").read_text(encoding="utf-8")
    load = block(src, "int hos_sdl_load(void) {")
    init = block(src, "int hos_sdl_init(uint32_t subsystems) {")
    failures = []
    with tempfile.TemporaryDirectory(prefix="sdl-headless-signals-") as tmp:
        c, exe = Path(tmp) / "signals.c", Path(tmp) / "signals"
        c.write_text(FIXTURE.replace("LOAD", load).replace("INIT", init), encoding="utf-8")
        subprocess.run([args.cc, "-std=c11", "-Wall", "-Wextra", "-Werror", "-Wno-unused-function",
                        "-I", str(ROOT / "gpu" / "vk" / "compat"), str(c), "-o", str(exe)], check=True)
        cases = [(sdl, first, headless) for sdl in (3, 2) for first in ("audio", "gamepad") for headless in (True, False)]
        cases += [(sdl, "video", False) for sdl in (3, 2)]
        for sdl, first, headless in cases:
            env = {k: v for k, v in os.environ.items() if k not in ("HOS_HEADLESS", "SDL_NO_SIGNAL_HANDLERS")}
            if headless:
                env["HOS_HEADLESS"] = "1"
            out = subprocess.run([str(exe), str(sdl), first], env=env, capture_output=True, text=True)
            got = out.stdout.strip()
            want = "no handlers" if headless else "handlers"
            what = "SDL%d, %s first, %s" % (sdl, first, "headless" if headless else "a window")
            if out.returncode != 0 or got != want:
                failures.append("%s: SDL's SIGINT/SIGTERM handlers %r, want %r%s" % (
                    what, got, want, " (exit %d)" % out.returncode if out.returncode else ""))
            else:
                print("ok: %s: %s" % (what, got))
    if failures:
        for line in failures:
            print("FAIL: " + line)
        raise SystemExit(1)
    print("PASS: a headless run leaves SIGINT and SIGTERM to the system, whichever SDL subsystem starts first")


if __name__ == "__main__":
    main()
