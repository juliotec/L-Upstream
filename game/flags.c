/* The game's flags (flags.h): GameDataMgr's typed containers, read from guest memory with every access bounded.

   The one reader of them: the Debug tab's tools and the map line (game/oot_tools.c), the control bridge's samples and
   their changes scan (game/oot_state_tap.c), the boss heart containers (game/oot_heart_containers.c) and the Voice
   Language option (game/oot_voice.c) all come here. Before, hos_main.c called the game's own getters (f_8022d9c0 for a
   Bool, f_80cd0ea0 for a String64) from Link's update, with a second copy of the layout for its writes, and
   oot_voice.c walked the Enum container a third way: f_8022d9c0 reads the Bool entries as this does (+24 entries, +16
   their count, +32 the table, +44 its size, the value at +22), and the String64 getter hands back the entry's text pointer
   (+0x68), which this reads. tests/test_game_flags.py checks the reads on a synthetic GameDataMgr. */
#include "game_host.h"   /* (what the host gives the game: host/game_host.h) */
#include "flags.h"
#include <math.h>
#include <string.h>

const GameFlagLayout game_flag_layout[GAME_FLAG_KINDS] = {
    {0x008, 0x18, 0x16}, {0x0c8, 0x28, 0x1c}, {0x188, 0x28, 0x1c}, {0x248, 0x38, 0x1c}, {0x608, 0x108, 0x68}, {0x788, 0x28, 0x1c}};

#define FLAG_TABLE_MAX (1u << 22)   /* (slots: a larger size is a header read mid-change, or not a table) */
#define FLAG_BOOL64 0xc58u

/* n guest bytes at a into dst: 0 for a null pointer's page or anything outside the guest window */
static int flag_peek(uint64_t a, void *dst, size_t n) {
    const void *p = a >= 0x100000 ? hos_guest_range(a, n) : NULL;
    if (!p) return 0;
    memcpy(dst, p, n);
    return 1;
}
static uint64_t flag_u64(uint64_t a) {
    uint64_t v = 0;
    return flag_peek(a, &v, 8) ? v : 0;
}
static uint32_t flag_u32(uint64_t a) {
    uint32_t v = 0;
    return flag_peek(a, &v, 4) ? v : 0;
}

uint32_t game_flag_hash(const char *s, size_t n) {
    uint32_t h = 0, k;
    size_t i = 0;
    for (; i + 4 <= n; i += 4) {
        k = (uint32_t)(unsigned char)s[i] | (uint32_t)(unsigned char)s[i + 1] << 8 | (uint32_t)(unsigned char)s[i + 2] << 16 |
            (uint32_t)(unsigned char)s[i + 3] << 24;
        k *= 0xcc9e2d51u, k = k << 15 | k >> 17, k *= 0x1b873593u;
        h ^= k, h = h << 13 | h >> 19, h = h * 5 + 0xe6546b64u;
    }
    k = 0;
    switch (n & 3) {
    case 3: k ^= (uint32_t)(unsigned char)s[i + 2] << 16; /* fall through */
    case 2: k ^= (uint32_t)(unsigned char)s[i + 1] << 8; /* fall through */
    case 1: k ^= (uint32_t)(unsigned char)s[i]; k *= 0xcc9e2d51u, k = k << 15 | k >> 17, k *= 0x1b873593u, h ^= k;
    }
    h ^= (uint32_t)n;
    h ^= h >> 16, h *= 0x85ebca6bu, h ^= h >> 13, h *= 0xc2b2ae35u, h ^= h >> 16;
    return h;
}

uint64_t game_flags_mgr(void) { return flag_u64(flag_u64(GAME_FLAGS_SLOT)); }

static uint64_t entry_of(uint64_t mgr, int kind, uint32_t hash, int past_count_first) {
    if (!mgr || kind < 0 || kind >= GAME_FLAG_KINDS) return 0;
    const uint64_t box = mgr + game_flag_layout[kind].at, entries = flag_u64(box + 16), table = flag_u64(box + 24);
    const uint32_t count = flag_u32(box + 8), size = flag_u32(box + 36);
    if (!entries || !table || !size || size > FLAG_TABLE_MAX || !hash) return 0;
    for (uint32_t probe = 0, slot = hash % size; probe < size; probe++, slot = slot + 1 == size ? 0 : slot + 1) {
        uint32_t pair[2];
        if (!flag_peek(table + slot * 8ULL, pair, sizeof pair) || !pair[0]) return 0;
        if (pair[0] != hash) continue;
        if (pair[1] < count) return entries + (uint64_t)pair[1] * game_flag_layout[kind].stride;
        return past_count_first && count ? entries : 0;
    }
    return 0;
}

uint64_t game_flag_entry(uint64_t mgr, int kind, uint32_t hash) { return entry_of(mgr, kind, hash, 0); }

static int number_at(uint64_t e, int kind, int64_t *v) {
    if (!e) return 0;
    const uint64_t at = e + game_flag_layout[kind].value;
    uint8_t b;
    uint32_t u;
    if (kind == GAME_FLAG_BOOL) return flag_peek(at, &b, 1) && (*v = b, 1);
    if (!flag_peek(at, &u, 4)) return 0;
    *v = kind == GAME_FLAG_INT ? (int64_t)(int32_t)u : (int64_t)u;
    return 1;
}

