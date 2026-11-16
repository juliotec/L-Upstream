"""The NVN 5-5-5 and BGRA8 formats map to the Vulkan format with their bit layout, and sampled views fix up the rest.

gpu/vk/vk_formats.cpp left BGR5, BGR5A1, A1BGR5, BGRA8 and BGRA8_SRGB UNDEFINED, and mapped RGB5 to B5G5R5A1, whose bits
run the other way. NVN's 16-bit formats put the first named component in the low bits (as RGB565 -> B5G6R5_PACK16).
So RGB5A1 and RGB5 (R low) live in A1R5G5B5 (B low), and the sampled view swaps R and B. RGB5 and BGR5 also read
alpha as one. With VK_KHR_maintenance5 (kept only where A1B5G5R5 is sampled and rendered to) RGB5A1 and RGB5 are
A1B5G5R5, bit for bit, and the view swaps nothing. This reads both switches and create_device from the source and checks
each format's mapping, its view swizzle and when maintenance5 is used.

python -I -B tests/test_555_formats.py [--source <checkout>]
"""
import argparse
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]

WANT = {
    "RGB5": "A1R5G5B5_UNORM_PACK16", "RGB5A1": "A1R5G5B5_UNORM_PACK16",
    "BGR5": "A1R5G5B5_UNORM_PACK16", "BGR5A1": "A1R5G5B5_UNORM_PACK16", "A1BGR5": "R5G5B5A1_UNORM_PACK16",
    "RGBA4": "A4B4G4R4_UNORM_PACK16", "BGRA8": "B8G8R8A8_UNORM", "BGRA8_SRGB": "B8G8R8A8_SRGB",
}
ALPHA_ONE = {"RGB5", "BGR5"}
SWAP_RB = {"RGB5", "RGB5A1"}
# (formats without mandatory sampled support: each must be asked of the device, else UNDEFINED)
OPTIONAL = {"A1BGR5": "R5G5B5A1_UNORM_PACK16"}
GATES = {}


def block(text, marker):
    start = text.index(marker)
    end = text.index("{", start) + 1
    depth = 1
    while depth:
        depth += (text[end] == "{") - (text[end] == "}")
        end += 1
    return text[start:end]


