"""DLSS 5's forwarder (gpu/vk/dlss_nr_forwarder.c, longshot_nvngx.dll_nr.dll) and the two back ends behind it (vk_dlss.cpp
nr_create_feature, nr_evaluate_feature, nr_release_feature), with no GPU, NGX or NVIDIA file.

- The forwarder, built as the build builds it (-O2, a library of its own), is loaded and each of its four calls made with
  a stand-in for the model's export: every argument reaches the model as given, its result comes back, and the model sees
  a caller (the address its call returns to) inside a module whose name holds "nvngx.dll", the model's own check. A call
  compiled as a jump (a tail call) would return into this program instead, and fails here.
- The names agree: the forwarder's file and its four exports as vk_dlss.cpp loads them, the builder links it
  (builder/crates/build-compile link_forwarder, after the renderer) and puts it beside the game (build-driver
  finished_files); it is not compiled into longshot_vk.dll (plan.rs takes gpu/vk's .cpp files only).
- Windows: the back ends, cut from vk_dlss.cpp. With NVIDIA's file, NGX's own calls (a capability block, unsigned
  integers); with the FP16 file, the model's exports through the real forwarder (a block NGX allocates, signed integers,
  the model's own handle), and nothing of NGX's feature calls.

    python -I -B tests/test_dlss_nr_forwarder.py [--cxx g++]
"""
import argparse
from pathlib import Path
import re
import subprocess
import sys
import tempfile

RUN_ALL_ARGS = ["--cxx", "{cxx}"]

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests"))   # (python -I leaves the check's own folder off the path)
from srccut import block, between  # noqa: E402

NAME = "longshot_nvngx.dll_nr.dll"
EXPORTS = ("longshot_nr_init", "longshot_nr_create", "longshot_nr_evaluate", "longshot_nr_release")

CALLS = r'''
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cwchar>
#include <string>
#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#define CALL __cdecl
#else
#include <dlfcn.h>
#define CALL
#endif
#define CHECK(x) do { if (!(x)) { std::printf("FAIL: %d: %s\n", __LINE__, #x); std::exit(1); } } while (0)
/// The module the return address is in: the model's question about its caller
std::string caller_module(void* at) {
#ifdef _WIN32
    HMODULE m = nullptr;
    if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, (LPCWSTR)at, &m))
        return "";
    char path[4096];
    const DWORD n = GetModuleFileNameA(m, path, sizeof path);
    return std::string(path, n);
#else
    Dl_info info{};
    return dladdr(at, &info) && info.dli_fname ? info.dli_fname : "";
#endif
}
int seen = 0;
std::string last_caller;
#define SEEN() (++seen, last_caller = caller_module(__builtin_return_address(0)))
extern "C" {
__attribute__((noinline)) int CALL model_init(unsigned long long app, const wchar_t* data, void* instance, void* physical,
                                               void* device, const void* info, int sdk) {
    SEEN();
    CHECK(app == 0x1234567890ull && data && !std::wcscmp(data, L"C:\\data") && instance == (void*)0x11 &&
          physical == (void*)0x22 && device == (void*)0x33 && info == (void*)0x44 && sdk == 0x15);
    return 0x101;
}
__attribute__((noinline)) int CALL model_create(void* command, int feature, void* params, void** handle) {
    SEEN();
    CHECK(command == (void*)0x55 && feature == 18 && params == (void*)0x66 && handle);
    *handle = (void*)0x77;
    return 0x102;
}
__attribute__((noinline)) int CALL model_evaluate(void* command, void* handle, void* params, void* callback) {
    SEEN();
    CHECK(command == (void*)0x55 && handle == (void*)0x77 && params == (void*)0x66 && callback == nullptr);
    return 0x103;
}
__attribute__((noinline)) int CALL model_release(void* handle) {
    SEEN();
    CHECK(handle == (void*)0x77);
    return 0x104;
}
}
using Init = int(CALL*)(void*, unsigned long long, const wchar_t*, void*, void*, void*, const void*, int);
using Create = int(CALL*)(void*, void*, int, void*, void**);
using Evaluate = int(CALL*)(void*, void*, void*, void*, void*);
using Release = int(CALL*)(void*, void*);
int main(int, char** argv) {
#ifdef _WIN32
    HMODULE lib = LoadLibraryA(argv[1]);
    const auto get = [&](const char* n) { return (void*)GetProcAddress(lib, n); };
#else
    void* lib = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    const auto get = [&](const char* n) { return dlsym(lib, n); };
#endif
    CHECK(lib);
    const auto init = (Init)get("longshot_nr_init");
    const auto create = (Create)get("longshot_nr_create");
    const auto evaluate = (Evaluate)get("longshot_nr_evaluate");
    const auto release = (Release)get("longshot_nr_release");
    CHECK(init && create && evaluate && release);
    const auto from_forwarder = [] { return last_caller.find("nvngx.dll") != std::string::npos; };
    CHECK(init((void*)model_init, 0x1234567890ull, L"C:\\data", (void*)0x11, (void*)0x22, (void*)0x33, (void*)0x44, 0x15) == 0x101);
    CHECK(seen == 1 && from_forwarder());
    void* handle = nullptr;
    CHECK(create((void*)model_create, (void*)0x55, 18, (void*)0x66, &handle) == 0x102 && handle == (void*)0x77);
    CHECK(seen == 2 && from_forwarder());
    CHECK(evaluate((void*)model_evaluate, (void*)0x55, handle, (void*)0x66, nullptr) == 0x103);
    CHECK(seen == 3 && from_forwarder());
    CHECK(release((void*)model_release, handle) == 0x104);
    CHECK(seen == 4 && from_forwarder());
    // (and a call straight from here is not from the forwarder: the check above can fail)
    model_release(handle);
    CHECK(!from_forwarder());
    std::printf("calls: PASS (%s)\n", last_caller.c_str());
}
'''

