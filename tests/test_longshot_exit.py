"""The game's side of play (docs/dev/reference/build.md, "Play: the version folder and the profile"): the hand-over of
a game started by itself in a version folder's game folder to the builder two folders up (longshot_handover, checked by
behaviour on made-up paths; hos_main.c's use of it, by source), and the profile's paths and start slot as the game
takes them from LONGSHOT_PROFILE (host/longshot_profile.h: by behaviour, the start slot by launcher_profile's rule;
their use in hos_main.c, nn_fs.c and options_path.c, by source). No game or GPU required.

python -I -B tests/test_longshot_exit.py [--cc gcc]
"""
import argparse
import re
import subprocess
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "gcc"]

ROOT = Path(__file__).resolve().parents[1]

HANDOVER_TEST = r"""
#include "longshot_exit.h"
#include <assert.h>
#include <stdio.h>

static const char *files[8];
static int is_file(const char *p) {
    for (int i = 0; files[i]; i++)
        if (!strcmp(files[i], p)) return 1;
    return 0;
}
static int handover(const char *game, const char *by, int argc, char **argv, char *builder) {
    return longshot_handover(game, by, argc, argv, is_file, "Longshot.x86_64", builder, 4096);
}
static const struct { const char *text; int got, slot; } slots[] = {
    {"{\"v\": 1, \"start_slot\": \"slot_07\"}", 1, 7},
    {"{\"start_slot\": \"SLOT_99\"}", 1, 99},
    {"{\"v\": 1, \"start_slot\": \" slot_05 \"}", 1, 5},
    {"{\"v\": 1, \"start_slot\": \"3\"}", 1, 3},
    {"{\"v\": 1, \"start_slot\": 12}", 1, 12},
    {"\xef\xbb\xbf{\"start_slot\": 0}", 1, 0},
    {"{\"v\": 1, \"start_slot\": null}", 0, 0},
    {"{\"v\": 1, \"name\": \"Ann\"}", 0, 0},
    {"{}", 0, 0},
    {"{\"start_slot\": \"slot_100\"}", -1, 0},
    {"{\"start_slot\": \"slot_\"}", -1, 0},
    {"{\"start_slot\": -1}", -1, 0},
    {"{\"start_slot\": 2.0}", -1, 0},
    {"{\"start_slot\": true}", -1, 0},
    {"{\"start_slot\": [2]}", -1, 0},
    /* (the rest of the file read whole: nested values, escapes, the Mod API's mods, a key it does not know) */
    {"{\"v\":1,\"mods\":[{\"id\":\"a\\\"b\",\"on\":false,\"granted\":[\"network\"]}],\"x\":{\"start_slot\":9},\"start_slot\":\"slot_02\"}", 1, 2},
    {"{\"name\": \"\\u00e9\\n\", \"start_slot\": 4}", 1, 4},
    /* (not a profile it reads: no slot taken) */
    {"[1]", -2, 0},
    {"{\"start_slot\": 1, \"start_slot\": 2}", -2, 0},
    {"{\"v\": 2, \"start_slot\": 1}", -2, 0},
    {"{\"v\": \"1\", \"start_slot\": 1}", -2, 0},
    {"{\"start_slot\": 1", -2, 0},
    {"{\"start_slot\": 1} x", -2, 0},
    {"{\"a\": tru, \"start_slot\": 1}", -2, 0},
    {"", -2, 0},
    {NULL, 0, 0},
};

int main(void) {
    static char builder[4096];
    char *none[] = {"longshot", NULL}, *args[] = {"longshot", "--fullscreen", "", NULL};
    files[0] = "/v/Longshot.x86_64";
    /* (a version folder's game folder, the builder two folders up, started by itself: handed over) */
    assert(handover("/v/builds/default/longshot", NULL, 1, none, builder) && !strcmp(builder, "/v/Longshot.x86_64"));
    assert(handover("/v/builds/ab12/longshot", "", 1, none, builder));
    /* (started by the builder, or the build's own run: never) */
    assert(!handover("/v/builds/default/longshot", "builder", 1, none, builder));
    char *prep[] = {"longshot", "--prepare-shaders", NULL};
    assert(!handover("/v/builds/default/longshot", NULL, 2, prep, builder));
    /* (a folder of no version folder: a developer's, dev.py's, the Test Farm's) */
    assert(!handover("/home/d/game/longshot", NULL, 1, none, builder));
    assert(!handover("/v/build/default/longshot", NULL, 1, none, builder));
    assert(!handover("/w/builds/default/longshot", NULL, 1, none, builder));   /* (no builder there) */
    assert(!handover("longshot", NULL, 1, none, builder) && !handover("/longshot", NULL, 1, none, builder));
    assert(!handover("/builds/default/longshot", NULL, 1, none, builder));
    /* (Windows' separators) */
    files[1] = "C:\\v\\Longshot.x86_64";
    assert(handover("C:\\v\\builds\\default\\longshot.exe", NULL, 1, none, builder) && !strcmp(builder, files[1]));
    /* (a path too long for the buffer: not handed over) */
    assert(!longshot_handover("/v/builds/default/longshot", NULL, 1, none, is_file, "Longshot.x86_64", builder, 8));
    /* (the builder's command line: play, then the game's arguments after --, an empty one kept) */
    char **v = longshot_handover_argv("/v/Longshot.x86_64", 3, args);
    assert(!strcmp(v[0], "/v/Longshot.x86_64") && !strcmp(v[1], "play") && !strcmp(v[2], "--") &&
           !strcmp(v[3], "--fullscreen") && !strcmp(v[4], "") && !v[5]);
    free(v);
    /* (Windows: the command line's arguments after the program's name, as Windows splits it) */
    assert(!wcscmp(longshot_cmdline_tail(L"\"C:\\a b\\longshot.exe\"  --x \"y z\""), L"--x \"y z\""));
    assert(!wcscmp(longshot_cmdline_tail(L"longshot.exe\t-w"), L"-w"));
    assert(!wcscmp(longshot_cmdline_tail(L"longshot.exe"), L""));
    assert(!wcscmp(longshot_cmdline_tail(L"\"C:\\unclosed"), L""));
    /* (the profile's saves, settings and file: from LONGSHOT_PROFILE, a separator at its end left out) */
    static char saves[64];
    assert(longshot_profile_saves("/p/ann", '/', saves, sizeof saves) && !strcmp(saves, "/p/ann/saves"));
    assert(longshot_profile_saves("C:\\P\\", '\\', saves, sizeof saves) && !strcmp(saves, "C:\\P\\saves"));
    assert(!longshot_profile_saves(NULL, '/', saves, sizeof saves) && !longshot_profile_saves("", '/', saves, sizeof saves));
    assert(!longshot_profile_saves("/a/long/profile", '/', saves, 12));
    assert(longshot_profile_saves("/a", '/', saves, 9) && !longshot_profile_saves("/a", '/', saves, 8));
    assert(longshot_profile_settings("/p/ann/", '/', saves, sizeof saves) && !strcmp(saves, "/p/ann/settings.ini"));
    assert(longshot_profile_file("/p/ann", '/', saves, sizeof saves) && !strcmp(saves, "/p/ann/profile.json"));
    /* (profile.json's start_slot, as launcher_profile's Profile::start_slot reads it) */
    for (int i = 0; slots[i].text; i++) {
        int slot = -7;
        const char *why = NULL;
        const int got = longshot_profile_start_slot(slots[i].text, strlen(slots[i].text), &slot, &why);
        if (got != slots[i].got || (got == 1 && slot != slots[i].slot) || (got < 0 && !why)) {
            fprintf(stderr, "start_slot of %s: %d (slot %d), not %d (slot %d)\n", slots[i].text, got, slot, slots[i].got,
                    slots[i].slot);
            return 1;
        }
    }
    puts("handover: ok");
    return 0;
}
"""


