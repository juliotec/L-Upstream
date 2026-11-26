"""The Linux game's move of its saves to $XDG_DATA_HOME/longshot leaves a linked folder alone (host/hos_main.c, saves_dir).

A user/ folder beside the game is moved to the new place at the first start. A symbolic link in it, to saves the player
keeps elsewhere (a synced folder, another drive), was moved as a file: by rename, and one written from where it stood
("../../sync") then named nothing; or, from another drive, copied as the folder it names, and that folder emptied
afterwards. That start still found the saves; the next found the link in user/, kept the old place for good and showed
none. An entry the new place had already kept the old place too, but only after the others had gone.

Linux only. Builds saves_dir and what it calls, from the source as it is, over host/posix/win32_posix.c, and starts it
twice (the move, then the start after) on folders of its own:

- a link, written in full or from where it stands, is made again in the new place to the same folder, whose files stay;
- so is one deeper in (user/save/<user id>); one to an entry that moves too names it in the new place, written in full
  or not; absolute links made before and after save/ cover both creation-order and newest-first listings (the file
  system's order used to decide whether they named the old place after its entry had gone); one that names nothing
  goes as it was written;
- from another drive too, when this PC has one to try (/dev/shm);
- a user/ that is a link itself is used as it is;
- with an entry the new place has already, nothing moves;
- with an entry that cannot go (a copy that fails, as on a full disk), the entries that went before it come back, renamed
  and copied ones alike: at both starts the old place keeps every entry and the new place is empty; once the entry can
  go, the next start moves them all. The entries' names are picked so that both kinds are listed before the one that
  fails, whatever the file system's order; from another drive too, where every entry is copied;
- a start cut at each step of the move in turn (a copy part way, a rename, the commit, a deletion), here and from
  another drive: the next start uses the new place, which holds every entry as it was, and the old place is gone;
- a delete of the old place's entries that fails after the commit: the new place is used, whole, and a later start
  deletes them;
- a user/ without links moves as before, and saves-moved.txt says where to;
- the Windows branch compiles with llvm-mingw's clang, when the PATH has x86_64-w64-mingw32-clang.

python -I -B tests/test_linux_saves_move.py [--cc gcc]
"""
import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
USER_ID = "5a3c9e1f0b7d24680000000000000001"


def build(cc, tmp):
    source = (ROOT / "host" / "hos_main.c").read_text(encoding="utf-8")
    start = source.index("static int is_dir(const char *p) {")
    end = source.index("/* (longshot) What a write of a setting", source.index("static const char *saves_dir(const char *beside) {"))
    src, exe = tmp / "mover.c", tmp / "mover"
    src.write_text(program(source[start:end], "/"), encoding="utf-8")
    subprocess.run([cc, "-std=gnu11", "-D_GNU_SOURCE", "-w", "-I", str(ROOT / "host" / "posix"), "-I", str(ROOT / "host"), str(src),
                    str(ROOT / "host" / "posix" / "win32_posix.c"), "-o", str(exe), "-lpthread", "-ldl"], check=True)
    return exe


def program(code, sep):
    """saves_dir and what it calls, with a main that prints the place it chose. SAVES_CUT_AT=<n> ends the process
    (exit 70) at its n-th step (each HOS_AF_STEP: a copy or rename's, an entry copied, an old entry deleted), as a
    PC that lost power there would"""
    return r'''#include <windows.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#ifdef _WIN32
#include <process.h>
#else
#include <unistd.h>
#endif
static int cut_steps;
static void cut_step(const char *what) {
    const char *at = getenv("SAVES_CUT_AT");
    if (at && ++cut_steps == atoi(at)) { fprintf(stderr, "cut at %s\n", what); fflush(NULL); _exit(70); }
}
#define HOS_AF_STEP(what) cut_step(what)
#include "hos_atomic_file.h"
#define SEP '@SEP@'
#define SEP_S "@SEP@"
#define HOS_PATH_CAP MAX_PATH
static void hos_log(const char *fmt, ...) { va_list ap; va_start(ap, fmt); vprintf(fmt, ap); va_end(ap); putchar(10); }
'''.replace("@SEP@", sep.replace("\\", "\\\\")) + code + r'''
int main(int argc, char **argv) { if (argc != 2) return 2; printf("uses %s\n", saves_dir(argv[1])); return 0; }
'''


