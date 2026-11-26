/* (longshot) The pieces of heart Link has found, for the Discord status ({heart_pieces}: game/discord_activity.c).

   The game keeps the pieces Link holds, 0 to 3 (four make a heart: game/oot_tools.c counts them in his pouch), and nothing
   that counts the ones he found. The total comes from his hearts instead: he starts with 3, each boss heart container he
   takes gives one and every other heart was four pieces, so

       pieces found = 4 * (hearts - 3 - containers taken) + pieces held

   (exact, and it counts every piece, some of which the map's own heart icons do not mark). Each of the eight dungeons with
   a boss places one container, an Obj_Heart_Container actor in its map file
   (romfs Banc/<dungeon>/<dungeon>_Static.bcett.byml.zs), and the game data keeps whether Link took it in a Bool64bitKey
   flag named by the actor's 64-bit placement id (game/oot_state_tap.c reads it). The ids are read from the player's own
   RomFS at the first use, not written here: the map files are zstd frames made with the dictionary bcett.byml.zsdic of
   Pack/ZsDic.pack.zs (host/zs.c finds it by the frame's dictionary id), a BYML inside. Checked on a save at the Water Temple: five containers taken (Deku Tree to Fire), 9 hearts and 1 piece held,
   so 5 found, the pieces its map marks plus one it does not. */
#include "oot_heart_containers.h"
#include "game_host.h"   /* (what the host gives the game: host/game_host.h) */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ---- the files: SARC and BYML, read with bounds (anything short or odd is "not found") */

static uint32_t rd(const uint8_t *d, size_t n, size_t o, int bytes, int be) {
    if (o > n || bytes > 4 || n - o < (size_t)bytes) return 0;
    uint32_t v = 0;
    for (int i = 0; i < bytes; i++) v |= (uint32_t)d[o + (be ? bytes - 1 - i : i)] << (8 * i);
    return v;
}
static uint64_t rd64(const uint8_t *d, size_t n, size_t o, int be) {
    const uint64_t a = rd(d, n, o, 4, be), b = rd(d, n, o + 4, 4, be);
    return be ? a << 32 | b : b << 32 | a;
}

typedef struct {
    const uint8_t *d;
    size_t n;
    int be;
    size_t keys, strs;   /* (the key and string tables) */
} Byml;

/* the table's entry i is text (a C string): 1 or 0 */
static int byml_table_is(const Byml *b, size_t table, uint32_t i, const char *text) {
    if (!table || table >= b->n || b->d[table] != 0xc2 || i >= rd(b->d, b->n, table + 1, 3, b->be)) return 0;
    const size_t at = table + rd(b->d, b->n, table + 4 + 4 * (size_t)i, 4, b->be), len = strlen(text);
    return at < b->n && b->n - at > len && !memcmp(b->d + at, text, len) && !b->d[at + len];
}

/* the value of key in the dictionary at off: its type, and the value word's place at *word; 0 when it has none */
static int byml_get(const Byml *b, size_t off, const char *key, size_t *word) {
    if (off >= b->n || b->d[off] != 0xc1) return 0;
    const uint32_t count = rd(b->d, b->n, off + 1, 3, b->be);
    for (uint32_t i = 0; i < count; i++) {
        const size_t e = off + 4 + 8 * (size_t)i;
        if (e + 8 > b->n) return 0;
        if (byml_table_is(b, b->keys, rd(b->d, b->n, e, 3, b->be), key)) {
            *word = e + 4;
            return b->d[e + 3];
        }
    }
    return 0;
}

