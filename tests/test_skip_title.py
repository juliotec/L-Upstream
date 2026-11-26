"""The Insert menu's Skip title screen row, and the host that carries it out.

The row (gpu/vk/vk_menu.cpp) is Off by default and saves as skip_title_screen; the renderer answers HOS_OPT_SKIP_TITLE
(game/options.h) with it. game/oot_autoload.c finds the newest progress.sav under user/save/<profile>/slot_* and taps A
through the title only while the option is on, a save exists, the Insert menu is closed, the player is not yet running,
and 120 s have not passed. On the supported build (the sites menu.buttonWait and menu.buttonWaitNoDecideAnim) it presses
only as the game waits for a button of Title_00, SystemLoadList_00 or SystemSaveLoad_00, and no more once the game has
begun the load after the last one's press; on a build without them, every 2 s from 20 s, and when no wait on those screens
is told by 45 s, every 2 s from then, until one is. The host is compiled and run against a temporary save tree whose slots have
known mtimes, and with fakes for the clock, the option, the player, the sites and the tap it sends; the sites' callback is
cut out of game/oot_sites.c and run over made-up guest memory.

    python -I -B tests/test_skip_title.py [--cc gcc]
"""
import argparse
import os
import re
import subprocess
import tempfile
import time
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves the check's own folder off the path)
from menu_model_fixture import GAME_OPTION_ANSWER, game_row, menu_text  # noqa: E402

RUN_ALL_ARGS = ["--cc", "gcc"]

ROOT = Path(__file__).resolve().parents[1]
MENU = ROOT / "gpu" / "vk" / "vk_menu.cpp"
HEADER = ROOT / "gpu" / "vk" / "vk_menu.h"
REPLAY = ROOT / "gpu" / "vk" / "vk_replay.cpp"
API = ROOT / "gpu" / "vk" / "vk_api.h"
OPTIONS = ROOT / "game" / "options.h"   # (the game's option ids, which vk_api.h includes)
AUTOLOAD = ROOT / "game" / "oot_autoload.c"