def sources():
    """The code agrees with itself: the builder's exit code, the variable and the builders' names"""
    header = (ROOT / "host" / "longshot_exit.h").read_text(encoding="utf-8")
    play = (ROOT / "builder" / "crates" / "build-driver" / "src" / "play.rs").read_text(encoding="utf-8")
    # (a game the builder started has its channel and never hands over: the one variable, the builder's)
    if f"getenv({'LONGSHOT_BUILDER_CHANNEL'})" not in (ROOT / "host" / "hos_main.c").read_text(encoding="utf-8"):
        raise SystemExit("hos_main.c: the hand-over is not keyed on LONGSHOT_BUILDER_CHANNEL")
    for gone in ("LONGSHOT_EXIT_REBUILD", "LONGSHOT_STARTED_BY"):
        if gone in header:
            raise SystemExit(f"host/longshot_exit.h: {gone} is gone: restart_to_apply is over the channel")
    if "REBUILD_EXIT" in play or "STARTED_BY" in play:
        raise SystemExit("build_driver::play still has the exit code or the variable the channel replaced")
    if '#define LONGSHOT_BUILDS "builds"' not in header or 'pub const BUILDS: &str = "builds";' not in play:
        raise SystemExit("the version folder's builds/ is not the same on both sides")
    names = (ROOT / "builder" / "crates" / "longshot-builder" / "src" / "play.rs").read_text(encoding="utf-8")
    for name in ("Longshot.exe", "Longshot.x86_64"):
        if f'"{name}"' not in header or f'"{name}"' not in names:
            raise SystemExit(f"{name}: not a name both the game and the builder have")
    # (both starts hand over first, before anything of the game runs or is written: Windows after its paths' check,
    # Linux first thing)
    main = (ROOT / "host" / "hos_main.c").read_text(encoding="utf-8")
    calls = [m.start() for m in re.finditer(r"\n    hand_over\(argc, argv\);", main)]
    if len(calls) != 2:
        raise SystemExit(f"hos_main.c: hand_over is called {len(calls)} times, not once in each start")
    win = main.index("if (!narrow_paths_ok(argc, argv)) return 1;")
    if not win < calls[0] < main.index("hos_game_layout();", win):
        raise SystemExit("hos_main.c: Windows' start hands over after it lays the folder out")
    linux = main.rindex("int main(int argc, char **argv) {")
    if not linux < calls[1] < main.index("hos_game_layout();", linux):
        raise SystemExit("hos_main.c: Linux's start hands over after it lays the folder out")
    if "execv(builder, v)" not in main or "CreateProcessW(wbuilder, cmd" not in main:
        raise SystemExit("hos_main.c: the hand-over starts no builder")
    # (the profile: its saves/ the save folder, never the user folder, whatever --user names; its settings.ini the
    # settings file, nothing else naming one; its start slot handed on in both starts, after longshot_env.txt, one the
    # environment brought taken away first)
    nn_fs = (ROOT / "host" / "nn_fs.c").read_text(encoding="utf-8")
    if not re.search(r"if \(longshot_profile_saves\(getenv\(LONGSHOT_PROFILE\)", nn_fs) or "hos_user_named" in nn_fs:
        raise SystemExit("nn_fs.c: the profile's saves/ is not the save folder, or --user still changes it")
    if "hos_user_named" in main or "longshot_profile_saves" in main:
        raise SystemExit("hos_main.c: --user still says it was named, or the profile is still the user folder")
    if main.count("    load_longshot_env();\n    profile_start_slot();") != 2:
        raise SystemExit("hos_main.c: the profile's start slot is not handed on right after longshot_env.txt in both starts")
    slot = main[main.index("static void profile_start_slot(void) {"):]
    slot = slot[:slot.index("\n}\n")]
    if not slot.index('unsetenv("HOS_START_SLOT")') < slot.index("longshot_profile_file(profile"):
        raise SystemExit("hos_main.c: a HOS_START_SLOT the environment brought is not taken away before the profile's")
    options = (ROOT / "host" / "options_path.c").read_text(encoding="utf-8")
    if not re.search(r"if \(longshot_profile_settings\(getenv\(LONGSHOT_PROFILE\)", options) or \
            "HOS_OPTIONS_FILE" in options or '"--user"' in options:
        raise SystemExit("options_path.c: the settings are not the profile's settings.ini alone")
    profile_h = (ROOT / "host" / "longshot_profile.h").read_text(encoding="utf-8")
    crate = (ROOT / "builder" / "crates" / "launcher-profile" / "src" / "lib.rs").read_text(encoding="utf-8")
    for c, rust in (("LONGSHOT_PROFILE", "SWITCH"), ("LONGSHOT_PROFILE_FILE", "FILE"),
                    ("LONGSHOT_PROFILE_SETTINGS", "SETTINGS"), ("LONGSHOT_PROFILE_SAVES", "SAVES")):
        value = re.search(r'#define %s "([^"]+)"' % c, profile_h).group(1)
        if f'pub const {rust}: &str = "{value}";' not in crate:
            raise SystemExit(f"host/longshot_profile.h's {c} is not launcher_profile::{rust}")
    play_rs = (ROOT / "builder" / "crates" / "build-driver" / "src" / "play.rs").read_text(encoding="utf-8")
    for line in ("pub const PROFILE_VAR: &str = launcher_profile::SWITCH;", "pub const SAVES: &str = launcher_profile::SAVES;",
                 "pub const SETTINGS: &str = launcher_profile::SETTINGS;", "pub const PROFILE_FILE: &str = launcher_profile::FILE;"):
        if line not in play_rs:
            raise SystemExit(f"play.rs: {line!r} is not there: the profile's names are launcher-profile's")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cc", default="gcc")
    args = parser.parse_args()
    sources()
    with tempfile.TemporaryDirectory(prefix="longshot-exit-") as tmp:
        src, exe = Path(tmp) / "handover_test.c", Path(tmp) / "handover-test"
        src.write_text(HANDOVER_TEST, encoding="utf-8")
        subprocess.run([args.cc, "-std=c11", "-Wall", "-Wextra", "-Werror", "-Wno-write-strings", "-I", str(ROOT / "host"),
                        str(src), "-o", str(exe)], check=True)
        subprocess.run([str(exe)], check=True)
    print("PASS: the rebuild exit code and the hand-over")


if __name__ == "__main__":
    main()
