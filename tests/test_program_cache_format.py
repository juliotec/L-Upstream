"""The program cache is kept across builds that change only how it is read or scheduled, and dropped when the translator or the
cache's own format changes.

longshot_programs.bin (6.9 GB with every program of the game) is good for program_cache_stamp: the translator's stamp
(translator_stamp in gpu/vk/vk_shader.cpp, LONGSHOT_SHADER_STAMP from the builder, which hashes vk_shader.cpp and the recompiler)
mixed with kProgramCacheFormat. The cache's reads and the prepare's scheduling are in gpu/vk/vk_progcache.cpp, outside the hashed
files: an edit there kept nothing out before (they were in vk_shader.cpp, so every such change made every player translate the whole
game again). A change to what a record holds raises kProgramCacheFormat instead.

Checked here: the builder's stamp (builder/crates/build-compile/src/generated/shader_stamp.rs) is made of vk_shader.cpp and not of
vk_progcache.cpp or vk_progcache.h; and in the source, that the program cache's file, index, notes and prepared stamps all come from
program_cache_stamp, which takes translator_stamp and kProgramCacheFormat, and that the cache's reads are no longer in vk_shader.cpp.

No compiler, Vulkan, GPU or game. python -I -B tests/test_program_cache_format.py
"""
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
VK = ROOT / "gpu" / "vk"


def body(text, marker):
    if text.count(marker) != 1:
        raise SystemExit("FAIL: expected one %r" % marker)
    start = text.index(marker)
    at = text.index("{", start) + 1
    depth = 1
    while depth:
        depth += (text[at] == "{") - (text[at] == "}")
        at += 1
    return text[start:at]


def stamped():
    """(the folders, the files) under gpu/vk the builder's LONGSHOT_SHADER_STAMP is made of (shader_stamp.rs DIRS, FILES)"""
    src = (ROOT / "builder" / "crates" / "build-compile" / "src" / "generated" / "shader_stamp.rs").read_text(encoding="utf-8")
    if "-DLONGSHOT_SHADER_STAMP=0x{}ull" not in (ROOT / "builder" / "crates" / "build-compile" / "src" / "plan.rs").read_text(encoding="utf-8"):
        raise SystemExit("FAIL: the builder's LONGSHOT_SHADER_STAMP is no longer shader_stamp.rs's: update this check")
    lists = []
    for name in ("DIRS", "FILES"):
        m = re.search(r"const %s: \[&str; \d+\] = \[([^\]]*)\];" % name, src)
        if not m:
            raise SystemExit("FAIL: shader_stamp.rs has no %s: update this check" % name)
        lists.append(set(re.findall(r'"([^"]+)"', m.group(1))))
    return lists


def main():
    dirs, files = stamped()
    if "vk_shader.cpp" not in files:
        raise SystemExit("FAIL: an edit to the translator keeps the programs it translated before")
    if {"vk_progcache.cpp", "vk_progcache.h"} & files or any(d.startswith("vk_progcache") for d in dirs):
        raise SystemExit("FAIL: an edit to the program cache's reads changes the translator stamp: every player translates again")

    shader = (VK / "vk_shader.cpp").read_text(encoding="utf-8")
    cache = (VK / "vk_progcache.cpp").read_text(encoding="utf-8")
    header = (VK / "vk_progcache.h").read_text(encoding="utf-8")
    draw = (VK / "vk_draw.cpp").read_text(encoding="utf-8")
    translator = body(shader, "uint64_t translator_stamp() {")
    if "LONGSHOT_SHADER_STAMP" not in translator:
        raise SystemExit("FAIL: translator_stamp does not take the build's translator stamp")
    if not re.search(r"constexpr uint32_t kProgramCacheFormat = \d+;", cache):
        raise SystemExit("FAIL: the program cache has no format version of its own")
    stamp_fn = body(cache, "uint64_t program_cache_stamp() {")
    stamp_of = body(cache, "uint64_t program_cache_stamp_of(uint32_t format) {")
    if "program_cache_stamp_of(kProgramCacheFormat)" not in stamp_fn or "translator_stamp()" not in stamp_of:
        raise SystemExit("FAIL: program_cache_stamp is not the translator's stamp and the cache's format")
    # (the pipeline side outlives a format change, never a translator's: tests/test_pipelines_outlive_cache_format.py runs it)
    ok = body(cache, "bool pipelines_stamp_ok(uint64_t stamp) {")
    if "program_cache_stamp_of(format)" not in ok or "format < kProgramCacheFormat" not in ok:
        raise SystemExit("FAIL: pipelines_stamp_ok does not take this translator's earlier formats")
    init = body(cache, "void disk_init() {")
    if "const uint64_t stamp = program_cache_stamp();" not in init or "kept == stamp" not in init or "is == stamp" not in init:
        raise SystemExit("FAIL: the cache file or its index is not checked against program_cache_stamp")
    if "disk_stamp" in shader + cache + header + draw:
        raise SystemExit("FAIL: a second stamp (disk_stamp) is still used somewhere")
    if draw.count("program_cache_stamp()") < 4:
        raise SystemExit("FAIL: the notes, the state list's notes or longshot_prepared.bin are no longer stamped with the cache")
    for fn in ("void disk_init(", "void program_cache_save(", "void program_cache_read(", "size_t program_cache_load_some(",
               "void program_cache_borrow(", "void program_cache_each(", "std::vector<uint64_t> program_cache_keys(",
               "void put_info(", "void get_info(", "unsigned usable_cpus(", "unsigned half_cpus("):
        if fn + ")" in shader or re.search(re.escape(fn) + r"[^;]*\)\s*\{", shader):
            raise SystemExit("FAIL: %s is in vk_shader.cpp, inside the translator stamp" % fn.rstrip("("))
        if not re.search(re.escape(fn) + r"[^;]*\)\s*\{", cache):
            raise SystemExit("FAIL: %s is not defined in vk_progcache.cpp" % fn.rstrip("("))
    # (what a record writes as it lies in memory from vk_shader.h, which the translator stamp does not hash: each named in a
    # static_assert beside kProgramCacheFormat, so a layout change stops the build until the format is raised)
    fmt = cache.index("constexpr uint32_t kProgramCacheFormat")
    asserts = cache[fmt:cache.index("\nuint64_t program_cache_stamp()", fmt)]
    written = set(re.findall(r"put(?:_list)?\(o, p\.(\w+)\)", body(cache, "void put_head(")))
    save = body(cache, "void program_cache_save(")
    stage = set(re.findall(r"put(?:_list)?\(o, st\.(\w+)\)", save))
    if not written or not stage:
        raise SystemExit("FAIL: put_head or program_cache_save no longer writes as this test reads it")
    for member in sorted(written):
        if "Program::" + member not in asserts:
            raise SystemExit("FAIL: put_head writes Program::%s as it lies, and no static_assert holds its layout" % member)
    for member in sorted(stage - {"info"}):   # (Shader::Info: the recompiler's, in the translator stamp)
        if "Stage::" + member not in asserts:
            raise SystemExit("FAIL: program_cache_save writes Stage::%s as it lies, and no static_assert holds its layout" % member)
    if "sizeof(Program::Answer) ==" not in asserts or "offsetof(Program::Answer" not in asserts:
        raise SystemExit("FAIL: Program::Answer (the answers, written whole) has no size and offset asserts")
    print("PASS: the program cache survives edits to its own reads, and goes with the translator or its format")
    return 0


if __name__ == "__main__":
    sys.exit(main())
