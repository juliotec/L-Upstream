"""mods.json, the one list of which mods in the game folder's mods/ (the emulators' layout, game/mods.c) are on and in
which order: the game (game/mods.c, game/mods_list.c) reads and writes it. The builder reads no mods.json: it builds in
manifest mods alone (docs/dev/reference/mods.md, "One mod format"). No game: made-up game folders with empty mods and
authored texts.

game/mods.c, game/mods_list.c and host/options_path.c are built as the game builds them (Windows with its own headers and
the game's manifest, so its file names are UTF-8 as the game's are; Linux with the POSIX shim) into a program that stands in
a made-up game folder, starts the mods as the game does, takes the Mods tab's changes from its command line and prints the
list before and after. Checked:

- where the file is: beside the settings file (the game folder when the game is started without a profile, the profile
  with LONGSHOT_PROFILE; --user moves nothing); no note for a build is written in the game folder;
- the order and the switches written at a change and read at the next start, the cheats' switches with them; the file is
  replaced whole (a new file moved over it);
- what the game does not know is kept when it writes the file: keys beside "mods" and "cheats", keys in a mod's entry (they
  go with their mod when it moves), values as they are written, a cheat that is not a switch, the entry of a mod that is not
  there (in its place); a mod the file does not name is on, after the named ones, and gets an entry;
- what the launcher wrote while the game ran stays: the game writes only what the Mods tab changed in this run (a switch the
  tab changed, the whole order when the tab moved a mod) onto the file as it is then; a mod both changed takes the tab's;
- the game folder's list is copied into the profile once when that has none, as the settings are (not a file that is no
  list);
- mods.json is the only list: a longshot_mods.txt (the list of builds before mods.json) is not read, and left as it is;
- a mods.json that is not a list: every mod on, by name, and the Mods tab's note says so; the file is left as it is until
  the Mods tab changes something, and is then kept as mods.json.bad beside the new one; a file of 1 MB or more is not a
  list;
- the mods no list names in their order (capitals, letters outside ASCII); an id is a folder's name byte for byte; an id
  twice: its first entry counts, the second stays in the file;
- a folder name outside ASCII is its id (UTF-8 in the file); a name that is not UTF-8 (Windows: the program built without
  the manifest) is never written, the file stays a list, and the mod is on after the named ones;
- (Windows) another program has mods.json open: the write is tried again for a moment; when it still fails the change is
  kept, the Mods tab's note says so, and the next change writes both; a file that cannot be opened is not moved aside;
- game/mods_list.c over authored texts for each rule and a few hundred texts made from valid ones by changing bytes: what
  it takes and refuses, and what it writes is a list again, and writing that changes nothing.

python -I -B tests/test_mods_list.py [--cc gcc] --cargo cargo
"""
import argparse
import json
import os
from pathlib import Path
import random
import shutil
import re
import subprocess
import sys
import tempfile

RUN_ALL_ARGS = ["--cc", "gcc", "--cargo", "{cargo}"]
ROOT = Path(__file__).resolve().parents[1]

WINDOWS = sys.platform == "win32"
EXE = "longshot.exe" if WINDOWS else "longshot"

LIST_MAX = 1 << int(re.search(r"MODS_LIST_MAX = 1 << (\d+)", (ROOT / "game" / "mods_list.h").read_text(encoding="utf-8")).group(1))


def manifest_res(path):
    """The game's manifest as the .res the builder links into the exe (build-compile's manifest_res example), at path"""
    subprocess.run([CARGO, "run", "-q", "-p", "build-compile", "--example", "manifest_res", "--", str(path)],
                   cwd=ROOT / "builder", check=True, timeout=1800)
    return str(path)


HOS_H = ("#pragma once\n#include <stddef.h>\n#include <stdint.h>\nvoid hos_log(const char *f, ...);\n"
         "int64_t hos_now_ns(void);\n")
GAME_HOST_H = '#include "hos.h"\n#include "romfs.h"\n#include "hos_paths.h"\n#include "options_path.h"\n#include "restbl.h"\n'