HARNESS = r"""
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include "oot_autoload.h"
#include "vk_api.h"

static int64_t fake_now_ns;
static int option, menu, player, taps;
static int64_t tap_at[128];
const char *hos_user_dir = "";
const char *hos_saves_root(void) { return hos_user_dir; }   /* (host/nn_fs.c: no profile here) */

int64_t hos_now_ns(void) { return fake_now_ns; }
int rx_option(int id, int otherwise) {
    if (id == HOS_OPT_SKIP_TITLE) return option;
    if (id == 1) return menu;
    return otherwise;
}
int hos_oot_player_seen(void) { return player; }
static int loading;   /* (the game's Sequence_IsLoadingBanc: game/oot_tools.c hos_oot_loading) */
int hos_oot_loading(void) { return loading; }
static int sites;   /* (whether the build has the menu.buttonWait sites: game/oot_sites.c) */
int hos_oot_button_wait_sites(void) { return sites; }
volatile int64_t hos_present_count;
void hos_hid_tap(const char *presses) {
    (void)presses;
    if (taps < (int)(sizeof tap_at / sizeof tap_at[0])) tap_at[taps++] = fake_now_ns / 1000000;
}
void hos_log(const char *fmt, ...) { (void)fmt; }

static int failures;
#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL %d: %s\n", __LINE__, #x); failures++; } } while (0)

static void at(int64_t ms) { fake_now_ns = ms * 1000000; }

int main(int argc, char **argv) {
    CHECK(argc == 3);
    if (argc != 3) return 2;

    /* the newest save, by progress.sav's mtime, not the slot's name */
    char path[260] = "";
    CHECK(hos_oot_newest_save(argv[1], path, sizeof path) > 0);
    CHECK(strstr(path, "slot_02") != NULL);
    CHECK(hos_oot_newest_save(argv[2], path, sizeof path) == 0);

    /* off: no press, now or later */
    hos_user_dir = argv[1], option = 0, menu = player = 0;
    hos_oot_autoload_reset(), taps = 0;
    at(0), game_autoload_input();
    at(30000), game_autoload_input();
    CHECK(taps == 0);

    /* on: the title gets A at 20 s, then every 2 s; the Insert menu pauses it */
    option = 1, hos_oot_autoload_reset(), taps = 0;
    at(0), game_autoload_input();
    at(19999), game_autoload_input();
    CHECK(taps == 0);
    at(20000), game_autoload_input();
    CHECK(taps == 1 && tap_at[0] == 20000);
    at(21999), game_autoload_input();
    CHECK(taps == 1);
    at(22000), game_autoload_input();
    CHECK(taps == 2 && tap_at[1] == 22000);
    menu = 1, at(24000), game_autoload_input();
    CHECK(taps == 2);
    menu = 0, at(24000), game_autoload_input();
    CHECK(taps == 3);

    /* the player runs: loaded, and nothing more is pressed */
    player = 1, at(26000), game_autoload_input();
    CHECK(taps == 3);
    at(28000), game_autoload_input();
    CHECK(taps == 3);

    /* turned off while waiting: it stops */
    option = 1, player = 0, hos_oot_autoload_reset(), taps = 0;
    at(0), game_autoload_input();
    at(20000), game_autoload_input();
    CHECK(taps == 1);
    option = 0, at(20001), game_autoload_input();
    at(22000), game_autoload_input();
    CHECK(taps == 1);

    /* no save yet: nothing is pressed, so a new game can still start */
    option = 1, hos_user_dir = argv[2], hos_oot_autoload_reset(), taps = 0;
    at(0), game_autoload_input();
    at(30000), game_autoload_input();
    CHECK(taps == 0);

    /* the give-up time */
    hos_user_dir = argv[1], option = 1, hos_oot_autoload_reset(), taps = 0;
    at(0), game_autoload_input();
    at(120000), game_autoload_input();
    CHECK(taps == 0);

    /* ---- the hook path (the build has the sites): A only as the game waits on the title, its list and its check ---- */
    sites = 1;

    /* off: the sites read nothing, a wait changes nothing, and nothing is pressed (as without the sites) */
    hos_user_dir = argv[1], option = 0, player = 0, hos_oot_autoload_reset(), taps = 0;
    at(0), game_autoload_input();
    CHECK(!hos_oot_autoload_watching());
    hos_oot_autoload_wait("Title_00");
    at(30000), game_autoload_input();
    CHECK(taps == 0);

    /* no save yet: the title stays, and the sites stop reading */
    option = 1, hos_user_dir = argv[2], hos_oot_autoload_reset(), taps = 0;
    at(0), game_autoload_input();
    CHECK(!hos_oot_autoload_watching());
    hos_oot_autoload_wait("Title_00");
    at(5000), game_autoload_input();
    CHECK(taps == 0);

    /* on: nothing until the game waits; then 300 ms after each wait, every 600 ms for 4, every 2 s after */
    hos_user_dir = argv[1], option = 1, hos_oot_autoload_reset(), taps = 0;
    CHECK(hos_oot_autoload_watching());   /* (a wait before the first controller read is heard too) */
    at(0), game_autoload_input();
    at(30000), game_autoload_input();
    CHECK(taps == 0);   /* (no blind presses at 20 s on this path) */
    at(31000), hos_oot_autoload_wait("Title_00");
    at(31299), game_autoload_input();
    CHECK(taps == 0);
    at(31300), game_autoload_input();
    CHECK(taps == 1 && tap_at[0] == 31300);
    at(31899), game_autoload_input();
    CHECK(taps == 1);
    at(31900), game_autoload_input();
    at(32500), game_autoload_input();
    at(33100), game_autoload_input();
    CHECK(taps == 4);
    at(33700), game_autoload_input();
    CHECK(taps == 4);   /* (after 4, every 2 s) */
    at(35100), game_autoload_input();
    CHECK(taps == 5 && tap_at[4] == 35100);
    /* the Insert menu open: no press */
    menu = 1, at(40000), game_autoload_input();
    CHECK(taps == 5);
    menu = 0;
    /* a wait on another screen: nothing pressed while it is the latest */
    at(41000), hos_oot_autoload_wait("ConfirmationNewGame_00");
    at(45000), game_autoload_input();
    CHECK(taps == 5);
    /* the list's wait, then the check's: each its own 300 ms */
    at(46000), hos_oot_autoload_wait("SystemLoadList_00");
    at(46200), game_autoload_input();
    CHECK(taps == 5);
    at(46300), game_autoload_input();
    CHECK(taps == 6);
    at(46500), hos_oot_autoload_wait("SystemSaveLoad_00");
    at(46700), game_autoload_input();
    CHECK(taps == 6);
    at(46800), game_autoload_input();
    CHECK(taps == 7 && tap_at[6] == 46800);
    /* loaded: nothing more, and the sites stop reading */
    player = 1, at(48000), game_autoload_input();
    CHECK(!hos_oot_autoload_watching());
    player = 0, at(50000), game_autoload_input();
    CHECK(taps == 7);

    /* a press that leads back (the check's No): the 4th wait on one screen ends the presses */
    hos_oot_autoload_reset(), taps = 0;
    at(0), game_autoload_input();
    for (int i = 0; i < 3; i++) {
        at(1000 + 1000 * i), hos_oot_autoload_wait("SystemSaveLoad_00");
        at(1300 + 1000 * i), game_autoload_input();
    }
    CHECK(taps == 3);
    at(4000), hos_oot_autoload_wait("SystemSaveLoad_00");
    at(4300), game_autoload_input();
    at(9000), game_autoload_input();
    CHECK(taps == 3);
    CHECK(!hos_oot_autoload_watching());

    /* the fallback: no wait told by 45 s (the sites placed nowhere this run reaches): A every 2 s from then */
    hos_oot_autoload_reset(), taps = 0;
    at(0), game_autoload_input();
    at(44999), game_autoload_input();
    CHECK(taps == 0);
    at(45000), game_autoload_input();
    at(45000), game_autoload_input();
    CHECK(taps == 1 && tap_at[0] == 45000);
    at(47000), game_autoload_input();
    CHECK(taps == 2);
    /* and a wait told after it: the hook path again, 300 ms after the wait, not the clock's 2 s */
    at(48000), hos_oot_autoload_wait("Title_00");
    at(48299), game_autoload_input();
    CHECK(taps == 2);
    at(48300), game_autoload_input();
    CHECK(taps == 3 && tap_at[2] == 48300);
    at(48899), game_autoload_input();
    CHECK(taps == 3);
    at(48900), game_autoload_input();
    CHECK(taps == 4 && tap_at[3] == 48900);   /* (the wait's 600 ms, not the fallback's 2 s from 47000) */

    /* the fallback too when every wait told so far was on a screen it does not press on */
    hos_oot_autoload_reset(), taps = 0;
    at(0), game_autoload_input();
    at(1000), hos_oot_autoload_wait("ConfirmationNewGame_00");
    at(44999), game_autoload_input();
    CHECK(taps == 0);
    at(45000), game_autoload_input();
    CHECK(taps == 1 && tap_at[0] == 45000);

    /* the game took the check's A and the load began: no more presses (they would land on the load and the spawn) */
    hos_oot_autoload_reset(), taps = 0, loading = 1;
    at(0), game_autoload_input();
    at(1000), hos_oot_autoload_wait("Title_00");
    at(1300), game_autoload_input();
    CHECK(taps == 1);   /* (a load before the check's press stops nothing: the title may load too) */
    loading = 0;
    at(2000), hos_oot_autoload_wait("SystemSaveLoad_00");
    at(2300), game_autoload_input();
    CHECK(taps == 2);
    at(2900), game_autoload_input();
    CHECK(taps == 3);   /* (not loading yet: the press may not have been taken, so again) */
    loading = 1, at(3500), game_autoload_input();
    at(9000), game_autoload_input();
    CHECK(taps == 3);
    CHECK(hos_oot_autoload_watching());   /* (still waiting for the player, to log the load) */
    loading = 0, player = 1, at(12000), game_autoload_input();
    CHECK(!hos_oot_autoload_watching());
    player = 0;

    /* the loading flag already up when the check waits (kokiri-start's run): the check's A comes 300 ms after its wait and
       is held 200 ms, and the game may not take it; a flag up before the press tells nothing of it, so the A comes again,
       and only a load that begins after the press (the flag seen down since, then up once the A is released) ends them */
    hos_oot_autoload_reset(), taps = 0, loading = 1, player = 0;
    at(0), game_autoload_input();
    at(1000), hos_oot_autoload_wait("SystemSaveLoad_00");
    at(1299), game_autoload_input();
    CHECK(taps == 0);
    at(1300), game_autoload_input();
    CHECK(taps == 1);
    at(1316), game_autoload_input();   /* (the next reads, the A not delivered yet, then held) */
    at(1500), game_autoload_input();
    at(1900), game_autoload_input();
    CHECK(taps == 2);   /* (the flag was up before the A: again, 600 ms on) */
    loading = 0, at(2200), game_autoload_input();   /* (down after the A: the game took it) */
    loading = 1, at(2400), game_autoload_input();   /* (and up: the load it began) */
    at(9000), game_autoload_input();
    CHECK(taps == 2);
    /* a load that begins while the A is still held: the presses end once it is released, not before */
    hos_oot_autoload_reset(), taps = 0, loading = 0;
    at(0), game_autoload_input();
    at(1000), hos_oot_autoload_wait("SystemSaveLoad_00");
    at(1300), game_autoload_input();
    CHECK(taps == 1);
    at(1316), game_autoload_input();   /* (down after the press) */
    loading = 1, at(1400), game_autoload_input();
    at(1600), game_autoload_input();
    at(9000), game_autoload_input();
    CHECK(taps == 1);

    return failures ? 1 : 0;
}
"""


