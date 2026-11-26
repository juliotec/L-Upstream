"""Mods that grow files load together: game/mods.c's list, order and switches, and the resource size table it merges.

Linux only (built over host/posix/win32_posix.c's file functions). Builds game/mods.c (from its index on), host/restbl.c,
host/zs.c's reads and writes and zstd's decoder (third_party/zstd) into one program, makes a synthetic game folder (a
RomFS with a synthetic RESTBL table and synthetic .zs files: no game data) and mods, and checks:

- two mods that grow the same file and one that grows another: the file served is the lower mod's, and the merged table's
  entries fit each served file (in proportion to the game's own allowance); a mod's own table is merged in (its larger entry
  wins), a file a mod adds gets an estimate; the merged table is what the game's table's path serves;
- the mods' counts (replaced, added, taken by a lower mod) and the Mods tab's lines;
- mods.json (tests/test_mods_list.py has the file's other rules, on Windows too): the menu's moves and switches written
  at once, the next start's order and switches read from it (a
  mod switched off serves nothing), "changed since the start" is 0 when moved back;
- ExeFS patch files against the build's exefs_patches.txt: built in, or waiting for the next build (the list is the
  builder's own, game-files' list_text through its mods example, so a change of its lines' shape fails here);
- with no mods folder nothing is served and nothing is written.

python -I -B tests/test_mods_sizes.py --cargo cargo [--cc gcc]
"""
import argparse
import json
import os
import struct
import subprocess
import sys
import tempfile
import zlib
from pathlib import Path

RUN_ALL_ARGS = ["--cargo", "{cargo}"]

ROOT = Path(__file__).resolve().parents[1]
ZSTD = ROOT / "third_party/zstd/lib"
ZSTD_FILES = ("common/debug.c", "common/entropy_common.c", "common/error_private.c", "common/fse_decompress.c", "common/xxhash.c",
              "common/zstd_common.c", "decompress/huf_decompress.c", "decompress/zstd_ddict.c", "decompress/zstd_decompress.c",
              "decompress/zstd_decompress_block.c")
TABLE = "System/Resource/ResourceSizeTable.Product.111.Nin_Ounce_NVN2.rsizetable.zs"


def block(text, marker):
    start = text.index(marker)
    end = text.index("{", start) + 1
    depth = 1
    while depth:
        depth += (text[end] == "{") - (text[end] == "}")
        end += 1
    return text[start:end]


def raw_frame(data):
    """data as a zstd frame of raw blocks (what host/zs.c writes; zstd's decoder reads it)"""
    out = bytearray(b"\x28\xb5\x2f\xfd\x80" + bytes([7 << 3]) + struct.pack("<I", len(data)))
    at = 0
    while True:
        k = min(len(data) - at, 131072)
        last = at + k == len(data)
        bh = (k << 3) | (1 if last else 0)
        out += bytes([bh & 0xFF, (bh >> 8) & 0xFF, (bh >> 16) & 0xFF]) + data[at:at + k]
        at += k
        if last:
            return bytes(out)


def restbl(entries):
    """a RESTBL of {key: size} (all hashed)"""
    hashed = sorted((zlib.crc32(k.encode()), v) for k, v in entries.items())
    b = b"RESTBL" + struct.pack("<IIII", 1, 160, len(hashed), 0)
    return b + b"".join(struct.pack("<II", h, v) for h, v in hashed)


