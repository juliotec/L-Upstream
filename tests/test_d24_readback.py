"""A DEPTH24 texture kept as D32_SFLOAT (vk_formats.cpp, GPUs without 24-bit depth attachments) written back to the
game's memory: flushable (gpu/vk/vk_texture.cpp) took colour images only, so the depth the GPU drew never reached
memory, and the copy read the colour aspect. flushable now takes it, flush_record copies its depth aspect and
flush_finish turns each float into the game's 24-bit unorm word (clamped to [0, 1], NaN as 0, top 8 bits zero).

This compiles flush_finish's conversion out of the source and checks its words.

python -I -B tests/test_d24_readback.py --cxx clang++ --headers <Vulkan-Headers include>
"""
import argparse
from pathlib import Path
import subprocess
import tempfile

RUN_ALL_ARGS = ["--cxx", "{cxx}", "--headers", "{vulkan_headers}"]

ROOT = Path(__file__).resolve().parents[1]
GUARD = "if (ok && t.nvn_format == NVN_FORMAT_DEPTH24 && t.format == VK_FORMAT_D32_SFLOAT) {"


def block(text, marker):
    start = text.index(marker)
    end = text.index("{", start) + 1
    depth = 1
    while depth:
        depth += (text[end] == "{") - (text[end] == "}")
        end += 1
    return text[start:end]


FIXTURE = r"""
#include <vulkan/vulkan.h>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <vector>

enum { NVN_FORMAT_DEPTH24 = 0x33 };
struct Part { uint32_t level, layer; uint64_t at, n; };
struct Texture { int nvn_format; VkFormat format; };

static void convert(const Texture& t, const std::vector<Part>& parts, void* map, bool ok) {
    /* ACTUAL */
}

int main() {
    const float in[] = {0.0f, 1.0f, 0.5f, -0.25f, 2.0f, NAN, 1.0f / 16777215.0f, 0.75f};
    const uint32_t want[] = {0, 0xFFFFFF, 0x800000, 0, 0xFFFFFF, 0, 1, 0xBFFFFF};
    const int n = sizeof in / sizeof in[0];
    std::vector<uint8_t> buf(64 + n * 4, 0xEE);
    std::memcpy(buf.data() + 16, in, 16);       // (two parts, the second 16-aligned after the first)
    std::memcpy(buf.data() + 48, in + 4, 16);
    const std::vector<Part> parts = {{0, 0, 16, 16}, {1, 0, 48, 16}};
    convert({NVN_FORMAT_DEPTH24, VK_FORMAT_D32_SFLOAT}, parts, buf.data(), true);
    int bad = 0;
    for (int i = 0; i < n; i++) {
        uint32_t u;
        std::memcpy(&u, buf.data() + (i < 4 ? 16 : 48) + (i % 4) * 4, 4);
        if (u != want[i]) {
            std::printf("float %g: word %#x, wanted %#x\n", (double)in[i], u, want[i]);
            bad++;
        }
    }
    if (buf[32] != 0xEE) { std::printf("bytes between parts written\n"); bad++; }
    std::vector<uint8_t> color(32, 0);
    std::memcpy(color.data(), in, 16);
    convert({0x25, VK_FORMAT_R8G8B8A8_UNORM}, {{0, 0, 0, 16}}, color.data(), true);
    if (std::memcmp(color.data(), in, 16)) { std::printf("a colour texture was converted\n"); bad++; }
    return bad ? 1 : 0;
}
"""


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--source", type=Path, default=ROOT)
    p.add_argument("--headers", type=Path, required=True)
    p.add_argument("--cxx", default="g++")
    a = p.parse_args()
    tex = (a.source / "gpu/vk/vk_texture.cpp").read_text(encoding="utf-8")
    flushable = block(tex, "bool flushable(const Texture& t) {")
    if "(t.aspect != VK_IMAGE_ASPECT_COLOR_BIT && !d24_as_d32)" not in flushable or \
            "t.nvn_format == NVN_FORMAT_DEPTH24 && t.format == VK_FORMAT_D32_SFLOAT" not in flushable:
        raise SystemExit("flushable takes colour images only: a DEPTH24 kept as D32_SFLOAT is never written back")
    record = block(tex, "bool flush_record(Flush& f, VkCommandBuffer cmd")
    if "t.aspect == VK_IMAGE_ASPECT_DEPTH_BIT ? VK_IMAGE_ASPECT_DEPTH_BIT : VK_IMAGE_ASPECT_COLOR_BIT" not in record:
        raise SystemExit("flush_record copies the colour aspect of a depth image")
    finish = block(tex, "bool flush_finish(Flush& f, bool gpu_ok) {")
    if GUARD not in finish:
        raise SystemExit("flush_finish writes a DEPTH24 kept as D32_SFLOAT back as floats")
    if finish.index("invalidate_mapped(f.alloc, total);") > finish.index(GUARD):
        raise SystemExit("flush_finish converts the floats before the mapped range is invalidated")
    actual = block(finish, GUARD)
    with tempfile.TemporaryDirectory(prefix="d24-readback-") as temp:
        cpp, exe = Path(temp) / "fixture.cpp", Path(temp) / "fixture.exe"
        cpp.write_text(FIXTURE.replace("/* ACTUAL */", actual), encoding="utf-8")
        subprocess.run([a.cxx, "-std=c++20", "-O1", "-Wall", "-Wextra", "-Werror", "-I" + str(a.headers), str(cpp), "-o",
                        str(exe)], check=True)
        if subprocess.run([str(exe)], timeout=15).returncode:
            raise SystemExit("a DEPTH24 read back as D32_SFLOAT is not turned into the game's 24-bit words")
    print("DEPTH24 kept as D32_SFLOAT is written back as 24-bit unorm words: ok")


if __name__ == "__main__":
    main()