MAIN = r'''
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "mods_list.h"
#include "options_path.h"
#ifdef _WIN32
#include <windows.h>
#endif
void hos_log(const char *fmt, ...) { va_list ap; va_start(ap, fmt); printf("log: "); vprintf(fmt, ap); printf("\n"); va_end(ap); }
int64_t hos_now_ns(void) { return 0; }
#include "romfs_index.h"
const char *hos_romfs_dir;   /* (no game RomFS: no size table to merge; host/romfs.c's folder backend, with no folder) */
HosRomfsIndex *hos_romfs_index;   /* (no .nup's index either) */
size_t hos_zs_frame(void *dst, size_t cap, const void *src, size_t n) { (void)dst; (void)cap; (void)src; (void)n; return 0; }
size_t hos_zs_frame_head(void *dst, size_t cap, const void *src, size_t n) { (void)dst; (void)cap; (void)src; (void)n; return 0; }
uint8_t *hos_zs_read_romfs(const char *rel, size_t *n, size_t limit) { (void)rel; (void)n; (void)limit; return NULL; }
uint64_t hos_zs_romfs_content_size(const char *rel) { (void)rel; return 0; }
uint8_t *hos_zs_read(const char *path, size_t *n, size_t limit) { (void)path; (void)n; (void)limit; return NULL; }
uint64_t hos_zs_content_size(const char *path) { (void)path; return 0; }
int hos_zs_write(const char *path, const uint8_t *data, size_t n) { (void)path; (void)data; (void)n; return 0; }
int hos_prompts_romfs(const char *key, const char *mod, char *out, size_t cap) { (void)key; (void)mod; (void)out; (void)cap; return 0; }
uint32_t hos_prompts_font_entry(uint32_t entry) { (void)entry; return 0; }   /* (prompts.c: no prompts' font) */
int hos_texts_romfs(const char *key, const char *mod, char *out, size_t cap) { (void)key; (void)mod; (void)out; (void)cap; return 0; }
void hos_mods_init(const char *image_path);
int hos_mod_info(int i, const char **name, const char **detail, const char **note, int *enabled);
void hos_mod_edit(int i, int op);
uint64_t hos_mods_changed(void);
int hos_mods_cheat_switch(const char *key, int otherwise);
void hos_mods_cheat_set(const char *key, int on);

#ifdef _WIN32
static HANDLE held = INVALID_HANDLE_VALUE;   /* (a file another program has open) */
static DWORD WINAPI release_later(void *ms) {
    Sleep((DWORD)(uintptr_t)ms);
    CloseHandle(held);
    return 0;
}
#endif
static void hex(const char *s, size_t n) { for (size_t i = 0; i < n; i++) printf("%02x", (unsigned char)s[i]); }
static void show(const char *when) {
    const char *name, *detail, *note;
    int on;
    for (int i = 0; hos_mod_info(i, &name, &detail, &note, &on); i++) {
        printf("%s %s ", when, on ? "on" : "off");
        hex(name, strlen(name));
        printf("\n");
    }
}
/* T_PARSE=<folder> <count>: <folder>/<i>.json through mods_list_parse; its mods, and what is written from it */
static int parse_files(const char *dir, int count) {
    static char buf[1 << 20];
    for (int i = 0; i < count; i++) {
        char path[4096];
        snprintf(path, sizeof path, "%s/%d.json", dir, i);
        FILE *f = fopen(path, "rb");
        if (!f) return 3;
        const size_t n = fread(buf, 1, sizeof buf, f);
        fclose(f);
        ModsList list, again;
        const char *why = "";
        if (!mods_list_parse(&list, buf, n, &why)) {
            printf("file %d bad %s\n", i, why);
            continue;
        }
        printf("file %d ok\n", i);
        for (int k = 0; k < list.nmods; k++) {
            printf("entry %d ", list.mods[k].enabled);
            hex(list.mods[k].id, strlen(list.mods[k].id));
            printf("\n");
        }
        size_t len = 0, len2 = 0;
        char *text = mods_list_write(&list, &len);
        if (!text) return 4;
        printf("text ");
        hex(text, len);
        printf("\n");
        if (!mods_list_parse(&again, text, len, &why)) {
            printf("again bad %s\n", why);
        } else {
            char *text2 = mods_list_write(&again, &len2);
            printf("again %s\n", text2 && len == len2 && !memcmp(text, text2, len) ? "same" : "differs");
            free(text2);
            mods_list_free(&again);
        }
        free(text);
        mods_list_free(&list);
    }
    return 0;
}
int main(int argc, char **argv) {
    if (getenv("T_PARSE")) return argc == 2 ? parse_files(getenv("T_PARSE"), atoi(argv[1])) : 2;
    int split = argc;   /* (the game's own arguments, then "--" and the Mods tab's changes) */
    for (int i = 1; i < argc && split == argc; i++)
        if (!strcmp(argv[i], "--")) split = i;
#ifdef _WIN32
    printf("names %s\n", GetACP() == 65001 ? "utf-8" : "mbcs");   /* (the code page of this program's file names) */
#else
    printf("names utf-8\n");
#endif
    hos_options_choose();   /* (the game's arguments, --user among them, name no settings file) */
    hos_options_carry();
    hos_mods_init(NULL);
    show("start");
    for (int i = split + 1; i < argc; i++) {
        if (!strcmp(argv[i], "edit") && i + 2 < argc) {
            hos_mod_edit(atoi(argv[i + 1]), atoi(argv[i + 2]));
            i += 2;
        } else if (!strcmp(argv[i], "cheat") && i + 2 < argc) {
            hos_mods_cheat_set(argv[i + 1], atoi(argv[i + 2]));
            i += 2;
        } else if (!strcmp(argv[i], "switch") && i + 1 < argc) {
            printf("switch %s %d\n", argv[i + 1], hos_mods_cheat_switch(argv[i + 1], -1));
            i += 1;
        } else if (!strcmp(argv[i], "copy") && i + 2 < argc) {   /* (another program writes a file while the game runs) */
            static char buf[1 << 16];
            FILE *from = fopen(argv[i + 1], "rb"), *to = fopen(argv[i + 2], "wb");
            if (!from || !to) return 5;
            fwrite(buf, 1, fread(buf, 1, sizeof buf, from), to);
            fclose(from), fclose(to);
            i += 2;
        } else if (!strcmp(argv[i], "note") && i + 1 < argc) {   /* (the Mods tab's second line of a mod) */
            const char *name, *detail, *note = "";
            int on;
            hos_mod_info(atoi(argv[i + 1]), &name, &detail, &note, &on);
            printf("note %s\n", note);
            i += 1;
#ifdef _WIN32
        } else if (!strcmp(argv[i], "hold") && i + 3 < argc) {   /* (hold <path> <share> <ms>: another program has the file open,
                                                                    letting others read it (1) or not (0), for ms or until "release") */
            held = CreateFileA(argv[i + 1], GENERIC_READ, atoi(argv[i + 2]) ? FILE_SHARE_READ : 0, NULL, OPEN_EXISTING, 0, NULL);
            if (held == INVALID_HANDLE_VALUE) return 6;
            if (atoi(argv[i + 3])) CloseHandle(CreateThread(NULL, 0, release_later, (void *)(uintptr_t)atoi(argv[i + 3]), 0, NULL));
            i += 3;
        } else if (!strcmp(argv[i], "release")) {
            CloseHandle(held);
#endif
        } else {
            return 2;
        }
    }
    show("end");
    printf("changed %d\n", hos_mods_changed() != 0);
    return 0;
}
'''


def need(ok, what):
    if not ok:
        raise SystemExit("test_mods_list: " + what)


