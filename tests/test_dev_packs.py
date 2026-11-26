"""`dev.py packs` (scripts/devtools/packs.py): how a change is built, which modules and files a pack keeps, and a runtime from a base and packs.

python -I -B tests/test_dev_packs.py
"""
import contextlib
import importlib.util
import io
import json
import os
import re
import shutil
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


sys.path.insert(0, str(ROOT / "scripts"))
from devtools import common, packs  # noqa: E402
dev = load("dev_under_test", ROOT / "scripts" / "dev.py")   # (its devtools are the ones above: one module each)
PACKS_SOURCE = ROOT / "scripts" / "devtools" / "packs.py"


def packs_main(argv):
    """`dev.py packs ARGV`"""
    return dev.main(["packs"] + list(argv))


def check(what, ok):
    if not ok:
        raise SystemExit("FAIL: " + what)


def write(p, data=b"x"):
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_bytes(data)


def test_route():
    r = packs.route
    for f in ("gpu/vk/vk_replay.cpp", "gpu/vk/eden/shader_recompiler/ir_opt/x.cpp", "third_party/imgui/imgui.cpp",
              "host/hos_thread.c", "gpu/render/x.h", "builder/crates/lift/src/lib.rs", "Cargo.lock", "Cargo.toml", "longshot-rs/src/lib.rs", "builder/rt/a64.h",
              "assets/profiles/guest.profile", "assets/pipeline_states/tour.bin.xz", "assets/prompt_faces/x.png",
              "assets/longshot.ico", "assets/addresses.ini", "VERSION"):
        check("%s (read by longshot-builder) builds the game" % f, r([f]) == "game")
    check("docs and tests build nothing", r(["docs/dev/setup.md", "tests/test_a.py", "README.md", "builder/README.md"]) is None)
    check("the forge's workflows build nothing", r([".forgejo/workflows/ci.yml", ".forgejo/pull_request_template.md"]) is None)
    check("developer tools and CI's settings build nothing",
          r(["scripts/perf.py", "scripts/profiling/cpu.py", "scripts/dev.py", "scripts/devtools/packs.py", "mise.toml",
             "tools.lock", "scripts/longshot_builder.py",
             "assets/routes/a.route", "assets/saves/x/save.bin"]) is None)
    check("a renderer file and a host file build the game", r(["gpu/vk/vk_draw.cpp", "host/nvn_native.c"]) == "game")


def test_built_covers_what_rust_reads():
    """Every path of the tree that longshot-builder's sources name (builder/crates' root.join("...") and "<dir>/..." strings)
    is one packs.BUILT covers: a change there builds the game"""
    named = set()
    for f in (ROOT / "builder" / "crates").rglob("*.rs"):
        if "/tests/" in f.as_posix() or f.name == "tests.rs" or f.name.endswith("_tests.rs"):   # (a module's tests' file)
            continue
        text = f.read_text(encoding="utf-8", errors="replace")
        named |= set(re.findall(r'root\.join\("([^"]+)"\)', text))
    named = {n for n in named if n.split("/")[0] in ("host", "game", "gpu", "third_party", "assets")}
    check("builder/ names tree paths (%s)" % sorted(named)[:3], named)
    missed = sorted(n for n in named if not n.startswith(packs.BUILT) and not packs.built_paths([n + "/x.c"]))
    check("every tree path builder/ reads is BUILT (%s)" % missed, not missed)


def test_clone(d):
    """A runtime's copies (packs.clone): the same bytes, times and mode as the base's file, and a write to the copy leaves the
    base's file as it was (a copy-on-write clone where the file system has them, else a copy)"""
    src, dst = d / "game.bin", d / "copy.bin"
    src.write_bytes(b"base" * 4096)
    os.chmod(src, 0o755)
    os.utime(src, (1_000_000_000, 1_000_000_000))
    check("clone gives back the copy's path", packs.clone(str(src), str(dst)) == str(dst))
    check("the same bytes", dst.read_bytes() == src.read_bytes())
    check("the same time", int(dst.stat().st_mtime) == 1_000_000_000)
    check("the same mode", os.name == "nt" or (dst.stat().st_mode & 0o777) == 0o755)
    with open(dst, "r+b") as f:
        f.write(b"play")
    check("a write to the copy leaves the base's file", src.read_bytes() == b"base" * 4096)


def test_stamp_matches_build(d):
    """A runtime prepares again when its build's shader stamp moved: the translator_stamp longshot-builder writes in the
    game folder's longshot_version.txt (build-driver's build.rs, of build-compile's generated/shader_stamp.rs), compared
    with the base's: the builder's own key, not a copy of its list of files"""
    src = (ROOT / "builder" / "crates" / "build-driver" / "src" / "build.rs").read_text(encoding="utf-8")
    check("the build writes translator_stamp= into longshot_version.txt",
          'translator_stamp={translator}' in src and '"longshot_version.txt"' in src)
    base, same, moved, none = d / "stamp-base", d / "stamp-same", d / "stamp-moved", d / "stamp-none"
    for folder, stamp in ((base, "aa"), (same, "aa"), (moved, "bb")):
        write(folder / "longshot_version.txt", b"version=x\ncompiler_stamp=cc\ntranslator_stamp=" + stamp.encode() + b"\n")
    none.mkdir()
    check("the stamp read", packs.translator_stamp(str(base)) == "aa")
    check("the same stamp: no prepare", not packs.translator_changed(str(base), str(same)))
    check("another stamp: the runtime prepares again", packs.translator_changed(str(base), str(moved)))
    check("a folder without one: prepares again", packs.translator_changed(str(base), str(none)))


def test_setting_flags():
    """dev.py's LONGSHOT_NGX_SDK goes to longshot-builder as --ngx-sdk (its flag; the variable is the builder's for one
    more release), not when the command line has one already"""
    check("--ngx-sdk from the environment", packs.setting_flags({"LONGSHOT_NGX_SDK": "/sdk"}) == ["--ngx-sdk", "/sdk"])
    check("none without it", packs.setting_flags({}) == [] and packs.setting_flags({"LONGSHOT_NGX_SDK": " "}) == [])
    check("the command line's wins", packs.setting_flags({"LONGSHOT_NGX_SDK": "/sdk"}, ["build", "--ngx-sdk", "/x"]) == [])


