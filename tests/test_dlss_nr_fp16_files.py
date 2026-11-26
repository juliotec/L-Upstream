"""Where DLSS 5's community FP16 file is looked for and how it is checked (vk_dlss.cpp nr_fp16_runtime, over runtime_dirs),
with real folders and no game, GPU or NVIDIA file: <ngx folder>/fp16/nvngx_dlssnr.dll in the folders NGX searches, in its
order (RX_VK_DLSS_DIR, the game's ngx folder, LOCALAPPDATA's Longshot/ngx); an empty file or a folder of that name is not
it; a file of another size is not even read, and one whose SHA-256 is not on the allow-list is said in the log and not
used, and the next folder is looked in; the file taken stays open (the handle given back) so that no program can write,
rename or delete it while the game runs (it is the file loaded later). The allow-list itself and the hash are
tests/test_dlss_nr_model.py's: here a test file stands for the known one. Windows only, as the FP16 path is.

    python -I -B tests/test_dlss_nr_fp16_files.py [--cxx g++]
"""
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile

RUN_ALL_ARGS = ["--cxx", "{cxx}"]
RUN_ALL_SKIP = {"linux": "the FP16 file and its direct path are Windows only (vk_dlss.cpp nr_fp16_runtime under _WIN32)"}

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))   # (python -I leaves the check's own folder off the path)
from srccut import block  # noqa: E402

