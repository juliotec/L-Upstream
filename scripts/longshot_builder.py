"""longshot-builder, the builder players' builders run (docs/dev/reference/builder.md), built from builder/ as one
program per system with the pinned Rust (scripts/renhdrum.py's RUST: each part's SHA-256 checked, kept in --cache) and
the crates Cargo.lock pins (each checked by its SHA-256; nothing else is asked of the network). Python's standard
library only.

    python -I scripts/longshot_builder.py build [--target linux|windows] [--profile dist|release] [--llvm-mingw <folder>]
                                              [--linker <cc>] [--cache <folder>] [--out <file>] [--jobs N]
    python -I scripts/longshot_builder.py check <file> [--target linux|windows]
    python -I scripts/longshot_builder.py llvm-mingw [<folder>] [--name] [--host linux]
    python -I scripts/longshot_builder.py link-tools <folder>

Only what must run before there is a longshot-builder stays here; the rest of a release's packaging is the builder's own,
`longshot-builder dev release` (builder/crates/longshot-builder/src/dev/release_cmd.rs: the pack it carries, `sources`
and `unpack`, whose one writer and reader is build-driver's sources.rs; `dry-run`; the earlier releases' files, the
deltas and the manifest, `fetch-bases`, `deltas`, `files`; RecompLauncher's zip, `recomplauncher`). CI runs it with the
Linux job's shipped longshot-builder before its pack is attached (`build --target linux`, the dist profile); the Windows
job takes that program from the Linux job as its tool.
- `build`: the program itself, from nothing but the pinned Rust (a builder cannot build its first self).
- `llvm-mingw`: llvm-mingw unpacked into <folder>, its SHA-256 checked, and its folder printed (--name: only its name, a
  cache key): the Linux build of tools.lock's release (builder/tools/pins.sh pins it), which links the Windows
  longshot-builder, so it is needed before that program exists. (On Windows, tools.lock's own llvm-mingw:
  `longshot-builder dev tools install . --system windows-x64 --only llvm-mingw`.)
- `link-tools`: tools.lock's linux-x64 `llvm` and `sysroot` from Longshot's store, each archive checked by its size and
  SHA-256 and unpacked into <folder>/<name>, each printed as `<name>=<folder>`: what links the Linux longshot-builder
  (`build --linker <llvm>/bin/clang --sysroot <sysroot>`), needed before there is a builder to install them with
  (`longshot-builder dev tools install`). Needs the zstd program (the archives are .tar.zst).
- `check`: a built program's headers, as `build` checks its own output: x86-64, and a Linux program that loads glibc's
  libraries only (GLIBC_2.35 at most: the game's floor, Ubuntu 22.04), a Windows one with Windows' own only. It reads the program before anything runs it (a Windows one cannot run on
  the Linux runner that built it) and shares scripts/renhdrum.py's readers of ELF and PE headers.

`build` compiles the longshot-builder package with Cargo.toml's profile (dist, the release's: thin LTO, 16 codegen
units, no debug info but its symbols; release, a developer's quicker one):
- linux (the default elsewhere): x86_64-unknown-linux-gnu, linked against glibc 2.35, the game's own floor: by
  --linker (default cc) against the system's glibc, or with --sysroot <folder> against tools.lock's `sysroot` (Ubuntu
  22.04's glibc and GCC's runtime pieces), as the game is (then --linker is the bundle's clang, which links with lld).
  Not static: its windows (builder/crates/ui, Slint's winit) load X11 or Wayland at run time (dlopen), which a static
  program can't; it loads glibc's own libraries and libgcc_s only;
- windows (the default on Windows): x86_64-pc-windows-gnullvm, linked by llvm-mingw (--llvm-mingw, else LONGSHOT_MINGW),
  the one the game is built with, its unwinder linked in and the C runtime Windows' own UCRT.
No time stamp and no path of this machine go in, and no pack: a release's program carries its pack appended after the
link (`longshot-builder dev release sources` makes it, `dev release attach` appends it: build-driver's sources.rs). Its
output (--out, default
build-<system>/longshot-builder[.exe]) is checked as `check` checks it.

The release builds both in CI (.forgejo/workflows/build-linux.yml, build-windows.yml) and publishes them as they are:
longshot-builder-<version>-linux-x64 and longshot-builder-<version>-windows-x64.exe. A developer runs `cargo build
--release -p longshot-builder` in builder/ with mise's Rust instead (scripts/devtools/packs.py does), which carries no
sources and builds from the checkout (--root).
"""
import argparse
import hashlib
import os
import shutil
import re
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))   # (python -I leaves the script's folder off the path)
import helpers  # noqa: E402
import renhdrum  # noqa: E402
from helpers import ROOT  # noqa: E402

