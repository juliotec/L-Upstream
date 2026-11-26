"""The guest profile's counters written by two threads at once (host/guest_profile.c): the timer thread's writes and
the one at exit wrote the same .tmp file together, so a file could come out cut or mixed, or a write fail. Now each
is whole, none fails, and none is written after the one at exit. Builds tests/guest_profile_test.c (the file as it is,
over host/posix's Windows calls) and runs it; no game.

python -I -B tests/test_guest_profile_write.py [--cc gcc]
"""
import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "{cc}"]
RUN_ALL_SKIP = {"windows": "built over host/posix (Linux)"}
ROOT = Path(__file__).resolve().parents[1]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--cc", default=os.environ.get("CC", "gcc"))
    a = p.parse_args()
    with tempfile.TemporaryDirectory(prefix="guest-profile-") as tmp:
        exe = Path(tmp) / "guest_profile_test"
        subprocess.run([a.cc, "-std=gnu11", "-O1", "-Wall", "-Wextra", "-Werror", "-D_GNU_SOURCE", "-pthread", "-I",
                        str(ROOT / "host/posix"), str(ROOT / "tests/guest_profile_test.c"), "-o", str(exe)], check=True)
        run = subprocess.run([str(exe), str(Path(tmp) / "profile.txt")], capture_output=True, text=True, timeout=120)
        sys.stdout.write(run.stdout)
        if run.returncode or "cannot be written" in run.stderr:
            print(run.stderr[-2000:])
            print("FAIL: the guest profile's writes at once")
            return 1
    print("PASS: the guest profile written by two threads at once: each file whole, none after the last")
    return 0


if __name__ == "__main__":
    sys.exit(main())