def build(cc, tmp):
    """The program: game/mods.c (beside a hos.h and a game_host.h of the test's), mods_list.c, options_path.c, restbl.c and romfs.c (its
    folder backend, with no RomFS: romfs_index.c beside it, never opened) as they are. On
    Windows (the program, the same without the game's manifest: its file names in the system's code page, as on a Windows
    from before the manifest's); elsewhere (the program, None)"""
    src = tmp / "src"
    src.mkdir()
    (src / "mods.c").write_bytes((ROOT / "game" / "mods.c").read_bytes())
    (src / "hos.h").write_text(HOS_H)
    # (host/game_host.h's parts mods.c uses, over the hos.h above: the whole of it needs the lift's runtime)
    (src / "game_host.h").write_text(GAME_HOST_H)
    (tmp / "main.c").write_text(MAIN)
    host, game = ROOT / "host", ROOT / "game"
    inc = (["-std=gnu11"] + ([] if WINDOWS else ["-D_GNU_SOURCE", "-I", str(host / "posix")]) + ["-I", str(host)]
           + ["-I", str(game)])
    objs = []
    for name, c, strict in (("mods_list", game / "mods_list.c", True), ("mods", src / "mods.c", False),
                            ("options_path", host / "options_path.c", True), ("restbl", host / "restbl.c", False),
                            ("romfs", host / "romfs.c", False), ("romfs_index", host / "romfs_index.c", False),
                            ("main", tmp / "main.c", True)):
        obj = tmp / (name + ".o")
        subprocess.run([cc, *inc, *(["-Wall", "-Wextra", "-Werror"] if strict else ["-w"]), "-c", str(c), "-o", str(obj)], check=True)
        objs.append(str(obj))
    built = tmp / EXE
    if not WINDOWS:   # (the shim as tests/test_options_path.py builds it)
        subprocess.run([cc, "-std=gnu11", "-I", str(host / "posix"), "-I", str(host), *objs, str(host / "posix" / "win32_posix.c"),
                        "-o", str(built), "-lpthread", "-ldl"], check=True)
        return built, None
    plain = tmp / ("plain-" + EXE)
    subprocess.run([cc, *objs, "-o", str(plain)], check=True)
    res = manifest_res(tmp / "longshot_manifest.res")   # (the builder links it into the game)
    if subprocess.run([cc, *objs, res, "-fuse-ld=lld", "-o", str(built)]).returncode:
        print("note: this compiler does not link the game's manifest: the program's file names are the system code page's")
        return plain, plain
    return built, plain


class Game:
    """A made-up game folder: the program as its executable, a placeholder image, mods that are empty folders"""

    def __init__(self, built, top, mods):
        self.top = top
        self.dir = top / "game"
        self.dir.mkdir(parents=True)
        (self.dir / "image.bin").write_text("image")
        shutil.copy2(built, self.dir / EXE)
        for name in mods:
            (self.dir / "mods" / name / "romfs").mkdir(parents=True)
        self.cwd = top / "cwd"
        self.cwd.mkdir()

    def run(self, *args, ops=(), **env):
        """The game started with args, the Mods tab's changes ops: ([(name, on)] at the start, the same at the end, the log)"""
        e = {k: v for k, v in os.environ.items() if k not in ("LONGSHOT_PROFILE", "HOS_MODS", "T_PARSE")}
        e.update(env)
        r = subprocess.run([str(self.dir / EXE), *args, "--", *map(str, ops)], cwd=self.cwd, env=e, capture_output=True, timeout=60)
        self.names = "mbcs" if b"names mbcs" in r.stdout else "utf-8"   # (how the program's file names and log are text)
        out = r.stdout.decode(self.names, "replace")
        need(r.returncode == 0, "the program failed (exit %d):\n%s%s" % (r.returncode, out, r.stderr.decode(self.names, "replace")))
        lists = {"start": [], "end": []}
        for line in out.splitlines():
            word = line.split(" ")
            if word[0] in lists:
                lists[word[0]].append((bytes.fromhex(word[2]).decode(self.names, "replace"), word[1] == "on"))
        return lists["start"], lists["end"], out


CARGO = "cargo"   # (main: --cargo, for the game's manifest on Windows)


def read(path):
    return json.loads(path.read_text(encoding="utf-8"))


def entry(name, on, **more):
    return dict({"id": "local/" + name, "enabled": on}, **more)


def no_mods(built, tmp):
    # no mods folder, and a mods folder with nothing in it: nothing counts as changed since the start (the menu asked to
    # restart at every close), and nothing is written
    for name, make in (("none", False), ("empty", True)):
        g = Game(built, tmp / ("nomods-" + name), [])
        if make:
            (g.dir / "mods").mkdir()
        start, end, log = g.run()
        need(start == end == [] and "changed 0" in log, "no mods (%s): the mods count as changed:\n%s" % (name, log))
        need(not (g.dir / "mods.json").exists() and not (g.dir / "modcache").exists(),
             "no mods (%s): something was written: %r" % (name, sorted(p.name for p in g.dir.iterdir())))