FIXTURE_MAIN = r'''
#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL:%d %s\n", __LINE__, #x); return 1; } } while (0)
static uint32_t entry_of(Restbl *t, const char *key) { uint32_t v = 0; return restbl_get(t, key, &v) ? v : 0; }
int main(int argc, char **argv) {
    if (argc != 3) return 2;
    snprintf(game_root, sizeof game_root, "%s/", argv[1]);
    static char romfs[1024];
    snprintf(romfs, sizeof romfs, "%s/romfs", argv[1]);
    hos_romfs_dir = romfs;
    const char *mode = argv[2];
    hos_mods_init(NULL);
    char out[1024];
    const char *table = "System/Resource/ResourceSizeTable.Product.111.Nin_Ounce_NVN2.rsizetable.zs";
    if (!strcmp(mode, "long")) {   /* (a mod's file whose host path passes MAX_PATH: left out, never a cut path served) */
        static char rel[8192], served[8192];
        char rel_file[1100];
        snprintf(rel_file, sizeof rel_file, "%s/long_rel.txt", argv[1]);
        FILE *rf = fopen(rel_file, "rb");
        CHECK(rf && fgets(rel, sizeof rel, rf));
        fclose(rf);
        CHECK(!hos_mods_romfs(rel, served, sizeof served));
        CHECK(hos_mods_romfs("New/D.bgyml.zs", out, sizeof out));   /* (its short one is served) */
        printf("PASS: a mod's file whose path is too long is left out\n");
        return 0;
    }
    if (!strcmp(mode, "none")) {
        CHECK(!hos_mods_romfs(table, out, sizeof out) && !hos_mods_romfs("Model/A.bfres.zs", out, sizeof out));
        char cache[1100];
        snprintf(cache, sizeof cache, "%s/modcache", argv[1]);
        CHECK(GetFileAttributesA(cache) == INVALID_FILE_ATTRIBUTES);
        CHECK(hos_mods_changed() == 0);   /* (the menu asked to restart at every close) */
        printf("PASS: no mods: nothing served, nothing written, nothing changed\n");
        return 0;
    }
    CHECK(hos_mods_romfs(table, out, sizeof out) && strstr(out, "modcache/rsizetable.zs"));
    size_t n = 0;
    uint8_t *raw = hos_zs_read(out, &n, 64u << 20);
    Restbl *t = raw ? restbl_parse(raw, n) : NULL;
    CHECK(t);
    const char *name, *detail, *note;
    int on;
    if (!strcmp(mode, "first")) {
        CHECK(hos_mods_romfs("Model/A.bfres.zs", out, sizeof out) && strstr(out, "02-second"));   /* (the lower mod's) */
        CHECK(hos_mods_romfs("Tex/B.bntx.zs", out, sizeof out) && strstr(out, "01-first"));
        CHECK(hos_mods_romfs("New/D.bgyml.zs", out, sizeof out) && strstr(out, "03-added"));
        CHECK(!hos_mods_romfs("Other/C.byml.zs", out, sizeof out));                              /* (the game's) */
        CHECK(entry_of(t, "Model/A.bfres") >= 5000 + 300 && entry_of(t, "Model/A.bfres") >= 1300 * 5);   /* (5000 bytes now) */
        CHECK(entry_of(t, "Tex/B.bntx") >= 600 * 4);                                             /* (500 -> 2000) */
        CHECK(entry_of(t, "Other/C.byml") == 9000);                                              /* (02-second's own table) */
        CHECK(entry_of(t, "New/D.bgyml") >= 700 * 10 && entry_of(t, "Keep/E.bntx") == 777);      /* (an estimate; untouched) */
        CHECK(hos_mod_info(0, &name, &detail, &note, &on) && !strcmp(name, "01-first") && on);
        CHECK(strstr(note, "1 of its files are taken by a mod lower in the list"));
        CHECK(strstr(detail, "RomFS: 2 files (2 replace the game's, 0 added)"));
        CHECK(strstr(detail, "ExeFS: 2 patch files, some built into the game"));
        CHECK(strstr(note, "1 ExeFS patch file is not in the game yet"));
        CHECK(hos_mod_info(1, &name, &detail, &note, &on) && !strcmp(name, "02-second") && !*note);
        CHECK(!hos_mod_info(3, &name, &detail, &note, &on));
        CHECK(hos_mods_changed() == 0);
        hos_mod_edit(1, 2);   /* (02-second up a place) */
        CHECK(hos_mods_changed() != 0 && hos_mod_info(0, &name, &detail, &note, &on) && !strcmp(name, "02-second"));
        hos_mod_edit(0, 3);   /* (and back) */
        CHECK(hos_mods_changed() == 0);
        hos_mod_edit(2, 2), hos_mod_edit(0, 0);   /* (03-added over 02-second; 01-first off: for the next start) */
        hos_mods_cheat_set("cheat_0badf00d", 1);
        CHECK(hos_mods_changed() != 0);
        printf("PASS: two mods that grow the same file and a third that adds one: one merged table fits them all\n");
    } else {
        /* (the next start, from mods.json: 01-first off; 03-added before 02-second) */
        CHECK(hos_mod_info(0, &name, &detail, &note, &on) && !strcmp(name, "01-first") && !on);
        CHECK(hos_mod_info(1, &name, &detail, &note, &on) && !strcmp(name, "03-added") && on);
        CHECK(hos_mod_info(2, &name, &detail, &note, &on) && !strcmp(name, "02-second") && on);
        CHECK(hos_mods_cheat_switch("cheat_0badf00d", 0) == 1 && hos_mods_cheat_switch("cheat_other", 0) == 0);
        CHECK(!hos_mods_romfs("Tex/B.bntx.zs", out, sizeof out));                               /* (01-first is off) */
        CHECK(hos_mods_romfs("Model/A.bfres.zs", out, sizeof out) && strstr(out, "02-second"));
        CHECK(entry_of(t, "Model/A.bfres") >= 5000 && entry_of(t, "Tex/B.bntx") == 600);
        printf("PASS: the order and switches of mods.json at the next start\n");
    }
    return 0;
}
'''


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default="gcc")
    p.add_argument("--cargo", required=True, help="builds game-files' mods example, which writes exefs_patches.txt")
    a = p.parse_args()
    if os.name == "nt" or sys.platform == "darwin":
        print("SKIP: the posix layer's checks run on Linux")
        return
    posix = (ROOT / "host/posix/win32_posix.c").read_text(encoding="utf-8")
    mods = (ROOT / "game/mods.c").read_text(encoding="utf-8")
    zs = (ROOT / "host/zs.c").read_text(encoding="utf-8")
    pieces = [
        block(posix, "static DWORD error_of(int e) {"),
        "static BOOL fail_errno(void) { last_error = error_of(errno); return FALSE; }",
        "enum { H_THREAD = 0x48540001, H_EVENT, H_SEMA, H_TIMER, H_FILE, H_FIND };",
        "typedef struct { int type; } HObj;",
        "static int is(HANDLE h, int type) { return h && h != INVALID_HANDLE_VALUE && (uintptr_t)h > 0x10000 && ((HObj *)h)->type == type; }",
        block(posix, "static void slashes(char *dst, size_t cap, const char *src) {"),
        block(posix, "DWORD GetFileAttributesA(LPCSTR name) {"),
        block(posix, "BOOL CreateDirectoryA(LPCSTR name, void *attr) {"),
        block(posix, "BOOL MoveFileExA(LPCSTR from, LPCSTR to, DWORD flags) {"),
        "typedef struct { HObj h; DIR *d; char dir[PATH_MAX], pat[256]; } HFind;",
        block(posix, "static BOOL find_next(HFind *f, WIN32_FIND_DATAA *fd) {"),
        block(posix, "HANDLE FindFirstFileA(LPCSTR pattern, WIN32_FIND_DATAA *fd) {"),
        block(posix, "BOOL FindNextFileA(HANDLE h, WIN32_FIND_DATAA *fd) {"),
        block(posix, "BOOL FindClose(HANDLE h) {"),
        zs[zs.index("typedef struct { unsigned id; ZSTD_DDict *dict; char name[48]; } Dict;"):],
        mods[mods.index("typedef struct { char *key; char *host; char *rel;"):],
    ]
    fixture = r'''#define _GNU_SOURCE
#include <windows.h>
#include <dirent.h>
#include <errno.h>
#include <fnmatch.h>
#include <limits.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <ctype.h>
#include <time.h>
#include <sys/stat.h>
#include <unistd.h>
#define ZSTD_STATIC_LINKING_ONLY
#include "zstd.h"
#include "restbl.h"
#include "mods_list.h"
#include "romfs.h"
#include "romfs_index.h"
#include "prompts_base.h"
static DWORD last_error;
static char game_root[1024];
#define HOS_PATH_CAP MAX_PATH   /* (host/hos_paths.h, as on Linux: game/mods.c's path buffers) */
const char *hos_options_file(void) { return ""; }   /* (host/options_path.c: no settings file, so mods.json is the game folder's) */
void Sleep(DWORD ms) { usleep(ms * 1000); }   /* (game/mods.c waits before it tries a write again) */
const char *hos_romfs_dir;
HosRomfsIndex *hos_romfs_index;   /* (host/romfs.c's folder backend reads the romfs above: no .nup index here) */
void hos_log(const char *f, ...);
void hos_log(const char *f, ...) { if (getenv("MODS_TEST_LOG")) { va_list a; va_start(a, f); vfprintf(stderr, f, a); va_end(a); fputc(10, stderr); } }
static int64_t hos_now_ns(void) { return 0; }
static DWORD hos_game_path_a(char *buf, DWORD size) { return (DWORD)snprintf(buf, size, "%shookshot", game_root); }
int hos_prompts_romfs(const char *key, const char *mod, char *out, size_t cap) { (void)key; (void)mod; (void)out; (void)cap; return 0; }
uint32_t hos_prompts_font_entry(uint32_t entry) { (void)entry; return 0; }   /* (prompts.c: no prompts' font) */
int hos_texts_romfs(const char *key, const char *mod, char *out, size_t cap) { (void)key; (void)mod; (void)out; (void)cap; return 0; }   /* (game/texts.c: no texts of the build's here) */
''' + "\n".join(pieces) + FIXTURE_MAIN
    with tempfile.TemporaryDirectory(prefix="mods-sizes-") as tmp:
        tmp = Path(tmp)
        src = tmp / "fixture.c"
        src.write_text(fixture, encoding="utf-8")
        exe = tmp / "fixture"
        subprocess.run([a.cc, "-std=gnu11", "-O1", "-I", str(ROOT / "host/posix"), "-I", str(ROOT / "host"), "-I", str(ROOT / "game"), "-I", str(ZSTD),
                        "-DZSTD_DISABLE_ASM", "-DZSTD_LEGACY_SUPPORT=0", "-DZSTD_MULTITHREAD=0", "-Wno-unused-function",
                        str(src), str(ROOT / "host/restbl.c"), str(ROOT / "game/mods_list.c"), str(ROOT / "host/romfs.c"),
                        str(ROOT / "host/romfs_index.c"), str(ROOT / "host/prompts_base.c")] + [str(ZSTD / f) for f in ZSTD_FILES] +
                       ["-o", str(exe)],
                       check=True)

        def put(path, data):
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)

        game = tmp / "game"
        romfs = game / "romfs"
        put(romfs / TABLE, raw_frame(restbl({"Model/A.bfres": 1300, "Tex/B.bntx": 600, "Other/C.byml": 500, "Keep/E.bntx": 777})))
        put(romfs / "Model/A.bfres.zs", raw_frame(bytes(1000)))
        put(romfs / "Tex/B.bntx.zs", raw_frame(bytes(500)))
        put(romfs / "Other/C.byml.zs", raw_frame(bytes(400)))
        m = game / "mods"
        put(m / "01-first/romfs/Model/A.bfres.zs", raw_frame(bytes(3000)))   # (both grow A: 02-second's wins)
        put(m / "01-first/romfs/Tex/B.bntx.zs", raw_frame(bytes(2000)))
        put(m / "01-first/exefs/built.pchtxt", b"@nsobid-00\n")
        put(m / "01-first/exefs/new.ips", b"PATCHEOF")
        put(m / "02-second/romfs/Model/A.bfres.zs", raw_frame(bytes(5000)))
        put(m / "02-second/romfs" / TABLE, raw_frame(restbl({"Other/C.byml": 9000, "Tex/B.bntx": 100})))
        put(m / "03-added/romfs/New/D.bgyml.zs", raw_frame(bytes(700)))
        put(m / "-off/romfs/Model/A.bfres.zs", raw_frame(bytes(99999)))       # (off by its name: never seen)
        # (the list as the builder writes it: built.pchtxt applied as code, new.ips not in it)
        listed = subprocess.run([a.cargo, "run", "-q", "-p", "game-files", "--example", "mods", "--",
                                 "exefs-list", "01-first", str(m / "01-first/exefs/built.pchtxt"), "code", ""],
                                cwd=ROOT, capture_output=True, check=True).stdout
        (game / "exefs_patches.txt").write_bytes(listed)

        def run(mode, root):
            r = subprocess.run([str(exe), str(root), mode], capture_output=True, text=True, timeout=60)
            print(r.stdout, end="")
            if r.returncode:
                print(r.stderr, end="")
                raise SystemExit("test_mods_sizes: %s failed (exit %d)" % (mode, r.returncode))

        run("first", game)
        state = json.loads((game / "mods.json").read_text(encoding="utf-8"))
        if state != {"mods": [{"id": "local/01-first", "enabled": False}, {"id": "local/03-added", "enabled": True},
                              {"id": "local/02-second", "enabled": True}], "cheats": {"cheat_0badf00d": True}}:
            raise SystemExit("test_mods_sizes: mods.json is %r" % state)
        run("next", game)
        bare = tmp / "bare"
        put(bare / "romfs" / TABLE, raw_frame(restbl({"Model/A.bfres": 1300})))
        run("none", bare)
        # (the game folder padded so its romfs folder's path is 60 bytes or more whatever the scratch folder's is: a short
        # one, such as /tmp's, left the host path of the file below within MAX_PATH)
        long_game = tmp / "long"
        long_game = tmp / ("long" + "-" * max(0, 60 - len(str(long_game / "mods/04-long/romfs"))))
        put(long_game / "romfs" / TABLE, raw_frame(restbl({"Model/A.bfres": 1300})))
        put(long_game / "mods/04-long/romfs/New/D.bgyml.zs", raw_frame(bytes(700)))
        # (a RomFS path of 4,040 bytes, 19 folders and a file: it fits MAX_PATH (4096 here), and so does its last
        # folder's listing, but not the file's host path with the game folder before it; made folder by folder, as no
        # one path reaches it)
        top = long_game / "mods/04-long/romfs"
        if len(str(top)) < 60:
            raise SystemExit("test_mods_sizes: the scratch folder's path is too short for the long path's case")
        fd, parts = os.open(top, os.O_RDONLY), []
        for _ in range(19):
            os.mkdir("d" * 200, dir_fd=fd)
            nxt = os.open("d" * 200, os.O_RDONLY, dir_fd=fd)
            os.close(fd)
            fd = nxt
            parts.append("d" * 200)
        leaf = "f" * 212 + ".bgyml.zs"
        with os.fdopen(os.open(leaf, os.O_WRONLY | os.O_CREAT, 0o644, dir_fd=fd), "wb") as out:
            out.write(raw_frame(bytes(700)))
        os.close(fd)
        (long_game / "long_rel.txt").write_text("/".join(parts + [leaf]), encoding="utf-8")
        run("long", long_game)


if __name__ == "__main__":
    main()