BACK_ENDS = r'''
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#define CHECK(x) do { if (!(x)) { std::printf("FAIL: %d: %s\n", __LINE__, #x); std::exit(1); } } while (0)
namespace mkwvk {
namespace dlss_nr { enum class Model { none, nvidia, fp16 }; enum class Issue { none }; }
namespace rec { enum { B_DIRECT }; inline void boundary(int) {} }
using VkCommandBuffer = void*;
struct NVSDK_NGX_Parameter { int id; };
struct NVSDK_NGX_Handle { int id; };
enum NVSDK_NGX_Result : unsigned { NVSDK_NGX_Result_Success = 0x1, NVSDK_NGX_Result_Fail = 0xBAD00000 };
enum NVSDK_NGX_Feature { NVSDK_NGX_Feature_Reserved18 = 18 };
std::string ngx;   // (what of NGX's was called, in order)
NVSDK_NGX_Parameter block_ngx{1}, block_alloc{2};
NVSDK_NGX_Handle handle_ngx{3};
NVSDK_NGX_Result NVSDK_NGX_VULKAN_GetCapabilityParameters(NVSDK_NGX_Parameter** p) { ngx += "caps "; *p = &block_ngx; return NVSDK_NGX_Result_Success; }
NVSDK_NGX_Result NVSDK_NGX_VULKAN_AllocateParameters(NVSDK_NGX_Parameter** p) { ngx += "alloc "; *p = &block_alloc; return NVSDK_NGX_Result_Success; }
void NVSDK_NGX_Parameter_SetUI(NVSDK_NGX_Parameter*, const char* k, unsigned v) { ngx += "UI:" + std::string(k) + "=" + std::to_string(v) + " "; }
void NVSDK_NGX_Parameter_SetI(NVSDK_NGX_Parameter*, const char* k, int v) { ngx += "I:" + std::string(k) + "=" + std::to_string(v) + " "; }
NVSDK_NGX_Result NVSDK_NGX_VULKAN_CreateFeature(VkCommandBuffer, NVSDK_NGX_Feature f, NVSDK_NGX_Parameter*, NVSDK_NGX_Handle** h) {
    ngx += "create" + std::to_string((int)f) + " "; *h = &handle_ngx; return NVSDK_NGX_Result_Success;
}
NVSDK_NGX_Result NVSDK_NGX_VULKAN_EvaluateFeature(VkCommandBuffer, const NVSDK_NGX_Handle*, const NVSDK_NGX_Parameter*, void*) {
    ngx += "evaluate "; return NVSDK_NGX_Result_Success;
}
NVSDK_NGX_Result NVSDK_NGX_VULKAN_ReleaseFeature(NVSDK_NGX_Handle*) { ngx += "release "; return NVSDK_NGX_Result_Success; }
struct State { dlss_nr::Model nr_model = dlss_nr::Model::none; };
State& st() { static State s; return s; }
// (the model's exports: what reached them, and from where)
std::string model;
int model_handle = 0;
extern "C" {
__attribute__((noinline)) int __cdecl fake_create(void* cmd, int f, void* p, void** h) {
    model += "create" + std::to_string(f) + (p == &block_alloc ? ":alloc" : ":?") + (cmd == (void*)0x9 ? "" : ":cmd?") + " ";
    *h = &model_handle; return 1;
}
__attribute__((noinline)) int __cdecl fake_evaluate(void*, void* h, void* p, void* cb) {
    model += std::string("evaluate") + (h == &model_handle && p == &block_alloc && !cb ? "" : ":?") + " "; return 1;
}
__attribute__((noinline)) int __cdecl fake_release(void* h) { model += std::string("release") + (h == &model_handle ? "" : ":?") + " "; return 1; }
}
@@DIRECT@@
@@BACK_ENDS@@
} // namespace mkwvk
using namespace mkwvk;
int main(int, char** argv) {
    // NVIDIA's file: NGX's own calls, as before
    st().nr_model = dlss_nr::Model::nvidia;
    CHECK(!nr_direct());
    NVSDK_NGX_Parameter* p = nullptr;
    NVSDK_NGX_Handle* h = nullptr;
    CHECK(nr_new_params(&p) == NVSDK_NGX_Result_Success && p == &block_ngx);
    nr_set_int(p, "DLSSNR.Width", 1712);
    CHECK(nr_create_feature((void*)0x9, p, &h) == NVSDK_NGX_Result_Success && h == &handle_ngx);
    CHECK(nr_evaluate_feature((void*)0x9, h, p) == NVSDK_NGX_Result_Success);
    nr_release_feature(h);
    CHECK(ngx == "caps UI:DLSSNR.Width=1712 create18 evaluate release " && model.empty());
    // the FP16 file: the model's exports through the forwarder, and nothing of NGX's feature calls
    ngx.clear();
    st().nr_model = dlss_nr::Model::fp16;
    CHECK(nr_direct());
    NrDirect& d = nrd();
    d.forwarder = LoadLibraryA(argv[1]);
    CHECK(d.forwarder);
    d.create = reinterpret_cast<NrDirect::Create>(reinterpret_cast<void*>(GetProcAddress(d.forwarder, "longshot_nr_create")));
    d.evaluate = reinterpret_cast<NrDirect::Evaluate>(reinterpret_cast<void*>(GetProcAddress(d.forwarder, "longshot_nr_evaluate")));
    d.release = reinterpret_cast<NrDirect::Release>(reinterpret_cast<void*>(GetProcAddress(d.forwarder, "longshot_nr_release")));
    CHECK(d.create && d.evaluate && d.release);
    d.model_create = (void*)fake_create, d.model_evaluate = (void*)fake_evaluate, d.model_release = (void*)fake_release;
    CHECK(nr_new_params(&p) == NVSDK_NGX_Result_Success && p == &block_alloc);
    nr_set_int(p, "DLSSNR.Width", 1712);
    CHECK(nr_create_feature((void*)0x9, p, &h) == NVSDK_NGX_Result_Success && (void*)h == &model_handle);
    CHECK(nr_evaluate_feature((void*)0x9, h, p) == NVSDK_NGX_Result_Success);
    nr_release_feature(h);
    CHECK(ngx == "alloc I:DLSSNR.Width=1712 ");
    CHECK(model == "create18:alloc evaluate release ");
    // (a model that says it made the feature but gives no handle: a failure, not a null feature)
    model_handle = 0;
    d.model_create = (void*)+[](void*, int, void*, void** out) -> int { *out = nullptr; return 1; };
    h = &handle_ngx;
    CHECK(nr_create_feature((void*)0x9, p, &h) == NVSDK_NGX_Result_Fail && h == nullptr);
    std::puts("back ends: PASS");
}
'''


