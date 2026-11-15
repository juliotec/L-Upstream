"""DEPTH24 and DEPTH24_STENCIL8 on a GPU without 24-bit depth attachments (AMD RDNA 2, 3 and 4 have neither
X8_D24_UNORM_PACK32 nor D24_UNORM_S8_UINT as one): vk_format kept the D24 formats, so every depth target made with
them failed. depth24_or_32 (gpu/vk/vk_formats.cpp) asks the GPU and gives the D32 float twin where the D24 format
cannot be rendered to or sampled; upload() (gpu/vk/vk_texture.cpp) turns the guest's 24-bit unorm words into the
floats such an image holds, and splits DEPTH24_STENCIL8's words into the depth's floats and the stencil's bytes.

This compiles depth24_or_32 out of the source against a stand-in vkGetPhysicalDeviceFormatProperties and checks both
answers, then checks that upload() converts both formats.

python -I -B tests/test_d24_fallback.py --cxx clang++ --headers <Vulkan-Headers include>
"""
import argparse
from pathlib import Path
import subprocess
import tempfile

RUN_ALL_ARGS = ["--cxx", "{cxx}", "--headers", "{vulkan_headers}"]

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
#include <vulkan/vulkan.h>
#include <cstdio>

struct Dev { VkPhysicalDevice physical = VK_NULL_HANDLE; } dev;
static VkFormatFeatureFlags g_feats = 0, g_twin = 0;
static int g_logs = 0;
static void vkGetPhysicalDeviceFormatProperties_(VkPhysicalDevice, VkFormat f, VkFormatProperties* p) {
    *p = {};
    p->optimalTilingFeatures = f == VK_FORMAT_D32_SFLOAT || f == VK_FORMAT_D32_SFLOAT_S8_UINT ? g_twin : g_feats;
}
#define vkGetPhysicalDeviceFormatProperties vkGetPhysicalDeviceFormatProperties_
static void logf(const char*, ...) { g_logs++; }

namespace amd {
/* ACTUAL */
}
namespace nv {
/* ACTUAL */
}
namespace nt {
/* ACTUAL */
}

int main() {
    int bad = 0;
    g_feats = VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT;   // (sampled, not a depth attachment: AMD)
    g_twin = VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT | VK_FORMAT_FEATURE_DEPTH_STENCIL_ATTACHMENT_BIT |
             VK_FORMAT_FEATURE_TRANSFER_SRC_BIT | VK_FORMAT_FEATURE_TRANSFER_DST_BIT;
    if (amd::depth24_or_32(VK_FORMAT_X8_D24_UNORM_PACK32, VK_FORMAT_D32_SFLOAT) != VK_FORMAT_D32_SFLOAT) {
        std::printf("X8_D24 kept where the GPU cannot render to it\n");
        bad++;
    }
    if (amd::depth24_or_32(VK_FORMAT_D24_UNORM_S8_UINT, VK_FORMAT_D32_SFLOAT_S8_UINT) != VK_FORMAT_D32_SFLOAT_S8_UINT) {
        std::printf("D24_UNORM_S8 kept where the GPU cannot render to it\n");
        bad++;
    }
    g_feats = VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT | VK_FORMAT_FEATURE_DEPTH_STENCIL_ATTACHMENT_BIT;
    if (nv::depth24_or_32(VK_FORMAT_X8_D24_UNORM_PACK32, VK_FORMAT_D32_SFLOAT) != VK_FORMAT_X8_D24_UNORM_PACK32 ||
        nv::depth24_or_32(VK_FORMAT_D24_UNORM_S8_UINT, VK_FORMAT_D32_SFLOAT_S8_UINT) != VK_FORMAT_D24_UNORM_S8_UINT) {
        std::printf("a D24 format the GPU supports was replaced\n");
        bad++;
    }
    g_feats = VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT;
    g_twin = VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT | VK_FORMAT_FEATURE_DEPTH_STENCIL_ATTACHMENT_BIT;
    if (nt::depth24_or_32(VK_FORMAT_D24_UNORM_S8_UINT, VK_FORMAT_D32_SFLOAT_S8_UINT) != VK_FORMAT_D24_UNORM_S8_UINT) {
        std::printf("D32_SFLOAT_S8_UINT used where the GPU cannot copy into it\n");
        bad++;
    }
    if (g_logs != 3) {
        std::printf("%d fallback lines logged, wanted 3\n", g_logs);
        bad++;
    }
    return bad ? 1 : 0;
}
"""


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--source", type=Path, default=ROOT)
    p.add_argument("--headers", type=Path, required=True)
    p.add_argument("--cxx", default="g++")
    a = p.parse_args()
    fmt = (a.source / "gpu/vk/vk_formats.cpp").read_text(encoding="utf-8")
    tex = (a.source / "gpu/vk/vk_texture.cpp").read_text(encoding="utf-8")
    if "VkFormat depth24_or_32(VkFormat d24, VkFormat d32) {" not in fmt:
        raise SystemExit("vk_format keeps the D24 formats on every GPU: no depth24_or_32")
    for case in ("case NVN_FORMAT_DEPTH24: return {depth24_or_32(VK_FORMAT_X8_D24_UNORM_PACK32, VK_FORMAT_D32_SFLOAT),",
                 "case NVN_FORMAT_DEPTH24_STENCIL8: return {depth24_or_32(VK_FORMAT_D24_UNORM_S8_UINT, "
                 "VK_FORMAT_D32_SFLOAT_S8_UINT),"):
        if case not in fmt:
            raise SystemExit("vk_format does not use depth24_or_32: " + case.split(":")[0])
    up = block(tex, "bool upload_inner(Texture& tex, const Ranges* dirty) {")
    for need, what in (
            ("tex.nvn_format == NVN_FORMAT_DEPTH24 && tex.format == VK_FORMAT_D32_SFLOAT", "DEPTH24 as D32_SFLOAT"),
            ("(float)(v & 0xffffffu) / 16777215.0f", "the 24-bit unorm to float conversion"),
            ("&& !d24s8_as_d32) return false;", "DEPTH24_STENCIL8 as D32_SFLOAT_S8_UINT is still dropped"),
            ("sten[i / 4] = (uint8_t)(v >> 24);", "the stencil byte split"),
            ("c.imageSubresource.aspectMask = VK_IMAGE_ASPECT_STENCIL_BIT;", "the stencil copy"),
            ("d24s8_as_d32 ? s_off + s_bytes : out_bytes", "the stencil bytes flushed")):
        if need not in up:
            raise SystemExit("upload_inner: missing " + what)
    actual = block(fmt, "VkFormat depth24_or_32(VkFormat d24, VkFormat d32) {")
    with tempfile.TemporaryDirectory(prefix="d24-fallback-") as temp:
        cpp, exe = Path(temp) / "fixture.cpp", Path(temp) / "fixture.exe"
        cpp.write_text(FIXTURE.replace("/* ACTUAL */", actual), encoding="utf-8")
        subprocess.run([a.cxx, "-std=c++20", "-O1", "-Wall", "-Wextra", "-Werror", "-I" + str(a.headers), str(cpp), "-o",
                        str(exe)], check=True)
        if subprocess.run([str(exe)], timeout=15).returncode:
            raise SystemExit("depth24_or_32 does not pick the D32 twin where the GPU lacks the D24 format")
    print("DEPTH24 / DEPTH24_STENCIL8 fall back to D32 and are uploaded converted: ok")


if __name__ == "__main__":
    main()