def check_menu():
    menu = menu_text()   # (vk_menu.cpp and its modules' rows, #1633)
    header = HEADER.read_text(encoding="utf-8")
    api = API.read_text(encoding="utf-8")
    replay = REPLAY.read_text(encoding="utf-8")
    # (the row is the game's, game/menu/rows.c: kept by its option id, Off out of the box, a restart's)
    row = game_row("skip_title_screen")
    if not row or (row["label"], row["tab"], row["kind"], row["option"], row["lo"], row["hi"], row.get("restart")) != (
            '"Skip title screen"', '"gameplay"', "GAME_MENU_TOGGLE", "HOS_OPT_SKIP_TITLE", "0", "1", "1"):
        raise SystemExit("game/menu/rows.c: no Gameplay row for the Skip title screen option")
    if row["default_value"] != "0":
        raise SystemExit("game/menu/rows.c: Skip title screen is not Off by default")
    if "skip_title" in header:
        raise SystemExit("vk_menu.h: Options keeps a skip_title of its own beside the game's row")
    if "enum { HOS_OPT_HDR_NATIVE = 40, HOS_OPT_HDR_PEAK, HOS_OPT_HDR_WHITE };" not in api:
        raise SystemExit("vk_api.h: the HDR option ids are not 40 to 42")
    if "enum { HOS_OPT_DISCORD = 43, HOS_OPT_SKIP_TITLE = 44 };" not in OPTIONS.read_text(encoding="utf-8"):
        raise SystemExit("game/options.h: HOS_OPT_SKIP_TITLE is not the next option id (44, after Discord's 43)")
    rows_cpp = (ROOT / "gpu" / "vk" / "menu" / "game_rows.cpp").read_text(encoding="utf-8")
    if GAME_OPTION_ANSWER not in replay or "row_of[id]->kind == GAME_MENU_TOGGLE ? *v != 0" not in rows_cpp:
        raise SystemExit("vk_replay.cpp: the renderer does not answer HOS_OPT_SKIP_TITLE, 0 or 1")
    renderer_menu = "\n".join(p.read_text(encoding="utf-8") for p in [ROOT / "gpu" / "vk" / "vk_menu.cpp"] +
                              sorted((ROOT / "gpu" / "vk" / "menu").glob("*.cpp")))
    if renderer_menu.count("HOS_OPT_SKIP_TITLE") or header.count("HOS_OPT_SKIP_TITLE"):
        raise SystemExit("the renderer's menu should keep the game row's value by its id generically, naming no HOS_OPT_SKIP_TITLE")
    if "| | Skip title screen |" not in (ROOT / "docs" / "user" / "graphics-and-controls.md").read_text(encoding="utf-8"):
        raise SystemExit("docs/user/graphics-and-controls.md: the menu's Skip title screen row is not in the settings table")


