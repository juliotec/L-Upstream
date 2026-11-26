/* The player's settings, longshot_options.ini's lines (options_path.h). Where they are:
   - the profile's settings.ini (LONGSHOT_PROFILE, which longshot-builder's play sets: host/longshot_profile.h), the file
     every build the profile plays reads, so an update, a new build or another channel keeps the Insert menu's choices
     and the controls. The profile is the one place: no switch and no command line names another file (a developer's or a
     test's run plays a throwaway profile of its own, scripts/longshot_bridge.py);
   - without a profile (a developer's game folder, a test), beside the game, as before.
   Chosen with no side effect, before anything reads it. The build's own choice stays in the game folder's file: compile_shaders=,
   which longshot-builder writes there (gpu/vk/vk_draw.cpp compile_all). A game folder's file from before is copied once into the
   profile when that has none (hos_options_carry), whole or not at all (host/hos_atomic_file.h): a kill in the copy leaves no
   file there, and the next start copies again. */
#include "hos_paths.h"   /* (first: windows.h, host/posix's on Linux) */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "options_path.h"
#include "hos_atomic_file.h"
#include "longshot_profile.h"   /* (LONGSHOT_PROFILE: the profile's settings.ini) */

#ifdef _WIN32
#define SEP '\\'
#define SEP_S "\\"
#else
#define SEP '/'
#define SEP_S "/"
#endif

void hos_log(const char *fmt, ...);   /* (hos.h; declared here so a test builds this file alone) */

static const char options_name[] = "longshot_options.ini";
static char file[HOS_PATH_CAP];
static int chosen;

static int is_sep(char c) { return c == '/' || c == '\\'; }
static int absolute(const char *p) {
#ifdef _WIN32
    return is_sep(p[0]) || (p[0] && p[1] == ':');
#else
    return p[0] == '/';
#endif
}
/* the game folder, with its separator: 0 when not known */
static size_t game_folder(char *out, size_t cap) {
    DWORD n = hos_game_path_a(out, (DWORD)cap);
    if (!n || n >= cap) return 0;
    while (n && !is_sep(out[n - 1])) n--;
    out[n] = 0;
    return n;
}
/* folder/name (name NULL: folder itself) into file, made whole against base (with its separator; "" the working folder); file
   "" when too long */
static void put(const char *base, const char *folder, const char *name) {
    char whole[HOS_PATH_CAP];
    file[0] = 0;
    if (!absolute(folder)) {
        char cwd[HOS_PATH_CAP];
        if (!base[0]) {
            const DWORD n = GetCurrentDirectoryA(HOS_PATH_CAP, cwd);
            if (!n || n >= HOS_PATH_CAP - 1) return;
            if (!is_sep(cwd[n - 1])) cwd[n] = SEP, cwd[n + 1] = 0;
            base = cwd;
        }
        if (snprintf(whole, sizeof whole, "%s%s", base, folder) >= (int)sizeof whole) return;
        folder = whole;
    }
    const size_t n = strlen(folder);
    if (snprintf(file, sizeof file, "%s%s%s", folder, !name || (n && is_sep(folder[n - 1])) ? "" : SEP_S, name ? name : "") >=
        (int)sizeof file)
        file[0] = 0;
}

void hos_options_choose(void) {
    if (chosen) return;
    chosen = 1;
    file[0] = 0;
    char settings[HOS_PATH_CAP], game[HOS_PATH_CAP];
    if (longshot_profile_settings(getenv(LONGSHOT_PROFILE), SEP, settings, sizeof settings)) {
        put("", settings, NULL);   /* (absolute: the builder gives it so) */
        return;
    }
    if (game_folder(game, sizeof game)) put(game, options_name, NULL);   /* (beside the game, as before) */
}

const char *hos_options_file(void) { return file; }

/* Whether the file at path has a setting of the player's: a key=value line other than compile_shaders= (longshot-builder writes
   that one into a new folder before the game has started) */
static int has_settings(const char *path) {
    FILE *f = fopen(path, "r");
    if (!f) return 0;
    char line[256];
    int found = 0;
    while (!found && fgets(line, sizeof line, f)) {
        const char *eq = strchr(line, '=');
        found = eq && eq != line && strncmp(line, "compile_shaders=", 16);
    }
    fclose(f);
    return found;
}

void hos_options_carry(void) {
    static int done;
    if (done) return;
    done = 1;
    /* (where it is, for a report: longshot-builder share reads it from this line) */
    hos_log("options: the settings file is %s", file[0] ? file : "none (every setting at its default, nothing saved)");
    if (!file[0] || GetFileAttributesA(file) != INVALID_FILE_ATTRIBUTES) return;
    char from[HOS_PATH_CAP];
    const size_t g = game_folder(from, sizeof from);
    if (!g || snprintf(from + g, sizeof from - g, "%s", options_name) >= (int)(sizeof from - g)) return;
#ifdef _WIN32
    if (!_stricmp(from, file)) return;   /* (beside the game: the file is already where it is read) */
#else
    if (!strcmp(from, file)) return;
#endif
    if (!has_settings(from)) return;
    char folder[HOS_PATH_CAP];
    snprintf(folder, sizeof folder, "%s", file);
    char *slash = folder + strlen(folder);
    while (slash > folder && !is_sep(slash[-1])) slash--;
    if (slash > folder) slash[-1] = 0, CreateDirectoryA(folder, NULL);   /* (the builder made the profile: in case) */
    if (hos_atomic_copy_file(from, file)) {
        hos_log("options: the settings in %s were copied to %s, where every build the profile plays reads them now (the game "
                "folder's copy is no longer read, but for compile_shaders=)", from, file);
        return;
    }
    /* (a profile that cannot be written: this run reads and saves the game folder's, as before, and the next one tries again) */
    hos_log("options: %s could not be copied to %s (error %lu): this run keeps the game folder's", from, file,
            (unsigned long)GetLastError());
    snprintf(file, sizeof file, "%s", from);
}