def where_and_round_trip(built, tmp):
    # started by itself: the list beside the game, written at a change, read at the next start
    g = Game(built, tmp / "beside", ["b", "a", "c"])
    start, end, log = g.run(ops=["edit", 2, 2, "edit", 0, 0, "cheat", "cheat_0badf00d", 1, "switch", "cheat_0badf00d"])
    need(start == [("a", True), ("b", True), ("c", True)], "no list: the mods by name, on: %r" % start)
    need(end == [("a", False), ("c", True), ("b", True)] and "changed 1" in log, "the Mods tab's changes: %r" % end)
    need("no list yet" in log and "switch cheat_0badf00d 1" in log, "the log of a first start:\n" + log)
    path = g.dir / "mods.json"
    need(read(path) == {"mods": [entry("a", False), entry("c", True), entry("b", True)], "cheats": {"cheat_0badf00d": True}},
         "mods.json after the changes: %s" % path.read_text(encoding="utf-8"))
    need(not (g.dir / "longshot_mods.txt").exists() and not (g.dir / "modcache").exists() and not (g.dir / "mods.json.bad").exists()
         and not (g.dir / "mods.json.new").exists(), "more than mods.json was written: %r" % sorted(p.name for p in g.dir.iterdir()))
    start, end, log = g.run(ops=["switch", "cheat_0badf00d", "switch", "cheat_other"])
    need(start == end == [("a", False), ("c", True), ("b", True)] and "changed 0" in log, "the next start: %r" % start)
    need("switch cheat_0badf00d 1" in log and "switch cheat_other -1" in log and "the list (the order and the switches) is" in log,
         "the next start's cheats or log:\n" + log)
    was = os.stat(path).st_ino   # (written whole: a new file moved over the old one, never the old one written into)
    g.run(ops=["edit", 0, 1])
    need(read(path)["mods"][0] == entry("a", True) and os.stat(path).st_ino != was and not (g.dir / "mods.json.new").exists(),
         "mods.json was written in place, or its new file was left behind")

    # a profile: the list in it, beside its settings.ini, and no note for a build in the game folder. --user moves nothing
    g = Game(built, tmp / "user", ["a", "b"])
    user = g.top / "profile"
    user.mkdir()
    start, end, log = g.run("--user", str(g.top / "elsewhere"), ops=["edit", 0, 3, "edit", 1, 0], LONGSHOT_PROFILE=str(user))
    need(end == [("b", True), ("a", False)], "a profile: the changes: %r" % end)
    need(read(user / "mods.json") == {"mods": [entry("b", True), entry("a", False)]} and not (g.dir / "mods.json").exists()
         and not (g.top / "elsewhere").exists(), "a profile: mods.json is not in the profile alone")
    note = g.dir / "modcache" / "mods_json.txt"
    need(not note.exists(), "a note for a build was written in the game folder")
    # (HOS_OPTIONS_FILE, a switch that is gone, names no list)
    own = g.top / "own"
    own.mkdir()
    start, end, log = g.run(ops=["edit", 0, 0], LONGSHOT_PROFILE=str(user), HOS_OPTIONS_FILE=str(own / "own.ini"))
    need(read(user / "mods.json") == {"mods": [entry("b", False), entry("a", False)]} and not (own / "mods.json").exists(),
         "HOS_OPTIONS_FILE moved the list")
    # (by itself again: the game folder's list)
    start, end, log = g.run(ops=["edit", 1, 0])
    need(start == [("a", True), ("b", True)] and end == [("a", True), ("b", False)] and not note.exists()
         and read(g.dir / "mods.json") == {"mods": [entry("a", True), entry("b", False)]},
         "started by itself after a profile: %r" % start)
    print("ok: mods.json beside the settings file (the game folder, the profile; not --user, not HOS_OPTIONS_FILE)")


LAUNCHER = '''{ "profile": "randomizer",
  "mods": [ { "id": "local/b", "enabled": true, "version": "1.2.0", "pin": {"a":[1,2.50,"x"]} },
            { "id": "mooki/heart-tracker", "enabled": true, "version": "1.2.0" },
            { "id": "local/gone", "note": "kept", "enabled": false },
            { "id": "local/a", "enabled": false } ],
  "cheats": { "cheat_00000001": false, "cheat_00000002": "later" },
  "room": { "sync": [true, null] } }
'''


def unknown_kept(built, tmp):
    g = Game(built, tmp / "kept", ["a", "b", "new"])
    path = g.dir / "mods.json"
    path.write_text(LAUNCHER, encoding="utf-8")
    start, end, log = g.run(ops=["switch", "cheat_00000001", "switch", "cheat_00000002", "edit", 0, 3, "edit", 2, 0,
                                 "cheat", "cheat_00000001", 1])
    need(start == [("b", True), ("a", False), ("new", True)], "the named mods first, a new one after them, on: %r" % start)
    need("switch cheat_00000001 0" in log and "switch cheat_00000002 -1" in log, "the cheats' switches:\n" + log)
    need(end == [("a", False), ("b", True), ("new", False)], "the changes: %r" % end)
    text = path.read_text(encoding="utf-8")
    need(read(path) == {
        "profile": "randomizer", "room": {"sync": [True, None]},
        "mods": [entry("a", False),
                 {"id": "mooki/heart-tracker", "enabled": True, "version": "1.2.0"},
                 {"id": "local/gone", "enabled": False, "note": "kept"},
                 entry("b", True, version="1.2.0", pin={"a": [1, 2.5, "x"]}),
                 entry("new", False)],
        "cheats": {"cheat_00000001": True, "cheat_00000002": "later"}}, "what the game wrote:\n" + text)
    need('{"a":[1,2.50,"x"]}' in text and '{ "sync": [true, null] }' in text, "a value is not kept as it was written:\n" + text)
    # (the launcher writes the file while the game runs: its keys, its order and its switches are in what the game writes after it)
    later = g.top / "later.json"
    later.write_text(json.dumps({"profile": "vanilla", "mods": [entry("new", False, version="3.0.0"), entry("a", False)]}),
                     encoding="utf-8")
    start, end, log = g.run(ops=["copy", later, path, "edit", 1, 0])
    need(end == [("a", False), ("b", False), ("new", False)], "the list after the launcher's write: %r" % end)
    need(read(path) == {"profile": "vanilla", "mods": [entry("new", False, version="3.0.0"), entry("a", False), entry("b", False)]},
         "the launcher's write is not in what the game wrote after it:\n" + path.read_text(encoding="utf-8"))
    print("ok: unknown keys, values as written and a missing mod's entry are kept; a new mod is added")