BUILD = "e0f3ccfaf50065ae4ea01eb5aeb13b1a45da3348"
SITES = (("menu.buttonWait", "f_832e8ad0"), ("menu.buttonWaitNoDecideAnim", "f_832e8d70"))

# The sites' callback (game/oot_sites.c), cut out and run over a made-up guest memory: the node's ScreenName input at +0xe8 (a
# tagged pointer to the name's pointer), read only while Skip title waits for the title
CALLBACK = r"""
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#define A64_NOESC
typedef struct { uint64_t x[32]; } Ctx;
static uint8_t mem[0x1000];
static int watching, reads;
static char told[64];
static uint64_t recomp_load64(Ctx *c, uint64_t a) { uint64_t v; (void)c; reads++; memcpy(&v, mem + a, 8); return v; }
static uint64_t recomp_load8(Ctx *c, uint64_t a) { (void)c; reads++; return mem[a]; }
static void put64(uint64_t a, uint64_t v) { memcpy(mem + a, &v, 8); }
int hos_oot_autoload_watching(void) { return watching; }
void hos_oot_autoload_wait(const char *screen) { snprintf(told, sizeof told, "%s", screen); }
#define OOT_BUTTON_WAIT_SCREEN 0xe8
/* FUNCTION */
#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL %d: %s\n", __LINE__, #x); return 1; } } while (0)
int main(void) {
    Ctx c;
    memset(&c, 0, sizeof c);
    c.x[0] = 0x100;
    put64(0x100 + 0xe8, 0x400 | 2);   /* (the input's tag bits) */
    put64(0x400, 0x600);
    memcpy(mem + 0x600, "SystemLoadList_00", 18);
    watching = 0;
    CHECK(hos_site_menu_button_wait(&c) == 0 && reads == 0 && !told[0]);
    watching = 1;
    CHECK(hos_site_menu_button_wait(&c) == 0 && !strcmp(told, "SystemLoadList_00"));
    memset(mem + 0x600, 'x', 200);   /* (a name too long is cut, not overrun) */
    CHECK(hos_site_menu_button_wait(&c) == 0 && strlen(told) == 47);
    put64(0x100 + 0xe8, 0);   /* (no input: told an empty name) */
    CHECK(hos_site_menu_button_wait(&c) == 0 && told[0] == 0);
    puts("PASS");
    return 0;
}
"""


