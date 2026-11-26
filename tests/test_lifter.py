"""The Rust workspace's checks, the same three as CI's `rust` job and `mise run rust`: its formatting (rustfmt), its lints
(clippy, warnings as errors) and `cargo test` of the whole workspace (the builder, its lifter and longshot-rs). Here so
that tests/run_all.py runs them with the other checks, with two differences from that job: llvm-mc is named, so the
A64 decoder's check against it word for word (builder/crates/a64/tests/llvm_mc.rs) runs rather than skips, and the
tests run in release. Game-free: every word is generated.

    python -I tests/test_lifter.py --cargo <cargo> --llvm-mc <llvm-mc> [--words N]

--locked: the crates Cargo.lock pins, by checksum. In cargo's own target folder, target/ at the root (which .gitignore leaves
out; CARGO_TARGET_DIR overrides it). --words is the random words per encoding group (the check's default 40000, about
0.8 million words in all).
"""
import argparse
import os
import subprocess
import sys

RUN_ALL_ARGS = ["--cargo", "{cargo}", "--llvm-mc", "{llvm_mc}"]

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LIFTER = os.path.join(ROOT, "builder")


def run(cmd, env):
    print("test_lifter: " + " ".join(cmd[1:]), flush=True)
    return subprocess.run(cmd, cwd=LIFTER, env=env).returncode


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--cargo", required=True)
    ap.add_argument("--llvm-mc", required=True)
    ap.add_argument("--words", type=int)
    a = ap.parse_args()
    env = dict(os.environ, LLVM_MC=a.llvm_mc)   # (named, it must run: the check fails rather than skips)
    if a.words:
        env["LIFTER_WORDS"] = str(a.words)
    steps = [
        [a.cargo, "fmt", "--all", "--check"],
        [a.cargo, "clippy", "--locked", "--workspace", "--all-targets", "--", "-D", "warnings"],
        [a.cargo, "test", "--locked", "--release", "--workspace"],
    ]
    failed = [" ".join(s[1:3]) for s in steps if run(s, env)]
    if failed:
        print("test_lifter: failed: %s" % ", ".join(failed))
        return 1
    print("test_lifter: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
