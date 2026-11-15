"""Buffer copies of a DEPTH24 texture kept as D32_SFLOAT (vk_formats.cpp, GPUs without 24-bit depth attachments),
gpu/vk/vk_xfer.cpp copy_b2t: into the texture, the game's 24-bit unorm words were copied as they are into a float image
(so the depth read back was garbage); out of it, the game read floats where it expects its 24-bit word. Now a copy into
it goes through the texture's storage (texture_read_texels, whose upload converts the words), and a copy out of it is
followed by shaders/d32_to_d24.comp, which turns each float of the region into round(clamp(f, 0, 1) * 16777215).

Checks copy_b2t's two paths, the push constants against the shader's block, the shader's math (its main() run as C++
over floats from 0 to 1, NaN and out of range: 1.0 must give 0xffffff, not 1 << 24), and, when glslangValidator is
here, that d32_to_d24_comp_spv.h is made from the shader as it is now.

python -I -B tests/test_d24_copies.py [--cxx clang++] [--glslang glslangValidator]
"""
import argparse
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def block(text, marker):
    start = text.index(marker)
    end = text.index("{", start) + 1
    depth = 1
    while depth:
        depth += (text[end] == "{") - (text[end] == "}")
        end += 1
    return text[start:end]


FIXTURE = r"""
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <algorithm>
namespace glsl {   // (GLSL's names, apart from the C library's round and min)
typedef uint32_t uint;
static float uintBitsToFloat(uint u) { float f; std::memcpy(&f, &u, 4); return f; }
static float min(float a, float b) { return a < b ? a : b; }
static uint min(uint a, uint b) { return a < b ? a : b; }
static float round(float f) { return std::round(f); }

static uint convert(uint word) {
    struct { uint w[1]; } b = {{word}};
    const uint i = 0;
    /* ACTUAL */
    return b.w[i];
}
}  // namespace glsl

int main() {
    const float in[] = {0.0f, 1.0f, 0.5f, -0.25f, 2.0f, NAN, 1.0f / 16777215.0f, 0.75f, 0.99999994f, INFINITY};
    const uint32_t want[] = {0, 0xFFFFFF, 0x800000, 0, 0xFFFFFF, 0, 1, 0xBFFFFF, 0xFFFFFE, 0xFFFFFF};
    int bad = 0;
    for (unsigned k = 0; k < sizeof in / sizeof in[0]; k++) {
        uint32_t u;
        std::memcpy(&u, &in[k], 4);
        const uint32_t got = glsl::convert(u);
        if (got != want[k]) {
            std::printf("float %.9g: word %#x, wanted %#x\n", (double)in[k], got, want[k]);
            bad++;
        }
    }
    return bad ? 1 : 0;
}
"""


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--source", type=Path, default=ROOT)
    p.add_argument("--cxx", default="g++")
    p.add_argument("--glslang", default=shutil.which("glslangValidator"))
    a = p.parse_args()
    xfer = (a.source / "gpu/vk/vk_xfer.cpp").read_text(encoding="utf-8")
    b2t = block(xfer, "void copy_b2t(")
    if "d24_as_d32" not in b2t or "(astc || d24_as_d32)" not in b2t:
        raise SystemExit("copy_b2t copies the game's 24-bit words as they are into a DEPTH24 texture kept as D32_SFLOAT")
    t2b = b2t[b2t.index("vkCmdCopyImageToBuffer"):]
    if "if (d24_as_d32 || d24s8_as_d32) d32_to_d24(cmd, s, span, c);" not in t2b.split("\n", 2)[1]:
        raise SystemExit("copy_b2t leaves floats in the buffer when a DEPTH24 texture kept as D32_SFLOAT is copied out")
    conv = block(xfer, "void d32_to_d24(")
    if "span.address" in conv or "vkGetBufferDeviceAddress(dev.device, &da)" not in conv or \
            "pc.base = base + span.offset;" not in conv:
        raise SystemExit("d32_to_d24 takes the span's address from gpu_buffer_bind, which gives none: never converted")
    if "(base + span.offset) % 4" not in conv:
        raise SystemExit("d32_to_d24 reads 4-byte words through an address it never checked is 4-byte aligned")
    if "maxComputeWorkGroupCount" not in conv or "gz > most[2]" not in conv:
        raise SystemExit("d32_to_d24 dispatches one group per layer without checking maxComputeWorkGroupCount")
    if "NVN_FORMAT_DEPTH24_STENCIL8" not in b2t or "VK_FORMAT_D32_SFLOAT_S8_UINT" not in b2t:
        raise SystemExit("copy_b2t leaves floats in the buffer when a DEPTH24_STENCIL8 kept as D32_SFLOAT_S8_UINT is copied out")
    shaders = a.source / "gpu/vk/shaders"
    comp = (shaders / "d32_to_d24.comp").read_text(encoding="utf-8")
    push = re.sub(r"//[^\n]*", "", block(xfer, "struct D32ToD24Push {"))
    glsl = re.sub(r"//[^\n]*", "", block(comp, "layout(push_constant) uniform P {"))
    cpp_fields = re.findall(r"(uint64_t|uint32_t) ([\w, ]+);", push)
    glsl_fields = re.findall(r"(uvec2|uint) (\w+);", glsl)
    sizes = {"uint64_t": 8, "uint32_t": 4, "uvec2": 8, "uint": 4}
    cpp_list = [sizes[t] for t, names in cpp_fields for _ in names.split(",")]
    if cpp_list != [sizes[t] for t, _ in glsl_fields]:
        raise SystemExit("D32ToD24Push is not d32_to_d24.comp's push constants: %s against %s" % (cpp_fields, glsl_fields))
    body = block(comp, "void main() {")
    actual = body[body.index("const float v"):body.rindex("}")]
    with tempfile.TemporaryDirectory(prefix="d24-copies-") as temp:
        cpp, exe = Path(temp) / "fixture.cpp", Path(temp) / "fixture.exe"
        cpp.write_text(FIXTURE.replace("/* ACTUAL */", actual), encoding="utf-8")
        subprocess.run([a.cxx, "-std=c++20", "-O1", "-Wall", "-Werror", str(cpp), "-o", str(exe)], check=True)
        if subprocess.run([str(exe)], timeout=15).returncode:
            raise SystemExit("d32_to_d24.comp does not turn the floats into the game's 24-bit words")
        if not a.glslang:
            print("glslangValidator not found: d32_to_d24_comp_spv.h not checked")
        else:
            out = Path(temp) / "d32_to_d24_comp_spv.h"
            subprocess.run([a.glslang, "-V", "--target-env", "vulkan1.3", "--vn", "D32_TO_D24_COMP_SPV", "-o", str(out),
                            str(shaders / "d32_to_d24.comp")], check=True, capture_output=True)
            words = lambda t: re.findall(r"0x[0-9a-f]{8}", t)
            if words(out.read_text()) != words((shaders / out.name).read_text()):
                raise SystemExit("shaders/d32_to_d24_comp_spv.h is not made from d32_to_d24.comp as it is now")
    print("DEPTH24 kept as D32_SFLOAT: buffer copies go through its storage in and are converted out: ok")


if __name__ == "__main__":
    main()
