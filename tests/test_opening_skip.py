"""Skip opening: only the new-game story movie and Navi waking Link (game/oot_tools.c).

The host function is cut out of the source and compiled with stubs, so the check needs no game dump. It checks that
Opening_Introduction and Opening_LoadingKokiriForest are the only events skipped, that their timelines
Dm_Kokiri_0000 and Dm_Kokiri_0030 are the only ones not started, that Meet_DekuTree is not cancelled, that the
option defaults off, and that option id 44 stays Skip title's. The request is still sent. The cutscene frame
counter is not moved.

    python -I -B tests/test_opening_skip.py [--cc gcc]
"""
import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))   # (python -I leaves the check's own folder off the path)
from menu_model_fixture import GAME_OPTION_ANSWER, game_row, menu_text  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))
import sitecheck  # noqa: E402


def block(text, marker):
    start = text.index(marker)
    end = text.index("{", start) + 1
    depth = 1
    while depth:
        depth += (text[end] == "{") - (text[end] == "}")
        end += 1
    return text[start:end]


def need(ok, what):
    if not ok:
        raise SystemExit("FAIL: " + what)


HARNESS = r"""
#define _CRT_SECURE_NO_WARNINGS
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
typedef struct { uint64_t x[32]; } Ctx;
static uint8_t mem[0x4000];
static char said[2048];
static int opt;
enum { HOS_OPT_SKIP_OPENING = %d };
static uint64_t recomp_load8(Ctx *c, uint64_t a) { (void)c; return mem[a]; }
static uint64_t recomp_load64(Ctx *c, uint64_t a) {
    uint64_t v; (void)c; memcpy(&v, mem + a, 8); return v;
}
static void recomp_store8(Ctx *c, uint64_t a, uint64_t v) { (void)c; mem[a] = (uint8_t)v; }
static void put64(uint64_t a, uint64_t v) { memcpy(mem + a, &v, 8); }
int rx_option(int id, int otherwise) { return id == HOS_OPT_SKIP_OPENING ? opt : otherwise; }
void hos_log(const char *fmt, ...) {
    va_list ap; va_start(ap, fmt);
    vsnprintf(said + strlen(said), sizeof said - strlen(said), fmt, ap);
    va_end(ap);
    strncat(said, "\n", sizeof said - strlen(said) - 1);
}
#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL %%d: %%s\n", __LINE__, #x); return 1; } } while (0)
/* FUNCTION */
int main(void) {
    Ctx c;
    memset(&c, 0, sizeof c);
    memcpy(mem + 0x100, "Opening_Introduction", 21);
    memcpy(mem + 0x180, "Opening_LoadingKokiriForest", 28);
    memcpy(mem + 0x200, "Meet_DekuTree", 13);
    memcpy(mem + 0x280, "Dm_Kokiri_0000", 14);
    memcpy(mem + 0x300, "Dm_Kokiri_0030", 14);
    memcpy(mem + 0x380, "Dm_Kokiri_0070", 14);
    hos_oot_opening(0, 3);

    opt = 0;
    c.x[1] = 0x1000; put64(0x1000 + 6800, 0x100);
    CHECK(hos_oot_opening(&c, 0) == 0);
    CHECK(said[0] == 0);
    c.x[0] = 0x500; c.x[1] = 0x400; put64(0x400, 0x280); mem[0x500 + 0xf25] = 0xff;
    CHECK(hos_oot_opening(&c, 1) == 0);
    CHECK(mem[0x500 + 0xf25] == 0xff);

    opt = 1;
    c.x[1] = 0x1000; put64(0x1000 + 6800, 0x100);
    CHECK(hos_oot_opening(&c, 0) == 0);
    CHECK(strstr(said, "opening skip: Opening_Introduction request sent\n") != 0);
    said[0] = 0;
    CHECK(hos_oot_opening(&c, 0) == 0);
    CHECK(said[0] == 0);

    c.x[0] = 0x500; c.x[1] = 0x400; put64(0x400, 0x280); mem[0x500 + 0xf25] = 0xff;
    CHECK(hos_oot_opening(&c, 1) == 1);
    CHECK(mem[0x500 + 0xf25] == 0xfe);
    put64(0x400, 0x300); mem[0x500 + 0xf25] = 0xff;
    CHECK(hos_oot_opening(&c, 1) == 1);
    CHECK(mem[0x500 + 0xf25] == 0xfe);
    put64(0x400, 0x380); mem[0x500 + 0xf25] = 0xff;
    CHECK(hos_oot_opening(&c, 1) == 0);
    CHECK(mem[0x500 + 0xf25] == 0xff);

    c.x[0] = 7;
    CHECK(hos_oot_opening(&c, 2) == 1);
    CHECK(c.x[0] == 1);

    c.x[1] = 0x1000; put64(0x1000 + 6800, 0x200);
    CHECK(hos_oot_opening(&c, 0) == 0);
    CHECK(strstr(said, "Meet_DekuTree") == 0);
    c.x[0] = 7;
    CHECK(hos_oot_opening(&c, 2) == 0);
    CHECK(c.x[0] == 7);
    put64(0x400, 0x280); c.x[0] = 0x500; c.x[1] = 0x400; mem[0x500 + 0xf25] = 0xff;
    CHECK(hos_oot_opening(&c, 1) == 0);
    CHECK(mem[0x500 + 0xf25] == 0xff);

    put64(0x1000 + 6800, 0x180);
    c.x[1] = 0x1000;
    CHECK(hos_oot_opening(&c, 0) == 0);
    CHECK(strstr(said, "opening skip: Opening_LoadingKokiriForest request sent\n") != 0);
    put64(0x400, 0x300); c.x[0] = 0x500; c.x[1] = 0x400; mem[0x500 + 0xf25] = 1;
    CHECK(hos_oot_opening(&c, 1) == 1);
    CHECK(mem[0x500 + 0xf25] == 0);

    hos_oot_opening(&c, 3);
    c.x[0] = 7;
    CHECK(hos_oot_opening(&c, 2) == 0);
    CHECK(c.x[0] == 7);

    opt = 0;
    c.x[1] = 0x1000; put64(0x1000 + 6800, 0x100);
    CHECK(hos_oot_opening(&c, 0) == 0);
    opt = 1;
    put64(0x400, 0x280); c.x[0] = 0x500; c.x[1] = 0x400;
    CHECK(hos_oot_opening(&c, 1) == 0);
    puts("PASS");
    return 0;
}
"""