def compile_windows(tmp):
    """The same, its Windows branch, compiled by llvm-mingw's clang when this PC has it (x86_64-w64-mingw32-clang on the
    PATH): the Windows code is at least built"""
    mingw = shutil.which("x86_64-w64-mingw32-clang")
    if not mingw:
        print("the Windows branch: not compiled (no x86_64-w64-mingw32-clang on the PATH)")
        return
    source = (ROOT / "host" / "hos_main.c").read_text(encoding="utf-8")
    start = source.index("static int is_dir(const char *p) {")
    end = source.index("/* (longshot) What a write of a setting", source.index("static const char *saves_dir(const char *beside) {"))
    src = tmp / "mover_windows.c"
    src.write_text(program(source[start:end], "\\"), encoding="utf-8")
    r = subprocess.run([mingw, "-std=gnu11", "-Wall", "-Werror", "-Wno-unused-function", "-I", str(ROOT / "host"), str(src),
                        "-o", str(tmp / "mover.exe")], capture_output=True, text=True)
    if r.returncode != 0:
        raise SystemExit("test_linux_saves_move: the Windows branch does not compile:\n%s" % r.stderr)
    print("the Windows branch: compiled by %s" % mingw)


class Case:
    """A game folder with user/ in it, the player's own folder beside it, and an empty data folder: each start's answer"""

    def __init__(self, exe, name, game_parent, data_parent):
        self.exe, self.name = exe, name
        self.game = Path(tempfile.mkdtemp(prefix="game-", dir=game_parent))
        # (deeper than the game's user/, as ~/.local/share is: a link written from where it stands names another place from it)
        self.data = Path(tempfile.mkdtemp(prefix="home-", dir=data_parent)) / ".local" / "share"
        self.data.mkdir(parents=True)
        self.user, self.new = self.game / "user", self.data / "longshot"
        self.sync = self.game.parent / (self.game.name + "-sync")   # (beside the game: "../../<name>-sync" from user/save)
        (self.user / "temp").mkdir(parents=True)
        (self.sync / USER_ID).mkdir(parents=True)
        (self.sync / USER_ID / "account.sav").write_text("my save", encoding="ascii")

    def start(self):
        r = self.run()
        self.check(r.returncode == 0, "the start failed: %s" % r.stderr)
        return Path(r.stdout.strip().splitlines()[-1][len("uses "):])

    def run(self, **extra):
        env = dict(os.environ, XDG_DATA_HOME=str(self.data))
        env.pop("SAVES_CUT_AT", None)
        env.update(extra)
        return subprocess.run([str(self.exe), str(self.user)], env=env, capture_output=True, text=True, timeout=60)

    def check(self, ok, what):
        if not ok:
            raise SystemExit("test_linux_saves_move: %s: %s" % (self.name, what))

    def saves_kept(self):
        save = self.sync / USER_ID / "account.sav"
        self.check(save.is_file() and save.read_text(encoding="ascii") == "my save", "the linked folder lost its save")

    def moved(self, save):
        """Both starts use the new place, where save reads the player's file; user/ is gone"""
        for n in (1, 2):
            self.check(self.start() == self.new, "start %d did not use the new place" % n)
        self.check((self.new / save / "account.sav").is_file(), "the save is not read from the new place")
        self.check(not self.user.exists(), "user/ is still there")
        self.check("%s\n" % self.new in (self.game / "saves-moved.txt").read_text(encoding="utf-8"), "saves-moved.txt")


def tree(root):
    """What a folder holds: {path in it: ("dir",) | ("file", text, or None when it cannot be read) | ("link", what it names)}"""
    found = {}
    for d, dirs, files in os.walk(root):
        for n in dirs + files:
            p = Path(d) / n
            rel = str(p.relative_to(root))
            if p.is_symlink():
                found[rel] = ("link", os.path.realpath(p))
            elif p.is_dir():
                found[rel] = ("dir",)
            else:
                try:
                    found[rel] = ("file", p.read_text(encoding="ascii"))
                except PermissionError:
                    found[rel] = ("file", None)
        dirs[:] = [n for n in dirs if not (Path(d) / n).is_symlink()]
    return found


