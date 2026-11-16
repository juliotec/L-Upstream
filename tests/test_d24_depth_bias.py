#!/usr/bin/env python3
"""A D24 depth the device keeps as D32_SFLOAT is biased in D24 units (VK_EXT_depth_bias_control).

On a float depth format the constant bias unit follows the depth's exponent, so the game's D24 units came out
too small (shadow acne). The checks:
- the extension is asked for and kept only with depthBiasControl and leastRepresentableValueForceUnormRepresentation
  (VUID-VkDepthBiasRepresentationInfoEXT-leastRepresentableValueForceUnormRepresentation-08947);
- floatRepresentation and depthBiasExact are never enabled (08948 / 08949: the replay does not use them);
- vkCmdSetDepthBias2EXT is loaded and DynState carries bias_unorm, set only for an NVN D24 on a D32 image;
- the bias is set again when bias_unorm changes, with FORCE_UNORM, and vkCmdSetDepthBias stays the fallback.
"""
import argparse
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def read(root, rel):
    with open(os.path.join(root, rel), encoding="utf-8") as f:
        return f.read()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--source", default=os.path.dirname(HERE), help="the repository root to check")
    a = ap.parse_args()
    loader = read(a.source, "gpu/vk/vk_loader.h")
    internal = read(a.source, "gpu/vk/vk_internal.h")
    main_cpp = read(a.source, "gpu/vk/vk_main.cpp")
    draw = read(a.source, "gpu/vk/vk_draw.cpp")
    fails = []

    def check(ok, what):
        print(("ok   " if ok else "FAIL ") + what)
        if not ok:
            fails.append(what)

    check("X(vkCmdSetDepthBias2EXT)" in loader, "vk_loader.h loads vkCmdSetDepthBias2EXT")
    check(re.search(r"\bdepth_bias_control = false", internal) is not None, "Device has depth_bias_control")
    check("optional(VK_EXT_DEPTH_BIAS_CONTROL_EXTENSION_NAME, dev.depth_bias_control);" in main_cpp,
          "create_device asks for VK_EXT_depth_bias_control")
    check("if (dev.depth_bias_control) *tail = &dbc" in main_cpp, "its features are queried and enabled")
    m = re.search(r"dev\.depth_bias_control = dev\.depth_bias_control && dbc\.depthBiasControl &&\s*"
                  r"dbc\.leastRepresentableValueForceUnormRepresentation;", main_cpp)
    check(m is not None, "kept only with depthBiasControl and leastRepresentableValueForceUnormRepresentation (08947)")
    check("dbc.floatRepresentation = VK_FALSE, dbc.depthBiasExact = VK_FALSE;" in main_cpp,
          "floatRepresentation and depthBiasExact are not enabled (08948, 08949)")
    q = main_cpp.find("vkGetPhysicalDeviceFeatures2(dev.physical, &f);")
    check(m is not None and q >= 0 and m.start() > q, "the features are trimmed after they are queried")

    ds = re.search(r"struct DynState \{(.*?)\n\};", draw, re.S)
    check(ds is not None and re.search(r"VkBool32 bias_unorm;", ds.group(1)) is not None, "DynState has bias_unorm")
    s = re.search(r"d\.bias_unorm = (.*?);", draw, re.S)
    body = s.group(1) if s else ""
    check(all(k in body for k in ("dev.depth_bias_control", "NVN_FORMAT_DEPTH24", "NVN_FORMAT_DEPTH24_STENCIL8",
                                  "VK_FORMAT_D32_SFLOAT", "VK_FORMAT_D32_SFLOAT_S8_UINT")),
          "bias_unorm only for an NVN D24 kept as D32, on a device with the extension")
    check("if (DYN_CHANGED(bias) || DYN_CHANGED(bias_unorm))" in draw, "the bias is set again when its unit changes")
    blk = re.search(r"if \(DYN_CHANGED\(bias\) \|\| DYN_CHANGED\(bias_unorm\)\) \{(.*?)\n    \}\n", draw, re.S)
    b = blk.group(1) if blk else ""
    check("if (dev.depth_bias_control && vkCmdSetDepthBias2EXT)" in b and "vkCmdSetDepthBias2EXT(cmd, &bi);" in b,
          "vkCmdSetDepthBias2EXT with the extension")
    check("d.bias_unorm ? VK_DEPTH_BIAS_REPRESENTATION_LEAST_REPRESENTABLE_VALUE_FORCE_UNORM_EXT" in b and
          ": VK_DEPTH_BIAS_REPRESENTATION_LEAST_REPRESENTABLE_VALUE_FORMAT_EXT" in b,
          "FORCE_UNORM for bias_unorm, the format's unit otherwise")
    check(re.search(r"\} else\s*vkCmdSetDepthBias\(cmd, d\.bias\[0\], d\.bias\[1\], d\.bias\[2\]\);", b) is not None,
          "vkCmdSetDepthBias without the extension")
    # VUID-vkCmdSetDepthBias-depthBiasClamp-00790 / VUID-VkDepthBiasInfoEXT-depthBiasClamp-08950: 0 without the feature
    check("dev.depth_bias_clamp = f.features.depthBiasClamp == VK_TRUE;" in main_cpp,
          "the depthBiasClamp feature is read from the enabled features")
    sb = draw.find("d.bias[1] = s.poly_offset[2]")
    zb = draw.find("if (!dev.depth_bias_clamp) d.bias[1] = 0.0f;")
    check(sb >= 0 and zb > sb and zb < draw.find("if (DYN_CHANGED(bias)"),
          "the game's bias clamp is 0 on a device without depthBiasClamp (00790, 08950)")

    if fails:
        print("%d check(s) failed" % len(fails))
        return 1
    print("all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