def launcher_meanwhile(built, tmp):
    """The launcher changes the list while the game runs: the game's next write keeps it, but for what the tab changed"""
    g = Game(built, tmp / "meanwhile", ["a", "b", "c"])
    path, theirs = g.dir / "mods.json", g.top / "launcher.json"
    all_on = {"mods": [entry("a", True), entry("b", True), entry("c", True)]}

    def case(launcher, ops):
        path.write_text(json.dumps(all_on), encoding="utf-8")
        theirs.write_text(json.dumps(launcher), encoding="utf-8")
        start, end, log = g.run(ops=["copy", theirs, path, *ops])
        need(start == [("a", True), ("b", True), ("c", True)], "the start: %r" % start)
        return read(path), log

    moved = {"profile": "p", "mods": [entry("c", True), entry("a", True), entry("b", False, version="2")]}
    got, log = case(moved, ["cheat", "cheat_0badf00d", 1])   # (the player switches a cheat: no mod is the tab's to write)
    need(got == dict(moved, cheats={"cheat_0badf00d": True}) and "changed 0" in log, "after a cheat's switch: %r" % got)
    start, end, log = g.run()
    need(start == [("c", True), ("a", True), ("b", False)], "the next start does not have the launcher's list: %r" % start)
    got, log = case(moved, ["edit", 0, 0])   # (the player switches another mod off: that switch, and nothing else)
    need(got == {"profile": "p", "mods": [entry("c", True), entry("a", False), entry("b", False, version="2")]},
         "after a switch of another mod: %r" % got)
    off = {"mods": [entry("c", True), entry("a", False), entry("b", False)]}
    got, log = case(off, ["edit", 0, 0, "edit", 0, 1])   # (both changed a, the player last, off and on again: the tab's)
    need(got == {"mods": [entry("c", True), entry("a", True), entry("b", False)]}, "a mod both changed: %r" % got)
    all_off = {"mods": [entry("a", False), entry("b", False), entry("c", False)]}
    got, log = case(all_off, ["edit", 2, 2])   # (the player moves a mod: the tab's order, the launcher's switches)
    need(got == {"mods": [entry("a", False), entry("c", False), entry("b", False)]}, "after a move in the tab: %r" % got)
    print("ok: what the launcher wrote while the game ran stays; the game writes only what the Mods tab changed")


def old_list_not_read(built, tmp):
    """longshot_mods.txt, the list of builds before mods.json, is not read by the game, nor touched"""
    g = Game(built, tmp / "old", ["a", "b", "c"])
    old = g.dir / "longshot_mods.txt"
    old_text = "off b\non c\ncheat cheat_0badf00d on\n"
    old.write_text(old_text, encoding="utf-8")
    by_name = [("a", True), ("b", True), ("c", True)]
    start, end, log = g.run(ops=["switch", "cheat_0badf00d"])
    need(start == by_name and "switch cheat_0badf00d -1" in log and "longshot_mods.txt" not in log,
         "the old list was read: the game %r\n%s" % (start, log))
    need(old.read_text(encoding="utf-8") == old_text and not (g.dir / "longshot_mods.txt.imported").exists()
         and not (g.dir / "mods.json").exists(), "the old list was moved, or a list written from it")
    print("ok: longshot_mods.txt is not read, and left as it is")


def carried(built, tmp):
    """The game folder's list goes to the profile once, as the settings do"""
    g = Game(built, tmp / "carried", ["a", "b", "c"])
    user = g.top / "profile"
    user.mkdir()
    mine = [("a", False), ("c", True), ("b", True)]
    start, end, log = g.run(ops=["edit", 0, 0, "edit", 2, 2])   # (by itself: a off, c up)
    need(end == mine and "copied" not in log, "by itself: %r" % end)
    start, end, log = g.run(ops=["edit", 0, 1], LONGSHOT_PROFILE=str(user))   # (a profile: the same list; a on there)
    need(start == mine and log.count("was copied to") == 1, "the first start with a profile: %r\n%s" % (start, log))
    theirs = [("a", True), ("c", True), ("b", True)]
    need(read(g.dir / "mods.json") == {"mods": [entry(n, on) for n, on in mine]} and
         read(user / "mods.json") == {"mods": [entry(n, on) for n, on in theirs]},
         "the copy does not leave the game folder's list as it is")
    start, end, log = g.run()
    need(start == mine and "copied" not in log, "by itself again, the game folder's: %r" % start)
    start, end, log = g.run(LONGSHOT_PROFILE=str(user))
    need(start == theirs and "copied" not in log, "with the profile again, the profile's, not copied twice: %r" % start)
    # (a game folder's file that is not a list is not copied)
    g = Game(built, tmp / "carried-bad", ["a", "b"])
    user = g.top / "profile"
    user.mkdir()
    (g.dir / "mods.json").write_text('{"mods": [{"id": "local/b", "enabled": 0}]}', encoding="utf-8")
    start, end, log = g.run(LONGSHOT_PROFILE=str(user))
    need(start == [("a", True), ("b", True)] and not (user / "mods.json").exists() and "not copied" in log,
         "a file that is not a list was carried: %r\n%s" % (start, log))
    print("ok: the game folder's list is copied into the profile once, as the settings are")


