/* (longshot) The game's side of a player's profile (docs/dev/reference/profiles.md): the folder LONGSHOT_PROFILE names,
   which longshot-builder's play gives the game it starts (builder/crates/build-driver/src/play.rs; its reader and
   writer of profile.json is builder/crates/launcher-profile):
     <profile>/profile.json   which profile it is; the game reads its start_slot alone (longshot_profile_start_slot)
     <profile>/settings.ini   the game's settings (longshot_options.ini's lines: host/options_path.c)
     <profile>/saves/         the saves alone, save/<uid>/ and save-work/ (host/save_dir.h, host/nn_fs.c)
   These three come from the profile alone: no switch and no command line names another place (a developer's or a
   test's run plays a profile of its own, scripts/longshot_bridge.py). Everything else of the game's stays where it is
   without a profile: the user folder (the recordings, the mouse's sensitivity, the game's cache storage; --user names
   another), the game folder's logs/ and cache/ (host/game_layout.c).
   Plain C, no system call: tests/test_longshot_exit.py builds it alone. The Mod API's sandbox takes the same paths from
   here. */
#pragma once

#include <stddef.h>
#include <string.h>

/* The variable the builder gives the game its profile in (absolute): launcher_profile::SWITCH */
#define LONGSHOT_PROFILE "LONGSHOT_PROFILE"
/* The variable a dev run (longshot-builder mod run) gives the game extra mod folders in: the OS's path-separator-joined
   list (build_driver::play::DEV_MODS_VAR). The Mod API's game side links each folder's mod in beside the profile's. */
#define LONGSHOT_DEV_MODS "LONGSHOT_DEV_MODS"
#define LONGSHOT_PROFILE_FILE "profile.json"
#define LONGSHOT_PROFILE_SETTINGS "settings.ini"
#define LONGSHOT_PROFILE_SAVES "saves"
/* The most bytes of a profile.json read (launcher_profile::FILE_MAX) */
#define LONGSHOT_PROFILE_FILE_MAX (1u << 20)

/* <profile><sep><name> into out (the profile's trailing separators left out): 0 when there is no profile, or it does
   not fit */
static inline int longshot_profile_path(const char *profile, char sep, const char *name, char *out, size_t cap) {
    if (!profile || !profile[0]) return 0;
    size_t n = strlen(profile);
    while (n > 1 && (profile[n - 1] == '/' || profile[n - 1] == '\\')) n--;
    const size_t k = strlen(name);
    if (n + 1 + k + 1 > cap) return 0;
    memcpy(out, profile, n);
    out[n] = sep;
    memcpy(out + n + 1, name, k + 1);
    return 1;
}
/* The profile's saves/ (getenv(LONGSHOT_PROFILE) as profile) */
static inline int longshot_profile_saves(const char *profile, char sep, char *out, size_t cap) {
    return longshot_profile_path(profile, sep, LONGSHOT_PROFILE_SAVES, out, cap);
}
/* The profile's settings.ini */
static inline int longshot_profile_settings(const char *profile, char sep, char *out, size_t cap) {
    return longshot_profile_path(profile, sep, LONGSHOT_PROFILE_SETTINGS, out, cap);
}
/* The profile's profile.json */
static inline int longshot_profile_file(const char *profile, char sep, char *out, size_t cap) {
    return longshot_profile_path(profile, sep, LONGSHOT_PROFILE_FILE, out, cap);
}

/* ---- profile.json's start_slot, by launcher_profile's rule (Profile::start_slot): read without a JSON library */

