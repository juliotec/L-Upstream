/* (longshot) The run's save folder: where it keeps its saves (save/<uid>/ and save-work/), in place of the user folder.
   It is the profile's saves/ (LONGSHOT_PROFILE, which longshot-builder's play sets: host/longshot_profile.h), and only
   that: no switch and no command line names another (a developer's or a test's run plays a throwaway profile of its
   own, scripts/longshot_bridge.py). Everything else of the user folder (the recordings, the mouse's sensitivity, the
   game's cache storage) stays there.

   No profile: the user folder, as before. A profile: its saves/ made when it is not there (and its save/ and save-work/),
   and written to once to be sure the game may; a profile that is not a whole path (on Windows a drive's or a share's,
   \\server\share\...), or whose saves/ cannot be made or written, is refused, and the game does not start
   (hos_main.c's game_main refuses the start, save_dir_unusable): it never falls back to the user folder's saves, which
   may be another profile's.

   The choice's rules are here (header-only, for the tests); the run's choice is kept by host/nn_fs.c, chosen once at
   the start, which keeps its saves under hos_saves_root(); the control bridge's save paths come from there
   (hos_fs_save_root), and so does the skip title's newest save (host/oot_autoload.c). Only the real environment sets
   the profile: longshot_env.txt cannot (hos_main.c env_line_allowed), as a run's saves are the builder's choice.
   Docs: docs/dev/reference/profiles.md. */
#pragma once
#include <stddef.h>
#include <stdio.h>
#include <string.h>
#include <windows.h>
#include "hos_paths.h"

#define SAVE_DIR_MAX HOS_PATH_CAP   /* (a path as long as the host's other paths: host/hos_paths.h) */
#ifdef _WIN32
#define SAVE_DIR_SEP "\\"
#else
#define SAVE_DIR_SEP "/"
#endif

/* (Windows: the length of a share's root, \\server\share, when p is a path on one ('/' or '\' between its parts; not the
   device paths \\?\ and \\.\); 0 when it is not) */
static inline size_t hos_save_dir_share_root(const char *p) {
    const int sep0 = p && (p[0] == '\\' || p[0] == '/'), sep1 = sep0 && (p[1] == '\\' || p[1] == '/');
    if (!sep1 || ((p[2] == '?' || p[2] == '.') && (p[3] == '\\' || p[3] == '/'))) return 0;
    size_t i = 2, server = 0, share = 0;
    while (p[i] && p[i] != '\\' && p[i] != '/') i++, server++;
    if (!server || !p[i]) return 0;
    i++;
    while (p[i] && p[i] != '\\' && p[i] != '/') i++, share++;
    return share ? i : 0;
}
/* (whether p is a whole path, from the root; on Windows a drive's, or a share's: \\server\share\...) */
static inline int hos_save_dir_absolute(const char *p) {
    if (!p || !*p) return 0;
#ifdef _WIN32
    return (((p[0] >= 'A' && p[0] <= 'Z') || (p[0] >= 'a' && p[0] <= 'z')) && p[1] == ':' && (p[2] == '\\' || p[2] == '/')) ||
           hos_save_dir_share_root(p) != 0;
#else
    return p[0] == '/';
#endif
}

/* (mkdir -p, every parent then the folder itself: 1 when it is a folder at the end) */
static inline int hos_save_dir_make(const char *path) {
    char tmp[SAVE_DIR_MAX];
    if ((size_t)snprintf(tmp, sizeof tmp, "%s", path) >= sizeof tmp) return 0;
#ifdef _WIN32
    const size_t share = hos_save_dir_share_root(tmp);   /* (a share's root is there or not: never made) */
#else
    const size_t share = 0;
#endif
    for (char *p = tmp + (share ? share : 1); *p; p++) {
        if (*p != '/' && *p != '\\') continue;
        if (p > tmp && p[-1] == ':') continue;   /* (a drive's root) */
        const char c = *p;
        *p = 0;
        CreateDirectoryA(tmp, NULL);
        *p = c;
    }
    CreateDirectoryA(tmp, NULL);
    const DWORD at = GetFileAttributesA(tmp);
    return at != INVALID_FILE_ATTRIBUTES && (at & FILE_ATTRIBUTE_DIRECTORY);
}

/* A save folder's value (a profile's saves/) made the run's save folder: its trailing separators off, the folder and its
   save/ and save-work/ made, a file written in save/ and taken away again. 1 with the folder in out ("" for unset or
   empty: the user folder's), else 0 with why (a sentence for the player, the folder named after label: "the profile's
   saves folder ") */
static inline int hos_save_dir_check_from(const char *value, const char *label, char *out, size_t cap, char *why,
                                          size_t why_cap) {
    if (!value || !*value) {
        if (cap) out[0] = 0;
        return 1;
    }
    size_t n = strlen(value);
    while (n > 1 && (value[n - 1] == '/' || value[n - 1] == '\\') && !(n == 3 && value[1] == ':')) n--;
    if (!hos_save_dir_absolute(value)) {
        snprintf(why, why_cap, "%s%s is not a whole path (a folder from the root, as a launcher gives it)", label, value);
        return 0;
    }
    if (n + 32 >= cap) {
        snprintf(why, why_cap, "%s%s is too long a path", label, value);
        return 0;
    }
    snprintf(out, cap, "%.*s", (int)n, value);
    char sub[SAVE_DIR_MAX];
    const char *const subs[] = {"save", "save-work"};
    if (!hos_save_dir_make(out)) {
        snprintf(why, why_cap, "%s%s: the folder could not be made", label, out);
        return 0;
    }
    for (int i = 0; i < 2; i++) {
        snprintf(sub, sizeof sub, "%s" SAVE_DIR_SEP "%s", out, subs[i]);
        if (!hos_save_dir_make(sub)) {
            snprintf(why, why_cap, "%s%s: its %s folder could not be made", label, out, subs[i]);
            return 0;
        }
    }
    /* (written to once: a folder the game may read but not write would lose every save) */
    char probe[SAVE_DIR_MAX];
    snprintf(probe, sizeof probe, "%s" SAVE_DIR_SEP "save" SAVE_DIR_SEP ".longshot-write-test", out);
    HANDLE h = CreateFileA(probe, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    DWORD put = 0;
    const int wrote = h != INVALID_HANDLE_VALUE && WriteFile(h, "ok", 2, &put, NULL) && put == 2;
    if (h != INVALID_HANDLE_VALUE) CloseHandle(h);
    DeleteFileA(probe);
    if (!wrote) {
        snprintf(why, why_cap, "%s%s: the game cannot write there", label, out);
        return 0;
    }
    return 1;
}
/* ---- the run's choice (host/nn_fs.c) ---- */
/* Chosen once: the profile's saves/ (LONGSHOT_PROFILE: host/longshot_profile.h), else none (the user folder's). 1
   usable, 0 refused (hos_save_dir_refused says why) */
int hos_save_dir_choose(void);
/* Why the save folder was refused, NULL when it was not */
const char *hos_save_dir_refused(void);
/* The save folder chosen for this run (the profile's), NULL when none or refused */
const char *hos_save_dir_chosen(void);
/* Which chose it: "LONGSHOT_PROFILE" (NULL when none did) */
const char *hos_save_dir_source(void);
/* The folder the saves (save/<uid>/, save-work/) are kept in: the one chosen, else the user folder */
const char *hos_saves_root(void);