RUST_DIR = os.path.join(ROOT, "builder")   # (the builder's crates; the Cargo workspace and its Cargo.lock are the root's)
TARGETS = {"linux": "x86_64-unknown-linux-gnu", "windows": "x86_64-pc-windows-gnullvm"}
STD = {"linux": "linux", "windows": "windows"}   # (renhdrum.RUST_STD's keys)
# (what the Linux program may load: renhdrum.SYSTEM_NEEDED, glibc's own libraries and GCC's unwinder, which every Linux
# system the game runs on has. X11, Wayland and xkbcommon are opened with dlopen by winit, never named here. Windows:
# Windows' own DLLs, renhdrum.SYSTEM_IMPORT's and those the windows (builder/crates/ui: winit, rfd's file chooser) import)
WINDOWS_UI_IMPORT = re.compile(r"^(gdi32|dwmapi|uxtheme|imm32|comctl32|comdlg32|shell32|ole32|oleaut32|shlwapi|"
                               r"propsys|uiautomationcore|winmm|dbghelp|version|opengl32|crypt32|secur32|psapi|combase|dwrite)\.dll$", re.I)
GLIBC_MAX = (2, 35)   # (Ubuntu 22.04's: the game's floor, tools.lock's sysroot)
NAMES = {"linux": "longshot-builder", "windows": "longshot-builder.exe"}
PROFILES = ("dist", "release")
PT_INTERP = 3
# llvm-mingw's Linux build, the same release as tools.lock's (its pin is builder/tools/pins.sh's LLVM_MINGW_VER and
# LLVM_MINGW_LINUX_SHA256, written there only): CI has no Windows runner, so the Windows longshot-builder, the Windows
# renderer and dav1d are compiled on Linux with it.


def linux_llvm_mingw():
    """(version, folder in the archive, SHA-256, URL) of llvm-mingw's Linux build, as builder/tools/pins.sh pins it"""
    with open(os.path.join(ROOT, "builder", "tools", "pins.sh"), encoding="utf-8") as f:
        pins = dict(re.findall(r"^(LLVM_MINGW_VER|LLVM_MINGW_LINUX_SHA256)=([0-9a-f]+)$", f.read(), re.M))
    if len(pins) != 2:
        fail("builder/tools/pins.sh pins no LLVM_MINGW_VER and LLVM_MINGW_LINUX_SHA256")
    version = pins["LLVM_MINGW_VER"]
    prefix = "llvm-mingw-%s-ucrt-ubuntu-22.04-x86_64" % version
    url = "https://github.com/mstorsjo/llvm-mingw/releases/download/%s/%s.tar.xz" % (version, prefix)
    return version, prefix, pins["LLVM_MINGW_LINUX_SHA256"], url


def fail(message):
    print("longshot_builder: " + message, file=sys.stderr)
    raise SystemExit(1)


def elf_interpreter(data):
    """Whether a 64-bit ELF asks for a program interpreter (a dynamic one: ld-linux, which a static program has none of)"""
    phoff = int.from_bytes(data[32:40], "little")
    phentsize, phnum = int.from_bytes(data[54:56], "little"), int.from_bytes(data[56:58], "little")
    return any(int.from_bytes(data[phoff + i * phentsize:phoff + i * phentsize + 4], "little") == PT_INTERP
               for i in range(phnum))


def problems(data, target, this_pc=False):
    """What makes the program unusable on a player's PC of target's system: [] when nothing does"""
    is_elf = data[:4] == b"\x7fELF"
    if target == "windows":
        if is_elf:
            return ["a Linux (ELF) program, not a Windows one (--target windows)"]
        machine, imports, _ = renhdrum.pe_info(data)
        out = [] if machine == 0x8664 else ["machine %#x, not x86-64" % machine]
        return out + ["needs %s, which is not part of Windows: a player would need it installed" % i
                      for i in imports if not (renhdrum.SYSTEM_IMPORT.match(i) or WINDOWS_UI_IMPORT.match(i))]
    if not is_elf:
        return ["not a Linux (ELF) program (--target linux)"]
    machine, needed, _ = renhdrum.elf_info(data)
    out = [] if machine == 62 else ["machine %#x, not x86-64" % machine]
    out += ["needs %s, which is not glibc's: a player would need it installed (X11 and Wayland are loaded at run time)" % n
            for n in needed if not renhdrum.SYSTEM_NEEDED.match(n)]
    if not elf_interpreter(data):
        out.append("asks for no program interpreter: not a program linked against glibc")
    versions = {tuple(int(x) for x in m.groups()) for m in re.finditer(rb"GLIBC_(\d+)\.(\d+)", data)}
    newer = sorted(v for v in versions if v > GLIBC_MAX)
    if newer and not this_pc:
        out.append("needs glibc %s, newer than %s (link with --sysroot: tools.lock's sysroot)"
                   % (".".join(map(str, newer[-1])), ".".join(map(str, GLIBC_MAX))))
    return out


