"""The Windows game builds the Vulkan back end only, as the Linux one does: no WebGPU renderer, no wgpu-native.

Every Windows build used to compile TOTKPort's WebGPU renderer (gpu/render/*.cpp) and suyu's shader recompiler with its WGSL
back end into the exe (the old builder's gpu_sources, ~160 C++ files at -O2) when tools.lock's wgpu-native was there, and the
builder downloaded it for every player, while the game drew with it only when RX_BACKEND named another renderer than Vulkan
or Vulkan did not start. The renderer and RX_BACKEND are gone: the exe is linked with the stand-ins host/rx_nowgpu.c (Linux:
host/posix/linux_stubs.c), which draw nothing.

Reads the source: the build compiles nothing of gpu/render's and names no wgpu-native, no tool list asks for it, host/rx_backend.c
starts the Vulkan back end without RX_BACKEND, and every renderer function the host names is defined by the stand-ins on both
platforms (the link). No game, no compiler.

python -I -B tests/test_no_webgpu.py
"""
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def check(what, ok):
    if not ok:
        raise SystemExit("FAIL: " + what)


def read(*parts):
    return (ROOT.joinpath(*parts)).read_text(encoding="utf-8")


def defined(source):
    """The C functions a file defines (a name, its parameters, then its body)"""
    return set(re.findall(r"^[A-Za-z_][\w \*]*?\b(\w+)\([^;{]*\)\s*\{", source, re.M))


def main():
    # (the builder: builder/crates/build-compile and build-driver name no wgpu-native and compile nothing of gpu/render;
    # the Linux host leaves rx_nowgpu.c out for linux_stubs.c, the Windows one compiles it)
    rust = ROOT / "builder" / "crates"
    build = "\n".join(p.read_text(encoding="utf-8") for crate in ("build-compile", "build-driver")
                      for p in sorted((rust / crate / "src").rglob("*.rs")))
    check("the builder still names WGPU_NATIVE, wgpu-native or WebGPU",
          not re.search(r"wgpu[_-]native|webgpu", build, re.I))   # (rx_nowgpu.c is the stand-ins)
    check("the builder compiles gpu/render", "gpu/render" not in build and 'join("render")' not in build)
    plan = read("builder", "crates", "build-compile", "src", "plan.rs")
    check("the builder leaves host/rx_nowgpu.c out of the Windows build, or compiles it on Linux",
          '&& !(linux && matches!(f, "display.c" | "mf_av1.c" | "rx_nowgpu.c"))' in plan
          and '"linux_stubs.c"' in plan)
    sources = sorted(p.name for p in (ROOT / "gpu" / "render").iterdir() if p.suffix in (".c", ".cpp"))
    check("gpu/render still has sources: %s" % sources, not sources)
    for header in ("rx.h", "rx_nvn.h", "nvn_api.h"):   # (host/display.c, host/nvn_native.c, gpu/vk/vk_api.h)
        check("gpu/render/%s is gone: the host or the Vulkan back end includes it" % header, (ROOT / "gpu" / "render" / header).is_file())

    lock = read("tools.lock")
    check("tools.lock still has wgpu-native", not re.search(r"^tool wgpu-native ", lock, re.M))
    # (no builder requires it: build-driver's tools/lock.rs, which names none, is in the builder's sources checked above)

    backend = read("host", "rx_backend.c")
    start = backend[backend.index("int rx_start(void *hwnd) {"):]
    start = start[:start.index("\n}\n")]
    check("host/rx_backend.c still reads RX_BACKEND", 'getenv("RX_BACKEND")' not in backend)
    check("rx_start does not start the Vulkan back end first, unconditionally",
          start.index("load_vulkan(dll)") < start.index("wgr_start(hwnd)") and "if (!b" not in start)
    load = backend[backend.index("static const MkwVkRenderer *load_vulkan(const char *dll) {"):]
    load = load[:load.index("\n}\n")]
    check("load_vulkan still names a WebGPU renderer to fall back to", "WebGPU" not in load and "RX_BACKEND" not in load)

    # (the link: every wgr_* the host declares, and the live input's commands, defined by each platform's stand-ins)
    wanted = set(re.findall(r"^[\w \*]*?\b(wgr_\w+)\(", backend, re.M)) | {"rx_collect_written"}
    hid = read("host", "nn_hid.c")
    wanted |= {n for n in ("rx_request_frame_dump", "rx_set_screenshot_ms", "rx_request_rdoc_capture", "rx_flash_dark_arm")
               if n + "(" in hid}
    check("host/rx_backend.c declares no wgr_* stand-ins (the check reads the wrong file)", len(wanted) > 10)
    for path in (("host", "rx_nowgpu.c"), ("host", "posix", "linux_stubs.c")):
        missing = sorted(wanted - defined(read(*path)))
        check("%s does not define %s: the game does not link" % ("/".join(path), ", ".join(missing)), not missing)
    print("ok   no WebGPU renderer and no wgpu-native in the Windows build; the stand-ins define what the host calls")


if __name__ == "__main__":
    main()