def test_cache_format(d):
    """A new kProgramCacheFormat drops every cache the base prepared (program_cache_stamp holds it), so a runtime prepares
    again, though the translator's stamp is the same: found in the renderer's source as the game reads it"""
    src = (ROOT / packs.PROGRAM_CACHE).read_text(encoding="utf-8")
    check("kProgramCacheFormat found in %s" % packs.PROGRAM_CACHE, packs.CACHE_FORMAT.search(src))
    check("program_cache_stamp holds kProgramCacheFormat", "program_cache_stamp_of(kProgramCacheFormat)" in src)
    import subprocess
    repo = d / "fmt"
    path = repo / "gpu" / "vk" / "vk_progcache.cpp"
    def run(*args):
        subprocess.run(["git", "-C", str(repo)] + list(args), check=True, capture_output=True)
    write(path, b"// a\nconstexpr uint32_t kProgramCacheFormat = 1;\n")
    run("init", "-q")
    run("add", "-A")
    run("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "base")
    base = subprocess.run(["git", "-C", str(repo), "rev-parse", "HEAD"], check=True, capture_output=True, text=True).stdout.strip()
    changed = [packs.PROGRAM_CACHE]
    write(path, b"// b\nconstexpr uint32_t kProgramCacheFormat = 1;\n")
    check("an edit that keeps the format does not prepare", not packs.cache_format_changed(str(repo), base, changed))
    write(path, b"// b\nconstexpr uint32_t kProgramCacheFormat = 2;\n")
    check("a new format prepares", packs.cache_format_changed(str(repo), base, changed))
    check("the file not changed: no prepare", not packs.cache_format_changed(str(repo), base, ["gpu/vk/vk_draw.cpp"]))


def test_stamped_objects_match_build():
    """The objects named in MODULES as holding the version are the files build-compile gives it to"""
    src = (ROOT / "builder" / "crates" / "build-compile" / "src" / "plan.rs").read_text(encoding="utf-8")
    check("plan.rs gives version.c and vk_version.cpp the version",
          'if name == "version.c"' in src and '"vk_version.cpp" => f.push(i.version.clone())' in src)
    check("they are packs.STAMPED_SOURCES", sorted(os.path.basename(s) for s in packs.STAMPED_SOURCES) ==
          ["version.c", "vk_version.cpp"])
    lib = (ROOT / "builder" / "crates" / "build-compile" / "src" / "lib.rs").read_text(encoding="utf-8")
    for _, _, rsp, _ in packs.MODULES:
        check("build-compile writes %s" % rsp, 'l.out.join("%s")' % rsp in lib)


def test_caches_named_in_renderer():
    text = "".join(p.read_text(encoding="utf-8", errors="replace") for g in ("*.cpp", "*.h") for p in (ROOT / "gpu" / "vk").glob(g))
    for c in packs.CACHES + (packs.SEED,):
        # (some are a named file with a suffix the renderer adds: .predicted and .built to the notes, .idx for .bin)
        name = c.replace(".predicted_keys", "").replace(".predicted", "").replace(".built", "").replace(".zst.idx", ".zst.bin")
        check("the renderer still names %s" % name, name in text)
    check("the predicted and built notes' suffixes, and the index's", all(s in text for s in ('".predicted"', '".predicted_keys"', '".built"', '".idx"')))
    # (and the other way round: every .bin or .idx the renderer names beside the game is a cache, or one of the two files a
    # build makes (the shader seed, the shipped state list); a new cache file in the renderer fails here until it is classified)
    named = set(re.findall(r'"(longshot_[A-Za-z0-9_.]*\.(?:bin|idx))"', text))
    unknown = sorted(n for n in named if not packs.is_cache(n) and n not in (packs.SEED, "longshot_pipeline_states_shipped.bin"))
    check("every file the renderer keeps beside the game is classified (unknown: %s)" % unknown, not unknown)


def test_module_changed(d):
    base, mine = d / "base-build", d / "pack-build"
    for build in (base, mine):
        write(build / "obj" / "a.o", b"same a")
        write(build / "obj" / "host_version.o", b"version " + build.name.encode())
        (build / "objs.rsp").write_text("".join('"%s"\n' % (build / "obj" / n) for n in ("a.o", "host_version.o")), encoding="utf-8")
    mc = lambda changed_source=False: packs.module_changed(str(base), str(mine), "objs.rsp", "host_version.o", changed_source)
    check("only the version differs: the base's module", not mc())
    check("the version's own source changed: the pack's", mc(True))
    write(mine / "obj" / "a.o", b"other a")
    check("another object differs: the pack's", mc())
    write(mine / "obj" / "a.o", b"same a")
    write(mine / "obj" / "b.o", b"new")
    (mine / "objs.rsp").write_text("".join('"%s"\n' % (mine / "obj" / n) for n in ("a.o", "b.o", "host_version.o")), encoding="utf-8")
    check("another set of objects: the pack's", mc())
    check("no link list in the base: the pack's", packs.module_changed(str(d / "none"), str(mine), "objs.rsp", "host_version.o", False))


def test_inputs_beside_the_objects():
    lic = packs.link_inputs_changed
    check("a host source shows in the objects", not lic(["host/hos_thread.c", "docs/x.md"]))
    check("longshot-builder's own code (flags, links): both modules", lic(["builder/crates/build-compile/src/link.rs"]))
    check("a library: both modules", lic(["third_party/sdl2/SDL2.lib"]))
    check("a shader source: both modules", lic(["gpu/vk/shaders/blit.comp"]))
    check("rust's documents do not", not lic(["builder/README.md"]))
    for src in packs.STAMPED_SOURCES:
        check("%s includes nothing, so no other file reaches its object" % src,
              "#include" not in (ROOT / src).read_text(encoding="utf-8"))


def refused(what, argv, reason):
    """Refused for that reason, not another check's. argv: a `dev.py packs` command line, or a function to call"""
    try:
        argv() if callable(argv) else packs_main(argv)
    except SystemExit as e:
        check(what + " refused because %s (%r)" % (reason, e.code), isinstance(e.code, str) and reason in e.code)
    else:
        check(what + " refused", False)


def test_guards(d):
    base = d / "base"
    write(base / "game" / "user" / "save0.bin", b"played")
    refused("a base built into a played game folder", ["base", str(base), "--game", "g.nup", "--tree", str(ROOT)],
            "new game folder")
    f = packs.pack_folders(str(base))
    refused("a base whose work folder is its packs' own",
            lambda: packs.check_folders(str(base), {"work": f["game_work"], "game": str(base / "game")}), "packs are built")
    write(base / "game" / packs.GAME, b"g")
    write(base / "game" / packs.RENDERER, b"r")
    packs.write_base(str(base), {"commit": "c" * 40, "work": str(d / "w"), "game": str(base / "game"), "game_source": "g.nup"})
    refused("a base whose game folder is inside its packs' output folder",
            lambda: packs.check_folders(str(d / "b2"), {"work": str(d / "w"),
                                                        "game": os.path.join(packs.pack_folders(str(d / "b2"))["game_out"], "g")}),
            "where its packs are built")
    check("folders on two drives do not overlap (no error)", not packs.overlaps("C:\\a", "D:\\a") if os.name == "nt" else True)
    link = d / "alias"
    target = packs.pack_folders(str(d / "b3"))["game_out"]
    os.makedirs(target)
    try:
        os.symlink(target, link, target_is_directory=True)
    except OSError:   # (Windows without the right to make links: a junction, which needs none)
        import _winapi
        _winapi.CreateJunction(target, str(link))
    refused("a base whose game folder is its packs' output folder through a link",
            lambda: packs.check_folders(str(d / "b3"), {"work": str(d / "w"), "game": str(link)}), "where its packs are built")
    refused("a runtime inside the base's game folder", ["rt", str(base), str(base / "game" / "runtime")], "folder of its own")
    check("...and nothing written there", not (base / "game" / "runtime").exists())
    for name in ("../game", "..", "a/b", str(d)):
        refused("the pack name %r" % name, ["pack", str(base), name, "--tree", str(ROOT)], "pack's name")
    check("the base is still there", (base / "game" / "user" / "save0.bin").exists())


def test_pack_folders(d):
    """Every pack of a base builds in the same two folders (longshot-builder keeps the lift there by its key; the objects
    come from the stores), never inside packs/"""
    f = packs.pack_folders(str(d))
    check("the same folders for every pack", f == packs.pack_folders(str(d)) and
          not f["game_work"].startswith(os.path.join(str(d), "packs")))
    check("a work folder and a game folder", sorted(f) == ["game_out", "game_work"])


def test_generated_and_runtime(d):
    game_mod, vk_mod = packs.GAME, packs.RENDERER
    base_dir, game, out = d / "base", d / "base" / "game", d / "out"
    write(game / "romfs" / "a" / "data.bin", b"romfs")
    write(game / "romfs" / "prompts" / "icon.bntx", b"base icon")
    write(game / "texts" / "a.txt", b"base text")
    write(game / game_mod, b"base game")
    write(game / vk_mod, b"base renderer")
    write(game / "cache" / "longshot_programs.bin", b"cache")
    write(game / "cache" / "prompts" / "Common.bntx.zs", b"made at run time")   # (the game's, never a build's output)
    write(game / "logs" / "longshot_log.txt", b"a log")
    write(game / "language.txt", b"en-US")
    write(game / "prompts" / "Common.bntx", b"base prompts")
    write(game / "longshot_pipeline_states_shipped.bin", b"shipped")
    write(game / "longshot_options.ini", b"[menu]")
    write(game / "cache" / "longshot_vk_pipelines_play_2.bin", b"seeded play cache")   # (what --seed brings, before the build records outputs)
    write(game / "cache" / "longshot_vk_pipelines.stamp", b"the pipeline cache's stamp")
    write(game / "longshot_pipelines.bin.predicting", b"a prediction a crash left unfinished")
    if packs.MODULES[0][1]:   # (the game's map: Windows only)
        write(game / packs.MODULES[0][1], b"map")
    outputs = packs.build_outputs(str(game))
    check("a play cache and the pipeline stamp are not build outputs (%s)" % outputs,
          "cache/longshot_vk_pipelines_play_2.bin" not in outputs and "cache/longshot_vk_pipelines.stamp" not in outputs
          and "longshot_pipelines.bin.predicting" not in outputs)
    write(game / "user" / "save0.bin", b"a save, made in play")
    write(game / "longshot_env.txt", b"HOS_X=1")
    os.makedirs(out / "romfs" / "a")
    os.link(game / "romfs" / "a" / "data.bin", out / "romfs" / "a" / "data.bin")
    write(out / "romfs" / "prompts" / "icon.bntx", b"new icon")
    write(out / "texts" / "a.txt", b"new text")
    write(out / ".longshot_installed.json", b"another folder's note")
    write(out / game_mod, b"built game")
    write(out / "cache" / "longshot_programs.bin", b"other cache")
    write(out / "cache" / "longshot_vk_pipelines_play_5.bin", b"a play cache the pack's build has")
    write(out / "language.txt", b"en-US")
    gen, gone = packs.generated_changes(str(game), str(out), outputs)
    check("a regenerated file is kept; the same ones, the modules, the caches, the builder's notes and the romfs are not: %s"
          % gen, gen == ["texts/a.txt"])
    check("generated files the build no longer makes are gone; saves, settings and the game's own files are not: %s" % gone,
          gone == ["longshot_pipeline_states_shipped.bin", "prompts/Common.bntx"])
    check("a base that recorded no outputs loses nothing", packs.generated_changes(str(game), str(out))[1] == [])
    # (a list recorded before caches and the preparation's recovery markers were filtered: a crash mid-prediction leaves one)
    legacy = outputs + ["cache/longshot_vk_pipelines.stamp", "cache/longshot_vk_pipelines_play_2.bin",
                        "longshot_pipelines.bin.predicting", "longshot_pipelines.bin.running", "longshot_pipelines.bin.built.warming"]
    gone = packs.generated_changes(str(game), str(out), legacy)[1]
    check("an older base's outputs list never deletes a cache or a recovery marker, and still a generated file the build no "
          "longer makes: %s" % gone,
          gone == ["longshot_pipeline_states_shipped.bin", "prompts/Common.bntx"])
    write(out / "stale.bin", b"an earlier pack's")
    write(out / "image.bin", b"image")
    write(out / packs.SEED, b"shader list")
    write(out / ".longshot_installed.json", b"{}")
    write(out / "longshot_build.json", b"{}")
    packs.clear_output(str(out), packs.PACK_KEEPS)
    check("the output folder keeps romfs and what longshot-builder checks itself (%s)" % sorted(os.listdir(out)),
          sorted(os.listdir(out)) == sorted(["romfs", packs.GAME, "image.bin", "longshot_build.json",
                                             ".longshot_installed.json"]))
    packs.clear_output(str(out))
    check("the output folder keeps only romfs for the next build", sorted(os.listdir(out)) == ["romfs"])
    check("...its links intact", os.path.samefile(out / "romfs" / "a" / "data.bin", game / "romfs" / "a" / "data.bin"))
    b = {"commit": "c" * 40, "work": str(base_dir / "work"), "game": str(game), "game_source": "g.nup"}
    packs.write_base(str(base_dir), b)
    for name, files, generated, on in (("cpu", {game_mod: "h"}, ["romfs/prompts/icon.bntx"], b["commit"]),
                                       ("cpu2", {game_mod: "h"}, [], b["commit"]), ("old", {vk_mod: "h"}, [], "o" * 40)):
        p = base_dir / "packs" / name
        for m in files:
            write(p / m, b"pack " + name.encode())
        for rel in generated:
            write(p / "files" / rel, b"new icon")
        (p / "pack.json").write_text(json.dumps({"base": on, "files": files, "generated": generated, "prepare": False,
                                                 "deleted": ["prompts/Common.bntx"] if name == "cpu" else []}))
    rt = d / "rt"
    packs_main(["rt", str(base_dir), str(rt), "cpu"])
    check("the pack's game", (rt / game_mod).read_bytes() == b"pack cpu")
    check("the base's renderer", (rt / vk_mod).read_bytes() == b"base renderer")
    check("romfs hard-linked", os.path.samefile(rt / "romfs" / "a" / "data.bin", game / "romfs" / "a" / "data.bin"))
    check("the caches copied, not linked (the game writes them)", not os.path.samefile(rt / "cache" / "longshot_programs.bin", game / "cache" / "longshot_programs.bin"))
    check("the pack's generated file", (rt / "romfs" / "prompts" / "icon.bntx").read_bytes() == b"new icon")
    check("...not written through the base's hard link", (game / "romfs" / "prompts" / "icon.bntx").read_bytes() == b"base icon")
    check("the pack's deleted file is not in the runtime", not (rt / "prompts" / "Common.bntx").exists())
    check("...and still in the base", (game / "prompts" / "Common.bntx").exists())
    for asked, what, why in ((["cpu", "cpu2"], "two packs with the same module", "both bring"),
                             (["old"], "a pack on another base", "not the base")):
        refused(what, ["rt", str(base_dir), str(d / ("rt-" + asked[-1])), *asked], why)
    victim = d / "outside.txt"
    write(victim, b"not the runtime's")
    for name, meta, why in (("abs", {"generated": [str(victim)]}, "outside the runtime"),
                            ("up", {"deleted": ["../outside.txt"]}, "outside the runtime"),
                            ("deep", {"generated": ["romfs/../../outside.txt"]}, "outside the runtime"),
                            ("mod", {"files": {"../outside.txt": "h"}}, "names modules")):
        p = base_dir / "packs" / name
        (p / "files").mkdir(parents=True, exist_ok=True)
        (p / "pack.json").write_text(json.dumps(dict({"base": b["commit"], "files": {}, "generated": [], "deleted": [],
                                                      "prepare": False}, **meta)))
        refused("a pack.json naming %s" % meta, ["rt", str(base_dir), str(d / ("rt-" + name)), name], why)
        check("...and nothing made", not (d / ("rt-" + name)).exists())
    check("the file outside is untouched", victim.read_bytes() == b"not the runtime's")
    refused("a pack.json naming ..\\outside.txt (a backslash path)",
            ["rt", str(base_dir), str(d / "rt-bs"), *_pack(base_dir, b, "bs", {"deleted": ["..\\outside.txt"]})], "outside the runtime")
    # A link in the base's romfs is cloned into the runtime. A relative one that names a file from the base names a folder
    # beside the runtime once cloned (../../outside from base/game/romfs is base/outside, from rt-link/romfs it is outside/),
    # so a deleted path through it reads as inside and is not. The runtime is made, then refused at that file.
    write(base_dir / "outside", b"a file beside the base's game folder")
    write(d / "outside" / "victim.txt", b"beside the runtime")
    try:
        os.symlink(os.path.join("..", "..", "outside"), game / "romfs" / "escape")
    except OSError:   # (no right to make a file link here, as on Windows without developer mode: nothing to check)
        print("  skipped: a file link in the base's romfs (this account cannot make one)")
    else:
        try:
            packs_main(["rt", str(base_dir), str(d / "rt-link"), _pack(base_dir, b, "link", {"deleted": ["romfs/escape/victim.txt"]})[0]])
        except SystemExit as e:
            if os.path.islink(d / "rt-link" / "romfs" / "escape"):
                check("a deleted path through a cloned link refused (%r)" % e.code, isinstance(e.code, str) and "through a link" in e.code)
        check("the file beside the runtime is untouched", (d / "outside" / "victim.txt").read_bytes() == b"beside the runtime")
        os.remove(game / "romfs" / "escape")


def test_rt_refresh(d):
    """rt over an existing runtime refreshes it in place: files that differ are replaced, files no longer wanted removed, and
    its caches, logs, saves and settings kept; a folder that is not a runtime is refused, one of an older layout made again"""
    base_dir, game = fake_base(d)
    write(game / "texts" / "a.txt", b"base text")
    b = packs.read_base(str(base_dir))
    rt = d / "rt"
    _pack(base_dir, b, "p", {"generated": ["romfs/new.bin", "texts/a.txt"], "files": {packs.GAME: "h"}})
    write(base_dir / "packs" / "p" / packs.GAME, b"pack game, longer")
    write(base_dir / "packs" / "p" / "files" / "romfs" / "new.bin", b"generated")
    write(base_dir / "packs" / "p" / "files" / "texts" / "a.txt", b"pack text, longer")
    packs_main(["rt", str(base_dir), str(rt), "p"])
    check("the pack's game", (rt / packs.GAME).read_bytes() == b"pack game, longer")
    renderer_ino = (rt / packs.RENDERER).stat().st_ino
    write(rt / "cache" / "longshot_programs.bin", b"grown in play")
    write(rt / "logs" / "longshot_log.txt", b"a log")
    write(rt / "user" / "save0.bin", b"a save")
    write(rt / "longshot_options.ini", b"[menu]")
    packs_main(["rt", str(base_dir), str(rt)])   # (the same runtime, now with no pack)
    check("the base's game again", (rt / packs.GAME).read_bytes() == b"base game")
    check("the base's text again", (rt / "texts" / "a.txt").read_bytes() == b"base text")
    check("the pack's generated file is gone", not (rt / "romfs" / "new.bin").exists())
    check("romfs still hard-linked", os.path.samefile(rt / "romfs" / "a" / "data.bin", game / "romfs" / "a" / "data.bin"))
    check("an unchanged module is not copied again", (rt / packs.RENDERER).stat().st_ino == renderer_ino)
    check("the caches play grew are kept", (rt / "cache" / "longshot_programs.bin").read_bytes() == b"grown in play")
    check("the logs, saves and settings are kept", (rt / "logs" / "longshot_log.txt").exists() and
          (rt / "user" / "save0.bin").exists() and (rt / "longshot_options.ini").read_bytes() == b"[menu]")
    made = json.loads((rt / "dev_packs.json").read_text())
    check("dev_packs.json names the runtime's files and layout", made.get("layout") == packs.RT_LAYOUT and
          packs.GAME in made["files"] and "romfs/new.bin" not in made["files"] and made["packs"] == [])
    with open(rt / packs.GAME, "r+b") as f:   # (a module rewritten by hand: another time, replaced)
        f.write(b"HAND")
    os.utime(rt / packs.GAME, (1, 1))
    packs_main(["rt", str(base_dir), str(rt)])
    check("a module that differs is replaced", (rt / packs.GAME).read_bytes() == b"base game")
    check("...and the base's is untouched", (game / packs.GAME).read_bytes() == b"base game")
    # (a runtime from before the layout: no files list. Everything but its play state is made again)
    made.pop("layout")
    made.pop("files")
    (rt / "dev_packs.json").write_text(json.dumps(made))
    write(rt / "stale-from-an-old-pack.bin", b"old")
    packs_main(["rt", str(base_dir), str(rt)])
    check("an older runtime's unknown file goes", not (rt / "stale-from-an-old-pack.bin").exists())
    check("...its caches and saves stay", (rt / "cache" / "longshot_programs.bin").read_bytes() == b"grown in play" and
          (rt / "user" / "save0.bin").exists())
    # (the base moved, its translator the same: the runtime's warm caches stay)
    made = json.loads((rt / "dev_packs.json").read_text())
    made["base"] = "0" * 40
    (rt / "dev_packs.json").write_text(json.dumps(made))
    write(game / "cache" / "longshot_programs.idx", b"the base's")
    packs_main(["rt", str(base_dir), str(rt)])
    grown = rt / "cache" / "longshot_programs.bin"
    check("a moved base with the same translator keeps the caches", grown.read_bytes() == b"grown in play")
    # (the translator changed: what the runtime's game made for the old one is dropped, the base's caches taken instead)
    write(game / "longshot_version.txt", b"release=x\ntranslator_stamp=another\n")
    packs_main(["rt", str(base_dir), str(rt)])
    check("a new translator's caches are dropped", not grown.exists() or grown.read_bytes() != b"grown in play")
    check("...the base's taken", (rt / "cache" / "longshot_programs.idx").read_bytes() == b"the base's")
    check("...the saves stay", (rt / "user" / "save0.bin").exists())
    write(d / "not-a-runtime" / "precious.txt", b"mine")
    refused("a folder that is not a runtime", ["rt", str(base_dir), str(d / "not-a-runtime")], "not a runtime")
    check("...untouched", (d / "not-a-runtime" / "precious.txt").read_bytes() == b"mine")


def test_resolves_inside(d):
    """The check made where each file is written: a folder link inside the runtime that leads out is outside"""
    rt, out = d / "rt", d / "out"
    (rt / "romfs" / "real").mkdir(parents=True)
    out.mkdir()
    try:
        os.symlink(out, rt / "romfs" / "door", target_is_directory=True)
    except OSError:   # (Windows without the right to make links: a junction, which needs none)
        import _winapi
        _winapi.CreateJunction(str(out), str(rt / "romfs" / "door"))
    check("a real folder in the runtime is inside", packs.resolves_inside(str(rt), str(rt / "romfs" / "real")))
    check("a folder not made yet below the runtime is inside", packs.resolves_inside(str(rt), str(rt / "romfs" / "new" / "deeper")))
    check("a folder link in the runtime that leads out is outside", not packs.resolves_inside(str(rt), str(rt / "romfs" / "door")))
    check("...and a path through it", not packs.resolves_inside(str(rt), str(rt / "romfs" / "door" / "x")))
    check("the folder beside the runtime is outside", not packs.resolves_inside(str(rt), str(out)))
    upper = str(rt).upper() + os.sep + "ROMFS"
    if os.path.exists(upper) and os.path.samefile(upper, rt / "romfs"):   # (a case-insensitive folder: the usual one on Windows)
        check("a path written in another case is the same folder (compared by identity, not by name)",
              packs.resolves_inside(str(rt), upper))
    for rel, ok in (("romfs/a.bin", True), ("longshot.exe", True), ("..", False), ("a/../b", False), ("a\\..\\b", False),
                    (".", False), ("", False), ("/x", False), ("romfs/foo:evil", False), ("romfs/foo::$DATA", False)):
        check("in_runtime(%r) is %s" % (rel, ok), packs.in_runtime(str(rt), rel) == ok)


def _pack(base_dir, b, name, meta):
    """A pack folder with this pack.json; returns [name] for an rt command line"""
    p = base_dir / "packs" / name
    (p / "files").mkdir(parents=True, exist_ok=True)
    (p / "pack.json").write_text(json.dumps(dict({"base": b["commit"], "files": {}, "generated": [], "deleted": [], "prepare": False},
                                                 **meta)))
    return [name]


def fake_base(d):
    """A base with a game folder: romfs, both modules, two caches"""
    base_dir, game = d / "base", d / "base" / "game"
    write(game / "romfs" / "a" / "data.bin", b"romfs")
    write(game / packs.GAME, b"base game")
    write(game / packs.RENDERER, b"base renderer")
    write(game / "cache" / "longshot_programs.bin", b"programs")
    write(game / "cache" / "longshot_vk_pipelines.bin", b"pipelines")
    packs.write_base(str(base_dir), {"commit": "c" * 40, "work": str(base_dir / "work"), "game": str(game), "game_source": "g.nup"})
    return base_dir, game


def test_outputs_before_prepare(d):
    """The base's outputs are what longshot-builder made, recorded before the preparation: a file the preparation writes beside the
    game (here one no list names) is never an output, so no pack can delete it; the preparation gets the Compile shaders level"""
    calls = []

    def fake_build(program, tree, b, work, game):
        calls.append(("build", b["game_source"]))
        for m in packs.MODULES:
            write(Path(game) / m[0], b"built")
            if m[1]:
                write(Path(work) / "build" / m[1], b"map")
        write(Path(game) / "longshot_shaders.bin", b"seed")

    def fake_run(argv, cwd):
        calls.append(("prepare", list(argv)))
        game = Path(cwd)
        write(game / "cache" / "longshot_vk_pipelines.stamp", b"stamp")
        write(game / "cache" / "longshot_pipelines.bin.predicted_keys", b"keys")
        write(game / "longshot_some_future_cache.dat", b"a file a later renderer writes")
        return 0, []
    saved = packs.longshot_builder, packs.changed_files, packs.head, packs.run_events, packs.builder
    packs.longshot_builder, packs.changed_files, packs.head = fake_build, (lambda tree, commit: set()), (lambda tree, *a: "c" * 40)
    packs.builder = lambda tree, keep: "longshot-builder"
    packs.run_events = fake_run
    try:
        packs_main(["base", str(d / "b"), "--game", "g.nup", "--tree", str(ROOT)])
    finally:
        packs.longshot_builder, packs.changed_files, packs.head, packs.run_events, packs.builder = saved
    outputs = json.loads((d / "b" / "base.json").read_text())["outputs"]
    check("longshot-builder builds first, from the base's game (%s)" % calls, calls and calls[0][0] == "build")
    if not packs.CROSS:
        prep = [c for c in calls if c[0] == "prepare"]
        check("the preparation runs once, after the build, supervised by longshot-builder prepare with the Compile shaders "
              "level (%s)" % prep, len(prep) == 1 and prep[0][1][:3] == ["longshot-builder", "prepare", str(d / "b" / "game")]
              and "--compile-shaders" in prep[0][1])
        check("what the preparation wrote is not an output (%s)" % outputs,
              not {"cache/longshot_vk_pipelines.stamp", "cache/longshot_pipelines.bin.predicted_keys", "longshot_some_future_cache.dat"}
              & set(outputs))
    check("what longshot-builder made is (%s)" % outputs, "longshot_shaders.bin" in outputs)


def test_prepare_outcomes(d):
    """A base whose preparation fails (exit 3: no usable Vulkan) is kept, base.json written with the reason, as longshot-builder's
    own preparation leaves the work to the first start; LONGSHOT_NO_PREPARE skips it by value (0 still prepares)"""
    if packs.CROSS:
        return

    def fake_build(program, tree, b, work, game):
        for m in packs.MODULES:
            write(Path(game) / m[0], b"built")
            if m[1]:
                write(Path(work) / "build" / m[1], b"map")
    vulkan = {"v": 1, "type": "warning", "code": "shaders_not_prepared",
              "text": "The shaders were not prepared now (no usable Vulkan here): the game prepares them at its first start."}
    for name, env, code, prepares, why in (("vulkan", None, (1, [vulkan]), True, "no usable Vulkan here"),
                                           ("oserror", None, OSError("cannot start"), True, "cannot start"),
                                           ("silent", None, (1, []), True, "longshot-builder prepare stopped with exit status 1"),
                                           ("skip", "1", (0, []), False, "LONGSHOT_NO_PREPARE is set"),
                                           ("zero", "0", (0, []), True, None)):
        calls = []

        def fake_run(argv, cwd):
            calls.append(argv)
            if isinstance(code, OSError):   # (longshot-builder could not start)
                raise code
            return code
        saved = packs.longshot_builder, packs.changed_files, packs.head, packs.run_events, packs.builder, os.environ.get("LONGSHOT_NO_PREPARE")
        packs.longshot_builder, packs.changed_files, packs.head = fake_build, (lambda tree, commit: set()), (lambda tree, *a: "c" * 40)
        packs.builder = lambda tree, keep: "longshot-builder"
        packs.run_events = fake_run
        if env is None:
            os.environ.pop("LONGSHOT_NO_PREPARE", None)
        else:
            os.environ["LONGSHOT_NO_PREPARE"] = env
        try:
            packs_main(["base", str(d / name), "--game", "g.nup", "--tree", str(ROOT)])
        except (packs.subprocess.CalledProcessError, OSError):
            pass
        finally:
            packs.longshot_builder, packs.changed_files, packs.head, packs.run_events, packs.builder = saved[:5]
            if saved[5] is None:
                os.environ.pop("LONGSHOT_NO_PREPARE", None)
            else:
                os.environ["LONGSHOT_NO_PREPARE"] = saved[5]
        bj = d / name / "base.json"
        check("%s: the base is kept (base.json written)" % name, bj.exists())
        b = json.loads(bj.read_text()) if bj.exists() else {}
        check("%s: the preparation %s (%d runs)" % (name, "runs" if prepares else "is skipped", len(calls)), (len(calls) == 1) == prepares)
        check("%s: base.json says why it was not prepared (%r)" % (name, b.get("not_prepared")), b.get("not_prepared") == why)


def test_seed(d):
    """--seed copies every cache the seed folder's cache/ holds, the play files too (recent pipelines can live only there),
    and nothing else"""
    seed = d / "seed"
    for name, data in (("longshot_vk_pipelines.bin", b"p"), ("longshot_vk_pipelines_play_4.bin", b"play"),
                       ("longshot_vk_pipelines.stamp", b"stamp"), ("longshot_log.txt", b"not a cache")):
        write(seed / "cache" / name, data)
    for name, data in (("longshot_programs.bin", b"loose beside the game: not a cache of this layout"), ("image.bin", b"game")):
        write(seed / name, data)

    def fake_build(program, tree, b, work, game):
        for m in packs.MODULES:
            write(Path(game) / m[0], b"built")
            if m[1]:
                write(Path(work) / "build" / m[1], b"map")
    saved = packs.longshot_builder, packs.changed_files, packs.head, packs.run_events, packs.builder
    packs.longshot_builder, packs.changed_files, packs.head = fake_build, (lambda tree, commit: set()), (lambda tree, *a: "c" * 40)
    packs.builder = lambda tree, keep: "longshot-builder"
    packs.run_events = lambda argv, cwd: (0, [])
    try:
        packs_main(["base", str(d / "b"), "--game", "g.nup", "--tree", str(ROOT), "--seed", str(seed)])
    finally:
        packs.longshot_builder, packs.changed_files, packs.head, packs.run_events, packs.builder = saved
    game = d / "b" / "game"
    for name in ("longshot_vk_pipelines.bin", "longshot_vk_pipelines_play_4.bin", "longshot_vk_pipelines.stamp"):
        check("the seed's cache/%s is copied into cache/" % name, (game / "cache" / name).exists())
    for name in ("longshot_programs.bin", "image.bin", "cache/longshot_log.txt"):
        check("the seed's %s is not" % name, not (game / name).exists())


def test_warm(d):
    """warm plays a runtime with no pack, copies back the caches play made or grew and nothing else, and deletes the runtime"""
    base_dir, game = fake_base(d)
    play = d / "play.py"
    play.write_text("import sys, os\n"
                    "rt = sys.argv[1]\n"
                    "open(os.path.join(rt, 'cache', 'longshot_programs.bin'), 'ab').write(b' and more')\n"
                    "open(os.path.join(rt, 'cache', 'longshot_vk_pipelines_play_1.bin'), 'wb').write(b'played')\n"
                    "os.makedirs(os.path.join(rt, 'logs'), exist_ok=True)\n"
                    "open(os.path.join(rt, 'logs', 'longshot_log.txt'), 'w').write('a log')\n"
                    "open(os.path.join(rt, 'cache', 'longshot_log.txt'), 'w').write('not a cache either')\n"
                    "open(os.path.join(rt, %r), 'wb').write(b'not a cache')\n" % packs.GAME)
    cmd = '"%s" -I "%s" "{runtime}"' % (sys.executable.replace("\\", "/"), str(play).replace("\\", "/"))
    packs_main(["warm", str(base_dir), "--run", cmd])
    check("a grown cache comes back", (game / "cache" / "longshot_programs.bin").read_bytes() == b"programs and more")
    check("a new play file comes back", (game / "cache" / "longshot_vk_pipelines_play_1.bin").read_bytes() == b"played")
    check("an unchanged cache stays", (game / "cache" / "longshot_vk_pipelines.bin").read_bytes() == b"pipelines")
    check("a log is not a cache", not (game / "logs").exists() and not (game / "cache" / "longshot_log.txt").exists())
    check("a module is never copied back", (game / packs.GAME).read_bytes() == b"base game")
    check("the warm-up's runtime is gone", not (base_dir / "warm-runtime").exists())
    b = json.loads((base_dir / "base.json").read_text())
    check("base.json records the warm-up (%s)" % b.get("warmed"),
          b.get("warmed", {}).get("files") == ["longshot_programs.bin", "longshot_vk_pipelines_play_1.bin"])
    check("the busy marker is gone after the job", not (base_dir / packs.BUSY).exists())
    packs_main(["rt", str(base_dir), str(base_dir / "warm-runtime")])
    packs_main(["warm", str(base_dir), "--run", cmd])
    check("a runtime made beforehand by rt is warmed and deleted", not (base_dir / "warm-runtime").exists())
    packs_main(["rt", str(base_dir), str(base_dir / "warm-runtime")])
    p = base_dir / "packs" / "x"
    write(p / packs.GAME, b"pack game")
    (p / "pack.json").write_text(json.dumps({"base": "c" * 40, "files": {packs.GAME: "h"}, "generated": [], "deleted": [],
                                            "prepare": False}))
    (base_dir / "warm-runtime" / "dev_packs.json").write_text(json.dumps({"base": "c" * 40, "packs": ["x"]}))
    refused("warming a runtime with a pack in it", ["warm", str(base_dir), "--run", cmd], "not this base's runtime with no pack")
    shutil.rmtree(base_dir / "warm-runtime")
    # (a cache in the runtime that is a link to a file outside it is never followed: its bytes stay out of the base)
    outside = d / "outside.bin"
    outside.write_bytes(b"not a cache of this runtime")
    packs_main(["rt", str(base_dir), str(base_dir / "warm-runtime")])
    lnk = base_dir / "warm-runtime" / "cache" / "longshot_vk_pipelines_play_7.bin"
    try:
        os.symlink(outside, lnk)
    except OSError:
        lnk = None
    if lnk is None:
        print("  skipped: a file link in the warm-up's runtime (this account cannot make one)")
        shutil.rmtree(base_dir / "warm-runtime")
    else:
        packs_main(["warm", str(base_dir), "--run", '"%s" -c "pass"' % sys.executable.replace("\\", "/")])
        check("a linked cache file is not copied back", not (game / "cache" / "longshot_vk_pipelines_play_7.bin").exists())


def test_busy(d):
    """A second job on a busy base is refused, and the other job's marker is left alone"""
    base_dir, game = fake_base(d)
    (base_dir / packs.BUSY).write_text("1234 pack 2026-11-12T12:00:00Z")
    refused("a runtime while the base is busy", ["rt", str(base_dir), str(d / "rt-x")], "busy")
    refused("a warm-up while the base is busy", ["warm", str(base_dir), "--run", "x"], "busy")
    check("nothing made", not (d / "rt-x").exists() and not (base_dir / "warm-runtime").exists())
    check("the other job's marker is untouched", (base_dir / packs.BUSY).read_text() == "1234 pack 2026-11-12T12:00:00Z")


def test_busy_folders_above_hidden(d):
    """The Beanstalk agent's Windows sandbox may open the base's folder but not see the folders above it (they look missing,
    and making one is refused): taking and leaving the busy marker, and making a folder inside, still work"""
    base = (d / "granted" / "base").resolve()
    base.mkdir(parents=True)
    real_exists, real_isdir, real_mkdir = os.path.exists, os.path.isdir, os.mkdir

    def inside(p):
        p = os.path.abspath(p)
        return p == str(base) or p.startswith(str(base) + os.sep)

    def exists(p):
        return real_exists(p) if inside(p) else False

    def isdir(p):
        return real_isdir(p) if inside(p) else False

    def mkdir(p, *args, **kwargs):
        if not inside(p):
            raise PermissionError(13, "Access is denied", p)
        return real_mkdir(p, *args, **kwargs)

    os.path.exists, os.path.isdir, os.mkdir = exists, isdir, mkdir
    try:
        with packs.Busy(str(base), "pack"):
            check("the marker is taken", real_exists(base / packs.BUSY))
        common.ensure_dir(str(base))
        common.ensure_dir(str(base / "new"))
    finally:
        os.path.exists, os.path.isdir, os.mkdir = real_exists, real_isdir, real_mkdir
    check("and left", not (base / packs.BUSY).exists())
    check("a folder inside is still made", (base / "new").is_dir())


def test_env_upscalers():
    """(the Test Farm's upscaler builds, main's #2402) with no --upscaler, a base builds with the upscalers the environment
    names: DLSS by LONGSHOT_NGX_SDK, FSR 4 by LONGSHOT_FSR, XeSS by LONGSHOT_XESS_SDK; a vendor's runtime fetched under its
    license (LONGSHOT_FFX_SDK, LONGSHOT_XESS_SDK) is that license accepted, for longshot-builder to fetch its own"""
    e = packs.env_upscalers
    check("nothing asked: none", e({}) == ([], []))
    check("LONGSHOT_FSR=0 is off", e({"LONGSHOT_FSR": "0"}) == ([], []))
    check("FSR 4", e({"LONGSHOT_FSR": "1"}) == (["fsr4"], []))
    check("FSR 4 with AMD's fetched runtime", e({"LONGSHOT_FSR": "1", "LONGSHOT_FFX_SDK": "/sdk/ffx"}) == (["fsr4"], ["amd"]))
    check("AMD's runtime without FSR 4: nothing", e({"LONGSHOT_FFX_SDK": "/sdk/ffx"}) == ([], []))
    check("Intel's XeSS library", e({"LONGSHOT_XESS_SDK": "/sdk/xess"}) == (["xess"], ["intel"]))
    check("DLSS by the developer's own SDK, no license", e({"LONGSHOT_NGX_SDK": "/sdk/ngx"}) == (["dlss"], []))
    ups, lic = e({"LONGSHOT_NGX_SDK": "n", "LONGSHOT_FSR": "1", "LONGSHOT_FFX_SDK": "f", "LONGSHOT_XESS_SDK": "x"})
    check("each a longshot-builder upscaler (%s)" % ups, set(ups) <= set(packs.UPSCALERS) and len(ups) == 3)
    main_rs = (ROOT / "builder" / "crates" / "longshot-builder" / "src" / "main.rs").read_text(encoding="utf-8")
    check("--upscaler and --accept-licenses are longshot-builder's", "dlss,fsr4,xess" in main_rs and "nvidia,amd,intel" in main_rs)
    check("each license a vendor it takes", set(lic) <= {"nvidia", "amd", "intel"})
    # (a base: the environment's when --upscaler names none, its licenses passed to every build of it; --upscaler wins)
    for given, env, want, want_lic in (("", {"LONGSHOT_FSR": "1", "LONGSHOT_XESS_SDK": "x"}, ["fsr4", "xess"], ["intel"]),
                                       ("dlss", {"LONGSHOT_FSR": "1"}, ["dlss"], [])):
        argv = []

        def fake_build(program, tree, b, work, game):
            argv.append(b)
            for m in packs.MODULES:
                write(Path(game) / m[0], b"built")
        saved = (packs.longshot_builder, packs.changed_files, packs.head, packs.builder, dict(os.environ))
        packs.longshot_builder, packs.changed_files, packs.head = fake_build, (lambda tree, commit: set()), (lambda tree, *a: "c" * 40)
        packs.builder = lambda tree, keep: "longshot-builder"
        for k in ("LONGSHOT_NGX_SDK", "LONGSHOT_FSR", "LONGSHOT_FFX_SDK", "LONGSHOT_XESS_SDK"):
            os.environ.pop(k, None)
        os.environ.update(env, LONGSHOT_NO_PREPARE="1")
        try:
            with tempfile.TemporaryDirectory(prefix="dev-packs-ups-") as t:
                packs_main(["base", str(Path(t) / "b"), "--game", "g.nup", "--tree", str(ROOT)] +
                           (["--upscaler", given] if given else []))
        finally:
            packs.longshot_builder, packs.changed_files, packs.head, packs.builder = saved[:4]
            os.environ.clear()
            os.environ.update(saved[4])
        b = argv[0]
        check("--upscaler %r with %s: %s, licenses %s" % (given, env, b.get("upscalers"), b.get("licenses")),
              b.get("upscalers") == want and b.get("licenses") == want_lic)
    # (the command longshot-builder is given)
    seen = []
    saved = packs.run, packs.store
    packs.run, packs.store = (lambda cmd, cwd, env=None, events=False: seen.append(cmd)), (lambda: "store")
    try:
        packs.longshot_builder("lb", ".", {"game_source": "g.nup", "upscalers": ["fsr4"], "licenses": ["amd"]}, "w", "o")
    finally:
        packs.run, packs.store = saved
    cmd = seen[0]
    check("the build is given the base's upscalers and licenses (%s)" % cmd,
          cmd[cmd.index("--upscaler") + 1] == "fsr4" and cmd[cmd.index("--accept-licenses") + 1] == "amd")


def test_json(d):
    """`dev.py packs --json` (or LONGSHOT_BUILD_JSON=1): longshot-builder is run with --output json and its event lines reach
    stdout as it wrote them, this script's own lines going to stderr; without it, with --output text (never a window)"""
    import subprocess
    if os.name == "nt":
        return
    fake = d / "longshot-builder"
    fake.write_text("#!/bin/sh\necho '{\"v\":1,\"type\":\"error\",\"code\":\"nup_moved\",\"text\":\"'\"$*\"'\"}'\n")
    fake.chmod(0o755)
    snippet = ("import sys; sys.path.insert(0, sys.argv[1]); from devtools import packs; packs.JSON = sys.argv[3] == '1'; "
               "packs.longshot_builder(sys.argv[2], '.', {'game_source': 'g.nup'}, 'w', 'o')")
    def run(json_on):
        env = dict(os.environ, LONGSHOT_STORE=str(d / "store"))
        return subprocess.run([sys.executable, "-I", "-c", snippet, str(ROOT / "scripts"), str(fake), "1" if json_on else "0"],
                              cwd=d, env=env, capture_output=True, text=True)
    on = run(True)
    lines = on.stdout.splitlines()
    check("one event line on stdout, as the builder wrote it: %r" % on.stdout,
          len(lines) == 1 and json.loads(lines[0])["code"] == "nup_moved")
    check("the builder was asked for JSON", lines[0].rstrip('"}').endswith("--output json"))
    check("this script's own lines on stderr", on.stderr.startswith("+ ") and "+ " not in on.stdout)
    off = run(False)
    check("without --json: the command line on stdout, the builder asked for text",
          off.stdout.startswith("+ ") and "--output text" in off.stdout and "--output json" not in off.stdout)
    # (the switch: the option, or the variable)
    parsed = dev.parser().parse_args(["packs", "--json", "base", "x", "--game", "g"])
    check("--json parses before the subcommand", packs.json_wanted(parsed))
    os.environ["LONGSHOT_BUILD_JSON"] = "1"
    try:
        check("LONGSHOT_BUILD_JSON=1 asks for it", packs.json_wanted(argparse_namespace()))
    finally:
        del os.environ["LONGSHOT_BUILD_JSON"]
    check("neither: text", not packs.json_wanted(argparse_namespace()))


def argparse_namespace(**kw):
    import argparse
    return argparse.Namespace(**kw)


def test_builder_lock_not_held_across_the_queue(d):
    """packs.builder never holds the cargo target's lock while it waits for a queue slot: a running job that builds
    longshot-builder itself (inside the job: directly, under that lock) would wait for the lock while this waited for that
    job's slot. Queued, the job it asks for takes the lock itself (`dev.py packs builder`); run directly, the compile
    holds it"""
    from devtools import jobqueue
    saved_env = {k: os.environ.get(k) for k in ("LONGSHOT_DEV_HOME", "LONGSHOT_CARGO", "LONGSHOT_BUILD")}
    saved = packs.why_direct, jobqueue.run
    os.environ["LONGSHOT_DEV_HOME"] = str(d / "home")
    os.environ.pop("LONGSHOT_BUILD", None)
    (d / "home" / "cargo").mkdir(parents=True)
    lock = common.dev_path("cargo") + ".lock"
    tree, keep = d / "tree", d / "keep" / "longshot-builder"
    (tree / "builder").mkdir(parents=True)
    asked = []

    def fake_run(name, mode, cmd, environ=None, cwd=None, desc=None, **kw):
        try:
            with common.Lock(lock, wait=False):
                held = False
        except BlockingIOError:
            held = True
        asked.append((name, mode, cmd, held))
        return 0

    # (a cargo that says whether the lock was held while it ran, and leaves a program where cargo puts it)
    cargo = d / "cargo.py"
    cargo.write_text(
        "import fcntl, os, sys\n"
        "f = open(%r, 'a+')\n"
        "try:\n    fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB); held = 'free'\n"
        "except OSError:\n    held = 'held'\n"
        "out = os.path.join(os.environ['CARGO_TARGET_DIR'], 'release')\n"
        "os.makedirs(out, exist_ok=True)\n"
        "open(os.path.join(out, 'longshot-builder'), 'w').write(held)\n" % lock, encoding="utf-8")
    cargo_sh = d / "cargo"
    cargo_sh.write_text("#!/bin/sh\nexec %s %s \"$@\"\n" % (sys.executable, cargo), encoding="utf-8")
    cargo_sh.chmod(0o755)
    os.environ["LONGSHOT_CARGO"] = str(cargo_sh)
    try:
        packs.why_direct, jobqueue.run = (lambda environ: None), fake_run
        check("queued: the program's place", packs.builder(str(tree), str(keep)) == str(keep))
        check("queued: one job (%s)" % asked, len(asked) == 1 and asked[0][1] == "build")
        check("the lock is not held while the job waits for its slot", not asked[0][3])
        cmd = asked[0][2]
        check("the job is dev.py packs builder for the tree and the program (%s)" % cmd,
              cmd[2].endswith(os.path.join("scripts", "dev.py")) and cmd[3:] == ["packs", "builder", "--tree", str(tree), "--keep", str(keep)])
        if os.name != "nt":
            # (the job itself, or a build inside a job: directly, under the lock)
            packs.why_direct = lambda environ: "inside a job"
            with contextlib.redirect_stderr(io.StringIO()), contextlib.redirect_stdout(io.StringIO()):
                packs.builder(str(tree), str(keep))
            check("run directly, the compile holds the lock", keep.read_text() == "held")
            check("and lets it go", not os.path.exists(str(keep) + ".new"))
            with common.Lock(lock, wait=False):
                pass
    finally:
        packs.why_direct, jobqueue.run = saved
        for k, v in saved_env.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v


def test_run_other_commands_get_the_dev_folders(d):
    """`dev.py packs run -- <a command that is no build>` (stamps, check, prepare...) runs longshot-builder with the
    development folder's builder-cache/ too, never the player's cache"""
    if os.name == "nt":
        return
    saved = {k: os.environ.get(k) for k in ("LONGSHOT_DEV_HOME", "LONGSHOT_BUILD", "LONGSHOT_CACHE")}
    seen = d / "seen.txt"
    program = d / "longshot-builder"
    program.write_text('#!/bin/sh\nprintf "%%s\\n" "$LONGSHOT_CACHE" > %s\n' % seen, encoding="utf-8")
    program.chmod(0o755)
    try:
        os.environ["LONGSHOT_DEV_HOME"] = str(d / "home")
        os.environ["LONGSHOT_BUILD"] = str(program)
        os.environ.pop("LONGSHOT_CACHE", None)
        with contextlib.redirect_stdout(io.StringIO()):
            rc = packs_main(["run", "--tree", str(d), "--", "stamps"])
        check("packs run's status (%s)" % rc, rc == 0)
        cache, = seen.read_text(encoding="utf-8").splitlines()
        check("the development folder's cache (%s)" % cache, cache == common.dev_path("builder-cache"))
    finally:
        for k, v in saved.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v


def main():
    test_env_upscalers()
    test_setting_flags()
    test_route()
    test_built_covers_what_rust_reads()
    test_stamped_objects_match_build()
    test_caches_named_in_renderer()
    test_inputs_beside_the_objects()
    with tempfile.TemporaryDirectory(prefix="longshot-dev-packs-") as d:
        d = Path(d)
        for name, t in (("mc", test_module_changed), ("pf", test_pack_folders), ("rt", test_generated_and_runtime),
                        ("guard", test_guards), ("inside", test_resolves_inside), ("outputs", test_outputs_before_prepare), ("prepare", test_prepare_outcomes), ("seed", test_seed), ("warm", test_warm), ("busy", test_busy), ("hidden", test_busy_folders_above_hidden),
                        ("stamp", test_stamp_matches_build), ("format", test_cache_format), ("clone", test_clone),
                        ("refresh", test_rt_refresh), ("json", test_json), ("cargo-lock", test_builder_lock_not_held_across_the_queue),
                        ("run-env", test_run_other_commands_get_the_dev_folders)):
            (d / name).mkdir()
            t(d / name)
    print("test_dev_packs: ok")


if __name__ == "__main__":
    main()
