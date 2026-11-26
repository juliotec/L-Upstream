"""Where DLSS 5's model is looked for (vk_dlss.cpp nr_runtime, over runtime_dirs), with real folders and no game: the
folders NGX searches for DLSS's runtime, in its order (RX_VK_DLSS_DIR, the game's ngx folder, LOCALAPPDATA's
Longshot/ngx), and only a file with something in it. No file: no DLSS 5 rows (vk_dlss.cpp dlss_nr_offered). On Linux the
native NGX finds its own runtime: nothing is looked for there.

    python -I -B tests/test_dlss_nr_runtime.py [--cxx g++]
"""
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile

RUN_ALL_ARGS = ["--cxx", "{cxx}"]

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))   # (python -I leaves the check's own folder off the path)
from srccut import block  # noqa: E402

FIXTURE = r'''
#include <cassert>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <string>
#include <vector>
namespace fs = std::filesystem;
fs::path game, local, envdir;
#ifdef _WIN32
fs::path env_path(const wchar_t* name) {
    const std::wstring n(name);
    return n == L"RX_VK_DLSS_DIR" ? envdir : n == L"LOCALAPPDATA" ? local : fs::path();
}
#else
fs::path env_path(const char* name) {
    const std::string n(name);
    return n == "RX_VK_DLSS_DIR" ? envdir : n == "XDG_DATA_HOME" ? local : fs::path();
}
#endif
fs::path game_dir() { return game; }
RUNTIME_DIRS
NR_RUNTIME
void put(const fs::path& p, size_t bytes) {
    fs::create_directories(p.parent_path());
    std::ofstream f(p, std::ios::binary);
    f << std::string(bytes, 'x');
}
int main(int, char** argv) {
    const fs::path root = argv[1];
    game = root / "game", local = root / "local";
    put(game / "ngx" / "nvngx_dlss.dll", 10);   // (DLSS's own runtime: not the model)
    const fs::path in_game = game / "ngx" / "nvngx_dlssnr.dll", in_user = local / "Longshot" / "ngx" / "nvngx_dlssnr.dll";
#ifdef _WIN32
    assert(nr_runtime().empty());   // (no DLL: no rows, no message)
    put(in_game, 0);
    assert(nr_runtime().empty());   // (an empty file is not the model)
    fs::remove(in_game);
    fs::create_directories(in_game);
    assert(nr_runtime().empty());   // (nor a folder of that name)
    fs::remove(in_game);
    put(in_user, 5);
    assert(nr_runtime() == in_user);   // (the user folder, which a Launcher update leaves alone)
    put(in_game, 5);
    assert(nr_runtime() == in_game);   // (the game's ngx folder before it, as NGX searches)
    envdir = root / "env";
    put(envdir / "nvngx_dlssnr.dll", 5);
    assert(nr_runtime() == envdir / "nvngx_dlssnr.dll");   // (RX_VK_DLSS_DIR first)
    game = "relative";
    assert(nr_runtime().empty());   // (no game folder known: NGX does not start either, ngx_init)
    game.clear();
    assert(nr_runtime().empty());
#else
    put(in_game, 5);
    assert(nr_runtime().empty());   // (Linux: RX_VK_DLSS_NR_LINUX, and the native NGX's own search)
#endif
    std::puts("PASS");
}
'''


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cxx", default="g++")
    args = parser.parse_args()
    source = (ROOT / "gpu" / "vk" / "vk_dlss.cpp").read_text(encoding="utf-8")
    fixture = FIXTURE.replace("RUNTIME_DIRS", block(source, "std::vector<fs::path> runtime_dirs(const fs::path& game) {"))
    fixture = fixture.replace("NR_RUNTIME", block(source, "fs::path nr_runtime() {"))
    with tempfile.TemporaryDirectory(prefix="nr-runtime-") as tmp:
        cpp, exe = Path(tmp) / "runtime.cpp", Path(tmp) / "runtime.exe"
        cpp.write_text(fixture, encoding="utf-8")
        subprocess.run([args.cxx, "-std=c++20", "-Wall", "-Wextra", "-Werror", str(cpp), "-o", str(exe)], check=True)
        folders = Path(tmp) / "folders"
        folders.mkdir()
        out = subprocess.run([str(exe), str(folders)], capture_output=True, encoding="utf-8")
    if out.returncode or "PASS" not in out.stdout:
        raise SystemExit("FAIL: DLSS 5's model looked for in the wrong places:\n" + out.stdout + out.stderr)
    print("PASS: DLSS 5's model looked for where NGX looks, a file with something in it; none, no rows")


if __name__ == "__main__":
    main()
