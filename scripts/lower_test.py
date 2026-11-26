"""(longshot) Builds gpu/vk/tests/lower_test.exe and runs it over a folder of dumped native programs
(RX_VK_DUMP_FAILED=<folder> in a run): what the Ampere -> Maxwell lowering still rejects, by reason and opcode.
    python -I scripts/lower_test.py <folder> [samples]   (CXX: the C++ compiler, else clang++)
"""
import os, shutil, subprocess, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "gpu", "vk", "tests", "lower_test.cpp")
HDR = os.path.join(ROOT, "gpu", "vk", "eden", "shader_recompiler", "frontend", "ampere", "lower_to_maxwell.h")
EXE = os.path.join(ROOT, "work", "lower_test.exe")


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    if not os.path.exists(EXE) or max(os.path.getmtime(SRC), os.path.getmtime(HDR)) > os.path.getmtime(EXE):
        cxx = os.environ.get("CXX") or shutil.which("clang++") or "clang++"
        cmd = [cxx] + [
            "-std=c++20", "-O1", "-Wno-everything", "-I" + os.path.join(ROOT, "gpu", "vk", "eden"),
            "-I" + os.path.join(ROOT, "gpu", "vk", "compat"), SRC, "-o", EXE]
        subprocess.run(cmd, check=True)
    return subprocess.run([EXE] + sys.argv[1:]).returncode


if __name__ == "__main__":
    sys.exit(main())