# (the trailer after the pack a release's program carries: build-driver's sources.rs, TRAILER_MAGIC and TRAILER_LEN)
TRAILER_MAGIC = b"LSPKEND\n"
TRAILER_LEN = 64


def program_part(data):
    """A program's bytes without the pack appended to it, and the pack's size (0 when it carries none): its headers are
    checked, not the pack's compressed bytes"""
    if len(data) >= TRAILER_LEN and data[-TRAILER_LEN:].startswith(TRAILER_MAGIC):
        offset = int.from_bytes(data[-TRAILER_LEN + 16:-TRAILER_LEN + 24], "little")
        if offset <= len(data) - TRAILER_LEN:
            return data[:offset], len(data) - TRAILER_LEN - offset
    return data, 0


def cmd_check(a):
    if not os.path.isfile(a.file):
        fail("%s is not there (python -I scripts/longshot_builder.py build makes it)" % a.file)
    with open(a.file, "rb") as f:
        whole = f.read()
    data, pack = program_part(whole)
    if pack:
        print("%s: carries a pack of %d bytes (the program's own headers checked)" % (a.file, pack))
    target = a.target or ("windows" if a.file.lower().endswith(".exe") else "linux")
    print("%s: %d bytes, sha256 %s" % (a.file, len(whole), hashlib.sha256(whole).hexdigest()))
    bad = problems(data, target)
    for p in bad:
        print("  " + p)
    print("  usable" if not bad else "  not usable")
    return 1 if bad else 0


def cmd_llvm_mingw(a):
    version, name, sha256, url = linux_llvm_mingw()
    if a.name:
        print(name)
        return 0
    folder = os.path.join(a.folder, name)
    # (the archive, kept in <folder>/.cache, is checked by its size and SHA-256 each time; the unpacked folder is taken
    # only when its stamp names that SHA-256, written last: another pin, or an unpack cut short, unpacks it again. CI keeps
    # only .cache, never the unpacked folder, which nothing checks again)
    # (named <sha256>.tar.zst though it is a .tar.xz, as before: CI's kept caches still match)
    archive = os.path.join(a.folder, ".cache", sha256 + ".tar.zst")
    try:
        helpers.download([url], archive, sha256)
    except (OSError, ValueError) as e:
        fail("llvm-mingw: %s" % e)
    stamp = os.path.join(folder, ".archive-sha256")
    try:
        with open(stamp, encoding="utf-8") as f:
            fresh = f.read().strip() != sha256
    except OSError:
        fresh = True
    if fresh:
        shutil.rmtree(folder, ignore_errors=True)
        helpers.extract(archive, a.folder)
        with open(stamp, "w", encoding="utf-8") as f:
            f.write(sha256 + "\n")
    print(folder)
    return 0


LINK_TOOLS = ("llvm", "sysroot")


def lock_lines(names, system):
    """{name: (sha256, size, urls)} of tools.lock's lines for these tools on this system"""
    found = {}
    with open(os.path.join(ROOT, "tools.lock"), encoding="utf-8") as f:
        for line in f:
            w = line.split()
            if len(w) >= 9 and w[0] == "tool" and w[1] in names and w[3] == system:
                if w[7] != "-":
                    fail("tools.lock's %s line has a prefix (%s): link-tools takes a tool's own files only" % (w[1], w[7]))
                found[w[1]] = (w[4], int(w[5]), w[8:])
    missing = [n for n in names if n not in found]
    if missing:
        fail("tools.lock has no %s line for %s" % (", ".join(missing), system))
    return found