FIXTURE = r'''
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <cassert>
#include <cstdarg>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <string>
#include <vector>
#include "gpu/vk/dlss_nr_model.h"
namespace fs = std::filesystem;
namespace mkwvk {
fs::path game, local, envdir;
fs::path env_path(const wchar_t* name) {
    const std::wstring n(name);
    return n == L"RX_VK_DLSS_DIR" ? envdir : n == L"LOCALAPPDATA" ? local : fs::path();
}
fs::path game_dir() { return game; }
std::string shown(const fs::path& p) { return p.string(); }
std::string logged;
void logf(const char* fmt, ...) {
    char b[2048];
    va_list a;
    va_start(a, fmt);
    vsnprintf(b, sizeof b, fmt, a);
    va_end(a);
    logged += b, logged += "\n";
}
std::string known;   // (the SHA-256 and size the test's own "known" file has, standing for the FP16 file Longshot knows)
uint64_t known_size = 0;
bool test_known(const std::string& hex) { return hex == known; }
bool test_size_known(uint64_t size) { return size == known_size; }
RUNTIME_DIRS
FP16_RUNTIME
} // namespace mkwvk
using namespace mkwvk;
std::string sha(const std::string& s) { dlss_nr::Sha256 h; h.update(s.data(), s.size()); return h.hex(); }
void put(const fs::path& p, const std::string& bytes) {
    fs::create_directories(p.parent_path());
    std::ofstream f(p, std::ios::binary);
    f << bytes;
}
bool writable(const fs::path& p) {
    const HANDLE h = CreateFileW(p.c_str(), GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr, OPEN_EXISTING, 0, nullptr);
    if (h == INVALID_HANDLE_VALUE) return false;
    CloseHandle(h);
    return true;
}
/// A fresh set of folders for one case (a file taken stays open for the run, so each case has its own)
void fresh(const fs::path& root, const char* name) {
    game = root / name / "game", local = root / name / "local", envdir = root / name / "env";
    logged.clear();
}
int main(int, char** argv) {
    const fs::path root = argv[1];
    // (a file of several 4 MB reads and a part: the hash is fed as it is read)
    std::string model(9u << 20, '\0');
    for (size_t i = 0; i < model.size(); i++) model[i] = char((i * 131 + i / 4096) & 0xff);
    known = sha(model), known_size = model.size();
    const auto fp16 = [](const fs::path& ngx) { return ngx / "fp16" / "nvngx_dlssnr.dll"; };

    fresh(root, "none");   // (no file, or NVIDIA's beside the folder only: no FP16 file)
    put(game / "ngx" / "nvngx_dlssnr.dll", "NVIDIA's file");
    assert(nr_fp16_runtime().empty());

    fresh(root, "empty");   // (an empty file, a folder of that name: not it)
    put(fp16(game / "ngx"), "");
    fs::create_directories(fp16(local / "Longshot" / "ngx"));
    assert(nr_fp16_runtime().empty());

    fresh(root, "size");   // (another size: said, and not read)
    put(fp16(game / "ngx"), model + "x");
    assert(nr_fp16_runtime().empty());
    assert(logged.find("is not the known FP16 file (9437185 bytes)") != std::string::npos);
    assert(logged.find("SHA-256") == std::string::npos);

    fresh(root, "other");   // (the size, not the hash: said and not used)
    std::string other = model;
    other[12345] ^= 1;
    put(fp16(game / "ngx"), other);
    assert(nr_fp16_runtime().empty());
    assert(logged.find("is not the known FP16 file (SHA-256 " + sha(other).substr(0, 16)) != std::string::npos);

    fresh(root, "game");   // (the known file in the game's ngx folder: taken, and nobody may write, rename or delete it now)
    put(fp16(game / "ngx"), model);
    HANDLE held = INVALID_HANDLE_VALUE;
    const fs::path found = nr_fp16_runtime(&held);
    assert(found == fp16(game / "ngx") && held != INVALID_HANDLE_VALUE);
    assert(logged.find("the known file") != std::string::npos);
    assert(!writable(found));
    assert(!MoveFileW(found.c_str(), (found.wstring() + L".moved").c_str()) && !DeleteFileW(found.c_str()));
    assert(std::ifstream(found, std::ios::binary).good());   // (read it may be: Windows' loader reads it)
    CloseHandle(held);   // (the handle given back is the one that holds it)
    assert(writable(found));

    fresh(root, "order");   // (RX_VK_DLSS_DIR's other file is passed over; LOCALAPPDATA's known one taken)
    put(fp16(envdir), "not the model");
    put(fp16(local / "Longshot" / "ngx"), model);
    assert(nr_fp16_runtime() == fp16(local / "Longshot" / "ngx"));
    assert(logged.find("is not the known FP16 file") != std::string::npos);

    fresh(root, "first");   // (known files in all three: RX_VK_DLSS_DIR's first, as NGX searches)
    for (const fs::path& d : {envdir, game / "ngx", local / "Longshot" / "ngx"}) put(fp16(d), model);
    assert(nr_fp16_runtime() == fp16(envdir));
    assert(writable(fp16(game / "ngx")) && writable(fp16(local / "Longshot" / "ngx")));   // (only the one taken is held)

    game = fs::path("relative");   // (no game folder: nothing)
    assert(nr_fp16_runtime().empty());
    std::puts("the FP16 file: PASS");
}
'''


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cxx", default="g++")
    args = parser.parse_args()
    if sys.platform != "win32":
        print("SKIP: the FP16 file and its direct path are Windows only")
        return
    source = (ROOT / "gpu" / "vk" / "vk_dlss.cpp").read_text(encoding="utf-8")
    search = block(source, "fs::path nr_fp16_runtime(HANDLE* held = nullptr) {")
    if "dlss_nr::fp16_known(hex)" not in search or "dlss_nr::Sha256" not in search or "dlss_nr::fp16_size_known(size)" not in search:
        raise SystemExit("FAIL: nr_fp16_runtime no longer checks the file's SHA-256 against the allow-list (dlss_nr::fp16_known)")
    if "FILE_SHARE_READ, nullptr" not in search or "CloseHandle(file)" not in search:
        raise SystemExit("FAIL: nr_fp16_runtime no longer holds the file it takes against writes")
    fixture = FIXTURE.replace("RUNTIME_DIRS", block(source, "std::vector<fs::path> runtime_dirs(const fs::path& game) {")) \
        .replace("FP16_RUNTIME", search.replace("dlss_nr::fp16_known(hex)", "test_known(hex)")
                 .replace("dlss_nr::fp16_size_known(size)", "test_size_known(size)"))
    with tempfile.TemporaryDirectory(prefix="nr-fp16-") as tmp:
        cpp, exe = Path(tmp) / "fp16.cpp", Path(tmp) / "fp16.exe"
        cpp.write_text(fixture, encoding="utf-8")
        subprocess.run([args.cxx, "-std=c++20", "-O1", "-Wall", "-Wextra", "-Werror", "-I" + str(ROOT), str(cpp), "-o", str(exe)],
                       check=True)
        out = subprocess.run([str(exe), str(Path(tmp) / "folders")], capture_output=True, encoding="utf-8")
        if out.returncode or "PASS" not in out.stdout:
            raise SystemExit("FAIL:\n" + out.stdout + out.stderr)
    print("PASS: the FP16 file looked for in NGX's folders, in their order; only the known one taken, and held for the run")


if __name__ == "__main__":
    main()
