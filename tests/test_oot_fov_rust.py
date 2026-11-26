"""Field of view in Rust (game/crates/oot/src/fov.rs) against the C it replaced (tests/fixtures/oot_fov.c), case for case,
without a game.

tests/oot_fov_parity.c is linked with the fixture (renamed c_hos_oot_fov) and liblongshot_rs.a (cargo build -p longshot-rs
--profile game: the library the game links), and runs both over the same made-up guest memory on every case: each
percentage from 49 to 151 and some no row has, angles of 0, pi, NaN, the infinities and the extremes, and each way the
camera can be refused (a null camera, no views, the wrong vtable, another camera than the main one, CameraMgr with no
camera or none, each of its pointers null, an event running) or applied with no EventMgr. V0's two halves and guest
memory must be the same after each, and the Rust may read nothing outside the made-up memory (a null it followed, where
the C would crash).

Then the check must fail where the Rust is wrong: five changes planted in a copy of the Rust (its clamp's upper bound,
the event check left out, the 100 % shortcut gone, no EventMgr taken as an event, a null first camera followed), each
built and run in a workspace of its own, must each give a mismatch. And the Rust's option id is game/options.h's.

python -I -B tests/test_oot_fov_rust.py [--cc gcc] [--cargo cargo]
"""
import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

RUN_ALL_ARGS = ["--cc", "{cc}", "--cargo", "{cargo}"]
RUN_ALL_SKIP = {"windows": "the parity program's guest memory is laid out for Linux; the Rust is the same on both"}

ROOT = Path(__file__).resolve().parents[1]
FOV_RS = Path("game") / "crates" / "oot" / "src" / "fov.rs"
CONFIG_H = """/* test_oot_fov_rust.py: the lift's choices for the fixture (a64.h): guest memory at fake_base, no page marks */
#ifndef A64_CONFIG_H
#define A64_CONFIG_H
#include <stdint.h>
extern uint8_t *fake_base;
#define A64_HOST_BASE ((uintptr_t)fake_base)
#define A64_DIRTY_PAGES 0x100ULL
#define A64_WRITE_TRACK 1
#define A64_MARK_BITMAP 0
#define A64_POLLING 1
#define A64_NO_MARKS 1
#endif
"""
FIXTURE_H = """/* test_oot_fov_rust.py: what the fixture had of game_host.h */
#pragma once
#include "a64.h"
int rx_option(int id, int otherwise);
#include "options.h"
"""
# (each: what to change in fov.rs, what it is)
PLANTED = (
    # (a bound that is another f32: 2.443_460_9 would round to the same one)
    ("const LENS_MAX: f32 = 2.443_461;", "const LENS_MAX: f32 = 2.44;", "the clamp's upper bound"),
    ("if !is_main_camera_view(g, view) || event_running(g) {", "if !is_main_camera_view(g, view) {", "the event check"),
    ("if percent == 100 || !PERCENT.contains(&percent)", "if !PERCENT.contains(&percent)", "the 100 % shortcut"),
    ("let events = if holder.is_null() { return Some(false) }", "let events = if holder.is_null() { return Some(true) }",
     "no EventMgr taken as an event"),
    ("if main.is_null() { None } else { g.read_ptr(main.field::<GuestPtr<View>>(MGR_CAMERA_VIEW)) }",
     "g.read_ptr(main.field::<GuestPtr<View>>(MGR_CAMERA_VIEW))", "a null first camera followed"),
)


def fail(what):
    raise SystemExit("FAIL: " + what)


def cargo_build(cargo, workspace, target_dir):
    """liblongshot_rs.a of the workspace, built as the game links it"""
    r = subprocess.run([cargo, "build", "--quiet", "--offline", "--profile", "game", "-p", "longshot-rs",
                        "--manifest-path", str(workspace / "Cargo.toml")],
                       env=dict(os.environ, CARGO_TARGET_DIR=str(target_dir)), capture_output=True, text=True)
    if r.returncode:
        fail("cargo build -p longshot-rs failed:\n" + r.stderr[-4000:])
    lib = target_dir / "game" / "liblongshot_rs.a"
    if not lib.is_file():
        fail("no %s after the build" % lib)
    return lib