def cmd_link_tools(a):
    zstd = shutil.which("zstd")
    if not zstd:
        fail("link-tools needs the zstd program (the archives are .tar.zst)")
    for name, (sha256, size, urls) in sorted(lock_lines(LINK_TOOLS, "linux-x64").items()):
        folder = os.path.join(a.folder, name)
        # (as llvm-mingw: the archive kept in <folder>/.cache, checked by its size and SHA-256 each time; the unpacked
        # folder taken only when its stamp, written last, names that SHA-256. CI keeps only .cache)
        archive = os.path.join(a.folder, ".cache", sha256 + ".tar.zst")
        try:
            helpers.download(urls, archive, sha256, size=size, tries=3)
        except (OSError, ValueError) as e:
            fail("%s: %s" % (name, e))
        if os.path.getsize(archive) != size:
            fail("%s: %d bytes, not tools.lock's %d" % (name, os.path.getsize(archive), size))
        stamp = os.path.join(folder, ".archive-sha256")
        try:
            with open(stamp, encoding="utf-8") as f:
                fresh = f.read().strip() != sha256
        except OSError:
            fresh = True
        if fresh:
            shutil.rmtree(folder, ignore_errors=True)
            tar = os.path.join(a.folder, ".cache", sha256 + ".tar")
            if subprocess.run([zstd, "-d", "-q", "-f", archive, "-o", tar]).returncode:
                fail("%s: zstd could not unpack %s" % (name, archive))
            try:
                helpers.extract(tar, folder)
            finally:
                os.remove(tar)
            with open(stamp, "w", encoding="utf-8") as f:
                f.write(sha256 + "\n")
        print("%s=%s" % (name, folder))
    return 0


def cmd_build(a):
    target = a.target or renhdrum.host()
    triple = TARGETS[target]
    exe = ".exe" if os.name == "nt" else ""
    if target == "windows":
        mingw = a.llvm_mingw or os.environ.get("LONGSHOT_MINGW", "")
        linker = os.path.join(mingw, "bin", "x86_64-w64-mingw32-clang" + exe)
        if not mingw or not os.path.isfile(linker):
            fail("no llvm-mingw: pass --llvm-mingw <folder> or set LONGSHOT_MINGW (%s is not there)" % (linker or "its clang"))
    else:
        linker = shutil.which(a.linker or "cc")
        if not linker:
            fail("no %s to link with: pass --linker <cc>" % (a.linker or "cc"))
        sysroot = a.sysroot or os.environ.get("LONGSHOT_SYSROOT", "")
        if sysroot and not os.path.isdir(os.path.join(sysroot, "usr", "lib")):
            fail("--sysroot %s has no usr/lib: tools.lock's sysroot (`longshot-builder dev tools install`)" % sysroot)
    cache = os.path.abspath(a.cache or os.path.join(ROOT, "build", "longshot-builder-rust"))
    bin_dir = renhdrum.toolchain(cache, STD[target])
    out = os.path.abspath(a.out or os.path.join(ROOT, "build-windows" if target == "windows" else "build-linux",
                                                NAMES[target]))
    # (the crates, offline: a cargo directory source in a cargo home of its own, made again each time from the checked
    # downloads. The work folder, the target folder in it, is kept apart from the cache (--work, default beside it): a
    # CI cache holds downloads only, and nothing compiled is ever restored from one. Kept between builds on one PC, so a
    # second build compiles only what changed)
    work = os.path.abspath(a.work or cache + "-work")
    shutil.rmtree(os.path.join(work, "cargo-home"), ignore_errors=True)
    shutil.rmtree(os.path.join(work, "crates"), ignore_errors=True)
    home = os.path.join(work, "cargo-home")
    renhdrum.vendor_crates(cache, os.path.join(work, "crates"), lock=os.path.join(ROOT, "Cargo.lock"))
    os.makedirs(home)
    with open(os.path.join(home, "config.toml"), "w", encoding="utf-8") as f:
        f.write('[source.crates-io]\nreplace-with = "pinned"\n\n[source.pinned]\ndirectory = "%s"\n'
                % os.path.join(work, "crates").replace("\\", "/"))
    # (no path of this machine in it: the tree's and the crates' folders by a name of their own. Windows: crt-static
    # links llvm-mingw's unwinder in, the C runtime staying Windows' UCRT; no time stamp in the PE header. Linux: musl's
    # own start files and C library, linked in; no build id)
    flags = "--remap-path-prefix=%s=rust --remap-path-prefix=%s=crates" % (RUST_DIR, os.path.join(work, "crates"))
    if target == "windows":
        flags += " -C target-feature=+crt-static -C link-arg=-Wl,--no-insert-timestamp"
    else:
        flags += " -C link-arg=-Wl,--build-id=none"
        if sysroot:
            # (the game's link: clang with the sysroot's glibc 2.35 and GCC runtime, lld; the sysroot's path by a name of
            # its own in nothing but the link, which records none)
            flags += " -C link-arg=--sysroot=%s" % sysroot
            if "clang" in os.path.basename(linker):
                flags += " -C link-arg=-fuse-ld=lld"
    under = triple.replace("-", "_")
    env = dict(os.environ)
    env.update({
        "PATH": os.pathsep.join([bin_dir] + ([os.path.dirname(linker)] if target == "windows" else [])
                                + [env.get("PATH", "")]),
        "RUSTC": os.path.join(bin_dir, "rustc" + exe),
        "CARGO_HOME": home,
        "CARGO_TARGET_DIR": os.path.join(work, "target"),
        "CARGO_TARGET_%s_LINKER" % under.upper(): linker,
        "RUSTFLAGS": flags,
    })
    for k in ("RUSTUP_TOOLCHAIN", "CARGO_BUILD_TARGET", "CARGO_ENCODED_RUSTFLAGS", "CARGO_PROFILE_DIST_LTO",
              "LONGSHOT_SOURCES"):
        env.pop(k, None)
    cmd = [os.path.join(bin_dir, "cargo" + exe), "build", "--profile", a.profile, "--locked", "--offline",
           "-p", "longshot-builder", "--target", triple] + (["--jobs", str(a.jobs)] if a.jobs else [])
    print("longshot_builder: %s (Rust %s, linker %s)" % (" ".join(cmd[1:]), renhdrum.RUST_VERSION, linker), flush=True)
    if subprocess.run(cmd, cwd=ROOT, env=env).returncode:
        fail("cargo build failed")
    built = os.path.join(work, "target", triple, a.profile, NAMES[target])
    if not os.path.isfile(built):
        fail("cargo made no %s" % built)
    with open(built, "rb") as f:
        data = f.read()
    bad = problems(data, target, this_pc=a.this_pc)
    if bad:
        fail("what was built is not usable: " + "; ".join(bad))
    os.makedirs(os.path.dirname(out), exist_ok=True)
    shutil.copyfile(built, out + ".new")
    os.chmod(out + ".new", 0o755)
    os.replace(out + ".new", out)
    print("longshot_builder: %s, %d bytes, sha256 %s" % (out, len(data), hashlib.sha256(data).hexdigest()))
    return 0


