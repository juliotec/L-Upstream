/* The game folder's path, for the files the game keeps there (image.bin, romfs, saves, settings, caches, logs).
   On Windows the game is bin\longshot.exe since the launcher came: longshot.exe in the folder is its launcher (the builder's
   setup, builder/setup/setup.c, which starts bin\longshot.exe), the game and its DLLs are in bin\, the game's files stay in the folder. hos_game_path_a/_w answer as
   GetModuleFileName(NULL, ...) does, but with the folder's path when the exe is in a "bin" folder beside image.bin, so code that
   strips the exe's name and adds a file's finds the folder's file. Modules (DLLs) are still loaded from beside the exe itself. */
#pragma once
#include <string.h>
#include <wchar.h>
#ifdef _WIN32
#ifndef NOMINMAX
#define NOMINMAX
#endif
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>

/* A path's bytes in the host's buffers (UTF-8): MAX_PATH is 260 UTF-16 units on Windows, which UTF-8 can take three bytes each
   for, and the manifest's longPathAware lets a path be longer still (Windows' own LongPathsEnabled allowing) */
#define HOS_PATH_CAP 4096

static inline DWORD hos_game_path_w(wchar_t *buf, DWORD size) {
    DWORD n = GetModuleFileNameW(NULL, buf, size);
    if (!n || n >= size) return n;
    wchar_t *name = buf + n;
    while (name > buf && name[-1] != L'\\' && name[-1] != L'/') name--;
    if (name - buf < 5) return n;
    wchar_t *dir = name - 1;
    wchar_t *bin = dir;
    while (bin > buf && bin[-1] != L'\\' && bin[-1] != L'/') bin--;
    if (dir - bin != 3 || _wcsnicmp(bin, L"bin", 3) != 0) return n;
    wchar_t probe[HOS_PATH_CAP + 16];
    const size_t root = (size_t)(bin - buf);
    if (root + 10 >= sizeof probe / sizeof probe[0]) return n;
    wmemcpy(probe, buf, root);
    wcscpy(probe + root, L"image.bin");
    if (GetFileAttributesW(probe) == INVALID_FILE_ATTRIBUTES) return n;
    wmemmove(buf + root, name, wcslen(name) + 1);
    return (DWORD)wcslen(buf);
}

/* hos_game_path_w's path as UTF-8 (the code page the manifest gives the A functions), whatever the system's: GetModuleFileNameA
   gave the ANSI code page's bytes before the manifest, and at most MAX_PATH of them. size or more when it does not fit. Where
   the manifest's code page does not apply (before Windows 10 1903: 1809 LTSC) UTF-8 and the ANSI code page agree only on a
   plain ASCII path: host/hos_main.c's narrow_paths_ok refuses any other at the start */
static inline DWORD hos_game_path_a(char *buf, DWORD size) {
    wchar_t w[HOS_PATH_CAP];
    const DWORD wn = hos_game_path_w(w, HOS_PATH_CAP);
    if (!wn || wn >= HOS_PATH_CAP) return wn ? size : 0;
    const int n = WideCharToMultiByte(CP_UTF8, 0, w, (int)wn + 1, buf, (int)size, NULL, NULL);
    return n > 0 ? (DWORD)n - 1 : size;
}
#else
#include <unistd.h>
#include <windows.h>   /* (linux: host/posix's, for DWORD and GetModuleFileNameA) */
#define HOS_PATH_CAP MAX_PATH   /* (host/posix's: 4096) */
/* (Linux) the same bin/ layout when the folder was built by the packaged builder: longshot in the folder is the
   launcher script, and the game is bin/longshot. A development build has no bin/ and this is GetModuleFileName. */
static inline DWORD hos_game_path_a(char *buf, DWORD size) {
    DWORD n = GetModuleFileNameA(NULL, buf, size);
    if (!n || n >= size) return n;
    char *name = buf + n;
    while (name > buf && name[-1] != '/') name--;
    if (name - buf < 5) return n;
    char *dir = name - 1;
    char *bin = dir;
    while (bin > buf && bin[-1] != '/') bin--;
    if (dir - bin != 3 || memcmp(bin, "bin", 3) != 0) return n;
    char probe[MAX_PATH + 16];
    const size_t root = (size_t)(bin - buf);
    if (root + 10 >= sizeof probe) return n;
    memcpy(probe, buf, root);
    memcpy(probe + root, "image.bin", 10);
    if (access(probe, F_OK) != 0) return n;
    memmove(buf + root, name, strlen(name) + 1);
    return (DWORD)strlen(buf);
}
#endif

/* (longshot) The window's title: the game's own name, "The Legend of Zelda: Ocarina of Time (Longshot)", the first line of
   longshot_title.txt in the game folder (UTF-8), which the builder writes from the game's .nup (builder/crates/game-files/src/title.rs); fallback
   when the folder has none (a build from an unpacked dump, a development build). Read once */
#include <stdio.h>
static inline const char *hos_game_title(const char *fallback) {
    static char title[256];
    static int done;
    if (!done) {
        done = 1;
        FILE *f = NULL;
#ifdef _WIN32
        wchar_t path[HOS_PATH_CAP + 32];
        DWORD n = hos_game_path_w(path, HOS_PATH_CAP);
        if (n && n < HOS_PATH_CAP) {
            while (n && path[n - 1] != L'\\' && path[n - 1] != L'/') n--;
            wcscpy(path + n, L"longshot_title.txt");
            f = _wfopen(path, L"rb");
        }
#else
        char path[HOS_PATH_CAP + 32];
        DWORD n = hos_game_path_a(path, HOS_PATH_CAP);
        if (n && n < HOS_PATH_CAP) {
            while (n && path[n - 1] != '/') n--;
            strcpy(path + n, "longshot_title.txt");
            f = fopen(path, "rb");
        }
#endif
        if (f) {
            if (!fgets(title, sizeof title, f)) title[0] = 0;
            fclose(f);
            size_t len = strcspn(title, "\r\n");
            title[len] = 0;
        }
    }
    return title[0] ? title : fallback;
}
