// The game folder's path for the files the game keeps there (the shader list, settings): host/hos_paths.h's rule, for this
// DLL. The game is bin\longshot.exe beside its launcher since the launcher came; its files stay in the folder above bin.
// game_module_path answers as GetModuleFileNameA(nullptr, ...) does, with the folder's path when the exe is in a "bin" folder
// beside image.bin. What the game makes as it runs goes in two folders: the shader cache folder (cache_path: the shader and
// pipeline caches, made for this translator and this GPU and driver, which can be deleted), and the game folder's logs/
// (log_path: the logs and traces). The shader cache folder is LONGSHOT_SHADER_CACHE when longshot-builder started the game
// (its play and its preparation: <the builder's cache>/shaders/<translator stamp>/<driver>/, one for every build of that
// translator on this driver), else the game folder's cache/. The host makes cache/ and logs/ at its start and moves a build's
// from before into them (game/game_layout.c).
#pragma once
#ifndef NOMINMAX
#define NOMINMAX
#endif
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#undef min
#undef max
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <string>

namespace mkwvk {
/// A path's bytes in the renderer's buffers: host/hos_paths.h's HOS_PATH_CAP (UTF-8 takes up to three bytes for each of
/// MAX_PATH's UTF-16 units, and the manifest's longPathAware lets a path be longer still): never MAX_PATH, which cut the
/// host's longer game folder paths short here
constexpr DWORD kPathCap = 4096;
inline DWORD game_module_path(char* buf, DWORD size) {
    DWORD n = GetModuleFileNameA(nullptr, buf, size);
    if (!n || n >= size) return n;
    char* name = buf + n;
    while (name > buf && name[-1] != '\\' && name[-1] != '/') name--;
    if (name - buf < 5) return n;
    char* dir = name - 1;
    char* bin = dir;
    while (bin > buf && bin[-1] != '\\' && bin[-1] != '/') bin--;
    if (dir - bin != 3 || _strnicmp(bin, "bin", 3) != 0) return n;
    const std::string probe = std::string(buf, (size_t)(bin - buf)) + "image.bin";
    const size_t root = (size_t)(bin - buf);
    if (GetFileAttributesA(probe.c_str()) == INVALID_FILE_ATTRIBUTES) return n;
    std::memmove(buf + root, name, std::strlen(name) + 1);
    return (DWORD)std::strlen(buf);
}
/// game_module_path's path as a string, "" when it can't be told or does not fit kPathCap
inline std::string game_module_string() {
    std::string exe(kPathCap, '\0');
    const DWORD n = game_module_path(exe.data(), kPathCap);
    exe.resize(n && n < kPathCap ? n : 0);
    return exe;
}
/// The executable's own path (GetModuleFileNameA: bin\ beside the launcher), "" when it can't be told or does not fit
inline std::string exe_module_string() {
    std::string exe(kPathCap, '\0');
    const DWORD n = GetModuleFileNameA(nullptr, exe.data(), kPathCap);
    exe.resize(n && n < kPathCap ? n : 0);
    return exe;
}
/// The game folder, with its separator at the end
inline std::string game_folder_path() {
    const std::string path = game_module_string();
    return path.substr(0, path.find_last_of("\\/") + 1);
}
/// <game folder>/<sub>/<name>, the folder made when it is not there (the host makes it at its start; a tool may not have)
inline std::string game_sub_path(const char* sub, const char* name) {
    const std::string dir = game_folder_path() + sub;
    if (GetFileAttributesA(dir.c_str()) == INVALID_FILE_ATTRIBUTES) CreateDirectoryA(dir.c_str(), nullptr);
    return dir + "/" + name;
}
/// The shader cache folder longshot-builder named (LONGSHOT_SHADER_CACHE: absolute, made when it is not there), with its
/// separator at the end; "" when none is named, it is not absolute or it can't be made: the game folder's cache/ then. Read
/// once: every file of the caches is in one folder for the whole run
inline const std::string& shader_cache_dir() {
    static const std::string dir = [] {
        const char* v = std::getenv("LONGSHOT_SHADER_CACHE");
        if (!v || !*v) return std::string();
        const std::filesystem::path p(v);
        std::error_code ec;
        if (!p.is_absolute() || (!std::filesystem::create_directories(p, ec) && !std::filesystem::is_directory(p, ec)))
            return std::string();
        std::string d = v;
        if (d.back() != '/' && d.back() != '\\') d += '/';
        return d;
    }();
    return dir;
}
/// <shader cache folder>/<name>: the shader and pipeline caches ("" for the folder itself, with its separator): the folder
/// LONGSHOT_SHADER_CACHE names (shader_cache_dir), else <game folder>/cache/
inline std::string cache_path(const char* name) {
    const std::string& shared = shader_cache_dir();
    return shared.empty() ? game_sub_path("cache", name) : shared + name;
}
/// <game folder>/logs/<name>: the logs and traces a run writes
inline std::string log_path(const char* name) { return game_sub_path("logs", name); }
/// The player's settings, longshot_options.ini: the host's (MkwVkHost.options_path, host/options_path.c: in the user folder
/// --user names, else beside the game), "" when it has none (no file: every row at its default). Without the host (a tool), the
/// game folder's. vk_main.cpp. The build's compile_shaders= stays in the game folder's file (vk_draw.cpp compile_all)
std::string options_path();
/// The game folder as a path, game_module_path's rule. On Windows from GetModuleFileNameW: a folder with characters outside
/// the ANSI code page, or longer than kPathCap, is still the right one (open it with std::filesystem or the streams)
inline std::filesystem::path game_dir_path() {
#ifdef _WIN32
    std::wstring exe(32768, L'\0');
    const DWORD n = GetModuleFileNameW(nullptr, exe.data(), (DWORD)exe.size());
    if (!n || n >= exe.size()) return {};
    exe.resize(n);
    std::filesystem::path dir = std::filesystem::path(exe).parent_path();
    if (!_wcsicmp(dir.filename().c_str(), L"bin") &&
        GetFileAttributesW((dir.parent_path() / L"image.bin").c_str()) != INVALID_FILE_ATTRIBUTES)
        dir = dir.parent_path();
    return dir;
#else
    return std::filesystem::path(game_module_string()).parent_path();
#endif
}
}  // namespace mkwvk