def block(text, marker):
    start = text.index(marker)
    end = text.index("{", start) + 1
    depth = 1
    while depth:
        depth += (text[end] == "{") - (text[end] == "}")
        end += 1
    return text[start:end]


def check_sites(cc, tmp, extra):
    """The two sites: entries of the button waits' enter in the supported build's table, engine sites calling the one callback,
    which only looks (the function goes on), on every user of the skip (the row, the A/B bench, the game tests: one entry)"""
    sys.path.insert(0, str(ROOT / "tests"))
    import sitecheck
    table = {x.name: x for x in sitecheck.load(BUILD)}
    sites_def = (ROOT / "game" / "sites.def").read_text(encoding="utf-8")
    for name, function in SITES:
        x = table.get(name)
        if x is None or (x.at, x.function, bool(x.can_return), x.shape) != ("entry", function, False, "event"):
            raise SystemExit("%s is not an event at the entry of %s" % (name, function))
        if '"%s", hos_site_menu_button_wait)' % name not in sites_def or "HOS_SITE_ENGINE(%s," % sitecheck.ident(name) not in sites_def:
            raise SystemExit("game/sites.def: %s is not an engine site calling hos_site_menu_button_wait" % name)
        if not x.guards or x.guards[0][0] != 0x24:
            raise SystemExit("%s: the ScreenName loads the callback repeats are not its guard" % name)
    sites_c = (ROOT / "game" / "oot_sites.c").read_text(encoding="utf-8")
    fn = block(sites_c, "int hos_site_menu_button_wait(Ctx *A64_NOESC c) {")
    if "return 0;" not in fn or "return 1" in fn:
        raise SystemExit("hos_site_menu_button_wait answers: the sites only look")
    known = block(sites_c, "int hos_oot_button_wait_sites(void) {")
    if "OOT_SITES_BUILD" not in known or "menu.buttonWaitNoDecideAnim" not in known:
        raise SystemExit("hos_oot_button_wait_sites does not ask for this build and both sites")
    if '#define OOT_SITES_BUILD "%s"' % BUILD not in sites_c:
        raise SystemExit("game/oot_sites.c: the callback's offset is not named as build e0f3ccfa's")
    autoload = AUTOLOAD.read_text(encoding="utf-8")
    entry = block(autoload, "void game_autoload_input(void) {")
    if "hooked = hos_oot_button_wait_sites();" not in entry or "skip_title_on()" not in entry:
        raise SystemExit("the one entry (the row, the bench) does not take the hook path when the build has it")
    src = Path(tmp) / "button_wait.c"
    exe = Path(tmp) / "button_wait"
    src.write_text(CALLBACK.replace("/* FUNCTION */", fn), encoding="utf-8")
    build = subprocess.run([cc, "-std=c11", "-Wall", "-Wextra", "-Werror", str(src), "-o", str(exe)], **extra)
    if build.returncode:
        raise SystemExit("the sites' callback did not compile: see the compiler's output above")
    run = subprocess.run([str(exe)], capture_output=True, text=True, **extra)
    if run.returncode or run.stdout.strip() != "PASS":
        raise SystemExit("the sites' callback did not behave:\n" + run.stdout + run.stderr)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default=os.environ.get("CC", "gcc"))
    a = p.parse_args()
    check_menu()
    with tempfile.TemporaryDirectory(prefix="skip-title-") as tmp:
        root = Path(tmp).resolve()
        profile = root / "user" / "save" / ("0" * 32)
        for slot, age in (("slot_00", 30), ("slot_01", 60), ("slot_02", 10)):
            d = profile / slot
            d.mkdir(parents=True)
            (d / "progress.sav").write_bytes(b"save")
            when = time.time() - age
            os.utime(d / "progress.sav", (when, when))
        empty = root / "empty"
        (empty / "user" / "save" / ("0" * 32) / "slot_00").mkdir(parents=True)
        src = root / "skip_title_test.c"
        exe = root / "skip_title_test"
        src.write_text(HARNESS, encoding="utf-8")
        # (host/game_host.h's part oot_autoload.c uses, the option ids (gpu/vk/vk_api.h, game/options.h): the whole of it
        # needs the lift's runtime)
        (root / "game_host.h").write_text('#include "vk_api.h"\n', encoding="utf-8")
        windows = os.name == "nt"
        flags = subprocess.CREATE_NO_WINDOW | subprocess.BELOW_NORMAL_PRIORITY_CLASS if windows else 0
        extra = {"creationflags": flags} if windows else {}
        build = subprocess.run([a.cc, "-std=c11", "-Wall", "-Wextra", "-Werror", "-D_CRT_SECURE_NO_WARNINGS", "-I", str(root),
                                "-I", str(ROOT / "host"), "-I", str(ROOT / "game"), "-I", str(ROOT / "gpu" / "vk"),
                                str(src), str(AUTOLOAD), "-o", str(exe)], **extra)
        if build.returncode:
            raise SystemExit("the host did not compile: see the compiler's output above")
        run = subprocess.run([str(exe), str(root / "user"), str(empty / "user")], capture_output=True, text=True, **extra)
        print(run.stdout, end="")
        print(run.stderr, end="")
        if run.returncode:
            raise SystemExit("the Skip title screen host check failed")
        check_sites(a.cc, tmp, extra)
    print("PASS: Skip title screen is the menu's own option, the host presses only when it should (as the game waits on "
          "the title, its list and its check where the build has menu.buttonWait, by the clock where not), and the sites "
          "only look")


if __name__ == "__main__":
    main()