def cannot_go(case, name, game_parent):
    """user/ with entries that are renamed and entries that are copied (a link in them; every entry from another drive),
    and stuck/, whose unreadable file stops its copy as a full disk would. Entries are made before and after stuck/ (for
    creation-order and newest-first listings), under new names until both kinds are listed before it (for a hashed one):
    each start has then moved some before it fails, and they must come back"""
    for attempt in range(50):
        c = case(name, game_parent)
        far = c.game.stat().st_dev != c.data.stat().st_dev
        (c.user / "save" / USER_ID).mkdir(parents=True)
        (c.user / "save" / USER_ID / "account.sav").write_text("my save", encoding="ascii")
        kinds = {"temp": "renamed", "save": "renamed"}
        stuck = c.user / "stuck"
        for side in ("a", "z"):
            if side == "z":
                stuck.mkdir()
                (stuck / "sync").symlink_to(c.sync)
                (stuck / "locked").write_text("locked", encoding="ascii")
                (stuck / "locked").chmod(0)
            linked = c.user / ("%s-linked-%d" % (side, attempt))
            linked.mkdir()
            (linked / "sync").symlink_to(c.sync)
            (linked / "notes.txt").write_text(linked.name, encoding="ascii")
            plain = c.user / ("%s-file-%d" % (side, attempt))
            plain.write_text(plain.name, encoding="ascii")
            kinds[linked.name], kinds[plain.name] = "copied", "renamed"
        order = os.listdir(c.user)
        before = {"copied" if far else kinds[n] for n in order[:order.index("stuck")]}
        if before >= ({"copied"} if far else {"copied", "renamed"}):
            break
        (stuck / "locked").chmod(0o600)
    else:
        raise SystemExit("test_linux_saves_move: %s: no names put both kinds of entry before stuck/ in this listing: %s"
                         % (name, order))
    try:
        whole = tree(c.user)
        for n in (1, 2):
            place = c.start()
            c.check(place == c.user, "start %d uses %s, not user/" % (n, place))
            c.check(tree(c.user) == whole, "start %d: user/ lost or changed entries (listed %s): %s" % (
                n, order, sorted(set(whole.items()) ^ set(tree(c.user).items()))))
            c.check(sorted(os.listdir(c.new)) == [], "start %d left entries in the new place: %s" % (n, sorted(os.listdir(c.new))))
            c.check(not (c.game / "saves-moved.txt").exists(), "start %d wrote saves-moved.txt" % n)
    finally:
        (stuck / "locked").chmod(0o600)
    whole = tree(c.user)
    c.moved("save/" + USER_ID)
    c.check(tree(c.new) == whole, "once stuck/ could go, the new place did not get every entry: %s" % (
        sorted(set(whole.items()) ^ set(tree(c.new).items()))))
    c.saves_kept()


def cut_anywhere(case, game_parent, where=""):
    """A start cut at each of the move's steps in turn (a copy part way, a rename, the commit, a deletion), then a start
    as usual: that one uses the new place, which holds every entry as it was, the old place's entries are gone and none
    of the move's own names is left. Until all the steps are tried (the move then ends before the cut)"""
    for at in range(1, 400):
        c = case("cut at step %d%s" % (at, where), game_parent)
        (c.user / "save" / USER_ID).mkdir(parents=True)
        (c.user / "save" / USER_ID / "account.sav").write_text("my save", encoding="ascii")
        (c.user / "save" / USER_ID / "backup.sav").write_text("my backup", encoding="ascii")
        (c.user / "mouse_sens.txt").write_text("1.5", encoding="ascii")
        (c.user / "linked").symlink_to(c.sync)
        whole = tree(c.user)
        r = c.run(SAVES_CUT_AT=str(at))
        if r.returncode == 0:
            c.check(at > 5, "the move has only %d steps: the cut points are not reached" % at)
            c.check(tree(c.new) == whole and not c.user.exists(), "the uncut move")
            return at - 1
        c.check(r.returncode == 70, "the cut start failed otherwise: %s" % r.stderr)
        place = c.start()
        c.check(place == c.new, "after %s the next start uses %s" % (r.stderr.strip(), place))
        c.check(tree(c.new) == whole, "after %s the new place is not the saves as they were: %s" % (
            r.stderr.strip(), sorted(set(whole.items()) ^ set(tree(c.new).items()))))
        c.check(not c.user.exists(), "after %s the old place is still there: %s" % (r.stderr.strip(), sorted(os.listdir(c.user))
                                                                                    if c.user.exists() else ""))
        c.check(c.start() == c.new and tree(c.new) == whole, "the start after that")
        c.saves_kept()
    raise SystemExit("test_linux_saves_move: the move never ended uncut")