# (the game's Rust, longshot-rs: a staticlib for the game's own target, not the builder's static one)
GAME_RUST_TARGETS = {"linux": "x86_64-unknown-linux-gnu", "windows": "x86_64-pc-windows-gnullvm"}
GAME_RUST_STD = {"linux": "linux", "windows": "windows"}   # (renhdrum.RUST_STD's keys)


def cmd_game_rust(a):
    """liblongshot_rs.a, the game's Rust (longshot-rs over game/crates), for target, with the pinned Rust and the
    crates Cargo.lock pins, offline: the library a player's build links instead of compiling Rust (build-compile's
    rust.rs). The release writes the sources' key beside it (longshot-builder dev rust-key) and carries both at
    longshot-rs/prebuilt/<platform>/."""
    target = a.target or renhdrum.host()
    triple = GAME_RUST_TARGETS[target]
    cache = os.path.abspath(a.cache or os.path.join(ROOT, "build", "longshot-builder-rust"))
    bin_dir = renhdrum.toolchain(cache, GAME_RUST_STD[target])
    work = os.path.abspath(a.work or cache + "-game-work")
    shutil.rmtree(os.path.join(work, "cargo-home"), ignore_errors=True)
    shutil.rmtree(os.path.join(work, "crates"), ignore_errors=True)
    home = os.path.join(work, "cargo-home")
    renhdrum.vendor_crates(cache, os.path.join(work, "crates"), lock=os.path.join(ROOT, "Cargo.lock"))
    os.makedirs(home)
    with open(os.path.join(home, "config.toml"), "w", encoding="utf-8") as f:
        f.write('[source.crates-io]\nreplace-with = "pinned"\n\n[source.pinned]\ndirectory = "%s"\n'
                % os.path.join(work, "crates").replace("\\", "/"))
    exe = ".exe" if os.name == "nt" else ""
    env = dict(os.environ)
    env.update({
        "PATH": os.pathsep.join([bin_dir, env.get("PATH", "")]),
        "RUSTC": os.path.join(bin_dir, "rustc" + exe),
        "CARGO_HOME": home,
        "CARGO_TARGET_DIR": os.path.join(work, "target"),
        # (no path of this machine in it: the tree's folder by a name of its own. Encoded, one flag: RUSTFLAGS would
        # split a tree path with a space in it)
        "CARGO_ENCODED_RUSTFLAGS": "--remap-path-prefix=%s=longshot" % ROOT,
    })
    for k in ("RUSTUP_TOOLCHAIN", "CARGO_BUILD_TARGET", "RUSTFLAGS"):
        env.pop(k, None)
    cmd = [os.path.join(bin_dir, "cargo" + exe), "build", "--profile", "game", "--locked", "--offline",
           "-p", "longshot-rs", "--target", triple] + (["--jobs", str(a.jobs)] if a.jobs else [])
    print("longshot_builder: %s (Rust %s)" % (" ".join(cmd[1:]), renhdrum.RUST_VERSION), flush=True)
    if subprocess.run(cmd, cwd=ROOT, env=env).returncode:
        fail("cargo build -p longshot-rs failed")
    built = os.path.join(work, "target", triple, "game", "liblongshot_rs.a")
    if not os.path.isfile(built):
        fail("cargo made no %s" % built)
    os.makedirs(a.out, exist_ok=True)
    out = os.path.join(a.out, "liblongshot_rs.a")
    shutil.copyfile(built, out + ".new")
    os.replace(out + ".new", out)
    with open(out, "rb") as f:
        data = f.read()
    print("longshot_builder: %s, %d bytes, sha256 %s" % (out, len(data), hashlib.sha256(data).hexdigest()))
    return 0


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    s = p.add_subparsers(dest="what", required=True)
    b = s.add_parser("build", help="build longshot-builder from builder/ with the pinned Rust")
    b.add_argument("--target", choices=sorted(TARGETS), help="linux or windows (default: this system's)")
    b.add_argument("--profile", choices=PROFILES, default="dist", help="Cargo.toml's profile (default: dist)")
    b.add_argument("--llvm-mingw", help="windows: llvm-mingw's folder (default: LONGSHOT_MINGW)")
    b.add_argument("--linker", help="linux: the C compiler that links it (default: cc)")
    b.add_argument("--sysroot", help="linux: tools.lock's sysroot to link against (glibc 2.35; default LONGSHOT_SYSROOT, "
                                     "else the system's own glibc)")
    b.add_argument("--cache", help="where the pinned Rust's and the crates' downloads are kept (default: "
                                   "build/longshot-builder-rust)")
    b.add_argument("--work", help="where the build is made (default: <cache>-work, never inside the cache)")
    b.add_argument("--out", help="its path (default: build-linux/longshot-builder or build-windows/longshot-builder.exe)")
    b.add_argument("--jobs", type=int, help="cargo's jobs")
    b.add_argument("--this-pc", action="store_true",
                   help="linux: a builder for this machine only (a runner's own), never shipped: the glibc floor does not "
                        "apply, every other check does")
    b.set_defaults(fn=cmd_build)
    m = s.add_parser("llvm-mingw", help="llvm-mingw unpacked, its folder printed")
    m.add_argument("folder", nargs="?", default="")
    m.add_argument("--name", action="store_true")
    m.add_argument("--host", choices=("linux",), default="linux")
    m.set_defaults(fn=cmd_llvm_mingw)
    t = s.add_parser("link-tools", help="tools.lock's linux-x64 llvm and sysroot unpacked, each printed as <name>=<folder>")
    t.add_argument("folder")
    t.set_defaults(fn=cmd_link_tools)
    g = s.add_parser("game-rust", help="liblongshot_rs.a, the game's Rust, with the pinned Rust (for the release)")
    g.add_argument("--target", choices=sorted(GAME_RUST_TARGETS), help="linux or windows (default: this system's)")
    g.add_argument("--cache", help="the pinned Rust's and the crates' downloads (default: build/longshot-builder-rust)")
    g.add_argument("--work", help="where it is built (default: <cache>-game-work)")
    g.add_argument("--out", required=True, help="the folder liblongshot_rs.a is put in")
    g.add_argument("--jobs", type=int, help="cargo's jobs")
    g.set_defaults(fn=cmd_game_rust)
    c = s.add_parser("check", help="check a longshot-builder program's headers: x86-64, and no library a player lacks")
    c.add_argument("file")
    c.add_argument("--target", choices=sorted(TARGETS), help="its system (default: by its name)")
    c.set_defaults(fn=cmd_check)
    a = p.parse_args(argv)
    return a.fn(a)


if __name__ == "__main__":
    sys.exit(main())