def mapping(src):
    """NVN format name -> the VkFormat vk_format returns for it (case labels that fall through share a return)"""
    body = block(src, "VkFormatInfo vk_format(int nvn_format) {")
    out, pending = {}, []
    for m in re.finditer(r"case NVN_FORMAT_(\w+):|return \{(?:(\w+)\(\) \? )?VK_FORMAT_(\w+)|default:", body):
        if m.group(1):
            pending.append(m.group(1))
        elif m.group(3):
            for name in pending:
                out.setdefault(name, m.group(3))
                if m.group(2):
                    GATES[name] = m.group(2)
            pending = []
        else:
            pending = []
    return out


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--source", type=Path, default=ROOT)
    a = p.parse_args()
    fmt = (a.source / "gpu/vk/vk_formats.cpp").read_text(encoding="utf-8")
    tex = (a.source / "gpu/vk/vk_texture.cpp").read_text(encoding="utf-8")
    dev = (a.source / "gpu/vk/vk_main.cpp").read_text(encoding="utf-8")
    bad = []
    got = mapping(fmt)
    for name, want in WANT.items():
        if got.get(name) != want:
            bad.append("NVN_FORMAT_%s maps to %s, wanted %s" % (name, got.get(name, "UNDEFINED"), want))
    for name, vk in OPTIONAL.items():
        gate = GATES.get(name)
        fn = re.search(r"bool %s\(\) \{(.*?)\n\}" % gate, fmt, re.S) if gate else None
        at = fmt.index("case NVN_FORMAT_%s:" % name)
        ret = fmt[at:fmt.index(";", at)]
        if not fn or "VK_FORMAT_%s" % vk not in fn.group(1) or "VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT" not in fn.group(1) \
                or ": VK_FORMAT_UNDEFINED" not in ret:
            bad.append("NVN_FORMAT_%s maps to %s without asking the device for its sampled support "
                       "(no mandatory support: VUID-VkImageCreateInfo-imageCreateMaxMipLevels-02251)" % (name, vk))
    if not re.search(r"VkFormatInfo vk_format\(int nvn_format\) \{\s*(//[^\n]*\s*)*"
                     r"if \(dev\.maintenance5 && \(nvn_format == NVN_FORMAT_RGB5A1 \|\| nvn_format == NVN_FORMAT_RGB5\)\)\s*"
                     r"return \{VK_FORMAT_A1B5G5R5_UNORM_PACK16_KHR,", fmt):
        bad.append("with maintenance5 NVN_FORMAT_RGB5A1 and RGB5 are not A1B5G5R5")
    view = tex[tex.index("// (X formats read alpha as one)"):]
    switch = view[:view.index("default: break;")]
    for name in ALPHA_ONE:
        if "case NVN_FORMAT_%s:" % name not in switch:
            bad.append("the sampled view of NVN_FORMAT_%s does not read alpha as one" % name)
    swap = re.search(r"if \((.*)\)\s*for \(unsigned& s : sw\)\s*s = s == NVN_TEXTURE_SWIZZLE_R \? "
                     r"NVN_TEXTURE_SWIZZLE_B : s == NVN_TEXTURE_SWIZZLE_B \? NVN_TEXTURE_SWIZZLE_R : s;", view)
    for name in SWAP_RB:
        if not swap or "nf == NVN_FORMAT_%s" % name not in swap.group(1):
            bad.append("the sampled view of NVN_FORMAT_%s does not swap R and B" % name)
    if not swap or "!dev.maintenance5" not in swap.group(1):
        bad.append("the sampled view swaps R and B on A1B5G5R5 too (maintenance5)")
    if "optional(VK_KHR_MAINTENANCE_5_EXTENSION_NAME, dev.maintenance5);" not in dev:
        bad.append("create_device does not ask for VK_KHR_maintenance5")
    if not re.search(r"dev\.maintenance5 = dev\.maintenance5 && m5\.maintenance5 && f13\.dynamicRendering && "
                     r"!formatless_borders;", dev):
        bad.append("maintenance5 is not kept to its feature and dynamic rendering, or is kept with format-less borders")
    # (a custom border color made without a format must not sample an A1B5G5R5 view:
    # VUID-VkSamplerCustomBorderColorCreateInfoEXT-format-04015)
    border = re.search(r"const bool formatless_borders = ([^;]+);", dev)
    kept = re.search(r"dev\.custom_border_color = ([^;]+);", dev)
    same = lambda m: re.sub(r"\s+", " ", m.group(1)).strip() if m else None
    if not border or same(border) != same(kept) or dev.index(border.group(0)) > dev.index("dev.maintenance5 = dev.maintenance5"):
        bad.append("maintenance5 is not turned off where custom border colors are made without a format")
    draw = (a.source / "gpu/vk/vk_draw.cpp").read_text(encoding="utf-8")
    # (either every custom border color has no format, or an A1B5G5R5 view's gets that format)
    formatted = re.search(r"case VK_FORMAT_A1B5G5R5_UNORM_PACK16:[^}]*return view_format;", draw) and "bc.format = bfmt;" in draw
    if "bc.format = VK_FORMAT_UNDEFINED;" not in draw and not formatted:
        bad.append("vk_draw.cpp's custom border colors now have a format: recheck the maintenance5 rule")
    if not re.search(r"VK_FORMAT_A1B5G5R5_UNORM_PACK16_KHR, &fp\);\s*const VkFormatFeatureFlags need = "
                     r"VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT \| VK_FORMAT_FEATURE_COLOR_ATTACHMENT_BIT;\s*"
                     r"if \(\(fp\.optimalTilingFeatures & need\) != need\) dev\.maintenance5 = false;", dev):
        bad.append("maintenance5 is used where A1B5G5R5 is not sampled and rendered to")
    if "m5.maintenance5 = dev.maintenance5;" not in dev:
        bad.append("the maintenance5 feature is enabled where it is not used")
    if bad:
        raise SystemExit("\n".join(bad))
    print("5-5-5, 4-4-4-4 and BGRA8 formats map with their bit layout (A1B5G5R5 with maintenance5): ok")


if __name__ == "__main__":
    main()