def compile_and_run(fn, cc, opt_id):
    text = HARNESS % opt_id
    text = text.replace("/* FUNCTION */", fn)
    # The %% in the CHECK macro survived one formatting pass as %.
    with tempfile.TemporaryDirectory(prefix="opening-skip-") as tmp:
        src = Path(tmp) / "opening_skip.c"
        exe = Path(tmp) / ("opening_skip.exe" if os.name == "nt" else "opening_skip")
        src.write_text(text, encoding="utf-8")
        cmd = [cc, "-std=c11", "-Wall", "-Wextra", "-Werror", str(src), "-o", str(exe)]
        result = subprocess.run(cmd, capture_output=True, text=True)
        need(result.returncode == 0, "the sliced function did not compile:\n" + result.stderr)
        result = subprocess.run([str(exe)], capture_output=True, text=True)
        need(result.returncode == 0 and result.stdout.strip() == "PASS",
             "the sliced function did not behave:\n" + result.stdout + result.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cc", default="")
    args = parser.parse_args()
    source = (ROOT / "game" / "oot_tools.c").read_text(encoding="utf-8")
    api = (ROOT / "gpu" / "vk" / "vk_api.h").read_text(encoding="utf-8")
    options = (ROOT / "game" / "options.h").read_text(encoding="utf-8")   # (the game's option ids, which vk_api.h includes)
    menu_h = (ROOT / "gpu" / "vk" / "vk_menu.h").read_text(encoding="utf-8")
    menu = menu_text()   # (vk_menu.cpp and its modules' rows, #1633)
    replay = (ROOT / "gpu" / "vk" / "vk_replay.cpp").read_text(encoding="utf-8")

    fn = block(source, "int hos_oot_opening(Ctx *c, int mode) {")
    need('strcmp(name, "Opening_Introduction")' in fn, "Opening_Introduction is not a skipped event")
    need('strcmp(name, "Opening_LoadingKokiriForest")' in fn, "Opening_LoadingKokiriForest is not a skipped event")
    need(fn.count('strcmp(name, "Opening_') == 2, "an opening event besides those two is named")
    need('strcmp(name, "Dm_Kokiri_0000")' in fn and 'strcmp(name, "Dm_Kokiri_0030")' in fn,
         "the two opening timelines are not the ones left unstarted")
    need(fn.count('strcmp(name, "Dm_Kokiri_') == 2, "a timeline besides the two opening movies is named")
    need("Meet_DekuTree" in fn and "is not skipped" in fn, "Meet_DekuTree is not left as a later flow")
    need('strcmp(name, "Meet_DekuTree")' not in fn, "Meet_DekuTree is cancelled with the opening")
    need("return 0;   /* the request is still sent */" in fn, "the opening request is not sent on")
    need("0x2f8" not in fn and "a64_lookup" not in fn, "the skip moves the cutscene frame counter or calls a new guest")
    need("rx_option(HOS_OPT_SKIP_OPENING, 0)" in fn, "the option does not default off when the menu has not answered")
    need("HOS_OPT_SKIP_TITLE" not in fn and "44" not in fn, "the opening skip uses Skip title's id 44")

    # Skip title now occupies its reserved slot: check the declaration, not the obsolete reservation comment.
    need(re.search(r"enum\s*\{\s*HOS_OPT_HDR_NATIVE\s*=\s*40\s*,\s*HOS_OPT_HDR_PEAK\s*,\s*HOS_OPT_HDR_WHITE\s*\};", api)
         and re.search(r"enum\s*\{\s*HOS_OPT_DISCORD\s*=\s*43\s*,\s*HOS_OPT_SKIP_TITLE\s*=\s*44\s*\};", options),
         "Skip title is not option id 44 (game/options.h, after gpu/vk/vk_api.h's HDR ids and Discord's 43)")
    need(re.search(r"enum \{ HOS_OPT_SKIP_OPENING = 52 \};", options), "Skip opening is not option id 52 (game/options.h)")
    need("HOS_OPT_SKIP_OPENING = 44" not in api + options, "Skip opening took id 44")
    need("skip_opening" not in api, "Skip opening added to MkwVkHost or MkwVkRenderer (it is an option id, not an interface change)")

    # (the row is the game's, game/menu/rows.c: kept by its option id, answered by it)
    row = game_row("skip_opening")
    need(row and (row["label"], row["tab"], row["kind"], row["option"], row["lo"], row["hi"]) ==
         ('"Skip opening"', '"gameplay"', "GAME_MENU_TOGGLE", "HOS_OPT_SKIP_OPENING", "0", "1"), "the Gameplay row is missing")
    need(row["default_value"] == "0", "Skip opening does not default off")
    need("skip_opening" not in menu_h, "the renderer's options keep a Skip opening of their own")
    help_at = menu.index('"Skip opening"')
    help_text = menu[help_at:help_at + 400]
    need("story movie and Navi waking Link" in help_text and "Deku Tree" in help_text and "title screen" in help_text,
         "the row's help does not say what is skipped and what is left")
    need(GAME_OPTION_ANSWER in replay, "the option callback does not answer it")
    option = replay.split("t.option =", 1)[1].split("};", 1)[0]
    need(not re.search(r"\bid\s*==\s*44\b", option), "the option callback handles id 44")
    need('getenv("HOS_SKIP_OPENING")' in menu, "a run cannot turn the row on without saving it")

    lift = block(source, "void hos_oot_lift_player(Ctx *c, uint64_t actor) {")
    need("if (!oot_event_playing(c)) hos_oot_opening(c, 3);" in lift,
         "Link updating during the opening drops the skip before the flowchart finishes")

    # (the three places are hook sites now: game/sites/sites.toml, game/oot_sites.c; the lift places them)
    sites = {x.name: x for x in sitecheck.load(sitecheck.BUILD)}
    calls = (ROOT / "game" / "oot_sites.c").read_text(encoding="utf-8")
    for name, function, can_return, body in (
            ("opening.request", "f_80d9a820", False, "hos_oot_opening(c, 0);\n    return 0;"),
            ("opening.timeline", "f_83b5bfb0", True, "return hos_oot_opening(c, 1);"),
            ("opening.skipQuery", "f_80de6cb0", True, "return hos_oot_opening(c, 2);")):
        x = sites.get(name)
        need(x is not None and (x.at, x.function, bool(x.can_return)) == ("entry", function, can_return),
             "%s is not the entry of %s%s" % (name, function, " that can return" if can_return else ""))
        need(sitecheck.host_sites().get(sitecheck.ident(name)) == name, "game/sites.def has no slot for " + name)
        need(body in calls, "%s's callback is not hos_oot_opening as the text patch called it" % name)
    need(sites["opening.timeline"].can_return and not sites["opening.request"].can_return,
         "the timeline's hook does not return when told to, or the request's swallows the request")
    # (Skip title screen's own sites, the button waits' enter, are its alone: the opening's callback names none of them, and
    # theirs reads Skip title's state, not Skip opening's)
    for name in ("menu.buttonWait", "menu.buttonWaitNoDecideAnim"):
        x = sites.get(name)
        need(x is not None and x.at == "entry" and not x.can_return and x.function not in ("f_80d9a820", "f_83b5bfb0", "f_80de6cb0"),
             "%s is not an entry of its own that only looks" % name)
    wait = block(calls, "int hos_site_menu_button_wait(Ctx *A64_NOESC c) {")
    need("hos_oot_autoload_watching()" in wait and "HOS_OPT_SKIP_OPENING" not in wait and "hos_oot_opening" not in wait,
         "the button waits' callback is not Skip title screen's alone")
    need("buttonWait" not in fn, "Skip opening reads the title's button waits")

    cc = args.cc or shutil.which("gcc") or shutil.which("clang") or shutil.which("cc")
    if cc:
        compile_and_run(fn, cc, 52)
        print("PASS: skip opening leaves Meet_DekuTree and id 44, and the sliced function behaves")
    else:
        print("PASS: skip opening leaves Meet_DekuTree and id 44 (no C compiler here for the sliced function)")


if __name__ == "__main__":
    main()