def not_a_list(built, tmp):
    g = Game(built, tmp / "bad", ["a", "b", "c"])
    path = g.dir / "mods.json"
    bad = '{ "profile": "x", "mods": [ { "id": "local/b", "enabled": false }, ] }\n'   # (a comma too many)
    path.write_text(bad, encoding="utf-8")
    by_name = [("a", True), ("b", True), ("c", True)]
    start, end, log = g.run()
    need(start == end == by_name and "is not a list of mods" in log, "a file that is not a list: %r\n%s" % (start, log))
    start, end, log = g.run(ops=["note", 0])
    need("note mods.json is not a list of mods, so every mod is on" in log, "the Mods tab's note does not say it:\n" + log)
    need(path.read_text(encoding="utf-8") == bad and not (g.dir / "mods.json.bad").exists(), "the file was touched at a start")
    start, end, log = g.run(ops=["edit", 1, 0, "note", 0])
    need("note mods.json is not" not in log, "the note stays after a new list is written:\n" + log)
    need((g.dir / "mods.json.bad").read_text(encoding="utf-8") == bad and "was not a list of mods" in log and "kept as" in log,
         "the file is not kept aside:\n" + log)
    need(read(path) == {"mods": [entry("a", True), entry("b", False), entry("c", True)]}, "the new list")
    start, end, log = g.run()
    need(start == [("a", True), ("b", False), ("c", True)] and "is not a list" not in log, "the start after: %r" % start)
    # (the size: a file one byte under the limit is a list, a file of the limit is not)
    g = Game(built, tmp / "size", ["a", "b"])
    text = json.dumps({"mods": [entry("b", False)]}).encode()
    for size, want in ((LIST_MAX - 1, [("b", False), ("a", True)]), (LIST_MAX, [("a", True), ("b", True)])):
        (g.dir / "mods.json").write_bytes(text + b" " * (size - len(text)))
        start, end, log = g.run()
        need(start == want and ("too large" in log) == (size == LIST_MAX), "a file of %d bytes: the game %r" % (size, start))
    # (a file under the limit that the game's own way of writing, a line a mod and a cheat, would take past it: not written)
    near = ('{"cheats":{%s}}' % ",".join('"k%d":true' % i for i in range(74000))).encode()
    (g.dir / "mods.json").write_bytes(near)
    start, end, log = g.run(ops=["edit", 0, 0, "note", 0])
    need(len(near) < LIST_MAX and "too large to be read again" in log and "not saved yet" in log
         and (g.dir / "mods.json").read_bytes() == near,
         "the game wrote a file too large to be read again, or does not say that it did not")
    print("ok: a mods.json that is not a list: every mod on by name, left as it is, then kept as mods.json.bad at a change")


def odd_lists(built, tmp):
    """Folder sets and lists: the order of what no list names, ids"""
    names = ["B", "a", "Zeta", "alpha"]
    g = Game(built, tmp / "sorted", names)
    if g.run()[0] and g.names == "utf-8":   # (letters outside ASCII where the program's file names are UTF-8)
        try:
            more = ["\u00c9mod", "\u00dfa", "\u00e4", "\u00e9dit"]
            for name in more:
                (g.dir / "mods" / name / "romfs").mkdir(parents=True)
            names += more
        except OSError:
            pass
    want = sorted(names, key=lambda n: bytes(c + 32 if 65 <= c <= 90 else c for c in n.encode("utf-8")))
    start, end, log = g.run()
    need([n for n, _ in start] == want and want[:2] == ["a", "alpha"], "the mods no list names: the game %r" % start)
    # (an id is the folder's name byte for byte: another case is another mod)
    g = Game(built, tmp / "case", ["a", "b"])
    path = g.dir / "mods.json"
    path.write_text(json.dumps({"mods": [{"id": "local/A", "enabled": False}, entry("b", False)]}), encoding="utf-8")
    start, end, log = g.run()
    need(start == [("b", False), ("a", True)], "an id in another case: the game %r" % start)
    # (an id twice: its first entry counts; the second stays in the file. A cheat's key twice: the later one counts,
    # and both stay)
    twice = [entry("a", False), entry("b", True), entry("a", True, note="second")]
    path.write_text('{"mods": %s, "cheats": {"cheat_1": false, "cheat_1": true}}' % json.dumps(twice), encoding="utf-8")
    start, end, log = g.run(ops=["switch", "cheat_1", "edit", 1, 0])
    need(start == [("a", False), ("b", True)] and end == [("a", False), ("b", False)], "an id twice: %r, %r" % (start, end))
    need(read(path) == {"mods": [entry("a", False), entry("b", False), entry("a", True, note="second")], "cheats": {"cheat_1": True}}
         and "switch cheat_1 1" in log and path.read_text(encoding="utf-8").count('"cheat_1"') == 2, "an id or a cheat's key twice, written")
    print("ok: the order of what no list names; ids byte for byte, the first of two")


def held_open(built, tmp):
    """Another program has mods.json open when the game writes it (Windows: the new file cannot be moved over it)"""
    if not WINDOWS:
        print("skip: a file held open stops a write on Windows only")
        return
    g = Game(built, tmp / "held", ["a", "b"])
    path = g.dir / "mods.json"
    g.run(ops=["edit", 0, 0])
    was = path.read_bytes()
    # (held for longer than the game tries: the change is kept, the tab says so, and the next change writes both)
    start, end, log = g.run(ops=["hold", path, 1, 0, "edit", 1, 0, "note", 0, "copy", path, g.top / "then.json", "release",
                                 "cheat", "cheat_0badf00d", 1, "note", 0])
    notes = [line for line in log.splitlines() if line.startswith("note ")]
    need((g.top / "then.json").read_bytes() == was and "cannot write" in log and "not saved yet" in notes[0],
         "a write while the file is held:\n" + log)
    need(read(path) == {"mods": [entry("a", False), entry("b", False)], "cheats": {"cheat_0badf00d": True}}
         and "not saved" not in notes[1] and not (g.dir / "mods.json.new").exists() and not (g.dir / "mods.json.bad").exists(),
         "the change is not written with the next one:\n" + log)
    # (held for a moment: the game tries again and the change is written)
    start, end, log = g.run(ops=["hold", path, 1, 40, "edit", 1, 1, "note", 0])
    need(read(path)["mods"] == [entry("a", False), entry("b", True)] and "cannot write" not in log and "not saved" not in log,
         "a write while the file is held for a moment:\n" + log)
    # (held so that it cannot even be read: not a verdict on the file, which is not moved aside)
    start, end, log = g.run(ops=["hold", path, 0, 0, "edit", 0, 1, "note", 0, "release", "edit", 1, 0])
    need("cannot be opened now" in log and "not saved yet" in log and not (g.dir / "mods.json.bad").exists()
         and read(path)["mods"] == [entry("a", True), entry("b", False)], "a write while the file cannot be opened:\n" + log)
    print("ok: a write while another program has mods.json open is tried again, then kept for the next change and said on the tab")