static inline const char *lp_ws(const char *p, const char *e) {
    while (p < e && (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r')) p++;
    return p;
}
/* The string at p (its opening quote): past its closing quote, NULL when it is none. Its text (escapes decoded, but
   \u's: kept as written, which no slot or key of ours has) into out when given, cut at cap */
static inline const char *lp_string(const char *p, const char *e, char *out, size_t cap) {
    if (p >= e || *p != '"') return NULL;
    size_t n = 0;
    for (p++; p < e; p++) {
        char c = *p;
        if (c == '"') {
            if (out && cap) out[n < cap ? n : cap - 1] = 0;
            return p + 1;
        }
        if ((unsigned char)c < 0x20) return NULL;
        if (c == '\\') {
            if (++p >= e) return NULL;
            switch (*p) {
            case '"': case '\\': case '/': c = *p; break;
            case 'b': c = '\b'; break;
            case 'f': c = '\f'; break;
            case 'n': c = '\n'; break;
            case 'r': c = '\r'; break;
            case 't': c = '\t'; break;
            case 'u':
                for (int i = 1; i <= 4; i++) {
                    const char h = p + i < e ? p[i] : 0;
                    if (!((h >= '0' && h <= '9') || (h >= 'a' && h <= 'f') || (h >= 'A' && h <= 'F'))) return NULL;
                }
                c = '?';   /* (a character no slot name has) */
                p += 4;
                break;
            default: return NULL;
            }
        }
        if (out && n + 1 < cap) out[n] = c;
        n++;
    }
    return NULL;
}
/* The value at p: past it, NULL when it is none (arrays and objects at most 64 deep) */
static inline const char *lp_value(const char *p, const char *e, int depth) {
    p = lp_ws(p, e);
    if (p >= e) return NULL;
    if (*p == '"') return lp_string(p, e, NULL, 0);
    if (*p == '{' || *p == '[') {
        const char close = *p == '{' ? '}' : ']';
        if (depth >= 64) return NULL;
        p = lp_ws(p + 1, e);
        if (p < e && *p == close) return p + 1;
        for (;;) {
            if (close == '}') {
                p = lp_string(lp_ws(p, e), e, NULL, 0);
                if (!p || (p = lp_ws(p, e)) >= e || *p != ':') return NULL;
                p++;
            }
            if (!(p = lp_value(p, e, depth + 1)) || (p = lp_ws(p, e)) >= e) return NULL;
            if (*p == close) return p + 1;
            if (*p != ',') return NULL;
            p++;
        }
    }
    static const char *const words[] = {"true", "false", "null"};
    for (int i = 0; i < 3; i++) {
        const size_t k = strlen(words[i]);
        if ((size_t)(e - p) >= k && !memcmp(p, words[i], k)) return p + k;
    }
    const char *s = p;
    if (p < e && *p == '-') p++;
    while (p < e && ((*p >= '0' && *p <= '9') || *p == '.' || *p == 'e' || *p == 'E' || *p == '+' || *p == '-')) p++;
    return p > s && (s[0] != '-' || p > s + 1) ? p : NULL;
}
/* A slot's text (a string's, trimmed, its "slot_" in any case left out; or a number's as written): 0-99 into *slot, 1;
   0 when it is no slot */
static inline int lp_slot_digits(const char *t, size_t n, int *slot) {
    while (n && (*t == ' ' || *t == '\t' || *t == '\n' || *t == '\r')) t++, n--;
    while (n && (t[n - 1] == ' ' || t[n - 1] == '\t' || t[n - 1] == '\n' || t[n - 1] == '\r')) n--;
    if (n >= 5 && (t[0] | 32) == 's' && (t[1] | 32) == 'l' && (t[2] | 32) == 'o' && (t[3] | 32) == 't' && t[4] == '_')
        t += 5, n -= 5;
    if (!n || n > 9) return 0;
    int v = 0;
    for (size_t i = 0; i < n; i++) {
        if (t[i] < '0' || t[i] > '9') return 0;
        v = v * 10 + (t[i] - '0');
    }
    if (v > 99) return 0;
    *slot = v;
    return 1;
}
/* profile.json's text (n bytes; a byte order mark before it allowed): its start_slot, by launcher_profile's rule.
   1: the slot in *slot (0-99); 0: none (no key, or null); -1: a value that is no slot ("slot_00" to "slot_99", "N" or
   N, 0-99); -2: the text is no profile it reads (not one object, a key twice, "v" not 1), so no slot taken. why (when
   given) says which, for the log */
static inline int longshot_profile_start_slot(const char *text, size_t n, int *slot, const char **why) {
    const char *p = text, *e = text + n, *at = NULL, *at_end = NULL;
    const char *said = NULL;
    if (!why) why = &said;
    if (n >= 3 && (unsigned char)p[0] == 0xEF && (unsigned char)p[1] == 0xBB && (unsigned char)p[2] == 0xBF) p += 3;
    p = lp_ws(p, e);
    if (p >= e || *p != '{') return *why = "it is not a JSON object", -2;
    /* (the keys seen, by where each starts, to find one twice: a profile has a few) */
    const char *keys[64];
    size_t key_len[64], count = 0;
    p = lp_ws(p + 1, e);
    if (p < e && *p == '}') p++;
    else
        for (;;) {
            p = lp_ws(p, e);
            char key[32];
            const char *k = p;
            if (!(p = lp_string(p, e, key, sizeof key))) return *why = "it is not a JSON object", -2;
            const size_t kl = (size_t)(p - k);
            for (size_t i = 0; i < count; i++)
                if (key_len[i] == kl && !memcmp(keys[i], k, kl)) return *why = "a key twice", -2;
            if (count < 64) keys[count] = k, key_len[count++] = kl;
            if ((p = lp_ws(p, e)) >= e || *p != ':') return *why = "it is not a JSON object", -2;
            const char *v = lp_ws(p + 1, e);
            if (!(p = lp_value(v, e, 1))) return *why = "it is not a JSON object", -2;
            if (kl == 3 && !memcmp(k, "\"v\"", 3) && !(p - v == 1 && *v == '1')) return *why = "\"v\" is not 1", -2;
            if (kl == 12 && !memcmp(k, "\"start_slot\"", 12)) at = v, at_end = p;
            if ((p = lp_ws(p, e)) >= e) return *why = "it is not a JSON object", -2;
            if (*p == '}') {
                p++;
                break;
            }
            if (*p != ',') return *why = "it is not a JSON object", -2;
            p++;
        }
    if (lp_ws(p, e) != e) return *why = "it is not a JSON object", -2;
    if (!at || (at_end - at == 4 && !memcmp(at, "null", 4))) return 0;
    char t[16];
    if (*at == '"') {
        const size_t len = (size_t)(at_end - at) - 2;
        if (len >= sizeof t || !lp_string(at, at_end, t, sizeof t)) return *why = "it is no slot (slot_00 to slot_99)", -1;
        return lp_slot_digits(t, strlen(t), slot) ? 1 : (*why = "it is no slot (slot_00 to slot_99)", -1);
    }
    if ((*at >= '0' && *at <= '9') && lp_slot_digits(at, (size_t)(at_end - at), slot)) return 1;
    return *why = "it is no slot (slot_00 to slot_99)", -1;
}
