"""The queue waited for before a sparse bind (vk_texture.cpp sparse_flush): on by default on AMD, whose binds land
while earlier passes still sample the image, off elsewhere; RX_VK_SPARSE_IDLE turns it off or on on any GPU.

The no_idle switch is cut from sparse_flush and compiled with compat/longshot_env.h and a made-up device, then run
for AMD, NVIDIA and Intel with the variable unset, "0", "off", "1".

    python -I -B tests/test_sparse_idle.py [--cxx g++]
"""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile

RUN_ALL_ARGS = ["--cxx", "{cxx}"]
ROOT = Path(__file__).resolve().parents[1]
VK = ROOT / "gpu/vk"


def cut_switch():
    src = (VK / "vk_texture.cpp").read_text(encoding="utf-8")
    fn = src[src.index("bool sparse_flush(Texture* tex) {"):]
    at = fn.index("static const bool no_idle = ")
    end = fn.index(";", at)
    if "[]" in fn[at:end + 1]:
        end = fn.index("}();", at) + 3
    stmt = fn[at:end + 1]
    if "if (!no_idle) vkQueueWaitIdle(dev.queue);" not in fn[end:fn.index("vkQueueBindSparse", end)]:
        raise SystemExit("FAIL: sparse_flush does not wait for the queue before the bind when no_idle is false")
    return stmt.replace("static const bool", "const bool")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cxx", default="g++")
    args = ap.parse_args()
    stmt = cut_switch()
    cpp = ("#include <cstdlib>\n#include <cstdio>\n#include <cstdint>\n#include \"compat/longshot_env.h\"\n"
           "struct { struct { uint32_t vendorID; } props; } dev;\n"
           "int main(int argc, char** argv) {\n"
           "    dev.props.vendorID = (uint32_t)strtoul(argv[1], nullptr, 0);\n"
           "    " + stmt + "\n"
           "    printf(\"%d\", no_idle ? 0 : 1);\n    return 0;\n}\n")
    cases = [  # vendor, value (None: unset), wait expected
        (0x1002, None, 1), (0x10DE, None, 0), (0x8086, None, 0),
        (0x1002, "0", 0), (0x1002, "off", 0), (0x1002, "1", 1),
        (0x10DE, "1", 1), (0x8086, "yes", 1), (0x10DE, "0", 0),
    ]
    with tempfile.TemporaryDirectory(prefix="sparse-idle-", ignore_cleanup_errors=True) as tmp:
        c = Path(tmp) / "t.cpp"
        exe = Path(tmp) / "t.exe"
        c.write_text(cpp, encoding="utf-8")
        r = subprocess.run([args.cxx, "-std=c++20", "-I", str(VK), str(c), "-o", str(exe)], capture_output=True, text=True)
        if r.returncode:
            print(r.stdout + r.stderr)
            raise SystemExit("FAIL: the switch did not build")
        bad = 0
        for vendor, value, want in cases:
            env = dict(os.environ)
            env.pop("RX_VK_SPARSE_IDLE", None)
            if value is not None:
                env["RX_VK_SPARSE_IDLE"] = value
            got = int(subprocess.run([str(exe), hex(vendor)], capture_output=True, text=True, env=env).stdout)
            if got != want:
                bad += 1
                print("FAIL: vendor %#x, RX_VK_SPARSE_IDLE=%r: wait %d, %d expected" % (vendor, value, got, want))
    if bad:
        raise SystemExit("FAIL: %d of %d cases" % (bad, len(cases)))
    print("PASS: the wait before a sparse bind is on for AMD and off elsewhere, and RX_VK_SPARSE_IDLE forces it (%d cases)"
          % len(cases))


if __name__ == "__main__":
    main()