def other_names(built, plain, tmp):
    name = "caf\u00e9 \u30b2\u30fc\u30e0"
    try:
        g = Game(built, tmp / "names", ["a", name])
        start, end, log = g.run(ops=["edit", 1, 2, "edit", 1, 0])
        if g.names != "utf-8":
            raise UnicodeError
    except (OSError, UnicodeError):
        print("skip: a folder name outside ASCII (this system's file names, or the program's, cannot hold %r)" % name)
    else:
        need(end == [(name, True), ("a", False)], "a name outside ASCII: %r" % end)
        need(read(g.dir / "mods.json") == {"mods": [entry(name, True), entry("a", False)]}, "its id in mods.json is not its name")
        start, end, log = g.run()
        need(start == [(name, True), ("a", False)], "the next start reads %r" % start)
        print("ok: a folder name outside ASCII is its id")
    # a name that is not UTF-8 in the program (its file names in the system's code page): never in the file
    name = "caf\u00e9"
    try:
        if plain is None or name.encode("mbcs").decode("mbcs") != name or name.encode("mbcs") == name.encode("utf-8"):
            raise UnicodeError
    except UnicodeError:
        print("skip: a folder name that is not UTF-8 (needs the program without the manifest, and a code page with %r)" % name)
        return
    g = Game(plain, tmp / "bytes", ["b", name, "z"])
    start, end, log = g.run(ops=["edit", 2, 2, "edit", 0, 0])   # (z over the name; b off)
    need(g.names == "mbcs" and start == [("b", True), (name, True), ("z", True)] and "cannot name this folder" in log,
         "a name that is not UTF-8 at the start: %r\n%s" % (start, log))
    need(end == [("b", False), ("z", True), (name, True)], "the changes: %r" % end)
    need(read(g.dir / "mods.json") == {"mods": [entry("b", False), entry("z", True)]}, "the file with a name it cannot hold")
    start, end, log = g.run()
    need(start == [("b", False), ("z", True), (name, True)], "the next start reads %r" % start)
    print("ok: a folder name that is not UTF-8 is never written: the file stays a list, the mod is on after the named ones")


# ---- the reader's rules: texts for each rule (True: a list), then texts made by changing bytes
DEEP = b"[" * 63 + b"]" * 63   # (as a value in the list's object: 64 deep, the most)
TEXTS = [
    (True, b"{}"), (True, b" \t\r\n{ } \n"), (True, b'{"mods":[]}'), (True, b'{"cheats":{}}'),
    (True, b"\xef\xbb\xbf" + LAUNCHER.encode()),
    (True, b'{"mods":[{"id":"local/a"}]}'), (True, b'{"mods":[{"enabled":false,"id":""}]}'),
    (True, b'{"mods":[{"id":"local\\/a\\u00e9\\ud83d\\ude00 \\"q\\" \\\\ \\b\\f\\n\\r\\t"}]}'),
    (True, '{"mods":[{"id":"local/caf\u00e9 \u4e2d\U0001F600"}]}'.encode()),
    (True, b'{"mods":[{"id":"a"},{"id":"a","enabled":false}],"x":1,"x":2}'),
    (True, b'{"n":[0,-0,1.5e+10,1E-2,-12.0e0,' + b"9" * 5000 + b'],"cheats":{"k":true,"k":false,"j":[1]}}'),
    (True, b'{"x":' + DEEP + b"}"), (True, b'{"\\u006dods":[{"\\u0069d":"local/a","enabled":false}]}'),
    (True, b'{"mods":[{"id":"a","version":"1.0","enabled":true,"\x7f":null}]}'),
    (False, b""), (False, b"  \n"), (False, b"\xef\xbb\xbf"), (False, b"[]"), (False, b"null"), (False, b'"mods"'), (False, b"1"),
    (False, b'{"mods":[{"id":"a"},]}'), (False, b'{"mods":[{"id":"a",}]}'), (False, b"{'mods':[]}"), (False, b'{mods:[]}'),
    (False, b'{"mods":[]} // x'), (False, b'{"mods":[]} x'), (False, b"{}{}"), (False, b"{}\x00"), (False, b'{"a":"\x00"}'),
    (False, b'{"mods":[{"id":"a","enabled":1}]}'), (False, b'{"mods":[{"id":"a","enabled":null}]}'),
    (False, b'{"mods":[{"id":"a","enabled":"true"}]}'), (False, b'{"mods":[{"id":7}]}'), (False, b'{"mods":[{"enabled":true}]}'),
    (False, b'{"mods":[{"id":"a","id":"a"}]}'), (False, b'{"mods":[{"id":"a","enabled":true,"enabled":true}]}'),
    (False, b'{"mods":[],"mods":[]}'), (False, b'{"cheats":{},"cheats":{}}'), (False, b'{"mods":{}}'), (False, b'{"mods":null}'),
    (False, b'{"cheats":[]}'), (False, b'{"cheats":true}'), (False, b'{"mods":["local/a"]}'), (False, b'{"mods":[[]]}'),
    (False, b'{"a":NaN}'), (False, b'{"a":Infinity}'), (False, b'{"a":-Infinity}'), (False, b'{"a":01}'), (False, b'{"a":1.}'),
    (False, b'{"a":.5}'), (False, b'{"a":+1}'), (False, b'{"a":1e}'), (False, b'{"a":-}'), (False, b'{"a":0x10}'),
    (False, b'{"a":True}'), (False, b'{"a":None}'), (False, b'{"a":"\t"}'), (False, b'{"a":"\n"}'), (False, b'{"a":"\\x41"}'),
    (False, b'{"a":"\\u12G4"}'), (False, b'{"a":"\\u12"}'), (False, b'{"a":"\\ud800"}'), (False, b'{"a":"\\udc00"}'),
    (False, b'{"a":"\\ud800\\u0041"}'), (False, b'{"a":"\\ud800x"}'), (False, b'{"a":"\\u0000"}'), (False, b'{"\\ud800":1}'),
    (False, b'{"a":"\xff"}'), (False, b'{"a":"\xc0\x80"}'), (False, b'{"a":"\xed\xa0\x80"}'), (False, b'{"a":"\xf5\x80\x80\x80"}'),
    (False, b'{"a":"\xe4\xb8"}'), (False, b"\xef\xbb\xbf\xef\xbb\xbf{}"), (False, b"{\xef\xbb\xbf}"), (False, b"{}\xc2\xa0"),
    (False, b"{}\x0c"), (False, b"\x0b{}"), (False, b'{"a":"x'), (False, b'{"a":[1,2'), (False, b'{"a" 1}'), (False, b'{"a":}'),
    (False, b"{,}"), (False, b'{"a":[1 2]}'), (False, b'{"x":[' + DEEP + b"]}"), (False, b'{"x":' + b"[" * 3000 + b"]" * 3000 + b"}"),
    (False, b'{"mods":[{"id":"a"}]'), (False, b'{"mods":[{"id":"a"}}'), (False, b'{"a":tru}'), (False, b'{"a":1,"mods":[{}]}'),
]
BYTES = (b'{}[]",:\\ \n\t01.eE+-ntfu/' b"\x00\x7f\x80\xc3\xa9\xed\xef\xff")