def parity(cc, lib, tmp):
    """The program's output and status, the fixture against the library lib"""
    inc = tmp / "include"
    inc.mkdir(exist_ok=True)
    (inc / "a64_config.h").write_text(CONFIG_H, encoding="utf-8")
    (inc / "oot_fov_fixture.h").write_text(FIXTURE_H, encoding="utf-8")
    exe, fixture = tmp / "parity", tmp / "fixture.o"
    flags = [cc, "-std=c11", "-O1", "-Wall", "-Wextra", "-Werror", "-fno-strict-aliasing",
             "-I", str(inc), "-I", str(ROOT / "builder" / "rt"), "-I", str(ROOT / "game")]
    # (the fixture alone renamed: c_hos_oot_fov beside the Rust's hos_oot_fov)
    for cmd in (flags + ["-c", "-Dhos_oot_fov=c_hos_oot_fov", str(ROOT / "tests" / "fixtures" / "oot_fov.c"), "-o", str(fixture)],
                flags + [str(ROOT / "tests" / "oot_fov_parity.c"), str(fixture), str(lib), "-lpthread", "-ldl", "-lm",
                         "-o", str(exe)]):
        r = subprocess.run(cmd, capture_output=True, text=True)
        if r.returncode:
            fail("the parity program does not build:\n" + r.stderr[-4000:])
    run = subprocess.run([str(exe)], capture_output=True, text=True, timeout=600)
    return run.returncode, run.stdout


def planted_workspace(tmp, old, new):
    """A workspace of the game's Rust alone (its crates copied, a Cargo.toml of its own), with old replaced by new in fov.rs"""
    ws = tmp / "planted"
    shutil.rmtree(ws, ignore_errors=True)
    for part in (Path("longshot-rs"), Path("game") / "crates"):
        shutil.copytree(ROOT / part, ws / part, ignore=shutil.ignore_patterns("target", "prebuilt"))
    root_toml = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    keep = []
    for head in ("[workspace.package]", "[workspace.lints.rust]", "[workspace.lints.clippy]", "[profile.release]",
                 "[profile.game]"):
        m = re.search(re.escape(head) + r"\n(.*?)(?=\n\[|\Z)", root_toml, re.S)
        if not m:
            fail("Cargo.toml has no %s for the planted workspace" % head)
        keep.append(head + "\n" + m.group(1).strip() + "\n")
    (ws / "Cargo.toml").write_text('[workspace]\nresolver = "3"\nmembers = ["game/crates/guest", "game/crates/oot", '
                                   '"longshot-rs"]\n\n' + "\n".join(keep), encoding="utf-8")
    fov = ws / FOV_RS
    text = fov.read_text(encoding="utf-8")
    if text.count(old) != 1:
        fail("%s does not hold %r once: the planted change no longer fits it" % (FOV_RS, old))
    fov.write_text(text.replace(old, new), encoding="utf-8")
    return ws


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--cc", default="gcc")
    p.add_argument("--cargo", default="cargo")
    a = p.parse_args()

    options_h = (ROOT / "game" / "options.h").read_text(encoding="utf-8")
    rust_ids = (ROOT / "game" / "crates" / "oot" / "src" / "options.rs").read_text(encoding="utf-8")
    want = re.search(r"HOS_OPT_FOV = (\d+)", options_h)
    have = re.search(r"pub const FOV: i32 = (\d+);", rust_ids)
    if not want or not have or want.group(1) != have.group(1):
        fail("the Rust's FOV option id (game/crates/oot/src/options.rs) is not game/options.h's HOS_OPT_FOV")
    print("PASS: the Rust's option id is game/options.h's (%s)" % want.group(1))

    with tempfile.TemporaryDirectory(prefix="oot-fov-rust-") as t:
        tmp = Path(t)
        # (the tree's own Rust, in cargo's target folder: built once, then quick)
        lib = cargo_build(a.cargo, ROOT, ROOT / "target" / "oot-fov-parity")
        status, out = parity(a.cc, lib, tmp)
        if status:
            fail("the Rust and the C differ:\n" + out)
        print("PASS: the Rust does what the C did: " + out.strip().splitlines()[-1])
        for old, new, what in PLANTED:
            ws = planted_workspace(tmp, old, new)
            planted_lib = cargo_build(a.cargo, ws, tmp / "planted-target")
            status, out = parity(a.cc, planted_lib, tmp)
            if status == 0:
                fail("a change of %s planted in the Rust is not seen: %s" % (what, out.strip().splitlines()[-1]))
            print("PASS: a change of %s planted in the Rust is seen (%s)" % (what, out.strip().splitlines()[-1]))


if __name__ == "__main__":
    main()