def compile_forwarder(cxx, out):
    """gpu/vk/dlss_nr_forwarder.c as a library of its own, as the builder's link_forwarder links it (-O2, C)"""
    src = ROOT / "gpu" / "vk" / "dlss_nr_forwarder.c"
    flags = ["-x", "c", "-O2", "-g0", "-Wall", "-Wextra", "-Werror", "-shared"]
    if sys.platform != "win32":
        flags += ["-fPIC"]
    subprocess.run([cxx] + flags + [str(src), "-o", str(out)], check=True)


def check_names():
    """The forwarder's file and exports, as every place that makes, ships, carries and loads it names them"""
    failed = []
    vk = (ROOT / "gpu" / "vk" / "vk_dlss.cpp").read_text(encoding="utf-8")
    forwarder = (ROOT / "gpu" / "vk" / "dlss_nr_forwarder.c").read_text(encoding="utf-8")
    crates = ROOT / "builder" / "crates"
    compile_lib = (crates / "build-compile" / "src" / "lib.rs").read_text(encoding="utf-8")
    plan = (crates / "build-compile" / "src" / "plan.rs").read_text(encoding="utf-8")
    driver_build = (crates / "build-driver" / "src" / "build.rs").read_text(encoding="utf-8")
    if "nvngx.dll" not in NAME:
        failed.append("the forwarder's name lacks nvngx.dll: the model refuses its calls")
    if 'kNrForwarder = L"%s"' % NAME not in vk:
        failed.append("vk_dlss.cpp does not load %s" % NAME)
    link = block(compile_lib, "fn link_forwarder(")
    if 'pub const NR_FORWARDER: &str = "%s";' % NAME not in compile_lib or "gpu/vk/dlss_nr_forwarder.c" not in link:
        failed.append("build-compile does not link %s from gpu/vk/dlss_nr_forwarder.c" % NAME)
    if "link_forwarder(r, tc, l, p, cc_id)" not in block(compile_lib, "fn link_renderer("):
        failed.append("build-compile's link_renderer does not link the forwarder after the renderer")
    if "!tc.windows || p.ngx.is_none()" not in link or '"/Brepro"' not in link:
        failed.append("the forwarder is linked other than on Windows with the NGX SDK, or with a time stamp in it")
    if "build_compile::NR_FORWARDER" not in block(driver_build, "fn finished_files("):
        failed.append("build-driver's finished_files does not put %s beside the game" % NAME)
    for name in EXPORTS:
        if not re.search(r"\bint NR_CALL %s\(" % name, forwarder):
            failed.append("dlss_nr_forwarder.c has no export %s" % name)
        if 'GetProcAddress(d.forwarder, "%s")' % name not in vk:
            failed.append("vk_dlss.cpp does not take %s from the forwarder" % name)
    if forwarder.count("volatile int result") != 4 or "disable_tail_calls" not in forwarder:
        failed.append("a forwarder call may compile to a jump: each result goes through a volatile, with disable_tail_calls")
    if 'sorted_files(&vk, |f| f.ends_with(".cpp") && f != NGX_MSVC_RT)' not in plan:
        failed.append("build-compile's plan may take dlss_nr_forwarder.c into longshot_vk.dll")
    # (the model's feature lives through the back ends only: no NGX feature call of its own where DLSS 5's is made, used
    # or let go)
    for where in ("bool neural_render(", "void drop_nr() {", "bool dlss_nr_chain(VkCommandBuffer cmd, TemporalFrame& f,"):
        body = block(vk, where, unique=False)
        for call in ("NVSDK_NGX_VULKAN_ReleaseFeature", "NVSDK_NGX_VULKAN_CreateFeature", "NVSDK_NGX_VULKAN_EvaluateFeature",
                     "NVSDK_NGX_VULKAN_GetCapabilityParameters", "NVSDK_NGX_VULKAN_AllocateParameters", "NVSDK_NGX_Parameter_SetUI"):
            if call in body:
                failed.append("%s calls %s itself, not through the back ends (the FP16 file's would go to NGX)" % (where, call))
    return failed


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cxx", default="g++")
    args = parser.parse_args()
    failed = check_names()
    with tempfile.TemporaryDirectory(prefix="nr-forwarder-") as tmp:
        lib = Path(tmp) / (NAME if sys.platform == "win32" else NAME + ".so")
        compile_forwarder(args.cxx, lib)
        calls_cpp, calls_exe = Path(tmp) / "calls.cpp", Path(tmp) / "calls.exe"
        calls_cpp.write_text(CALLS, encoding="utf-8")
        subprocess.run([args.cxx, "-std=c++20", "-O2", "-Wall", "-Wextra", "-Werror", str(calls_cpp), "-o", str(calls_exe)] +
                       ([] if sys.platform == "win32" else ["-ldl"]), check=True)
        out = subprocess.run([str(calls_exe), str(lib)], capture_output=True, encoding="utf-8")
        if out.returncode or "calls: PASS" not in out.stdout:
            failed.append("the forwarder's calls: " + out.stdout + out.stderr)
        if sys.platform == "win32":
            vk = (ROOT / "gpu" / "vk" / "vk_dlss.cpp").read_text(encoding="utf-8")
            direct = between(vk, "struct NrDirect {", "#else\nconstexpr bool nr_direct()")
            back_ends = between(vk, "/// A new parameter block:", "\nvoid drop_nr() {")
            cpp, exe = Path(tmp) / "back_ends.cpp", Path(tmp) / "back_ends.exe"
            cpp.write_text(BACK_ENDS.replace("@@DIRECT@@", direct).replace("@@BACK_ENDS@@", back_ends), encoding="utf-8")
            subprocess.run([args.cxx, "-std=c++20", "-O1", "-Wall", "-Wextra", "-Werror", "-Wno-unused-function", "-Wno-unused-const-variable", str(cpp), "-o",
                            str(exe)], check=True)
            out = subprocess.run([str(exe), str(lib)], capture_output=True, encoding="utf-8")
            if out.returncode or "back ends: PASS" not in out.stdout:
                failed.append("the back ends: " + out.stdout + out.stderr)
    if failed:
        raise SystemExit("FAIL:\n" + "\n".join(failed))
    print("PASS: the forwarder's four calls come from a module named for the model's check; its name agrees everywhere; "
          "each file's back end makes its own calls")


if __name__ == "__main__":
    main()