def changed_texts(count):
    """Texts made from valid ones by taking out, putting in, repeating and swapping bytes: most are not lists any more"""
    rng = random.Random(1337)
    seeds = [text for ok, text in TEXTS if ok and len(text) < 2000]
    out = []
    while len(out) < count:
        text = bytearray(rng.choice(seeds))
        for _ in range(rng.randint(1, 3)):
            at = rng.randrange(len(text) + 1)
            how = rng.randrange(5)
            if how == 0 and text:
                del text[min(at, len(text) - 1)]
            elif how == 1:
                text[at:at] = bytes([rng.choice(BYTES)])
            elif how == 2 and text:
                text[min(at, len(text) - 1)] = rng.choice(BYTES)
            elif how == 3:
                end = min(len(text), at + rng.randint(1, 12))
                text[at:at] = text[at:end]
            elif text:
                other = rng.randrange(len(text))
                at = min(at, len(text) - 1)
                text[at], text[other] = text[other], text[at]
        out.append(bytes(text))
    return out


def the_reader(built, tmp):
    folder = tmp / "texts"
    folder.mkdir()
    texts = TEXTS + [(None, text) for text in changed_texts(600)]
    for i, (_, text) in enumerate(texts):
        (folder / ("%d.json" % i)).write_bytes(text)
    r = subprocess.run([str(built), str(len(texts))], env=dict(os.environ, T_PARSE=str(folder)), capture_output=True, timeout=120)
    need(r.returncode == 0, "the program failed over the texts (exit %d)" % r.returncode)
    host = {}
    for line in r.stdout.decode("ascii").splitlines():
        word = line.split(" ")
        if word[0] == "file":
            at = host[int(word[1])] = {"ok": word[2] == "ok", "why": " ".join(word[3:]), "mods": []}
        elif word[0] == "entry":
            at["mods"].append((bytes.fromhex(word[2]).decode("utf-8"), word[1] == "1"))
        elif word[0] == "text":
            at["text"] = bytes.fromhex(word[1])
        elif word[0] == "again":
            at["again"] = " ".join(word[1:])
    lists = 0
    for i, (want, text) in enumerate(texts):
        got = host[i]
        need(want is None or got["ok"] == want, "%r is %sa list (%s)" % (text[:200], "" if got["ok"] else "not ", got["why"]))
        if not got["ok"]:
            continue
        lists += 1
        need(got["again"] == "same", "written again, %r is %s: %r" % (text[:200], got["again"], got["text"][:300]))
        # (what the game does not know is what it writes: the other keys, their numbers by their digits)
        whole = json.loads(text.decode("utf-8-sig"), parse_int=str, parse_float=str)
        again = json.loads(got["text"].decode("utf-8"), parse_int=str, parse_float=str)
        for known in ("mods", "cheats"):
            whole.pop(known, None), again.pop(known, None)
        need(whole == again, "the other keys of %r are not kept: %r" % (text[:200], got["text"][:300]))
    need(lists >= 40, "only %d of the texts are lists: the changed ones do not test much" % lists)
    print("ok: the game read %d texts (%d lists; written again, the same)" % (len(texts), lists))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default="gcc")
    p.add_argument("--cargo", default="cargo", help="for the game's manifest on Windows (build-compile's manifest_res example)")
    a = p.parse_args()
    global CARGO
    CARGO = a.cargo
    cc = shutil.which(a.cc) or shutil.which("clang") or a.cc
    with tempfile.TemporaryDirectory(prefix="longshot-mods-list-") as tmp:
        tmp = Path(tmp)
        built, plain = build(cc, tmp)
        where_and_round_trip(built, tmp)
        no_mods(built, tmp)
        unknown_kept(built, tmp)
        launcher_meanwhile(built, tmp)
        old_list_not_read(built, tmp)
        carried(built, tmp)
        not_a_list(built, tmp)
        odd_lists(built, tmp)
        held_open(built, tmp)
        other_names(built, plain, tmp)
        the_reader(built, tmp)
    print("PASS: mods.json: where it is, what is kept, no old file read, a file that is not a list, what the reader takes")


if __name__ == "__main__":
    main()