def delete_fails(case):
    """The old place's entries not deleted after the commit (a file there that cannot be removed): the new place is used,
    whole, at that start and the next; once the entry can go, a start deletes it and the old place is gone"""
    c = case("a delete that fails")
    held = c.user / "save" / USER_ID
    held.mkdir(parents=True)
    (held / "account.sav").write_text("my save", encoding="ascii")
    whole = tree(c.user)
    held.chmod(0o555)   # (its file cannot be removed)
    try:
        for n in (1, 2):
            c.check(c.start() == c.new, "start %d did not use the new place" % n)
            got = {k: v for k, v in tree(c.new).items() if k != ".moved-names"}
            c.check(got == whole, "start %d: the new place is not the saves as they were" % n)
            c.check((c.user / "save" / USER_ID / "account.sav").is_file() and not (c.user / "temp").exists(),
                    "start %d: the old place's entries that could go did not, or the one that could not did" % n)
    finally:
        if held.exists():
            held.chmod(0o755)
    c.check(c.start() == c.new and tree(c.new) == whole, "the start once it can go")
    c.check(not c.user.exists(), "the old place is still there")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default="gcc")
    a = p.parse_args()
    if os.name == "nt" or sys.platform == "darwin":
        print("SKIP: the posix layer's checks run on Linux")
        return
    with tempfile.TemporaryDirectory(prefix="saves-move-") as tmp:
        tmp = Path(tmp)
        exe = build(a.cc, tmp)

        def case(name, game_parent=tmp):
            return Case(exe, name, game_parent, tmp)

        c = case("a link written in full")
        (c.user / "save").symlink_to(c.sync)
        c.moved("save/" + USER_ID)
        c.check((c.new / "save").is_symlink() and (c.new / "save").resolve() == c.sync.resolve(), "the new place has no link to it")
        c.saves_kept()

        c = case("a link written from where it stands")
        (c.user / "save").symlink_to("../../%s" % c.sync.name)
        c.moved("save/" + USER_ID)
        c.check((c.new / "save").resolve() == c.sync.resolve(), "the link names another folder now")
        c.saves_kept()

        c = case("a link deeper in")
        (c.user / "save").mkdir()
        (c.user / "save" / USER_ID).symlink_to("../../../%s/%s" % (c.sync.name, USER_ID))
        (c.user / "save" / "notes.txt").write_text("mine", encoding="ascii")
        c.moved("save/" + USER_ID)
        c.check((c.new / "save" / USER_ID).is_symlink() and (c.new / "save" / "notes.txt").read_text(encoding="ascii") == "mine",
                "the folder around the link did not move as it was")
        c.saves_kept()

        c = case("a link to an entry that moves too")
        # (both creation-order and newest-first listings visit some links after save/, which has gone by then)
        firsts = ["last-%s" % ch for ch in "abcdefghijklmnopqrstuvwxyz0123456789"]
        for name in firsts[::2]:
            (c.user / name).symlink_to(c.user / "save" / USER_ID)
        (c.user / "save" / USER_ID).mkdir(parents=True)
        (c.user / "save" / USER_ID / "account.sav").write_text("my save", encoding="ascii")
        (c.user / "save" / "latest").symlink_to(USER_ID)
        (c.user / "last").symlink_to(c.user / "save" / USER_ID)
        for name in firsts[1::2]:
            (c.user / name).symlink_to(c.user / "save" / USER_ID)
        c.moved("save/latest")
        for link in [c.new / "save" / "latest", c.new / "last"] + [c.new / name for name in firsts]:
            c.check(link.is_symlink() and link.resolve() == (c.new / "save" / USER_ID).resolve(), "%s names another folder" % link.name)

        c = case("a link that names nothing")
        (c.user / "old").symlink_to("../gone")
        (c.user / "save" / USER_ID).mkdir(parents=True)
        (c.user / "save" / USER_ID / "account.sav").write_text("my save", encoding="ascii")
        c.moved("save/" + USER_ID)
        c.check(os.readlink(c.new / "old") == "../gone", "it did not go as it was written")

        other = Path("/dev/shm")
        drives = other.is_dir() and os.access(other, os.W_OK) and other.stat().st_dev != tmp.stat().st_dev
        if drives:
            with tempfile.TemporaryDirectory(prefix="saves-move-", dir=other) as far:
                c = case("a link, the game on another drive", Path(far))
                (c.user / "save").symlink_to(c.sync)
                c.moved("save/" + USER_ID)
                c.check((c.new / "save").resolve() == c.sync.resolve(), "the new place has no link to it")
                c.saves_kept()

                c = case("no link, the game on another drive", Path(far))
                (c.user / "save" / USER_ID).mkdir(parents=True)
                (c.user / "save" / USER_ID / "account.sav").write_text("my save", encoding="ascii")
                c.moved("save/" + USER_ID)
                c.check(not (c.new / "save").is_symlink(), "a folder became a link")

        c = case("user/ is a link itself")
        kept = c.game / "kept"
        c.user.rename(kept)
        (kept / "save").symlink_to(c.sync)
        c.user.symlink_to(kept)
        for n in (1, 2):
            c.check(c.start() == c.user, "start %d left the player's place" % n)
        c.check((kept / "temp").is_dir() and (kept / "save").is_symlink() and not c.new.joinpath("temp").exists(), "something moved")
        c.saves_kept()

        c = case("the new place has an entry already")
        (c.user / "save" / USER_ID).mkdir(parents=True)
        (c.user / "cache").mkdir()
        (c.new / "temp").mkdir(parents=True)
        for n in (1, 2):
            c.check(c.start() == c.user, "start %d left the old place" % n)
        c.check((c.user / "cache").is_dir() and (c.user / "save" / USER_ID).is_dir() and not (c.new / "cache").exists()
                and not (c.new / "save").exists(), "an entry went to the new place all the same")

        if os.geteuid() != 0:   # (root reads the unreadable file)
            cannot_go(case, "an entry that cannot go", tmp)
            if drives:
                with tempfile.TemporaryDirectory(prefix="saves-move-", dir=other) as far:
                    cannot_go(case, "an entry that cannot go, the game on another drive", Path(far))
            else:
                print("an entry that cannot go, the game on another drive: not tried (no other drive here: /dev/shm is "
                      "missing, read-only or the temporary folder's own)")

        steps = cut_anywhere(case, tmp)
        if drives:
            with tempfile.TemporaryDirectory(prefix="saves-move-", dir=other) as far:
                cut_anywhere(case, Path(far), ", the game on another drive")
        if os.geteuid() != 0:   # (root removes the held file)
            delete_fails(case)
        compile_windows(tmp)

        c = case("save-work stays behind")
        (c.user / "save" / USER_ID).mkdir(parents=True)
        (c.user / "save" / USER_ID / "account.sav").write_text("committed", encoding="ascii")
        work = c.user / "save-work" / USER_ID
        work.mkdir(parents=True)
        (work / "account.sav").write_text("uncommitted", encoding="ascii")
        for n in (1, 2):
            c.check(c.start() == c.new, "start %d did not move committed saves" % n)
        c.check((c.new / "save" / USER_ID / "account.sav").read_text() == "committed", "committed bytes changed")
        c.check(work.is_dir() and not (c.new / "save-work").exists(), "save-work was copied or removed")

        c = case("ownership file is not pending work")
        (c.user / "save" / USER_ID).mkdir(parents=True)
        (c.user / "save" / USER_ID / "account.sav").write_text("committed", encoding="ascii")
        work = c.user / "save-work"
        work.mkdir()
        lock = work / (USER_ID + ".lock")
        lock.touch()
        (work / (USER_ID + ".journal.failed")).write_text("retired", encoding="ascii")
        for n in (1, 2):
            c.check(c.start() == c.new, "start %d treated the ownership file as pending" % n)
        c.check(lock.exists() and not (c.new / "save-work").exists(), "ownership file was moved or removed")
        c.check((c.new / "save" / USER_ID / "account.sav").read_text() == "committed", "committed bytes changed")

        c = case("pending journal keeps C beside its recovery data")
        (c.user / "save" / USER_ID).mkdir(parents=True)
        (c.user / "save-work").mkdir()
        journal = c.user / "save-work" / (USER_ID + ".journal")
        journal.write_text("R account.sav\nE\n")
        c.check(c.start() == c.user, "a pending commit was separated from its journal")
        c.check((c.user / "save" / USER_ID).exists() and journal.exists(), "recovery data moved")

        c = case("no link")
        (c.user / "save" / USER_ID).mkdir(parents=True)
        (c.user / "save" / USER_ID / "account.sav").write_text("my save", encoding="ascii")
        c.moved("save/" + USER_ID)
        c.check((c.new / "temp").is_dir() and not (c.new / "save").is_symlink(), "the folders did not move as they were")

    print("PASS: the saves' move makes a link again in the new place and leaves its folder alone%s; a linked user/ and a "
          "new place with entries keep the old place whole; a start cut at any of the move's %d steps, or a delete that "
          "fails, leaves the saves whole in the place the next start uses" % (
              ", from another drive too" if drives else " (no other drive to try here)", steps))


if __name__ == "__main__":
    main()