int heart_byml_actor_ids(const uint8_t *d, size_t n, const char *gyaml, uint64_t *ids, int cap) {
    if (n < 16 || !((d[0] == 'Y' && d[1] == 'B') || (d[0] == 'B' && d[1] == 'Y'))) return -1;
    Byml b = {d, n, d[0] == 'B', 0, 0};
    b.keys = rd(d, n, 4, 4, b.be), b.strs = rd(d, n, 8, 4, b.be);
    size_t word;
    if (byml_get(&b, rd(d, n, 12, 4, b.be), "Actors", &word) != 0xc0) return -1;
    const size_t list = rd(d, n, word, 4, b.be);
    if (list >= n || d[list] != 0xc0) return -1;
    const uint32_t count = rd(d, n, list + 1, 3, b.be);
    const size_t types = list + 4, values = types + ((count + 3u) & ~3u);
    int found = 0;
    for (uint32_t i = 0; i < count && found < cap; i++) {
        if (types + i >= n || d[types + i] != 0xc1) continue;
        const size_t actor = rd(d, n, values + 4 * (size_t)i, 4, b.be);
        size_t w;
        if (byml_get(&b, actor, "Gyaml", &w) != 0xa0 || !byml_table_is(&b, b.strs, rd(d, n, w, 4, b.be), gyaml)) continue;
        const int t = byml_get(&b, actor, "Hash", &w);
        if (t == 0xd4 || t == 0xd5) ids[found++] = rd64(d, n, rd(d, n, w, 4, b.be), b.be);   /* (64-bit: the word is where) */
        else if (t == 0xd3) ids[found++] = rd(d, n, w, 4, b.be);
    }
    return found;
}

int heart_pieces_total(int max_life, int containers, int held) {
    if (max_life < 12 || max_life % 4 || containers < 0 || containers > HEART_CONTAINERS || held < 0 || held > 3) return -1;
    const int total = 4 * (max_life / 4 - 3 - containers) + held;
    return total >= 0 && total <= 99 ? total : -1;
}

#ifndef HEART_CONTAINERS_NO_GAME   /* (tests/test_discord_activity.py compiles the readers above alone) */
#include "flags.h"   /* (game_flag_bool64: whether a container was taken) */

void hos_log(const char *fmt, ...);

/* the eight dungeons with a boss, by their folder in romfs Banc */
static const char *const HEART_DUNGEONS[HEART_CONTAINERS] = {"InsideTheDekuTree", "DodongoCavern", "InsideJabuJabuBelly",
                                                             "ForestTemple", "FireTemple", "WaterTemple", "ShadowTemple",
                                                             "SpiritTemple"};
static uint64_t heart_ids[HEART_CONTAINERS];
static int heart_ready;   /* (1 once heart_ids holds all eight; published with release, read with acquire) */


uint8_t *hos_zs_read_romfs_named(const char *rel, size_t *n, size_t limit, const char *named);   /* zs.c: with the game's dictionary */

void hos_oot_heart_containers_load(void) {
    static int tried;
    if (tried || !hos_romfs_ready()) return;
    tried = 1;
    char path[1024];
    uint64_t ids[HEART_CONTAINERS];
    int got = 0;
    for (int i = 0; i < HEART_CONTAINERS; i++) {   /* (each map a frame made with bcett.byml.zsdic: zs.c finds it by its id, or by that name for a frame of none) */
        snprintf(path, sizeof path, "Banc/%s/%s_Static.bcett.byml.zs", HEART_DUNGEONS[i], HEART_DUNGEONS[i]);
        size_t map_n = 0;
        uint8_t *map = hos_zs_read_romfs_named(path, &map_n, 1u << 30, "bcett.byml.zsdic");
        uint64_t one;
        if (map && heart_byml_actor_ids(map, map_n, "Obj_Heart_Container", &one, 1) == 1) ids[got++] = one;
        free(map);
    }
    if (got == HEART_CONTAINERS) {
        memcpy(heart_ids, ids, sizeof ids);
        __atomic_store_n(&heart_ready, 1, __ATOMIC_RELEASE);
    }
    hos_log("discord: the boss heart containers: %d of %d found in the RomFS%s", got, HEART_CONTAINERS,
            got == HEART_CONTAINERS ? "" : " (no total of the pieces of heart)");
}

int hos_oot_heart_containers_taken(void) {
    static int at[HEART_CONTAINERS] = {-1, -1, -1, -1, -1, -1, -1, -1};   /* (where each flag was: the game's thread only) */
    if (!__atomic_load_n(&heart_ready, __ATOMIC_ACQUIRE)) return -1;
    int taken = 0;
    for (int i = 0; i < HEART_CONTAINERS; i++) {
        const int v = game_flag_bool64(game_flags_mgr(), heart_ids[i], &at[i]);
        if (v < 0) return -1;
        taken += v;
    }
    return taken;
}
#endif
