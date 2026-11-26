"""The player's settings file (host/options_path.c): the profile's settings.ini (LONGSHOT_PROFILE, longshot-builder's play),
the one place the game takes them from, so every build the profile plays reads the same one; beside the game without a
profile. Built as the game builds it (Windows with its own headers, Linux with the POSIX shim) and run from a made-up game
folder. No game: authored placeholder files only.

- a profile (LONGSHOT_PROFILE): its settings.ini (and the copy below goes there). Nothing names another file: --user
  (whatever the words around it) and the switch that used to (HOS_OPTIONS_FILE) change nothing;
- no profile (a developer's game folder, a test): beside the game, as before, whatever the working folder;
- chosen once;
- the copy: the game folder's file (a build's from before) into the profile when that has none, and the log says where the
  file is. Not a file with only the builder's compile_shaders=, not over a file there, not without a profile; a profile that
  cannot be written keeps the game folder's for the run, and the log names the error that stopped the copy. The copy is whole
  or not there: a process killed at each step of it leaves no file, or the whole one, and the next start copies again;
- the start (host/hos_main.c recover_writes): a "<name>.hosnew" that a killed write of a setting left is removed, beside the
  settings file the run chose (the profile's, or the game folder's without one: the options' one-time copy leaves it there),
  language.txt and mouse_sens.txt, and the settings are left as they are.

python -I -B tests/test_options_path.py [--cc gcc]
"""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

RUN_ALL_ARGS = ["--cc", "gcc"]
ROOT = Path(__file__).resolve().parents[1]
WINDOWS = sys.platform == "win32"
EXE = "longshot.exe" if WINDOWS else "longshot"

MAIN = r'''
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "hos_paths.h"   /* (first of the game's: windows.h, host/posix's on Linux) */
#include "options_path.h"
#include "hos_atomic_file.h"
#ifdef _WIN32
#define SEP '\\'
#define SEP_S "\\"
#else
#include <signal.h>
#include <unistd.h>
#define SEP '/'
#define SEP_S "/"
#endif
const char *hos_user_dir;
static long kill_step, steps;
/* (the helper's kill points, as tests/settings_atomic_test.c: HOS_AF_STEP is defined on the compile line) */
void hos_test_step(const char *what) {
    (void)what;
    if (++steps != kill_step) return;
#ifdef _WIN32
    TerminateProcess(GetCurrentProcess(), 137);
#else
    kill(getpid(), SIGKILL);
#endif
}
void hos_log(const char *fmt, ...) { va_list ap; va_start(ap, fmt); printf("log: "); vprintf(fmt, ap); printf("\n"); va_end(ap); }
@RECOVER@
int main(int argc, char **argv) {
    if (getenv("T_KILL")) kill_step = atol(getenv("T_KILL"));
    hos_user_dir = getenv("T_USER");
    (void)argc, (void)argv;
    hos_options_choose();
    hos_options_choose();   /* (once a process: the second call changes nothing) */
    if (getenv("T_RECOVER")) recover_writes();   /* (host/hos_main.c, as the start calls it: after the choice, before the copy) */
    if (getenv("T_CARRY")) hos_options_carry(), hos_options_carry();
    printf("steps: %ld\n", steps);
    printf("file: %s\n", hos_options_file());
    return 0;
}
'''


def recover_writes_text():
    """recover_writes() of host/hos_main.c, as it is (cut out by its text, as the saves' other fixtures cut nn_fs.c)"""
    source = (ROOT / "host" / "hos_main.c").read_text(encoding="utf-8")
    start = source.find("static void recover_writes(void) {")
    assert start >= 0, "host/hos_main.c has no recover_writes()"
    return source[start:source.index("\n}\n", start) + 3]