int game_flag_number(uint64_t mgr, int kind, uint32_t hash, int64_t *v) {
    if (kind == GAME_FLAG_FLOAT || kind == GAME_FLAG_TEXT) return 0;
    return number_at(entry_of(mgr, kind, hash, 0), kind, v);
}

int game_flag_number_as_game(uint64_t mgr, int kind, uint32_t hash, int64_t *v) {
    if (kind == GAME_FLAG_FLOAT || kind == GAME_FLAG_TEXT) return 0;
    return number_at(entry_of(mgr, kind, hash, 1), kind, v);
}

int game_flag_bool(uint64_t mgr, uint32_t hash, int *value) {
    int64_t v;
    if (!game_flag_number(mgr, GAME_FLAG_BOOL, hash, &v)) return 0;
    *value = (int)(v & 1);
    return 1;
}

int game_flag_float(uint64_t mgr, uint32_t hash, float *f) {
    const uint64_t e = game_flag_entry(mgr, GAME_FLAG_FLOAT, hash);
    return e && flag_peek(e + game_flag_layout[GAME_FLAG_FLOAT].value, f, 4) && isfinite(*f);
}

int game_flag_text(uint64_t mgr, uint32_t hash, char *buf, size_t size) {
    if (!size) return 0;
    buf[0] = 0;
    const uint64_t e = game_flag_entry(mgr, GAME_FLAG_TEXT, hash), p = e ? flag_u64(e + game_flag_layout[GAME_FLAG_TEXT].value) : 0;
    const size_t n = size < 64 ? size : 64;
    if (!p || !flag_peek(p, buf, n)) return buf[0] = 0, 0;
    buf[n - 1] = 0;
    return 1;
}

int game_flag_set_bool(uint64_t mgr, uint32_t hash, int value) {
    const uint64_t e = game_flag_entry(mgr, GAME_FLAG_BOOL, hash);
    uint8_t *const p = e ? (uint8_t *)hos_guest_range(e + game_flag_layout[GAME_FLAG_BOOL].value, 1) : NULL;
    if (!p) return 0;
    *p = value ? 1 : 0;
    HOS_WRITTEN(p, 1);
    return 1;
}

int game_flag_bool64(uint64_t mgr, uint64_t id, int *at) {
    const uint32_t count = mgr ? flag_u32(mgr + FLAG_BOOL64) : 0;
    const uint64_t entries = mgr ? flag_u64(mgr + FLAG_BOOL64 + 8) : 0;
    if (!count || count > (1u << 20) || entries < 0x100000) return -1;
    const uint8_t *e = (const uint8_t *)hos_guest_range(entries, (uint64_t)count * 16);
    if (!e) return -1;
    uint64_t k;
    if (*at >= 0 && (uint32_t)*at < count && (memcpy(&k, e + (size_t)*at * 16, 8), k == id)) return e[(size_t)*at * 16 + 12] != 0;
    for (uint32_t i = 0; i < count; i++) {
        memcpy(&k, e + (size_t)i * 16, 8);
        if (k == id) {
            *at = (int)i;
            return e[(size_t)i * 16 + 12] != 0;
        }
    }
    return -1;
}

int game_flag_table(uint64_t mgr, int kind, GameFlagTable *h) {
    if (!mgr || kind < 0 || kind >= GAME_FLAG_KINDS) return 0;
    const uint64_t box = mgr + game_flag_layout[kind].at;
    if (!flag_peek(box + 16, &h->entries, 8) || !flag_peek(box + 24, &h->table, 8) ||
        !flag_peek(box + 8, &h->count, 4) || !flag_peek(box + 36, &h->size, 4)) return 0;
    return h->entries >= 0x100000 && h->table >= 0x100000 && h->size && h->size <= FLAG_TABLE_MAX &&
        h->count <= h->size && hos_guest_range(h->table, (uint64_t)h->size * 8) &&
        (!h->count || hos_guest_range(h->entries, (uint64_t)h->count * game_flag_layout[kind].stride));
}

int game_flag_slot(const GameFlagTable *h, int kind, uint32_t slot, uint32_t *key, uint32_t *value) {
    uint32_t pair[2], again[2], v = 0;
    uint8_t b;
    *key = *value = 0;
    if (kind < 0 || kind >= GAME_FLAG_KINDS || slot >= h->size || !flag_peek(h->table + slot * 8ULL, pair, sizeof pair)) return -1;
    if (!pair[0]) return 0;
    if (pair[1] >= h->count) return -1;
    const uint64_t at = h->entries + (uint64_t)pair[1] * game_flag_layout[kind].stride + game_flag_layout[kind].value;
    if (kind == GAME_FLAG_BOOL) {
        if (!flag_peek(at, &b, 1)) return -1;
        v = b;
    } else if (!flag_peek(at, &v, 4)) return -1;
    /* (the pair again: a slot rewritten while it was read names another entry's value) */
    if (!flag_peek(h->table + slot * 8ULL, again, sizeof again) || memcmp(pair, again, sizeof pair)) return -1;
    *key = pair[0], *value = v;
    return 1;
}
