"""Game-free: the host's C (host/, game/, host/posix/ or the Windows files) compiled for a system with the exact
commands a player's build gives it, so the other system's branches of host code are compiled on every change. CI runs it
for Windows with llvm-mingw on its Linux runner (.forgejo/workflows/ci.yml, host-windows); nothing is linked and no game
file is read. Python's standard library only.

    RX_TARGET=windows LONGSHOT_MINGW=<llvm-mingw> RX_CPM=<CPM cache> \\
        python -I scripts/check_host_compile.py --work <scratch folder> [--cargo cargo] [--jobs N]

The commands are build-compile's (its `plan` example, `--mode dry-run --commands`: every unit, its flags, nothing
compiled), in a work folder whose gen/ holds only the lift's runtime headers: builder/rt/ and an a64_config.h with the lift's
switches at placeholder values (lift::whole::files config_h's form; their values change no branch the compiler checks).
Each host unit is then compiled with -fsyntax-only, as many at once as --jobs.
"""
import argparse
import concurrent.futures
import os
import shutil
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))   # (python -I leaves the script's folder off the path)
from helpers import ROOT  # noqa: E402

CONFIG_H = """/* check_host_compile.py: placeholder values of the lift's choices (lift::whole::files config_h) */
#ifndef A64_CONFIG_H
#define A64_CONFIG_H
#define A64_HOST_BASE 0x100000000ULL
#define A64_DIRTY_PAGES 0x20000000000ULL
#define A64_WRITE_TRACK 1
#define A64_MARK_BITMAP 1
#define A64_POLLING 1
#endif
"""
# (a unit's line in the plan's output, then its command on the next, indented)
FATES = ("compile ", "stored ", "up to date ")


def synthetic_gen(work):
    rt = os.path.join(work, "gen", "rt")
    shutil.rmtree(os.path.join(work, "gen"), ignore_errors=True)
    os.makedirs(rt)
    for f in os.listdir(os.path.join(ROOT, "builder", "rt")):
        shutil.copy2(os.path.join(ROOT, "builder", "rt", f), rt)
    with open(os.path.join(rt, "a64_config.h"), "w", encoding="utf-8") as f:
        f.write(CONFIG_H)


def rejoin(words):
    """The plan prints a command's arguments joined by spaces, unquoted: an argument with spaces in it (the version's
    define, -DLONGSHOT_VERSION="v3.1 (source ...)") is joined again, its double quotes balanced"""
    out = []
    for w in words:
        if out and out[-1].count('"') % 2:
            out[-1] += " " + w
        else:
            out.append(w)
    return out


def host_commands(plan_text):
    """[(source, command)] of the plan's units whose source is the host's C"""
    out, lines = [], plan_text.splitlines()
    host = os.path.join(ROOT, "host") + os.sep
    for i, line in enumerate(lines):
        if not line.startswith(FATES) or i + 1 >= len(lines) or not lines[i + 1].startswith("    "):
            continue
        cmd = rejoin(lines[i + 1].split())
        src = next((a for a in cmd if a.endswith((".c", ".cpp")) and not a.startswith("-")), None)
        if src and os.path.normpath(src).startswith(host):
            out.append((src, cmd))
    return out


def syntax_only(cmd):
    """The command compiling nothing: no object, no depfile, -fsyntax-only"""
    out, skip = [], False
    for a in cmd:
        if skip:
            skip = False
        elif a in ("-o", "-MF", "-MT", "-MQ"):
            skip = True
        elif a not in ("-MD", "-MMD", "-c"):
            out.append(a)
    return out + ["-fsyntax-only"]


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--work", required=True, help="a scratch folder (its gen/ and build/ are made again)")
    p.add_argument("--cargo", default="cargo")
    p.add_argument("--jobs", type=int, default=os.cpu_count() or 1)
    p.add_argument("--launcher", help="a program each compile runs under, e.g. ccache (CI: ccache, with CCACHE_BASEDIR "
                                      "the checkout and --work in it, so the commands are the same every run)")
    a = p.parse_args(argv)
    work = os.path.abspath(a.work)
    synthetic_gen(work)
    plan = subprocess.run([a.cargo, "run", "--quiet", "--locked", "-p", "build-compile", "--example", "plan", "--",
                           "--work", work, "--root", ROOT, "--mode", "dry-run", "--commands"],
                          cwd=os.path.join(ROOT, "builder"), stdout=subprocess.PIPE, text=True)
    if plan.returncode:
        print("check_host_compile: the plan failed (above)", file=sys.stderr)
        return 1
    units = host_commands(plan.stdout)
    if not units:
        print("check_host_compile: the plan names no host unit", file=sys.stderr)
        return 1

    def one(unit):
        cmd = syntax_only(unit[1])
        r = subprocess.run(([a.launcher] if a.launcher else []) + cmd, capture_output=True, text=True, errors="replace")
        return unit[0], r.returncode, r.stdout + r.stderr

    failed = 0
    with concurrent.futures.ThreadPoolExecutor(max(1, a.jobs)) as pool:
        for src, rc, said in pool.map(one, units):
            if rc:
                failed += 1
                print("FAILED %s\n%s" % (os.path.relpath(src, ROOT), said.strip()[-4000:]), flush=True)
    print("check_host_compile: %d host units, %d failed" % (len(units), failed))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