def same(a, b):
    return os.path.normcase(os.path.normpath(str(a))) == os.path.normcase(os.path.normpath(str(b)))


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default="gcc")
    a = p.parse_args()
    cc = shutil.which(a.cc) or shutil.which("clang") or a.cc
    if not (ROOT / "host" / "hos_atomic_file.h").exists():   # (a tree from before the change: nothing here to kill)
        raise SystemExit("test_options_path: host/hos_atomic_file.h is not there: the options' one-time copy and the start's clean-up do not use it")
    with tempfile.TemporaryDirectory(prefix="longshot-options-path-") as tmp:
        tmp = Path(tmp)
        (tmp / "main.c").write_text(MAIN.replace("@RECOVER@", recover_writes_text()))
        # (the helper's kill points call the program's hos_test_step: options_path.c is compiled with the hook, as the game with none)
        (tmp / "step.h").write_text('void hos_test_step(const char *what);\n#define HOS_AF_STEP(what) hos_test_step(what)\n')
        hook = ["-include", str(tmp / "step.h")]
        built = tmp / EXE
        if WINDOWS:
            subprocess.run([cc, "-std=gnu11", "-I", str(ROOT / "host"), "-I", str(ROOT / "game"), "-Wall", "-Wextra", "-Werror", *hook,
                            str(ROOT / "host" / "options_path.c"), str(tmp / "main.c"), "-o", str(built)], check=True)
        else:
            inc = ["-std=gnu11", "-I", str(ROOT / "host" / "posix"), "-I", str(ROOT / "host"), "-I", str(ROOT / "game"), *hook]
            obj = tmp / "options_path.o"   # (no warning in it; the shim is built as the game builds it)
            subprocess.run([cc, *inc, "-Wall", "-Wextra", "-Werror", "-c", str(ROOT / "host" / "options_path.c"), "-o", str(obj)],
                           check=True)
            subprocess.run([cc, *inc, str(obj), str(ROOT / "host" / "posix" / "win32_posix.c"), str(tmp / "main.c"), "-o", str(built),
                            "-lpthread", "-ldl"], check=True)

        def game_folder(name, beside_image=True):
            game = tmp / name / "game"
            game.mkdir(parents=True)
            if beside_image:
                (game / "image.bin").write_text("image")
            shutil.copy2(built, game / EXE)
            return game

        cwd = tmp / "cwd"
        cwd.mkdir()

        def run(game, *args, carry=False, recover=False, kill=0, **env):
            e = {k: v for k, v in os.environ.items()
                 if k not in ("HOS_OPTIONS_FILE", "LONGSHOT_PROFILE", "T_CARRY", "T_RECOVER", "T_KILL", "T_USER")}
            if carry:
                e["T_CARRY"] = "1"
            if recover:
                e["T_RECOVER"] = "1"
            if kill:
                e["T_KILL"] = str(kill)
            e.update(env)
            r = subprocess.run([str(game / EXE), *args], cwd=cwd, env=e, capture_output=True, text=True, timeout=60)
            if kill:   # (the process kills itself at that step: its exit says so, or that it took fewer steps)
                return r.returncode, r.stdout
            assert r.returncode == 0, r.stderr
            return r.stdout.split("file: ", 1)[1].strip(), r.stdout

        def want(got, path, what):
            assert same(got, path), "%s: %s, not %s" % (what, got, path)

        g = game_folder("a")
        beside = g / "longshot_options.ini"
        want(run(g)[0], beside, "started with nothing")
        want(run(g, "image.bin")[0], beside, "an image named, no profile")
        want(run(game_folder("b", beside_image=False))[0], tmp / "b" / "game" / "longshot_options.ini", "no image.bin beside")
        want(run(g, "--user", str(tmp / "u"))[0], beside, "--user names no settings file")
        want(run(g, HOS_OPTIONS_FILE=str(tmp / "own.ini"))[0], beside, "HOS_OPTIONS_FILE is gone")
        # a profile (LONGSHOT_PROFILE, longshot-builder's play): its settings.ini, and nothing else names another
        prof = tmp / "profiles" / "ann"
        want(run(g, LONGSHOT_PROFILE=str(prof))[0], prof / "settings.ini", "a profile")
        want(run(g, LONGSHOT_PROFILE=str(prof) + os.sep)[0], prof / "settings.ini", "a profile, a separator at its end")
        want(run(g, "--romfs", "--user", "--user", str(tmp / "u"), LONGSHOT_PROFILE=str(prof))[0], prof / "settings.ini",
             "--user is not over the profile")
        want(run(g, "image.bin", "--user", "rel", LONGSHOT_PROFILE=str(prof))[0], prof / "settings.ini",
             "a relative --user with an image is not over the profile")
        want(run(g, LONGSHOT_PROFILE=str(prof), HOS_OPTIONS_FILE=str(tmp / "own.ini"))[0], prof / "settings.ini",
             "HOS_OPTIONS_FILE is not over the profile")
        want(run(g, LONGSHOT_PROFILE="")[0], beside, "an empty LONGSHOT_PROFILE: none")

        # the copy: a build's from before into the profile, once; the log says where the file is
        g = game_folder("d")
        (g / "longshot_options.ini").write_text("dlss=1\nframe_generation=2\ncompile_shaders=1\n")
        prof = tmp / "d" / "profile"
        got, out = run(g, carry=True, LONGSHOT_PROFILE=str(prof))
        want(got, prof / "settings.ini", "after the copy")
        assert (prof / "settings.ini").read_text() == "dlss=1\nframe_generation=2\ncompile_shaders=1\n", out
        assert out.count("options: the settings file is ") == 1 and str(prof) in out, out
        assert out.count("were copied to") == 1, out
        (prof / "settings.ini").write_text("dlss=4\n")   # (the player's since: never written over)
        got, out = run(g, carry=True, LONGSHOT_PROFILE=str(prof))
        assert (prof / "settings.ini").read_text() == "dlss=4\n" and "copied" not in out, out
        got, out = run(g, carry=True)   # (no profile: the game folder's, as before, nothing copied)
        want(got, g / "longshot_options.ini", "no profile, after a copy")
        assert "copied" not in out and str(g / "longshot_options.ini") in out, out

        g = game_folder("e")
        (g / "longshot_options.ini").write_text("compile_shaders=1\n")   # (the builder's, a new folder: not the player's)
        prof = tmp / "e" / "profile"
        got, out = run(g, carry=True, LONGSHOT_PROFILE=str(prof))
        assert not (prof / "settings.ini").exists() and "copied" not in out, out

        g = game_folder("f")
        (g / "longshot_options.ini").write_text("dlss=1\n")
        blocked = tmp / "f" / "blocked"
        blocked.write_text("a file where the profile would be")
        got, out = run(g, carry=True, LONGSHOT_PROFILE=str(blocked))
        want(got, g / "longshot_options.ini", "a profile that cannot be written")
        assert "could not be copied" in out, out

        def copy_is_whole_or_not_there():
            """the copy, killed at each step it takes: the profile has no settings file or the whole one, the next start copies"""
            g = game_folder("k")
            original = "".join("setting_%03d=%d\n" % (i, i * 7) for i in range(400)) + "dlss=1\n"
            (g / "longshot_options.ini").write_text(original)
            prof = tmp / "k" / "profile"
            got, out = run(g, carry=True, LONGSHOT_PROFILE=str(prof))
            steps = int(out.split("steps: ", 1)[1].split()[0])
            assert steps >= 1, "the copy took no step the helper can be killed at (host/hos_atomic_file.h): it copies in place"
            assert (prof / "settings.ini").read_text() == original, out
            outcomes = set()
            for k in range(1, steps + 1):
                shutil.rmtree(prof)
                code, _ = run(g, carry=True, kill=k, LONGSHOT_PROFILE=str(prof))
                assert code == (137 if WINDOWS else -9), "kill step %d: exit %s, not killed" % (k, code)
                ini = prof / "settings.ini"
                assert not ini.exists() or ini.read_text() == original, \
                    "kill step %d: the profile's settings file is %d bytes, not none or the whole %d" % (k, ini.stat().st_size, len(original))
                outcomes.add(ini.exists())
                got, out = run(g, carry=True, LONGSHOT_PROFILE=str(prof))
                assert ini.read_text() == original, "after kill step %d the next start left %d bytes" % (k, ini.stat().st_size)
                assert not (prof / "settings.ini.hosnew").exists(), "after kill step %d the next copy left a stand-in" % k
            assert outcomes == {False, True}, "the kills saw only %s: a kill before the move and one after it were both expected" % outcomes
            return "%d kill steps, each left no file or the whole one" % steps

        def start_removes_what_a_kill_left():
            """recover_writes (hos_main.c) removes what a killed write of a setting left; the settings stay as they are"""
            g = game_folder("r")
            user, prof = tmp / "r" / "user-folder", tmp / "r" / "profile"
            user.mkdir(parents=True)
            prof.mkdir(parents=True)
            own = tmp / "r" / "own.ini"   # (a settings file no run reads: HOS_OPTIONS_FILE named one)
            files = [(prof / "settings.ini", "dlss=4\n"), (user / "mouse_sens.txt", "2\n"),
                     (g / "language.txt", "language=de\n"), (own, "dlss=2\n")]
            for f, text in files:
                f.write_text(text)
                Path(str(f) + ".hosnew").write_text("half a write")
            run(g, "--user", str(user), recover=True, T_USER=str(user), LONGSHOT_PROFILE=str(prof),
                HOS_OPTIONS_FILE=str(own))
            for f, text in files[:-1]:
                assert not Path(str(f) + ".hosnew").exists(), "the start left %s.hosnew (in %s)" % (f.name, f.parent.name)
                assert f.read_text() == text, "the start changed %s" % f.name
            assert Path(str(own) + ".hosnew").exists() and own.read_text() == "dlss=2\n", \
                "a settings file that no run reads was cleaned (HOS_OPTIONS_FILE)"
            Path(str(g / "longshot_options.ini") + ".hosnew").write_text("half a write")   # (no profile: the game folder's file is the chosen one)
            run(g, recover=True, T_USER=str(user))
            assert not Path(str(g / "longshot_options.ini") + ".hosnew").exists(), "the stand-in beside the game was left"
            return "the stand-ins of the profile's settings, the game folder's, language.txt and mouse_sens.txt removed"

        def failed_copy_says_why():
            """a profile that refuses a new file: the log names that error (access denied), not the clean-up's (no such file)"""
            if WINDOWS or os.geteuid() == 0:
                return "skipped (a read-only folder refuses nothing here)"
            g = game_folder("w")
            (g / "longshot_options.ini").write_text("dlss=1\n")
            prof = tmp / "w" / "read-only"
            prof.mkdir(parents=True)
            prof.chmod(0o555)
            try:
                got, out = run(g, carry=True, LONGSHOT_PROFILE=str(prof))
            finally:
                prof.chmod(0o755)
            want(got, g / "longshot_options.ini", "a profile that refuses a new file")
            assert "could not be copied" in out and "(error 5)" in out, out
            assert not (prof / "settings.ini").exists() and not (prof / "settings.ini.hosnew").exists(), "a file was left"
            return "the log says error 5, no file or stand-in left"

        failed = []
        for check in (copy_is_whole_or_not_there, start_removes_what_a_kill_left, failed_copy_says_why):
            try:
                print("%s: %s" % (check.__name__, check()))
            except AssertionError as e:
                failed.append("%s: %s" % (check.__name__, e))
        for line in failed:
            print("FAIL: " + line)
        if failed:
            raise SystemExit("test_options_path: %d checks failed" % len(failed))
    print("PASS: the settings in the profile's settings.ini (nothing else names a file: not --user, not HOS_OPTIONS_FILE; "
          "without a profile beside the game, as before), chosen once; a build's from before copied there once, whole or not at "
          "all at any kill step (not compile_shaders= alone, not over a file), the log saying where it is; a killed write's "
          ".hosnew removed at the start")


if __name__ == "__main__":
    main()
